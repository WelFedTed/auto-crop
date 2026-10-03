// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The decode boundary (PLAN 3.1 rule 5): bytes in, one RGB8 raster out, EXIF orientation applied
//! once, ICC kept byte-exact, every loss named as a notice.

use crate::parse::{self, Header, IccLoc};
use crate::{
    CodecError, DecodeLimits, Format, Limit, Probe, est_bytes_for_pixels, guard::guard_item, sniff,
};
use auto_crop_imgproc::Raster;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, metadata::Orientation};
use std::io::Cursor;

/// A decoded image.
#[derive(Debug, Clone)]
pub struct Decoded {
    /// Pixels with the EXIF orientation already applied.
    pub raster: Raster,
    pub format: Format,
    /// The EXIF orientation found (1 = none), already applied to `raster`.
    pub exif_orientation: u8,
    pub icc: Option<Vec<u8>>,
    /// Bits per sample of the source (the raster is always 8-bit until the 16-bit `Raster` of
    /// ROADMAP M1.08 lands).
    pub source_bit_depth: u8,
    /// Frames, pages or IFDs the file declares; only the first was decoded.
    pub frames: u32,
    /// Stable codes for every loss or oddity, e.g. `tiff.multi_page`, `anim.first_frame_only`,
    /// `alpha.dropped`, `depth.reduced_to_8`, `cmyk.naive_conversion` (PLAN 3.1 rule 7).
    pub notices: Vec<&'static str>,
}

fn limit_err(limit: Limit, actual: u64, cap: u64) -> CodecError {
    CodecError::LimitExceeded { limit, actual, cap }
}

/// Reads the header only: format, stored size, depth, frame count, orientation and ICC size. No
/// pixel buffer is allocated; the metadata, scan and frame caps apply, the pixel cap does not (so a
/// caller can report the real size of a refused image).
pub fn probe_with(bytes: &[u8], limits: &DecodeLimits) -> Result<Probe, CodecError> {
    guard_item(|| {
        let (format, h) = sniff_and_parse(bytes, limits)?;
        Ok(probe_of(format, &h))
    })
}

/// [`probe_with`] under the default limits.
pub fn probe(bytes: &[u8]) -> Result<Probe, CodecError> {
    probe_with(bytes, &DecodeLimits::default())
}

fn sniff_and_parse(bytes: &[u8], limits: &DecodeLimits) -> Result<(Format, Header), CodecError> {
    let format = sniff(bytes).ok_or(CodecError::Unsupported)?;
    if !format.is_decodable() {
        return Err(CodecError::NotDecodable(format));
    }
    let cap = limits.file_cap(format);
    if bytes.len() as u64 > cap {
        return Err(limit_err(Limit::FileBytes, bytes.len() as u64, cap));
    }
    Ok((format, parse::parse(bytes, format, limits)?))
}

fn probe_of(format: Format, h: &Header) -> Probe {
    Probe {
        format,
        width: h.width,
        height: h.height,
        bit_depth: h.bit_depth,
        channels: h.channels,
        frames: h.frames,
        scans: h.scans,
        orientation: h.orientation,
        icc_len: h.icc_len,
    }
}

/// Decodes to RGB8 with the EXIF orientation applied once, under the default limits.
pub fn decode(bytes: &[u8]) -> Result<Decoded, CodecError> {
    decode_with(bytes, &DecodeLimits::default())
}

/// Decodes under explicit limits. Order: file size, header probe (metadata, scans, frames), pixel
/// cap, memory estimate, then the decoder with `image::Limits`. Never panics: a panic inside a
/// decoder becomes [`CodecError::InternalPanic`].
pub fn decode_with(bytes: &[u8], limits: &DecodeLimits) -> Result<Decoded, CodecError> {
    guard_item(|| decode_inner(bytes, limits))
}

fn image_format(f: Format) -> ImageFormat {
    match f {
        Format::Jpeg => ImageFormat::Jpeg,
        Format::Png => ImageFormat::Png,
        Format::Tiff => ImageFormat::Tiff,
        _ => ImageFormat::WebP,
    }
}

/// Checks `width x height` against the pixel cap and the memory estimate.
fn check_size(width: u32, height: u32, limits: &DecodeLimits) -> Result<(), CodecError> {
    let pixels = u64::from(width) * u64::from(height);
    if pixels > limits.max_pixels {
        return Err(CodecError::TooLarge(pixels));
    }
    let est = est_bytes_for_pixels(pixels);
    if est > limits.max_est_bytes {
        return Err(limit_err(Limit::EstBytes, est, limits.max_est_bytes));
    }
    Ok(())
}

/// Every check that precedes the pixel decoder: file size, header probe, pixel cap, memory
/// estimate, unsupported features and the plausibility rules that stop a tiny file from reserving
/// a huge buffer. Shared by the `image`/zune path and the libjpeg-turbo path.
pub(crate) fn precheck(
    bytes: &[u8],
    limits: &DecodeLimits,
) -> Result<(Format, Header), CodecError> {
    let (format, h) = sniff_and_parse(bytes, limits)?;
    check_size(h.width, h.height, limits)?;
    if let Some(reason) = &h.unsupported {
        return Err(CodecError::UnsupportedFeature(reason.clone()));
    }
    if format == Format::Jpeg {
        parse::jpeg::check_plausible(&h, bytes.len())?;
    }
    if format == Format::Png {
        // Deflate cannot shrink data by more than about 1032:1, so a PNG much smaller than its
        // unpacked size divided by that cannot hold the image it declares. Refusing it here keeps
        // a 140-byte file from making the decoder reserve the whole declared buffer.
        let bits_row = u64::from(h.width) * u64::from(h.bit_depth) * u64::from(h.channels);
        let raw = u64::from(h.height).saturating_mul(1 + bits_row.div_ceil(8));
        if (bytes.len() as u64) < raw / 1100 {
            return Err(CodecError::corrupt(
                "PNG data is too short for the dimensions in its header",
            ));
        }
    }
    if format == Format::Tiff && h.compression == 1 {
        // Uncompressed pixel data must be present in full.
        let bits_row = u64::from(h.width) * u64::from(h.bit_depth) * u64::from(h.channels);
        let raw = u64::from(h.height).saturating_mul(bits_row.div_ceil(8));
        if (bytes.len() as u64) < raw {
            return Err(CodecError::corrupt(
                "uncompressed TIFF is smaller than its declared pixel data",
            ));
        }
    }
    if h.truncated && format == Format::Jpeg {
        // A partial JPEG decodes "successfully" with grey fill; overwriting an original with that
        // would be silent data loss (B3, B4), so it is refused.
        return Err(CodecError::corrupt(
            "truncated JPEG (no end-of-image marker)",
        ));
    }
    Ok((format, h))
}

fn decode_inner(bytes: &[u8], limits: &DecodeLimits) -> Result<Decoded, CodecError> {
    decode_raw(bytes, limits, true)
}

/// The decode without (or with) the EXIF turn: scaled decode reduces the stored pixels first and
/// turns afterwards.
pub(crate) fn decode_raw(
    bytes: &[u8],
    limits: &DecodeLimits,
    apply_orientation: bool,
) -> Result<Decoded, CodecError> {
    let (format, h) = precheck(bytes, limits)?;

    let mut reader = ImageReader::with_format(Cursor::new(bytes), image_format(format));
    reader.limits(limits.image_limits(h.width, h.height));
    let pixels = u64::from(h.width) * u64::from(h.height);
    let to_err = |e| map_image_err(e, pixels, limits);
    let mut decoder = reader.into_decoder().map_err(to_err)?;
    // The decoder's own idea of the size must also be inside the caps and must agree with ours.
    let (dw, dh) = decoder.dimensions();
    check_size(dw, dh, limits)?;
    if (dw, dh) != (h.width, h.height) {
        return Err(CodecError::corrupt(
            "header and decoder disagree about the image size",
        ));
    }
    let colour = decoder.color_type();
    let mut notices: Vec<&'static str> = Vec::new();
    let png_icc = if h.icc == IccLoc::PngCompressed {
        decoder.icc_profile().ok().flatten()
    } else {
        None
    };

    let img = DynamicImage::from_decoder(decoder).map_err(to_err)?;
    if h.frames > 1 {
        notices.push(if format == Format::Tiff {
            "tiff.multi_page"
        } else {
            "anim.first_frame_only"
        });
    }
    let bits = u32::from(colour.bits_per_pixel()) / u32::from(colour.channel_count()).max(1);
    if bits > 8 {
        notices.push("depth.reduced_to_8");
    }
    if h.channels == 4 && format == Format::Jpeg
        || h.channels == 4 && format == Format::Tiff && !colour.has_alpha()
    {
        notices.push("cmyk.naive_conversion");
    }
    if colour.has_alpha() && has_translucent_pixels(&img) {
        notices.push("alpha.dropped");
    }

    let rgb = img.into_rgb8();
    let mut img = DynamicImage::ImageRgb8(rgb);
    if let Some(o) = Orientation::from_exif(h.orientation).filter(|_| apply_orientation) {
        img.apply_orientation(o);
    }
    let rgb = img.into_rgb8();
    let (width, height) = rgb.dimensions();
    let raster = Raster::from_raw(width, height, rgb.into_raw())
        .ok_or_else(|| CodecError::corrupt("pixel buffer size mismatch"))?;

    let icc = extract_icc(bytes, &h, png_icc, &mut notices);

    Ok(Decoded {
        raster,
        format,
        exif_orientation: h.orientation,
        icc,
        source_bit_depth: bits.min(255) as u8,
        frames: h.frames,
        notices,
    })
}

/// Applies the EXIF orientation (1..=8; anything else is a no-op) to an RGB8 raster.
pub(crate) fn orient(raster: Raster, orientation: u8) -> Result<Raster, CodecError> {
    let Some(o) = Orientation::from_exif(orientation).filter(|o| *o != Orientation::NoTransforms)
    else {
        return Ok(raster);
    };
    let (w, h) = (raster.width, raster.height);
    let img = image::RgbImage::from_raw(w, h, raster.data)
        .ok_or_else(|| CodecError::corrupt("pixel buffer size mismatch"))?;
    let mut img = DynamicImage::ImageRgb8(img);
    img.apply_orientation(o);
    let rgb = img.into_rgb8();
    let (width, height) = rgb.dimensions();
    Raster::from_raw(width, height, rgb.into_raw())
        .ok_or_else(|| CodecError::corrupt("pixel buffer size mismatch"))
}

/// The embedded ICC profile, byte-exact (`png_icc` is the PNG decoder's expansion of `iCCP`).
pub(crate) fn extract_icc(
    bytes: &[u8],
    h: &Header,
    png_icc: Option<Vec<u8>>,
    notices: &mut Vec<&'static str>,
) -> Option<Vec<u8>> {
    match &h.icc {
        IccLoc::None => None,
        IccLoc::PngCompressed => png_icc,
        IccLoc::Range(r) => Some(bytes[r.clone()].to_vec()),
        IccLoc::JpegChunks(chunks) => {
            let icc = assemble_jpeg_icc(bytes, chunks);
            if icc.is_none() {
                notices.push("icc.invalid");
            }
            icc
        }
    }
}

fn has_translucent_pixels(img: &DynamicImage) -> bool {
    match img {
        DynamicImage::ImageRgba8(i) => i.pixels().any(|p| p.0[3] != 255),
        DynamicImage::ImageLumaA8(i) => i.pixels().any(|p| p.0[1] != 255),
        DynamicImage::ImageRgba16(i) => i.pixels().any(|p| p.0[3] != u16::MAX),
        DynamicImage::ImageLumaA16(i) => i.pixels().any(|p| p.0[1] != u16::MAX),
        DynamicImage::ImageRgba32F(i) => i.pixels().any(|p| p.0[3] < 1.0),
        _ => false,
    }
}

/// Joins the APP2 `ICC_PROFILE` segments in sequence order; `None` unless sequence numbers run
/// 1..=total without gaps and every segment agrees on the total.
fn assemble_jpeg_icc(bytes: &[u8], chunks: &[(u8, u8, std::ops::Range<usize>)]) -> Option<Vec<u8>> {
    let total = usize::from(chunks.first()?.1);
    if total == 0 || chunks.len() != total {
        return None;
    }
    let mut out = Vec::with_capacity(chunks.iter().map(|c| c.2.len()).sum());
    for (i, (seq, t, r)) in chunks.iter().enumerate() {
        if usize::from(*seq) != i + 1 || usize::from(*t) != total {
            return None;
        }
        out.extend_from_slice(&bytes[r.clone()]);
    }
    Some(out)
}

/// Maps an `image` failure to a typed codec error. `pixels` is the declared size, for the limit
/// variants.
fn map_image_err(e: image::ImageError, pixels: u64, limits: &DecodeLimits) -> CodecError {
    use image::ImageError as E;
    use image::error::{LimitErrorKind, UnsupportedErrorKind};
    match e {
        E::Limits(l) => match l.kind() {
            LimitErrorKind::DimensionError => CodecError::TooLarge(pixels),
            _ => limit_err(
                Limit::EstBytes,
                est_bytes_for_pixels(pixels),
                limits.max_est_bytes,
            ),
        },
        E::Unsupported(u) => match u.kind() {
            UnsupportedErrorKind::Format(_) => CodecError::Unsupported,
            _ => CodecError::UnsupportedFeature(u.to_string()),
        },
        other => CodecError::Corrupt(other.to_string()),
    }
}
