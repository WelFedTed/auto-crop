// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Image decoders and encoders (PLAN 3). Decoding of untrusted bytes is pure safe Rust (JPEG via
//! zune-jpeg, PNG, TIFF and WebP through the `image` crate) behind hard limits and a panic guard;
//! the libjpeg-turbo and HEIC paths of M1.18, M1.19 and M6 extend this crate later.
//!
//! The pipeline for every file:
//!
//! 1. [`sniff`] the magic bytes (the file name is never trusted);
//! 2. [`probe_with`] reads the header with no pixel allocation (dimensions, depth, frames, EXIF
//!    orientation, ICC size) and enforces the metadata, scan and frame caps;
//! 3. [`decode_with`] checks the pixel cap and the memory estimate, then decodes with
//!    `image::Limits`, applies the EXIF orientation once, keeps the ICC profile byte-exact and
//!    reports every loss as a notice;
//! 4. every public entry point catches panics ([`guard`]), so a hostile file fails one item.
//!
//! Limits of this stage (all tracked in ROADMAP): the raster is RGB8 (16-bit sources are reduced
//! with a notice until M1.08 adds 16-bit rasters), only the colour data and the ICC profile survive
//! a re-encode, and JPEG output is always a full re-encode.

mod decode;
mod error;
mod exif;
#[cfg(any(test, feature = "fixtures"))]
pub mod fixtures;
mod format;
pub mod guard;
#[cfg(any(test, feature = "fixtures"))]
pub mod hostile;
pub mod jpeg_lossless;
mod limits;
mod parse;
#[cfg(test)]
mod suite;

pub use decode::{Decoded, decode, decode_with, probe, probe_with};
pub use error::CodecError;
pub use exif::orientation as exif_orientation;
pub use format::{Format, sniff};
pub use guard::{decode_guarded, guard_item, guard_item_timeout, pool_panic_handler};
pub use limits::{
    DEFAULT_MAX_FILE_BYTES, DEFAULT_MAX_FILE_BYTES_STREAMED, DEFAULT_MAX_FRAMES,
    DEFAULT_MAX_METADATA_BYTES, DEFAULT_MAX_PIXELS, DEFAULT_MAX_SCANS, DecodeLimits,
    HARD_MAX_PIXELS, Limit, est_bytes, est_bytes_for_pixels,
};

use auto_crop_imgproc::Raster;
use image::ImageEncoder;

/// Largest image accepted by default (PLAN C1: 100 MP).
pub const MAX_PIXELS: u64 = DEFAULT_MAX_PIXELS;

/// Formats this build can decode.
pub fn supported_input_formats() -> &'static [&'static str] {
    &["jpeg", "png", "tiff", "webp"]
}

/// What the header says about a file; produced without allocating pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Probe {
    pub format: Format,
    /// Stored size, before any EXIF turn.
    pub width: u32,
    pub height: u32,
    /// Bits per sample as stored.
    pub bit_depth: u8,
    /// Samples per pixel as stored (JPEG components, TIFF samples, PNG colour type).
    pub channels: u8,
    /// Frames, pages or IFDs (1 for a still).
    pub frames: u32,
    /// JPEG scan count (1 for other formats).
    pub scans: u32,
    /// EXIF orientation 1..=8 (1 when absent).
    pub orientation: u8,
    /// Embedded ICC profile size as stored (compressed for PNG), if any.
    pub icc_len: Option<u32>,
}

/// Encodes RGB8. `quality` (1..=100) applies to JPEG only; PNG is lossless. A panic inside an
/// encoder becomes [`CodecError::InternalPanic`] (ROADMAP M1.14).
pub fn encode(
    raster: &Raster,
    format: Format,
    quality: u8,
    icc: Option<&[u8]>,
) -> Result<Vec<u8>, CodecError> {
    guard_item(|| encode_inner(raster, format, quality, icc))
}

fn encode_inner(
    raster: &Raster,
    format: Format,
    quality: u8,
    icc: Option<&[u8]>,
) -> Result<Vec<u8>, CodecError> {
    let mut out = Vec::new();
    let (w, h) = (raster.width, raster.height);
    let err = |e: image::ImageError| CodecError::Encode(e.to_string());
    match format {
        Format::Jpeg => {
            let mut enc =
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100));
            if let Some(p) = icc {
                let _ = enc.set_icc_profile(p.to_vec());
            }
            enc.write_image(&raster.data, w, h, image::ExtendedColorType::Rgb8)
                .map_err(err)?;
        }
        Format::Png => {
            let mut enc = image::codecs::png::PngEncoder::new_with_quality(
                &mut out,
                image::codecs::png::CompressionType::Default,
                image::codecs::png::FilterType::Adaptive,
            );
            if let Some(p) = icc {
                let _ = enc.set_icc_profile(p.to_vec());
            }
            enc.write_image(&raster.data, w, h, image::ExtendedColorType::Rgb8)
                .map_err(err)?;
        }
        other => {
            return Err(CodecError::Encode(format!(
                "writing {} is not supported in this build",
                other.extension()
            )));
        }
    }
    Ok(out)
}

// tests live in tests/ and in the module test blocks
