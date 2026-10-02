// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Decode limits for untrusted input (PLAN 3.10.1, ROADMAP M1.13).
//!
//! Every field is checked in a fixed order: file size, then the header probe (which enforces the
//! metadata, scan and frame caps), then the pixel cap, then the estimated decode memory. Only after
//! all of those pass is a pixel buffer allocated, and the decoder itself is given an
//! `image::Limits` derived from the same numbers. A header claiming 60000 x 60000 is therefore
//! refused after reading a few dozen bytes.

use crate::Format;

const MIB: u64 = 1 << 20;

/// Default pixel cap: the tested class of image (PLAN C1).
pub const DEFAULT_MAX_PIXELS: u64 = 100_000_000;
/// Absolute pixel ceiling that the Advanced setting, `--max-pixels` and "allow this file" can reach.
pub const HARD_MAX_PIXELS: u64 = 500_000_000;
/// File size cap for formats that are decoded from memory (PLAN 3.10.1, PROVISIONAL).
pub const DEFAULT_MAX_FILE_BYTES: u64 = 256 * MIB;
/// File size cap for the formats that can be streamed (PNG, TIFF; PLAN 3.10.1, PROVISIONAL). The
/// decoders here still read from a byte slice, so this only matters once streaming exists.
pub const DEFAULT_MAX_FILE_BYTES_STREAMED: u64 = 2048 * MIB;
/// Cap on each EXIF, XMP and ICC blob (PLAN 3.10.1).
pub const DEFAULT_MAX_METADATA_BYTES: u64 = 16 * MIB;
/// Progressive JPEG scan cap (PLAN 3.10.1).
pub const DEFAULT_MAX_SCANS: u32 = 100;
/// Frame, page and item cap for the probe (PLAN 3.10.1).
pub const DEFAULT_MAX_FRAMES: u32 = 10_000;

/// Estimated peak decode memory for an image: the engine's admission weight, `pixels x 9 + 64 MiB`
/// (PLAN 2.8). Saturates instead of overflowing.
pub fn est_bytes(width: u32, height: u32) -> u64 {
    est_bytes_for_pixels(u64::from(width) * u64::from(height))
}

/// [`est_bytes`] from a pixel count.
pub fn est_bytes_for_pixels(pixels: u64) -> u64 {
    pixels.saturating_mul(9).saturating_add(64 * MIB)
}

/// Which limit was exceeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    Pixels,
    FileBytes,
    EstBytes,
    MetadataBytes,
    Scans,
    Frames,
}

impl Limit {
    /// Stable machine name (for logs and `--json`).
    pub fn name(self) -> &'static str {
        match self {
            Limit::Pixels => "max_pixels",
            Limit::FileBytes => "max_file_bytes",
            Limit::EstBytes => "max_est_bytes",
            Limit::MetadataBytes => "max_metadata_bytes",
            Limit::Scans => "max_scans",
            Limit::Frames => "max_frames",
        }
    }
}

/// Caps applied to every decode of untrusted bytes. `Default` is the shipped policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeLimits {
    /// Largest `width x height` accepted (default 100 MP, ceiling 500 MP).
    pub max_pixels: u64,
    /// Largest file for JPEG and WebP (default 256 MiB).
    pub max_file_bytes: u64,
    /// Largest file for PNG and TIFF, which can be streamed (default 2 GiB).
    pub max_file_bytes_streamed: u64,
    /// Largest EXIF, XMP or ICC blob (default 16 MiB each).
    pub max_metadata_bytes: u64,
    /// Most progressive JPEG scans (default 100).
    pub max_scans: u32,
    /// Most frames, pages or IFDs a file may declare (default 10,000).
    pub max_frames: u32,
    /// Largest [`est_bytes`] admitted (default: the estimate for `max_pixels`).
    pub max_est_bytes: u64,
    /// Soft wall-clock limit for `decode_guarded`; `None` = none. In-process decoders cannot be
    /// pre-empted, so a timeout abandons the result and the thread runs on (PLAN 3.10.1).
    pub max_decode_ms: Option<u64>,
}

impl Default for DecodeLimits {
    fn default() -> Self {
        Self {
            max_pixels: DEFAULT_MAX_PIXELS,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            max_file_bytes_streamed: DEFAULT_MAX_FILE_BYTES_STREAMED,
            max_metadata_bytes: DEFAULT_MAX_METADATA_BYTES,
            max_scans: DEFAULT_MAX_SCANS,
            max_frames: DEFAULT_MAX_FRAMES,
            max_est_bytes: est_bytes_for_pixels(DEFAULT_MAX_PIXELS),
            max_decode_ms: None,
        }
    }
}

impl DecodeLimits {
    /// Raises or lowers the pixel cap (and the memory estimate cap with it). Requests above
    /// [`HARD_MAX_PIXELS`] are clamped to it: there is no way to lift the ceiling.
    pub fn with_max_pixels(mut self, pixels: u64) -> Self {
        self.max_pixels = pixels.min(HARD_MAX_PIXELS);
        self.max_est_bytes = est_bytes_for_pixels(self.max_pixels);
        self
    }

    /// The file size cap that applies to `format`.
    pub fn file_cap(&self, format: Format) -> u64 {
        match format {
            Format::Png | Format::Tiff => self.max_file_bytes_streamed,
            _ => self.max_file_bytes,
        }
    }

    /// The `image::Limits` handed to the decoder (the second guard behind the declared-size
    /// checks; `image`'s own allocation limit is not strict for every decoder, PLAN 3.10.1).
    pub(crate) fn image_limits(&self, width: u32, height: u32) -> image::Limits {
        let mut l = image::Limits::default();
        l.max_image_width = Some(width);
        l.max_image_height = Some(height);
        l.max_alloc = Some(self.max_est_bytes);
        l
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_plan() {
        let l = DecodeLimits::default();
        assert_eq!(l.max_pixels, 100_000_000);
        assert_eq!(l.max_metadata_bytes, 16 << 20);
        assert_eq!(l.max_scans, 100);
        assert_eq!(l.max_frames, 10_000);
        assert_eq!(l.max_file_bytes, 256 << 20);
        assert_eq!(l.max_est_bytes, 100_000_000 * 9 + (64 << 20));
    }

    #[test]
    fn pixel_cap_has_a_hard_ceiling() {
        let l = DecodeLimits::default().with_max_pixels(u64::MAX);
        assert_eq!(l.max_pixels, HARD_MAX_PIXELS);
        assert_eq!(l.max_est_bytes, est_bytes_for_pixels(HARD_MAX_PIXELS));
        assert_eq!(
            DecodeLimits::default().with_max_pixels(1000).max_pixels,
            1000
        );
    }

    #[test]
    fn est_bytes_saturates() {
        assert_eq!(est_bytes(10, 10), 900 + (64 << 20));
        assert_eq!(est_bytes_for_pixels(u64::MAX), u64::MAX);
        assert_eq!(est_bytes(60_000, 60_000), 3_600_000_000 * 9 + (64 << 20));
    }
}
