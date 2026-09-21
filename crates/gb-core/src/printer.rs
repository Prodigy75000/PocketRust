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

/// Bands the printer's buffer holds before it reports itself full. Nine bands
/// of 16 pixels is 144 lines, one Game Boy screen.
const MAX_BANDS: usize = 9;

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
                0x00
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

        if self.buffer.len() >= MAX_BANDS * MAX_PAYLOAD {
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
        // Everything before the trailer is zero, which is what tells a game the
        // packet is being consumed rather than echoed.
        assert!(answers[..answers.len() - 2].iter().all(|&b| b == 0));
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
