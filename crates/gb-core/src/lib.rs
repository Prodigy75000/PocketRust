//! gb-core: a clean-room Game Boy (DMG) emulator, written from scratch in Rust.
//!
//! The public surface is deliberately tiny:
//!   - [`GameBoy::new`] takes a ROM image.
//!   - [`GameBoy::step_frame`] runs until one video frame is ready.
//!   - [`GameBoy::framebuffer`] returns the 160x144 grid of 2-bit shades.
//!   - [`GameBoy::set_button`] feeds input.
//!
//! Everything else (CPU, MMU, PPU, timer, cartridge) is an implementation detail.

mod apu;
mod cartridge;
mod colorize;
mod cpu;
mod gbc_palettes;
mod joypad;
mod link;
mod mmu;
mod ppu;
mod save;
mod serial;
mod sgb;
mod timer;

pub use link::{packet_len, LinkProto, PROTOCOL_VERSION, TAG_CLOCK, TAG_OUTPUT, TAG_REPLY};
pub use serial::{local_pair, LinkCable, LocalLink};

use save::{ReadCursor, WriteCursor};

/// Magic + version prefix for a save-state blob. Bumped to 2 when the MBC3 RTC
/// was added to the state (older blobs are cleanly refused rather than misread).
const STATE_MAGIC: &[u8] = b"PRGB2";

pub use apu::SAMPLE_RATE;
pub use colorize::Colorize;
pub use joypad::Button;
pub use ppu::{Pixel, SCREEN_H, SCREEN_W};

use colorize::DmgPalette;

use cartridge::Cartridge;
use cpu::Cpu;
use mmu::Mmu;

/// Number of T-cycles the DMG runs per video frame (roughly 59.7 fps).
const CYCLES_PER_FRAME: u32 = 70224;

pub struct GameBoy {
    cpu: Cpu,
    mmu: Mmu,
}

impl GameBoy {
    /// Create a Game Boy with the given cartridge ROM loaded, positioned as if
    /// the boot ROM has just handed control to the cartridge at 0x0100.
    pub fn new(rom: Vec<u8>) -> GameBoy {
        let cartridge = Cartridge::new(rom);
        let cgb = cartridge.is_cgb();
        GameBoy {
            cpu: Cpu::new(cgb),
            mmu: Mmu::new(cartridge, cgb),
        }
    }

    /// Whether the loaded cartridge is running in Game Boy Color mode.
    pub fn is_cgb(&self) -> bool {
        self.mmu.cgb
    }

    /// Set how monochrome (DMG) games are colorized. No effect on CGB games,
    /// which always use their own palettes. Drives Trophy Hub's colorize toggle.
    pub fn set_colorization(&mut self, mode: Colorize) {
        if !self.mmu.cgb {
            let cart = &self.mmu.cartridge;
            let palette = DmgPalette::resolve(
                mode,
                cart.title_checksum(),
                cart.title_fourth(),
                cart.nintendo_licensed(),
            );
            self.mmu.ppu.set_dmg_palette(palette);
        }
    }

    pub fn title(&self) -> &str {
        self.mmu.cartridge.title()
    }

    /// Execute a single CPU step. Returns the T-cycles it took.
    pub fn step(&mut self) -> u32 {
        self.cpu.step(&mut self.mmu)
    }

    /// Run the machine until the PPU signals a completed frame, then hand back
    /// a reference to the fresh framebuffer (RGB888, one [`Pixel`] per dot).
    pub fn step_frame(&mut self) -> &[Pixel] {
        self.mmu.ppu.frame_ready = false;
        // Double-speed mode packs twice as many CPU cycles into a frame.
        let mut budget = CYCLES_PER_FRAME * 4; // generous safety bound
        while !self.mmu.ppu.frame_ready && budget > 0 {
            let c = self.cpu.step(&mut self.mmu);
            budget = budget.saturating_sub(c);
        }
        &self.mmu.ppu.framebuffer
    }

    pub fn framebuffer(&self) -> &[Pixel] {
        &self.mmu.ppu.framebuffer
    }

    pub fn set_button(&mut self, button: Button, pressed: bool) {
        self.mmu.set_button(button, pressed);
    }

    /// Drain any bytes the ROM has pushed out the serial port. Blargg's test
    /// ROMs report their progress this way, so this is our headless oracle.
    pub fn take_serial(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.mmu.serial.log)
    }

    /// Attach a link-cable transport (local or networked). The connected peer
    /// exchanges a byte whenever this Game Boy performs a serial transfer.
    pub fn connect_link(&mut self, link: Box<dyn LinkCable>) {
        self.mmu.serial.connect(link);
    }

    pub fn disconnect_link(&mut self) {
        self.mmu.serial.disconnect();
    }

    pub fn link_connected(&self) -> bool {
        self.mmu.serial.is_linked()
    }

    /// Serialize the full machine state into a portable byte blob (save state).
    pub fn save_state(&mut self) -> Vec<u8> {
        let mut c = WriteCursor::new();
        c.buf.extend_from_slice(STATE_MAGIC);
        self.cpu.transfer(&mut c);
        self.mmu.transfer(&mut c);
        c.buf
    }

    /// Restore machine state from a blob produced by [`GameBoy::save_state`].
    /// Returns false if the blob is not a valid state for this build.
    pub fn load_state(&mut self, data: &[u8]) -> bool {
        if data.len() < STATE_MAGIC.len() || &data[..STATE_MAGIC.len()] != STATE_MAGIC {
            return false;
        }
        let mut c = ReadCursor::new(&data[STATE_MAGIC.len()..]);
        self.cpu.transfer(&mut c);
        self.mmu.transfer(&mut c);
        c.ok
    }

    /// Drain the audio produced since the last call: interleaved stereo i16
    /// samples at [`SAMPLE_RATE`]. The frontend feeds these to its audio device.
    pub fn take_audio(&mut self) -> Vec<i16> {
        self.mmu.apu.take_output()
    }

    pub fn has_battery(&self) -> bool {
        self.mmu.cartridge.has_battery()
    }

    /// Battery-backed cartridge RAM, for save persistence by the frontend.
    pub fn sram(&self) -> &[u8] {
        self.mmu.cartridge.ram()
    }

    /// Work RAM, laid out exactly as the hardware banks it: bank 0 first
    /// (0xC000..0xCFFF), then banks 1..7 (what 0xD000..0xDFFF selects between).
    /// A DMG only ever uses the first two banks, so its 8 KiB is the leading
    /// 8 KiB of this slice and a front-end can take either length without
    /// re-mapping anything.
    ///
    /// Exposed for front-ends that read guest memory: RetroAchievements needs
    /// this as libretro's SYSTEM_RAM, and it is the region every Game Boy
    /// achievement set is written against.
    pub fn wram(&self) -> &[u8] {
        &self.mmu.wram
    }

    /// Overwrite cartridge RAM from a previously saved battery file.
    pub fn load_sram(&mut self, data: &[u8]) {
        let ram = self.mmu.cartridge.ram_mut();
        let n = ram.len().min(data.len());
        ram[..n].copy_from_slice(&data[..n]);
        // If this is an RTC cart and the save carried a clock footer, restore it.
        self.mmu.cartridge.restore_rtc_from_footer();
    }

    /// Decode the MBC3 RTC from the battery-RAM footer. A libretro frontend fills
    /// `sram()` in place via RETRO_MEMORY_SAVE_RAM (bypassing [`load_sram`]), so
    /// it must call this once after loading the save file to bring the clock back.
    /// A no-op for carts without an RTC or without a footer present.
    pub fn restore_rtc(&mut self) {
        self.mmu.cartridge.restore_rtc_from_footer();
    }

    /// Read a byte from the bus without side effects worth worrying about.
    /// Used by the test harness to read Blargg's in-memory result protocol.
    pub fn peek(&self, addr: u16) -> u8 {
        self.mmu.read(addr)
    }

    /// Debug: (SGB commands seen as (code, data_len), whether SGB is active,
    /// the four resolved SGB palettes). For validating SGB command capture.
    pub fn sgb_debug(&self) -> (Vec<(u8, usize)>, bool, [[u32; 4]; 4]) {
        (
            self.mmu.sgb.log.clone(),
            self.mmu.sgb.active,
            self.mmu.sgb.palettes,
        )
    }

    /// Debug: interrupt master enable, for diagnosing stuck interrupt waits.
    pub fn debug_ime(&self) -> bool {
        self.cpu.ime
    }

    /// Debug: (PC, LCDC, LY, halted) — a quick peek for debugging stuck ROMs.
    pub fn debug_state(&self) -> (u16, u8, u8, bool) {
        (
            self.cpu.reg.pc,
            self.mmu.ppu.lcdc,
            self.mmu.ppu.ly,
            self.cpu.halted,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Smallest cartridge that loads: 32 KiB, no MBC, no RAM. `cgb` picks the
    /// 0x0143 flag, which is what decides whether all eight work-RAM banks are
    /// in play.
    fn bare_rom(cgb: bool) -> Vec<u8> {
        let mut rom = vec![0u8; 0x8000];
        rom[0x0143] = if cgb { 0xC0 } else { 0x00 };
        rom[0x0147] = 0x00; // ROM only
        rom[0x0148] = 0x00; // 32 KiB
        rom[0x0149] = 0x00; // no cartridge RAM
        rom
    }

    /// `wram()` is handed to a front-end as libretro's SYSTEM_RAM, and every
    /// Game Boy achievement address is an offset into it. So the guarantee that
    /// matters is not the length but the *window*: offset 0 has to be guest
    /// 0xC000 and offset 0x1FFF has to be guest 0xDFFF, with bank 0 first. If
    /// the banks were stored in any other order this still returns 32 KiB of
    /// plausible-looking memory and every achievement silently reads the wrong
    /// byte, so pin both ends.
    #[test]
    fn wram_offset_zero_is_guest_c000() {
        let mut gb = GameBoy::new(bare_rom(false));
        gb.mmu.write(0xC000, 0xAB);
        gb.mmu.write(0xC123, 0x5A);
        gb.mmu.write(0xDFFF, 0xCD);

        assert_eq!(gb.wram()[0x0000], 0xAB, "offset 0 must be guest 0xC000");
        assert_eq!(gb.wram()[0x0123], 0x5A);
        assert_eq!(gb.wram()[0x1FFF], 0xCD, "offset 0x1FFF must be guest 0xDFFF");
        // Reading back through the bus has to agree, or one of the two paths is
        // addressing a different bank.
        assert_eq!(gb.peek(0xC000), 0xAB);
        assert_eq!(gb.peek(0xDFFF), 0xCD);
    }

    /// The whole array is always 32 KiB; `is_cgb` is what tells a front-end how
    /// much of it the running machine can reach, and it drives the SYSTEM_RAM
    /// size the libretro layer reports (8 KiB for DMG, 32 KiB for CGB).
    #[test]
    fn wram_is_eight_banks_and_cgb_is_reported() {
        let dmg = GameBoy::new(bare_rom(false));
        assert_eq!(dmg.wram().len(), 0x8000);
        assert!(!dmg.is_cgb());

        let cgb = GameBoy::new(bare_rom(true));
        assert_eq!(cgb.wram().len(), 0x8000);
        assert!(cgb.is_cgb());
    }
}
