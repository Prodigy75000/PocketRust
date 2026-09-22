// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! The Game Boy Printer, as a thing you can plug into the link port.
//!
//! The printer is a pure slave: the Game Boy is always the master and the
//! printer never starts a transfer. That makes it a very good fit for the
//! [`LinkCable`] trait, which was written for a second Game Boy and happens to
//! be exactly the shape a printer needs: `master_exchange` hands us the byte the
//! Game Boy clocked out and asks for ours, so the whole device is a byte at a
//! time state machine living in one function.
//!
//! # The protocol
//!
//! Every exchange is a packet:
//!
//! ```text
//!   $88 $33 | cmd | compression | len lo | len hi | payload | chk lo | chk hi | $00 | $00
//! ```
//!
//! The printer answers `$00` to everything up to the two trailer bytes, then
//! `$81` (it is alive) and then its status byte. The checksum is a plain 16-bit
//! sum of the command, the compression flag, the two length bytes and the
//! payload; the magic and the checksum itself are not in it.
//!
//! Commands are `$01` initialise, `$02` print, `$04` data, `$0F` read status.
//! `$08` is a break, which no game we have seen sends, and which is honoured
//! here by throwing the half-built packet away.
//!
//! Image data arrives as ordinary 2bpp tiles, sixteen bytes each, in tilemap
//! order, twenty tiles to a row. A full data packet is $280 bytes, which is
//! forty tiles, which is one band of 160 by 16 pixels.
//!
//! # What this deliberately does not do
//!
//! There is no paper, no battery and no heat, so the faults a real printer
//! reports are not simulated: nothing here ever sets paper jam, low battery or
//! other error. Checksum and packet errors are real and are reported, because
//! those are the ones a game can provoke and therefore the ones worth being
//! honest about.
//!
//! The printer is not part of a save state, the same as the link cable is not.
//! A state taken half way through a print comes back with an empty printer, so
//! the print is lost rather than corrupted.

use crate::serial::LinkCable;

/// What the printer answers on the first trailer byte to say it is there.
pub const DEVICE_ID: u8 = 0x81;

/// The largest payload a data packet may carry: one band of 160 by 16 pixels.
pub const MAX_PAYLOAD: usize = 0x280;

/// A printed page is always this wide. The printer's head is 160 dots across.
pub const WIDTH: usize = 160;

/// The printer's graphics buffer.
///
/// 8 KiB, which the documentation gives as "a maximum bitmap area of 160*200
/// pixels between prints". A single print is at most 20 by 18 tiles, which is
/// 160 by 144, or 5760 bytes, so a legal print never fills this; the flag is for
/// a game that sends more than the hardware can hold.
///
/// This was nine bands, reasoned from "144 lines is one Game Boy screen", and
/// that number was wrong in the worst possible way: 5760 bytes is EXACTLY nine
/// bands, so the Game Boy Camera, which prints a full screen, landed precisely
/// on the invented limit and was told the buffer was full when it was two
/// thirds empty. Pokemon Yellow prints five and seven bands and never came near
/// it, so the harness could not see the bug.
const BUFFER_BYTES: usize = 8 * 1024;

/// Status bits, from the published protocol.
mod status {
    pub const CHECKSUM_ERROR: u8 = 1 << 0;
    pub const PRINTING: u8 = 1 << 1;
    pub const IMAGE_FULL: u8 = 1 << 2;
    pub const UNPROCESSED: u8 = 1 << 3;
    pub const PACKET_ERROR: u8 = 1 << 4;
}

/// How many status reads a print stays "printing" for.
///
/// Real hardware takes seconds and games draw a progress bar while they wait.
/// Reporting the job finished instantly would work, but it skips the animation
/// the game wants to show. Counting down over a handful of status polls gives
/// games their progress bar and is guaranteed to terminate, which "when the
/// paper has moved" would not be.
const PRINTING_POLLS: u8 = 8;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    Magic1,
    Magic2,
    Command,
    Compression,
    LenLo,
    LenHi,
    Payload,
    ChecksumLo,
    ChecksumHi,
    Alive,
    Status,
}

/// One finished print.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sheet {
    pub width: usize,
    pub height: usize,
    /// One byte per pixel, each 0 to 3, darkest last. Row major from the top
    /// left. The printer's palette has already been applied.
    pub pixels: Vec<u8>,
    /// Paper fed before this sheet, in the printer's own units.
    pub margin_before: u8,
    /// Paper fed after it. **Zero here and zero on the next sheet's
    /// `margin_before` means the two are one continuous strip**, which is what
    /// a Pokedex entry is: several prints that belong end to end. A frontend
    /// that saves each sheet on its own turns one picture into confetti.
    pub margin_after: u8,
    pub palette: u8,
    pub exposure: u8,
    /// Copies asked for. Zero means "feed paper only", and produces no sheet.
    pub copies: u8,
}

/// An emulated Game Boy Printer on the end of the link cable.
pub struct Printer {
    phase: Phase,
    cmd: u8,
    compressed: bool,
    len: u16,
    got: u16,
    raw: Vec<u8>,
    checksum_want: u16,
    checksum_have: u16,
    status: u8,
    printing_left: u8,

    /// Tile data gathered from data packets, waiting for a print command.
    buffer: Vec<u8>,
    /// Finished prints, oldest first. Drain them with [`Printer::take_sheets`].
    sheets: Vec<Sheet>,
    /// Every packet the Game Boy has sent, as (command, payload length). Cheap,
    /// and the first thing worth looking at when a game says it cannot print.
    pub log: Vec<(u8, usize)>,
}

impl Default for Printer {
    fn default() -> Self {
        Self::new()
    }
}

impl Printer {
    pub fn new() -> Printer {
        Printer {
            phase: Phase::Magic1,
            cmd: 0,
            compressed: false,
            len: 0,
            got: 0,
            raw: Vec::new(),
            checksum_want: 0,
            checksum_have: 0,
            status: 0,
            printing_left: 0,
            buffer: Vec::new(),
            sheets: Vec::new(),
            log: Vec::new(),
        }
    }

    /// Finished prints, removed from the printer.
    pub fn take_sheets(&mut self) -> Vec<Sheet> {
        std::mem::take(&mut self.sheets)
    }

    pub fn has_sheets(&self) -> bool {
        !self.sheets.is_empty()
    }

    /// The byte to answer with, given the byte the Game Boy just clocked out.
    fn exchange(&mut self, b: u8) -> u8 {
        match self.phase {
            // Resynchronise on the magic rather than trusting the stream to be
            // aligned. A game that gives up half way through a packet, or a
            // frontend that attaches the printer mid-transfer, leaves junk in
            // front of the next $88 and a printer that cannot find its footing
            // again would stay deaf for the rest of the session.
            Phase::Magic1 => {
                if b == 0x88 {
                    self.phase = Phase::Magic2;
                }
                // Open bus, NOT $00, while no packet is in progress.
                //
                // This printer can be left plugged in permanently, so every game
                // that pokes the serial port for a cable meets it, not only the
                // ones that print. An unplugged Game Boy reads $FF back, and a
                // game looking for a partner uses exactly that to decide nobody
                // is there. Answering $00 while idle would tell Pokemon's Cable
                // Club that SOMETHING is on the wire, and the failure would land
                // on trading rather than on printing, which is a bad place for a
                // printer to cause a bug.
                //
                // A real printer does answer $00 here. The deviation is one byte,
                // only ever the first of a packet, and only when the printer was
                // not already mid-packet; games read the reply at the trailer,
                // not at the magic. Verified by printing a Pokedex entry with
                // this in place.
                0xFF
            }
            Phase::Magic2 => {
                if b == 0x33 {
                    self.phase = Phase::Command;
                    self.checksum_have = 0;
                    self.raw.clear();
                    self.got = 0;
                } else if b != 0x88 {
                    // Not the magic, and not the start of another attempt.
                    self.phase = Phase::Magic1;
                }
                0x00
            }
            Phase::Command => {
                self.cmd = b;
                self.checksum_have = self.checksum_have.wrapping_add(b as u16);
                self.phase = Phase::Compression;
                0x00
            }
            Phase::Compression => {
                self.compressed = b & 1 != 0;
                self.checksum_have = self.checksum_have.wrapping_add(b as u16);
                self.phase = Phase::LenLo;
                0x00
            }
            Phase::LenLo => {
                self.len = b as u16;
                self.checksum_have = self.checksum_have.wrapping_add(b as u16);
                self.phase = Phase::LenHi;
                0x00
            }
            Phase::LenHi => {
                self.len |= (b as u16) << 8;
                self.checksum_have = self.checksum_have.wrapping_add(b as u16);
                self.phase = if self.len == 0 {
                    Phase::ChecksumLo
                } else {
                    Phase::Payload
                };
                0x00
            }
            Phase::Payload => {
                self.raw.push(b);
                self.checksum_have = self.checksum_have.wrapping_add(b as u16);
                self.got += 1;
                if self.got >= self.len {
                    self.phase = Phase::ChecksumLo;
                }
                0x00
            }
            Phase::ChecksumLo => {
                self.checksum_want = b as u16;
                self.phase = Phase::ChecksumHi;
                0x00
            }
            Phase::ChecksumHi => {
                self.checksum_want |= (b as u16) << 8;
                self.phase = Phase::Alive;
                self.run_command();
                0x00
            }
            // The Game Boy sends $00 twice here and reads our two answers: are
            // you there, and how are you.
            Phase::Alive => {
                self.phase = Phase::Status;
                DEVICE_ID
            }
            Phase::Status => {
                self.phase = Phase::Magic1;
                let s = self.status;
                // The print finishes over a few polls so a game's progress bar
                // has something to show.
                if self.printing_left > 0 {
                    self.printing_left -= 1;
                    if self.printing_left == 0 {
                        self.status &= !status::PRINTING;
                    }
                }
                s
            }
        }
    }

    fn run_command(&mut self) {
        self.log.push((self.cmd, self.raw.len()));

        if self.checksum_want != self.checksum_have {
            // Say so and do nothing else. Acting on a packet we know arrived
            // wrong is how a printer prints garbage.
            self.status |= status::CHECKSUM_ERROR;
            return;
        }
        self.status &= !status::CHECKSUM_ERROR;

        match self.cmd {
            0x01 => {
                // Initialise: forget everything, including any complaint.
                self.buffer.clear();
                self.status = 0;
                self.printing_left = 0;
            }
            0x02 => self.start_print(),
            0x04 => self.take_data(),
            0x08 => {
                // Break. Throw away what has been gathered but stay attached.
                self.buffer.clear();
                self.status &= !status::UNPROCESSED;
            }
            0x0F => {} // Read status: the answer is sent below regardless.
            _ => self.status |= status::PACKET_ERROR,
        }
    }

    fn take_data(&mut self) {
        if self.raw.is_empty() {
            // An empty data packet is the game saying "that is all of it".
            self.status |= status::UNPROCESSED;
            return;
        }
        if self.raw.len() > MAX_PAYLOAD {
            self.status |= status::PACKET_ERROR;
            return;
        }

        let mut data = Vec::new();
        if self.compressed {
            decompress(&self.raw, &mut data);
        } else {
            data.extend_from_slice(&self.raw);
        }
        self.buffer.extend_from_slice(&data);

        if self.buffer.len() >= BUFFER_BYTES {
            self.status |= status::IMAGE_FULL;
        }
        self.status |= status::UNPROCESSED;
    }

    fn start_print(&mut self) {
        // Sheets, margins, palette, exposure.
        if self.raw.len() < 4 {
            self.status |= status::PACKET_ERROR;
            return;
        }
        let copies = self.raw[0];
        let margins = self.raw[1];
        let palette = self.raw[2];
        let exposure = self.raw[3];

        let before = margins >> 4;
        let after = margins & 0x0F;

        if !self.buffer.is_empty() && copies > 0 {
            let pixels = render(&self.buffer, palette);
            let height = pixels.len() / WIDTH;
            self.sheets.push(Sheet {
                width: WIDTH,
                height,
                pixels,
                margin_before: before,
                margin_after: after,
                palette,
                exposure,
                copies,
            });
        }

        self.buffer.clear();
        self.status &= !(status::UNPROCESSED | status::IMAGE_FULL);
        self.status |= status::PRINTING;
        self.printing_left = PRINTING_POLLS;
    }
}

impl LinkCable for Printer {
    fn set_output(&mut self, _byte: u8) {
        // What the Game Boy is presenting is its business. The printer only
        // ever answers a clock.
    }

    fn master_exchange(&mut self, out: u8) -> u8 {
        self.exchange(out)
    }

    fn poll_slave_input(&mut self) -> Option<u8> {
        // The printer is never the master, so there is never a byte it has
        // clocked into the Game Boy of its own accord.
        None
    }
}

/// How long the printer must be silent before a held page is given up on.
///
/// Measured rather than guessed: Pokemon Yellow's Pokedex entry is two print
/// commands 750 frames apart, twelve and a half seconds, because the second
/// page's eight data packets have to crawl over an 8192 Hz serial link. A flat
/// timeout would have to be longer than that and would then make every
/// abandoned print wait an age.
///
/// Silence is the better signal. Within a job the printer is never quiet for
/// more than about a second, so five seconds of nothing means the game has moved
/// on: the player cancelled, or reset, or the cartridge crashed.
pub const IDLE_FLUSH_FRAMES: u32 = 300;

/// Assembles print commands into printouts, live.
///
/// A page whose `margin_after` is zero is not finished: the game intends to
/// carry on printing onto the same piece of paper. Holding it until the
/// continuation arrives is the difference between one Pokedex entry and two
/// fragments.
///
/// This exists because [`stitch`] alone was not enough, and the way it was not
/// enough is worth recording. `stitch` takes a slice of pages and joins them,
/// which is correct when you have all of them; the libretro core called it once
/// a frame, so it never held more than a single page and joined nothing at all.
/// The command-line tool collected everything first and looked right. Same
/// function, opposite behaviour, and the divergence only showed up on a phone.
///
/// So `stitch` is now defined in terms of this, and there is one rule rather
/// than two that agree until they do not.
#[derive(Default)]
pub struct Spool {
    pending: Option<Sheet>,
    idle: u32,
}

impl Spool {
    pub const fn new() -> Spool {
        Spool {
            pending: None,
            idle: 0,
        }
    }

    /// Is a page being held for a continuation that has not arrived?
    pub fn is_holding(&self) -> bool {
        self.pending.is_some()
    }

    /// Feed one finished print command. Returns whatever is now complete, which
    /// is usually nothing or one printout, and occasionally two: a held page
    /// that turned out not to continue, followed by a self-contained one.
    pub fn push(&mut self, sheet: Sheet) -> Vec<Sheet> {
        self.idle = 0;
        let mut done = Vec::new();
        match self.pending.take() {
            // The held page said "no feed" and this one says "no feed before",
            // so they are one strip.
            Some(held) if sheet.margin_before == 0 => {
                let joined = join(held, sheet);
                self.hold_or_emit(joined, &mut done);
            }
            // The held page expected a continuation and this is not one. Let it
            // go as it stands rather than gluing unrelated pictures together.
            Some(held) => {
                done.push(held);
                self.hold_or_emit(sheet, &mut done);
            }
            None => self.hold_or_emit(sheet, &mut done),
        }
        done
    }

    fn hold_or_emit(&mut self, sheet: Sheet, done: &mut Vec<Sheet>) {
        if sheet.margin_after == 0 {
            self.pending = Some(sheet);
        } else {
            done.push(sheet);
        }
    }

    /// Call once a frame. `saw_traffic` is whether the printer received anything
    /// this frame. Returns a held page once the game has clearly stopped.
    pub fn tick(&mut self, saw_traffic: bool) -> Option<Sheet> {
        if saw_traffic {
            self.idle = 0;
            return None;
        }
        if self.pending.is_none() {
            return None;
        }
        self.idle += 1;
        if self.idle >= IDLE_FLUSH_FRAMES {
            self.idle = 0;
            return self.pending.take();
        }
        None
    }

    /// Give up anything held, for a game being unloaded or reset. A printout
    /// that arrives late is better than one that never arrives.
    pub fn flush(&mut self) -> Option<Sheet> {
        self.idle = 0;
        self.pending.take()
    }
}

fn join(mut a: Sheet, b: Sheet) -> Sheet {
    a.height += b.height;
    a.pixels.extend_from_slice(&b.pixels);
    a.margin_after = b.margin_after;
    a
}

/// Join the pages the printer was told not to feed paper between.
///
/// For when every page is already in hand; the live path uses [`Spool`], which
/// this is written in terms of so the two cannot drift apart.
///
/// A Pokedex entry is **two** print commands: the first ends with a paper feed
/// of zero and the second begins with one, and on real paper that means one
/// continuous strip. This is protocol knowledge rather than presentation, which
/// is why it lives here and not in each frontend.
pub fn stitch(sheets: &[Sheet]) -> Vec<Sheet> {
    let mut spool = Spool::new();
    let mut out = Vec::new();
    for s in sheets {
        out.extend(spool.push(s.clone()));
    }
    out.extend(spool.flush());
    out
}

impl Sheet {
    /// The page as a PNG, ready to write to a file or hand to an app.
    ///
    /// Written out here rather than pulled in, because the libretro core has no
    /// dependencies at all and a printed page is not a good enough reason to
    /// give it one. It is an indexed, two-bit image, which is exactly what the
    /// printer produces: 160 pixels is 40 bytes a row, so a full Pokedex strip
    /// is about eight kilobytes rather than the thirty it would be as
    /// greyscale. The deflate stream is stored blocks, so there is no compressor
    /// here either; PNG allows that and every decoder accepts it.
    pub fn to_png(&self) -> Vec<u8> {
        // The printer's four shades as paper actually looks: no ink to full.
        const INK: [[u8; 3]; 4] = [
            [0xFF, 0xFF, 0xFF],
            [0xA8, 0xA8, 0xA8],
            [0x54, 0x54, 0x54],
            [0x00, 0x00, 0x00],
        ];

        let w = self.width;
        let h = self.height;
        let row_bytes = w.div_ceil(4);

        // Each scanline is a filter byte (0, none) then four pixels per byte,
        // most significant pair first.
        let mut raw = Vec::with_capacity((row_bytes + 1) * h);
        for y in 0..h {
            raw.push(0);
            for xb in 0..row_bytes {
                let mut byte = 0u8;
                for i in 0..4 {
                    let x = xb * 4 + i;
                    let v = if x < w { self.pixels[y * w + x] & 3 } else { 0 };
                    byte |= v << (6 - i * 2);
                }
                raw.push(byte);
            }
        }

        let mut png = Vec::new();
        png.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);

        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&(w as u32).to_be_bytes());
        ihdr.extend_from_slice(&(h as u32).to_be_bytes());
        ihdr.extend_from_slice(&[2, 3, 0, 0, 0]); // 2 bits, indexed, no interlace
        chunk(&mut png, b"IHDR", &ihdr);

        let mut plte = Vec::with_capacity(12);
        for c in INK {
            plte.extend_from_slice(&c);
        }
        chunk(&mut png, b"PLTE", &plte);

        chunk(&mut png, b"IDAT", &zlib_stored(&raw));
        chunk(&mut png, b"IEND", &[]);
        png
    }
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    let mut crc_input = Vec::with_capacity(4 + body.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(body);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

/// A zlib stream of stored (uncompressed) deflate blocks.
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01]; // deflate, 32K window, no preset dictionary
    let mut chunks = data.chunks(0xFFFF).peekable();
    if data.is_empty() {
        out.extend_from_slice(&[0x01, 0x00, 0x00, 0xFF, 0xFF]);
    }
    while let Some(part) = chunks.next() {
        let last = chunks.peek().is_none();
        out.push(last as u8);
        out.extend_from_slice(&(part.len() as u16).to_le_bytes());
        out.extend_from_slice(&(!(part.len() as u16)).to_le_bytes());
        out.extend_from_slice(part);
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// A printer you can hand to a core and still hold on to.
///
/// [`GameBoy::connect_link`] takes ownership of whatever it is given, so a
/// printer passed straight in would be unreachable afterwards and its pages
/// unrecoverable. This is the same shape `LocalLink` already uses for the same
/// reason: clone it, give one away, keep the other.
#[derive(Clone, Default)]
pub struct PrinterHandle(std::rc::Rc<std::cell::RefCell<Printer>>);

impl PrinterHandle {
    pub fn new() -> PrinterHandle {
        PrinterHandle::default()
    }
    /// Finished pages, removed from the printer.
    pub fn take_sheets(&self) -> Vec<Sheet> {
        self.0.borrow_mut().take_sheets()
    }
    pub fn has_sheets(&self) -> bool {
        self.0.borrow().has_sheets()
    }
    /// Every packet the game has sent, as (command, payload length).
    pub fn log(&self) -> Vec<(u8, usize)> {
        self.0.borrow().log.clone()
    }
    /// How many packets have arrived. Cheap enough to ask every frame, which
    /// `log()` is not: the spool needs to know whether the game is still
    /// talking, not what it said.
    pub fn packet_count(&self) -> usize {
        self.0.borrow().log.len()
    }
}

impl LinkCable for PrinterHandle {
    fn set_output(&mut self, _byte: u8) {}
    fn master_exchange(&mut self, out: u8) -> u8 {
        self.0.borrow_mut().exchange(out)
    }
    fn poll_slave_input(&mut self) -> Option<u8> {
        None
    }
}

/// The printer's run-length encoding.
///
/// A control byte with bit 7 set starts a compressed run: the low seven bits
/// are the length **minus two**, and the byte after it is repeated that many
/// times. With bit 7 clear it is a literal run: the low seven bits are the
/// length **minus one**, and that many bytes follow verbatim.
///
/// A truncated run stops decoding rather than reading off the end. That can only
/// happen on a malformed packet, and half a picture is a better answer than a
/// panic in the middle of someone's game.
pub fn decompress(src: &[u8], out: &mut Vec<u8>) {
    let mut i = 0;
    while i < src.len() {
        let control = src[i];
        i += 1;
        if control & 0x80 != 0 {
            let run = (control & 0x7F) as usize + 2;
            let Some(&b) = src.get(i) else { return };
            i += 1;
            out.extend(std::iter::repeat(b).take(run));
        } else {
            let run = (control & 0x7F) as usize + 1;
            let end = (i + run).min(src.len());
            out.extend_from_slice(&src[i..end]);
            if end != i + run {
                return;
            }
            i = end;
        }
    }
}

/// Turn gathered tile data into pixels, with the printer's palette applied.
///
/// A final partial row of tiles is still drawn, and the rest of that row is left
/// at shade 0. That is deliberate and it is not the palette being skipped: it is
/// paper the printer never put ink on, and blank paper is blank whatever the
/// palette maps colour 0 to.
///
/// The data is ordinary 2bpp tiles, sixteen bytes each, laid out in tilemap
/// order twenty tiles to a row, so this is the same unpacking the PPU does with
/// the same bit order. The palette maps each two-bit value to a shade exactly as
/// BGP does: bits 1-0 give the shade for value 0, bits 3-2 for value 1, and so
/// on.
pub fn render(buffer: &[u8], palette: u8) -> Vec<u8> {
    let tiles = buffer.len() / 16;
    let tiles_across = WIDTH / 8;
    // Round up, so a final partial row of tiles is still drawn rather than
    // silently dropped.
    let rows_of_tiles = tiles.div_ceil(tiles_across);
    let height = rows_of_tiles * 8;
    let mut out = vec![0u8; WIDTH * height];

    for t in 0..tiles {
        let tx = (t % tiles_across) * 8;
        let ty = (t / tiles_across) * 8;
        for row in 0..8 {
            let lo = buffer[t * 16 + row * 2];
            let hi = buffer[t * 16 + row * 2 + 1];
            for col in 0..8 {
                let bit = 7 - col;
                let v = ((lo >> bit) & 1) | (((hi >> bit) & 1) << 1);
                let shade = (palette >> (v * 2)) & 0b11;
                out[(ty + row) * WIDTH + tx + col] = shade;
            }
        }
    }
    out
}

/// Build a packet the way a game does, for tests and for tools.
pub fn packet(cmd: u8, compressed: bool, payload: &[u8]) -> Vec<u8> {
    let mut p = vec![0x88, 0x33, cmd, compressed as u8];
    p.push((payload.len() & 0xFF) as u8);
    p.push((payload.len() >> 8) as u8);
    p.extend_from_slice(payload);
    let sum: u16 = p[2..].iter().fold(0u16, |a, &b| a.wrapping_add(b as u16));
    p.push((sum & 0xFF) as u8);
    p.push((sum >> 8) as u8);
    p.push(0x00); // alive
    p.push(0x00); // status
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Clock a whole packet through and return what the printer answered.
    fn send(p: &mut Printer, bytes: &[u8]) -> Vec<u8> {
        bytes.iter().map(|&b| p.exchange(b)).collect()
    }

    fn reply_of(answers: &[u8]) -> (u8, u8) {
        let n = answers.len();
        (answers[n - 2], answers[n - 1])
    }

    #[test]
    fn it_answers_an_init_packet_with_its_device_id_and_a_clean_status() {
        let mut p = Printer::new();
        let answers = send(&mut p, &packet(0x01, false, &[]));
        let (alive, st) = reply_of(&answers);
        assert_eq!(alive, DEVICE_ID, "the printer has to say it is there");
        assert_eq!(st, 0, "a fresh printer has nothing to complain about");

        // The very first byte is open bus, because until $88 arrives the printer
        // has no idea a packet is starting and a game poking the port for a
        // cable must see what an empty port looks like. See `Phase::Magic1`.
        assert_eq!(answers[0], 0xFF, "an idle printer has to look unplugged");

        // Everything after that and before the trailer is zero, which is what
        // tells a game the packet is being consumed rather than echoed.
        assert!(answers[1..answers.len() - 2].iter().all(|&b| b == 0));
    }

    #[test]
    fn an_idle_printer_is_indistinguishable_from_an_empty_port() {
        // The whole reason this printer can be left plugged in permanently. A
        // game hunting for a link partner sends its own handshake, not $88, and
        // has to get open bus back or it will think somebody is there.
        let mut p = Printer::new();
        let pokes = [0x00, 0x01, 0x60, 0xFE, 0x02, 0x81, 0x33];
        for b in pokes {
            assert_eq!(
                p.exchange(b),
                0xFF,
                "answering ${b:02X} with anything but open bus tells a game a                  cable is attached when one is not"
            );
        }
        // And it is still a printer afterwards.
        let answers = send(&mut p, &packet(0x01, false, &[]));
        assert_eq!(reply_of(&answers).0, DEVICE_ID);
    }

    #[test]
    fn a_bad_checksum_is_reported_and_the_packet_is_ignored() {
        let mut p = Printer::new();
        send(&mut p, &packet(0x01, false, &[]));

        let band = vec![0xFF; 64];
        let mut bad = packet(0x04, false, &band);
        // Corrupt the payload after the checksum was worked out.
        let n = bad.len();
        bad[8] ^= 0xFF;
        let answers = send(&mut p, &bad);
        let (_, st) = reply_of(&answers);
        assert_eq!(
            st & status::CHECKSUM_ERROR,
            status::CHECKSUM_ERROR,
            "a corrupted packet has to be reported"
        );
        assert!(
            p.buffer.is_empty(),
            "a packet known to be corrupt must not reach the image buffer"
        );
        assert_eq!(n, bad.len());
    }

    #[test]
    fn a_band_of_data_becomes_a_sheet_when_the_print_command_arrives() {
        let mut p = Printer::new();
        send(&mut p, &packet(0x01, false, &[]));

        // One full band: forty tiles, every pixel set to value 3.
        let band = vec![0xFF; MAX_PAYLOAD];
        let answers = send(&mut p, &packet(0x04, false, &band));
        let (_, st) = reply_of(&answers);
        assert_eq!(
            st & status::UNPROCESSED,
            status::UNPROCESSED,
            "data received but not printed is 'unprocessed'"
        );
        assert!(!p.has_sheets(), "data alone does not print anything");

        // The empty data packet a game sends to say that is all of it.
        send(&mut p, &packet(0x04, false, &[]));
        // Then print: one copy, default margins, the usual palette, default burn.
        send(&mut p, &packet(0x02, false, &[0x01, 0x13, 0xE4, 0x40]));

        let sheets = p.take_sheets();
        assert_eq!(sheets.len(), 1, "one print command, one sheet");
        let s = &sheets[0];
        assert_eq!(s.width, 160);
        assert_eq!(s.height, 16, "a band of $280 bytes is 160 by 16");
        assert_eq!(s.pixels.len(), 160 * 16);
        assert_eq!(s.margin_before, 1, "the high nibble of $13");
        assert_eq!(s.margin_after, 3, "the low nibble of $13");
        // $FF in both bit planes is colour 3, and palette $E4 maps 3 to 3.
        assert!(
            s.pixels.iter().all(|&v| v == 3),
            "an all-ones band prints solid"
        );
    }

    #[test]
    fn a_full_screen_print_does_not_report_the_buffer_full() {
        // The Game Boy Camera prints a whole 160x144 screen, which is nine
        // bands and 5760 bytes. The printer holds 8 KiB, so that is two thirds
        // of it and the buffer is NOT full.
        //
        // This existed as a bug: the limit was nine bands, reasoned from "144
        // lines is one screen", and a full-screen print landed exactly on it. On
        // a real device the Camera then sat on "transferring" with a full
        // progress bar forever. Pokemon Yellow prints five and seven bands, so
        // no test that only drove Yellow could ever have seen it.
        let mut p = Printer::new();
        send(&mut p, &packet(0x01, false, &[]));

        let band = vec![0xAA; MAX_PAYLOAD];
        let mut last = 0;
        for _ in 0..9 {
            last = reply_of(&send(&mut p, &packet(0x04, false, &band))).1;
        }
        assert_eq!(
            last & status::IMAGE_FULL,
            0,
            "a 160x144 print reported the buffer full; status {last:#010b}"
        );
        assert_eq!(
            last & status::UNPROCESSED,
            status::UNPROCESSED,
            "nine bands of data should be waiting to print"
        );

        // And the flag still works for a game that really does overrun it.
        for _ in 0..5 {
            last = reply_of(&send(&mut p, &packet(0x04, false, &band))).1;
        }
        assert_eq!(
            last & status::IMAGE_FULL,
            status::IMAGE_FULL,
            "fourteen bands is past 8 KiB and should report full"
        );
    }

    #[test]
    fn the_palette_is_applied_rather_than_ignored() {
        // A FULL row of tiles, twenty of them. Anything less leaves the rest of
        // the row as unwritten paper, and "all of it is zero" would then be true
        // whatever the palette did.
        let row = vec![0xFF; 20 * 16];
        let pixels = render(&row, 0b00011011);
        assert!(
            pixels.iter().all(|&v| v == 0),
            "%00011011 maps colour 3 to shade 0, so a solid band prints blank"
        );
        let pixels = render(&row, 0xE4);
        assert!(pixels.iter().all(|&v| v == 3), "identity palette changed it");
    }

    #[test]
    fn a_partial_row_of_tiles_leaves_the_rest_of_the_page_blank() {
        // Two tiles, not twenty. The rest of the row is paper nobody printed on,
        // and paper is no ink whatever the palette says about colour 0.
        let pixels = render(&vec![0xFF; 2 * 16], 0b00011011);
        assert_eq!(pixels.len(), WIDTH * 8);
        assert!(pixels[..16].iter().all(|&v| v == 0));
        assert!(
            pixels[16..WIDTH].iter().all(|&v| v == 0),
            "unprinted paper has to stay blank"
        );
    }

    #[test]
    fn printing_reports_busy_and_then_stops() {
        let mut p = Printer::new();
        send(&mut p, &packet(0x01, false, &[]));
        send(&mut p, &packet(0x04, false, &vec![0xFF; 64]));
        let answers = send(&mut p, &packet(0x02, false, &[0x01, 0x13, 0xE4, 0x40]));
        let (_, st) = reply_of(&answers);
        assert_eq!(st & status::PRINTING, status::PRINTING, "print says busy");

        // A game polls with $0F until it stops saying busy. This has to end.
        let mut polls = 0;
        loop {
            let answers = send(&mut p, &packet(0x0F, false, &[]));
            let (alive, st) = reply_of(&answers);
            assert_eq!(alive, DEVICE_ID);
            polls += 1;
            if st & status::PRINTING == 0 {
                break;
            }
            assert!(polls < 64, "the printer never stopped reporting busy");
        }
        assert!(polls > 1, "it finished so fast a game could not show it");
    }

    #[test]
    fn it_finds_the_magic_again_after_junk_on_the_wire() {
        let mut p = Printer::new();
        // A game that gave up mid-packet, or a printer attached part way
        // through a transfer, leaves exactly this in front of the next packet.
        send(&mut p, &[0x00, 0xFF, 0x88, 0x11, 0x42, 0x88]);
        let answers = send(&mut p, &packet(0x01, false, &[]));
        let (alive, st) = reply_of(&answers);
        assert_eq!(alive, DEVICE_ID, "the printer never recovered its footing");
        assert_eq!(st, 0);
    }

    #[test]
    fn compressed_data_unpacks_to_the_same_picture_as_plain_data() {
        // Bit 7 set is a run of (len - 2); clear is (len - 1) literal bytes.
        let mut out = Vec::new();
        decompress(&[0x80 | 0, 0xAB], &mut out);
        assert_eq!(out, vec![0xAB; 2], "a compressed run is biased by two");

        out.clear();
        decompress(&[0x00, 0x11], &mut out);
        assert_eq!(out, vec![0x11], "a literal run is biased by one");

        out.clear();
        decompress(&[0x02, 1, 2, 3, 0x81, 0xEE], &mut out);
        assert_eq!(out, vec![1, 2, 3, 0xEE, 0xEE, 0xEE]);

        // And the whole way through the printer, against the plain version.
        let plain = vec![0x5A; MAX_PAYLOAD];
        let mut squashed = Vec::new();
        for _ in 0..(MAX_PAYLOAD / 129) {
            squashed.push(0x80 | 127); // a run of 129
            squashed.push(0x5A);
        }
        let left = MAX_PAYLOAD % 129;
        if left >= 2 {
            squashed.push(0x80 | (left as u8 - 2));
            squashed.push(0x5A);
        }

        let print = |compressed: bool, payload: &[u8]| {
            let mut p = Printer::new();
            send(&mut p, &packet(0x01, false, &[]));
            send(&mut p, &packet(0x04, compressed, payload));
            send(&mut p, &packet(0x02, false, &[0x01, 0x13, 0xE4, 0x40]));
            p.take_sheets().pop().unwrap()
        };
        assert_eq!(
            print(true, &squashed).pixels,
            print(false, &plain).pixels,
            "the compressed and plain forms of the same band differ"
        );
    }

    #[test]
    fn a_truncated_compressed_run_stops_rather_than_reading_off_the_end() {
        let mut out = Vec::new();
        decompress(&[0x80 | 5], &mut out); // a run with no byte after it
        assert!(out.is_empty());
        out.clear();
        decompress(&[0x7F, 1, 2, 3], &mut out); // 128 literals, only 3 present
        assert_eq!(out, vec![1, 2, 3]);
    }

    fn page(before: u8, after: u8, height: usize) -> Sheet {
        Sheet {
            width: WIDTH,
            height,
            pixels: vec![1; WIDTH * height],
            margin_before: before,
            margin_after: after,
            palette: 0xE4,
            exposure: 0x40,
            copies: 1,
        }
    }

    #[test]
    fn the_spool_holds_a_page_that_says_it_continues() {
        let mut sp = Spool::new();

        // Page one of a Pokedex entry: no feed after, so not finished.
        assert!(sp.push(page(1, 0, 80)).is_empty(), "a continuing page emitted early");
        assert!(sp.is_holding());

        // It stays held across the twelve seconds Yellow really takes, as long
        // as the game is still talking.
        for _ in 0..750 {
            assert!(sp.tick(true).is_none(), "traffic should reset the idle count");
        }
        assert!(sp.is_holding());

        // Page two arrives and completes the printout.
        let done = sp.push(page(0, 3, 112));
        assert_eq!(done.len(), 1, "the two halves are one printout");
        assert_eq!(done[0].height, 192);
        assert_eq!(done[0].margin_before, 1, "outer feeds are kept");
        assert_eq!(done[0].margin_after, 3);
        assert!(!sp.is_holding());
    }

    #[test]
    fn a_page_that_is_never_continued_still_lands() {
        // The player cancelled, or the game reset. Late is better than never.
        let mut sp = Spool::new();
        assert!(sp.push(page(1, 0, 80)).is_empty());

        for _ in 0..(IDLE_FLUSH_FRAMES - 1) {
            assert!(sp.tick(false).is_none(), "gave up too early");
        }
        let out = sp.tick(false).expect("the held page was never released");
        assert_eq!(out.height, 80);
        assert!(!sp.is_holding());
        assert!(sp.tick(false).is_none(), "released it twice");
    }

    #[test]
    fn a_self_contained_page_is_emitted_at_once() {
        let mut sp = Spool::new();
        let done = sp.push(page(1, 3, 16));
        assert_eq!(done.len(), 1, "a page that feeds paper is finished");
        assert!(!sp.is_holding(), "nothing to wait for");
    }

    #[test]
    fn an_unrelated_page_does_not_get_glued_to_a_held_one() {
        let mut sp = Spool::new();
        sp.push(page(1, 0, 80)); // held
        // This one asks for a feed BEFORE it, so it is a new piece of paper.
        let done = sp.push(page(2, 3, 16));
        assert_eq!(done.len(), 2, "both should come out, separately");
        assert_eq!(done[0].height, 80, "the held one, as it stood");
        assert_eq!(done[1].height, 16, "and then the new one");
    }

    #[test]
    fn flush_releases_a_held_page_for_an_unloading_game() {
        let mut sp = Spool::new();
        sp.push(page(1, 0, 80));
        assert_eq!(sp.flush().map(|s| s.height), Some(80));
        assert_eq!(sp.flush().map(|s| s.height), None);
    }

    #[test]
    fn stitch_and_the_live_spool_agree() {
        // The bug this whole type exists for: the batch path joined and the live
        // path did not. They are the same rule now, so prove it on the same
        // input rather than trusting that they are.
        let pages = [page(1, 0, 80), page(0, 3, 112), page(1, 3, 16)];

        let batch = stitch(&pages);

        let mut sp = Spool::new();
        let mut live = Vec::new();
        for p in &pages {
            live.extend(sp.push(p.clone()));
        }
        live.extend(sp.flush());

        assert_eq!(batch.len(), live.len(), "batch and live disagree on count");
        for (b, l) in batch.iter().zip(&live) {
            assert_eq!(b.height, l.height, "batch and live disagree on a height");
            assert_eq!(b.pixels, l.pixels, "batch and live disagree on pixels");
        }
        assert_eq!(batch.len(), 2, "three commands, two printouts");
    }

    #[test]
    fn a_page_encodes_to_a_png_a_decoder_would_accept() {
        let s = Sheet {
            width: WIDTH,
            height: 16,
            pixels: (0..WIDTH * 16).map(|i| (i % 4) as u8).collect(),
            margin_before: 1,
            margin_after: 0,
            palette: 0xE4,
            exposure: 0x40,
            copies: 1,
        };
        let png = s.to_png();

        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        // Chunks in order, each with a length and a type we can find.
        for (i, kind) in [b"IHDR", b"PLTE", b"IDAT", b"IEND"].iter().enumerate() {
            assert!(
                png.windows(4).any(|w| w == kind.as_slice()),
                "chunk {i} {:?} is missing",
                std::str::from_utf8(kind.as_slice()).unwrap()
            );
        }
        // The size and format the header claims.
        assert_eq!(&png[16..20], &(WIDTH as u32).to_be_bytes());
        assert_eq!(&png[20..24], &16u32.to_be_bytes());
        assert_eq!(&png[24..29], &[2, 3, 0, 0, 0], "2 bits, indexed");

        // Walk the chunks the way a decoder does and check every CRC, which is
        // what actually proves this is a file rather than a plausible-looking
        // pile of bytes.
        let mut at = 8;
        let mut kinds = Vec::new();
        while at + 8 <= png.len() {
            let len = u32::from_be_bytes(png[at..at + 4].try_into().unwrap()) as usize;
            let kind = &png[at + 4..at + 8];
            let body_end = at + 8 + len;
            let want = u32::from_be_bytes(png[body_end..body_end + 4].try_into().unwrap());
            assert_eq!(
                crc32(&png[at + 4..body_end]),
                want,
                "bad CRC on {:?}",
                std::str::from_utf8(kind).unwrap()
            );
            kinds.push(String::from_utf8_lossy(kind).to_string());
            at = body_end + 4;
        }
        assert_eq!(at, png.len(), "trailing bytes after the last chunk");
        assert_eq!(kinds, ["IHDR", "PLTE", "IDAT", "IEND"]);
    }

    #[test]
    fn the_checksums_in_the_png_writer_agree_with_known_answers() {
        // Without this, a broken crc32 would agree with itself above and the
        // chunk walk would pass on nonsense.
        assert_eq!(crc32(b""), 0x0000_0000);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(adler32(b""), 1);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn zero_copies_feeds_paper_without_printing_anything() {
        let mut p = Printer::new();
        send(&mut p, &packet(0x01, false, &[]));
        send(&mut p, &packet(0x04, false, &vec![0xFF; 64]));
        send(&mut p, &packet(0x02, false, &[0x00, 0x13, 0xE4, 0x40]));
        assert!(
            !p.has_sheets(),
            "a print of zero copies is a paper feed, not a page"
        );
    }
}
