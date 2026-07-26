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
    /// Whether the cart declares SGB support (header 0x146 == 0x03).
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
    /// A VRAM-transfer command just arrived; the PPU should blank the display
    /// so the transfer data isn't shown as garbage.
    transfer_pending: bool,

    /// Debug: (command code, total data bytes) of each completed command.
    pub log: Vec<(u8, usize)>,
}

impl Sgb {
    pub fn new(sgb_flag: u8) -> Sgb {
        Sgb {
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
            transfer_pending: false,
            log: Vec::new(),
        }
    }

    /// Whether a VRAM transfer just started (consumes the flag). The PPU freezes
    /// the display briefly so the transfer's on-screen data isn't shown.
    pub fn take_transfer(&mut self) -> bool {
        let t = self.transfer_pending;
        self.transfer_pending = false;
        t
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
            // PAL_SET (0xA): selects palettes from a transferred table we can't
            // read; stop overriding (else the black placeholder blanks the
            // screen). It draws nothing itself, so no display freeze.
            0x0A => {
                self.supported = false;
                self.palette_dirty = true;
            }
            // PAL_TRN (0xB) / ATTR_TRN (0x15): same, and they transfer through
            // VRAM (garbage on-screen), so also freeze the display.
            0x0B | 0x15 => {
                self.supported = false;
                self.palette_dirty = true;
                self.transfer_pending = true;
            }
            // VRAM transfers (SOU_TRN, CHR_TRN, PCT_TRN, OBJ_TRN): the cart shows
            // the transfer data on-screen for the SGB to read; blank it away.
            0x09 | 0x13 | 0x14 | 0x18 => self.transfer_pending = true,
            // MASK_EN (0x17) is deliberately NOT honored: a freeze/black mask
            // persists until the cart cancels it, and some carts (Donkey Kong)
            // never send the cancel in our timing, which would leave the screen
            // stuck. The transfer blanking above already hides the init garbage.
            _ => {} // ATTR_BLK/DIV, DATA_SND, MASK_EN... ignored
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
    fn pal_transfer_disables_override_to_avoid_black_screen() {
        // A cart that sets a black placeholder PAL then transfers its real
        // colors via PAL_TRN/PAL_SET (which we don't follow) must NOT be left
        // with the black override, or the whole screen goes black (Donkey Kong).
        let mut sgb = Sgb::new(0x03);
        let mut pal = [0u8; 16];
        pal[0] = (0x00 << 3) | 1; // PAL01, all colors black
        send(&mut sgb, &pal);
        // Degenerate all-black palette: bow out even before the transfer command.
        assert!(matches!(sgb.take_palette_override(), Some(None)));

        let mut trn = [0u8; 16];
        trn[0] = (0x0B << 3) | 1; // PAL_TRN
        send(&mut sgb, &trn);
        assert!(matches!(sgb.take_palette_override(), Some(None))); // stays released
        assert!(!sgb.supported);
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
