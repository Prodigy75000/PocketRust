// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! The assembler proper: source lines in, a Game Boy cartridge image out.
//!
//! Two passes. Pass one places every label; pass two emits bytes with those
//! labels final. Unlike a 6502 assembler there is no operand width to decide:
//! on the SM83 the mnemonic and the shape of its operands fix the instruction
//! length on sight, so a forward reference can never resize an instruction and
//! slide everything after it. Pass two still re-checks that no label moved,
//! because a `.res` or an `.align` fed by a forward reference could do it.

use crate::expr::{self, EvalError, Symbols, QUOTE};
use crate::opcodes::{self, Tail};

pub struct Options {
    /// Emit a `label = $addr` listing next to the ROM. Useful when reading a
    /// core trace back against the source.
    pub symbol_file: bool,
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum Section {
    /// Bytes land in the cartridge image.
    Rom,
    /// Addresses are handed out but nothing is written: this is how the work
    /// RAM map is declared, since a cartridge cannot ship its own RAM contents.
    Ram,
}

/// One source line, tagged with where it came from so errors can point at it.
struct Line {
    file: String,
    no: usize,
    text: String,
}

pub struct Output {
    pub rom: Vec<u8>,
    pub symbols: String,
    /// ROM bytes actually written, for the "how full is the cart" report.
    pub rom_used: usize,
    pub tiles_used: usize,
    pub header_checksum: u8,
    pub global_checksum: u16,
}

/// What `.gb` declared. These are the container fields, the Game Boy's
/// equivalent of an iNES header, and the assembler fills them in after the last
/// byte of the cartridge is placed.
struct Header {
    title: String,
    cgb: u8,
    sgb: u8,
    cart_type: u8,
    rom_bytes: usize,
    ram_code: u8,
    version: u8,
}

impl Default for Header {
    fn default() -> Header {
        Header {
            title: String::new(),
            cgb: 0x00,
            sgb: 0x00,
            cart_type: 0x00,
            rom_bytes: 32 * 1024,
            ram_code: 0x00,
            version: 0x00,
        }
    }
}

/// The container fields the assembler owns, $0134 through the two global
/// checksum bytes at $014F. Nothing in the source may write here.
const HEADER_START: usize = 0x0134;
const HEADER_END: usize = 0x0150;

struct Asm {
    syms: Symbols,
    header: Header,
    configured: bool,

    rom: Vec<u8>,
    written: Vec<bool>,

    section: Section,
    pc: u16,

    pass: usize,
    scope: String,
    in_tile: bool,
    tile_rows: Vec<[u8; 8]>,
    tiles: usize,
}

pub fn assemble(entry: &std::path::Path, opts: &Options) -> Result<Output, String> {
    let lines = load(entry)?;

    let mut a = Asm {
        syms: Symbols::new(),
        header: Header::default(),
        configured: false,
        rom: Vec::new(),
        written: Vec::new(),
        section: Section::Rom,
        pc: 0,
        pass: 1,
        scope: String::new(),
        in_tile: false,
        tile_rows: Vec::new(),
        tiles: 0,
    };

    for pass in 1..=2 {
        a.pass = pass;
        a.section = Section::Rom;
        a.scope.clear();
        a.in_tile = false;
        a.configured = false;
        a.pc = 0;
        a.tiles = 0;
        if pass == 2 {
            // Keep labels from pass one; wipe the image so pass two writes clean.
            a.rom.iter_mut().for_each(|b| *b = 0xff);
            a.written.iter_mut().for_each(|b| *b = false);
        }
        for line in &lines {
            a.line(line).map_err(|e| {
                format!("{}:{}: {}\n  | {}", line.file, line.no, e, line.text.trim())
            })?;
        }
        if a.in_tile {
            return Err("unterminated .tile block at end of source".into());
        }
    }

    if !a.configured {
        return Err("source never declared a cartridge with .gb".into());
    }

    let rom_used = a.written.iter().filter(|w| **w).count();
    if let Some(claimed) = a.written[HEADER_START..HEADER_END].iter().position(|w| *w) {
        return Err(format!(
            "${:04X} is inside the cartridge header, which .gb owns",
            HEADER_START + claimed
        ));
    }
    let mut rom = a.rom;
    write_header(&mut rom, &a.header);
    let header_checksum = rom[0x014d];
    let global_checksum = u16::from_be_bytes([rom[0x014e], rom[0x014f]]);

    let mut symbols = String::new();
    if opts.symbol_file {
        let mut names: Vec<_> = a.syms.iter().collect();
        names.sort_by(|x, y| x.1.cmp(y.1).then(x.0.cmp(y.0)));
        for (name, value) in names {
            symbols.push_str(&format!("{value:04X}  {name}\n"));
        }
    }

    Ok(Output {
        rom,
        symbols,
        rom_used,
        tiles_used: a.tiles,
        header_checksum,
        global_checksum,
    })
}

/// Fill in the fields the cartridge header carries about itself, then the two
/// checksums over what is now a finished image.
fn write_header(rom: &mut [u8], h: &Header) {
    // $0134-$0143: the title, padded with zeroes. On a colour cartridge the
    // last byte of that field is the CGB flag instead, which is why the title
    // is capped at fifteen characters rather than sixteen.
    let title = h.title.as_bytes();
    for i in 0..15 {
        rom[0x0134 + i] = title.get(i).copied().unwrap_or(0);
    }
    rom[0x0143] = h.cgb;
    // New licensee code. "00" is the code for "none", which is what an
    // independent cartridge is.
    rom[0x0144] = b'0';
    rom[0x0145] = b'0';
    rom[0x0146] = h.sgb;
    rom[0x0147] = h.cart_type;
    rom[0x0148] = match h.rom_bytes {
        0x8000 => 0,
        0x10000 => 1,
        0x20000 => 2,
        0x40000 => 3,
        0x80000 => 4,
        _ => 0,
    };
    rom[0x0149] = h.ram_code;
    rom[0x014a] = 0x01; // destination: not Japan
    rom[0x014b] = 0x33; // old licensee code $33 means "read $0144-$0145"
    rom[0x014c] = h.version;

    // Header checksum over $0134-$014C. The boot ROM refuses to hand over
    // control if this is wrong, so it is the one field that has teeth.
    let mut x: u8 = 0;
    for i in 0x0134..=0x014c {
        x = x.wrapping_sub(rom[i]).wrapping_sub(1);
    }
    rom[0x014d] = x;

    // Global checksum: a plain 16-bit sum of every other byte in the image.
    // No hardware checks it; it is here because the field exists and a zero
    // there is a tell that something skipped it.
    rom[0x014e] = 0;
    rom[0x014f] = 0;
    let sum: u16 = rom.iter().fold(0u16, |acc, &b| acc.wrapping_add(b as u16));
    rom[0x014e] = (sum >> 8) as u8;
    rom[0x014f] = sum as u8;
}

/// Read the entry file and splice in every `.include`, depth first.
fn load(path: &std::path::Path) -> Result<Vec<Line>, String> {
    let mut out = Vec::new();
    load_into(path, &mut out, 0)?;
    Ok(out)
}

fn load_into(path: &std::path::Path, out: &mut Vec<Line>, depth: usize) -> Result<(), String> {
    if depth > 8 {
        return Err(format!("include nested too deep at {}", path.display()));
    }
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let dir = path.parent().unwrap_or(std::path::Path::new("."));
    let name = path.display().to_string();

    for (i, raw) in text.lines().enumerate() {
        let no = i + 1;
        let code = strip_comment(raw);
        let trimmed = code.trim();
        if let Some(rest) = trimmed.strip_prefix(".include") {
            let inc = rest.trim().trim_matches('"');
            if inc.is_empty() {
                return Err(format!("{name}:{no}: .include needs a quoted path"));
            }
            load_into(&dir.join(inc), out, depth + 1)?;
            continue;
        }
        out.push(Line { file: name.clone(), no, text: code });
    }
    Ok(())
}

/// Drop a trailing comment, respecting char and string literals so a `;` inside
/// one survives.
fn strip_comment(line: &str) -> String {
    let b = line.as_bytes();
    let mut i = 0;
    let mut quote: Option<u8> = None;
    while i < b.len() {
        let c = b[i];
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
            }
            None => {
                if c == b';' {
                    return line[..i].to_string();
                }
                if c == b'"' || c == QUOTE {
                    quote = Some(c);
                }
            }
        }
        i += 1;
    }
    line.to_string()
}

impl Asm {
    fn line(&mut self, line: &Line) -> Result<(), String> {
        let text = line.text.clone();

        if self.in_tile {
            return self.tile_row(text.trim());
        }

        let mut rest = text.trim();
        if rest.is_empty() {
            return Ok(());
        }

        // Leading `label:` (or `@local:`).
        if let Some(colon) = top_level_colon(rest) {
            let label = rest[..colon].trim();
            self.define_label(label)?;
            rest = rest[colon + 1..].trim();
            if rest.is_empty() {
                return Ok(());
            }
        }

        // `NAME = expr` constant.
        if let Some(eq) = constant_split(rest) {
            let name = rest[..eq].trim().to_string();
            let value = self.value(rest[eq + 1..].trim())?.unwrap_or(0);
            return self.define(&name, value);
        }

        if rest.starts_with('.') {
            return self.directive(rest);
        }

        self.instruction(rest)
    }

    // ---- symbols ---------------------------------------------------------

    fn qualify(&self, name: &str) -> String {
        if name.starts_with('@') {
            format!("{}{}", self.scope, name)
        } else {
            name.to_string()
        }
    }

    fn define_label(&mut self, label: &str) -> Result<(), String> {
        if label.is_empty() {
            return Err("empty label".into());
        }
        if !label.starts_with('@') {
            self.scope = label.to_string();
        }
        let here = self.pc as i64;
        let name = self.qualify(label);
        self.define(&name, here)
    }

    fn define(&mut self, name: &str, value: i64) -> Result<(), String> {
        if self.pass == 1 {
            if self.syms.insert(name.to_string(), value).is_some() {
                return Err(format!("{name} defined twice"));
            }
        } else {
            // A label that moved between passes means something upstream of it
            // changed size. Catch it here rather than shipping a ROM that jumps
            // into the middle of an operand.
            match self.syms.get(name) {
                Some(&old) if old == value => {}
                Some(&old) => {
                    return Err(format!(
                        "phase error: {name} was ${old:04X} on pass 1, ${value:04X} on pass 2"
                    ))
                }
                None => return Err(format!("{name} appeared only on pass 2")),
            }
        }
        Ok(())
    }

    /// Rewrite `@local` references into their scoped form before evaluation, so
    /// two subroutines can each have their own `@loop` without colliding.
    fn qualify_expr(&self, src: &str) -> String {
        let b = src.as_bytes();
        let mut out = String::new();
        let mut i = 0;
        while i < b.len() {
            let c = b[i];
            if c == QUOTE || c == b'"' {
                out.push(c as char);
                i += 1;
                while i < b.len() {
                    let ch = b[i];
                    out.push(ch as char);
                    i += 1;
                    if ch == c {
                        break;
                    }
                }
                continue;
            }
            if expr::is_sym_start(c) {
                let start = i;
                while i < b.len() && expr::is_sym_char(b[i]) {
                    i += 1;
                }
                if c == b'@' {
                    out.push_str(&self.scope);
                }
                out.push_str(&src[start..i]);
                continue;
            }
            out.push(c as char);
            i += 1;
        }
        out
    }

    /// `Ok(None)` means "not resolvable yet", which is only tolerable on pass 1.
    fn value(&self, src: &str) -> Result<Option<i64>, String> {
        let src = &self.qualify_expr(src);
        match expr::eval(src, &self.syms) {
            Ok(v) => Ok(Some(v)),
            Err(EvalError::Unknown(n)) => {
                if self.pass == 1 {
                    Ok(None)
                } else {
                    Err(format!("undefined symbol: {n}"))
                }
            }
            Err(EvalError::Syntax(m)) => Err(m),
        }
    }

    fn value_now(&self, src: &str) -> Result<i64, String> {
        self.value(src)?
            .ok_or_else(|| format!("{src:?} must be known here, but is a forward reference"))
    }

    // ---- output ----------------------------------------------------------

    fn emit(&mut self, byte: u8) -> Result<(), String> {
        if self.section == Section::Ram {
            return Err(format!(
                "${:04X} is in the RAM map; a cartridge cannot ship bytes there",
                self.pc
            ));
        }
        let off = self.pc as usize;
        if off >= self.rom.len() {
            return Err(format!(
                "${:04X} is past the end of a {} KB cartridge",
                self.pc,
                self.rom.len() / 1024
            ));
        }
        if self.written[off] {
            return Err(format!("two things want to live at ${:04X}", self.pc));
        }
        self.rom[off] = byte;
        self.written[off] = true;
        self.pc = self.pc.wrapping_add(1);
        Ok(())
    }

    /// Advance without writing. Used by `.res` in the RAM map, where the point
    /// is only to hand out addresses.
    fn skip(&mut self, n: i64) -> Result<(), String> {
        let next = self.pc as i64 + n;
        if !(0..=0x10000).contains(&next) {
            return Err(format!("reserving {n} bytes from ${:04X} runs off the map", self.pc));
        }
        self.pc = next as u16;
        Ok(())
    }

    fn emit_byte_value(&mut self, v: i64, what: &str) -> Result<(), String> {
        if !(-128..=255).contains(&v) {
            return Err(format!("{what} value {v} does not fit in a byte"));
        }
        self.emit((v as i32 as u32 & 0xff) as u8)
    }

    // ---- directives ------------------------------------------------------

    fn directive(&mut self, rest: &str) -> Result<(), String> {
        let (name, args) = match rest.find(char::is_whitespace) {
            Some(i) => (&rest[..i], rest[i..].trim()),
            None => (rest, ""),
        };

        match name {
            ".gb" => self.d_gb(args),
            ".org" => {
                let v = self.value_now(args)?;
                if !(0..=0xffff).contains(&v) {
                    return Err(format!(".org ${v:X} is not a 16-bit address"));
                }
                self.section = Section::Rom;
                self.pc = v as u16;
                Ok(())
            }
            ".ram" => {
                let v = self.value_now(args)?;
                if !(0..=0xffff).contains(&v) {
                    return Err(format!(".ram ${v:X} is not a 16-bit address"));
                }
                self.section = Section::Ram;
                self.pc = v as u16;
                Ok(())
            }
            ".byte" | ".db" => self.d_bytes(args, 0),
            ".str" => self.d_bytes(args, 0x20),
            ".word" | ".dw" => self.d_words(args),
            ".res" => self.d_res(args),
            ".align" => self.d_align(args),
            ".assert" => self.d_assert(args),
            ".tile" => {
                if self.section != Section::Rom {
                    return Err(".tile has to go somewhere in the cartridge".into());
                }
                if !args.is_empty() {
                    self.define_label(args)?;
                }
                self.in_tile = true;
                self.tile_rows.clear();
                Ok(())
            }
            ".endtile" => Err(".endtile without .tile".into()),
            other => Err(format!("unknown directive {other}")),
        }
    }

    fn d_gb(&mut self, args: &str) -> Result<(), String> {
        let mut h = Header::default();
        let mut rom_kb = 32usize;

        for field in opcodes::split_top_level(args, ' ') {
            let field = field.trim();
            if field.is_empty() {
                continue;
            }
            let (k, v) = field
                .split_once('=')
                .ok_or_else(|| format!("bad .gb field {field:?}, want key=value"))?;
            let v = v.trim().trim_matches('"');
            match k.trim() {
                "title" => {
                    if v.len() > 15 || !v.bytes().all(|c| c.is_ascii_uppercase() || c == b' ') {
                        return Err(
                            "title is at most 15 characters, upper case and spaces only".into()
                        );
                    }
                    h.title = v.to_string();
                }
                "cgb" => {
                    h.cgb = match v {
                        // $80: uses colour where there is colour, still runs on
                        // a monochrome Game Boy. $C0 refuses to run on one.
                        "on" | "enhanced" => 0x80,
                        "only" => 0xc0,
                        "off" => 0x00,
                        _ => return Err("cgb must be on, only, or off".into()),
                    }
                }
                "sgb" => {
                    h.sgb = match v {
                        "on" => 0x03,
                        "off" => 0x00,
                        _ => return Err("sgb must be on or off".into()),
                    }
                }
                "mbc" => {
                    h.cart_type = match v {
                        "none" => 0x00,
                        "mbc1" => 0x01,
                        "mbc2" => 0x05,
                        "mbc3" => 0x11,
                        "mbc5" => 0x19,
                        _ => return Err("mbc must be none, mbc1, mbc2, mbc3 or mbc5".into()),
                    }
                }
                "rom" => {
                    rom_kb = v.parse().map_err(|_| "rom must be a number of KB")?;
                    if !matches!(rom_kb, 32 | 64 | 128 | 256 | 512) {
                        return Err("rom must be 32, 64, 128, 256 or 512 KB".into());
                    }
                }
                "version" => h.version = v.parse().map_err(|_| "version must be a number")?,
                other => return Err(format!("unknown .gb field {other}")),
            }
        }

        if h.cart_type == 0x00 && rom_kb != 32 {
            return Err("a cartridge with no mapper cannot be larger than 32 KB".into());
        }
        h.rom_bytes = rom_kb * 1024;

        if self.pass == 1 {
            self.rom = vec![0xff; h.rom_bytes];
            self.written = vec![false; h.rom_bytes];
        }
        self.header = h;
        self.configured = true;
        Ok(())
    }

    fn d_bytes(&mut self, args: &str, string_bias: u8) -> Result<(), String> {
        for item in opcodes::split_top_level(args, ',') {
            let item = item.trim();
            if item.is_empty() {
                continue;
            }
            if item.len() >= 2 && item.starts_with('"') && item.ends_with('"') {
                for ch in item[1..item.len() - 1].bytes() {
                    let v = ch
                        .checked_sub(string_bias)
                        .ok_or_else(|| format!("{:?} is below the character base", ch as char))?;
                    self.emit(v)?;
                }
            } else {
                let v = self.value(item)?.unwrap_or(0);
                self.emit_byte_value(v, ".byte")?;
            }
        }
        Ok(())
    }

    fn d_words(&mut self, args: &str) -> Result<(), String> {
        for item in opcodes::split_top_level(args, ',') {
            let item = item.trim();
            if item.is_empty() {
                continue;
            }
            let v = self.value(item)?.unwrap_or(0);
            if !(-32768..=65535).contains(&v) {
                return Err(format!(".word value {v} does not fit in 16 bits"));
            }
            let v = v as i32 as u32;
            self.emit((v & 0xff) as u8)?;
            self.emit(((v >> 8) & 0xff) as u8)?;
        }
        Ok(())
    }

    fn d_res(&mut self, args: &str) -> Result<(), String> {
        let mut parts = opcodes::split_top_level(args, ',');
        let count = self.value_now(parts.next().unwrap_or("").trim())?;
        if count < 0 {
            return Err(".res needs a non-negative count".into());
        }
        if self.section == Section::Ram {
            if parts.next().is_some() {
                return Err(".res in the RAM map cannot have a fill value".into());
            }
            return self.skip(count);
        }
        let fill = match parts.next() {
            Some(f) => self.value_now(f.trim())? as u8,
            None => 0,
        };
        for _ in 0..count {
            self.emit(fill)?;
        }
        Ok(())
    }

    fn d_align(&mut self, args: &str) -> Result<(), String> {
        let n = self.value_now(args)?;
        if n <= 0 {
            return Err(".align needs a positive boundary".into());
        }
        while (self.pc as i64) % n != 0 {
            if self.section == Section::Ram {
                self.skip(1)?;
            } else {
                self.emit(0xff)?;
            }
        }
        Ok(())
    }

    fn d_assert(&mut self, args: &str) -> Result<(), String> {
        // `.assert <expr>, "message"`. Evaluated on pass two, when every label
        // is final. This is how the source pins its own layout invariants:
        // "the font did not grow", "this table still tiles a full row".
        let mut parts = opcodes::split_top_level(args, ',');
        let cond = parts.next().unwrap_or("").trim().to_string();
        let msg = parts
            .next()
            .map(|m| m.trim().trim_matches('"').to_string())
            .unwrap_or_else(|| cond.clone());
        if self.pass != 2 {
            return Ok(());
        }
        if self.value_now(&cond)? == 0 {
            return Err(format!("assertion failed: {msg}"));
        }
        Ok(())
    }

    // ---- text-art tiles --------------------------------------------------

    fn tile_row(&mut self, row: &str) -> Result<(), String> {
        if row == ".endtile" {
            if self.tile_rows.len() != 8 {
                return Err(format!(
                    "a tile is 8 rows, this one has {}",
                    self.tile_rows.len()
                ));
            }
            self.in_tile = false;
            self.tiles += 1;
            let rows = std::mem::take(&mut self.tile_rows);
            // Two bitplanes, interleaved row by row: low bits of the eight
            // pixels, then their high bits, then the next row. That is the
            // Game Boy's tile format, and it is where it differs from the NES,
            // which keeps the two planes eight bytes apart.
            for r in &rows {
                for plane in 0..2 {
                    let mut byte = 0u8;
                    for (x, px) in r.iter().enumerate() {
                        if (px >> plane) & 1 != 0 {
                            byte |= 0x80 >> x;
                        }
                    }
                    self.emit(byte)?;
                }
            }
            return Ok(());
        }
        if row.is_empty() {
            return Ok(());
        }
        if row.len() != 8 {
            return Err(format!(
                "a tile row is 8 pixels, this one is {} ({row:?})",
                row.len()
            ));
        }
        let mut out = [0u8; 8];
        for (i, c) in row.bytes().enumerate() {
            out[i] = match c {
                b'.' | b'0' => 0,
                b'1' => 1,
                b'2' => 2,
                b'3' => 3,
                other => return Err(format!("tile pixels are . 1 2 3, not {:?}", other as char)),
            };
        }
        self.tile_rows.push(out);
        Ok(())
    }

    // ---- instructions ----------------------------------------------------

    fn instruction(&mut self, text: &str) -> Result<(), String> {
        let (mnem, operand) = match text.find(char::is_whitespace) {
            Some(i) => (text[..i].to_ascii_lowercase(), text[i..].trim()),
            None => (text.to_ascii_lowercase(), ""),
        };
        if !opcodes::is_mnemonic(&mnem) {
            return Err(format!("{mnem} is not a Game Boy instruction"));
        }
        if self.section != Section::Rom {
            return Err("code cannot go in the RAM map".into());
        }

        let ops = opcodes::parse_operands(operand)?;
        let enc = opcodes::encode(&mnem, &ops)?;

        let at = self.pc;
        let len = enc.len() as u16;
        for byte in &enc.opcode {
            self.emit(*byte)?;
        }

        match &enc.tail {
            Tail::None => {}
            Tail::Imm8(e) => {
                let v = self.value(e)?.unwrap_or(0);
                self.emit_byte_value(v, "immediate")?;
            }
            Tail::Imm16(e) => {
                let v = self.value(e)?.unwrap_or(0);
                if !(-32768..=65535).contains(&v) {
                    return Err(format!("${v:X} is not a 16-bit value"));
                }
                let v = v as i32 as u32;
                self.emit((v & 0xff) as u8)?;
                self.emit(((v >> 8) & 0xff) as u8)?;
            }
            Tail::Signed8(e) => {
                let v = self.value(e)?.unwrap_or(0);
                if !(-128..=127).contains(&v) {
                    return Err(format!("{v} is not a signed byte displacement"));
                }
                self.emit((v as i8) as u8)?;
            }
            Tail::Rel8(e) => {
                let next = at as i64 + len as i64;
                let target = self.value(e)?.unwrap_or(next);
                let delta = target - next;
                if !(-128..=127).contains(&delta) {
                    return Err(format!(
                        "jr to ${target:04X} is {delta} bytes away; only -128..127 reaches"
                    ));
                }
                self.emit((delta as i8) as u8)?;
            }
        }
        Ok(())
    }
}

/// Index of the `:` that ends a leading label, if there is one.
fn top_level_colon(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    if b.is_empty() || !expr::is_sym_start(b[0]) {
        return None;
    }
    let mut i = 0;
    while i < b.len() && expr::is_sym_char(b[i]) {
        i += 1;
    }
    if i < b.len() && b[i] == b':' {
        Some(i)
    } else {
        None
    }
}

/// Index of the `=` in a `NAME = expr` line, if that is what this line is.
fn constant_split(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    if b.is_empty() || !expr::is_sym_start(b[0]) {
        return None;
    }
    let mut i = 0;
    while i < b.len() && expr::is_sym_char(b[i]) {
        i += 1;
    }
    let mut j = i;
    while j < b.len() && b[j] == b' ' {
        j += 1;
    }
    // `=` but not `==`, and not `>=` / `<=` (which cannot start a line anyway).
    if j < b.len() && b[j] == b'=' && b.get(j + 1) != Some(&b'=') {
        Some(j)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_and_constant_lines_are_told_apart() {
        assert_eq!(top_level_colon("loop: ld a, 0"), Some(4));
        assert_eq!(top_level_colon("ld a, 0"), None);
        assert_eq!(constant_split("LCDC = $ff40"), Some(5));
        assert_eq!(constant_split("ld a, 0"), None);
        // A comparison inside an .assert is not a constant definition.
        assert_eq!(constant_split("TILES == 96"), None);
    }

    #[test]
    fn comments_stop_at_a_semicolon_but_not_inside_a_literal() {
        assert_eq!(strip_comment("ld a, 1 ; go").trim(), "ld a, 1");
        let src = format!(".byte {q};{q}, 2 ; real comment", q = QUOTE as char);
        assert_eq!(
            strip_comment(&src).trim(),
            format!(".byte {q};{q}, 2", q = QUOTE as char).trim()
        );
    }

    #[test]
    fn the_header_checksum_is_the_one_the_boot_rom_computes() {
        // The boot ROM's own routine, written out longhand against a known
        // header: a cartridge titled "A" with every other field as .gb sets it.
        let mut rom = vec![0u8; 0x8000];
        let mut h = Header::default();
        h.title = "A".into();
        write_header(&mut rom, &h);
        let mut expected: u8 = 0;
        for i in 0x0134..=0x014c {
            expected = expected.wrapping_sub(rom[i]).wrapping_sub(1);
        }
        assert_eq!(rom[0x014d], expected);
        // ...and it is not the trivially-passing zero.
        assert_ne!(rom[0x014d], 0);
    }

    #[test]
    fn the_global_checksum_excludes_its_own_two_bytes() {
        let mut rom = vec![0u8; 0x8000];
        rom[0x2000] = 0xff;
        let h = Header::default();
        write_header(&mut rom, &h);
        let stored = u16::from_be_bytes([rom[0x014e], rom[0x014f]]);
        let mut sum = 0u16;
        for (i, &b) in rom.iter().enumerate() {
            if i != 0x014e && i != 0x014f {
                sum = sum.wrapping_add(b as u16);
            }
        }
        assert_eq!(stored, sum);
    }
}
