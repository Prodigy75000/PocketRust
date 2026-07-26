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
        ram_bank: u8,  // RAM bank 0-3 (RTC registers 0x08-0x0C ignored)
    },
    Mbc5 {
        ram_enabled: bool,
        rom_bank: u16, // 9 bits
        ram_bank: u8,  // 4 bits
    },
}

impl Cartridge {
    pub fn new(rom: Vec<u8>) -> Cartridge {
        let header = Header::parse(&rom);
        // MBC2 has 512 x 4-bit of built-in RAM (no external banks); everything
        // else uses the header-declared bank count.
        let ram = if header.mbc_kind == MbcKind::Mbc2 {
            vec![0u8; 512]
        } else {
            vec![0u8; header.ram_banks.max(1) * 0x2000]
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
            } => match addr {
                0x0000..=0x1FFF => *ram_enabled = (val & 0x0F) == 0x0A,
                0x2000..=0x3FFF => *rom_bank = val & 0x7F,
                0x4000..=0x5FFF => *ram_bank = val, // RTC selects (0x08+) fall out as high RAM banks
                _ => {}                              // 0x6000-0x7FFF: RTC latch, ignored
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
            } => {
                c.bool(ram_enabled);
                c.u8(rom_bank);
                c.u8(ram_bank);
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
