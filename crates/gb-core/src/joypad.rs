//! The joypad register (0xFF00).
//!
//! Eight buttons are read through a 2x4 matrix. The CPU selects either the
//! direction row or the action row by clearing bit 5 or bit 4, then reads the
//! low nibble. A pressed button reads as 0 (active low).

#[derive(Clone, Copy)]
pub enum Button {
    Right,
    Left,
    Up,
    Down,
    A,
    B,
    Select,
    Start,
}

pub struct Joypad {
    /// Bitmask of currently pressed buttons (1 = pressed). Layout, low to high:
    /// Right, Left, Up, Down, A, B, Select, Start.
    pressed: u8,
    /// The row-select bits the CPU last wrote (bits 4 and 5).
    select: u8,
    /// Set when a button transitions to pressed while its row is selected.
    pub interrupt: bool,
}

impl Joypad {
    pub fn new() -> Joypad {
        Joypad {
            pressed: 0,
            select: 0x30,
            interrupt: false,
        }
    }

    pub fn set_button(&mut self, button: Button, down: bool) {
        let bit = 1u8 << button as u8;
        let was = self.pressed & bit != 0;
        if down {
            self.pressed |= bit;
            if !was {
                self.interrupt = true;
            }
        } else {
            self.pressed &= !bit;
        }
    }

    pub fn write(&mut self, val: u8) {
        self.select = val & 0x30;
    }

    pub fn read(&self) -> u8 {
        let mut out = self.select | 0xC0; // bits 6-7 read as 1
        let low = self.pressed & 0x0F; // direction buttons
        let high = (self.pressed >> 4) & 0x0F; // action buttons

        // A selected row is active when its select bit is 0.
        if self.select & 0x10 == 0 {
            out |= !low & 0x0F;
        }
        if self.select & 0x20 == 0 {
            out |= !high & 0x0F;
        }
        // If neither row selected, low nibble reads all-high (nothing pressed).
        if self.select & 0x30 == 0x30 {
            out |= 0x0F;
        }
        out
    }
}
