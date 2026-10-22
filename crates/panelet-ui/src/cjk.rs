// SPDX-License-Identifier: MPL-2.0

//! Panelet CJK subset of GNU Unifont 16.0.04 (font contributors).
//! SIL OFL 1.1 OR GPL-2.0-or-later WITH Font-exception-2.0.
//! Includes GB2312 level-1 Hanzi, the existing calendar set and punctuation.
//! Glyph shapes are unmodified; `scripts/build-cjk-font.py` reproduces the
//! sorted, big-endian codepoint + sixteen-row table. See 3rd-res/SOURCES.md.

pub(crate) const WIDTH: usize = 16;
pub(crate) const HEIGHT: usize = 16;
pub(crate) const ADVANCE: usize = 16;
const RECORD_LEN: usize = 34;
const DATA: &[u8] = include_bytes!("cjk-glyphs.dat");

pub(crate) const fn glyph(character: char) -> Option<[u16; HEIGHT]> {
    let mut low = 0;
    let mut high = DATA.len() / RECORD_LEN;
    while low < high {
        let mid = low + (high - low) / 2;
        let offset = mid * RECORD_LEN;
        let code = u16::from_be_bytes([DATA[offset], DATA[offset + 1]]) as u32;
        if code < character as u32 {
            low = mid + 1;
        } else if code > character as u32 {
            high = mid;
        } else {
            let mut rows = [0; HEIGHT];
            let mut row = 0;
            while row < HEIGHT {
                let start = offset + 2 + row * 2;
                rows[row] = u16::from_be_bytes([DATA[start], DATA[start + 1]]);
                row += 1;
            }
            return Some(rows);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn font_table_is_sorted_and_covers_the_declared_subset() {
        let chars: std::vec::Vec<_> = include_str!("cjk-subset.txt").trim().chars().collect();
        assert_eq!(chars.len() * RECORD_LEN, DATA.len());
        assert!(chars.windows(2).all(|pair| pair[0] < pair[1]));
        for character in chars {
            assert!(glyph(character).is_some());
        }
        for character in "买牛奶提交项目报告预约牙医待办事项暂无".chars() {
            assert!(glyph(character).is_some());
        }
        assert!(glyph('🦀').is_none());
    }
}
