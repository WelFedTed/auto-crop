// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Pure image-processing kernels. Must not depend on any codec or I/O crate.

pub mod cancel;
pub mod detect;
pub mod geometry;
pub mod homography;
pub mod minify;
pub mod pixels;
pub mod pyramid;
pub mod render;
pub mod scale;
pub mod synth;
pub mod threshold;
pub mod warp;

/// Pixel dimensions of an image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

impl Size {
    /// Total pixel count (as u64 to avoid overflow above ~4 GP).
    pub fn pixels(self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }
}

/// An 8-bit RGB image, row-major, no padding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl Raster {
    /// A black image. Panics only if the byte count overflows `usize`.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            data: vec![0; width as usize * height as usize * 3],
        }
    }

    pub fn filled(width: u32, height: u32, rgb: [u8; 3]) -> Self {
        let mut r = Self::new(width, height);
        for px in r.data.as_chunks_mut::<3>().0 {
            *px = rgb;
        }
        r
    }

    /// Wraps existing bytes; `None` if the length does not match.
    pub fn from_raw(width: u32, height: u32, data: Vec<u8>) -> Option<Self> {
        (data.len() == width as usize * height as usize * 3).then_some(Self {
            width,
            height,
            data,
        })
    }

    pub fn size(&self) -> Size {
        Size {
            width: self.width,
            height: self.height,
        }
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 3] {
        let i = (y as usize * self.width as usize + x as usize) * 3;
        [self.data[i], self.data[i + 1], self.data[i + 2]]
    }

    pub fn set_pixel(&mut self, x: u32, y: u32, rgb: [u8; 3]) {
        let i = (y as usize * self.width as usize + x as usize) * 3;
        self.data[i..i + 3].copy_from_slice(&rgb);
    }

    /// Rotates by `turns` clockwise quarter turns (0..=3).
    pub fn rotated_quarter_turns(&self, turns: u8) -> Raster {
        let (w, h) = (self.width as usize, self.height as usize);
        match turns % 4 {
            0 => self.clone(),
            1 => {
                let mut out = Raster::new(self.height, self.width);
                for y in 0..h {
                    for x in 0..w {
                        let (nx, ny) = (h - 1 - y, x);
                        let s = (y * w + x) * 3;
                        let d = (ny * h + nx) * 3;
                        out.data[d..d + 3].copy_from_slice(&self.data[s..s + 3]);
                    }
                }
                out
            }
            2 => {
                let mut out = Raster::new(self.width, self.height);
                for y in 0..h {
                    for x in 0..w {
                        let s = (y * w + x) * 3;
                        let d = ((h - 1 - y) * w + (w - 1 - x)) * 3;
                        out.data[d..d + 3].copy_from_slice(&self.data[s..s + 3]);
                    }
                }
                out
            }
            _ => {
                let mut out = Raster::new(self.height, self.width);
                for y in 0..h {
                    for x in 0..w {
                        let (nx, ny) = (y, w - 1 - x);
                        let s = (y * w + x) * 3;
                        let d = (ny * h + nx) * 3;
                        out.data[d..d + 3].copy_from_slice(&self.data[s..s + 3]);
                    }
                }
                out
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_count_does_not_overflow_u32() {
        let s = Size {
            width: 100_000,
            height: 100_000,
        };
        assert_eq!(s.pixels(), 10_000_000_000);
    }

    #[test]
    fn from_raw_checks_the_length() {
        assert!(Raster::from_raw(2, 2, vec![0; 12]).is_some());
        assert!(Raster::from_raw(2, 2, vec![0; 11]).is_none());
    }

    #[test]
    fn four_quarter_turns_are_the_identity() {
        let mut r = Raster::new(3, 2);
        for (i, b) in r.data.iter_mut().enumerate() {
            *b = i as u8;
        }
        let mut t = r.clone();
        for _ in 0..4 {
            t = t.rotated_quarter_turns(1);
        }
        assert_eq!(t, r);
        assert_eq!(r.rotated_quarter_turns(1).rotated_quarter_turns(3), r);
        assert_eq!(
            r.rotated_quarter_turns(2),
            r.rotated_quarter_turns(1).rotated_quarter_turns(1)
        );
    }

    #[test]
    fn a_clockwise_turn_moves_the_top_left_pixel_to_the_top_right() {
        let mut r = Raster::new(3, 2);
        r.set_pixel(0, 0, [9, 8, 7]);
        let t = r.rotated_quarter_turns(1);
        assert_eq!((t.width, t.height), (2, 3));
        assert_eq!(t.pixel(1, 0), [9, 8, 7]);
    }
}
