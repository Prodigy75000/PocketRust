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
/// One past the last valid palette index (`fourth-letter len + ambiguous base`).
const MAX_INDEX: usize = FOURTH_LETTERS.len() + AMBIGUOUS_BASE;

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
    // Ambiguous checksum: walk down the fourth-letter column until the 4th
    // title letter matches, advancing the index by the column each row.
    let col = pos - AMBIGUOUS_BASE;
    let mut index = pos;
    let mut letter = col;
    loop {
        if letter >= FOURTH_LETTERS.len() {
            return None;
        }
        if FOURTH_LETTERS[letter] == fourth_letter {
            return Some(index);
        }
        index += col;
        letter += LETTER_ROW;
        if index >= MAX_INDEX {
            return None;
        }
    }
}

/// Turn a palette index into the three resolved 4-colour palettes, applying the
/// boot ROM's "shuffle flags" that decide which triplet entry feeds OBP0/OBP1.
fn palette_for_index(index: usize) -> DmgPalette {
    let entry = PALETTE_INDEXES_AND_FLAGS[index];
    let triplet = TRIPLETS[(entry & 0x1F) as usize];
    let flags = entry >> 5;
    let [e0, e1, e2] = triplet;

    // Per the boot ROM: BGP is always the 3rd entry; OBP0 is the 3rd entry if
    // bit 0 is set else the 1st; OBP1 is the 2nd if bit 2, else 3rd if bit 1,
    // else the 1st.
    let obp0 = if flags & 1 != 0 { e2 } else { e0 };
    let obp1 = if flags & 4 != 0 {
        e1
    } else if flags & 2 != 0 {
        e2
    } else {
        e0
    };
    DmgPalette {
        bg: colors_at(e2),
        obj0: colors_at(obp0),
        obj1: colors_at(obp1),
    }
}

/// Expand one boot-ROM palette (a byte offset into the colour table) to RGB888.
fn colors_at(offset: u8) -> [u32; 4] {
    let row = PALETTE_COLORS[(offset / 8) as usize];
    let expand = |v: u8| {
        let v = v as u32;
        (v << 3) | (v >> 2) // 5-bit -> 8-bit
    };
    let mut out = [0u32; 4];
    for (c, slot) in out.iter_mut().enumerate() {
        let r = expand(row[c * 3]);
        let g = expand(row[c * 3 + 1]);
        let b = expand(row[c * 3 + 2]);
        *slot = (r << 16) | (g << 8) | b;
    }
    out
}

