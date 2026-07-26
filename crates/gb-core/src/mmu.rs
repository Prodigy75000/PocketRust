//! The memory bus.
//!
//! Everything the CPU touches goes through here: it decodes the 16-bit address
//! into the right device (cartridge, VRAM, WRAM, OAM, IO registers, HRAM) and
//! it aggregates interrupt requests from the sub-devices into the IF register.

use crate::apu::Apu;
use crate::cartridge::Cartridge;
use crate::sgb::Sgb;
use crate::save::Cursor;
use crate::joypad::{Button, Joypad};
use crate::ppu::Ppu;
use crate::serial::Serial;
use crate::timer::Timer;

// Interrupt bit positions, shared by IF (0xFF0F) and IE (0xFFFF).
pub const INT_VBLANK: u8 = 1 << 0;
pub const INT_STAT: u8 = 1 << 1;
pub const INT_TIMER: u8 = 1 << 2;
pub const INT_SERIAL: u8 = 1 << 3;
pub const INT_JOYPAD: u8 = 1 << 4;

pub struct Mmu {
    pub cartridge: Cartridge,
    pub ppu: Ppu,
    pub apu: Apu,
    pub timer: Timer,
    pub joypad: Joypad,

    pub cgb: bool,
    wram: [u8; 0x8000], // 8 banks of 4 KiB
    wram_bank: usize,   // 1..7 selects the 0xD000 region
    hram: [u8; 0x7F],

    /// Interrupt Flag (0xFF0F): a pending interrupt request.
    pub interrupt_flag: u8,
    /// Interrupt Enable (0xFFFF): which interrupts are allowed to fire.
    pub interrupt_enable: u8,

    // CGB double-speed (KEY1).
    pub double_speed: bool,
    pub key1_prepare: bool,

    // CGB HDMA/GDMA (0xFF51..=0xFF55).
    hdma_src: u16,
    hdma_dst: u16,
    hdma_len: u16,    // remaining bytes / 0x10 - 1, 0xFF when idle
    hdma_active: bool, // HBlank-driven transfer in progress

    /// The serial port (link cable). With no peer it captures sent bytes, which
    /// is how Blargg's test ROMs report results.
    pub serial: Serial,

    /// Super Game Boy command capture / palette state.
    pub sgb: Sgb,
}

impl Mmu {
    pub fn new(cartridge: Cartridge, cgb: bool) -> Mmu {
        let sgb = Sgb::new(cartridge.header.sgb_flag);
        Mmu {
            sgb,
            cartridge,
            ppu: Ppu::new(cgb),
            apu: Apu::new(),
            timer: Timer::new(),
            joypad: Joypad::new(),
            cgb,
            wram: [0; 0x8000],
            wram_bank: 1,
            hram: [0; 0x7F],
            interrupt_flag: 0xE1,
            interrupt_enable: 0x00,
            double_speed: false,
            key1_prepare: false,
            hdma_src: 0,
            hdma_dst: 0,
            hdma_len: 0xFF,
            hdma_active: false,
            serial: Serial::new(),
        }
    }

    pub fn set_button(&mut self, button: Button, down: bool) {
        self.joypad.set_button(button, down);
    }

    /// Transfer all persistent bus state for save/load. Joypad and serial log
    /// are omitted: the joypad is re-driven by the frontend each frame.
    pub(crate) fn transfer<C: Cursor>(&mut self, c: &mut C) {
        self.cartridge.transfer(c);
        self.ppu.transfer(c);
        self.apu.transfer(c);
        self.timer.transfer(c);
        self.serial.transfer(c);
        c.bytes(&mut self.wram);
        c.usize(&mut self.wram_bank);
        c.bytes(&mut self.hram);
        c.u8(&mut self.interrupt_flag);
        c.u8(&mut self.interrupt_enable);
        c.bool(&mut self.double_speed);
        c.bool(&mut self.key1_prepare);
        c.u16(&mut self.hdma_src);
        c.u16(&mut self.hdma_dst);
        c.u16(&mut self.hdma_len);
        c.bool(&mut self.hdma_active);
    }

    /// Advance all timed sub-devices, then fold their interrupt lines into IF.
    pub fn step(&mut self, cycles: u32) {
        // The timer/DIV run at the CPU clock (which doubles in CGB fast mode),
        // while the PPU and APU are tied to wall-clock, so they advance at half
        // the CPU cycle count when double speed is engaged.
        self.timer.step(cycles);
        // The serial clock, like the timer, tracks the CPU clock.
        self.serial.step(cycles);
        if self.serial.interrupt {
            self.interrupt_flag |= INT_SERIAL;
            self.serial.interrupt = false;
        }
        let av_cycles = cycles >> (self.double_speed as u32);
        let entered_hblank = self.ppu.step(av_cycles);
        self.apu.step(av_cycles);
        if entered_hblank && self.hdma_active {
            self.hdma_hblank_transfer();
        }

        if self.timer.interrupt {
            self.interrupt_flag |= INT_TIMER;
            self.timer.interrupt = false;
        }
        if self.ppu.vblank_interrupt {
            self.interrupt_flag |= INT_VBLANK;
            self.ppu.vblank_interrupt = false;
        }
        if self.ppu.stat_interrupt {
            self.interrupt_flag |= INT_STAT;
            self.ppu.stat_interrupt = false;
        }
        if self.joypad.interrupt {
            self.interrupt_flag |= INT_JOYPAD;
            self.joypad.interrupt = false;
        }
    }

    pub fn read(&self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x7FFF => self.cartridge.read_rom(addr),
            0x8000..=0x9FFF => self.ppu.read_vram(addr),
            0xA000..=0xBFFF => self.cartridge.read_ram(addr),
            0xC000..=0xDFFF => self.wram[self.wram_offset(addr)],
            0xE000..=0xFDFF => self.wram[self.wram_offset(addr - 0x2000)], // echo RAM
            0xFE00..=0xFE9F => self.ppu.read_oam(addr),
            0xFEA0..=0xFEFF => 0xFF, // unusable
            0xFF00..=0xFF7F => self.read_io(addr),
            0xFF80..=0xFFFE => self.hram[(addr - 0xFF80) as usize],
            0xFFFF => self.interrupt_enable,
        }
    }

    pub fn write(&mut self, addr: u16, val: u8) {
        match addr {
            0x0000..=0x7FFF => self.cartridge.write_rom(addr, val),
            0x8000..=0x9FFF => self.ppu.write_vram(addr, val),
            0xA000..=0xBFFF => self.cartridge.write_ram(addr, val),
            0xC000..=0xDFFF => self.wram[self.wram_offset(addr)] = val,
            0xE000..=0xFDFF => {
                let off = self.wram_offset(addr - 0x2000);
                self.wram[off] = val;
            }
            0xFE00..=0xFE9F => self.ppu.write_oam(addr, val),
            0xFEA0..=0xFEFF => {} // unusable
            0xFF00..=0xFF7F => self.write_io(addr, val),
            0xFF80..=0xFFFE => self.hram[(addr - 0xFF80) as usize] = val,
            0xFFFF => self.interrupt_enable = val,
        }
    }

    fn read_io(&self, addr: u16) -> u8 {
        match addr {
            0xFF00 => {
                let v = self.joypad.read();
                // In SGB multiplayer, a both-rows-deselected read reports the
                // active controller's ID in the low nibble instead of $F.
                if v & 0x30 == 0x30 {
                    (v & 0xF0) | self.sgb.player_id_nibble()
                } else {
                    v
                }
            }
            0xFF01 => self.serial.read_data(),
            0xFF02 => self.serial.read_control(),
            0xFF04 => self.timer.div(),
            0xFF05 => self.timer.tima,
            0xFF06 => self.timer.tma,
            0xFF07 => self.timer.tac | 0xF8,
            0xFF0F => self.interrupt_flag | 0xE0,
            0xFF10..=0xFF3F => self.apu.read(addr),
            0xFF40 => self.ppu.lcdc,
            0xFF41 => self.ppu.read_stat(),
            0xFF42 => self.ppu.scy,
            0xFF43 => self.ppu.scx,
            0xFF44 => self.ppu.ly,
            0xFF45 => self.ppu.lyc,
            0xFF47 => self.ppu.bgp,
            0xFF48 => self.ppu.obp0,
            0xFF49 => self.ppu.obp1,
            0xFF4A => self.ppu.wy,
            0xFF4B => self.ppu.wx,
            // --- CGB-only registers ---
            0xFF4D if self.cgb => {
                (self.double_speed as u8) << 7 | (self.key1_prepare as u8) | 0x7E
            }
            0xFF4F if self.cgb => self.ppu.vram_bank_reg(),
            0xFF55 if self.cgb => {
                if self.hdma_active {
                    self.hdma_len as u8
                } else {
                    0xFF
                }
            }
            0xFF68 if self.cgb => self.ppu.read_bg_pal_index(),
            0xFF69 if self.cgb => self.ppu.read_bg_pal_data(),
            0xFF6A if self.cgb => self.ppu.read_obj_pal_index(),
            0xFF6B if self.cgb => self.ppu.read_obj_pal_data(),
            0xFF70 if self.cgb => self.wram_bank as u8 | 0xF8,
            _ => 0xFF, // sound and unimplemented registers
        }
    }

    fn write_io(&mut self, addr: u16, val: u8) {
        match addr {
            0xFF00 => {
                self.joypad.write(val);
                self.sgb.write_p1(val);
                // An SGB PAL command supplies the game's own colors (which win
                // over the frontend colorize option); or signals we should stop
                // overriding for a cart whose palettes we cannot follow.
                if let Some(pal) = self.sgb.take_palette_override() {
                    self.ppu.set_sgb_palette(pal);
                }
                // Hide SGB VRAM-transfer garbage and honor MASK_EN.
                if self.sgb.take_transfer() {
                    self.ppu.sgb_begin_transfer();
                }
                if let Some(mask) = self.sgb.take_mask() {
                    self.ppu.set_sgb_mask(mask);
                }
            }
            0xFF01 => self.serial.write_data(val),
            0xFF02 => self.serial.write_control(val),
            0xFF04 => {
                self.timer.reset_div();
                self.apu.on_div_reset();
            }
            0xFF05 => self.timer.tima = val,
            0xFF06 => self.timer.tma = val,
            0xFF07 => self.timer.tac = val,
            0xFF0F => self.interrupt_flag = val,
            0xFF10..=0xFF3F => self.apu.write(addr, val),
            0xFF40 => self.ppu.write_lcdc(val),
            0xFF41 => self.ppu.write_stat(val),
            0xFF42 => self.ppu.scy = val,
            0xFF43 => self.ppu.scx = val,
            0xFF44 => {} // LY is read-only
            0xFF45 => self.ppu.lyc = val,
            0xFF46 => self.oam_dma(val),
            0xFF47 => self.ppu.bgp = val,
            0xFF48 => self.ppu.obp0 = val,
            0xFF49 => self.ppu.obp1 = val,
            0xFF4A => self.ppu.wy = val,
            0xFF4B => self.ppu.wx = val,
            // --- CGB-only registers ---
            0xFF4D if self.cgb => self.key1_prepare = val & 1 != 0,
            0xFF4F if self.cgb => self.ppu.set_vram_bank(val),
            0xFF51 if self.cgb => self.hdma_src = (self.hdma_src & 0xFF) | ((val as u16) << 8),
            0xFF52 if self.cgb => self.hdma_src = (self.hdma_src & 0xFF00) | (val as u16 & 0xF0),
            0xFF53 if self.cgb => {
                self.hdma_dst = 0x8000 | ((val as u16 & 0x1F) << 8) | (self.hdma_dst & 0xF0)
            }
            0xFF54 if self.cgb => self.hdma_dst = (self.hdma_dst & 0xFF00) | (val as u16 & 0xF0),
            0xFF55 if self.cgb => self.write_hdma5(val),
            0xFF68 if self.cgb => self.ppu.write_bg_pal_index(val),
            0xFF69 if self.cgb => self.ppu.write_bg_pal_data(val),
            0xFF6A if self.cgb => self.ppu.write_obj_pal_index(val),
            0xFF6B if self.cgb => self.ppu.write_obj_pal_data(val),
            0xFF70 if self.cgb => self.wram_bank = (val as usize & 7).max(1),
            _ => {} // sound and unimplemented registers
        }
    }

    /// OAM DMA: copy 0xN00..0xN9F into OAM. Real hardware takes 160 machine
    /// cycles during which the CPU can only touch HRAM; we do it instantly,
    /// which is fine for essentially all games.
    fn oam_dma(&mut self, page: u8) {
        let base = (page as u16) << 8;
        for i in 0..0xA0u16 {
            let byte = self.read(base + i);
            self.ppu.write_oam(0xFE00 + i, byte);
        }
    }

    /// Map a 0xC000..=0xDFFF address to the banked WRAM array.
    fn wram_offset(&self, addr: u16) -> usize {
        if addr < 0xD000 {
            (addr - 0xC000) as usize
        } else {
            self.wram_bank * 0x1000 + (addr - 0xD000) as usize
        }
    }

    /// Write a byte to VRAM at a masked destination (HDMA stays inside VRAM).
    fn hdma_write(&mut self, dst: u16, byte: u8) {
        self.ppu.write_vram(0x8000 | (dst & 0x1FFF), byte);
    }

    /// Copy one 16-byte block during HBlank (CGB HDMA mode 1).
    fn hdma_hblank_transfer(&mut self) {
        for i in 0..0x10u16 {
            let b = self.read(self.hdma_src.wrapping_add(i));
            self.hdma_write(self.hdma_dst.wrapping_add(i), b);
        }
        self.hdma_src = self.hdma_src.wrapping_add(0x10);
        self.hdma_dst = self.hdma_dst.wrapping_add(0x10);
        if self.hdma_len == 0 {
            self.hdma_active = false;
            self.hdma_len = 0xFF;
        } else {
            self.hdma_len -= 1;
        }
    }

    /// Handle a write to HDMA5: start an HBlank transfer or run a general one now.
    fn write_hdma5(&mut self, val: u8) {
        if val & 0x80 != 0 {
            // HBlank DMA: one block per HBlank.
            self.hdma_len = (val & 0x7F) as u16;
            self.hdma_active = true;
        } else if self.hdma_active {
            // Writing bit 7 = 0 while active cancels the transfer.
            self.hdma_active = false;
            self.hdma_len = 0xFF;
        } else {
            // General-purpose DMA: copy everything immediately.
            let blocks = (val & 0x7F) as u16 + 1;
            for i in 0..blocks * 0x10 {
                let b = self.read(self.hdma_src.wrapping_add(i));
                self.hdma_write(self.hdma_dst.wrapping_add(i), b);
            }
            self.hdma_src = self.hdma_src.wrapping_add(blocks * 0x10);
            self.hdma_dst = self.hdma_dst.wrapping_add(blocks * 0x10);
            self.hdma_len = 0xFF;
        }
    }
}
