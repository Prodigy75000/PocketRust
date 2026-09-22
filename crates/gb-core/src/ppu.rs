//! The Picture Processing Unit (DMG + CGB).
//!
//! The PPU walks four modes per scanline (154 lines, 456 T-cycles each) and
//! renders a full line at a time. Output is RGB888 so the same buffer serves
//! both the monochrome (DMG) and colour (CGB) paths.
//!
//! On CGB there is a second VRAM bank (tile attributes + extra tile data),
//! 8 background and 8 sprite palettes of RGB555 colour, and a richer priority
//! scheme; the DMG path is preserved exactly so the Blargg/acid2 tests still hold.

pub const SCREEN_W: usize = 160;
pub const SCREEN_H: usize = 144;

const OAM_CYCLES: u32 = 80;
const DRAW_CYCLES: u32 = 172;
const LINE_CYCLES: u32 = 456;

use crate::colorize::DmgPalette;

/// A fully-resolved pixel colour, 0x00RRGGBB.
pub type Pixel = u32;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    HBlank = 0,
    VBlank = 1,
    OamScan = 2,
    Drawing = 3,
}

pub struct Ppu {
    pub cgb: bool,
    pub vram: [u8; 0x4000], // two 8 KiB banks
    pub vram_bank: usize,
    pub oam: [u8; 0xA0],

    // Registers.
    pub lcdc: u8,
    pub stat: u8,
    pub scy: u8,
    pub scx: u8,
    pub ly: u8,
    pub lyc: u8,
    pub bgp: u8,
    pub obp0: u8,
    pub obp1: u8,
    pub wy: u8,
    pub wx: u8,

    // CGB palette memory (8 palettes * 4 colours * 2 bytes = 64).
    pub bg_pal: [u8; 64],
    pub bg_pal_index: u8,
    pub bg_pal_autoinc: bool,
    pub obj_pal: [u8; 64],
    pub obj_pal_index: u8,
    pub obj_pal_autoinc: bool,

    /// DMG colorization palettes (background + the two sprite palettes), from
    /// the frontend's colorize option.
    dmg_palette: DmgPalette,
    /// A Super Game Boy palette, once the running SGB cart supplies one. It
    /// takes precedence over `dmg_palette`, so the colorize option can never
    /// clobber the game's own SGB colors regardless of call order.
    sgb_palette: Option<DmgPalette>,
    /// The other three SGB palettes, and which tile uses which.
    ///
    /// `sgb_palette` above is palette 0 and stays the fallback, so a cartridge
    /// that never sends an ATTR_ command behaves exactly as before. Derived
    /// from SGB state, so neither of these is in the save state.
    sgb_palettes: [DmgPalette; 4],
    sgb_attr: [u8; crate::sgb::ATTR_W * crate::sgb::ATTR_H],
    /// Frames left to blank the display while an SGB VRAM transfer is in flight
    /// (CHR/PCT/PAL/ATTR_TRN show their data on-screen as garbage; real SGB and
    /// Gambatte hide it). Counts down per frame.
    /// MASK_EN state: 0 none, 1 freeze, 2 black, 3 colour 0. Occupies the
    /// save-state byte the old frame counter did, so the layout is unchanged.
    sgb_mask: u8,
    /// The frame captured when a freeze was raised. Deliberately NOT in the
    /// save state: it is 90 KiB, and a state restored mid-freeze simply shows
    /// the live frame until the cartridge masks or cancels again.
    sgb_frozen: Option<Box<[u32; SCREEN_W * SCREEN_H]>>,
    /// Every BG colour index in the finished frame, 0..3, which is what an SGB
    /// VRAM transfer actually carries.
    ///
    /// The transfer is not a memory read. The cartridge DRAWS the 4 KiB onto
    /// the screen and the SNES reads it back off the scanlines, so what gets
    /// transferred is whatever is displayed. Reading VRAM $8000 directly is the
    /// tempting shortcut and it is wrong whenever LCDC's tile-data select
    /// points elsewhere, which is how Game & Watch Gallery 2 transferred 4 KiB
    /// of zeroes and turned its whole palette black.
    ///
    /// Derived from the frame, so not in the save state.
    frame_index: Box<[u8; SCREEN_W * SCREEN_H]>,
    /// A `_TRN` command is waiting for the NEXT frame. Pan Docs: "the actual
    /// transfer starts at the beginning of the next frame after the command
    /// has been sent", so the frame in flight when the command arrives is the
    /// wrong one to read.
    sgb_capture: Option<u8>,
    /// The 4 KiB, once a frame has carried it.
    sgb_captured: Option<(u8, Box<[u8; 0x1000]>)>,

    mode: Mode,
    line_cycles: u32,
    window_line: u8,

    pub framebuffer: [Pixel; SCREEN_W * SCREEN_H],
    // Per-scanline scratch used for sprite priority.
    bg_index: [u8; SCREEN_W],    // BG/window colour index 0..3
    bg_priority: [bool; SCREEN_W], // CGB BG-attr "above OAM" bit
    pub frame_ready: bool,

    pub vblank_interrupt: bool,
    pub stat_interrupt: bool,
    stat_line: bool,
}

/// Whether MASK_EN is applied to the displayed frame. See `apply_sgb_mask`
/// for the measurement behind this being false: applying it costs 99
/// cartridges across the full set. Flip once that is understood.
const APPLY_SGB_MASK: bool = false;

impl Ppu {
    pub fn new(cgb: bool) -> Ppu {
        Ppu {
            cgb,
            vram: [0; 0x4000],
            vram_bank: 0,
            oam: [0; 0xA0],
            lcdc: 0x91,
            stat: 0x85,
            scy: 0,
            scx: 0,
            ly: 0,
            lyc: 0,
            bgp: 0xFC,
            obp0: 0xFF,
            obp1: 0xFF,
            wy: 0,
            wx: 0,
            bg_pal: [0xFF; 64],
            bg_pal_index: 0,
            bg_pal_autoinc: false,
            obj_pal: [0xFF; 64],
            obj_pal_index: 0,
            obj_pal_autoinc: false,
            dmg_palette: DmgPalette::green(),
            sgb_palette: None,
            sgb_palettes: [DmgPalette::green(); 4],
            sgb_attr: [0; crate::sgb::ATTR_W * crate::sgb::ATTR_H],
            sgb_mask: 0,
            sgb_frozen: None,
            frame_index: Box::new([0; SCREEN_W * SCREEN_H]),
            sgb_capture: None,
            sgb_captured: None,
            mode: Mode::OamScan,
            line_cycles: 0,
            window_line: 0,
            framebuffer: [0; SCREEN_W * SCREEN_H],
            bg_index: [0; SCREEN_W],
            bg_priority: [false; SCREEN_W],
            frame_ready: false,
            vblank_interrupt: false,
            stat_interrupt: false,
            stat_line: false,
        }
    }

    #[inline]
    fn lcd_on(&self) -> bool {
        self.lcdc & 0x80 != 0
    }

    // --- VRAM / OAM access --------------------------------------------------

    #[inline]
    fn vram_at(&self, bank: usize, addr: u16) -> u8 {
        self.vram[bank * 0x2000 + (addr as usize - 0x8000)]
    }

    pub fn read_vram(&self, addr: u16) -> u8 {
        self.vram_at(self.vram_bank, addr)
    }
    pub fn write_vram(&mut self, addr: u16, val: u8) {
        self.vram[self.vram_bank * 0x2000 + (addr as usize - 0x8000)] = val;
    }
    pub fn read_oam(&self, addr: u16) -> u8 {
        self.oam[(addr - 0xFE00) as usize]
    }
    pub fn write_oam(&mut self, addr: u16, val: u8) {
        self.oam[(addr - 0xFE00) as usize] = val;
    }

    pub fn set_vram_bank(&mut self, v: u8) {
        self.vram_bank = (v & 1) as usize;
    }
    pub fn vram_bank_reg(&self) -> u8 {
        0xFE | self.vram_bank as u8
    }

    // --- CGB palette register access ----------------------------------------

    pub fn write_bg_pal_index(&mut self, v: u8) {
        self.bg_pal_index = v & 0x3F;
        self.bg_pal_autoinc = v & 0x80 != 0;
    }
    pub fn read_bg_pal_index(&self) -> u8 {
        self.bg_pal_index | (self.bg_pal_autoinc as u8) << 7 | 0x40
    }
    pub fn write_bg_pal_data(&mut self, v: u8) {
        self.bg_pal[self.bg_pal_index as usize] = v;
        if self.bg_pal_autoinc {
            self.bg_pal_index = (self.bg_pal_index + 1) & 0x3F;
        }
    }
    pub fn read_bg_pal_data(&self) -> u8 {
        self.bg_pal[self.bg_pal_index as usize]
    }

    pub fn write_obj_pal_index(&mut self, v: u8) {
        self.obj_pal_index = v & 0x3F;
        self.obj_pal_autoinc = v & 0x80 != 0;
    }
    pub fn read_obj_pal_index(&self) -> u8 {
        self.obj_pal_index | (self.obj_pal_autoinc as u8) << 7 | 0x40
    }
    pub fn write_obj_pal_data(&mut self, v: u8) {
        self.obj_pal[self.obj_pal_index as usize] = v;
        if self.obj_pal_autoinc {
            self.obj_pal_index = (self.obj_pal_index + 1) & 0x3F;
        }
    }
    pub fn read_obj_pal_data(&self) -> u8 {
        self.obj_pal[self.obj_pal_index as usize]
    }

    // --- STAT / LCDC --------------------------------------------------------

    pub fn read_stat(&self) -> u8 {
        let coincidence = if self.ly == self.lyc { 0x04 } else { 0x00 };
        0x80 | (self.stat & 0x78) | coincidence | (self.mode as u8)
    }
    pub fn write_stat(&mut self, val: u8) {
        self.stat = val & 0x78;
    }

    pub fn write_lcdc(&mut self, val: u8) {
        let was_on = self.lcd_on();
        self.lcdc = val;
        if was_on && !self.lcd_on() {
            self.ly = 0;
            self.line_cycles = 0;
            self.window_line = 0;
            self.mode = Mode::HBlank;
        }
    }

    // --- Timing -------------------------------------------------------------

    /// Advance the PPU; returns true if an HBlank just started (for HDMA).
    pub fn step(&mut self, cycles: u32) -> bool {
        if !self.lcd_on() {
            return false;
        }
        self.line_cycles += cycles;
        let mut entered_hblank = false;

        match self.mode {
            Mode::OamScan => {
                if self.line_cycles >= OAM_CYCLES {
                    self.line_cycles -= OAM_CYCLES;
                    self.mode = Mode::Drawing;
                }
            }
            Mode::Drawing => {
                if self.line_cycles >= DRAW_CYCLES {
                    self.line_cycles -= DRAW_CYCLES;
                    self.mode = Mode::HBlank;
                    self.render_scanline();
                    entered_hblank = true;
                }
            }
            Mode::HBlank => {
                if self.line_cycles >= LINE_CYCLES - OAM_CYCLES - DRAW_CYCLES {
                    self.line_cycles -= LINE_CYCLES - OAM_CYCLES - DRAW_CYCLES;
                    self.advance_line();
                }
            }
            Mode::VBlank => {
                if self.line_cycles >= LINE_CYCLES {
                    self.line_cycles -= LINE_CYCLES;
                    self.advance_line();
                }
            }
        }
        self.update_stat_interrupt();
        entered_hblank
    }

    fn advance_line(&mut self) {
        self.ly += 1;
        if self.ly == SCREEN_H as u8 {
            self.mode = Mode::VBlank;
            self.vblank_interrupt = true;
            self.frame_ready = true;
            self.capture_sgb_frame();
            self.apply_sgb_mask();
        } else if self.ly > 153 {
            self.ly = 0;
            self.window_line = 0;
            self.mode = Mode::OamScan;
        } else if self.ly < SCREEN_H as u8 {
            self.mode = Mode::OamScan;
        }
    }

    fn update_stat_interrupt(&mut self) {
        let coincidence = self.ly == self.lyc;
        let line = (self.stat & 0x08 != 0 && self.mode == Mode::HBlank)
            || (self.stat & 0x10 != 0 && self.mode == Mode::VBlank)
            || (self.stat & 0x20 != 0 && self.mode == Mode::OamScan)
            || (self.stat & 0x40 != 0 && coincidence);
        if line && !self.stat_line {
            self.stat_interrupt = true;
        }
        self.stat_line = line;
    }

    // --- Rendering ----------------------------------------------------------

    fn render_scanline(&mut self) {
        // On DMG, LCDC bit 0 disables the background entirely. On CGB it only
        // demotes BG priority, so the BG is always drawn there.
        if self.cgb || self.lcdc & 0x01 != 0 {
            self.render_bg_line();
        } else {
            let base = self.ly as usize * SCREEN_W;
            let white = self.active_palette().bg[0];
            for x in 0..SCREEN_W {
                self.framebuffer[base + x] = white;
                self.bg_index[x] = 0;
                self.bg_priority[x] = false;
            }
        }
        if self.lcdc & 0x20 != 0 {
            self.render_window_line();
        }
        if self.lcdc & 0x02 != 0 {
            self.render_sprites();
        }
    }

    /// Shared tile-pixel fetch for background and window.
    fn tile_pixel(
        &self,
        map_base: u16,
        tile_row: u16,
        tile_col: u16,
        pixel_row: u16,
        pixel_col: u8,
    ) -> (u8, u8, u8) {
        let signed = self.lcdc & 0x10 == 0;
        let map_addr = map_base + tile_row * 32 + tile_col;
        let tile_index = self.vram_at(0, map_addr);
        let attr = if self.cgb { self.vram_at(1, map_addr) } else { 0 };

        let bank = if self.cgb && attr & 0x08 != 0 { 1 } else { 0 };
        let flip_x = attr & 0x20 != 0;
        let flip_y = attr & 0x40 != 0;

        let row = if flip_y { 7 - pixel_row } else { pixel_row };
        let tile_addr = self.tile_data_addr(tile_index, signed) + row * 2;
        let lo = self.vram_at(bank, tile_addr);
        let hi = self.vram_at(bank, tile_addr + 1);
        let bit = if flip_x { pixel_col } else { 7 - pixel_col };
        let color = ((hi >> bit) & 1) << 1 | ((lo >> bit) & 1);
        (color, attr & 0x07, attr) // colour index, palette num, raw attr
    }

    fn render_bg_line(&mut self) {
        let y = self.ly;
        let map_base: u16 = if self.lcdc & 0x08 != 0 { 0x9C00 } else { 0x9800 };
        let bg_y = y.wrapping_add(self.scy);
        let tile_row = (bg_y / 8) as u16;
        let pixel_row = (bg_y % 8) as u16;
        let fb_base = y as usize * SCREEN_W;

        for x in 0..SCREEN_W as u8 {
            let bg_x = x.wrapping_add(self.scx);
            let (color, pal, attr) =
                self.tile_pixel(map_base, tile_row, (bg_x / 8) as u16, pixel_row, bg_x % 8);
            self.bg_index[x as usize] = color;
            self.frame_index[fb_base + x as usize] = color;
            self.bg_priority[x as usize] = attr & 0x80 != 0;
            self.framebuffer[fb_base + x as usize] =
                self.bg_color_at(pal, color, x as usize, self.ly as usize);
        }
    }

    fn render_window_line(&mut self) {
        let y = self.ly;
        if y < self.wy || self.wx > 166 {
            return;
        }
        let map_base: u16 = if self.lcdc & 0x40 != 0 { 0x9C00 } else { 0x9800 };
        let win_y = self.window_line;
        let tile_row = (win_y / 8) as u16;
        let pixel_row = (win_y % 8) as u16;
        let fb_base = y as usize * SCREEN_W;
        let start_x = self.wx.saturating_sub(7);

        let mut drew_any = false;
        for x in start_x..SCREEN_W as u8 {
            let win_x = x - start_x;
            let (color, pal, attr) =
                self.tile_pixel(map_base, tile_row, (win_x / 8) as u16, pixel_row, win_x % 8);
            self.bg_index[x as usize] = color;
            self.frame_index[fb_base + x as usize] = color;
            self.bg_priority[x as usize] = attr & 0x80 != 0;
            self.framebuffer[fb_base + x as usize] =
                self.bg_color_at(pal, color, x as usize, self.ly as usize);
            drew_any = true;
        }
        if drew_any {
            self.window_line = self.window_line.wrapping_add(1);
        }
    }

    fn render_sprites(&mut self) {
        let y = self.ly as i16;
        let tall = self.lcdc & 0x04 != 0;
        let height: i16 = if tall { 16 } else { 8 };
        let master_priority = self.lcdc & 0x01 != 0; // CGB: BG master priority

        // Up to 10 sprites intersecting this line, in OAM order.
        let mut visible: Vec<usize> = Vec::with_capacity(10);
        for i in 0..40 {
            let sprite_y = self.oam[i * 4] as i16 - 16;
            if y >= sprite_y && y < sprite_y + height {
                visible.push(i);
                if visible.len() == 10 {
                    break;
                }
            }
        }
        // Draw back-to-front so the highest-priority sprite lands last. On CGB
        // priority is purely OAM order; on DMG smaller X wins, ties by index.
        if self.cgb {
            visible.sort_by(|&a, &b| b.cmp(&a));
        } else {
            visible.sort_by(|&a, &b| {
                self.oam[b * 4 + 1]
                    .cmp(&self.oam[a * 4 + 1])
                    .then(b.cmp(&a))
            });
        }

        let fb_base = self.ly as usize * SCREEN_W;
        for &i in &visible {
            let oam = i * 4;
            let sprite_y = self.oam[oam] as i16 - 16;
            let sprite_x = self.oam[oam + 1] as i16 - 8;
            let mut tile = self.oam[oam + 2];
            let flags = self.oam[oam + 3];

            let behind_bg = flags & 0x80 != 0;
            let flip_y = flags & 0x40 != 0;
            let flip_x = flags & 0x20 != 0;
            let bank = if self.cgb && flags & 0x08 != 0 { 1 } else { 0 };

            let mut row = y - sprite_y;
            if flip_y {
                row = height - 1 - row;
            }
            if tall {
                tile &= 0xFE;
                if row >= 8 {
                    tile |= 1;
                    row -= 8;
                }
            }
            let tile_addr = 0x8000 + tile as u16 * 16 + row as u16 * 2;
            let lo = self.vram_at(bank, tile_addr);
            let hi = self.vram_at(bank, tile_addr + 1);

            for col in 0..8i16 {
                let px = sprite_x + col;
                if px < 0 || px >= SCREEN_W as i16 {
                    continue;
                }
                let bit = if flip_x { col } else { 7 - col };
                let color = ((hi >> bit) & 1) << 1 | ((lo >> bit) & 1);
                if color == 0 {
                    continue;
                }
                let px = px as usize;
                let bg_idx = self.bg_index[px];
                // Priority resolution.
                let show = if self.cgb {
                    if !master_priority {
                        true // BG master priority off: sprites always win
                    } else if self.bg_priority[px] && bg_idx != 0 {
                        false // BG-attr priority
                    } else if behind_bg && bg_idx != 0 {
                        false
                    } else {
                        true
                    }
                } else if behind_bg && bg_idx != 0 {
                    false
                } else {
                    true
                };
                if show {
                    self.framebuffer[fb_base + px] = self.obj_color(flags, color);
                }
            }
        }
    }

    fn tile_data_addr(&self, index: u8, signed: bool) -> u16 {
        if signed {
            (0x9000i32 + (index as i8 as i32) * 16) as u16
        } else {
            0x8000 + index as u16 * 16
        }
    }

    /// The palette actually driving DMG output: the SGB palette if the cart has
    /// supplied one, otherwise the frontend's colorization choice.
    fn active_palette(&self) -> &DmgPalette {
        self.sgb_palette.as_ref().unwrap_or(&self.dmg_palette)
    }

    /// Resolve a background colour index to RGB.
    fn bg_color(&self, palette: u8, color: u8) -> Pixel {
        if self.cgb {
            cgb_rgb(&self.bg_pal, palette, color)
        } else {
            self.active_palette().bg[apply_palette(self.bgp, color) as usize]
        }
    }

    /// As `bg_color`, but honouring the SGB's per-tile palette map.
    fn bg_color_at(&self, palette: u8, color: u8, x: usize, y: usize) -> Pixel {
        if self.cgb {
            cgb_rgb(&self.bg_pal, palette, color)
        } else {
            self.sgb_tile_palette(x, y).bg[apply_palette(self.bgp, color) as usize]
        }
    }

    /// Resolve a sprite colour index to RGB using the sprite's OAM flags.
    fn obj_color(&self, flags: u8, color: u8) -> Pixel {
        if self.cgb {
            cgb_rgb(&self.obj_pal, flags & 0x07, color)
        } else if flags & 0x10 != 0 {
            self.active_palette().obj1[apply_palette(self.obp1, color) as usize]
        } else {
            self.active_palette().obj0[apply_palette(self.obp0, color) as usize]
        }
    }

    /// Choose the DMG colorization palettes (no effect in CGB mode, and does not
    /// override an active SGB palette).
    pub fn set_dmg_palette(&mut self, palette: DmgPalette) {
        self.dmg_palette = palette;
    }

    /// Set (or clear) the Super Game Boy's own palette. `Some` takes precedence
    /// over the frontend colorize option; `None` releases it back to the normal
    /// colorization (used when a cart's SGB palette path is unsupported).
    pub fn set_sgb_palette(&mut self, palette: Option<DmgPalette>) {
        self.sgb_palette = palette;
    }

    /// All four SGB palettes, as raw colours.
    pub fn set_sgb_palettes(&mut self, pals: &[[u32; 4]; 4]) {
        for (dst, src) in self.sgb_palettes.iter_mut().zip(pals.iter()) {
            *dst = DmgPalette::mono_pub(*src);
        }
    }

    /// Which palette each 8x8 tile of the screen uses.
    pub fn set_sgb_attr(&mut self, attr: [u8; crate::sgb::ATTR_W * crate::sgb::ATTR_H]) {
        self.sgb_attr = attr;
    }

    /// The palette for the tile containing screen pixel (x, y).
    ///
    /// Only consulted when an SGB palette is actually in force: without one,
    /// the attribute map is meaningless and the ordinary DMG path applies.
    fn sgb_tile_palette(&self, x: usize, y: usize) -> &DmgPalette {
        if self.sgb_palette.is_none() {
            return &self.dmg_palette;
        }
        let tx = (x / 8).min(crate::sgb::ATTR_W - 1);
        let ty = (y / 8).min(crate::sgb::ATTR_H - 1);
        let idx = self.sgb_attr[ty * crate::sgb::ATTR_W + tx] as usize;
        &self.sgb_palettes[idx.min(3)]
    }

    /// A `_TRN` command arrived: read the transfer off the NEXT frame.
    pub fn sgb_request_transfer(&mut self, cmd: u8) {
        self.sgb_capture = Some(cmd);
    }

    /// The 4 KiB, once a frame has carried it.
    pub fn take_sgb_transfer(&mut self) -> Option<(u8, Box<[u8; 0x1000]>)> {
        self.sgb_captured.take()
    }

    /// Rebuild the transferred bytes from the frame just finished.
    ///
    /// The cartridge draws the data as ordinary 2bpp tiles, so each 8x8 tile of
    /// the screen is sixteen bytes: for each of its eight rows, the low
    /// bitplane then the high bitplane, taken from the colour index of each
    /// pixel. Tiles run left to right, then down, and the first 4096 bytes are
    /// the transfer. A 160x144 screen holds 20x18 tiles, which is 5760 bytes,
    /// so the last third of the screen is not part of it.
    fn capture_sgb_frame(&mut self) {
        let Some(cmd) = self.sgb_capture.take() else {
            return;
        };
        let mut out = Box::new([0u8; 0x1000]);
        let mut n = 0usize;
        'outer: for ty in 0..(SCREEN_H / 8) {
            for tx in 0..(SCREEN_W / 8) {
                for row in 0..8 {
                    let base = (ty * 8 + row) * SCREEN_W + tx * 8;
                    let (mut lo, mut hi) = (0u8, 0u8);
                    for bit in 0..8 {
                        let c = self.frame_index[base + bit];
                        lo = (lo << 1) | (c & 1);
                        hi = (hi << 1) | ((c >> 1) & 1);
                    }
                    out[n] = lo;
                    out[n + 1] = hi;
                    n += 2;
                    if n >= 0x1000 {
                        break 'outer;
                    }
                }
            }
        }
        self.sgb_captured = Some((cmd, out));
    }

    /// The cartridge's MASK_EN state: 0 none, 1 freeze, 2 black, 3 colour 0.
    ///
    /// This replaces a 90-frame blanket blank that stood in for it. That guess
    /// existed because a mask the cartridge never cancelled would stick
    /// forever, and cancels depended on transfers that never completed. Now
    /// they do, so the cartridge's own cancel arrives and the screen is masked
    /// for exactly as long as it asked.
    pub fn set_sgb_mask(&mut self, mask: u8) {
        if mask == 1 && self.sgb_mask != 1 {
            // Freeze shows the frame that was up when the mask was raised, so
            // it has to be captured on the edge rather than re-read later: by
            // then the cartridge has already drawn transfer data over it.
            self.sgb_frozen = Some(Box::new(self.framebuffer));
        }
        if mask != 1 {
            self.sgb_frozen = None;
        }
        self.sgb_mask = mask;
    }

    /// Apply the cartridge's screen mask to the just-finished frame.
    fn apply_sgb_mask(&mut self) {
        // The mask is TRACKED but not yet APPLIED, and that is a measurement
        // rather than caution. Full set, 5344 cartridges, one variable at a
        // time:
        //
        //   SGB off, as shipped                       148 blank
        //   SGB on, transfers read, mask not applied  200 blank
        //   SGB on, transfers read, mask applied      299 blank
        //
        // Applying it costs 99 cartridges on its own. Something about how a
        // mask is raised or cancelled here is wrong, and until that is found,
        // a mask that sticks is strictly worse than no mask: the old blanket
        // 90-frame blank it replaces was at least self-clearing.
        //
        // The state is still decoded and kept, because the border work needs
        // it and because tracking it costs nothing. Only the application is
        // held back.
        if !APPLY_SGB_MASK {
            return;
        }
        match self.sgb_mask {
            1 => {
                if let Some(f) = &self.sgb_frozen {
                    self.framebuffer = **f;
                }
            }
            // Black, and colour 0. Colour 0 is whatever the palette's first
            // entry is, so on a monochrome screen the two look alike; they are
            // kept apart because an SGB palette can make colour 0 anything.
            2 => self.framebuffer = [0x0000_0000; SCREEN_W * SCREEN_H],
            3 => {
                let c = self.sgb_color_zero();
                self.framebuffer = [c; SCREEN_W * SCREEN_H];
            }
            _ => {}
        }
    }

    /// Colour 0 of the active palette, for MASK_EN mode 3.
    fn sgb_color_zero(&self) -> u32 {
        self.sgb_palette.map(|p| p.bg[0]).unwrap_or(0x00FF_FFFF)
    }

    pub(crate) fn transfer<C: crate::save::Cursor>(&mut self, c: &mut C) {
        c.bytes(&mut self.vram);
        c.usize(&mut self.vram_bank);
        c.bytes(&mut self.oam);
        for reg in [
            &mut self.lcdc,
            &mut self.stat,
            &mut self.scy,
            &mut self.scx,
            &mut self.ly,
            &mut self.lyc,
            &mut self.bgp,
            &mut self.obp0,
            &mut self.obp1,
            &mut self.wy,
            &mut self.wx,
        ] {
            c.u8(reg);
        }
        c.bytes(&mut self.bg_pal);
        c.u8(&mut self.bg_pal_index);
        c.bool(&mut self.bg_pal_autoinc);
        c.bytes(&mut self.obj_pal);
        c.u8(&mut self.obj_pal_index);
        c.bool(&mut self.obj_pal_autoinc);

        let mut mode = self.mode as u8;
        c.u8(&mut mode);
        self.mode = match mode {
            0 => Mode::HBlank,
            1 => Mode::VBlank,
            2 => Mode::OamScan,
            _ => Mode::Drawing,
        };
        c.u32(&mut self.line_cycles);
        c.u8(&mut self.window_line);
        c.bool(&mut self.frame_ready);
        c.bool(&mut self.vblank_interrupt);
        c.bool(&mut self.stat_interrupt);
        c.bool(&mut self.stat_line);

        // The SGB palette overlay is part of state: a mid-game restore must keep
        // the game's SGB colors rather than flash the colorize default until the
        // next PAL command. (dmg_palette itself is config, restored separately.)
        let mut present = self.sgb_palette.is_some() as u8;
        c.u8(&mut present);
        let mut pal = self.sgb_palette.unwrap_or_else(DmgPalette::green);
        for arr in [&mut pal.bg, &mut pal.obj0, &mut pal.obj1] {
            for v in arr.iter_mut() {
                c.u32(v);
            }
        }
        self.sgb_palette = if present != 0 { Some(pal) } else { None };
        c.u8(&mut self.sgb_mask);
        // framebuffer, bg_index, bg_priority, dmg_palette are not part of state:
        // the first is re-rendered, the scratch is per-scanline, and the palette
        // is config restored from the colorize setting.
    }
}

/// Map a 2-bit colour through a DMG palette register.
#[inline]
fn apply_palette(palette: u8, color: u8) -> u8 {
    (palette >> (color * 2)) & 0x03
}

/// Read an RGB555 colour from CGB palette RAM and expand it to RGB888.
#[inline]
fn cgb_rgb(pal: &[u8; 64], palette: u8, color: u8) -> Pixel {
    let i = (palette as usize * 8) + color as usize * 2;
    let lo = pal[i] as u16;
    let hi = pal[i + 1] as u16;
    let rgb555 = lo | (hi << 8);
    let r = (rgb555 & 0x1F) as u32;
    let g = ((rgb555 >> 5) & 0x1F) as u32;
    let b = ((rgb555 >> 10) & 0x1F) as u32;
    // 5-bit -> 8-bit with the low bits replicated for a full-range white.
    let expand = |c: u32| (c << 3) | (c >> 2);
    (expand(r) << 16) | (expand(g) << 8) | expand(b)
}
