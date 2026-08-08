//! DMG colorization.
//!
//! An original Game Boy game only ever produces four grey levels. On Game Boy
//! Color hardware those levels are remapped through colour palettes, which is
//! what makes monochrome games appear "colorized". We reproduce that here as a
//! toggle, so a front-end (e.g. Trophy Hub's "colorize GB games" switch) can
//! turn it on and off.
//!
//! `Colorize::Auto` reproduces the Game Boy Color boot ROM's own automatic
//! palette assignment: it checks the game is Nintendo-published, hashes the
//! title, and looks the result up in the boot ROM's tables (in `gbc_palettes`)
//! to pick the exact palette real hardware used. Games with no assignment fall
//! back to the boot ROM's default palette, just like a real GBC.

use crate::gbc_palettes::{
    CHECKSUMS, FOURTH_LETTERS, PALETTE_COLORS, PALETTE_INDEXES_AND_FLAGS, TRIPLETS,
};

/// `TitleChecksums.ambiguous - TitleChecksums`: indices below this are used
/// directly; at or above it the 4th title letter disambiguates.
const AMBIGUOUS_BASE: usize = 65;
/// Stride between rows of the fourth-letter table.
const LETTER_ROW: usize = 14;
/// How to colour a monochrome (DMG) game. Ignored for real CGB games.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Colorize {
    /// The classic greenish Game Boy look.
    Off,
    /// Plain white-to-black grey.
    Grayscale,
    /// A per-cartridge colour theme chosen from the header.
    Auto,
}

/// Resolved DMG colours: one 4-entry RGB888 palette each for the background and
/// the two sprite palette registers (OBP0/OBP1).
#[derive(Clone, Copy)]
pub struct DmgPalette {
    pub bg: [u32; 4],
    pub obj0: [u32; 4],
    pub obj1: [u32; 4],
}

impl DmgPalette {
    /// Same four colours for background and both sprite palettes.
    const fn mono(shades: [u32; 4]) -> DmgPalette {
        DmgPalette {
            bg: shades,
            obj0: shades,
            obj1: shades,
        }
    }

    pub const fn green() -> DmgPalette {
        DmgPalette::mono([0x00E0F8D0, 0x0088C070, 0x00346856, 0x00081820])
    }

    pub const fn grayscale() -> DmgPalette {
        DmgPalette::mono([0x00FFFFFF, 0x00A8A8A8, 0x00505050, 0x00000000])
    }

    /// The Game Boy Color boot ROM's automatic palette for a cartridge. Falls
    /// back to the boot ROM's default palette (ID 0) when the game is not
    /// Nintendo-published or has no assignment.
    pub fn auto(checksum: u8, fourth_letter: u8, nintendo: bool) -> DmgPalette {
        let index = if nintendo {
            gbc_palette_index(checksum, fourth_letter).unwrap_or(0)
        } else {
            0
        };
        palette_for_index(index)
    }

    pub fn resolve(mode: Colorize, checksum: u8, fourth: u8, nintendo: bool) -> DmgPalette {
        match mode {
            Colorize::Off => DmgPalette::green(),
            Colorize::Grayscale => DmgPalette::grayscale(),
            Colorize::Auto => DmgPalette::auto(checksum, fourth, nintendo),
        }
    }
}

/// Reproduce the boot ROM's title-checksum lookup with 4th-letter disambiguation.
/// Returns the palette index into `PALETTE_INDEXES_AND_FLAGS`, or None if the
/// checksum (or disambiguated letter) is not in the tables.
fn gbc_palette_index(checksum: u8, fourth_letter: u8) -> Option<usize> {
    let pos = CHECKSUMS.iter().position(|&c| c == checksum)?;
    if pos < AMBIGUOUS_BASE {
        return Some(pos);
    }
    // Ambiguous checksum: the fourth-letter table and the palette-assignment
    // table are parallel arrays for the ambiguous region (both start at
    // AMBIGUOUS_BASE). Walk down this checksum's column (stride LETTER_ROW),
    // and the matching slot's palette index is exactly AMBIGUOUS_BASE + letter.
    let col = pos - AMBIGUOUS_BASE;
    let mut letter = col;
    while letter < FOURTH_LETTERS.len() {
        if FOURTH_LETTERS[letter] == fourth_letter {
            return Some(AMBIGUOUS_BASE + letter);
        }
        letter += LETTER_ROW;
    }
    None
}

/// Turn a palette index into the three resolved 4-colour palettes, applying the
/// boot ROM's "shuffle flags" that decide which triplet entry feeds OBP0/OBP1.
fn palette_for_index(index: usize) -> DmgPalette {
    let entry = PALETTE_INDEXES_AND_FLAGS[index];
    let triplet = TRIPLETS[(entry & 0x1F) as usize];
    let flags = entry >> 5;
    let [e0, e1, e2] = triplet;

    // Per the boot ROM (flags = entry >> 5, triplet = [e0, e1, e2]): BGP is
    // always e2. OBP0 is e0 when bit 0 is set, else e2. OBP1 is e1 when bit 2
    // is set, else e0 when bit 1 is set, else e2. Derived by constraint
    // propagation over the PocketPalettes ground-truth pack, anchored on the
    // flags=0b101 identity case (Metroid II + 7 unambiguous games).
    let obp0 = if flags & 1 != 0 { e0 } else { e2 };
    let obp1 = if flags & 4 != 0 {
        e1
    } else if flags & 2 != 0 {
        e0
    } else {
        e2
    };
    DmgPalette {
        bg: colors_at(e2),
        obj0: colors_at(obp0),
        obj1: colors_at(obp1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Metroid II (title "METROID2", checksum 0x46, 4th letter 'R') is a plain
    // DMG game with an *ambiguous* checksum, so it exercises the fourth-letter
    // disambiguation (-> palette index 80) and the OBP flag decode. Both were
    // buggy: the index cursor advanced by the checksum column instead of
    // LETTER_ROW (landing on 67 = all-green), and OBP0 selected e2 not e0.
    // All three palettes are byte-exact against the on-hardware PocketPalettes
    // .pal for Metroid II:
    //   BG   = white / light-blue / blue / black
    //   OBP0 = yellow / red / dark-red / black   (Samus suit, START text, cursor)
    //   OBP1 = white / green / dark-green / black
    #[test]
    fn metroid2_matches_hardware_palette() {
        assert_eq!(gbc_palette_index(0x46, b'R'), Some(80));
        // GBC-corrected output (raw boot-ROM RGB555 run through gbc_correct):
        //   BG   = white / muted blue / dark blue / black
        //   OBP0 = Samus suit yellow / red / dark-red / black
        //   OBP1 = white / muted green / dark-green / black
        let pal = DmgPalette::auto(0x46, b'R', true);
        assert_eq!(pal.bg, [0x00F0F0F0, 0x0071B6D0, 0x000F3EAA, 0x00000000]);
        assert_eq!(pal.obj0, [0x00E8BA4D, 0x00C9002E, 0x004E0012, 0x00000000]);
        assert_eq!(pal.obj1, [0x00F0F0F0, 0x0083C656, 0x00106010, 0x00000000]);
    }

    #[test]
    fn gbc_correction_tames_neon_green() {
        // Palette 3's raw color 1 is 7BFF31 neon green (5-bit 0x0F,0x1F,0x06).
        // Correction must desaturate it to a muted sage, not leave it as
        // nuclear waste.
        assert_eq!(gbc_correct(0x0F, 0x1F, 0x06), 0x0083C656);
        // White and black are (near) fixed points.
        assert_eq!(gbc_correct(0x1F, 0x1F, 0x1F), 0x00F0F0F0);
        assert_eq!(gbc_correct(0x00, 0x00, 0x00), 0x00000000);
    }

    // Four of the 29 triplet entries point 6 bytes past a palette boundary, so
    // the palette they name starts on the last colour of one row of
    // PALETTE_COLORS and finishes in the next. Reading `PALETTE_COLORS[off / 8]`
    // instead lands on an unrelated palette; for 0x1E that is palette 3, whose
    // colour 1 is the neon green that turned Super Mario Land's Mario into
    // nuclear waste. Spelled out here because the wrong answer is a perfectly
    // plausible-looking palette, so only the exact colours catch it.
    #[test]
    fn misaligned_offsets_straddle_two_palette_rows() {
        // 0x1E = colour 15 = palette 3 colour 3, then palette 4 colours 0..2.
        assert_eq!(
            colors_at(0x1E),
            [0x00000000, 0x00F0F0F0, 0x00E18096, 0x007F3848]
        );
        // The old row-indexed decode returned palette 3 whole; make sure we are
        // not accidentally back on it.
        assert_ne!(
            colors_at(0x1E),
            [0x00F0F0F0, 0x0083C656, 0x00106010, 0x00000000]
        );
        // An aligned offset must be unaffected: 0x18 = palette 3, whole.
        assert_eq!(
            colors_at(0x18),
            [0x00F0F0F0, 0x0083C656, 0x00106010, 0x00000000]
        );
    }

    // End-to-end over the five Nintendo-published games in our ROM corpus whose
    // boot-ROM assignment lands on a misaligned triplet entry. Expected colours
    // are gambatte's GBC boot-ROM palette table (gbcpalettes.h) run through our
    // colour correction; before the fix every one of these was wrong.
    #[test]
    fn misaligned_games_match_boot_rom() {
        // (title checksum, 4th title letter, bg, obj0, obj1)
        const CASES: &[(u8, u8, [u32; 4], [u32; 4], [u32; 4])] = &[
            // SUPER MARIOLAND -> p30A. Mario is white/red, not green.
            (
                0x46,
                b'E',
                [0x00B4C2E1, 0x00F0DEB0, 0x00975256, 0x00000000],
                [0x00000000, 0x00F0F0F0, 0x00E18096, 0x007F3848],
                [0x00000000, 0x00F0F0F0, 0x00E18096, 0x007F3848],
            ),
            // KIRBY DREAM LAND -> p508.
            (
                0x49,
                b'B',
                [0x00A4B0DB, 0x00E8BA4D, 0x000C480C, 0x00000000],
                [0x00DA5C71, 0x00A90027, 0x004E0012, 0x00000000],
                [0x000F3EAA, 0x00F0F0F0, 0x00F0D8A0, 0x001F9EBA],
            ),
            // KIRBY2 -> p508.
            (
                0xB3,
                b'B',
                [0x00A4B0DB, 0x00E8BA4D, 0x000C480C, 0x00000000],
                [0x00DA5C71, 0x00A90027, 0x004E0012, 0x00000000],
                [0x000F3EAA, 0x00F0F0F0, 0x00F0D8A0, 0x001F9EBA],
            ),
            // KIRBY BLOCKBALL -> p508.
            (
                0x27,
                b'B',
                [0x00A4B0DB, 0x00E8BA4D, 0x000C480C, 0x00000000],
                [0x00DA5C71, 0x00A90027, 0x004E0012, 0x00000000],
                [0x000F3EAA, 0x00F0F0F0, 0x00F0D8A0, 0x001F9EBA],
            ),
            // TOPRANKTENNIS -> p502.
            (
                0xF0,
                b'R',
                [0x0073BA32, 0x00F0F0F0, 0x00D84E6A, 0x00000000],
                [0x00F0F0F0, 0x00F0F0F0, 0x0071B6D0, 0x000F3EAA],
                [0x00F0F0F0, 0x00E49685, 0x006E241E, 0x00000000],
            ),
        ];
        for &(checksum, fourth, bg, obj0, obj1) in CASES {
            let pal = DmgPalette::auto(checksum, fourth, true);
            assert_eq!(pal.bg, bg, "bg for checksum {checksum:#04X}");
            assert_eq!(pal.obj0, obj0, "obj0 for checksum {checksum:#04X}");
            assert_eq!(pal.obj1, obj1, "obj1 for checksum {checksum:#04X}");
        }
    }
}

/// Expand one boot-ROM palette (a byte offset into the colour table) to RGB888,
/// through the Game Boy Color LCD color-correction matrix.
///
/// The offset is a *raw byte* offset into the boot ROM's colour table, which is
/// one flat run of RGB555 colours: 2 bytes per colour, 4 colours per palette.
/// Most triplet entries are multiples of 8 and so name a whole palette, but four
/// of them (0x1E, 0xB6, 0xDE) are 6 past a boundary and deliberately start on
/// the *last* colour of one palette, running on into the next. So index the
/// table by colour, never by row: `PALETTE_COLORS[offset / 8]` silently rounds
/// those four down to a completely unrelated palette.
///
/// The highest offset in `TRIPLETS` is 0xE8, whose four colours end exactly on
/// the last entry of the table, so the indexing below cannot run off the end.
fn colors_at(offset: u8) -> [u32; 4] {
    let first = offset as usize / 2;
    let mut out = [0u32; 4];
    for (c, slot) in out.iter_mut().enumerate() {
        let row = PALETTE_COLORS[(first + c) / 4];
        let i = ((first + c) % 4) * 3;
        *slot = gbc_correct(row[i], row[i + 1], row[i + 2]);
    }
    out
}

/// The Game Boy Color's LCD did not display raw RGB555 values; it ran them
/// through a fixed response that heavily desaturates and slightly dims. Without
/// it the boot-ROM palettes look like nuclear waste (e.g. palette 3's `7BFF31`
/// neon green). This is higan's integer GBC matrix, matching
/// how real hardware (and Gambatte with color-correction on) actually look.
/// Inputs are 5-bit channels; output is 0x00RRGGBB.
pub(crate) fn gbc_correct(r: u8, g: u8, b: u8) -> u32 {
    let (r, g, b) = (r as u32, g as u32, b as u32);
    let rr = (r * 26 + g * 4 + b * 2).min(960) >> 2;
    let gg = (g * 24 + b * 8).min(960) >> 2;
    let bb = (r * 6 + g * 4 + b * 22).min(960) >> 2;
    (rr << 16) | (gg << 8) | bb
}

