//! The opcode decoder and executor.
//!
//! Every arm returns the number of T-cycles the instruction consumed. The
//! regular structure of the LR35902 map lets us fold the big 0x40-0xBF register
//! blocks into loops over an 8-bit register selector.

use super::registers::{FLAG_C, FLAG_H, FLAG_N, FLAG_Z};
use super::Cpu;
use crate::mmu::Mmu;

impl Cpu {
    // --- Memory / immediate access -----------------------------------------

    fn read8(&self, mmu: &Mmu, addr: u16) -> u8 {
        mmu.read(addr)
    }
    fn write8(&mut self, mmu: &mut Mmu, addr: u16, val: u8) {
        mmu.write(addr, val);
    }

    /// Fetch the 8-bit immediate at PC and advance.
    fn imm8(&mut self, mmu: &Mmu) -> u8 {
        let v = mmu.read(self.reg.pc);
        self.reg.pc = self.reg.pc.wrapping_add(1);
        v
    }
    /// Fetch the little-endian 16-bit immediate at PC and advance.
    fn imm16(&mut self, mmu: &Mmu) -> u16 {
        let lo = self.imm8(mmu) as u16;
        let hi = self.imm8(mmu) as u16;
        hi << 8 | lo
    }

    pub(super) fn push16(&mut self, mmu: &mut Mmu, val: u16) {
        self.reg.sp = self.reg.sp.wrapping_sub(1);
        self.write8(mmu, self.reg.sp, (val >> 8) as u8);
        self.reg.sp = self.reg.sp.wrapping_sub(1);
        self.write8(mmu, self.reg.sp, val as u8);
    }
    fn pop16(&mut self, mmu: &Mmu) -> u16 {
        let lo = self.read8(mmu, self.reg.sp) as u16;
        self.reg.sp = self.reg.sp.wrapping_add(1);
        let hi = self.read8(mmu, self.reg.sp) as u16;
        self.reg.sp = self.reg.sp.wrapping_add(1);
        hi << 8 | lo
    }

    /// The 8-bit register selector used across the 0x40-0xBF blocks and CB ops.
    /// 0=B 1=C 2=D 3=E 4=H 5=L 6=(HL) 7=A. Returns cycles cost via caller.
    fn get_r(&self, mmu: &Mmu, sel: u8) -> u8 {
        match sel {
            0 => self.reg.b,
            1 => self.reg.c,
            2 => self.reg.d,
            3 => self.reg.e,
            4 => self.reg.h,
            5 => self.reg.l,
            6 => self.read8(mmu, self.reg.hl()),
            7 => self.reg.a,
            _ => unreachable!(),
        }
    }
    fn set_r(&mut self, mmu: &mut Mmu, sel: u8, val: u8) {
        match sel {
            0 => self.reg.b = val,
            1 => self.reg.c = val,
            2 => self.reg.d = val,
            3 => self.reg.e = val,
            4 => self.reg.h = val,
            5 => self.reg.l = val,
            6 => {
                let hl = self.reg.hl();
                self.write8(mmu, hl, val);
            }
            7 => self.reg.a = val,
            _ => unreachable!(),
        }
    }

    // --- ALU primitives (all operate on A, set flags) -----------------------

    fn add_a(&mut self, v: u8, carry: bool) {
        let c = carry as u16;
        let a = self.reg.a as u16;
        let r = a + v as u16 + c;
        self.reg.set_flag(FLAG_Z, r as u8 == 0);
        self.reg.set_flag(FLAG_N, false);
        self.reg.set_flag(FLAG_H, (a & 0xF) + (v as u16 & 0xF) + c > 0xF);
        self.reg.set_flag(FLAG_C, r > 0xFF);
        self.reg.a = r as u8;
    }

    fn sub_a(&mut self, v: u8, carry: bool) {
        let c = carry as i16;
        let a = self.reg.a as i16;
        let r = a - v as i16 - c;
        self.reg.set_flag(FLAG_Z, r as u8 == 0);
        self.reg.set_flag(FLAG_N, true);
        self.reg.set_flag(FLAG_H, (a & 0xF) - (v as i16 & 0xF) - c < 0);
        self.reg.set_flag(FLAG_C, r < 0);
        self.reg.a = r as u8;
    }

    fn and_a(&mut self, v: u8) {
        self.reg.a &= v;
        self.reg.set_flag(FLAG_Z, self.reg.a == 0);
        self.reg.set_flag(FLAG_N, false);
        self.reg.set_flag(FLAG_H, true);
        self.reg.set_flag(FLAG_C, false);
    }
    fn or_a(&mut self, v: u8) {
        self.reg.a |= v;
        self.reg.set_flag(FLAG_Z, self.reg.a == 0);
        self.reg.set_flag(FLAG_N, false);
        self.reg.set_flag(FLAG_H, false);
        self.reg.set_flag(FLAG_C, false);
    }
    fn xor_a(&mut self, v: u8) {
        self.reg.a ^= v;
        self.reg.set_flag(FLAG_Z, self.reg.a == 0);
        self.reg.set_flag(FLAG_N, false);
        self.reg.set_flag(FLAG_H, false);
        self.reg.set_flag(FLAG_C, false);
    }
    /// CP is a subtract that discards the result but keeps the flags.
    fn cp_a(&mut self, v: u8) {
        let a = self.reg.a;
        self.sub_a(v, false);
        self.reg.a = a;
    }

    fn inc8(&mut self, v: u8) -> u8 {
        let r = v.wrapping_add(1);
        self.reg.set_flag(FLAG_Z, r == 0);
        self.reg.set_flag(FLAG_N, false);
        self.reg.set_flag(FLAG_H, (v & 0xF) + 1 > 0xF);
        r
    }
    fn dec8(&mut self, v: u8) -> u8 {
        let r = v.wrapping_sub(1);
        self.reg.set_flag(FLAG_Z, r == 0);
        self.reg.set_flag(FLAG_N, true);
        self.reg.set_flag(FLAG_H, (v & 0xF) == 0);
        r
    }

    fn add_hl(&mut self, v: u16) {
        let hl = self.reg.hl();
        let r = hl as u32 + v as u32;
        self.reg.set_flag(FLAG_N, false);
        self.reg.set_flag(FLAG_H, (hl & 0xFFF) + (v & 0xFFF) > 0xFFF);
        self.reg.set_flag(FLAG_C, r > 0xFFFF);
        self.reg.set_hl(r as u16);
    }

    /// The shared core of ADD SP,e8 and LD HL,SP+e8. Flags come from the low
    /// byte addition, which is the notorious part of these two instructions.
    fn add_sp_e8(&mut self, mmu: &Mmu) -> u16 {
        let e = self.imm8(mmu) as i8 as i16 as u16;
        let sp = self.reg.sp;
        self.reg.set_flag(FLAG_Z, false);
        self.reg.set_flag(FLAG_N, false);
        self.reg.set_flag(FLAG_H, (sp & 0xF) + (e & 0xF) > 0xF);
        self.reg.set_flag(FLAG_C, (sp & 0xFF) + (e & 0xFF) > 0xFF);
        sp.wrapping_add(e)
    }

    /// DAA adjusts A after a BCD add/subtract using the N/H/C flags.
    fn daa(&mut self) {
        let mut a = self.reg.a;
        let mut adjust = 0u8;
        let mut carry = self.reg.flag(FLAG_C);
        if self.reg.flag(FLAG_N) {
            if self.reg.flag(FLAG_H) {
                adjust |= 0x06;
            }
            if carry {
                adjust |= 0x60;
            }
            a = a.wrapping_sub(adjust);
        } else {
            if self.reg.flag(FLAG_H) || (a & 0x0F) > 0x09 {
                adjust |= 0x06;
            }
            if carry || a > 0x99 {
                adjust |= 0x60;
                carry = true;
            }
            a = a.wrapping_add(adjust);
        }
        self.reg.a = a;
        self.reg.set_flag(FLAG_Z, a == 0);
        self.reg.set_flag(FLAG_H, false);
        self.reg.set_flag(FLAG_C, carry);
    }

    // --- Control-flow helpers ----------------------------------------------

    fn jr(&mut self, mmu: &Mmu) {
        let e = self.imm8(mmu) as i8 as i16;
        self.reg.pc = (self.reg.pc as i16).wrapping_add(e) as u16;
    }
    fn cond(&self, sel: u8) -> bool {
        match sel {
            0 => !self.reg.flag(FLAG_Z), // NZ
            1 => self.reg.flag(FLAG_Z),  // Z
            2 => !self.reg.flag(FLAG_C), // NC
            3 => self.reg.flag(FLAG_C),  // C
            _ => unreachable!(),
        }
    }

    // --- The main decoder ---------------------------------------------------

    /// Decode and execute one instruction at PC. Returns T-cycles consumed.
    pub(super) fn execute(&mut self, mmu: &mut Mmu) -> u32 {
        let opcode = self.imm8(mmu);
        match opcode {
            // --- Misc / control ---
            0x00 => 4,                       // NOP
            0x10 => {
                // STOP. Its one real use for us is arming the CGB speed switch.
                self.imm8(mmu); // consume the padding byte
                if mmu.key1_prepare {
                    mmu.double_speed = !mmu.double_speed;
                    mmu.key1_prepare = false;
                }
                4
            }
            0x76 => { self.halted = true; 4 } // HALT
            0xF3 => { self.ime = false; self.ime_pending = false; 4 } // DI
            0xFB => { self.ime_pending = true; 4 }                    // EI
            0xCB => self.execute_cb(mmu),

            // --- 8-bit loads: LD r,d8 ---
            0x06 | 0x0E | 0x16 | 0x1E | 0x26 | 0x2E | 0x36 | 0x3E => {
                let sel = (opcode >> 3) & 7;
                let v = self.imm8(mmu);
                self.set_r(mmu, sel, v);
                if sel == 6 { 12 } else { 8 }
            }

            // --- LD r,r block (0x40-0x7F, excluding 0x76 HALT) ---
            0x40..=0x7F => {
                let dst = (opcode >> 3) & 7;
                let src = opcode & 7;
                let v = self.get_r(mmu, src);
                self.set_r(mmu, dst, v);
                if dst == 6 || src == 6 { 8 } else { 4 }
            }

            // --- 16-bit loads ---
            0x01 => { let v = self.imm16(mmu); self.reg.set_bc(v); 12 }
            0x11 => { let v = self.imm16(mmu); self.reg.set_de(v); 12 }
            0x21 => { let v = self.imm16(mmu); self.reg.set_hl(v); 12 }
            0x31 => { self.reg.sp = self.imm16(mmu); 12 }
            0x08 => {
                let addr = self.imm16(mmu);
                self.write8(mmu, addr, self.reg.sp as u8);
                self.write8(mmu, addr.wrapping_add(1), (self.reg.sp >> 8) as u8);
                20
            }
            0xF9 => { self.reg.sp = self.reg.hl(); 8 } // LD SP,HL
            0xF8 => { let v = self.add_sp_e8(mmu); self.reg.set_hl(v); 12 } // LD HL,SP+e8

            // --- Indirect loads to/from A ---
            0x02 => { self.write8(mmu, self.reg.bc(), self.reg.a); 8 }
            0x12 => { self.write8(mmu, self.reg.de(), self.reg.a); 8 }
            0x0A => { self.reg.a = self.read8(mmu, self.reg.bc()); 8 }
            0x1A => { self.reg.a = self.read8(mmu, self.reg.de()); 8 }
            0x22 => { let hl = self.reg.hl(); self.write8(mmu, hl, self.reg.a); self.reg.set_hl(hl.wrapping_add(1)); 8 }
            0x32 => { let hl = self.reg.hl(); self.write8(mmu, hl, self.reg.a); self.reg.set_hl(hl.wrapping_sub(1)); 8 }
            0x2A => { let hl = self.reg.hl(); self.reg.a = self.read8(mmu, hl); self.reg.set_hl(hl.wrapping_add(1)); 8 }
            0x3A => { let hl = self.reg.hl(); self.reg.a = self.read8(mmu, hl); self.reg.set_hl(hl.wrapping_sub(1)); 8 }

            // --- LDH / high-page and (a16) loads ---
            0xE0 => { let a = 0xFF00 + self.imm8(mmu) as u16; self.write8(mmu, a, self.reg.a); 12 }
            0xF0 => { let a = 0xFF00 + self.imm8(mmu) as u16; self.reg.a = self.read8(mmu, a); 12 }
            0xE2 => { let a = 0xFF00 + self.reg.c as u16; self.write8(mmu, a, self.reg.a); 8 }
            0xF2 => { let a = 0xFF00 + self.reg.c as u16; self.reg.a = self.read8(mmu, a); 8 }
            0xEA => { let a = self.imm16(mmu); self.write8(mmu, a, self.reg.a); 16 }
            0xFA => { let a = self.imm16(mmu); self.reg.a = self.read8(mmu, a); 16 }

            // --- 16-bit INC/DEC ---
            0x03 => { self.reg.set_bc(self.reg.bc().wrapping_add(1)); 8 }
            0x13 => { self.reg.set_de(self.reg.de().wrapping_add(1)); 8 }
            0x23 => { self.reg.set_hl(self.reg.hl().wrapping_add(1)); 8 }
            0x33 => { self.reg.sp = self.reg.sp.wrapping_add(1); 8 }
            0x0B => { self.reg.set_bc(self.reg.bc().wrapping_sub(1)); 8 }
            0x1B => { self.reg.set_de(self.reg.de().wrapping_sub(1)); 8 }
            0x2B => { self.reg.set_hl(self.reg.hl().wrapping_sub(1)); 8 }
            0x3B => { self.reg.sp = self.reg.sp.wrapping_sub(1); 8 }

            // --- 16-bit ADD HL,rr ---
            0x09 => { self.add_hl(self.reg.bc()); 8 }
            0x19 => { self.add_hl(self.reg.de()); 8 }
            0x29 => { self.add_hl(self.reg.hl()); 8 }
            0x39 => { self.add_hl(self.reg.sp); 8 }
            0xE8 => { self.reg.sp = self.add_sp_e8(mmu); 16 } // ADD SP,e8

            // --- 8-bit INC/DEC r ---
            0x04 | 0x0C | 0x14 | 0x1C | 0x24 | 0x2C | 0x34 | 0x3C => {
                let sel = (opcode >> 3) & 7;
                let v = self.get_r(mmu, sel);
                let r = self.inc8(v);
                self.set_r(mmu, sel, r);
                if sel == 6 { 12 } else { 4 }
            }
            0x05 | 0x0D | 0x15 | 0x1D | 0x25 | 0x2D | 0x35 | 0x3D => {
                let sel = (opcode >> 3) & 7;
                let v = self.get_r(mmu, sel);
                let r = self.dec8(v);
                self.set_r(mmu, sel, r);
                if sel == 6 { 12 } else { 4 }
            }

            // --- Accumulator rotates (Z always cleared) ---
            0x07 => { self.reg.a = self.rlc(self.reg.a); self.reg.set_flag(FLAG_Z, false); 4 } // RLCA
            0x0F => { self.reg.a = self.rrc(self.reg.a); self.reg.set_flag(FLAG_Z, false); 4 } // RRCA
            0x17 => { self.reg.a = self.rl(self.reg.a); self.reg.set_flag(FLAG_Z, false); 4 }  // RLA
            0x1F => { self.reg.a = self.rr(self.reg.a); self.reg.set_flag(FLAG_Z, false); 4 }  // RRA

            // --- Accumulator misc ---
            0x27 => { self.daa(); 4 }
            0x2F => { self.reg.a = !self.reg.a; self.reg.set_flag(FLAG_N, true); self.reg.set_flag(FLAG_H, true); 4 } // CPL
            0x37 => { self.reg.set_flag(FLAG_N, false); self.reg.set_flag(FLAG_H, false); self.reg.set_flag(FLAG_C, true); 4 } // SCF
            0x3F => { let c = self.reg.flag(FLAG_C); self.reg.set_flag(FLAG_N, false); self.reg.set_flag(FLAG_H, false); self.reg.set_flag(FLAG_C, !c); 4 } // CCF

            // --- ALU A,r block (0x80-0xBF) ---
            0x80..=0xBF => {
                let sel = opcode & 7;
                let v = self.get_r(mmu, sel);
                self.alu(opcode >> 3 & 7, v);
                if sel == 6 { 8 } else { 4 }
            }
            // --- ALU A,d8 ---
            0xC6 | 0xCE | 0xD6 | 0xDE | 0xE6 | 0xEE | 0xF6 | 0xFE => {
                let v = self.imm8(mmu);
                self.alu(opcode >> 3 & 7, v);
                8
            }

            // --- Jumps ---
            0xC3 => { self.reg.pc = self.imm16(mmu); 16 } // JP a16
            0xE9 => { self.reg.pc = self.reg.hl(); 4 }     // JP HL
            0xC2 | 0xCA | 0xD2 | 0xDA => {
                let addr = self.imm16(mmu);
                if self.cond(opcode >> 3 & 3) { self.reg.pc = addr; 16 } else { 12 }
            }
            0x18 => { self.jr(mmu); 12 } // JR e8
            0x20 | 0x28 | 0x30 | 0x38 => {
                if self.cond(opcode >> 3 & 3) { self.jr(mmu); 12 } else { self.imm8(mmu); 8 }
            }

            // --- Calls / returns / RST ---
            0xCD => { let addr = self.imm16(mmu); let pc = self.reg.pc; self.push16(mmu, pc); self.reg.pc = addr; 24 }
            0xC4 | 0xCC | 0xD4 | 0xDC => {
                let addr = self.imm16(mmu);
                if self.cond(opcode >> 3 & 3) {
                    let pc = self.reg.pc;
                    self.push16(mmu, pc);
                    self.reg.pc = addr;
                    24
                } else { 12 }
            }
            0xC9 => { self.reg.pc = self.pop16(mmu); 16 } // RET
            0xD9 => { self.reg.pc = self.pop16(mmu); self.ime = true; 16 } // RETI
            0xC0 | 0xC8 | 0xD0 | 0xD8 => {
                if self.cond(opcode >> 3 & 3) { self.reg.pc = self.pop16(mmu); 20 } else { 8 }
            }
            0xC7 | 0xCF | 0xD7 | 0xDF | 0xE7 | 0xEF | 0xF7 | 0xFF => {
                let pc = self.reg.pc;
                self.push16(mmu, pc);
                self.reg.pc = (opcode & 0x38) as u16; // RST vector
                16
            }

            // --- Stack push/pop ---
            0xC1 => { let v = self.pop16(mmu); self.reg.set_bc(v); 12 }
            0xD1 => { let v = self.pop16(mmu); self.reg.set_de(v); 12 }
            0xE1 => { let v = self.pop16(mmu); self.reg.set_hl(v); 12 }
            0xF1 => { let v = self.pop16(mmu); self.reg.set_af(v); 12 }
            0xC5 => { let v = self.reg.bc(); self.push16(mmu, v); 16 }
            0xD5 => { let v = self.reg.de(); self.push16(mmu, v); 16 }
            0xE5 => { let v = self.reg.hl(); self.push16(mmu, v); 16 }
            0xF5 => { let v = self.reg.af(); self.push16(mmu, v); 16 }

            // --- Undefined opcodes: behave as NOP-ish traps ---
            0xD3 | 0xDB | 0xDD | 0xE3 | 0xE4 | 0xEB | 0xEC | 0xED | 0xF4 | 0xFC | 0xFD => 4,
        }
    }

    /// Dispatch one of the eight ALU operations by its 3-bit selector.
    fn alu(&mut self, op: u8, v: u8) {
        let carry = self.reg.flag(FLAG_C);
        match op {
            0 => self.add_a(v, false),
            1 => self.add_a(v, carry),
            2 => self.sub_a(v, false),
            3 => self.sub_a(v, carry),
            4 => self.and_a(v),
            5 => self.xor_a(v),
            6 => self.or_a(v),
            7 => self.cp_a(v),
            _ => unreachable!(),
        }
    }

    // --- CB-prefixed opcodes: rotates, shifts, bit ops ----------------------

    fn execute_cb(&mut self, mmu: &mut Mmu) -> u32 {
        let opcode = self.imm8(mmu);
        let sel = opcode & 7;
        let v = self.get_r(mmu, sel);
        let hl = sel == 6;

        // 0x40-0x7F are BIT (read-only, and (HL) form is only 12 cycles).
        if (0x40..=0x7F).contains(&opcode) {
            let bit = (opcode >> 3) & 7;
            self.reg.set_flag(FLAG_Z, v & (1 << bit) == 0);
            self.reg.set_flag(FLAG_N, false);
            self.reg.set_flag(FLAG_H, true);
            return if hl { 12 } else { 8 };
        }

        let r = match opcode >> 3 {
            0x00 => self.rlc(v),
            0x01 => self.rrc(v),
            0x02 => self.rl(v),
            0x03 => self.rr(v),
            0x04 => self.sla(v),
            0x05 => self.sra(v),
            0x06 => self.swap(v),
            0x07 => self.srl(v),
            0x10..=0x17 => v & !(1 << ((opcode >> 3) & 7)), // RES
            0x18..=0x1F => v | (1 << ((opcode >> 3) & 7)),  // SET
            _ => unreachable!(),
        };
        self.set_r(mmu, sel, r);
        if hl { 16 } else { 8 }
    }

    // Rotate/shift primitives. All set Z from the result and clear N,H.
    fn rlc(&mut self, v: u8) -> u8 {
        let c = v >> 7;
        let r = v << 1 | c;
        self.shift_flags(r, c);
        r
    }
    fn rrc(&mut self, v: u8) -> u8 {
        let c = v & 1;
        let r = v >> 1 | c << 7;
        self.shift_flags(r, c);
        r
    }
    fn rl(&mut self, v: u8) -> u8 {
        let c = v >> 7;
        let r = v << 1 | self.reg.flag(FLAG_C) as u8;
        self.shift_flags(r, c);
        r
    }
    fn rr(&mut self, v: u8) -> u8 {
        let c = v & 1;
        let r = v >> 1 | (self.reg.flag(FLAG_C) as u8) << 7;
        self.shift_flags(r, c);
        r
    }
    fn sla(&mut self, v: u8) -> u8 {
        let c = v >> 7;
        let r = v << 1;
        self.shift_flags(r, c);
        r
    }
    fn sra(&mut self, v: u8) -> u8 {
        let c = v & 1;
        let r = (v >> 1) | (v & 0x80); // arithmetic: keep the sign bit
        self.shift_flags(r, c);
        r
    }
    fn srl(&mut self, v: u8) -> u8 {
        let c = v & 1;
        let r = v >> 1;
        self.shift_flags(r, c);
        r
    }
    fn swap(&mut self, v: u8) -> u8 {
        let r = v.rotate_left(4);
        self.reg.set_flag(FLAG_Z, r == 0);
        self.reg.set_flag(FLAG_N, false);
        self.reg.set_flag(FLAG_H, false);
        self.reg.set_flag(FLAG_C, false);
        r
    }
    fn shift_flags(&mut self, result: u8, carry: u8) {
        self.reg.set_flag(FLAG_Z, result == 0);
        self.reg.set_flag(FLAG_N, false);
        self.reg.set_flag(FLAG_H, false);
        self.reg.set_flag(FLAG_C, carry != 0);
    }
}
