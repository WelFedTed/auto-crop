// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Lossless WebP writer (ROADMAP M11.21 lossless half; PLAN 3.3 "WebP").
//!
//! Pure Rust, through `image-webp`'s VP8L encoder: the pixels come back exactly, so a lossless
//! source (or a conversion from PNG or TIFF) loses nothing, and a WebP source can be replaced in
//! place only when it was lossless itself. **There is no lossy mode here**: lossy WebP needs
//! libwebp (C, BSD-3, built with CMake in CI only), which is a later cargo feature; until then the
//! quality in the `OutputSpec` is ignored and the format is labelled "WebP (lossless)" wherever the
//! user sees it.
//!
//! * 8-bit grey or RGB only (a 16-bit raster is `UnsupportedOutput`: the caller reduces it with a
//!   notice, PLAN 3.7); the engine's rasters have no alpha, so none is written.
//! * VP8L stores width and height in 14 bits: more than 16,383 pixels on either side is refused.
//! * The ICC profile goes in an `ICCP` chunk byte-exact; the EXIF blob (a bare TIFF blob, no
//!   `Exif\0\0` prefix; Orientation already 1, set by the caller) and XMP packet go in `EXIF` and
//!   `XMP ` chunks, in the extended (`VP8X`) container. There is no dpi in WebP outside EXIF.

use super::{check, samples_u8};
use auto_crop_core::output::{Bits, OutFormat};
use auto_crop_core::ports::{EncodeMeta, Encoder, RasterView};
use auto_crop_core::{CancelToken, ErrKind, OutputSpec};

/// The largest side of a WebP image.
pub const WEBP_MAX_SIDE: u32 = 16_383;

#[derive(Debug, Clone, Default)]
pub struct WebpEncoder {
    /// A bare TIFF-structured EXIF blob, already patched (Orientation 1, GPS per policy).
    pub exif: Option<Vec<u8>>,
    /// An XMP packet.
    pub xmp: Option<Vec<u8>>,
}

impl Encoder for WebpEncoder {
    fn format(&self) -> OutFormat {
        OutFormat::Webp
    }

    fn encode(
        &self,
        src: &RasterView<'_>,
        spec: &OutputSpec,
        meta: &EncodeMeta,
        cancel: &CancelToken,
    ) -> Result<Vec<u8>, ErrKind> {
        check(cancel)?;
        let (w, h) = (src.width(), src.height());
        if !matches!(spec.bits, Bits::Eight)
            || src.format().bytes_per_sample() != 1
            || w == 0
            || h == 0
            || w > WEBP_MAX_SIDE
            || h > WEBP_MAX_SIDE
        {
            return Err(ErrKind::UnsupportedOutput);
        }
        let data = samples_u8(src);
        let colour = if src.format().channels() == 1 {
            image_webp::ColorType::L8
        } else {
            image_webp::ColorType::Rgb8
        };
        let mut out = Vec::new();
        let mut enc = image_webp::WebPEncoder::new(&mut out);
        if let Some(icc) = meta.icc.as_deref().filter(|p| !p.is_empty()) {
            enc.set_icc_profile(icc.to_vec());
        }
        if let Some(e) = self.exif.as_deref().filter(|e| !e.is_empty()) {
            enc.set_exif_metadata(e.to_vec());
        }
        if let Some(x) = self.xmp.as_deref().filter(|x| !x.is_empty()) {
            enc.set_xmp_metadata(x.to_vec());
        }
        enc.encode(&data, w, h, colour)
            .map_err(|_| ErrKind::EncodeFailed)?;
        check(cancel)?;
        Ok(out)
    }
}
