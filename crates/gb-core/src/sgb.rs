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

    /// The four SGB palettes (SGB0..3), each 4 colors, resolved to RGB888.
    pub palettes: [[u32; 4]; 4],

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
            palettes: [[0xFFFFFF, 0xAAAAAA, 0x555555, 0x000000]; 4],
            log: Vec::new(),
        }
    }

    /// Fed every write to the joypad register (0xFF00).
    pub fn write_p1(&mut self, val: u8) {
        if !self.enabled {
            return;
        }
        match val & 0x30 {
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
                let mut read = |base: usize| {
                    [
                        c0,
                        bgr555(d[base], d[base + 1]),
                        bgr555(d[base + 2], d[base + 3]),
                        bgr555(d[base + 4], d[base + 5]),
                    ]
                };
                self.palettes[a] = read(3);
                self.palettes[b] = read(9);
            }
            _ => {} // ATTR_*, PAL_TRN, PAL_SET, MASK_EN, MLT_REQ... later
        }
    }
}
