// SPDX-License-Identifier: MPL-2.0

#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

use core::fmt;

use panelet_core::{SessionResults, TestId, TestOutcome};
use ssd2683::{Framebuffer, FramebufferError, PanelColor, HEIGHT, WIDTH};

mod canvas;
mod cjk;
mod font16;

pub use canvas::{
    render_canvas, validate_canvas_commands, CanvasCommand, CanvasError, CanvasStats,
    CANVAS_MAX_COMMANDS, CANVAS_MAX_TEXT_BYTES,
};

pub const FONT_WIDTH: usize = 5;
pub const FONT_HEIGHT: usize = 7;
pub const GLYPH_ADVANCE: usize = 6;

/// Pixel advance of a supported glyph at scale 1.
pub fn glyph_advance(character: char) -> Option<usize> {
    glyph(character).map(Glyph::advance)
}

/// Advance for the opt-in, uniformly 16-pixel-high mixed text font.
pub fn glyph16_advance(character: char) -> Option<usize> {
    font16::glyph(character).map(Glyph::advance)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Glyph {
    Half([u8; 16]),
    Latin([u16; 16]),
    Narrow([u8; FONT_HEIGHT]),
    Wide([u16; cjk::HEIGHT]),
}

impl Glyph {
    pub(crate) const fn width(self) -> usize {
        match self {
            Self::Latin(_) => 10,
            Self::Half(_) => 8,
            Self::Narrow(_) => FONT_WIDTH,
            Self::Wide(_) => cjk::WIDTH,
        }
    }

    pub(crate) const fn height(self) -> usize {
        match self {
            Self::Latin(_) => 16,
            Self::Half(_) => 16,
            Self::Narrow(_) => FONT_HEIGHT,
            Self::Wide(_) => cjk::HEIGHT,
        }
    }

    pub(crate) const fn advance(self) -> usize {
        match self {
            Self::Latin(_) => 12,
            Self::Half(_) => 8,
            Self::Narrow(_) => GLYPH_ADVANCE,
            Self::Wide(_) => cjk::ADVANCE,
        }
    }

    pub(crate) fn pixel(self, row: usize, column: usize) -> bool {
        match self {
            Self::Latin(rows) => rows[row] & (1 << (9 - column)) != 0,
            Self::Half(rows) => rows[row] & (1 << (7 - column)) != 0,
            Self::Narrow(rows) => rows[row] & (1 << (FONT_WIDTH - 1 - column)) != 0,
            Self::Wide(rows) => rows[row] & (1 << (cjk::WIDTH - 1 - column)) != 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiError {
    InvalidScale,
    UnsupportedGlyph(char),
    OutOfBounds,
    Framebuffer(FramebufferError),
}

impl fmt::Display for UiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidScale => f.write_str("font scale must be non-zero"),
            Self::UnsupportedGlyph(character) => {
                write!(f, "unsupported glyph {character:?}")
            }
            Self::OutOfBounds => f.write_str("text exceeds the 400x300 framebuffer"),
            Self::Framebuffer(error) => write!(f, "framebuffer error: {error}"),
        }
    }
}

impl From<FramebufferError> for UiError {
    fn from(error: FramebufferError) -> Self {
        Self::Framebuffer(error)
    }
}

#[must_use]
pub const fn outcome_code(outcome: Option<TestOutcome>) -> char {
    match outcome {
        None => '-',
        Some(TestOutcome::Pass) => 'P',
        Some(TestOutcome::Observed) => 'O',
        Some(TestOutcome::Inconclusive) => '?',
        Some(TestOutcome::Fail) => 'F',
        Some(TestOutcome::Skipped) => 'S',
        Some(TestOutcome::Aborted) => 'A',
    }
}

pub fn draw_char(
    frame: &mut Framebuffer<'_>,
    x: usize,
    y: usize,
    character: char,
    scale: usize,
    color: PanelColor,
) -> Result<(), UiError> {
    if scale == 0 {
        return Err(UiError::InvalidScale);
    }
    let glyph = glyph(character).ok_or(UiError::UnsupportedGlyph(character))?;
    let width = glyph
        .width()
        .checked_mul(scale)
        .ok_or(UiError::OutOfBounds)?;
    let height = glyph
        .height()
        .checked_mul(scale)
        .ok_or(UiError::OutOfBounds)?;
    if x.checked_add(width).is_none_or(|end| end > WIDTH)
        || y.checked_add(height).is_none_or(|end| end > HEIGHT)
    {
        return Err(UiError::OutOfBounds);
    }

    for row in 0..glyph.height() {
        for column in 0..glyph.width() {
            if glyph.pixel(row, column) {
                for dy in 0..scale {
                    for dx in 0..scale {
                        frame.set_pixel(x + column * scale + dx, y + row * scale + dy, color)?;
                    }
                }
            }
        }
    }
    Ok(())
}

pub fn draw_text(
    frame: &mut Framebuffer<'_>,
    x: usize,
    y: usize,
    text: &str,
    scale: usize,
    color: PanelColor,
) -> Result<(), UiError> {
    let mut cursor = x;
    for character in text.chars() {
        let glyph = glyph(character).ok_or(UiError::UnsupportedGlyph(character))?;
        draw_char(frame, cursor, y, character, scale, color)?;
        cursor = cursor
            .checked_add(
                glyph
                    .advance()
                    .checked_mul(scale)
                    .ok_or(UiError::OutOfBounds)?,
            )
            .ok_or(UiError::OutOfBounds)?;
    }
    Ok(())
}

/// Render a static black/white dashboard of the volatile per-boot snapshot.
pub fn draw_status_dashboard(
    frame: &mut Framebuffer<'_>,
    safe_mode: bool,
    results: &SessionResults,
) -> Result<(), UiError> {
    frame.fill(PanelColor::White);
    draw_text(frame, 10, 10, "NOTE4C TEST", 3, PanelColor::Black)?;
    draw_text(
        frame,
        10,
        38,
        if safe_mode {
            "MODE SAFE"
        } else {
            "MODE NORMAL"
        },
        2,
        PanelColor::Black,
    )?;

    for (index, test) in TestId::ALL.into_iter().enumerate() {
        let y = 65 + index * 20;
        draw_char(
            frame,
            12,
            y,
            char::from(b'1' + index as u8),
            2,
            PanelColor::Black,
        )?;
        draw_text(frame, 28, y, test.name(), 2, PanelColor::Black)?;
        draw_char(
            frame,
            360,
            y,
            outcome_code(results.get(test)),
            2,
            PanelColor::Black,
        )?;
    }

    draw_text(frame, 8, 238, "UP DOWN SELECT", 1, PanelColor::Black)?;
    draw_text(frame, 8, 252, "LED PULSES ITEM", 1, PanelColor::Black)?;
    draw_text(frame, 8, 266, "HOLD BOOT FOR EPD", 1, PanelColor::Black)?;
    draw_text(frame, 8, 280, "SESSION SNAPSHOT", 1, PanelColor::Black)?;
    Ok(())
}

pub(crate) const fn glyph(character: char) -> Option<Glyph> {
    if let Some(rows) = cjk::glyph(character) {
        return Some(Glyph::Wide(rows));
    }
    Some(Glyph::Narrow(match character {
        ' ' => [0, 0, 0, 0, 0, 0, 0],
        '-' => [0, 0, 0, 0b11111, 0, 0, 0],
        '_' => [0, 0, 0, 0, 0, 0, 0b11111],
        '/' => [0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0, 0],
        '.' => [0, 0, 0, 0, 0, 0, 0b00100],
        ':' => [0, 0b00100, 0, 0, 0b00100, 0, 0],
        '?' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0, 0b00100],
        '0' => [
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
        ],
        '1' => [
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        '2' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        '3' => [
            0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        '4' => [
            0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
        ],
        '5' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b00001, 0b00001, 0b11110,
        ],
        '6' => [
            0b01110, 0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
        ],
        '7' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
        ],
        '8' => [
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
        ],
        '9' => [
            0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b01110,
        ],
        'A' => [
            0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'B' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110,
        ],
        'C' => [
            0b01111, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b01111,
        ],
        'D' => [
            0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110,
        ],
        'E' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ],
        'F' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'G' => [
            0b01111, 0b10000, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111,
        ],
        'H' => [
            0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'I' => [
            0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        'J' => [
            0b00111, 0b00010, 0b00010, 0b00010, 0b10010, 0b10010, 0b01100,
        ],
        'K' => [
            0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001,
        ],
        'L' => [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ],
        'M' => [
            0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001,
        ],
        'N' => [
            0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001,
        ],
        'O' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'P' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'Q' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101,
        ],
        'R' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
        ],
        'S' => [
            0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        'T' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'U' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'V' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100,
        ],
        'W' => [
            0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b10101, 0b01010,
        ],
        'X' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001,
        ],
        'Y' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'Z' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111,
        ],
        _ => return None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ssd2683::FRAMEBUFFER_LEN;

    #[test]
    fn glyph_bitmaps_cover_ascii_and_the_calendar_cjk_subset() {
        assert!(matches!(glyph('A'), Some(Glyph::Narrow(rows)) if rows[0] == 0b01110));
        assert!(matches!(glyph('_'), Some(Glyph::Narrow(rows)) if rows[6] == 0b11111));
        assert!(matches!(glyph('历'), Some(Glyph::Wide(rows)) if rows[1] == 0x3ffe));
        assert_eq!(glyph('a'), None);
    }

    #[test]
    fn scaling_multiplies_each_set_pixel_and_bounds_are_checked() {
        let mut storage = [0_u8; FRAMEBUFFER_LEN];
        let mut frame = Framebuffer::new(&mut storage, PanelColor::White);
        draw_char(&mut frame, 0, 0, 'I', 2, PanelColor::Black).unwrap();
        let black = (0..14)
            .flat_map(|y| (0..10).map(move |x| (x, y)))
            .filter(|(x, y)| frame.pixel(*x, *y).unwrap() == PanelColor::Black)
            .count();
        assert_eq!(black, 44);
        assert_eq!(
            draw_char(&mut frame, 398, 0, 'A', 1, PanelColor::Black),
            Err(UiError::OutOfBounds)
        );
        draw_char(&mut frame, 10, 20, '月', 1, PanelColor::Red).unwrap();
        assert_eq!(frame.pixel(21, 21).unwrap(), PanelColor::Red);
        assert_eq!(
            draw_char(&mut frame, 0, 0, 'A', 0, PanelColor::Black),
            Err(UiError::InvalidScale)
        );
    }

    #[test]
    fn all_session_statuses_have_stable_codes() {
        assert_eq!(outcome_code(None), '-');
        assert_eq!(outcome_code(Some(TestOutcome::Pass)), 'P');
        assert_eq!(outcome_code(Some(TestOutcome::Observed)), 'O');
        assert_eq!(outcome_code(Some(TestOutcome::Inconclusive)), '?');
        assert_eq!(outcome_code(Some(TestOutcome::Fail)), 'F');
        assert_eq!(outcome_code(Some(TestOutcome::Skipped)), 'S');
        assert_eq!(outcome_code(Some(TestOutcome::Aborted)), 'A');
    }

    #[test]
    fn complete_dashboard_renders_all_eight_rows_within_bounds() {
        let mut storage = [0_u8; FRAMEBUFFER_LEN];
        let mut frame = Framebuffer::new(&mut storage, PanelColor::Red);
        let mut results = SessionResults::new();
        for (index, test) in TestId::ALL.into_iter().enumerate() {
            let outcome = if index % 2 == 0 {
                TestOutcome::Pass
            } else {
                TestOutcome::Observed
            };
            results.record(test, outcome);
        }
        draw_status_dashboard(&mut frame, false, &results).unwrap();
        assert!(frame
            .as_bytes()
            .iter()
            .any(|byte| *byte != PanelColor::White.solid_byte()));
        assert_eq!(frame.pixel(399, 299).unwrap(), PanelColor::White);
    }
}
