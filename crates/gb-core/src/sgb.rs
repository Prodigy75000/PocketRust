//! Super Game Boy command capture + palette decoding.
//!
//! SGB-enhanced carts send commands to the SGB by pulsing the P14/P15 lines of
//! the joypad register (0xFF00): writing $00 resets the bit stream, then each
//! bit is clocked by pulsing exactly one line low ($20 = P14 low = 0, $10 = P15
//! low = 1) with $30 (both high) between bits. Bits are LSB-first; 128 bits make
//! one 16-byte packet. The first packet's header byte is `command << 3 | length`
//! where `length` is the packet count of the whole command.
//!
//! We decode the palette commands (PAL01/23/03/12) here so DMG games that carry
//! an SGB palette can display it. ATTR_* (per-region palette) and PAL_TRN (VRAM
//! palette tables) come later; for now the last-set SGB palette 0 is applied.

/// The Game Boy screen in 8x8 tiles: 20 across, 18 down.
pub const ATTR_W: usize = 20;
pub const ATTR_H: usize = 18;
const ATTR_TILES: usize = ATTR_W * ATTR_H;

/// Expand a 15-bit BGR555 SGB color (little-endian in the packet) to 0x00RRGGBB.
fn bgr555(lo: u8, hi: u8) -> u32 {
    let v = (lo as u16) | ((hi as u16) << 8);
    let r = (v & 0x1F) as u32;
    let g = ((v >> 5) & 0x1F) as u32;
    let b = ((v >> 10) & 0x1F) as u32;
    let e = |c: u32| (c << 3) | (c >> 2); // 5 -> 8 bit
    (e(r) << 16) | (e(g) << 8) | e(b)
}

pub struct Sgb {
    /// Whether the cart declares SGB support (header 0x146 == 0x03). Fixed by
    /// the ROM, unlike `enabled`, which the player can turn off.
    ///
    /// Not to be confused with the private `supported` below, which is about
    /// whether the inline-palette path still applies to this cartridge.
    pub declared: bool,
    /// Whether we ANSWER as an SGB. Separate from `supported` because
    /// answering is a commitment: a cartridge that detects an SGB goes on to
    /// send VRAM transfers and expects them to complete, so a half
    /// implementation is worse than none. See `Mmu::new`.
    pub enabled: bool,
    /// True once any command has been received (SGB actually in use).
    pub active: bool,

    // --- pulse-protocol decode state ---
    ready: bool,      // armed to latch the next bit (after $00 or $30)
    bit_count: u16,   // bits into the current 16-byte packet
    cur: [u8; 16],    // packet being assembled
    data: Vec<u8>,    // all packet bytes of the in-flight command
    packets_got: usize,
    expected: usize,

    // --- multiplayer (MLT_REQ) read-back state ---
    /// Number of controllers reported to the game (1, 2, or 4).
    player_count: u8,
    /// Which controller the game is currently reading (0-based).
    player_index: u8,
    /// Last P14/P15 select bits written, to detect the both-high rising edge
    /// that advances the controller counter.
    last_sel: u8,

    /// The four SGB palettes (SGB0..3), each 4 colors, resolved to RGB888.
    pub palettes: [[u32; 4]; 4],
    /// Set when the palette override may have changed, so the MMU re-syncs it
    /// into the PPU. Cleared by [`Sgb::take_palette_override`].
    palette_dirty: bool,
    /// Cleared once the cart uses a palette path we do not implement (PAL_TRN /
    /// PAL_SET transfer real colors through VRAM; the inline PAL we captured is
    /// then just a black placeholder). When false we stop overriding and let the
    /// normal colorization show, instead of blanking the screen. DK, Mole Mania.
    supported: bool,
    /// A VRAM-transfer command just arrived, and which one. The MMU hands us
    /// the 4 KiB the cartridge has prepared.
    ///
    /// Pan Docs: the SNES reads this off the display scanlines, but "will
    /// automatically re-produce the same ordering of bits and bytes, as being
    /// originally stored at 8000-8FFF in Game Boy memory". So the data IS VRAM
    /// $8000-$8FFF and there is nothing to decode from pixels.
    transfer_pending: Option<u8>,
    /// PAL_TRN's 512 system palettes, four BGR555 colours each. Not displayed
    /// directly: PAL_SET copies four of them into the visible palettes.
    sys_palettes: Box<[[u32; 4]; 512]>,
    /// MASK_EN: 0 none, 1 freeze the last frame, 2 black, 3 colour 0.
    mask: u8,
    /// Which of the four palettes each 8x8 tile of the screen uses, 20 by 18.
    ///
    /// The SGB does not colour a Game Boy screen with one palette; it colours
    /// it with four, chosen per tile by the ATTR_ commands. Applying palette 0
    /// everywhere is what made Pokemon Blue come up red: Blue's four palettes
    /// are three reds and a blue, and the blue is palette 3.
    attr: [u8; ATTR_TILES],
    /// Has the attribute map changed since the PPU last took it?
    attr_dirty: bool,

    /// Debug: (command code, total data bytes) of each completed command.
    pub log: Vec<(u8, usize)>,
}

impl Sgb {
    pub fn new(sgb_flag: u8) -> Sgb {
        Sgb {
            declared: sgb_flag == 0x03,
            enabled: sgb_flag == 0x03,
            active: false,
            ready: false,
            bit_count: 0,
            cur: [0; 16],
            data: Vec::new(),
            packets_got: 0,
            expected: 0,
            player_count: 1,
            player_index: 0,
            last_sel: 0x30,
            palettes: [[0xFFFFFF, 0xAAAAAA, 0x555555, 0x000000]; 4],
            palette_dirty: false,
            supported: true,
            transfer_pending: None,
            attr: [0; ATTR_TILES],
            attr_dirty: false,
            sys_palettes: Box::new([[0; 4]; 512]),
            mask: 0,
            log: Vec::new(),
        }
    }

    /// Whether a VRAM transfer just started (consumes the flag). The PPU freezes
    /// the display briefly so the transfer's on-screen data isn't shown.
    pub fn take_transfer(&mut self) -> Option<u8> {
        self.transfer_pending.take()
    }

    /// The screen mask the cartridge asked for: 0 none, 1 freeze, 2 black,
    /// 3 colour 0.
    pub fn mask(&self) -> u8 {
        self.mask
    }

    /// The per-tile palette map, if it has changed since the last call.
    pub fn take_attr(&mut self) -> Option<[u8; ATTR_TILES]> {
        if !self.attr_dirty {
            return None;
        }
        self.attr_dirty = false;
        Some(self.attr)
    }

    /// All four visible palettes, for the PPU to index with the map.
    pub fn palettes(&self) -> &[[u32; 4]; 4] {
        &self.palettes
    }

    /// Paint one rectangle's worth of attributes.
    ///
    /// `ctrl` bit 0 changes the inside, bit 1 the surrounding line, bit 2 the
    /// outside. The spec's exception matters and is easy to miss: "When
    /// changing only the Inside or Outside, then the Surrounding line becomes
    /// automatically changed to same color."
    fn attr_block(&mut self, ctrl: u8, pals: u8, x1: u8, y1: u8, x2: u8, y2: u8) {
        let (inside, line, outside) = (ctrl & 1 != 0, ctrl & 2 != 0, ctrl & 4 != 0);
        let p_in = pals & 0x03;
        let p_line = (pals >> 2) & 0x03;
        let p_out = (pals >> 4) & 0x03;
        // The exception above, both ways round.
        let (line, p_line) = if line {
            (true, p_line)
        } else if inside && !outside {
            (true, p_in)
        } else if outside && !inside {
            (true, p_out)
        } else {
            (false, p_line)
        };
        let (x1, x2) = (x1.min(x2) as usize, x2.max(x1) as usize);
        let (y1, y2) = (y1.min(y2) as usize, y2.max(y1) as usize);
        for ty in 0..ATTR_H {
            for tx in 0..ATTR_W {
                let on_edge = (tx == x1 || tx == x2) && (y1..=y2).contains(&ty)
                    || (ty == y1 || ty == y2) && (x1..=x2).contains(&tx);
                let within = (x1..=x2).contains(&tx) && (y1..=y2).contains(&ty);
                let pal = if on_edge {
                    if !line {
                        continue;
                    }
                    p_line
                } else if within {
                    if !inside {
                        continue;
                    }
                    p_in
                } else {
                    if !outside {
                        continue;
                    }
                    p_out
                };
                self.attr[ty * ATTR_W + tx] = pal;
            }
        }
        self.attr_dirty = true;
    }

    /// Consume the 4 KiB a `_TRN` command was waiting for.
    ///
    /// `data` is VRAM $8000-$8FFF. Only the transfers we act on are decoded;
    /// the rest are accepted and dropped, which is still the right answer,
    /// because what hangs a cartridge is a transfer that never completes
    /// rather than one whose contents go unused.
    pub fn consume_transfer(&mut self, cmd: u8, data: &[u8]) {
        if data.len() < 0x1000 {
            return;
        }
        if cmd == 0x0B {
            // PAL_TRN: 512 palettes of four little-endian BGR555 colours.
            for i in 0..512 {
                for c in 0..4 {
                    let o = i * 8 + c * 2;
                    self.sys_palettes[i][c] = bgr555(data[o], data[o + 1]);
                }
            }
            // The cartridge has now given us real colours, so the inline
            // placeholder path is no longer the best we can do.
            self.supported = true;
            self.palette_dirty = true;
        }
    }


    /// If the palette override may have changed since the last call, hand back
    /// the new state: `Some(Some(pal))` to override colorization, `Some(None)`
    /// to stop overriding (fall back to colorization), or `None` if unchanged.
    pub fn take_palette_override(&mut self) -> Option<Option<crate::colorize::DmgPalette>> {
        if !self.palette_dirty {
            return None;
        }
        self.palette_dirty = false;
        Some(self.palette_override())
    }

    /// The palette to force, or None to leave colorization alone. We only drive
    /// output from the inline PAL commands; if the cart uses transferred palettes
    /// (`supported` is false) or set a degenerate all-one-color placeholder, we
    /// bow out so the screen shows real colors instead of black. Palette 0 is
    /// applied globally for now (per-region ATTR is a later step).
    fn palette_override(&self) -> Option<crate::colorize::DmgPalette> {
        if !self.supported {
            return None;
        }
        let p = self.palettes[0];
        if p.iter().all(|&c| c == p[0]) {
            return None;
        }
        Some(crate::colorize::DmgPalette {
            bg: p,
            obj0: p,
            obj1: p,
        })
    }

    /// The low nibble the joypad register should report when both rows are
    /// deselected: the active controller's ID ($0F=P1, $0E=P2, ...). For a
    /// single player (or a non-SGB cart) this is the usual $0F.
    pub fn player_id_nibble(&self) -> u8 {
        if self.enabled && self.player_count > 1 {
            0x0F - self.player_index
        } else {
            0x0F
        }
    }

    /// Fed every write to the joypad register (0xFF00).
    pub fn write_p1(&mut self, val: u8) {
        if !self.enabled {
            return;
        }
        let sel = val & 0x30;
        // Multiplayer: a P15 (bit 5) low->high edge advances to the next
        // controller. The game clocks this between per-player reads.
        if self.player_count > 1 && sel & 0x20 != 0 && self.last_sel & 0x20 == 0 {
            self.player_index = (self.player_index + 1) % self.player_count;
        }
        self.last_sel = sel;
        match sel {
            0x00 => {
                // Reset: start assembling a fresh packet.
                self.bit_count = 0;
                self.cur = [0; 16];
                self.ready = true;
            }
            0x30 => self.ready = true,
            low => {
                if self.ready {
                    self.ready = false;
                    let bit = if low == 0x20 { 0u8 } else { 1u8 }; // P14 low=0, P15 low=1
                    let byte = (self.bit_count / 8) as usize;
                    let pos = (self.bit_count % 8) as u8;
                    if byte < 16 {
                        self.cur[byte] |= bit << pos;
                    }
                    self.bit_count += 1;
                    if self.bit_count == 128 {
                        self.finish_packet();
                    }
                }
            }
        }
    }

    fn finish_packet(&mut self) {
        self.data.extend_from_slice(&self.cur);
        self.packets_got += 1;
        if self.packets_got == 1 {
            self.expected = (self.data[0] & 0x07).max(1) as usize;
        }
        if self.packets_got >= self.expected {
            let cmd = self.data[0] >> 3;
            self.log.push((cmd, self.data.len()));
            self.active = true;
            self.dispatch(cmd);
            self.data.clear();
            self.packets_got = 0;
            self.expected = 0;
        }
    }

    fn dispatch(&mut self, cmd: u8) {
        match cmd {
            // PAL01/23/03/12: set two palettes from one packet. Color 0 (bytes
            // 1-2) is shared by all four palettes; then 3 colors per palette.
            0x00 | 0x01 | 0x02 | 0x03 => {
                let (a, b) = match cmd {
                    0x00 => (0, 1),
                    0x01 => (2, 3),
                    0x02 => (0, 3),
                    _ => (1, 2),
                };
                let d = &self.data;
                let c0 = bgr555(d[1], d[2]);
                let read = |base: usize| {
                    [
                        c0,
                        bgr555(d[base], d[base + 1]),
                        bgr555(d[base + 2], d[base + 3]),
                        bgr555(d[base + 4], d[base + 5]),
                    ]
                };
                self.palettes[a] = read(3);
                self.palettes[b] = read(9);
                self.palette_dirty = true;
            }
            // MLT_REQ: enable N-controller multiplayer. This is the SGB-detection
            // handshake: once we answer with the player-ID read-back, the game
            // knows it is on an SGB and proceeds to send its palette commands.
            0x11 => {
                self.player_count = match self.data[1] & 0x03 {
                    0x01 => 2,
                    0x03 => 4,
                    _ => 1,
                };
                self.player_index = 0;
            }
            // ATTR_BLK: colour attributes for one or more rectangles. Data
            // sets are six bytes each and run on across packets.
            0x04 => {
                let sets = (self.data[1] as usize).min(0x12);
                for i in 0..sets {
                    let o = 2 + i * 6;
                    if o + 5 >= self.data.len() {
                        break;
                    }
                    let d = &self.data;
                    let (ctrl, pals) = (d[o] & 0x07, d[o + 1]);
                    let (x1, y1, x2, y2) = (d[o + 2], d[o + 3], d[o + 4], d[o + 5]);
                    self.attr_block(ctrl, pals, x1, y1, x2, y2);
                }
            }
            // PAL_SET: copy four of PAL_TRN's 512 system palettes into the
            // visible ones. Before the transfer could be read this had to give
            // up and fall back to colorization; now it is the real thing.
            0x0A => {
                for slot in 0..4 {
                    let idx = u16::from_le_bytes([self.data[1 + slot * 2], self.data[2 + slot * 2]])
                        as usize
                        & 0x1FF;
                    self.palettes[slot] = self.sys_palettes[idx];
                }
                self.palette_dirty = true;
            }
            // Every VRAM transfer: PAL_TRN, SOU_TRN, CHR_TRN, PCT_TRN,
            // ATTR_TRN, OBJ_TRN. The MMU hands the data back through
            // `consume_transfer`.
            0x09 | 0x0B | 0x13 | 0x14 | 0x15 | 0x18 => self.transfer_pending = Some(cmd),
            // MASK_EN: freeze, blacken or blank the screen until cancelled.
            //
            // Honouring this used to be unsafe, because a cartridge masks the
            // screen while it transfers and cancels once the SGB has the data.
            // With transfers never completing, a cancel that depended on them
            // might never come and the screen stayed stuck, so the mask was
            // ignored and a blanket 90-frame blank stood in for it. Now that
            // the transfers complete, the cartridge's own cancel arrives and
            // the guess is not needed.
            0x17 => self.mask = self.data[1] & 0x03,
            _ => {} // ATTR_BLK/LIN/DIV/CHR, DATA_SND and friends: not yet
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Clock a 16-byte SGB packet in over the P14/P15 pulse protocol.
    fn send(sgb: &mut Sgb, bytes: &[u8; 16]) {
        sgb.write_p1(0x00); // reset + start packet
        for i in 0..128 {
            let bit = (bytes[i / 8] >> (i % 8)) & 1;
            sgb.write_p1(if bit == 0 { 0x20 } else { 0x10 }); // latch the bit
            sgb.write_p1(0x30); // re-arm for the next bit
        }
    }

    #[test]
    fn mlt_req_handshake_reports_two_players() {
        let mut sgb = Sgb::new(0x03);
        let mut pkt = [0u8; 16];
        pkt[0] = (0x11 << 3) | 1; // MLT_REQ, 1 packet
        pkt[1] = 0x01; // two players
        send(&mut sgb, &pkt);

        // The read-back cycles P1 / P2 as the game clocks P15 low->high edges.
        assert_eq!(sgb.player_id_nibble(), 0x0F); // player 1
        sgb.write_p1(0x10);
        sgb.write_p1(0x30);
        assert_eq!(sgb.player_id_nibble(), 0x0E); // player 2
        sgb.write_p1(0x10);
        sgb.write_p1(0x30);
        assert_eq!(sgb.player_id_nibble(), 0x0F); // wraps back to player 1
    }

    #[test]
    fn pal01_decodes_and_flags_a_palette_update() {
        let mut sgb = Sgb::new(0x03);
        let mut pkt = [0u8; 16];
        pkt[0] = (0x00 << 3) | 1; // PAL01, 1 packet
        pkt[1] = 0x1F; // color 0 = BGR555 pure red (lo)
        pkt[2] = 0x00; // (hi)
        send(&mut sgb, &pkt);

        assert_eq!(sgb.palettes[0][0], 0xFF0000);
        assert_eq!(sgb.palettes[1][0], 0xFF0000); // shared color 0
        let upd = sgb
            .take_palette_override()
            .expect("PAL command marks dirty")
            .expect("non-degenerate palette overrides");
        assert_eq!(upd.bg[0], 0xFF0000);
        assert!(sgb.take_palette_override().is_none()); // dirty flag cleared
    }

    #[test]
    fn pal_trn_then_pal_set_gives_the_cartridge_its_real_colours() {
        // This replaces a test that asserted the opposite. Before the VRAM
        // transfer could be read, PAL_TRN had to make the core GIVE UP on
        // palettes and fall back to colorization, because the inline palette a
        // cart leaves behind is often a black placeholder and keeping it turned
        // the screen black (Donkey Kong). That workaround was the best answer
        // available and is now the wrong one.
        let mut sgb = Sgb::new(0x03);

        // PAL_TRN, then the 4 KiB it was waiting for: 512 palettes of four
        // little-endian BGR555 colours. Put a recognisable red in palette 3.
        let mut trn = [0u8; 16];
        trn[0] = (0x0B << 3) | 1;
        send(&mut sgb, &trn);
        assert_eq!(sgb.take_transfer(), Some(0x0B), "PAL_TRN must ask for data");

        let mut data = vec![0u8; 0x1000];
        // Palette 3, colour 1 = pure red. BGR555 little-endian: R in bits 0-4.
        let o = 3 * 8 + 1 * 2;
        data[o] = 0x1F;
        data[o + 1] = 0x00;
        sgb.consume_transfer(0x0B, &data);

        // PAL_SET: put system palette 3 into visible slot 0.
        let mut set = [0u8; 16];
        set[0] = (0x0A << 3) | 1;
        set[1..3].copy_from_slice(&3u16.to_le_bytes());
        send(&mut sgb, &set);

        let pal = sgb
            .take_palette_override()
            .expect("PAL_SET must change the palette")
            .expect("and must supply colours rather than bowing out");
        assert_eq!(
            pal.bg[1], 0x00FF_0000,
            "colour 1 should be the red placed in system palette 3"
        );
    }

    #[test]
    fn a_transfer_command_asks_for_its_data() {
        // Every _TRN must raise the request. A cartridge masks the screen,
        // transfers, and cancels the mask when it is done; a transfer that is
        // never consumed is what left 52 games blank.
        for cmd in [0x09u8, 0x0B, 0x13, 0x14, 0x15, 0x18] {
            let mut sgb = Sgb::new(0x03);
            let mut pkt = [0u8; 16];
            pkt[0] = (cmd << 3) | 1;
            send(&mut sgb, &pkt);
            assert_eq!(sgb.take_transfer(), Some(cmd), "command ${cmd:02X}");
            assert_eq!(sgb.take_transfer(), None, "and only once");
        }
    }

    #[test]
    fn mask_en_is_honoured_and_cancellable() {
        // Ignoring MASK_EN was the other half of the old workaround: a mask the
        // cartridge never cancelled would stick, and cancels depended on
        // transfers that never completed.
        let mut sgb = Sgb::new(0x03);
        assert_eq!(sgb.mask(), 0);
        for mode in [1u8, 2, 3, 0] {
            let mut pkt = [0u8; 16];
            pkt[0] = (0x17 << 3) | 1;
            pkt[1] = mode;
            send(&mut sgb, &pkt);
            assert_eq!(sgb.mask(), mode);
        }
    }

    #[test]
    fn non_sgb_cart_ignores_pulses() {
        let mut sgb = Sgb::new(0x00);
        let mut pkt = [0u8; 16];
        pkt[0] = (0x11 << 3) | 1;
        pkt[1] = 0x01;
        send(&mut sgb, &pkt);
        assert!(!sgb.active);
        assert_eq!(sgb.player_id_nibble(), 0x0F);
    }
}
