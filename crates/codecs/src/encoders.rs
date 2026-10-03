// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Baseline encoders for the harness and the stage-sum benchmarks (ROADMAP M1.21): not the M2
//! write path, which adds verification, metadata policy and the safe-write protocol on top.
//!
//! * [`JpegRsEncoder`]: pure Rust, through the `jpeg-encoder` crate. That crate is
//!   `(MIT OR Apache-2.0) AND IJG`: its JPEG tables derive from the Independent JPEG Group's
//!   libjpeg, so the third-party notices carry the IJG acknowledgement (`about.hbs`).
//! * `JpegTurboEncoder` (feature `turbojpeg`): libjpeg-turbo's `tj3Compress8`, 4:2:0, with the
//!   libjpeg-turbo licence acknowledgement of the same notices.
//! * [`PngEncoder`]: the `png` crate, 8 and 16 bit, with the `pHYs` density and the `iCCP` profile.
//!
//! Which JPEG encoder is the default is a measured decision (ADR-0008).

use auto_crop_core::output::{Bits, OutFormat, Quality};
use auto_crop_core::ports::{EncodeMeta, Encoder, PixelFormat, RasterView, SampleSlice};
use auto_crop_core::{CancelToken, ErrKind, Interrupt, OutputSpec};
use std::borrow::Cow;

/// Quality used when the spec does not pin one (the M1.21 baseline).
pub const DEFAULT_JPEG_QUALITY: u8 = 90;

fn check(cancel: &CancelToken) -> Result<(), ErrKind> {
    cancel.check().map_err(|i| match i {
        Interrupt::Deadline => ErrKind::DeadlineExceeded,
        _ => ErrKind::Cancelled,
    })
}

/// The JPEG quality a spec asks for; `None` when the spec asks for something JPEG cannot do.
fn jpeg_quality(spec: &OutputSpec, default: u8) -> Option<u8> {
    if !matches!(spec.bits, Bits::Eight) {
        return None;
    }
    match spec.quality {
        Quality::Fixed { value } => Some(value.clamp(1, 100)),
        Quality::MatchSource { floor, cap } => Some(default.clamp(floor.min(cap), cap.max(floor))),
        Quality::Lossless => None,
    }
}

/// Contiguous 8-bit samples of `src` (16-bit samples are rounded down to their high byte).
fn samples_u8<'a>(src: &RasterView<'a>) -> Cow<'a, [u8]> {
    let (w, h) = (src.width() as usize, src.height() as usize);
    let row = w * src.format().channels();
    if let (SampleSlice::U8(s), true) = (src.samples(), src.stride() == row) {
        return Cow::Borrowed(&s[..row * h]);
    }
    let mut out = Vec::with_capacity(row * h);
    for y in 0..src.height() {
        if let Some(r) = src.row_u8(y) {
            out.extend_from_slice(r);
        } else if let Some(r) = src.row_u16(y) {
            out.extend(r.iter().map(|v| (v >> 8) as u8));
        }
    }
    Cow::Owned(out)
}

fn dpi_to_ppm(dpi: u32) -> u32 {
    ((f64::from(dpi) / 0.0254).round() as u64).min(u64::from(u32::MAX)) as u32
}

// ---------------------------------------------------------------------------------------------
// JPEG, pure Rust
// ---------------------------------------------------------------------------------------------

/// Baseline JPEG through the `jpeg-encoder` crate (4:2:0, 8-bit; RGB or grey).
#[derive(Debug, Clone, Copy)]
pub struct JpegRsEncoder {
    pub quality: u8,
}

impl Default for JpegRsEncoder {
    fn default() -> Self {
        Self {
            quality: DEFAULT_JPEG_QUALITY,
        }
    }
}

impl Encoder for JpegRsEncoder {
    fn format(&self) -> OutFormat {
        OutFormat::Jpeg
    }

    fn encode(
        &self,
        src: &RasterView<'_>,
        spec: &OutputSpec,
        meta: &EncodeMeta,
        cancel: &CancelToken,
    ) -> Result<Vec<u8>, ErrKind> {
        check(cancel)?;
        let quality = jpeg_quality(spec, self.quality).ok_or(ErrKind::UnsupportedOutput)?;
        let (w, h) = (
            u16::try_from(src.width()).map_err(|_| ErrKind::UnsupportedOutput)?,
            u16::try_from(src.height()).map_err(|_| ErrKind::UnsupportedOutput)?,
        );
        let data = samples_u8(src);
        let colour = match src.format().channels() {
            1 => jpeg_encoder::ColorType::Luma,
            _ => jpeg_encoder::ColorType::Rgb,
        };
        let mut out = Vec::new();
        let mut enc = jpeg_encoder::Encoder::new(&mut out, quality);
        enc.set_sampling_factor(jpeg_encoder::SamplingFactor::F_2_2);
        if let Some((x, y)) = meta.dpi {
            let clamp = |v: u32| u16::try_from(v).unwrap_or(u16::MAX).max(1);
            enc.set_density(jpeg_encoder::PixelDensity {
                density: (clamp(x), clamp(y)),
                unit: jpeg_encoder::PixelDensityUnit::Inches,
            });
        }
        if let Some(icc) = meta.icc.as_deref().filter(|p| !p.is_empty()) {
            enc.add_icc_profile(icc)
                .map_err(|_| ErrKind::EncodeFailed)?;
        }
        enc.encode(&data, w, h, colour)
            .map_err(|_| ErrKind::EncodeFailed)?;
        check(cancel)?;
        Ok(out)
    }
}

// ---------------------------------------------------------------------------------------------
// JPEG, libjpeg-turbo
// ---------------------------------------------------------------------------------------------

/// Baseline JPEG through libjpeg-turbo (4:2:0 for colour, grey stays grey).
#[cfg(feature = "turbojpeg")]
#[derive(Debug, Clone, Copy)]
pub struct JpegTurboEncoder {
    pub quality: u8,
}

#[cfg(feature = "turbojpeg")]
impl Default for JpegTurboEncoder {
    fn default() -> Self {
        Self {
            quality: DEFAULT_JPEG_QUALITY,
        }
    }
}

#[cfg(feature = "turbojpeg")]
impl Encoder for JpegTurboEncoder {
    fn format(&self) -> OutFormat {
        OutFormat::Jpeg
    }

    fn encode(
        &self,
        src: &RasterView<'_>,
        spec: &OutputSpec,
        meta: &EncodeMeta,
        cancel: &CancelToken,
    ) -> Result<Vec<u8>, ErrKind> {
        check(cancel)?;
        let quality = jpeg_quality(spec, self.quality).ok_or(ErrKind::UnsupportedOutput)?;
        let data = samples_u8(src);
        let out = crate::turbo::compress(
            &data,
            src.width() as usize,
            src.height() as usize,
            src.format().channels() == 1,
            quality,
            meta.dpi,
            meta.icc.as_deref(),
        )
        .map_err(|_| ErrKind::EncodeFailed)?;
        check(cancel)?;
        Ok(out)
    }
}

// ---------------------------------------------------------------------------------------------
// PNG
// ---------------------------------------------------------------------------------------------

/// PNG through the `png` crate: grey or RGB, 8 or 16 bit, optional `pHYs` and `iCCP`.
#[derive(Debug, Clone, Copy, Default)]
pub struct PngEncoder;

impl Encoder for PngEncoder {
    fn format(&self) -> OutFormat {
        OutFormat::Png
    }

    fn encode(
        &self,
        src: &RasterView<'_>,
        spec: &OutputSpec,
        meta: &EncodeMeta,
        cancel: &CancelToken,
    ) -> Result<Vec<u8>, ErrKind> {
        check(cancel)?;
        if !matches!(spec.bits, Bits::Eight) {
            return Err(ErrKind::UnsupportedOutput);
        }
        let (color, depth) = match src.format() {
            PixelFormat::Gray8 => (png::ColorType::Grayscale, png::BitDepth::Eight),
            PixelFormat::Rgb8 => (png::ColorType::Rgb, png::BitDepth::Eight),
            PixelFormat::Gray16 => (png::ColorType::Grayscale, png::BitDepth::Sixteen),
            PixelFormat::Rgb16 => (png::ColorType::Rgb, png::BitDepth::Sixteen),
        };
        let mut info = png::Info::with_size(src.width(), src.height());
        info.color_type = color;
        info.bit_depth = depth;
        if let Some((x, y)) = meta.dpi {
            info.pixel_dims = Some(png::PixelDimensions {
                xppu: dpi_to_ppm(x),
                yppu: dpi_to_ppm(y),
                unit: png::Unit::Meter,
            });
        }
        if let Some(icc) = meta.icc.as_deref().filter(|p| !p.is_empty()) {
            info.icc_profile = Some(Cow::Borrowed(icc));
        }
        // Rows in PNG order: 8-bit samples as they are, 16-bit as big-endian pairs.
        let row_samples = src.width() as usize * src.format().channels();
        let bytes_per_row = row_samples * src.format().bytes_per_sample();
        let mut data = Vec::with_capacity(bytes_per_row * src.height() as usize);
        for y in 0..src.height() {
            if let Some(r) = src.row_u8(y) {
                data.extend_from_slice(r);
            } else if let Some(r) = src.row_u16(y) {
                for v in r {
                    data.extend_from_slice(&v.to_be_bytes());
                }
            } else {
                return Err(ErrKind::Internal);
            }
            check(cancel)?;
        }
        let mut out = Vec::new();
        let mut enc = png::Encoder::with_info(&mut out, info).map_err(|_| ErrKind::EncodeFailed)?;
        enc.set_compression(png::Compression::Balanced);
        let mut writer = enc.write_header().map_err(|_| ErrKind::EncodeFailed)?;
        writer
            .write_image_data(&data)
            .map_err(|_| ErrKind::EncodeFailed)?;
        writer.finish().map_err(|_| ErrKind::EncodeFailed)?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests;
