// SPDX-License-Identifier: MPL-2.0

use core::fmt;

use crate::{HEIGHT, PIXELS_PER_BYTE, WIDTH};

/// Inclusive NOTE4C partial-window coordinates for SSD2683 PTLW (`0x83`).
///
/// This type only validates and encodes controller coordinates. It is not
/// evidence that the NOTE4C four-color panel supports partial refresh safely.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PartialWindow {
    x_start: u16,
    x_end: u16,
    y_start: u16,
    y_end: u16,
}

impl PartialWindow {
    pub fn new(
        x_start: u16,
        x_end: u16,
        y_start: u16,
        y_end: u16,
    ) -> Result<Self, PartialWindowError> {
        if usize::from(x_start) >= WIDTH
            || usize::from(x_end) >= WIDTH
            || usize::from(y_start) >= HEIGHT
            || usize::from(y_end) >= HEIGHT
        {
            return Err(PartialWindowError::OutOfBounds {
                x_start,
                x_end,
                y_start,
                y_end,
            });
        }
        if x_start > x_end || y_start > y_end {
            return Err(PartialWindowError::Reversed {
                x_start,
                x_end,
                y_start,
                y_end,
            });
        }
        if usize::from(x_start) % PIXELS_PER_BYTE != 0 {
            return Err(PartialWindowError::UnalignedXStart { x_start });
        }
        if usize::from(x_end) % PIXELS_PER_BYTE != PIXELS_PER_BYTE - 1 {
            return Err(PartialWindowError::UnalignedXEnd { x_end });
        }

        Ok(Self {
            x_start,
            x_end,
            y_start,
            y_end,
        })
    }

    #[must_use]
    pub const fn x_start(self) -> u16 {
        self.x_start
    }

    #[must_use]
    pub const fn x_end(self) -> u16 {
        self.x_end
    }

    #[must_use]
    pub const fn y_start(self) -> u16 {
        self.y_start
    }

    #[must_use]
    pub const fn y_end(self) -> u16 {
        self.y_end
    }

    /// Number of packed 2-bpp bytes represented by this inclusive window.
    #[must_use]
    pub const fn packed_len(self) -> usize {
        let width = (self.x_end - self.x_start + 1) as usize;
        let height = (self.y_end - self.y_start + 1) as usize;
        (width / PIXELS_PER_BYTE) * height
    }

    /// Encode the proposal's nine PTLW data bytes.
    #[must_use]
    pub const fn encode(self, enable: bool) -> [u8; 9] {
        [
            ((self.x_start >> 8) & 0x03) as u8,
            self.x_start as u8,
            ((self.x_end >> 8) & 0x03) as u8,
            self.x_end as u8,
            ((self.y_start >> 8) & 0x03) as u8,
            self.y_start as u8,
            ((self.y_end >> 8) & 0x03) as u8,
            self.y_end as u8,
            enable as u8,
        ]
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PartialWindowError {
    OutOfBounds {
        x_start: u16,
        x_end: u16,
        y_start: u16,
        y_end: u16,
    },
    Reversed {
        x_start: u16,
        x_end: u16,
        y_start: u16,
        y_end: u16,
    },
    UnalignedXStart {
        x_start: u16,
    },
    UnalignedXEnd {
        x_end: u16,
    },
}

impl fmt::Display for PartialWindowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfBounds {
                x_start,
                x_end,
                y_start,
                y_end,
            } => write!(
                f,
                "partial window ({x_start}..={x_end}, {y_start}..={y_end}) exceeds 400x300"
            ),
            Self::Reversed {
                x_start,
                x_end,
                y_start,
                y_end,
            } => write!(
                f,
                "partial window coordinates are reversed: ({x_start}..={x_end}, {y_start}..={y_end})"
            ),
            Self::UnalignedXStart { x_start } => {
                write!(f, "partial x_start {x_start} is not divisible by four")
            }
            Self::UnalignedXEnd { x_end } => {
                write!(f, "partial x_end {x_end} must be congruent to three modulo four")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn center_window_encodes_nine_bytes_and_packed_length() {
        let window = PartialWindow::new(160, 239, 110, 189).unwrap();
        assert_eq!(
            window.encode(true),
            [0x00, 0xA0, 0x00, 0xEF, 0x00, 0x6E, 0x00, 0xBD, 0x01]
        );
        assert_eq!(
            window.encode(false),
            [0x00, 0xA0, 0x00, 0xEF, 0x00, 0x6E, 0x00, 0xBD, 0x00]
        );
        assert_eq!(window.packed_len(), 1_600);
    }

    #[test]
    fn full_note4c_window_encodes_high_coordinate_bits() {
        let window = PartialWindow::new(0, 399, 0, 299).unwrap();
        assert_eq!(
            window.encode(true),
            [0x00, 0x00, 0x01, 0x8F, 0x00, 0x00, 0x01, 0x2B, 0x01]
        );
        assert_eq!(window.packed_len(), 30_000);
    }

    #[test]
    fn invalid_bounds_order_and_horizontal_alignment_are_rejected() {
        assert!(matches!(
            PartialWindow::new(0, 403, 0, 299),
            Err(PartialWindowError::OutOfBounds { .. })
        ));
        assert!(matches!(
            PartialWindow::new(160, 159, 110, 189),
            Err(PartialWindowError::Reversed { .. })
        ));
        assert_eq!(
            PartialWindow::new(161, 239, 110, 189),
            Err(PartialWindowError::UnalignedXStart { x_start: 161 })
        );
        assert_eq!(
            PartialWindow::new(160, 238, 110, 189),
            Err(PartialWindowError::UnalignedXEnd { x_end: 238 })
        );
        assert!(matches!(
            PartialWindow::new(160, 239, 0, 300),
            Err(PartialWindowError::OutOfBounds { .. })
        ));
    }
}
