// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! How rendered pixels become a file (ROADMAP M2.84, M2.22, M2.23, M2.25, M2.26, M2.80; PLAN 3).
//! The stand-in `image`-crate encoders are gone from the save path: JPEG goes through
//! [`auto_crop_codecs::encoders::default_jpeg_encoder`] (libjpeg-turbo when the build has it,
//! `jpeg-encoder` otherwise) and PNG through [`auto_crop_codecs::encoders::PngEncoder`].
//!
//! * **Quality** ([`QualitySetting`]): for a JPEG source, the quality its quantisation tables say
//!   (`q_est`, [`auto_crop_codecs::jpeg_meta::estimate_quality`]) shifted by the preset and clamped
//!   (Balanced `q_est + 5` in 80..=95, Small `q_est - 10` in 60..=85, Best `q_est + 10` in
//!   90..=97); q90, q80 or q95 when the source is not a JPEG or its tables are not IJG's.
//! * **Orientation once**: the decode turned the pixels, so the output carries Orientation = 1
//!   (`jpeg_meta` patches the EXIF in place; the encoders write none).
//! * **Metadata**: the source's EXIF (patched), XMP, IPTC and comments are carried into JPEG
//!   output; `strip_location` drops GPS, XMP and IPTC; `strip_metadata` drops all of it. The ICC
//!   profile is embedded byte-exact (JPEG and PNG) and the pixel density is kept.
//! * **Verification**: [`Expect`] for every output, so `commit::verify_temp_expect` can check the
//!   size, orientation, ICC and the pixels (exact for PNG, luma fingerprint for JPEG).

use crate::commit::{Expect, VerifyMode};
use crate::error::{ErrKind, Result};
use auto_crop_codecs::Format;
use auto_crop_codecs::encoders::{PngEncoder, default_jpeg_encoder};
use auto_crop_codecs::jpeg_meta::{self, JpegMeta, MetaPolicy};
use auto_crop_core::output::Quality;
use auto_crop_core::ports::{EncodeMeta, Encoder, PixelFormat, RasterView, SampleSlice};
use auto_crop_core::{CancelToken, OutputSpec};
use auto_crop_imgproc::Raster;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// The JPEG quality presets of PLAN 3.9 (M2.25).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QualityPreset {
    #[default]
    Balanced,
    Small,
    Best,
}

/// How the JPEG quality is chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum QualitySetting {
    /// From the source (`q_est`) and the preset.
    Preset { preset: QualityPreset },
    /// A fixed quality, 1..=100 (`--quality`).
    Fixed { value: u8 },
}

impl Default for QualitySetting {
    fn default() -> Self {
        QualitySetting::Preset {
            preset: QualityPreset::Balanced,
        }
    }
}

impl QualitySetting {
    /// The quality to encode at for a source whose tables estimate `q_est` (`None`: not a JPEG, or
    /// custom tables).
    pub fn jpeg_quality(self, q_est: Option<u8>) -> u8 {
        match self {
            QualitySetting::Fixed { value } => value.clamp(1, 100),
            QualitySetting::Preset { preset } => {
                let (delta, lo, hi, fallback): (i32, i32, i32, u8) = match preset {
                    QualityPreset::Balanced => (5, 80, 95, 90),
                    QualityPreset::Small => (-10, 60, 85, 80),
                    QualityPreset::Best => (10, 90, 97, 95),
                };
                match q_est {
                    Some(q) => (i32::from(q) + delta).clamp(lo, hi) as u8,
                    None => fallback,
                }
            }
        }
    }
}

/// How a TIFF is compressed (PLAN 3.3; 1-bit pages are Group 4 whatever this says).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TiffCompressionSetting {
    None,
    #[default]
    Lzw,
    Deflate,
    #[serde(rename = "packbits")]
    PackBits,
}

/// The format a save writes when the user picks one (the editor's Save format, the CLI's
/// `--format`); `Auto` keeps the source's own format when this build can write it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SaveFormat {
    #[default]
    Auto,
    Jpeg,
    Png,
    Tiff,
    Webp,
    Pdf,
}

impl SaveFormat {
    /// The format this choice names; `None` for `Auto`.
    pub fn format(self) -> Option<Format> {
        match self {
            SaveFormat::Auto => None,
            SaveFormat::Jpeg => Some(Format::Jpeg),
            SaveFormat::Png => Some(Format::Png),
            SaveFormat::Tiff => Some(Format::Tiff),
            SaveFormat::Webp => Some(Format::Webp),
            SaveFormat::Pdf => Some(Format::Pdf),
        }
    }
}

/// The format of a save that does not replace the source (a copy): the choice if there is one, else
/// the source's own format when this build can write it (PLAN 3.2.3), JPEG for HEIC, PNG for the
/// rest. A lossy WebP and a 1-bit TIFF are not written back as themselves (this build's WebP is
/// lossless only, and a 24-bit TIFF is not what a bilevel scan wants), so they give PNG.
pub fn copy_format(source: Format, notices: &[&str], choice: SaveFormat) -> Format {
    if let Some(f) = choice.format() {
        return f;
    }
    let lossy_webp = source == Format::Webp && notices.contains(&"webp.lossy");
    let bilevel_tiff = source == Format::Tiff && notices.contains(&"tiff.low_depth");
    if source.is_encodable() && source != Format::Pdf && !lossy_webp && !bilevel_tiff {
        source
    } else if source == Format::Heic {
        Format::Jpeg
    } else {
        Format::Png
    }
}

/// What the caller may choose about the written file. `Default` is the shipped behaviour: Balanced
/// quality, GPS kept, lossless JPEG when it is exact, full verification, no cloud downloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EngineOptions {
    pub quality: QualitySetting,
    /// Drop EXIF GPS, XMP and IPTC from the output (`--strip-location`).
    pub strip_location: bool,
    /// Carry no metadata at all.
    pub strip_metadata: bool,
    /// Use the lossless JPEG path when it is exact (M2.24).
    pub lossless_jpeg: bool,
    /// How hard a written temp is re-checked; there is no way to switch it off.
    #[serde(skip)]
    pub verify: VerifyMode,
    /// Download cloud placeholders (`--hydrate`); otherwise they are skipped as `CloudNotLocal`.
    pub hydrate_cloud_files: bool,
    /// The pixel cap of every decode (`--max-pixels`); the codecs clamp it to their ceiling.
    pub max_pixels: u64,
    /// Which backup methods may be tried (reflink, hardlink, copy).
    pub backup_method: crate::store::MethodPref,
    /// The compression of a TIFF this save writes (1-bit pages are always Group 4).
    pub tiff_compression: TiffCompressionSetting,
    /// Write an enhanced grey page as 1 bit: CCITT Group 4 in a TIFF or a PDF, thresholded at
    /// 128 with no dithering. Any other output is `UnsupportedOutput`, never a silent 8-bit file.
    pub one_bit: bool,
}

impl Default for EngineOptions {
    fn default() -> Self {
        Self {
            quality: QualitySetting::default(),
            strip_location: false,
            strip_metadata: false,
            lossless_jpeg: true,
            verify: VerifyMode::Full,
            hydrate_cloud_files: false,
            max_pixels: auto_crop_codecs::DEFAULT_MAX_PIXELS,
            backup_method: crate::store::MethodPref::Auto,
            tiff_compression: TiffCompressionSetting::default(),
            one_bit: false,
        }
    }
}

impl EngineOptions {
    /// The decode limits these options give.
    pub fn limits(&self) -> auto_crop_codecs::DecodeLimits {
        auto_crop_codecs::DecodeLimits::default().with_max_pixels(self.max_pixels)
    }
}

/// What the source file said about itself, read once per save.
#[derive(Debug, Clone, Default)]
pub struct SourceMeta {
    /// The ICC profile, byte-exact.
    pub icc: Option<Arc<Vec<u8>>>,
    /// JPEG only: EXIF, XMP, IPTC, comments, density, quality.
    pub jpeg: Option<JpegMeta>,
    /// The density of a PNG or TIFF source (a JPEG's is in `jpeg`).
    pub dpi: Option<(u32, u32)>,
    /// TIFF only: the descriptive tags and whether directories we cannot carry exist.
    pub tiff: Option<auto_crop_codecs::TiffMeta>,
}

impl SourceMeta {
    pub fn read(format: Format, bytes: &[u8], icc: Option<Arc<Vec<u8>>>) -> Self {
        let jpeg = (format == Format::Jpeg)
            .then(|| jpeg_meta::read_meta(bytes).ok())
            .flatten();
        let tiff = (format == Format::Tiff)
            .then(|| auto_crop_codecs::extract_tiff_meta(bytes))
            .flatten();
        let dpi = match format {
            Format::Png => auto_crop_codecs::extract_dpi(bytes, format),
            _ => tiff.as_ref().and_then(|t| t.dpi),
        };
        Self {
            icc,
            jpeg,
            dpi,
            tiff,
        }
    }

    /// The pixel density to write: a JPEG's JFIF density, else the PNG or TIFF source's.
    pub fn dpi(&self) -> Option<(u32, u32)> {
        self.jpeg.as_ref().and_then(|m| m.dpi).or(self.dpi)
    }

    /// [`SourceMeta::read`], plus the EXIF of a PNG, WebP, HEIC or AVIF source (a JPEG's comes from
    /// `read`): a conversion carries it into the new file with the Orientation reset (the decode
    /// turned the pixels) and the dimensions rewritten, like the metadata of a JPEG source.
    pub fn read_for_convert(
        format: Format,
        bytes: &[u8],
        icc: Option<Arc<Vec<u8>>>,
        limits: &auto_crop_codecs::DecodeLimits,
    ) -> Self {
        let mut m = Self::read(format, bytes, icc);
        if m.jpeg.is_none()
            && let Some(exif) = auto_crop_codecs::extract_exif(bytes, format, limits)
        {
            m.jpeg = Some(JpegMeta {
                exif: Some(exif),
                ..JpegMeta::default()
            });
        }
        m
    }

    /// After a colour conversion: the profile the output embeds is `icc`, and when `to_srgb` the
    /// EXIF `ColorSpace` tag, if there is one, says sRGB.
    pub fn with_colour(mut self, icc: Option<Vec<u8>>, to_srgb: bool) -> Self {
        self.icc = icc.map(Arc::new);
        if to_srgb
            && let Some(jm) = self.jpeg.as_mut()
            && let Some(e) = jm.exif.take()
        {
            jm.exif = Some(jpeg_meta::exif_set_srgb(&e));
        }
        self
    }

    /// `q_est` when the source's tables are IJG's.
    pub fn q_est(&self) -> Option<u8> {
        self.jpeg
            .as_ref()
            .and_then(|m| m.quality)
            .filter(|q| q.ijg)
            .map(|q| q.q)
    }
}

/// A finished output: the bytes and what they must decode to.
pub struct Encoded {
    pub bytes: Vec<u8>,
    pub expect: Expect,
}

pub(crate) fn view(r: &Raster) -> Result<RasterView<'_>> {
    RasterView::new(
        r.width,
        r.height,
        PixelFormat::Rgb8,
        r.width as usize * 3,
        SampleSlice::U8(&r.data),
    )
    .ok_or(ErrKind::Internal)
}

pub(crate) fn meta_policy(o: &EngineOptions, dims: (u32, u32)) -> MetaPolicy {
    MetaPolicy {
        dims,
        strip_location: o.strip_location,
        strip_all: o.strip_metadata,
    }
}

/// Encodes `raster` as `format` (JPEG or PNG) with the metadata policy applied.
pub fn encode_raster(
    raster: &Raster,
    format: Format,
    meta: &SourceMeta,
    o: &EngineOptions,
) -> Result<Encoded> {
    // 1-bit output exists for TIFF and PDF only, and only for a grey page (`formats_out`).
    if o.one_bit && !matches!(format, Format::Tiff | Format::Pdf) {
        return Err(ErrKind::UnsupportedOutput);
    }
    let icc: Option<Arc<[u8]>> = meta
        .icc
        .as_ref()
        .map(|v| Arc::<[u8]>::from(v.as_slice()))
        .filter(|p| !p.is_empty());
    let dpi = meta.dpi();
    let encode_meta = EncodeMeta {
        icc: icc.clone(),
        dpi,
    };
    let cancel = CancelToken::never();
    let src = view(raster)?;
    let dims = (raster.width, raster.height);
    let bytes = match format {
        Format::Jpeg => {
            let spec = OutputSpec {
                quality: Quality::Fixed {
                    value: o.quality.jpeg_quality(meta.q_est()),
                },
                ..OutputSpec::default()
            };
            let enc = default_jpeg_encoder();
            let raw = enc
                .encode(&src, &spec, &encode_meta, &cancel)
                .map_err(|_| ErrKind::EncodeFailed)?;
            match &meta.jpeg {
                Some(jm) => jpeg_meta::rewrite_metadata(&raw, jm, &meta_policy(o, dims))
                    .map_err(|_| ErrKind::EncodeFailed)?,
                None => raw,
            }
        }
        Format::Png => {
            let raw = PngEncoder
                .encode(&src, &OutputSpec::default(), &encode_meta, &cancel)
                .map_err(|_| ErrKind::EncodeFailed)?;
            png_with_exif(raw, meta, o, dims)
        }
        // TIFF, lossless WebP and PDF (M11): their own module decides the metadata and the check.
        Format::Tiff | Format::Webp | Format::Pdf => {
            return crate::formats_out::encode_rgb(raster, format, meta, o);
        }
        // The writers that do not exist yet (AVIF, JPEG XL): never reached for a replace (the
        // replaceability gate stops these formats) and refused for a copy.
        _ => return Err(ErrKind::UnsupportedOutput),
    };
    Ok(Encoded {
        bytes,
        expect: Expect::for_raster(raster, format, icc.as_deref()),
    })
}

/// The EXIF of the source (orientation reset, size rewritten) goes into a PNG as an `eXIf` chunk;
/// XMP and IPTC have no place in a PNG this writer makes.
fn png_with_exif(raw: Vec<u8>, meta: &SourceMeta, o: &EngineOptions, dims: (u32, u32)) -> Vec<u8> {
    match patched_exif(meta, o, dims) {
        Some(e) => png_insert_exif(&raw, &e),
        None => raw,
    }
}

/// The source's EXIF with the Orientation reset, the size rewritten and the location dropped when
/// the policy says so; `None` when there is none or the policy strips everything.
pub(crate) fn patched_exif(
    meta: &SourceMeta,
    o: &EngineOptions,
    dims: (u32, u32),
) -> Option<Vec<u8>> {
    meta.jpeg
        .as_ref()
        .filter(|_| !o.strip_metadata)
        .and_then(|jm| jm.exif.as_deref())
        .and_then(|e| {
            jpeg_meta::exif_patch(
                e,
                &jpeg_meta::ExifPatch {
                    dims,
                    strip_location: o.strip_location,
                },
            )
        })
}

/// CRC-32 (IEEE) as PNG chunks use it. Bitwise: only ever run over an EXIF blob.
fn crc32(parts: &[&[u8]]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for part in parts {
        for &b in *part {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
    }
    !crc
}

/// Inserts an `eXIf` chunk holding `exif` right after the `IHDR` chunk of `png`. A blob that does
/// not fit a chunk, or a stream that does not start with a PNG header, is returned unchanged.
pub(crate) fn png_insert_exif(png: &[u8], exif: &[u8]) -> Vec<u8> {
    const AFTER_IHDR: usize = 8 + 4 + 4 + 13 + 4;
    if png.len() < AFTER_IHDR
        || !png.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A])
        || &png[12..16] != b"IHDR"
        || exif.is_empty()
        || exif.len() > 0x00FF_FFFF
    {
        return png.to_vec();
    }
    let mut out = Vec::with_capacity(png.len() + exif.len() + 12);
    out.extend_from_slice(&png[..AFTER_IHDR]);
    out.extend_from_slice(&(exif.len() as u32).to_be_bytes());
    out.extend_from_slice(b"eXIf");
    out.extend_from_slice(exif);
    out.extend_from_slice(&crc32(&[b"eXIf", exif]).to_be_bytes());
    out.extend_from_slice(&png[AFTER_IHDR..]);
    out
}

/// Notice codes of an enhanced save (the UI looks the text up by them).
pub const NOTICE_ICC_DROPPED: &str = "enhance.icc_dropped";
pub const NOTICE_PNG_RECOMMENDED: &str = "enhance.png_recommended";

/// A grey JPEG is never written below this quality (PLAN 5.9: low quality rings around text).
pub const GRAY_JPEG_MIN_QUALITY: u8 = 90;

/// An enhanced page encoded, with the notices it earns.
pub struct EncodedPage {
    pub encoded: Encoded,
    pub notices: Vec<&'static str>,
}

/// Encodes the result of the enhancer as `format` (JPEG or PNG).
///
/// * **Colour** (Clean): exactly as [`encode_raster`]; the pixels are in the source's colour space
///   (the gain is applied there), so its ICC profile is kept byte-exact.
/// * **Grey** (Document, Receipt): ONE channel, 8 bits. A grey file carries no RGB profile, so the
///   source's is dropped (`enhance.icc_dropped`; an sRGB or grey reader shows it correctly with no
///   profile), EXIF, XMP and the density follow the same policy as any save, and a JPEG is written
///   at no less than [`GRAY_JPEG_MIN_QUALITY`] (PNG is lossless and keeps faint marks: a JPEG save
///   says so with `enhance.png_recommended`).
pub fn encode_enhanced(
    page: &auto_crop_imgproc::enhance::Enhanced,
    format: Format,
    meta: &SourceMeta,
    o: &EngineOptions,
) -> Result<EncodedPage> {
    use auto_crop_imgproc::enhance::Enhanced;
    if o.one_bit && !matches!(format, Format::Tiff | Format::Pdf) {
        return Err(ErrKind::UnsupportedOutput);
    }
    match page {
        Enhanced::Rgb(r) => Ok(EncodedPage {
            encoded: encode_raster(r, format, meta, o)?,
            notices: Vec::new(),
        }),
        Enhanced::Gray(g) => {
            let mut notices = Vec::new();
            if meta.icc.as_ref().is_some_and(|p| !p.is_empty()) {
                notices.push(NOTICE_ICC_DROPPED);
            }
            let dpi = meta.dpi();
            let encode_meta = EncodeMeta { icc: None, dpi };
            let cancel = CancelToken::never();
            let view = RasterView::new(
                g.width,
                g.height,
                PixelFormat::Gray8,
                g.width as usize,
                SampleSlice::U8(&g.data),
            )
            .ok_or(ErrKind::Internal)?;
            let dims = (g.width, g.height);
            let bytes = match format {
                Format::Jpeg => {
                    notices.push(NOTICE_PNG_RECOMMENDED);
                    let spec = OutputSpec {
                        quality: Quality::Fixed {
                            value: o
                                .quality
                                .jpeg_quality(meta.q_est())
                                .max(GRAY_JPEG_MIN_QUALITY),
                        },
                        ..OutputSpec::default()
                    };
                    let raw = default_jpeg_encoder()
                        .encode(&view, &spec, &encode_meta, &cancel)
                        .map_err(|_| ErrKind::EncodeFailed)?;
                    match &meta.jpeg {
                        Some(jm) => jpeg_meta::rewrite_metadata(&raw, jm, &meta_policy(o, dims))
                            .map_err(|_| ErrKind::EncodeFailed)?,
                        None => raw,
                    }
                }
                Format::Png => {
                    let raw = PngEncoder
                        .encode(&view, &OutputSpec::default(), &encode_meta, &cancel)
                        .map_err(|_| ErrKind::EncodeFailed)?;
                    png_with_exif(raw, meta, o, dims)
                }
                Format::Tiff | Format::Webp | Format::Pdf => {
                    return Ok(EncodedPage {
                        encoded: crate::formats_out::encode_gray(g, format, meta, o)?,
                        notices,
                    });
                }
                _ => return Err(ErrKind::UnsupportedOutput),
            };
            Ok(EncodedPage {
                encoded: Encoded {
                    bytes,
                    expect: Expect::for_gray(g, format),
                },
                notices,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_exif_chunk_is_inserted_with_a_valid_crc_and_the_png_still_decodes() {
        let r = gradient(20, 12);
        let blob = auto_crop_codecs::fixtures::exif_blob(6, true);
        let raw = auto_crop_codecs::encode(&r, Format::Png, 0, None).unwrap();
        let with = png_insert_exif(&raw, &blob);
        assert_eq!(with.len(), raw.len() + blob.len() + 12);
        let d = auto_crop_codecs::decode(&with).unwrap();
        // The orientation in the chunk is applied by the decoder: 6 turns the 20 x 12 picture.
        assert_eq!((d.raster.width, d.raster.height), (12, 20));
        assert_eq!(d.exif_orientation, 6);
        // The `png` crate checks chunk CRCs while decoding, so a bad CRC would have failed above;
        // check the arithmetic against a known value too ("123456789" has CRC-32 0xCBF43926).
        assert_eq!(crc32(&[b"123456789"]), 0xCBF4_3926);
        assert_eq!(png_insert_exif(b"not a png", &blob), b"not a png");
    }

    #[test]
    fn the_presets_clamp_around_the_estimate() {
        let b = QualitySetting::Preset {
            preset: QualityPreset::Balanced,
        };
        assert_eq!(b.jpeg_quality(Some(92)), 95); // 97 clamped to 95
        assert_eq!(b.jpeg_quality(Some(85)), 90);
        assert_eq!(b.jpeg_quality(Some(60)), 80); // 65 clamped up to 80
        assert_eq!(b.jpeg_quality(None), 90);
        let s = QualitySetting::Preset {
            preset: QualityPreset::Small,
        };
        assert_eq!(s.jpeg_quality(Some(92)), 82);
        assert_eq!(s.jpeg_quality(Some(100)), 85); // 90 clamped to 85
        assert_eq!(s.jpeg_quality(Some(40)), 60); // 30 clamped up to 60
        assert_eq!(s.jpeg_quality(None), 80);
        let best = QualitySetting::Preset {
            preset: QualityPreset::Best,
        };
        assert_eq!(best.jpeg_quality(Some(92)), 97);
        assert_eq!(best.jpeg_quality(Some(70)), 90);
        assert_eq!(best.jpeg_quality(None), 95);
        assert_eq!(
            QualitySetting::Fixed { value: 77 }.jpeg_quality(Some(92)),
            77
        );
        assert_eq!(QualitySetting::Fixed { value: 0 }.jpeg_quality(None), 1);
    }

    #[test]
    fn options_default_to_the_shipped_behaviour_and_round_trip() {
        let o = EngineOptions::default();
        assert!(
            o.lossless_jpeg && !o.strip_location && !o.strip_metadata && !o.hydrate_cloud_files
        );
        assert_eq!(o.verify, VerifyMode::Full);
        let json = serde_json::to_string(&o).unwrap();
        assert_eq!(serde_json::from_str::<EngineOptions>(&json).unwrap(), o);
        // Missing keys take the defaults.
        let p: EngineOptions = serde_json::from_str(r#"{"stripLocation":true}"#).unwrap();
        assert!(p.strip_location && p.lossless_jpeg);
    }

    fn gradient(w: u32, h: u32) -> Raster {
        let mut r = Raster::new(w, h);
        for y in 0..h {
            for x in 0..w {
                r.set_pixel(x, y, [(x * 255 / w) as u8, (y * 255 / h) as u8, 90]);
            }
        }
        r
    }

    #[test]
    fn jpeg_and_png_encode_through_the_codecs_encoders_and_verify() {
        let r = gradient(96, 64);
        for format in [Format::Jpeg, Format::Png] {
            let e = encode_raster(
                &r,
                format,
                &SourceMeta::default(),
                &EngineOptions::default(),
            )
            .unwrap();
            let d = auto_crop_codecs::decode(&e.bytes).unwrap();
            assert_eq!(d.format, format);
            assert_eq!((d.raster.width, d.raster.height), (96, 64));
            assert_eq!(d.exif_orientation, 1);
            crate::commit::verify_bytes(&e.bytes, &e.expect, VerifyMode::Full).unwrap();
            crate::commit::verify_bytes(&e.bytes, &e.expect, VerifyMode::Fast).unwrap();
        }
    }

    #[test]
    fn the_icc_profile_comes_back_byte_exact_in_both_formats() {
        let r = gradient(48, 40);
        let icc = Arc::new(auto_crop_codecs::fixtures::fake_icc(3000));
        let meta = SourceMeta {
            icc: Some(icc.clone()),
            ..SourceMeta::default()
        };
        for format in [Format::Jpeg, Format::Png] {
            let e = encode_raster(&r, format, &meta, &EngineOptions::default()).unwrap();
            let d = auto_crop_codecs::decode(&e.bytes).unwrap();
            assert_eq!(d.icc.as_deref(), Some(icc.as_slice()), "{format:?}");
            crate::commit::verify_bytes(&e.bytes, &e.expect, VerifyMode::Full).unwrap();
        }
    }

    #[test]
    fn a_jpeg_source_sets_the_quality_and_its_metadata_follows_the_policy() {
        // A source saved at q85 with EXIF orientation 6, a thumbnail and GPS.
        let r = gradient(64, 48);
        let src = auto_crop_codecs::encode(&r, Format::Jpeg, 85, None).unwrap();
        let mut exif = b"Exif\0\0".to_vec();
        exif.extend_from_slice(&auto_crop_codecs::fixtures::exif_blob(6, true));
        let src = auto_crop_codecs::fixtures::jpeg_insert_segment(&src, 0xE1, &exif);
        let meta = SourceMeta::read(Format::Jpeg, &src, None);
        assert_eq!(meta.q_est().map(|q| q / 5), Some(17)); // about 85
        let out = encode_raster(&r, Format::Jpeg, &meta, &EngineOptions::default()).unwrap();
        let jm = jpeg_meta::read_meta(&out.bytes).unwrap();
        // The EXIF is carried and says "upright": the pixels were turned by the decode, once.
        assert_eq!(
            auto_crop_codecs::exif_orientation(&jm.exif.expect("exif carried")),
            Some(1)
        );
        // The quality followed the source: 85 + 5.
        let q = jm.quality.unwrap();
        assert!(q.ijg && (i32::from(q.q) - 90).abs() <= 3, "{q:?}");
        // Stripping everything leaves no EXIF.
        let o = EngineOptions {
            strip_metadata: true,
            ..EngineOptions::default()
        };
        let bare = encode_raster(&r, Format::Jpeg, &meta, &o).unwrap();
        assert!(jpeg_meta::read_meta(&bare.bytes).unwrap().exif.is_none());
    }

    #[test]
    fn formats_with_no_writer_are_refused() {
        let r = gradient(16, 16);
        for f in [Format::Avif, Format::Heic, Format::Jxl] {
            assert_eq!(
                encode_raster(&r, f, &SourceMeta::default(), &EngineOptions::default()).err(),
                Some(ErrKind::UnsupportedOutput)
            );
        }
    }
}
