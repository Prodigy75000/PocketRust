//! The LR35902 register file.
//!
//! Eight 8-bit registers (A, F, B, C, D, E, H, L) that pair up into four
//! 16-bit registers (AF, BC, DE, HL), plus the 16-bit SP and PC.
//!
//! `F` is special: only the top four bits are meaningful (Z N H C), and the
//! low four bits are always zero on real hardware. We enforce that on write.

/// Zero flag: set when the result of an operation is zero.
pub const FLAG_Z: u8 = 0b1000_0000;
/// Subtract flag: set when the last op was a subtraction (used by DAA).
pub const FLAG_N: u8 = 0b0100_0000;
/// Half-carry flag: carry out of bit 3 (used by DAA).
pub const FLAG_H: u8 = 0b0010_0000;
/// Carry flag: carry out of bit 7 (8-bit) or bit 15 (16-bit).
pub const FLAG_C: u8 = 0b0001_0000;

#[derive(Debug, Clone, Copy, Default)]
pub struct Registers {
    pub a: u8,
    pub f: u8,
    pub b: u8,
    pub c: u8,
    pub d: u8,
    pub e: u8,
    pub h: u8,
    pub l: u8,
    pub sp: u16,
    pub pc: u16,
}

impl Registers {
    /// Post-BIOS register state for the DMG (as if the boot ROM just finished).
    /// These are the well-documented power-on values that games rely on.
    pub fn post_bios_dmg() -> Self {
        Registers {
            a: 0x01,
            f: 0xB0, // Z set, H set, C set (result of the boot ROM's checksum)
            b: 0x00,
            c: 0x13,
            d: 0x00,
            e: 0xD8,
            h: 0x01,
            l: 0x4D,
            sp: 0xFFFE,
            pc: 0x0100, // execution begins at the cartridge entry point
        }
    }

    /// Post-BIOS state for the CGB running in colour mode. A=0x11 is how games
    /// detect they are on Game Boy Color hardware.
    pub fn post_bios_cgb() -> Self {
        Registers {
            a: 0x11,
            f: 0x80,
            b: 0x00,
            c: 0x00,
            d: 0xFF,
            e: 0x56,
            h: 0x00,
            l: 0x0D,
            sp: 0xFFFE,
            pc: 0x0100,
        }
    }

    #[inline]
    pub fn af(&self) -> u16 {
        (self.a as u16) << 8 | self.f as u16
    }
    #[inline]
    pub fn bc(&self) -> u16 {
        (self.b as u16) << 8 | self.c as u16
    }
    #[inline]
    pub fn de(&self) -> u16 {
        (self.d as u16) << 8 | self.e as u16
    }
    #[inline]
    pub fn hl(&self) -> u16 {
        (self.h as u16) << 8 | self.l as u16
    }

    #[inline]
    pub fn set_af(&mut self, v: u16) {
        self.a = (v >> 8) as u8;
        self.f = (v as u8) & 0xF0; // low nibble of F is always zero
    }
    #[inline]
    pub fn set_bc(&mut self, v: u16) {
        self.b = (v >> 8) as u8;
        self.c = v as u8;
    }
    #[inline]
    pub fn set_de(&mut self, v: u16) {
        self.d = (v >> 8) as u8;
        self.e = v as u8;
    }
    #[inline]
    pub fn set_hl(&mut self, v: u16) {
        self.h = (v >> 8) as u8;
        self.l = v as u8;
    }

    // --- Flag helpers -------------------------------------------------------

    #[inline]
    pub fn flag(&self, mask: u8) -> bool {
        self.f & mask != 0
    }

    #[inline]
    pub fn set_flag(&mut self, mask: u8, on: bool) {
        if on {
            self.f |= mask;
        } else {
            self.f &= !mask;
        }
        self.f &= 0xF0;
    }
}
