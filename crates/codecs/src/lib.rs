// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Image decoders and encoders. JPEG and PNG for now, through the pure-Rust `image` crate; the
//! libjpeg-turbo, HEIC and wider format work of M1, M6 and M11 replaces or extends this.
//!
//! Limits of this stand-in (all tracked in ROADMAP): only the colour data and the ICC profile
//! survive a re-encode, so EXIF (other than the orientation, which is applied once at decode) and
//! other metadata are dropped; JPEG output is always a full re-encode.

use auto_crop_imgproc::Raster;
use image::{DynamicImage, ImageDecoder, ImageEncoder, ImageReader, Limits};
use std::io::Cursor;

/// Largest image accepted (PLAN C1: 100 MP by default).
pub const MAX_PIXELS: u64 = 100_000_000;
/// Decoder allocation cap, a little above a 100 MP RGB8 image.
const MAX_ALLOC: u64 = 1 << 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Jpeg,
    Png,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Format::Jpeg => "jpg",
            Format::Png => "png",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CodecError {
    #[error("unsupported format")]
    Unsupported,
    #[error("corrupt or unreadable image: {0}")]
    Corrupt(String),
    #[error("image too large: {0} pixels")]
    TooLarge(u64),
    #[error("encode failed: {0}")]
    Encode(String),
}

/// Formats this build can decode.
pub fn supported_input_formats() -> &'static [&'static str] {
    &["jpeg", "png"]
}

/// Identifies the format from the magic bytes (never from the file name).
pub fn sniff(bytes: &[u8]) -> Option<Format> {
    if bytes.len() >= 3 && bytes[..3] == [0xFF, 0xD8, 0xFF] {
        Some(Format::Jpeg)
    } else if bytes.len() >= 8 && bytes[..8] == [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A] {
        Some(Format::Png)
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Probe {
    pub format: Format,
    pub width: u32,
    pub height: u32,
}

/// Reads only the header: format and stored dimensions (before any EXIF turn).
pub fn probe(bytes: &[u8]) -> Result<Probe, CodecError> {
    let format = sniff(bytes).ok_or(CodecError::Unsupported)?;
    if format == Format::Png {
        // The IHDR chunk is always first: 8 signature bytes, then length, "IHDR", width, height.
        if bytes.len() < 24 || &bytes[12..16] != b"IHDR" {
            return Err(CodecError::Corrupt("missing PNG header".into()));
        }
        let be =
            |i: usize| u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
        return Ok(Probe {
            format,
            width: be(16),
            height: be(20),
        });
    }
    let (width, height) = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| CodecError::Corrupt(e.to_string()))?
        .into_dimensions()
        .map_err(|e| CodecError::Corrupt(e.to_string()))?;
    Ok(Probe {
        format,
        width,
        height,
    })
}

#[derive(Debug, Clone)]
pub struct Decoded {
    /// Pixels with the EXIF orientation already applied.
    pub raster: Raster,
    pub format: Format,
    /// The EXIF orientation found (1 = none), already applied to `raster`.
    pub exif_orientation: u8,
    pub icc: Option<Vec<u8>>,
}

fn orientation_number(o: image::metadata::Orientation) -> u8 {
    use image::metadata::Orientation::*;
    match o {
        NoTransforms => 1,
        FlipHorizontal => 2,
        Rotate180 => 3,
        FlipVertical => 4,
        Rotate90FlipH => 5,
        Rotate90 => 6,
        Rotate270FlipH => 7,
        Rotate270 => 8,
    }
}

/// Decodes to RGB8 with the EXIF orientation applied once. Refuses images above [`MAX_PIXELS`].
pub fn decode(bytes: &[u8]) -> Result<Decoded, CodecError> {
    let p = probe(bytes)?;
    let pixels = u64::from(p.width) * u64::from(p.height);
    if pixels > MAX_PIXELS {
        return Err(CodecError::TooLarge(pixels));
    }
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| CodecError::Corrupt(e.to_string()))?;
    let mut limits = Limits::default();
    limits.max_alloc = Some(MAX_ALLOC);
    reader.limits(limits);
    let mut decoder = reader
        .into_decoder()
        .map_err(|e| CodecError::Corrupt(e.to_string()))?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let icc = decoder.icc_profile().ok().flatten();
    let mut img =
        DynamicImage::from_decoder(decoder).map_err(|e| CodecError::Corrupt(e.to_string()))?;
    img.apply_orientation(orientation);
    let rgb = img.into_rgb8();
    let (width, height) = rgb.dimensions();
    let raster = Raster::from_raw(width, height, rgb.into_raw())
        .ok_or_else(|| CodecError::Corrupt("pixel buffer size mismatch".into()))?;
    Ok(Decoded {
        raster,
        format: p.format,
        exif_orientation: orientation_number(orientation),
        icc,
    })
}

/// Encodes RGB8. `quality` (1..=100) applies to JPEG only; PNG is lossless.
pub fn encode(
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
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Raster {
        let mut r = Raster::new(37, 23);
        for y in 0..23 {
            for x in 0..37 {
                r.set_pixel(x, y, [(x * 6) as u8, (y * 10) as u8, 120]);
            }
        }
        r
    }

    #[test]
    fn png_round_trips_exactly() {
        let r = sample();
        let bytes = encode(&r, Format::Png, 90, None).unwrap();
        assert_eq!(sniff(&bytes), Some(Format::Png));
        let d = decode(&bytes).unwrap();
        assert_eq!(d.raster, r);
        assert_eq!(d.exif_orientation, 1);
    }

    #[test]
    fn jpeg_round_trips_closely_and_probes_without_decoding() {
        let r = sample();
        let bytes = encode(&r, Format::Jpeg, 95, None).unwrap();
        assert_eq!(sniff(&bytes), Some(Format::Jpeg));
        assert_eq!(
            probe(&bytes).unwrap(),
            Probe {
                format: Format::Jpeg,
                width: 37,
                height: 23
            }
        );
        let d = decode(&bytes).unwrap();
        assert_eq!((d.raster.width, d.raster.height), (37, 23));
        let worst = d
            .raster
            .data
            .iter()
            .zip(&r.data)
            .map(|(a, b)| (i32::from(*a) - i32::from(*b)).abs())
            .max()
            .unwrap();
        assert!(worst <= 12, "worst error {worst}");
    }

    #[test]
    fn garbage_and_truncation_are_errors_not_panics() {
        assert!(matches!(
            decode(b"not an image at all"),
            Err(CodecError::Unsupported)
        ));
        let bytes = encode(&sample(), Format::Png, 90, None).unwrap();
        assert!(decode(&bytes[..bytes.len() / 2]).is_err());
        assert!(decode(&[]).is_err());
    }

    #[test]
    fn a_huge_declared_size_is_refused_before_allocating() {
        fn crc32(data: &[u8]) -> u32 {
            let mut c = 0xFFFF_FFFFu32;
            for b in data {
                c ^= u32::from(*b);
                for _ in 0..8 {
                    c = if c & 1 == 1 {
                        0xEDB8_8320 ^ (c >> 1)
                    } else {
                        c >> 1
                    };
                }
            }
            !c
        }
        // A PNG header declaring 20000 x 20000 (400 MP) with no pixel data.
        let mut ihdr = b"IHDR".to_vec();
        ihdr.extend_from_slice(&20000u32.to_be_bytes());
        ihdr.extend_from_slice(&20000u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
        png.extend_from_slice(&ihdr);
        png.extend_from_slice(&crc32(&ihdr).to_be_bytes());
        match decode(&png) {
            Err(CodecError::TooLarge(n)) => assert_eq!(n, 400_000_000),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn supported_formats_are_listed() {
        assert_eq!(supported_input_formats(), &["jpeg", "png"]);
    }
}
