//! The DMG timer.
//!
//! Internally the timer is a single 16-bit counter that increments every T-cycle.
//! - DIV (0xFF04) is the top 8 bits of that counter.
//! - TIMA (0xFF05) increments at a rate selected by TAC, and requests an
//!   interrupt when it overflows, reloading from TMA (0xFF06).
//!
//! Modelling the shared 16-bit counter (rather than two independent dividers)
//! is what makes the timing edge-cases in test ROMs line up.

pub struct Timer {
    /// The full 16-bit internal counter. DIV is `(counter >> 8) as u8`.
    counter: u16,
    pub tima: u8,
    pub tma: u8,
    pub tac: u8,
    /// Remembers the previous "tick" bit so we can detect falling edges.
    prev_bit: bool,
    /// Set for one step after TIMA overflows: request a timer interrupt.
    pub interrupt: bool,
}

impl Timer {
    pub fn new() -> Timer {
        Timer {
            counter: 0xABCC, // matches the DIV value after the DMG boot ROM
            tima: 0,
            tma: 0,
            tac: 0xF8,
            prev_bit: false,
            interrupt: false,
        }
    }

    pub fn div(&self) -> u8 {
        (self.counter >> 8) as u8
    }

    /// Writing any value to DIV resets the whole internal counter to zero.
    pub fn reset_div(&mut self) {
        self.counter = 0;
    }

    /// Advance the timer by `cycles` T-cycles.
    pub fn step(&mut self, cycles: u32) {
        for _ in 0..cycles {
            self.counter = self.counter.wrapping_add(1);
            self.update_tima();
        }
    }

    fn tac_bit(&self) -> u16 {
        // TAC bits 0-1 select which bit of the counter drives TIMA.
        match self.tac & 0b11 {
            0b00 => 1 << 9, // 4096 Hz
            0b01 => 1 << 3, // 262144 Hz
            0b10 => 1 << 5, // 65536 Hz
            _ => 1 << 7,    // 16384 Hz
        }
    }

    pub(crate) fn transfer<C: crate::save::Cursor>(&mut self, c: &mut C) {
        c.u16(&mut self.counter);
        c.u8(&mut self.tima);
        c.u8(&mut self.tma);
        c.u8(&mut self.tac);
        c.bool(&mut self.prev_bit);
        c.bool(&mut self.interrupt);
    }

    fn update_tima(&mut self) {
        let enabled = self.tac & 0b100 != 0;
        let bit = enabled && (self.counter & self.tac_bit()) != 0;
        // TIMA increments on the falling edge of the selected counter bit.
        if self.prev_bit && !bit {
            let (new, overflow) = self.tima.overflowing_add(1);
            if overflow {
                self.tima = self.tma;
                self.interrupt = true;
            } else {
                self.tima = new;
            }
        }
        self.prev_bit = bit;
    }
}
