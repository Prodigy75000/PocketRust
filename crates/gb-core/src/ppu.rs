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

    /// DMG colorization palettes (background + the two sprite palettes).
    dmg_palette: DmgPalette,

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
            let white = self.dmg_palette.bg[0];
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
            self.bg_priority[x as usize] = attr & 0x80 != 0;
            self.framebuffer[fb_base + x as usize] = self.bg_color(pal, color);
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
            self.bg_priority[x as usize] = attr & 0x80 != 0;
            self.framebuffer[fb_base + x as usize] = self.bg_color(pal, color);
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

    /// Resolve a background colour index to RGB.
    fn bg_color(&self, palette: u8, color: u8) -> Pixel {
        if self.cgb {
            cgb_rgb(&self.bg_pal, palette, color)
        } else {
            self.dmg_palette.bg[apply_palette(self.bgp, color) as usize]
        }
    }

    /// Resolve a sprite colour index to RGB using the sprite's OAM flags.
    fn obj_color(&self, flags: u8, color: u8) -> Pixel {
        if self.cgb {
            cgb_rgb(&self.obj_pal, flags & 0x07, color)
        } else if flags & 0x10 != 0 {
            self.dmg_palette.obj1[apply_palette(self.obp1, color) as usize]
        } else {
            self.dmg_palette.obj0[apply_palette(self.obp0, color) as usize]
        }
    }

    /// Choose the DMG colorization palettes (no effect in CGB mode).
    pub fn set_dmg_palette(&mut self, palette: DmgPalette) {
        self.dmg_palette = palette;
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
