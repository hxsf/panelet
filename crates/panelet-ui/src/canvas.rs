// SPDX-License-Identifier: MPL-2.0

use core::fmt;

use ssd2683::{Framebuffer, FramebufferError, PanelColor, HEIGHT, WIDTH};

use crate::glyph;

/// Maximum number of drawing operations accepted in one widget frame.
pub const CANVAS_MAX_COMMANDS: usize = 128;

/// Maximum combined UTF-8 payload of all text commands in one widget frame.
pub const CANVAS_MAX_TEXT_BYTES: usize = 1024;

/// A bounded, platform-independent drawing operation for the Panelet canvas.
///
/// Coordinates are signed so callers can position primitives partially outside
/// the panel and rely on clipping. Rectangle dimensions are pixel counts;
/// zero-width and zero-height rectangles are no-ops. Line and rectangle edges
/// include both endpoints.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CanvasCommand<'a> {
    Clear {
        color: PanelColor,
    },
    Pixel {
        x: i32,
        y: i32,
        color: PanelColor,
    },
    Line {
        x0: i32,
        y0: i32,
        x1: i32,
        y1: i32,
        color: PanelColor,
    },
    Rect {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        color: PanelColor,
    },
    FillRect {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        color: PanelColor,
    },
    Text {
        x: i32,
        y: i32,
        text: &'a str,
        scale: u8,
        color: PanelColor,
    },
    TextSized {
        x: i32,
        y: i32,
        text: &'a str,
        height: u8,
        color: PanelColor,
    },
}

/// Resource usage validated for one command list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CanvasStats {
    pub command_count: usize,
    pub text_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CanvasError {
    TooManyCommands { actual: usize, maximum: usize },
    TextBudgetExceeded { actual: usize, maximum: usize },
    InvalidScale,
    UnsupportedGlyph(char),
    ArithmeticOverflow,
    Framebuffer(FramebufferError),
}

impl fmt::Display for CanvasError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyCommands { actual, maximum } => {
                write!(f, "canvas has {actual} commands; maximum is {maximum}")
            }
            Self::TextBudgetExceeded { actual, maximum } => {
                write!(f, "canvas text uses {actual} bytes; maximum is {maximum}")
            }
            Self::InvalidScale => f.write_str("canvas text scale must be non-zero"),
            Self::UnsupportedGlyph(character) => {
                write!(f, "unsupported glyph {character:?}")
            }
            Self::ArithmeticOverflow => f.write_str("canvas coordinate arithmetic overflowed"),
            Self::Framebuffer(error) => write!(f, "framebuffer error: {error}"),
        }
    }
}

impl From<FramebufferError> for CanvasError {
    fn from(error: FramebufferError) -> Self {
        Self::Framebuffer(error)
    }
}

/// Validate the entire command list before rendering mutates the framebuffer.
pub fn validate_canvas_commands(
    commands: &[CanvasCommand<'_>],
) -> Result<CanvasStats, CanvasError> {
    if commands.len() > CANVAS_MAX_COMMANDS {
        return Err(CanvasError::TooManyCommands {
            actual: commands.len(),
            maximum: CANVAS_MAX_COMMANDS,
        });
    }

    let mut text_bytes = 0_usize;
    for command in commands {
        if let CanvasCommand::Text { text, scale, .. }
        | CanvasCommand::TextSized {
            text,
            height: scale,
            ..
        } = command
        {
            if *scale == 0
                || (matches!(command, CanvasCommand::TextSized { .. })
                    && (!(16..=32).contains(scale) || scale % 2 != 0))
            {
                return Err(CanvasError::InvalidScale);
            }
            text_bytes = text_bytes
                .checked_add(text.len())
                .ok_or(CanvasError::ArithmeticOverflow)?;
            if text_bytes > CANVAS_MAX_TEXT_BYTES {
                return Err(CanvasError::TextBudgetExceeded {
                    actual: text_bytes,
                    maximum: CANVAS_MAX_TEXT_BYTES,
                });
            }
            for character in text.chars() {
                let supported = if matches!(command, CanvasCommand::TextSized { .. }) {
                    crate::font16::glyph(character)
                } else {
                    glyph(character)
                };
                if supported.is_none() {
                    return Err(CanvasError::UnsupportedGlyph(character));
                }
            }
        }
    }

    Ok(CanvasStats {
        command_count: commands.len(),
        text_bytes,
    })
}

/// Render a validated command list into the existing 400x300, 30,000-byte
/// MSB-first 2-bpp framebuffer.
///
/// Quota and text validation happens for the whole list before the first pixel
/// is changed. Geometry outside the panel is clipped rather than rejected.
pub fn render_canvas(
    frame: &mut Framebuffer<'_>,
    commands: &[CanvasCommand<'_>],
) -> Result<CanvasStats, CanvasError> {
    let stats = validate_canvas_commands(commands)?;

    for command in commands {
        match *command {
            CanvasCommand::Clear { color } => frame.fill(color),
            CanvasCommand::Pixel { x, y, color } => {
                draw_pixel_clipped(frame, i64::from(x), i64::from(y), color)?;
            }
            CanvasCommand::Line {
                x0,
                y0,
                x1,
                y1,
                color,
            } => draw_line_clipped(
                frame,
                i64::from(x0),
                i64::from(y0),
                i64::from(x1),
                i64::from(y1),
                color,
            )?,
            CanvasCommand::Rect {
                x,
                y,
                width,
                height,
                color,
            } => draw_rect_clipped(
                frame,
                i64::from(x),
                i64::from(y),
                u64::from(width),
                u64::from(height),
                color,
            )?,
            CanvasCommand::FillRect {
                x,
                y,
                width,
                height,
                color,
            } => fill_rect_clipped(
                frame,
                i64::from(x),
                i64::from(y),
                u64::from(width),
                u64::from(height),
                color,
            )?,
            CanvasCommand::Text {
                x,
                y,
                text,
                scale,
                color,
            } => draw_text_clipped(
                frame,
                i64::from(x),
                i64::from(y),
                text,
                i64::from(scale),
                color,
                false,
            )?,
            CanvasCommand::TextSized {
                x,
                y,
                text,
                height,
                color,
            } => {
                draw_text_sized(
                    frame,
                    i64::from(x),
                    i64::from(y),
                    text,
                    usize::from(height),
                    color,
                )?;
            }
        }
    }

    Ok(stats)
}

fn draw_pixel_clipped(
    frame: &mut Framebuffer<'_>,
    x: i64,
    y: i64,
    color: PanelColor,
) -> Result<(), CanvasError> {
    if (0..i64::try_from(WIDTH).expect("panel width fits i64")).contains(&x)
        && (0..i64::try_from(HEIGHT).expect("panel height fits i64")).contains(&y)
    {
        frame.set_pixel(
            usize::try_from(x).map_err(|_| CanvasError::ArithmeticOverflow)?,
            usize::try_from(y).map_err(|_| CanvasError::ArithmeticOverflow)?,
            color,
        )?;
    }
    Ok(())
}

fn fill_rect_clipped(
    frame: &mut Framebuffer<'_>,
    x: i64,
    y: i64,
    width: u64,
    height: u64,
    color: PanelColor,
) -> Result<(), CanvasError> {
    if width == 0 || height == 0 {
        return Ok(());
    }

    let width = i64::try_from(width).map_err(|_| CanvasError::ArithmeticOverflow)?;
    let height = i64::try_from(height).map_err(|_| CanvasError::ArithmeticOverflow)?;
    let x_end = x
        .checked_add(width)
        .ok_or(CanvasError::ArithmeticOverflow)?;
    let y_end = y
        .checked_add(height)
        .ok_or(CanvasError::ArithmeticOverflow)?;
    let clipped_x0 = x.max(0);
    let clipped_y0 = y.max(0);
    let clipped_x1 = x_end.min(i64::try_from(WIDTH).expect("panel width fits i64"));
    let clipped_y1 = y_end.min(i64::try_from(HEIGHT).expect("panel height fits i64"));

    if clipped_x0 >= clipped_x1 || clipped_y0 >= clipped_y1 {
        return Ok(());
    }

    for py in clipped_y0..clipped_y1 {
        for px in clipped_x0..clipped_x1 {
            draw_pixel_clipped(frame, px, py, color)?;
        }
    }
    Ok(())
}

fn draw_rect_clipped(
    frame: &mut Framebuffer<'_>,
    x: i64,
    y: i64,
    width: u64,
    height: u64,
    color: PanelColor,
) -> Result<(), CanvasError> {
    if width == 0 || height == 0 {
        return Ok(());
    }

    let width = i64::try_from(width).map_err(|_| CanvasError::ArithmeticOverflow)?;
    let height = i64::try_from(height).map_err(|_| CanvasError::ArithmeticOverflow)?;
    let x1 = x
        .checked_add(
            width
                .checked_sub(1)
                .ok_or(CanvasError::ArithmeticOverflow)?,
        )
        .ok_or(CanvasError::ArithmeticOverflow)?;
    let y1 = y
        .checked_add(
            height
                .checked_sub(1)
                .ok_or(CanvasError::ArithmeticOverflow)?,
        )
        .ok_or(CanvasError::ArithmeticOverflow)?;

    draw_line_clipped(frame, x, y, x1, y, color)?;
    if y1 != y {
        draw_line_clipped(frame, x, y1, x1, y1, color)?;
    }
    draw_line_clipped(frame, x, y, x, y1, color)?;
    if x1 != x {
        draw_line_clipped(frame, x1, y, x1, y1, color)?;
    }
    Ok(())
}

const OUT_LEFT: u8 = 0b0001;
const OUT_RIGHT: u8 = 0b0010;
const OUT_TOP: u8 = 0b0100;
const OUT_BOTTOM: u8 = 0b1000;

fn out_code(x: i64, y: i64) -> u8 {
    let mut code = 0;
    if x < 0 {
        code |= OUT_LEFT;
    } else if x >= i64::try_from(WIDTH).expect("panel width fits i64") {
        code |= OUT_RIGHT;
    }
    if y < 0 {
        code |= OUT_TOP;
    } else if y >= i64::try_from(HEIGHT).expect("panel height fits i64") {
        code |= OUT_BOTTOM;
    }
    code
}

fn checked_intersection(
    origin: i64,
    delta: i64,
    numerator: i64,
    denominator: i64,
) -> Result<i64, CanvasError> {
    let scaled = i128::from(delta)
        .checked_mul(i128::from(numerator))
        .ok_or(CanvasError::ArithmeticOverflow)?;
    let intersection = i128::from(origin)
        .checked_add(scaled / i128::from(denominator))
        .ok_or(CanvasError::ArithmeticOverflow)?;
    i64::try_from(intersection).map_err(|_| CanvasError::ArithmeticOverflow)
}

fn clip_line(
    mut x0: i64,
    mut y0: i64,
    mut x1: i64,
    mut y1: i64,
) -> Result<Option<(i64, i64, i64, i64)>, CanvasError> {
    let max_x = i64::try_from(WIDTH - 1).expect("panel width fits i64");
    let max_y = i64::try_from(HEIGHT - 1).expect("panel height fits i64");

    // Cohen-Sutherland clipping. Each accepted intersection clears at least
    // one outside bit, so eight passes are sufficient for a four-edge box.
    for _ in 0..8 {
        let code0 = out_code(x0, y0);
        let code1 = out_code(x1, y1);
        if code0 | code1 == 0 {
            return Ok(Some((x0, y0, x1, y1)));
        }
        if code0 & code1 != 0 {
            return Ok(None);
        }

        let outside = if code0 != 0 { code0 } else { code1 };
        let dx = x1.checked_sub(x0).ok_or(CanvasError::ArithmeticOverflow)?;
        let dy = y1.checked_sub(y0).ok_or(CanvasError::ArithmeticOverflow)?;
        let (x, y) = if outside & OUT_TOP != 0 {
            if dy == 0 {
                return Ok(None);
            }
            (checked_intersection(x0, dx, -y0, dy)?, 0)
        } else if outside & OUT_BOTTOM != 0 {
            if dy == 0 {
                return Ok(None);
            }
            (checked_intersection(x0, dx, max_y - y0, dy)?, max_y)
        } else if outside & OUT_RIGHT != 0 {
            if dx == 0 {
                return Ok(None);
            }
            (max_x, checked_intersection(y0, dy, max_x - x0, dx)?)
        } else {
            if dx == 0 {
                return Ok(None);
            }
            (0, checked_intersection(y0, dy, -x0, dx)?)
        };

        if outside == code0 {
            (x0, y0) = (x, y);
        } else {
            (x1, y1) = (x, y);
        }
    }

    Err(CanvasError::ArithmeticOverflow)
}

fn draw_line_clipped(
    frame: &mut Framebuffer<'_>,
    x0: i64,
    y0: i64,
    x1: i64,
    y1: i64,
    color: PanelColor,
) -> Result<(), CanvasError> {
    let Some((mut x0, mut y0, x1, y1)) = clip_line(x0, y0, x1, y1)? else {
        return Ok(());
    };

    let dx = (x1 - x0).abs();
    let step_x = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let step_y = if y0 < y1 { 1 } else { -1 };
    let mut error = dx + dy;

    loop {
        draw_pixel_clipped(frame, x0, y0, color)?;
        if x0 == x1 && y0 == y1 {
            break;
        }
        let doubled = error
            .checked_mul(2)
            .ok_or(CanvasError::ArithmeticOverflow)?;
        if doubled >= dy {
            error = error
                .checked_add(dy)
                .ok_or(CanvasError::ArithmeticOverflow)?;
            x0 += step_x;
        }
        if doubled <= dx {
            error = error
                .checked_add(dx)
                .ok_or(CanvasError::ArithmeticOverflow)?;
            y0 += step_y;
        }
    }
    Ok(())
}

fn draw_text_sized(
    frame: &mut Framebuffer<'_>,
    x: i64,
    y: i64,
    text: &str,
    height: usize,
    color: PanelColor,
) -> Result<(), CanvasError> {
    let mut cursor = x;
    for character in text.chars() {
        let glyph =
            crate::font16::glyph(character).ok_or(CanvasError::UnsupportedGlyph(character))?;
        let width = glyph.width() * height / 16;
        for dy in 0..height {
            for dx in 0..width {
                if glyph.pixel(dy * 16 / height, dx * glyph.width() / width) {
                    draw_pixel_clipped(frame, cursor + dx as i64, y + dy as i64, color)?;
                }
            }
        }
        cursor += (glyph.advance() * height / 16) as i64;
    }
    Ok(())
}

fn draw_text_clipped(
    frame: &mut Framebuffer<'_>,
    x: i64,
    y: i64,
    text: &str,
    scale: i64,
    color: PanelColor,
    font16: bool,
) -> Result<(), CanvasError> {
    let mut cursor = x;
    for character in text.chars() {
        let glyph = (if font16 {
            crate::font16::glyph(character)
        } else {
            glyph(character)
        })
        .ok_or(CanvasError::UnsupportedGlyph(character))?;
        draw_glyph_clipped(frame, cursor, y, glyph, scale, color)?;
        cursor = cursor
            .checked_add(
                i64::try_from(glyph.advance())
                    .map_err(|_| CanvasError::ArithmeticOverflow)?
                    .checked_mul(scale)
                    .ok_or(CanvasError::ArithmeticOverflow)?,
            )
            .ok_or(CanvasError::ArithmeticOverflow)?;
    }
    Ok(())
}

fn draw_glyph_clipped(
    frame: &mut Framebuffer<'_>,
    x: i64,
    y: i64,
    glyph: crate::Glyph,
    scale: i64,
    color: PanelColor,
) -> Result<(), CanvasError> {
    for row in 0..glyph.height() {
        for column in 0..glyph.width() {
            if !glyph.pixel(row, column) {
                continue;
            }
            let pixel_x = x
                .checked_add(
                    i64::try_from(column)
                        .map_err(|_| CanvasError::ArithmeticOverflow)?
                        .checked_mul(scale)
                        .ok_or(CanvasError::ArithmeticOverflow)?,
                )
                .ok_or(CanvasError::ArithmeticOverflow)?;
            let pixel_y = y
                .checked_add(
                    i64::try_from(row)
                        .map_err(|_| CanvasError::ArithmeticOverflow)?
                        .checked_mul(scale)
                        .ok_or(CanvasError::ArithmeticOverflow)?,
                )
                .ok_or(CanvasError::ArithmeticOverflow)?;
            fill_rect_clipped(
                frame,
                pixel_x,
                pixel_y,
                u64::try_from(scale).map_err(|_| CanvasError::ArithmeticOverflow)?,
                u64::try_from(scale).map_err(|_| CanvasError::ArithmeticOverflow)?,
                color,
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ssd2683::FRAMEBUFFER_LEN;
    use std::{string::String, vec::Vec};

    fn white_frame<'a>(storage: &'a mut [u8; FRAMEBUFFER_LEN]) -> Framebuffer<'a> {
        Framebuffer::new(storage, PanelColor::White)
    }

    #[test]
    fn canvas_pixels_preserve_msb_first_four_color_packing() {
        let mut storage = [0_u8; FRAMEBUFFER_LEN];
        let mut frame = white_frame(&mut storage);
        let commands = [
            CanvasCommand::Pixel {
                x: 0,
                y: 0,
                color: PanelColor::Black,
            },
            CanvasCommand::Pixel {
                x: 1,
                y: 0,
                color: PanelColor::White,
            },
            CanvasCommand::Pixel {
                x: 2,
                y: 0,
                color: PanelColor::Yellow,
            },
            CanvasCommand::Pixel {
                x: 3,
                y: 0,
                color: PanelColor::Red,
            },
        ];

        assert_eq!(
            render_canvas(&mut frame, &commands).unwrap().command_count,
            4
        );
        assert_eq!(frame.as_bytes()[0], 0x1b);
    }

    #[test]
    fn pixels_and_filled_rectangles_clip_to_the_panel() {
        let mut storage = [0_u8; FRAMEBUFFER_LEN];
        let mut frame = white_frame(&mut storage);
        let commands = [
            CanvasCommand::Pixel {
                x: -1,
                y: 0,
                color: PanelColor::Red,
            },
            CanvasCommand::Pixel {
                x: 400,
                y: 299,
                color: PanelColor::Red,
            },
            CanvasCommand::FillRect {
                x: -2,
                y: -2,
                width: 4,
                height: 4,
                color: PanelColor::Black,
            },
        ];

        render_canvas(&mut frame, &commands).unwrap();
        for y in 0..2 {
            for x in 0..2 {
                assert_eq!(frame.pixel(x, y).unwrap(), PanelColor::Black);
            }
        }
        assert_eq!(frame.pixel(2, 0).unwrap(), PanelColor::White);
        assert_eq!(frame.pixel(399, 299).unwrap(), PanelColor::White);
    }

    #[test]
    fn lines_are_inclusive_and_clipped_before_rasterization() {
        let mut storage = [0_u8; FRAMEBUFFER_LEN];
        let mut frame = white_frame(&mut storage);
        let commands = [
            CanvasCommand::Line {
                x0: -100,
                y0: 5,
                x1: 10,
                y1: 5,
                color: PanelColor::Black,
            },
            CanvasCommand::Line {
                x0: 398,
                y0: 298,
                x1: 401,
                y1: 301,
                color: PanelColor::Red,
            },
        ];

        render_canvas(&mut frame, &commands).unwrap();
        for x in 0..=10 {
            assert_eq!(frame.pixel(x, 5).unwrap(), PanelColor::Black);
        }
        assert_eq!(frame.pixel(11, 5).unwrap(), PanelColor::White);
        assert_eq!(frame.pixel(398, 298).unwrap(), PanelColor::Red);
        assert_eq!(frame.pixel(399, 299).unwrap(), PanelColor::Red);
    }

    #[test]
    fn rectangle_edges_use_pixel_count_dimensions_and_clip() {
        let mut storage = [0_u8; FRAMEBUFFER_LEN];
        let mut frame = white_frame(&mut storage);
        let commands = [
            CanvasCommand::Rect {
                x: 397,
                y: 297,
                width: 5,
                height: 5,
                color: PanelColor::Yellow,
            },
            CanvasCommand::Rect {
                x: 10,
                y: 10,
                width: 0,
                height: 20,
                color: PanelColor::Black,
            },
        ];

        render_canvas(&mut frame, &commands).unwrap();
        for x in 397..400 {
            assert_eq!(frame.pixel(x, 297).unwrap(), PanelColor::Yellow);
        }
        for y in 297..300 {
            assert_eq!(frame.pixel(397, y).unwrap(), PanelColor::Yellow);
        }
        assert_eq!(frame.pixel(398, 298).unwrap(), PanelColor::White);
        assert_eq!(frame.pixel(10, 10).unwrap(), PanelColor::White);
    }

    #[test]
    fn text_uses_existing_glyphs_colors_scaling_and_clipping() {
        let mut storage = [0_u8; FRAMEBUFFER_LEN];
        let mut frame = white_frame(&mut storage);
        let commands = [CanvasCommand::Text {
            x: -2,
            y: -1,
            text: "A",
            scale: 2,
            color: PanelColor::Red,
        }];

        let stats = render_canvas(&mut frame, &commands).unwrap();
        assert_eq!(stats.text_bytes, 1);
        assert!(frame
            .as_bytes()
            .iter()
            .any(|byte| *byte != PanelColor::White.solid_byte()));
        assert_eq!(frame.pixel(0, 0).unwrap(), PanelColor::Red);
        assert_eq!(frame.pixel(10, 0).unwrap(), PanelColor::White);
    }

    #[test]
    fn quota_validation_precedes_framebuffer_mutation() {
        let too_many = [CanvasCommand::Clear {
            color: PanelColor::Black,
        }; CANVAS_MAX_COMMANDS + 1];
        assert_eq!(
            validate_canvas_commands(&too_many),
            Err(CanvasError::TooManyCommands {
                actual: CANVAS_MAX_COMMANDS + 1,
                maximum: CANVAS_MAX_COMMANDS,
            })
        );

        let long_text: String = "A".repeat(CANVAS_MAX_TEXT_BYTES + 1);
        let commands = [CanvasCommand::Text {
            x: 0,
            y: 0,
            text: &long_text,
            scale: 1,
            color: PanelColor::Black,
        }];
        let mut storage = [0_u8; FRAMEBUFFER_LEN];
        let mut frame = white_frame(&mut storage);
        assert_eq!(
            render_canvas(&mut frame, &commands),
            Err(CanvasError::TextBudgetExceeded {
                actual: CANVAS_MAX_TEXT_BYTES + 1,
                maximum: CANVAS_MAX_TEXT_BYTES,
            })
        );
        assert!(frame
            .as_bytes()
            .iter()
            .all(|byte| *byte == PanelColor::White.solid_byte()));
    }

    #[test]
    fn total_text_budget_is_aggregated_and_exact_limit_is_valid() {
        let text = "A".repeat(8);
        let commands: Vec<_> = (0..CANVAS_MAX_COMMANDS)
            .map(|_| CanvasCommand::Text {
                x: 0,
                y: 0,
                text: text.as_str(),
                scale: 1,
                color: PanelColor::Black,
            })
            .collect();
        assert_eq!(
            validate_canvas_commands(&commands),
            Ok(CanvasStats {
                command_count: CANVAS_MAX_COMMANDS,
                text_bytes: CANVAS_MAX_TEXT_BYTES,
            })
        );

        let parts = ["A".repeat(513), "B".repeat(512)];
        let over = [
            CanvasCommand::Text {
                x: 0,
                y: 0,
                text: &parts[0],
                scale: 1,
                color: PanelColor::Black,
            },
            CanvasCommand::Text {
                x: 0,
                y: 0,
                text: &parts[1],
                scale: 1,
                color: PanelColor::Black,
            },
        ];
        assert!(matches!(
            validate_canvas_commands(&over),
            Err(CanvasError::TextBudgetExceeded { actual: 1025, .. })
        ));
    }

    #[test]
    fn invalid_text_is_rejected_before_rendering() {
        let invalid_scale = [CanvasCommand::Text {
            x: 0,
            y: 0,
            text: "A",
            scale: 0,
            color: PanelColor::Black,
        }];
        assert_eq!(
            validate_canvas_commands(&invalid_scale),
            Err(CanvasError::InvalidScale)
        );

        let invalid_glyph = [CanvasCommand::Text {
            x: 0,
            y: 0,
            text: "a",
            scale: 1,
            color: PanelColor::Black,
        }];
        assert_eq!(
            validate_canvas_commands(&invalid_glyph),
            Err(CanvasError::UnsupportedGlyph('a'))
        );
    }

    #[test]
    fn extreme_geometry_is_checked_and_clipped_without_unbounded_walks() {
        let mut storage = [0_u8; FRAMEBUFFER_LEN];
        let mut frame = white_frame(&mut storage);
        let commands = [
            CanvasCommand::FillRect {
                x: i32::MAX,
                y: i32::MAX,
                width: u32::MAX,
                height: u32::MAX,
                color: PanelColor::Black,
            },
            CanvasCommand::Line {
                x0: i32::MIN,
                y0: 150,
                x1: i32::MAX,
                y1: 150,
                color: PanelColor::Yellow,
            },
            CanvasCommand::Line {
                x0: i32::MIN,
                y0: i32::MIN,
                x1: i32::MAX,
                y1: i32::MAX,
                color: PanelColor::Red,
            },
        ];

        render_canvas(&mut frame, &commands).unwrap();
        for x in 0..WIDTH {
            let expected = if x == 150 {
                PanelColor::Red
            } else {
                PanelColor::Yellow
            };
            assert_eq!(frame.pixel(x, 150).unwrap(), expected);
        }
        assert_eq!(frame.pixel(149, 149).unwrap(), PanelColor::Red);
        assert_eq!(frame.pixel(0, 149).unwrap(), PanelColor::White);
    }
}
