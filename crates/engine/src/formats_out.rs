// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! TIFF, lossless WebP and PDF output of a save (ROADMAP M11.21 lossless half, M11.29, M11.32 to
//! M11.35; PLAN 3.3, 3.6). The encoders live in `auto_crop_codecs::encoders`; this module decides
//! what each one is given and what the commit protocol must find when it reads the file back.
//!
//! * **TIFF**: LZW (or the chosen compression), the ICC profile byte-exact, the pixel density, the
//!   descriptive tags of a TIFF source (ImageDescription, Make, Model, DateTime, Artist, Copyright,
//!   XMP, IPTC) under the metadata policy, Orientation 1. No EXIF or GPS directory is written: a
//!   TIFF source that has one is never replaced (`tiff.exif_not_carried`, `fsplan`). A colour
//!   raster whose three channels are equal and that carries no profile is written as one grey
//!   channel (the decoder gives the same RGB back, so the check stays exact).
//! * **WebP**: lossless VP8L; ICC and the patched EXIF go in their chunks. There is no lossy WebP in
//!   this build, so the format is "WebP (lossless)".
//! * **PDF**: one page per output. A colour picture is embedded as a JPEG at the save's quality
//!   (DCTDecode, EXIF and the rest dropped), a grey enhanced page as lossless Flate. With
//!   [`EngineOptions::one_bit`] an enhanced grey page becomes a CCITT Group 4 page (TIFF Group 4 for
//!   a TIFF). The page size follows the density, 300 dpi when it is unknown.
//! * **AVIF** (feature `avif`, lossy): rav1e, 8-bit, 4:2:0 (or 4:4:4 by option); a grey page, or a
//!   colour raster whose channels are equal and that has no profile, is one plane. ICC byte-exact in
//!   a `prof` box, the patched EXIF as an `Exif` item, Orientation 1, no rotation box.
//! * **Checks** ([`Expect`]): lossless pages must decode to the exact pixels (blake3); a JPEG page
//!   to the same luma fingerprint as any JPEG output.

use crate::commit::{Content, Expect, gray_pixel_hash, luma_fingerprint};
use crate::error::{ErrKind, Result};
use crate::output::{
    Encoded, EngineOptions, SourceMeta, TiffCompressionSetting, patched_exif, view,
};
use auto_crop_codecs::Format;
use auto_crop_codecs::RawTag;
use auto_crop_codecs::encoders::bilevel::{BilevelImage, DEFAULT_CUT};
use auto_crop_codecs::encoders::pdf::{PageImage, PdfWriter};
use auto_crop_codecs::encoders::{TiffCompression, TiffEncoder, WebpEncoder, default_jpeg_encoder};
use auto_crop_core::output::{Bilevel, Bits, Quality};
use auto_crop_core::ports::{EncodeMeta, Encoder, PixelFormat, RasterView, SampleSlice};
use auto_crop_core::{CancelToken, OutputSpec};
use auto_crop_imgproc::Raster;
use auto_crop_imgproc::enhance::GrayImage;
use std::sync::Arc;

impl From<TiffCompressionSetting> for TiffCompression {
    fn from(c: TiffCompressionSetting) -> Self {
        match c {
            TiffCompressionSetting::None => TiffCompression::None,
            TiffCompressionSetting::Lzw => TiffCompression::Lzw,
            TiffCompressionSetting::Deflate => TiffCompression::Deflate,
            TiffCompressionSetting::PackBits => TiffCompression::PackBits,
        }
    }
}

/// Tags that can say where a picture was made or who made it, dropped by "strip location".
const LOCATIONISH_TAGS: [u16; 4] = [269, 270, 315, 316];

/// The descriptive tags of a TIFF source that the output carries (the metadata policy of M2.23):
/// none under "strip all"; under "strip location" no XMP, no IPTC and no free-text tags; the XMP
/// that stays has its orientation reset.
fn carried_tags(meta: &SourceMeta, o: &EngineOptions) -> Vec<RawTag> {
    if o.strip_metadata {
        return Vec::new();
    }
    let Some(t) = &meta.tiff else {
        return Vec::new();
    };
    t.le_tags()
        .into_iter()
        .filter(|x| {
            !o.strip_location
                || !(x.tag == auto_crop_codecs::TAG_XMP
                    || x.tag == auto_crop_codecs::TAG_IPTC
                    || LOCATIONISH_TAGS.contains(&x.tag))
        })
        .map(|mut x| {
            if x.tag == auto_crop_codecs::TAG_XMP {
                x.data = auto_crop_codecs::jpeg_meta::xmp_reset_orientation(&x.data);
            }
            x
        })
        .collect()
}

/// The compression of the TIFF this save writes: the chosen one, else the source TIFF's own class
/// (PLAN 3.2.3: "same compression class"), else LZW.
fn tiff_compression(meta: &SourceMeta, o: &EngineOptions) -> TiffCompression {
    if let Some(c) = o.tiff_compression {
        return c.into();
    }
    match meta.tiff.as_ref().map(|t| t.compression) {
        Some(1) => TiffCompression::None,
        Some(8 | 32946) => TiffCompression::Deflate,
        Some(32773) => TiffCompression::PackBits,
        _ => TiffCompression::Lzw,
    }
}

fn tiff_encoder(meta: &SourceMeta, o: &EngineOptions) -> TiffEncoder {
    TiffEncoder {
        compression: tiff_compression(meta, o),
        extra: carried_tags(meta, o),
        ..TiffEncoder::default()
    }
}

fn icc_arc(meta: &SourceMeta) -> Option<Arc<[u8]>> {
    meta.icc
        .as_ref()
        .map(|v| Arc::<[u8]>::from(v.as_slice()))
        .filter(|p| !p.is_empty())
}

/// The grey plane of an RGB raster whose channels are all equal, if it is one.
fn gray_plane(r: &Raster) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(r.data.len() / 3);
    for px in r.data.as_chunks::<3>().0 {
        if px[0] != px[1] || px[1] != px[2] {
            return None;
        }
        out.push(px[0]);
    }
    Some(out)
}

fn gray_view(g: &GrayImage) -> Result<RasterView<'_>> {
    RasterView::new(
        g.width,
        g.height,
        PixelFormat::Gray8,
        g.width as usize,
        SampleSlice::U8(&g.data),
    )
    .ok_or(ErrKind::Internal)
}

fn one_bit_spec() -> OutputSpec {
    OutputSpec {
        bits: Bits::One {
            bilevel: Bilevel::TiffG4,
        },
        ..OutputSpec::default()
    }
}

fn jpeg_spec(quality: u8) -> OutputSpec {
    OutputSpec {
        quality: Quality::Fixed { value: quality },
        ..OutputSpec::default()
    }
}

/// blake3 of the RGB expansion of the thresholded grey page (what a 1-bit file decodes to).
fn bilevel_hash(g: &GrayImage) -> Result<String> {
    let b = BilevelImage::from_view(&gray_view(g)?, DEFAULT_CUT, &CancelToken::never())
        .map_err(|_| ErrKind::EncodeFailed)?;
    let mut hasher = blake3::Hasher::new();
    let mut row = vec![0u8; g.width as usize * 3];
    for y in 0..g.height {
        for (x, px) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            px.fill(if b.is_black(x as u32, y) { 0 } else { 255 });
        }
        hasher.update(&row);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// The AVIF writer of an engine save: the quality of the options, their speed and chroma, the
/// patched EXIF. Without the `avif` feature there is none.
#[cfg(feature = "avif")]
fn avif_bytes(
    src: &RasterView<'_>,
    dims: (u32, u32),
    icc: Option<&Arc<[u8]>>,
    meta: &SourceMeta,
    o: &EngineOptions,
) -> Result<Vec<u8>> {
    use auto_crop_codecs::encoders::avif::DEFAULT_AVIF_SPEED;
    use auto_crop_codecs::encoders::{AvifChroma, AvifEncoder};
    let enc = AvifEncoder {
        chroma: match o.avif_chroma {
            crate::output::AvifChromaSetting::Yuv444 => AvifChroma::Yuv444,
            _ => AvifChroma::Yuv420,
        },
        speed: o.avif_speed.unwrap_or(DEFAULT_AVIF_SPEED).min(10),
        exif: patched_exif(meta, o, dims),
        ..AvifEncoder::default()
    };
    let enc_meta = EncodeMeta {
        icc: icc.cloned(),
        dpi: None,
    };
    enc.encode(
        src,
        &jpeg_spec(o.quality.avif_quality()),
        &enc_meta,
        &CancelToken::never(),
    )
    .map_err(|_| ErrKind::EncodeFailed)
}

/// An RGB raster as `format` (TIFF, WebP, PDF or AVIF).
pub(crate) fn encode_rgb(
    raster: &Raster,
    format: Format,
    meta: &SourceMeta,
    o: &EngineOptions,
) -> Result<Encoded> {
    if o.one_bit {
        // A 1-bit file is a choice made for a black-and-white page; colour is a typed error.
        return Err(ErrKind::UnsupportedOutput);
    }
    let icc = icc_arc(meta);
    let enc_meta = EncodeMeta {
        icc: icc.clone(),
        dpi: meta.dpi(),
    };
    let cancel = CancelToken::never();
    let src = view(raster)?;
    let dims = (raster.width, raster.height);
    let spec = OutputSpec::default();
    match format {
        Format::Tiff => {
            let enc = tiff_encoder(meta, o);
            let grey = icc.is_none().then(|| gray_plane(raster)).flatten();
            let bytes = match &grey {
                Some(g) => {
                    let v = RasterView::new(
                        raster.width,
                        raster.height,
                        PixelFormat::Gray8,
                        raster.width as usize,
                        SampleSlice::U8(g),
                    )
                    .ok_or(ErrKind::Internal)?;
                    enc.encode(&v, &spec, &enc_meta, &cancel)
                }
                None => enc.encode(&src, &spec, &enc_meta, &cancel),
            }
            .map_err(|_| ErrKind::EncodeFailed)?;
            Ok(Encoded {
                bytes,
                expect: Expect::for_raster(raster, Format::Tiff, icc.as_deref()),
            })
        }
        Format::Webp => {
            let enc = WebpEncoder {
                exif: patched_exif(meta, o, dims),
                xmp: None,
            };
            let bytes = enc
                .encode(&src, &spec, &enc_meta, &cancel)
                .map_err(|_| ErrKind::EncodeFailed)?;
            Ok(Encoded {
                bytes,
                expect: Expect::for_raster(raster, Format::Webp, icc.as_deref()),
            })
        }
        Format::Pdf => {
            let quality = o.quality.jpeg_quality(meta.q_est());
            let jpeg = default_jpeg_encoder()
                .encode(&src, &jpeg_spec(quality), &enc_meta, &cancel)
                .map_err(|_| ErrKind::EncodeFailed)?;
            let mut w = PdfWriter::new(Vec::new()).map_err(|_| ErrKind::EncodeFailed)?;
            let page = w
                .add_page(
                    PageImage::Jpeg {
                        bytes: &jpeg,
                        icc: icc.as_deref(),
                    },
                    meta.dpi(),
                )
                .map_err(|_| ErrKind::EncodeFailed)?;
            let bytes = w.finish().map_err(|_| ErrKind::EncodeFailed)?;
            Ok(Encoded {
                bytes,
                expect: Expect {
                    dims,
                    format: Format::Pdf,
                    icc: icc
                        .as_deref()
                        .filter(|_| !page.icc_dropped)
                        .map(<[u8]>::to_vec),
                    content: Content::Luma(luma_fingerprint(raster)),
                },
            })
        }
        #[cfg(feature = "avif")]
        Format::Avif => {
            // A raster that is grey in all three channels and has no profile is one plane.
            let grey = icc.is_none().then(|| gray_plane(raster)).flatten();
            let bytes = match &grey {
                Some(g) => {
                    let v = RasterView::new(
                        raster.width,
                        raster.height,
                        PixelFormat::Gray8,
                        raster.width as usize,
                        SampleSlice::U8(g),
                    )
                    .ok_or(ErrKind::Internal)?;
                    avif_bytes(&v, dims, None, meta, o)?
                }
                None => avif_bytes(&src, dims, icc.as_ref(), meta, o)?,
            };
            Ok(Encoded {
                bytes,
                expect: Expect::for_raster(raster, Format::Avif, icc.as_deref()),
            })
        }
        _ => Err(ErrKind::UnsupportedOutput),
    }
}

/// An enhanced grey page as `format` (TIFF, WebP or PDF): one channel, no colour profile.
pub(crate) fn encode_gray(
    g: &GrayImage,
    format: Format,
    meta: &SourceMeta,
    o: &EngineOptions,
) -> Result<Encoded> {
    let enc_meta = EncodeMeta {
        icc: None,
        dpi: meta.dpi(),
    };
    let cancel = CancelToken::never();
    let src = gray_view(g)?;
    let dims = (g.width, g.height);
    let spec = if o.one_bit {
        one_bit_spec()
    } else {
        OutputSpec::default()
    };
    if o.one_bit && !matches!(format, Format::Tiff | Format::Pdf) {
        return Err(ErrKind::UnsupportedOutput);
    }
    let content = if o.one_bit {
        Content::Pixels(bilevel_hash(g)?)
    } else {
        Content::Pixels(gray_pixel_hash(g))
    };
    let bytes = match format {
        Format::Tiff => tiff_encoder(meta, o)
            .encode(&src, &spec, &enc_meta, &cancel)
            .map_err(|_| ErrKind::EncodeFailed)?,
        Format::Webp => WebpEncoder {
            exif: patched_exif(meta, o, dims),
            xmp: None,
        }
        .encode(&src, &spec, &enc_meta, &cancel)
        .map_err(|_| ErrKind::EncodeFailed)?,
        Format::Pdf => {
            let mut w = PdfWriter::new(Vec::new()).map_err(|_| ErrKind::EncodeFailed)?;
            let bilevel;
            let image = if o.one_bit {
                bilevel = BilevelImage::from_view(&src, DEFAULT_CUT, &cancel)
                    .map_err(|_| ErrKind::EncodeFailed)?;
                PageImage::G4(&bilevel)
            } else {
                PageImage::Flate {
                    width: g.width,
                    height: g.height,
                    channels: 1,
                    samples: &g.data,
                    icc: None,
                }
            };
            w.add_page(image, meta.dpi())
                .map_err(|_| ErrKind::EncodeFailed)?;
            w.finish().map_err(|_| ErrKind::EncodeFailed)?
        }
        #[cfg(feature = "avif")]
        Format::Avif => avif_bytes(&src, dims, None, meta, o)?,
        _ => return Err(ErrKind::UnsupportedOutput),
    };
    // A lossy format is checked by luma, not by exact pixels.
    let content = if format == Format::Avif {
        Content::Luma(crate::commit::luma_fingerprint_gray(g))
    } else {
        content
    };
    Ok(Encoded {
        bytes,
        expect: Expect {
            dims,
            format,
            icc: None,
            content,
        },
    })
}
