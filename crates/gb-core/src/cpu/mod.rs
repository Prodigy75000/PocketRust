//! The Sharp LR35902 CPU: a close relative of the Z80/8080.
//!
//! `mod.rs` owns the top-level step loop, interrupt servicing, and the HALT/EI
//! quirks. The giant opcode decoder lives in `execute.rs`.

mod execute;
pub mod registers;

use crate::mmu::Mmu;
use registers::Registers;

pub struct Cpu {
    pub reg: Registers,
    /// Interrupt Master Enable: the global gate for all interrupts.
    pub ime: bool,
    /// EI enables interrupts only *after* the following instruction. This flag
    /// carries that one-instruction delay.
    ime_pending: bool,
    /// True while the CPU is parked on a HALT waiting for an interrupt.
    pub halted: bool,
}

impl Cpu {
    pub fn new(cgb: bool) -> Cpu {
        Cpu {
            reg: if cgb {
                Registers::post_bios_cgb()
            } else {
                Registers::post_bios_dmg()
            },
            ime: false,
            ime_pending: false,
            halted: false,
        }
    }

    pub(crate) fn transfer<C: crate::save::Cursor>(&mut self, c: &mut C) {
        c.u8(&mut self.reg.a);
        c.u8(&mut self.reg.f);
        c.u8(&mut self.reg.b);
        c.u8(&mut self.reg.c);
        c.u8(&mut self.reg.d);
        c.u8(&mut self.reg.e);
        c.u8(&mut self.reg.h);
        c.u8(&mut self.reg.l);
        c.u16(&mut self.reg.sp);
        c.u16(&mut self.reg.pc);
        c.bool(&mut self.ime);
        c.bool(&mut self.ime_pending);
        c.bool(&mut self.halted);
    }

    /// Run exactly one CPU step (an interrupt dispatch, a HALT idle, or one
    /// instruction), tick the rest of the machine for the elapsed cycles, and
    /// return how many T-cycles passed.
    pub fn step(&mut self, mmu: &mut Mmu) -> u32 {
        mmu.begin_instr();
        let cycles = self.dispatch(mmu);
        // Memory accesses ticked their own M-cycles during dispatch; settle the
        // internal cycles so the machine advances exactly `cycles` in total.
        mmu.settle(cycles);
        cycles
    }

    fn dispatch(&mut self, mmu: &mut Mmu) -> u32 {
        // A pending-and-enabled interrupt always wakes the CPU from HALT, even
        // if IME is off (it just won't be serviced in that case).
        let pending = mmu.interrupt_flag & mmu.interrupt_enable & 0x1F;
        if self.halted && pending != 0 {
            self.halted = false;
        }

        if self.ime {
            if let Some(cycles) = self.service_interrupt(mmu, pending) {
                return cycles;
            }
        }

        if self.halted {
            return 4; // idle machine cycle
        }

        // Latch whether the *previous* instruction was EI before we run this one.
        let enable_ime = self.ime_pending;
        self.ime_pending = false;
        let cycles = self.execute(mmu);
        if enable_ime {
            self.ime = true;
        }
        cycles
    }

    /// If an interrupt is pending and enabled, dispatch to its vector.
    fn service_interrupt(&mut self, mmu: &mut Mmu, pending: u8) -> Option<u32> {
        if pending == 0 {
            return None;
        }
        self.ime = false;
        self.halted = false;

        // Lowest set bit = highest priority (VBlank first).
        let bit = pending.trailing_zeros() as u8;
        mmu.interrupt_flag &= !(1 << bit);

        let vector = 0x40 + (bit as u16) * 8;
        let pc = self.reg.pc;
        self.push16(mmu, pc);
        self.reg.pc = vector;
        Some(20) // interrupt dispatch costs 5 machine cycles
    }
}
