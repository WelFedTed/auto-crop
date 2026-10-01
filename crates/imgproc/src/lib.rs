// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Pure image-processing kernels. Must not depend on any codec or I/O crate.

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
}
