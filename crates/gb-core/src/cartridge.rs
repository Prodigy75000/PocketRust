//! Cartridge loading and Memory Bank Controller (MBC) emulation.
//!
//! A Game Boy cartridge is ROM plus (optionally) a bank controller chip and
//! battery-backed RAM. The MBC intercepts writes to the ROM address space and
//! interprets them as bank-switching commands. We support the two most common
//! cases: no MBC (32 KiB flat ROM) and MBC1.

/// The cartridge header lives at 0x0100..=0x014F. We only pull out the fields
/// we actually need to configure the mapper.
#[derive(Debug, Clone)]
pub struct Header {
    pub title: String,
    pub mbc_kind: MbcKind,
    pub rom_banks: usize,
    pub ram_banks: usize,
    pub has_battery: bool,
    /// MBC3 carts with the on-board timer chip (cart types 0x0F / 0x10) carry a
    /// real-time clock. Only these expose the RTC registers.
    pub has_rtc: bool,
    pub cgb_flag: u8,
    /// SGB support flag (0x146): 0x03 means the cart carries SGB commands.
    /// Parsed but currently unused: SGB detection is disabled (see `Mmu::new`),
    /// so mono SGB carts run as plain DMG with our GBC-auto colorization.
    #[allow(dead_code)]
    pub sgb_flag: u8,
    /// Sum of the title bytes (0x134..=0x143); the CGB boot ROM uses this to
    /// pick a colorization palette, and we reuse it for `Colorize::Auto`.
    pub title_checksum: u8,
    /// The 4th title byte (0x137), used to disambiguate colliding checksums.
    pub title_fourth: u8,
    /// Whether the game is Nintendo-published (only those get boot-ROM palettes).
    pub nintendo_licensed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MbcKind {
    None,
    Mbc1,
    Mbc2,
    Mbc3,
    Mbc5,
    Unsupported(u8),
}

impl Header {
    fn parse(rom: &[u8]) -> Header {
        let title = rom[0x0134..=0x0143]
            .iter()
            .take_while(|&&b| b != 0)
            .map(|&b| b as char)
            .collect::<String>();

        let cart_type = rom[0x0147];
        let (mbc_kind, has_battery) = match cart_type {
            0x00 => (MbcKind::None, false),
            0x01 => (MbcKind::Mbc1, false),
            0x02 => (MbcKind::Mbc1, false),
            0x03 => (MbcKind::Mbc1, true),
            0x05 => (MbcKind::Mbc2, false),
            0x06 => (MbcKind::Mbc2, true),
            0x0F..=0x13 => (MbcKind::Mbc3, matches!(cart_type, 0x0F | 0x10 | 0x13)),
            0x19..=0x1E => (MbcKind::Mbc5, matches!(cart_type, 0x1B | 0x1E)),
            other => (MbcKind::Unsupported(other), false),
        };
        // Cart types 0x0F (MBC3+TIMER+BATTERY) and 0x10 (MBC3+TIMER+RAM+BATTERY)
        // are the only ones with the RTC crystal.
        let has_rtc = matches!(cart_type, 0x0F | 0x10);

        // ROM size: 32 KiB << N gives the total size, i.e. 2 << N banks of 16 KiB.
        let rom_banks = 2usize << rom[0x0148];

        // RAM size code -> number of 8 KiB banks.
        let ram_banks = match rom[0x0149] {
            0x00 => 0,
            0x01 => 1, // 2 KiB (a partial bank); we round up to one bank
            0x02 => 1,
            0x03 => 4,
            0x04 => 16,
            0x05 => 8,
            _ => 0,
        };

        Header {
            title,
            mbc_kind,
            rom_banks,
            ram_banks,
            has_battery,
            has_rtc,
            cgb_flag: rom[0x0143],
            sgb_flag: rom[0x0146],
            title_checksum: rom[0x0134..=0x0143]
                .iter()
                .fold(0u8, |acc, &b| acc.wrapping_add(b)),
            title_fourth: rom[0x0137],
            // The boot ROM only assigns palettes to Nintendo-published games:
            // old licensee 0x33 -> new licensee (0x144/0x145) must be "01",
            // otherwise old licensee must be 0x01.
            nintendo_licensed: if rom[0x014B] == 0x33 {
                rom[0x0144] == b'0' && rom[0x0145] == b'1'
            } else {
                rom[0x014B] == 0x01
            },
        }
    }
}

pub struct Cartridge {
    pub header: Header,
    rom: Vec<u8>,
    ram: Vec<u8>,
    mbc: Mbc,
}

/// Per-mapper mutable state.
enum Mbc {
    None,
    Mbc1 {
        ram_enabled: bool,
        rom_bank: u8, // low 5 bits
        ram_bank: u8, // 2 bits: RAM bank OR upper ROM bank bits
        /// false = ROM banking mode (0), true = RAM banking mode (1).
        banking_mode: bool,
    },
    Mbc2 {
        ram_enabled: bool,
        rom_bank: u8, // 4 bits (1..15)
    },
    Mbc3 {
        ram_enabled: bool,
        rom_bank: u8,  // 7 bits
        ram_bank: u8,  // 0x00-0x03 select a RAM bank; 0x08-0x0C select an RTC register
        has_rtc: bool, // whether this cart has the timer chip
        rtc: Rtc,
    },
    Mbc5 {
        ram_enabled: bool,
        rom_bank: u16, // 9 bits
        ram_bank: u8,  // 4 bits
    },
}

/// Wall-clock T-cycles per real second. The RTC crystal runs at real time, so
/// this is the DMG clock rate and is *not* affected by CGB double-speed (the MMU
/// feeds the RTC the wall-clock-rate cycle count).
const CYCLES_PER_SECOND: u32 = 4_194_304;

/// Bytes appended to the battery RAM to persist the RTC across power cycles.
/// Layout: b"PRTC" + version + S + M + H + days(u16 LE) + flags + sub(u32 LE).
const RTC_FOOTER_LEN: usize = 15;
const RTC_FOOTER_MAGIC: &[u8; 4] = b"PRTC";

/// The MBC3 real-time clock.
///
/// Deterministic by design: it advances from emulated cycles, never the host
/// wall clock, so two networked peers (or a replay) see byte-identical clock
/// state — a hard requirement for this core's netplay / save-state guarantees.
/// The consequence is that real time elapsed while the machine is powered off is
/// not counted; the clock resumes from where it was saved.
///
/// The chip keeps a live counter plus a *latched* snapshot. The game latches
/// (write 0x00 then 0x01 to 0x6000-0x7FFF) to freeze the current time into the
/// snapshot, then reads the snapshot back through the 0xA000 window.
struct Rtc {
    seconds: u8, // 0..59
    minutes: u8, // 0..59
    hours: u8,   // 0..23
    days: u16,   // 9-bit day counter (0..511)
    halted: bool,
    day_carry: bool,
    /// Sub-second accumulator, in wall-clock T-cycles.
    sub: u32,
    /// Latched register snapshot the game reads: [S, M, H, day-low, day-high].
    latch: [u8; 5],
    /// Last value written to the latch register, to detect the 0->1 sequence.
    latch_last: u8,
}

impl Rtc {
    fn new() -> Rtc {
        Rtc {
            seconds: 0,
            minutes: 0,
            hours: 0,
            days: 0,
            halted: false,
            day_carry: false,
            sub: 0,
            latch: [0; 5],
            latch_last: 0xFF,
        }
    }

    /// Advance by `cycles` wall-clock T-cycles. Returns true if at least one whole
    /// second elapsed (so the battery footer needs refreshing).
    fn tick(&mut self, cycles: u32) -> bool {
        if self.halted {
            return false;
        }
        self.sub += cycles;
        let mut advanced = false;
        while self.sub >= CYCLES_PER_SECOND {
            self.sub -= CYCLES_PER_SECOND;
            self.advance_second();
            advanced = true;
        }
        advanced
    }

    fn advance_second(&mut self) {
        self.seconds += 1;
        if self.seconds < 60 {
            return;
        }
        self.seconds = 0;
        self.minutes += 1;
        if self.minutes < 60 {
            return;
        }
        self.minutes = 0;
        self.hours += 1;
        if self.hours < 24 {
            return;
        }
        self.hours = 0;
        self.days += 1;
        if self.days >= 512 {
            self.days = 0;
            self.day_carry = true; // sticky until the game clears bit 7 of DH
        }
    }

    /// Build the five live register bytes (S, M, H, DL, DH).
    fn live_regs(&self) -> [u8; 5] {
        let dh = ((self.day_carry as u8) << 7)
            | ((self.halted as u8) << 6)
            | ((self.days >> 8) as u8 & 0x01);
        [
            self.seconds,
            self.minutes,
            self.hours,
            (self.days & 0xFF) as u8,
            dh,
        ]
    }

    /// Handle a write to the latch register (0x6000-0x7FFF): a 0x00 then 0x01
    /// sequence copies the live time into the latched snapshot.
    fn write_latch(&mut self, val: u8) {
        if self.latch_last == 0 && val == 1 {
            self.latch = self.live_regs();
        }
        self.latch_last = val;
    }

    /// Read a latched RTC register by index (0=S, 1=M, 2=H, 3=DL, 4=DH). Unused
    /// bits read back as 0.
    fn read_reg(&self, index: u8) -> u8 {
        match index {
            0 => self.latch[0] & 0x3F,
            1 => self.latch[1] & 0x3F,
            2 => self.latch[2] & 0x1F,
            3 => self.latch[3],
            4 => self.latch[4] & 0xC1,
            _ => 0xFF,
        }
    }

    /// Write a live RTC register by index. Writing seconds also resets the
    /// sub-second prescaler, as on hardware.
    fn write_reg(&mut self, index: u8, val: u8) {
        match index {
            0 => {
                self.seconds = val & 0x3F;
                self.sub = 0;
            }
            1 => self.minutes = val & 0x3F,
            2 => self.hours = val & 0x1F,
            3 => self.days = (self.days & 0x100) | val as u16,
            4 => {
                self.days = (self.days & 0xFF) | (((val & 0x01) as u16) << 8);
                self.halted = val & 0x40 != 0;
                self.day_carry = val & 0x80 != 0;
            }
            _ => {}
        }
    }

    /// Encode the live time into the battery footer (see [`RTC_FOOTER_LEN`]).
    fn encode_footer(&self) -> [u8; RTC_FOOTER_LEN] {
        let mut f = [0u8; RTC_FOOTER_LEN];
        f[0..4].copy_from_slice(RTC_FOOTER_MAGIC);
        f[4] = 1; // version
        f[5] = self.seconds;
        f[6] = self.minutes;
        f[7] = self.hours;
        f[8..10].copy_from_slice(&self.days.to_le_bytes());
        f[10] = (self.halted as u8) | ((self.day_carry as u8) << 1);
        f[11..15].copy_from_slice(&self.sub.to_le_bytes());
        f
    }

    /// Restore the live time from a battery footer, if it is present and valid.
    /// The latched snapshot is reset to the restored live time.
    fn decode_footer(&mut self, f: &[u8]) {
        if f.len() < RTC_FOOTER_LEN || &f[0..4] != RTC_FOOTER_MAGIC || f[4] != 1 {
            return; // no footer (e.g. an older .srm) — keep the power-on clock
        }
        self.seconds = f[5].min(59);
        self.minutes = f[6].min(59);
        self.hours = f[7].min(23);
        self.days = u16::from_le_bytes([f[8], f[9]]) & 0x1FF;
        self.halted = f[10] & 0x01 != 0;
        self.day_carry = f[10] & 0x02 != 0;
        self.sub = u32::from_le_bytes([f[11], f[12], f[13], f[14]]) % CYCLES_PER_SECOND;
        self.latch = self.live_regs();
    }

    fn transfer<C: crate::save::Cursor>(&mut self, c: &mut C) {
        c.u8(&mut self.seconds);
        c.u8(&mut self.minutes);
        c.u8(&mut self.hours);
        c.u16(&mut self.days);
        c.bool(&mut self.halted);
        c.bool(&mut self.day_carry);
        c.u32(&mut self.sub);
        c.bytes(&mut self.latch);
        c.u8(&mut self.latch_last);
    }
}

impl Cartridge {
    pub fn new(rom: Vec<u8>) -> Cartridge {
        let header = Header::parse(&rom);
        // MBC2 has 512 x 4-bit of built-in RAM (no external banks); everything
        // else uses the header-declared bank count.
        let ram = if header.mbc_kind == MbcKind::Mbc2 {
            vec![0u8; 512]
        } else {
            // RTC carts append a footer past the game-visible RAM so the clock
            // rides along in the battery save. The MBC never maps into it.
            let footer = if header.has_rtc { RTC_FOOTER_LEN } else { 0 };
            vec![0u8; header.ram_banks.max(1) * 0x2000 + footer]
        };
        let mbc = match header.mbc_kind {
            MbcKind::None => Mbc::None,
            MbcKind::Mbc1 => Mbc::Mbc1 {
                ram_enabled: false,
                rom_bank: 1,
                ram_bank: 0,
                banking_mode: false,
            },
            MbcKind::Mbc2 => Mbc::Mbc2 {
                ram_enabled: false,
                rom_bank: 1,
            },
            MbcKind::Mbc3 => Mbc::Mbc3 {
                ram_enabled: false,
                rom_bank: 1,
                ram_bank: 0,
                has_rtc: header.has_rtc,
                rtc: Rtc::new(),
            },
            MbcKind::Mbc5 => Mbc::Mbc5 {
                ram_enabled: false,
                rom_bank: 1,
                ram_bank: 0,
            },
            MbcKind::Unsupported(_) => Mbc::None,
        };
        Cartridge {
            header,
            rom,
            ram,
            mbc,
        }
    }

    /// Read from the ROM region (0x0000..=0x7FFF).
    pub fn read_rom(&self, addr: u16) -> u8 {
        match &self.mbc {
            Mbc::None => *self.rom.get(addr as usize).unwrap_or(&0xFF),
            Mbc::Mbc1 {
                rom_bank,
                ram_bank,
                banking_mode,
                ..
            } => {
                let bank = if addr < 0x4000 {
                    // Bank 0 region. In RAM-banking mode the upper bits can remap
                    // this to bank 0x20/0x40/0x60 on large carts.
                    if *banking_mode {
                        ((*ram_bank as usize) << 5) & (self.header.rom_banks - 1)
                    } else {
                        0
                    }
                } else {
                    // Switchable region. Combine the 5-bit low bank with the 2-bit high.
                    let low = (*rom_bank as usize) & 0x1F;
                    let low = if low == 0 { 1 } else { low }; // bank 0 not selectable here
                    let hi = (*ram_bank as usize) << 5;
                    (hi | low) & (self.header.rom_banks - 1)
                };
                let offset = bank * 0x4000 + (addr as usize & 0x3FFF);
                *self.rom.get(offset).unwrap_or(&0xFF)
            }
            Mbc::Mbc2 { rom_bank, .. } => {
                let bank = if addr < 0x4000 {
                    0
                } else {
                    ((*rom_bank as usize).max(1)) & (self.header.rom_banks - 1)
                };
                let offset = bank * 0x4000 + (addr as usize & 0x3FFF);
                *self.rom.get(offset).unwrap_or(&0xFF)
            }
            Mbc::Mbc3 { rom_bank, .. } => {
                let bank = if addr < 0x4000 {
                    0
                } else {
                    ((*rom_bank as usize).max(1)) & (self.header.rom_banks - 1)
                };
                let offset = bank * 0x4000 + (addr as usize & 0x3FFF);
                *self.rom.get(offset).unwrap_or(&0xFF)
            }
            Mbc::Mbc5 { rom_bank, .. } => {
                // MBC5 can select bank 0 into the switchable region.
                let bank = if addr < 0x4000 {
                    0
                } else {
                    (*rom_bank as usize) & (self.header.rom_banks - 1)
                };
                let offset = bank * 0x4000 + (addr as usize & 0x3FFF);
                *self.rom.get(offset).unwrap_or(&0xFF)
            }
        }
    }

    /// Write to the ROM region: interpreted as an MBC control write.
    pub fn write_rom(&mut self, addr: u16, val: u8) {
        match &mut self.mbc {
            Mbc::None => {}
            Mbc::Mbc1 {
                ram_enabled,
                rom_bank,
                ram_bank,
                banking_mode,
            } => match addr {
                0x0000..=0x1FFF => *ram_enabled = (val & 0x0F) == 0x0A,
                0x2000..=0x3FFF => *rom_bank = val & 0x1F,
                0x4000..=0x5FFF => *ram_bank = val & 0x03,
                0x6000..=0x7FFF => *banking_mode = (val & 0x01) != 0,
                _ => {}
            },
            Mbc::Mbc2 {
                ram_enabled,
                rom_bank,
            } => {
                // One shared register in 0x0000..=0x3FFF: address bit 8 picks
                // which. Clear -> RAM enable; set -> ROM bank (4 bits, min 1).
                if addr < 0x4000 {
                    if addr & 0x0100 == 0 {
                        *ram_enabled = (val & 0x0F) == 0x0A;
                    } else {
                        *rom_bank = (val & 0x0F).max(1);
                    }
                }
            }
            Mbc::Mbc3 {
                ram_enabled,
                rom_bank,
                ram_bank,
                has_rtc,
                rtc,
            } => match addr {
                0x0000..=0x1FFF => *ram_enabled = (val & 0x0F) == 0x0A,
                0x2000..=0x3FFF => *rom_bank = val & 0x7F,
                0x4000..=0x5FFF => *ram_bank = val, // 0x00-0x03 = RAM bank, 0x08-0x0C = RTC register
                0x6000..=0x7FFF => {
                    if *has_rtc {
                        rtc.write_latch(val);
                    }
                }
                _ => {}
            },
            Mbc::Mbc5 {
                ram_enabled,
                rom_bank,
                ram_bank,
            } => match addr {
                0x0000..=0x1FFF => *ram_enabled = (val & 0x0F) == 0x0A,
                0x2000..=0x2FFF => *rom_bank = (*rom_bank & 0x100) | val as u16,
                0x3000..=0x3FFF => *rom_bank = (*rom_bank & 0x0FF) | ((val as u16 & 1) << 8),
                0x4000..=0x5FFF => *ram_bank = val & 0x0F,
                _ => {}
            },
        }
    }

    /// Whether cartridge RAM is currently readable/writable.
    fn ram_enabled(&self) -> bool {
        match &self.mbc {
            Mbc::None => true,
            Mbc::Mbc1 { ram_enabled, .. }
            | Mbc::Mbc2 { ram_enabled, .. }
            | Mbc::Mbc3 { ram_enabled, .. }
            | Mbc::Mbc5 { ram_enabled, .. } => *ram_enabled,
        }
    }

    /// Read from cartridge RAM (0xA000..=0xBFFF).
    pub fn read_ram(&self, addr: u16) -> u8 {
        if !self.ram_enabled() {
            return 0xFF;
        }
        // MBC3: a selected RTC register (0x08-0x0C) reads the latched clock.
        if let Mbc::Mbc3 {
            ram_bank,
            has_rtc: true,
            rtc,
            ..
        } = &self.mbc
        {
            if (0x08..=0x0C).contains(ram_bank) {
                return rtc.read_reg(*ram_bank - 0x08);
            }
        }
        let idx = self.ram_offset(addr);
        let val = self.ram.get(idx).copied().unwrap_or(0xFF);
        // MBC2 RAM is 4-bit: the upper nibble reads back as 1s.
        if matches!(self.mbc, Mbc::Mbc2 { .. }) {
            val | 0xF0
        } else {
            val
        }
    }

    /// Write to cartridge RAM (0xA000..=0xBFFF).
    pub fn write_ram(&mut self, addr: u16, val: u8) {
        if !self.ram_enabled() {
            return;
        }
        // MBC3: a selected RTC register (0x08-0x0C) writes the live clock. Refresh
        // the battery footer so a plain .srm save keeps the new time.
        let rtc_footer = if let Mbc::Mbc3 {
            ram_bank,
            has_rtc: true,
            rtc,
            ..
        } = &mut self.mbc
        {
            if (0x08..=0x0C).contains(&*ram_bank) {
                rtc.write_reg(*ram_bank - 0x08, val);
                Some(rtc.encode_footer())
            } else {
                None
            }
        } else {
            None
        };
        if let Some(footer) = rtc_footer {
            self.write_rtc_footer(&footer);
            return;
        }
        let idx = self.ram_offset(addr);
        let val = if matches!(self.mbc, Mbc::Mbc2 { .. }) {
            val & 0x0F
        } else {
            val
        };
        if let Some(cell) = self.ram.get_mut(idx) {
            *cell = val;
        }
    }

    fn ram_offset(&self, addr: u16) -> usize {
        // MBC2's 512-half-byte RAM lives at 0xA000..=0xA1FF and echoes upward.
        if matches!(self.mbc, Mbc::Mbc2 { .. }) {
            return addr as usize & 0x1FF;
        }
        let local = addr as usize & 0x1FFF;
        let bank = match &self.mbc {
            Mbc::Mbc1 {
                ram_bank,
                banking_mode,
                ..
            } => {
                if *banking_mode {
                    *ram_bank as usize
                } else {
                    0
                }
            }
            Mbc::Mbc3 { ram_bank, .. } => (*ram_bank & 0x03) as usize,
            Mbc::Mbc5 { ram_bank, .. } => *ram_bank as usize,
            Mbc::None | Mbc::Mbc2 { .. } => 0, // MBC2 handled above
        };
        // Guard against carts that report no RAM banks.
        let banks = self.header.ram_banks.max(1);
        (bank % banks) * 0x2000 + local
    }

    pub fn title(&self) -> &str {
        &self.header.title
    }

    /// Transfer the mutable cartridge state (RAM + bank registers). The ROM and
    /// header are static and the MBC variant is fixed by the loaded cartridge,
    /// so only the active variant's registers are serialized.
    pub(crate) fn transfer<C: crate::save::Cursor>(&mut self, c: &mut C) {
        c.bytes(&mut self.ram);
        match &mut self.mbc {
            Mbc::None => {}
            Mbc::Mbc1 {
                ram_enabled,
                rom_bank,
                ram_bank,
                banking_mode,
            } => {
                c.bool(ram_enabled);
                c.u8(rom_bank);
                c.u8(ram_bank);
                c.bool(banking_mode);
            }
            Mbc::Mbc2 {
                ram_enabled,
                rom_bank,
            } => {
                c.bool(ram_enabled);
                c.u8(rom_bank);
            }
            Mbc::Mbc3 {
                ram_enabled,
                rom_bank,
                ram_bank,
                has_rtc,
                rtc,
            } => {
                c.bool(ram_enabled);
                c.u8(rom_bank);
                c.u8(ram_bank);
                if *has_rtc {
                    rtc.transfer(c);
                }
            }
            Mbc::Mbc5 {
                ram_enabled,
                rom_bank,
                ram_bank,
            } => {
                c.bool(ram_enabled);
                c.u16(rom_bank);
                c.u8(ram_bank);
            }
        }
    }

    /// Advance the MBC3 real-time clock by `cycles` wall-clock T-cycles. A no-op
    /// for every other cartridge. The MMU calls this each tick with the
    /// double-speed-adjusted cycle count so the clock always tracks real time.
    pub fn tick_rtc(&mut self, cycles: u32) {
        let footer = if let Mbc::Mbc3 {
            has_rtc: true, rtc, ..
        } = &mut self.mbc
        {
            if rtc.tick(cycles) {
                Some(rtc.encode_footer())
            } else {
                None
            }
        } else {
            None
        };
        if let Some(footer) = footer {
            self.write_rtc_footer(&footer);
        }
    }

    /// Byte offset of the RTC battery footer: immediately past the game-visible
    /// RAM. (RTC carts are never MBC2.)
    fn rtc_footer_offset(&self) -> usize {
        self.header.ram_banks.max(1) * 0x2000
    }

    fn write_rtc_footer(&mut self, footer: &[u8]) {
        let off = self.rtc_footer_offset();
        if off + footer.len() <= self.ram.len() {
            self.ram[off..off + footer.len()].copy_from_slice(footer);
        }
    }

    /// After the frontend loads a battery `.srm`, pull the RTC time back out of
    /// its footer (if the save carried one). Call once, right after `load_sram`.
    pub fn restore_rtc_from_footer(&mut self) {
        if !self.header.has_rtc {
            return;
        }
        let off = self.rtc_footer_offset();
        if off + RTC_FOOTER_LEN > self.ram.len() {
            return;
        }
        let footer: [u8; RTC_FOOTER_LEN] = self.ram[off..off + RTC_FOOTER_LEN]
            .try_into()
            .expect("slice is RTC_FOOTER_LEN");
        if let Mbc::Mbc3 {
            has_rtc: true, rtc, ..
        } = &mut self.mbc
        {
            rtc.decode_footer(&footer);
        }
    }

    pub fn has_battery(&self) -> bool {
        self.header.has_battery
    }

    /// Whether the cartridge requests Game Boy Color features (flag 0x80/0xC0).
    pub fn is_cgb(&self) -> bool {
        self.header.cgb_flag & 0x80 != 0
    }

    pub fn title_checksum(&self) -> u8 {
        self.header.title_checksum
    }
    pub fn title_fourth(&self) -> u8 {
        self.header.title_fourth
    }
    pub fn nintendo_licensed(&self) -> bool {
        self.header.nintendo_licensed
    }

    /// The raw cartridge RAM, for battery-save persistence.
    pub fn ram(&self) -> &[u8] {
        &self.ram
    }
    pub fn ram_mut(&mut self) -> &mut [u8] {
        &mut self.ram
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal MBC3 + RAM + TIMER + BATTERY cartridge (type 0x10), 32 KiB ROM,
    /// one 8 KiB RAM bank.
    fn mbc3_rtc_cart() -> Cartridge {
        let mut rom = vec![0u8; 0x8000];
        rom[0x0147] = 0x10; // MBC3+TIMER+RAM+BATTERY
        rom[0x0148] = 0x00; // 32 KiB (2 ROM banks)
        rom[0x0149] = 0x02; // 8 KiB RAM (1 bank)
        Cartridge::new(rom)
    }

    /// Enable RAM/RTC and latch the current time into the readable snapshot.
    fn latch(cart: &mut Cartridge) {
        cart.write_rom(0x6000, 0x00);
        cart.write_rom(0x6000, 0x01);
    }

    /// Read RTC register `reg` (0x08=S .. 0x0C=DH) after a fresh latch.
    fn read_rtc(cart: &mut Cartridge, reg: u8) -> u8 {
        cart.write_rom(0x4000, reg);
        cart.read_ram(0xA000)
    }

    #[test]
    fn rtc_present_only_for_timer_carts() {
        assert!(mbc3_rtc_cart().header.has_rtc);
        // MBC3 without the timer (type 0x13) has no clock.
        let mut rom = vec![0u8; 0x8000];
        rom[0x0147] = 0x13;
        rom[0x0149] = 0x02;
        assert!(!Cartridge::new(rom).header.has_rtc);
    }

    #[test]
    fn rtc_ticks_seconds_minutes_hours() {
        let mut cart = mbc3_rtc_cart();
        cart.write_rom(0x0000, 0x0A); // enable RAM/RTC
        latch(&mut cart);
        assert_eq!(read_rtc(&mut cart, 0x08), 0);

        // A snapshot is frozen until the next latch.
        cart.tick_rtc(5 * CYCLES_PER_SECOND);
        assert_eq!(read_rtc(&mut cart, 0x08), 0, "latched value stays put");
        latch(&mut cart);
        assert_eq!(read_rtc(&mut cart, 0x08), 5);

        // +60 s from 5 s -> 1 min 5 s.
        cart.tick_rtc(60 * CYCLES_PER_SECOND);
        latch(&mut cart);
        assert_eq!(read_rtc(&mut cart, 0x08), 5);
        assert_eq!(read_rtc(&mut cart, 0x09), 1);
    }

    #[test]
    fn rtc_register_write_sets_live_clock() {
        let mut cart = mbc3_rtc_cart();
        cart.write_rom(0x0000, 0x0A);
        cart.write_rom(0x4000, 0x08); // select seconds
        cart.write_ram(0xA000, 30);
        latch(&mut cart);
        assert_eq!(read_rtc(&mut cart, 0x08), 30);
    }

    #[test]
    fn rtc_halt_stops_the_clock() {
        let mut cart = mbc3_rtc_cart();
        cart.write_rom(0x0000, 0x0A);
        cart.write_rom(0x4000, 0x0C); // select DH
        cart.write_ram(0xA000, 0x40); // set HALT
        cart.tick_rtc(100 * CYCLES_PER_SECOND);
        latch(&mut cart);
        assert_eq!(read_rtc(&mut cart, 0x08), 0, "halted clock does not advance");
    }

    #[test]
    fn rtc_day_counter_carries() {
        let mut cart = mbc3_rtc_cart();
        cart.write_rom(0x0000, 0x0A);
        // Seed 23:59:59 on day 511.
        cart.write_rom(0x4000, 0x08);
        cart.write_ram(0xA000, 59);
        cart.write_rom(0x4000, 0x09);
        cart.write_ram(0xA000, 59);
        cart.write_rom(0x4000, 0x0A);
        cart.write_ram(0xA000, 23);
        cart.write_rom(0x4000, 0x0B);
        cart.write_ram(0xA000, 0xFF); // day low = 255
        cart.write_rom(0x4000, 0x0C);
        cart.write_ram(0xA000, 0x01); // day high bit = 1 -> day 511

        cart.tick_rtc(CYCLES_PER_SECOND); // one more second wraps the day counter
        latch(&mut cart);
        let dh = read_rtc(&mut cart, 0x0C);
        assert_eq!(dh & 0x80, 0x80, "day-carry bit set");
        assert_eq!(dh & 0x01, 0x00, "day high bit wrapped to 0");
        assert_eq!(read_rtc(&mut cart, 0x0B), 0, "day low wrapped to 0");
    }

    #[test]
    fn rtc_persists_through_battery_footer() {
        let mut cart = mbc3_rtc_cart();
        cart.write_rom(0x0000, 0x0A);
        cart.write_rom(0x4000, 0x08);
        cart.write_ram(0xA000, 42); // seconds
        cart.write_rom(0x4000, 0x0A);
        cart.write_ram(0xA000, 7); // hours
        let saved = cart.ram().to_vec();

        // Fresh cart, restore the .srm, and confirm the clock came back.
        let mut cart2 = mbc3_rtc_cart();
        {
            let ram = cart2.ram_mut();
            let n = ram.len().min(saved.len());
            ram[..n].copy_from_slice(&saved[..n]);
        }
        cart2.restore_rtc_from_footer();
        cart2.write_rom(0x0000, 0x0A);
        latch(&mut cart2);
        assert_eq!(read_rtc(&mut cart2, 0x08), 42);
        assert_eq!(read_rtc(&mut cart2, 0x0A), 7);
    }

    #[test]
    fn old_srm_without_footer_leaves_clock_at_power_on() {
        // A battery save from before RTC support (just RAM, no footer) must load
        // without corrupting the clock.
        let mut cart = mbc3_rtc_cart();
        let legacy = vec![0xABu8; 0x2000]; // 8 KiB of RAM, no footer
        {
            let ram = cart.ram_mut();
            let n = ram.len().min(legacy.len());
            ram[..n].copy_from_slice(&legacy[..n]);
        }
        cart.restore_rtc_from_footer();
        cart.write_rom(0x0000, 0x0A);
        latch(&mut cart);
        assert_eq!(read_rtc(&mut cart, 0x08), 0);
    }
}
