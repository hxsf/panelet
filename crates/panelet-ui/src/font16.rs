// SPDX-License-Identifier: MPL-2.0

//! Optically sized mixed text: existing 5x7 Latin/digit glyphs enlarged
//! to 10x14 ink, plus the 16x16 CJK subset. Supplementary ASCII glyphs use
//! GNU Unifont 16.0.04. That font data retains SIL OFL 1.1 OR GPL-2.0-or-later
//! WITH Font-exception-2.0; see 3rd-res/SOURCES.md.
use crate::{cjk, Glyph};
const ASCII: &[u8; 95 * 16] = include_bytes!("ascii16-glyphs.dat");

pub(crate) const fn glyph(character: char) -> Option<Glyph> {
    // Optical sizing: the existing 5x7 Latin/digit glyphs become 10x14
    // ink inside a 16px line. Merely using an 8x16 font leaves its x-height
    // much smaller than Chinese; keep the established uppercase display.
    if character.is_ascii() {
        if let Some(Glyph::Narrow(source)) = crate::glyph(character.to_ascii_uppercase()) {
            let mut rows = [0u16; 16];
            let mut row = 0;
            while row < 7 {
                let mut bits = 0;
                let mut col = 0;
                while col < 5 {
                    if source[row] & (1 << (4 - col)) != 0 {
                        bits |= 3 << (8 - col * 2);
                    }
                    col += 1;
                }
                rows[row * 2 + 1] = bits;
                rows[row * 2 + 2] = bits;
                row += 1;
            }
            return Some(Glyph::Latin(rows));
        }
    }
    let code = character as u32;
    if code >= 32 && code <= 126 {
        let offset = (code as usize - 32) * 16;
        let mut rows = [0; 16];
        let mut row = 0;
        while row < 16 {
            rows[row] = ASCII[offset + row];
            row += 1;
        }
        Some(Glyph::Half(rows))
    } else if let Some(rows) = cjk::glyph(character) {
        Some(Glyph::Wide(rows))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mixed_text_has_one_line_height_and_optically_sized_latin() {
        for character in "Aa019中文".chars() {
            assert_eq!(glyph(character).unwrap().height(), 16);
        }
        assert_eq!(glyph('A').unwrap().advance(), 12);
        assert_eq!(glyph('中').unwrap().advance(), 16);
        assert_eq!(glyph('A'), glyph('a'));
        assert_eq!(crate::glyph('A').unwrap().height(), 7); // existing calendar stays compatible
        let digit = glyph('0').unwrap();
        let ink_rows = (0..16)
            .filter(|&row| (0..digit.width()).any(|column| digit.pixel(row, column)))
            .count();
        assert_eq!(ink_rows, 14);
        assert!(glyph('🦀').is_none());
    }
}
