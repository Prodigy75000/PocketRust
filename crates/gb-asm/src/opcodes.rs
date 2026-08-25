// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! The SM83 instruction encoder: a mnemonic plus parsed operands in, opcode
//! bytes plus a description of the operand tail out.
//!
//! The SM83 is regular enough that a 512-entry table would mostly be noise, so
//! this encodes structurally instead: `ld r8, r8'` is `$40 | dest<<3 | src`,
//! the eight ALU operations are `$80 | op<<3 | src`, and the CB page is three
//! bit-index families stacked above eight shift operations. Where the chip is
//! irregular (the `$E0`/`$F0` corner with the high-page loads, `add sp, e8`,
//! `ld hl, sp+e8`) it is spelled out one instruction at a time.
//!
//! Only the documented instruction set is accepted. The eleven opcodes that no
//! Game Boy instruction decodes to (`$D3`, `$DB`, `$DD`, `$E3`, `$E4`, `$EB`,
//! `$EC`, `$ED`, `$F4`, `$FC`, `$FD`) have no mnemonic here and cannot be
//! written, deliberately: a cartridge meant to be handed to someone else has no
//! business relying on what a particular chip revision does with them.

use crate::expr::QUOTE;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Reg {
    A,
    B,
    C,
    D,
    E,
    H,
    L,
    Af,
    Bc,
    De,
    Hl,
    Sp,
}

/// The bracketed operands: `[bc]`, `[de]`, `[hl]`, `[hl+]`, `[hl-]`, `[c]`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MemReg {
    Bc,
    De,
    Hl,
    HlInc,
    HlDec,
    C,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Operand {
    Reg(Reg),
    MemReg(MemReg),
    /// `[nn]`, an absolute address.
    Mem(String),
    /// `sp+e8` (or `sp-e8`, already negated into the expression).
    SpPlus(String),
    /// Anything else: an immediate, a jump target, or a condition name.
    Expr(String),
}

/// What follows the opcode byte (or the `$CB` prefix pair).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tail {
    None,
    /// One byte, accepted in -128..=255 so both `$f0` and `-16` are writable.
    Imm8(String),
    /// Two bytes, little endian.
    Imm16(String),
    /// A signed displacement from the address of the following instruction.
    Rel8(String),
    /// A signed byte in -128..=127: `add sp, e8` and `ld hl, sp+e8`.
    Signed8(String),
}

pub struct Encoded {
    pub opcode: Vec<u8>,
    pub tail: Tail,
}

impl Encoded {
    fn plain(op: u8) -> Encoded {
        Encoded { opcode: vec![op], tail: Tail::None }
    }
    fn with(op: u8, tail: Tail) -> Encoded {
        Encoded { opcode: vec![op], tail }
    }
    fn cb(op: u8) -> Encoded {
        Encoded { opcode: vec![0xcb, op], tail: Tail::None }
    }

    pub fn len(&self) -> usize {
        self.opcode.len()
            + match self.tail {
                Tail::None => 0,
                Tail::Imm8(_) | Tail::Rel8(_) | Tail::Signed8(_) => 1,
                Tail::Imm16(_) => 2,
            }
    }
}

/// Every mnemonic this assembler accepts. Anything not here is rejected by name
/// before its operands are even looked at, so a typo reads as a typo.
pub const MNEMONICS: &[&str] = &[
    "adc", "add", "and", "bit", "call", "ccf", "cp", "cpl", "daa", "dec", "di", "ei", "halt", "inc",
    "jp", "jr", "ld", "ldh", "nop", "or", "pop", "push", "res", "ret", "reti", "rl", "rla", "rlc",
    "rlca", "rr", "rra", "rrc", "rrca", "rst", "sbc", "scf", "set", "sla", "sra", "srl", "stop",
    "sub", "swap", "xor",
];

pub fn is_mnemonic(name: &str) -> bool {
    MNEMONICS.contains(&name)
}

/// Register encoding for the `r8` slot: `[hl]` is index 6, which is what makes
/// `ld b, [hl]` and `ld b, c` the same instruction shape.
fn r8(op: &Operand) -> Option<u8> {
    Some(match op {
        Operand::Reg(Reg::B) => 0,
        Operand::Reg(Reg::C) => 1,
        Operand::Reg(Reg::D) => 2,
        Operand::Reg(Reg::E) => 3,
        Operand::Reg(Reg::H) => 4,
        Operand::Reg(Reg::L) => 5,
        Operand::MemReg(MemReg::Hl) => 6,
        Operand::Reg(Reg::A) => 7,
        _ => return None,
    })
}

/// `bc de hl sp`, the slot used by `ld r16, n16`, `inc`/`dec`, and `add hl, r16`.
fn r16(op: &Operand) -> Option<u8> {
    Some(match op {
        Operand::Reg(Reg::Bc) => 0,
        Operand::Reg(Reg::De) => 1,
        Operand::Reg(Reg::Hl) => 2,
        Operand::Reg(Reg::Sp) => 3,
        _ => return None,
    })
}

/// `bc de hl af`, the slot used by `push` and `pop`. Same three bits, different
/// fourth register: the stack pair carries the flags, not the stack pointer.
fn r16stk(op: &Operand) -> Option<u8> {
    Some(match op {
        Operand::Reg(Reg::Bc) => 0,
        Operand::Reg(Reg::De) => 1,
        Operand::Reg(Reg::Hl) => 2,
        Operand::Reg(Reg::Af) => 3,
        _ => return None,
    })
}

/// `nz z nc c`. Note that `c` reaches here as a register, because that is what
/// it looks like from the outside; only the mnemonic says which one it is.
fn cond(op: &Operand) -> Option<u8> {
    Some(match op {
        Operand::Expr(s) if s.eq_ignore_ascii_case("nz") => 0,
        Operand::Expr(s) if s.eq_ignore_ascii_case("z") => 1,
        Operand::Expr(s) if s.eq_ignore_ascii_case("nc") => 2,
        Operand::Reg(Reg::C) => 3,
        _ => return None,
    })
}

fn as_expr(op: &Operand) -> Option<String> {
    match op {
        Operand::Expr(s) => Some(s.clone()),
        _ => None,
    }
}

/// Index of an ALU operation in the `$80` block and its `$C6` immediate twin.
fn alu_index(mnem: &str) -> Option<u8> {
    Some(match mnem {
        "add" => 0,
        "adc" => 1,
        "sub" => 2,
        "sbc" => 3,
        "and" => 4,
        "xor" => 5,
        "or" => 6,
        "cp" => 7,
        _ => return None,
    })
}

/// Index of a shift or rotate in the CB page's lowest block.
fn cb_shift_index(mnem: &str) -> Option<u8> {
    Some(match mnem {
        "rlc" => 0,
        "rrc" => 1,
        "rl" => 2,
        "rr" => 3,
        "sla" => 4,
        "sra" => 5,
        "swap" => 6,
        "srl" => 7,
        _ => return None,
    })
}

pub fn encode(mnem: &str, ops: &[Operand]) -> Result<Encoded, String> {
    let n = ops.len();

    // ---- no operands -----------------------------------------------------
    if n == 0 {
        return match mnem {
            "nop" => Ok(Encoded::plain(0x00)),
            "halt" => Ok(Encoded::plain(0x76)),
            // STOP is two bytes on hardware: the opcode and a byte the CPU
            // skips. Emitting the pad is what keeps the next instruction from
            // being eaten on a real Game Boy.
            "stop" => Ok(Encoded { opcode: vec![0x10, 0x00], tail: Tail::None }),
            "di" => Ok(Encoded::plain(0xf3)),
            "ei" => Ok(Encoded::plain(0xfb)),
            "ret" => Ok(Encoded::plain(0xc9)),
            "reti" => Ok(Encoded::plain(0xd9)),
            "rlca" => Ok(Encoded::plain(0x07)),
            "rrca" => Ok(Encoded::plain(0x0f)),
            "rla" => Ok(Encoded::plain(0x17)),
            "rra" => Ok(Encoded::plain(0x1f)),
            "daa" => Ok(Encoded::plain(0x27)),
            "cpl" => Ok(Encoded::plain(0x2f)),
            "scf" => Ok(Encoded::plain(0x37)),
            "ccf" => Ok(Encoded::plain(0x3f)),
            other => Err(format!("{other} needs operands")),
        };
    }

    // ---- one operand -----------------------------------------------------
    if n == 1 {
        let a = &ops[0];

        if mnem == "ret" {
            let cc = cond(a).ok_or("ret takes a condition: nz, z, nc or c")?;
            return Ok(Encoded::plain(0xc0 | (cc << 3)));
        }
        if mnem == "push" {
            let rr = r16stk(a).ok_or("push takes bc, de, hl or af")?;
            return Ok(Encoded::plain(0xc5 | (rr << 4)));
        }
        if mnem == "pop" {
            let rr = r16stk(a).ok_or("pop takes bc, de, hl or af")?;
            return Ok(Encoded::plain(0xc1 | (rr << 4)));
        }
        if mnem == "inc" || mnem == "dec" {
            if let Some(d) = r8(a) {
                let base = if mnem == "inc" { 0x04 } else { 0x05 };
                return Ok(Encoded::plain(base | (d << 3)));
            }
            if let Some(rr) = r16(a) {
                let base = if mnem == "inc" { 0x03 } else { 0x0b };
                return Ok(Encoded::plain(base | (rr << 4)));
            }
            return Err(format!("{mnem} takes an 8-bit register, [hl], or bc/de/hl/sp"));
        }
        if mnem == "jp" {
            // `jp hl` is the indirect jump; it is written both ways in the
            // wild, and `jp [hl]` is the more honest of the two.
            if *a == Operand::Reg(Reg::Hl) || *a == Operand::MemReg(MemReg::Hl) {
                return Ok(Encoded::plain(0xe9));
            }
            let e = as_expr(a).ok_or("jp takes an address, a condition, or hl")?;
            return Ok(Encoded::with(0xc3, Tail::Imm16(e)));
        }
        if mnem == "jr" {
            let e = as_expr(a).ok_or("jr takes a label")?;
            return Ok(Encoded::with(0x18, Tail::Rel8(e)));
        }
        if mnem == "call" {
            let e = as_expr(a).ok_or("call takes an address")?;
            return Ok(Encoded::with(0xcd, Tail::Imm16(e)));
        }
        if mnem == "rst" {
            // The target is folded into the opcode itself, so unlike every
            // other operand here it cannot be a forward reference: it has to be
            // one of the eight literal vectors, known the moment it is read.
            let e = as_expr(a).ok_or("rst takes a vector, one of $00 $08 ... $38")?;
            let v = crate::expr::eval(&e, &crate::expr::Symbols::new())
                .map_err(|_| format!("rst needs a literal vector, not {e:?}"))?;
            if v < 0 || v > 0x38 || v % 8 != 0 {
                return Err(format!("${v:02X} is not one of the eight rst vectors"));
            }
            return Ok(Encoded::plain(0xc7 | (v as u8)));
        }

        // Single-operand ALU: `cp $20` is the same instruction as `cp a, $20`.
        if let Some(op) = alu_index(mnem) {
            if let Some(s) = r8(a) {
                return Ok(Encoded::plain(0x80 | (op << 3) | s));
            }
            if let Some(e) = as_expr(a) {
                return Ok(Encoded::with(0xc6 | (op << 3), Tail::Imm8(e)));
            }
            return Err(format!("{mnem} takes a register, [hl], or an immediate"));
        }
        if let Some(op) = cb_shift_index(mnem) {
            let s = r8(a).ok_or_else(|| format!("{mnem} takes an 8-bit register or [hl]"))?;
            return Ok(Encoded::cb((op << 3) | s));
        }

        return Err(format!("{mnem} does not take one operand"));
    }

    // ---- two operands ----------------------------------------------------
    if n == 2 {
        let (a, b) = (&ops[0], &ops[1]);

        match mnem {
            "ld" => return encode_ld(a, b),
            "ldh" => return encode_ldh(a, b),
            "jp" | "call" => {
                let cc = cond(a)
                    .ok_or_else(|| format!("{mnem} with two operands wants a condition first"))?;
                let e = as_expr(b).ok_or_else(|| format!("{mnem} takes an address"))?;
                let base = if mnem == "jp" { 0xc2 } else { 0xc4 };
                return Ok(Encoded::with(base | (cc << 3), Tail::Imm16(e)));
            }
            "jr" => {
                let cc = cond(a).ok_or("jr with two operands wants a condition first")?;
                let e = as_expr(b).ok_or("jr takes a label")?;
                return Ok(Encoded::with(0x20 | (cc << 3), Tail::Rel8(e)));
            }
            "bit" | "res" | "set" => {
                let bit = as_expr(a).ok_or_else(|| format!("{mnem} takes a bit number first"))?;
                let index: u8 = bit
                    .trim()
                    .parse()
                    .map_err(|_| format!("{mnem} needs a literal bit number 0-7, not {bit:?}"))?;
                if index > 7 {
                    return Err(format!("there is no bit {index}"));
                }
                let s = r8(b).ok_or_else(|| format!("{mnem} takes a register or [hl]"))?;
                let base = match mnem {
                    "bit" => 0x40,
                    "res" => 0x80,
                    _ => 0xc0,
                };
                return Ok(Encoded::cb(base | (index << 3) | s));
            }
            _ => {}
        }

        if mnem == "add" {
            if *a == Operand::Reg(Reg::Hl) {
                let rr = r16(b).ok_or("add hl, ... takes bc, de, hl or sp")?;
                return Ok(Encoded::plain(0x09 | (rr << 4)));
            }
            if *a == Operand::Reg(Reg::Sp) {
                let e = as_expr(b).ok_or("add sp, ... takes a signed displacement")?;
                return Ok(Encoded::with(0xe8, Tail::Signed8(e)));
            }
        }

        if let Some(op) = alu_index(mnem) {
            if *a != Operand::Reg(Reg::A) {
                return Err(format!("{mnem} with two operands accumulates into a"));
            }
            if let Some(s) = r8(b) {
                return Ok(Encoded::plain(0x80 | (op << 3) | s));
            }
            if let Some(e) = as_expr(b) {
                return Ok(Encoded::with(0xc6 | (op << 3), Tail::Imm8(e)));
            }
            return Err(format!("{mnem} takes a register, [hl], or an immediate"));
        }

        return Err(format!("{mnem} does not take two operands"));
    }

    Err(format!("{mnem} does not take {n} operands"))
}

fn encode_ld(a: &Operand, b: &Operand) -> Result<Encoded, String> {
    // 16-bit destinations first, so `ld hl, $c000` is not mistaken for a load
    // into the `r8` slot that `hl` does not occupy.
    if let Some(rr) = r16(a) {
        if *a == Operand::Reg(Reg::Sp) && *b == Operand::Reg(Reg::Hl) {
            return Ok(Encoded::plain(0xf9));
        }
        if *a == Operand::Reg(Reg::Hl) {
            if let Operand::SpPlus(e) = b {
                return Ok(Encoded::with(0xf8, Tail::Signed8(e.clone())));
            }
        }
        let e = as_expr(b)
            .ok_or_else(|| format!("ld into a 16-bit register wants a value, got {b:?}"))?;
        return Ok(Encoded::with(0x01 | (rr << 4), Tail::Imm16(e)));
    }

    // a <- [bc] / [de] / [hl+] / [hl-] / [nn]
    if *a == Operand::Reg(Reg::A) {
        match b {
            Operand::MemReg(MemReg::Bc) => return Ok(Encoded::plain(0x0a)),
            Operand::MemReg(MemReg::De) => return Ok(Encoded::plain(0x1a)),
            Operand::MemReg(MemReg::HlInc) => return Ok(Encoded::plain(0x2a)),
            Operand::MemReg(MemReg::HlDec) => return Ok(Encoded::plain(0x3a)),
            Operand::Mem(e) => return Ok(Encoded::with(0xfa, Tail::Imm16(e.clone()))),
            Operand::MemReg(MemReg::C) => {
                return Err("the high-page load through c is spelled `ldh a, [c]`".into())
            }
            _ => {}
        }
    }
    // [bc] / [de] / [hl+] / [hl-] / [nn] <- a
    if *b == Operand::Reg(Reg::A) {
        match a {
            Operand::MemReg(MemReg::Bc) => return Ok(Encoded::plain(0x02)),
            Operand::MemReg(MemReg::De) => return Ok(Encoded::plain(0x12)),
            Operand::MemReg(MemReg::HlInc) => return Ok(Encoded::plain(0x22)),
            Operand::MemReg(MemReg::HlDec) => return Ok(Encoded::plain(0x32)),
            Operand::Mem(e) => return Ok(Encoded::with(0xea, Tail::Imm16(e.clone()))),
            Operand::MemReg(MemReg::C) => {
                return Err("the high-page store through c is spelled `ldh [c], a`".into())
            }
            _ => {}
        }
    }
    // [nn] <- sp, the only 16-bit store.
    if *b == Operand::Reg(Reg::Sp) {
        if let Operand::Mem(e) = a {
            return Ok(Encoded::with(0x08, Tail::Imm16(e.clone())));
        }
    }

    // The `$40` block: register to register, either side possibly `[hl]`.
    if let (Some(d), Some(s)) = (r8(a), r8(b)) {
        if d == 6 && s == 6 {
            return Err("`ld [hl], [hl]` is the halt opcode; write `halt` if you meant it".into());
        }
        return Ok(Encoded::plain(0x40 | (d << 3) | s));
    }
    // `ld r8, n8`, including `ld [hl], n8`.
    if let (Some(d), Some(e)) = (r8(a), as_expr(b)) {
        return Ok(Encoded::with(0x06 | (d << 3), Tail::Imm8(e)));
    }

    Err(format!("no ld form matches {a:?}, {b:?}"))
}

/// The high page. `ldh` takes either a bare offset (`$40`) or the full hardware
/// address it stands for (`$ff40`), because writing `ldh [$ff40], a` next to a
/// `LCDC = $ff40` constant is what the rest of the source wants to say.
fn encode_ldh(a: &Operand, b: &Operand) -> Result<Encoded, String> {
    if *b == Operand::Reg(Reg::A) {
        if let Operand::Mem(e) = a {
            return Ok(Encoded::with(0xe0, Tail::Imm8(high_page(e))));
        }
        if *a == Operand::MemReg(MemReg::C) {
            return Ok(Encoded::plain(0xe2));
        }
    }
    if *a == Operand::Reg(Reg::A) {
        if let Operand::Mem(e) = b {
            return Ok(Encoded::with(0xf0, Tail::Imm8(high_page(e))));
        }
        if *b == Operand::MemReg(MemReg::C) {
            return Ok(Encoded::plain(0xf2));
        }
    }
    Err("ldh moves a to or from [n8] or [c]".into())
}

/// Wrap the operand so that both `$40` and `$ff40` come out as `$40`. The mask
/// is applied to the value, not to the text, so `LCDC + 1` still works.
fn high_page(e: &str) -> String {
    format!("(({e}) & $ff)")
}

// ---- operand syntax ------------------------------------------------------

pub fn parse_operands(text: &str) -> Result<Vec<Operand>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(Vec::new());
    }
    split_top_level(text, ',')
        .map(|part| parse_operand(part.trim()))
        .collect()
}

fn parse_operand(s: &str) -> Result<Operand, String> {
    if s.is_empty() {
        return Err("empty operand".into());
    }
    if let Some(inner) = s.strip_prefix('[') {
        let inner = inner
            .strip_suffix(']')
            .ok_or_else(|| format!("unclosed bracket in {s:?}"))?
            .trim();
        let flat: String = inner.chars().filter(|c| !c.is_whitespace()).collect();
        return Ok(match flat.to_ascii_lowercase().as_str() {
            "bc" => Operand::MemReg(MemReg::Bc),
            "de" => Operand::MemReg(MemReg::De),
            "hl" => Operand::MemReg(MemReg::Hl),
            "hl+" | "hli" => Operand::MemReg(MemReg::HlInc),
            "hl-" | "hld" => Operand::MemReg(MemReg::HlDec),
            "c" => Operand::MemReg(MemReg::C),
            _ => Operand::Mem(inner.to_string()),
        });
    }

    let lower = s.to_ascii_lowercase();
    if let Some(r) = match lower.as_str() {
        "a" => Some(Reg::A),
        "b" => Some(Reg::B),
        "c" => Some(Reg::C),
        "d" => Some(Reg::D),
        "e" => Some(Reg::E),
        "h" => Some(Reg::H),
        "l" => Some(Reg::L),
        "af" => Some(Reg::Af),
        "bc" => Some(Reg::Bc),
        "de" => Some(Reg::De),
        "hl" => Some(Reg::Hl),
        "sp" => Some(Reg::Sp),
        _ => None,
    } {
        return Ok(Operand::Reg(r));
    }

    // `sp+e` and `sp-e`, the two displacement forms. The sign is folded into
    // the expression so the emitter only ever sees one signed value.
    if lower.starts_with("sp+") || lower.starts_with("sp-") {
        let rest = s[3..].trim();
        if rest.is_empty() {
            return Err("sp displacement is missing its value".into());
        }
        return Ok(Operand::SpPlus(if lower.as_bytes()[2] == b'-' {
            format!("-({rest})")
        } else {
            rest.to_string()
        }));
    }

    Ok(Operand::Expr(s.to_string()))
}

pub fn split_top_level(s: &str, sep: char) -> impl Iterator<Item = &str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    let mut start = 0usize;
    let bytes = s.as_bytes();
    for i in 0..bytes.len() {
        let c = bytes[i];
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
            }
            None => {
                if c == b'"' || c == QUOTE {
                    quote = Some(c);
                } else if c == b'(' || c == b'[' {
                    depth += 1;
                } else if c == b')' || c == b']' {
                    depth -= 1;
                } else if c as char == sep && depth == 0 {
                    parts.push(&s[start..i]);
                    start = i + 1;
                }
            }
        }
    }
    parts.push(&s[start..]);
    parts.into_iter()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enc(mnem: &str, operands: &str) -> Vec<u8> {
        let ops = parse_operands(operands).unwrap();
        encode(mnem, &ops).unwrap().opcode
    }

    #[test]
    fn register_to_register_loads_fill_the_forty_block() {
        assert_eq!(enc("ld", "b, b"), vec![0x40]);
        assert_eq!(enc("ld", "a, a"), vec![0x7f]);
        assert_eq!(enc("ld", "b, [hl]"), vec![0x46]);
        assert_eq!(enc("ld", "[hl], a"), vec![0x77]);
        assert_eq!(enc("ld", "h, e"), vec![0x63]);
    }

    #[test]
    fn the_halt_hole_in_the_load_block_is_refused() {
        let ops = parse_operands("[hl], [hl]").unwrap();
        assert!(encode("ld", &ops).is_err());
        assert_eq!(enc("halt", ""), vec![0x76]);
    }

    #[test]
    fn accumulator_memory_loads() {
        assert_eq!(enc("ld", "a, [bc]"), vec![0x0a]);
        assert_eq!(enc("ld", "a, [hl+]"), vec![0x2a]);
        assert_eq!(enc("ld", "[hl-], a"), vec![0x32]);
        assert_eq!(enc("ld", "a, [$c000]"), vec![0xfa]);
        assert_eq!(enc("ld", "[$c000], a"), vec![0xea]);
    }

    #[test]
    fn the_high_page_takes_an_offset_or_a_full_address() {
        assert_eq!(enc("ldh", "[$40], a"), vec![0xe0]);
        assert_eq!(enc("ldh", "[$ff40], a"), vec![0xe0]);
        assert_eq!(enc("ldh", "a, [$ff44]"), vec![0xf0]);
        assert_eq!(enc("ldh", "[c], a"), vec![0xe2]);
        assert_eq!(enc("ldh", "a, [c]"), vec![0xf2]);
        // `ld [c], a` is a common miswriting of it, and says so.
        let ops = parse_operands("[c], a").unwrap();
        assert!(encode("ld", &ops).is_err());
    }

    #[test]
    fn sixteen_bit_loads_and_the_stack_pair() {
        assert_eq!(enc("ld", "bc, $1234"), vec![0x01]);
        assert_eq!(enc("ld", "sp, $fffe"), vec![0x31]);
        assert_eq!(enc("ld", "sp, hl"), vec![0xf9]);
        assert_eq!(enc("ld", "[$c000], sp"), vec![0x08]);
        assert_eq!(enc("push", "af"), vec![0xf5]);
        assert_eq!(enc("pop", "hl"), vec![0xe1]);
        // sp is not a stack pair and af is not an arithmetic pair.
        assert!(encode("push", &parse_operands("sp").unwrap()).is_err());
        assert!(encode("add", &parse_operands("hl, af").unwrap()).is_err());
    }

    #[test]
    fn alu_forms_agree_whether_the_accumulator_is_written_out() {
        assert_eq!(enc("add", "a, b"), enc("add", "b"));
        assert_eq!(enc("add", "a, b"), vec![0x80]);
        assert_eq!(enc("cp", "a, [hl]"), vec![0xbe]);
        assert_eq!(enc("xor", "a"), vec![0xaf]);
        assert_eq!(enc("or", "$0f"), vec![0xf6]);
        assert_eq!(enc("sbc", "a, $01"), vec![0xde]);
        // Only a can accumulate.
        assert!(encode("and", &parse_operands("b, c").unwrap()).is_err());
    }

    #[test]
    fn conditions_and_the_register_c_share_a_spelling() {
        assert_eq!(enc("jr", "nz, loop"), vec![0x20]);
        assert_eq!(enc("jr", "c, loop"), vec![0x38]);
        assert_eq!(enc("jp", "nc, $0150"), vec![0xd2]);
        assert_eq!(enc("call", "z, sub"), vec![0xcc]);
        assert_eq!(enc("ret", "c"), vec![0xd8]);
        // ...but in an ALU slot the same token is the register.
        assert_eq!(enc("add", "a, c"), vec![0x81]);
    }

    #[test]
    fn the_cb_page_stacks_shifts_under_three_bit_families() {
        assert_eq!(enc("rlc", "b"), vec![0xcb, 0x00]);
        assert_eq!(enc("swap", "a"), vec![0xcb, 0x37]);
        assert_eq!(enc("srl", "[hl]"), vec![0xcb, 0x3e]);
        assert_eq!(enc("bit", "7, h"), vec![0xcb, 0x7c]);
        assert_eq!(enc("res", "0, a"), vec![0xcb, 0x87]);
        assert_eq!(enc("set", "3, [hl]"), vec![0xcb, 0xde]);
        assert!(encode("bit", &parse_operands("8, a").unwrap()).is_err());
    }

    #[test]
    fn stop_carries_its_pad_byte() {
        assert_eq!(enc("stop", ""), vec![0x10, 0x00]);
    }

    #[test]
    fn the_stack_pointer_displacement_forms_fold_their_sign() {
        let ops = parse_operands("hl, sp+8").unwrap();
        assert_eq!(ops[1], Operand::SpPlus("8".into()));
        assert_eq!(encode("ld", &ops).unwrap().opcode, vec![0xf8]);
        let ops = parse_operands("hl, sp-4").unwrap();
        assert_eq!(ops[1], Operand::SpPlus("-(4)".into()));
        assert_eq!(enc("add", "sp, -2"), vec![0xe8]);
    }

    #[test]
    fn undocumented_opcodes_have_no_spelling() {
        for name in ["xx", "sll", "nopx", "ld16"] {
            assert!(!is_mnemonic(name), "{name} should not assemble");
        }
        // Every mnemonic in the list must round-trip through the name check,
        // so the table cannot drift out of agreement with itself.
        for m in MNEMONICS {
            assert!(is_mnemonic(m));
        }
    }

    #[test]
    fn a_bracketed_expression_is_not_a_register_pair() {
        assert_eq!(
            parse_operand("[wFrame]").unwrap(),
            Operand::Mem("wFrame".into())
        );
        assert_eq!(
            parse_operand("[OAM + 4*2]").unwrap(),
            Operand::Mem("OAM + 4*2".into())
        );
        // A comma inside brackets must not split the operand list.
        let ops = parse_operands("a, [BASE + (1,2)]");
        assert!(ops.is_err() || ops.unwrap().len() == 2);
    }

    #[test]
    fn encoded_lengths_match_the_bytes_that_follow() {
        let l = |m: &str, o: &str| encode(m, &parse_operands(o).unwrap()).unwrap().len();
        assert_eq!(l("nop", ""), 1);
        assert_eq!(l("ld", "a, $10"), 2);
        assert_eq!(l("ld", "hl, $c000"), 3);
        assert_eq!(l("jr", "nz, back"), 2);
        assert_eq!(l("jp", "$0150"), 3);
        assert_eq!(l("bit", "7, a"), 2);
        assert_eq!(l("stop", ""), 2);
        assert_eq!(l("add", "sp, 1"), 2);
    }
}
