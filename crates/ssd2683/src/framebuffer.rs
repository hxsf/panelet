// SPDX-License-Identifier: MPL-2.0

use core::fmt;

pub const WIDTH: usize = 400;
pub const HEIGHT: usize = 300;
pub const PIXELS_PER_BYTE: usize = 4;
pub const ROW_BYTES: usize = WIDTH / PIXELS_PER_BYTE;
pub const FRAMEBUFFER_LEN: usize = ROW_BYTES * HEIGHT;

/// NOTE4C panel palette inferred from the archived four-color reference firmware.
///
/// This mapping is panel-specific implementation evidence, not a physical-color
/// mapping specified by the SSD2683 proposal:
///
/// - `00`: black
/// - `01`: white
/// - `10`: yellow
/// - `11`: red
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum PanelColor {
    Black = 0b00,
    White = 0b01,
    Yellow = 0b10,
    Red = 0b11,
}

impl PanelColor {
    #[must_use]
    pub const fn bits(self) -> u8 {
        self as u8
    }

    /// Four pixels of this color packed into one SSD2683 data byte.
    #[must_use]
    pub const fn solid_byte(self) -> u8 {
        let bits = self.bits();
        (bits << 6) | (bits << 4) | (bits << 2) | bits
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FramebufferError {
    OutOfBounds { x: usize, y: usize },
    ZeroCellSize,
}

impl fmt::Display for FramebufferError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfBounds { x, y } => write!(f, "pixel ({x}, {y}) is outside 400x300"),
            Self::ZeroCellSize => f.write_str("checkerboard cell size must be non-zero"),
        }
    }
}

/// A 400x300, 2-bpp framebuffer whose storage is chosen by the caller.
///
/// Borrowing the 30,000-byte array keeps this crate from silently placing a
/// large owned value on an embedded task stack. The board adapter may put the
/// array in internal RAM or in explicitly configured PSRAM.
pub struct Framebuffer<'a> {
    bytes: &'a mut [u8; FRAMEBUFFER_LEN],
}

impl<'a> Framebuffer<'a> {
    /// Borrow an existing packed frame without clearing its pixels.
    pub fn from_bytes(bytes: &'a mut [u8; FRAMEBUFFER_LEN]) -> Self { Self { bytes } }

    #[must_use]
    pub fn new(bytes: &'a mut [u8; FRAMEBUFFER_LEN], fill: PanelColor) -> Self {
        let mut framebuffer = Self { bytes };
        framebuffer.fill(fill);
        framebuffer
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        FRAMEBUFFER_LEN
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        false
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8; FRAMEBUFFER_LEN] {
        self.bytes
    }

    pub fn as_bytes_mut(&mut self) -> &mut [u8; FRAMEBUFFER_LEN] {
        self.bytes
    }

    pub fn fill(&mut self, color: PanelColor) {
        self.bytes.fill(color.solid_byte());
    }

    pub fn set_pixel(
        &mut self,
        x: usize,
        y: usize,
        color: PanelColor,
    ) -> Result<(), FramebufferError> {
        let (byte_index, shift) = pixel_location(x, y)?;
        let mask = 0b11_u8 << shift;
        self.bytes[byte_index] = (self.bytes[byte_index] & !mask) | (color.bits() << shift);
        Ok(())
    }

    pub fn pixel(&self, x: usize, y: usize) -> Result<PanelColor, FramebufferError> {
        let (byte_index, shift) = pixel_location(x, y)?;
        let bits = (self.bytes[byte_index] >> shift) & 0b11;
        Ok(match bits {
            0b00 => PanelColor::Black,
            0b01 => PanelColor::White,
            0b10 => PanelColor::Yellow,
            0b11 => PanelColor::Red,
            _ => unreachable!(),
        })
    }

    /// Fill four equal-width vertical bars in caller-selected order.
    ///
    /// `WIDTH` is divisible by four, so every bar is exactly 100 pixels wide.
    pub fn fill_vertical_bars(&mut self, colors: [PanelColor; 4]) {
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let bar = x / (WIDTH / colors.len());
                // Coordinates are generated within the fixed geometry.
                let _ = self.set_pixel(x, y, colors[bar]);
            }
        }
    }

    /// Draw an MSB-first packing diagnostic followed by a checkerboard.
    ///
    /// The first four pixels become black, white, yellow, red and therefore
    /// pack to `0x1B`. The remaining pixels use `first`/`second` cells.
    pub fn draw_packing_test_pattern(
        &mut self,
        first: PanelColor,
        second: PanelColor,
        cell_size: usize,
    ) -> Result<(), FramebufferError> {
        if cell_size == 0 {
            return Err(FramebufferError::ZeroCellSize);
        }

        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let color = if ((x / cell_size) + (y / cell_size)) % 2 == 0 {
                    first
                } else {
                    second
                };
                self.set_pixel(x, y, color)?;
            }
        }

        for (x, color) in [
            PanelColor::Black,
            PanelColor::White,
            PanelColor::Yellow,
            PanelColor::Red,
        ]
        .into_iter()
        .enumerate()
        {
            self.set_pixel(x, 0, color)?;
        }
        Ok(())
    }
}

fn pixel_location(x: usize, y: usize) -> Result<(usize, u32), FramebufferError> {
    if x >= WIDTH || y >= HEIGHT {
        return Err(FramebufferError::OutOfBounds { x, y });
    }

    let byte_index = y * ROW_BYTES + x / PIXELS_PER_BYTE;
    let shift = 6 - ((x % PIXELS_PER_BYTE) * 2);
    Ok((
        byte_index,
        u32::try_from(shift).expect("pixel shift is at most six"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_and_buffer_length_are_fixed() {
        assert_eq!(WIDTH, 400);
        assert_eq!(HEIGHT, 300);
        assert_eq!(ROW_BYTES, 100);
        assert_eq!(FRAMEBUFFER_LEN, 30_000);

        let mut storage = [0_u8; FRAMEBUFFER_LEN];
        let framebuffer = Framebuffer::new(&mut storage, PanelColor::White);
        assert_eq!(framebuffer.len(), 30_000);
        assert!(!framebuffer.is_empty());
    }

    #[test]
    fn reference_solid_color_bytes_are_explicit() {
        assert_eq!(PanelColor::Black.solid_byte(), 0x00);
        assert_eq!(PanelColor::White.solid_byte(), 0x55);
        assert_eq!(PanelColor::Yellow.solid_byte(), 0xAA);
        assert_eq!(PanelColor::Red.solid_byte(), 0xFF);
    }

    #[test]
    fn four_pixels_pack_msb_first_to_0x1b() {
        let mut storage = [0_u8; FRAMEBUFFER_LEN];
        let mut framebuffer = Framebuffer::new(&mut storage, PanelColor::Black);
        framebuffer.set_pixel(0, 0, PanelColor::Black).unwrap();
        framebuffer.set_pixel(1, 0, PanelColor::White).unwrap();
        framebuffer.set_pixel(2, 0, PanelColor::Yellow).unwrap();
        framebuffer.set_pixel(3, 0, PanelColor::Red).unwrap();

        assert_eq!(framebuffer.as_bytes()[0], 0x1B);
        assert_eq!(framebuffer.pixel(0, 0), Ok(PanelColor::Black));
        assert_eq!(framebuffer.pixel(1, 0), Ok(PanelColor::White));
        assert_eq!(framebuffer.pixel(2, 0), Ok(PanelColor::Yellow));
        assert_eq!(framebuffer.pixel(3, 0), Ok(PanelColor::Red));
    }

    #[test]
    fn bounds_errors_do_not_modify_the_buffer() {
        let mut storage = [PanelColor::White.solid_byte(); FRAMEBUFFER_LEN];
        let mut framebuffer = Framebuffer::new(&mut storage, PanelColor::White);

        assert_eq!(
            framebuffer.set_pixel(WIDTH, 0, PanelColor::Black),
            Err(FramebufferError::OutOfBounds { x: WIDTH, y: 0 })
        );
        assert_eq!(
            framebuffer.set_pixel(0, HEIGHT, PanelColor::Black),
            Err(FramebufferError::OutOfBounds { x: 0, y: HEIGHT })
        );
        assert!(framebuffer
            .as_bytes()
            .iter()
            .all(|byte| *byte == PanelColor::White.solid_byte()));
    }

    #[test]
    fn diagnostic_pattern_starts_with_packing_sentinel() {
        let mut storage = [0_u8; FRAMEBUFFER_LEN];
        let mut framebuffer = Framebuffer::new(&mut storage, PanelColor::White);
        framebuffer
            .draw_packing_test_pattern(PanelColor::White, PanelColor::Black, 8)
            .unwrap();
        assert_eq!(framebuffer.as_bytes()[0], 0x1B);
        assert_eq!(
            framebuffer.draw_packing_test_pattern(PanelColor::White, PanelColor::Black, 0),
            Err(FramebufferError::ZeroCellSize)
        );
    }
}
