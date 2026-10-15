// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The finished bytes of an image's outputs, without saving anything. This is the same pixel
//! production as the engine's own saves ([`crate::Engine::save_items`]): the lossless JPEG path
//! when it is exact, else decode once, render each quad with the pixel cap, and encode through
//! `output::encode_raster` (quality from the source's tables, EXIF written once, the metadata and
//! ICC policy of [`EngineOptions`]). A front end that writes copies itself (`auto-crop render`,
//! `process --output`) calls this so its files equal what an in-place save would write.

use crate::curved::PageShape;
use crate::error::{ErrKind, Result, codec_err};
use crate::lossless::{Backend, try_lossless};
use crate::output::{EngineOptions, SourceMeta, encode_enhanced};
use auto_crop_codecs::{Format, decode_with, sniff};
use auto_crop_core::{Enhance, QuadWarp};
use auto_crop_imgproc::cancel::NeverCancel;
use auto_crop_imgproc::render::Limits;
use std::sync::Arc;

/// One finished output.
#[derive(Debug, Clone)]
pub struct RenderedOutput {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub format: Format,
    /// Produced by moving JPEG coefficients, with no re-encode.
    pub lossless: bool,
    /// Notice codes of an enhanced output (`enhance.icc_dropped`, `enhance.png_recommended`).
    pub notices: Vec<String>,
}

/// The format of the outputs of a source in `source` whose decode reported `notices`: the source's
/// own when this build can write it back as itself (JPEG, PNG, TIFF, lossless WebP), JPEG for HEIC
/// (the conversion target of PLAN 3.5), PNG for everything else (a lossy WebP, a 1-bit TIFF, AVIF,
/// BMP, GIF).
pub fn default_output_format(source: Format, notices: &[&str]) -> Format {
    crate::output::copy_format(source, notices, crate::output::SaveFormat::Auto)
}

/// Renders one output per quad of `source` (the bytes of the file). `want` picks the format
/// (`None`: [`default_output_format`]); JPEG, PNG, TIFF, lossless WebP and PDF can be written. A
/// panic in a codec is an error, not an unwind.
pub fn render_outputs(
    source: &[u8],
    quads: &[QuadWarp],
    want: Option<Format>,
    opts: &EngineOptions,
) -> Result<Vec<RenderedOutput>> {
    render_outputs_enhanced(source, quads, &[], want, opts)
}

/// [`render_outputs`] with an enhancement per quad (`enhances[i]` is the enhancement of
/// `quads[i]`; a missing entry is `Original`). The same pixel production as an in-place save: an
/// enhanced quad skips the lossless JPEG path, is rendered once, enhanced, and written through
/// `output::encode_enhanced` (grey for Document and Receipt, the colour profile dropped with a
/// notice in the saved path, PNG recommended), so `process --output` and an in-place `process`
/// agree byte for byte.
pub fn render_outputs_enhanced(
    source: &[u8],
    quads: &[QuadWarp],
    enhances: &[Enhance],
    want: Option<Format>,
    opts: &EngineOptions,
) -> Result<Vec<RenderedOutput>> {
    crate::run_isolated(std::panic::AssertUnwindSafe(|| {
        render_inner(source, quads, enhances, want, opts)
    }))
    .unwrap_or(Err(ErrKind::InternalPanic))
}

fn render_inner(
    source: &[u8],
    quads: &[QuadWarp],
    enhances: &[Enhance],
    want: Option<Format>,
    opts: &EngineOptions,
) -> Result<Vec<RenderedOutput>> {
    if quads.is_empty() {
        return Err(ErrKind::NoCrop);
    }
    let enhance_of = |i: usize| enhances.get(i).copied().unwrap_or_default();
    let src_format = sniff(source);
    // The exact lossless path: one JPEG to one JPEG, for the identity only.
    if let (1, Some(Format::Jpeg)) = (quads.len(), src_format)
        && want.is_none_or(|f| f == Format::Jpeg)
        && enhance_of(0).is_identity()
    {
        let meta = SourceMeta::read(Format::Jpeg, source, None);
        if let Some(l) = try_lossless(&mut Backend::new(), source, &quads[0], &meta, opts) {
            return Ok(vec![RenderedOutput {
                bytes: l.bytes,
                width: l.dims.0,
                height: l.dims.1,
                format: Format::Jpeg,
                lossless: true,
                notices: Vec::new(),
            }]);
        }
    }
    let decoded = decode_with(source, &opts.limits()).map_err(codec_err)?;
    let fmt = want.unwrap_or_else(|| default_output_format(decoded.format, &decoded.notices));
    if !fmt.is_writable() {
        return Err(ErrKind::UnsupportedOutput);
    }
    let meta = SourceMeta::read(decoded.format, source, decoded.icc.map(Arc::new));
    quads
        .iter()
        .enumerate()
        .map(|(i, q)| {
            let page = PageShape::Quad(q.clone())
                .render_enhanced(
                    &decoded.raster,
                    Limits::pixels(opts.max_pixels),
                    enhance_of(i),
                    &NeverCancel,
                )
                .map_err(|_| ErrKind::NoCrop)?;
            let (width, height) = page.pixels.dims();
            let enc = encode_enhanced(&page.pixels, fmt, &meta, opts)?;
            Ok(RenderedOutput {
                bytes: enc.encoded.bytes,
                width,
                height,
                format: fmt,
                lossless: false,
                notices: enc.notices.iter().map(|n| (*n).to_owned()).collect(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use auto_crop_core::Pt;
    use auto_crop_imgproc::Raster;

    fn quad() -> QuadWarp {
        QuadWarp::new([
            Pt::new(0.1, 0.1),
            Pt::new(0.9, 0.12),
            Pt::new(0.88, 0.9),
            Pt::new(0.12, 0.88),
        ])
    }

    #[test]
    fn renders_one_output_per_quad_in_the_wanted_format() {
        let src = auto_crop_codecs::encode(
            &Raster::filled(200, 150, [90, 120, 60]),
            Format::Jpeg,
            90,
            None,
        )
        .unwrap();
        let o = EngineOptions::default();
        let outs = render_outputs(&src, &[quad(), quad()], None, &o).unwrap();
        assert_eq!(outs.len(), 2);
        assert!(outs.iter().all(|r| r.format == Format::Jpeg && !r.lossless));
        let png = render_outputs(&src, &[quad()], Some(Format::Png), &o).unwrap();
        assert_eq!(png[0].format, Format::Png);
        assert_eq!(
            auto_crop_codecs::decode(&png[0].bytes)
                .unwrap()
                .raster
                .width,
            png[0].width
        );
        assert_eq!(
            render_outputs(&src, &[], None, &o).unwrap_err(),
            ErrKind::NoCrop
        );
        assert_eq!(
            render_outputs(&src, &[quad()], Some(Format::Avif), &o).unwrap_err(),
            ErrKind::UnsupportedOutput
        );
        assert!(render_outputs(b"not an image", &[quad()], None, &o).is_err());
    }

    #[test]
    fn the_default_format_follows_the_source() {
        assert_eq!(default_output_format(Format::Jpeg, &[]), Format::Jpeg);
        assert_eq!(default_output_format(Format::Heic, &[]), Format::Jpeg);
        assert_eq!(default_output_format(Format::Tiff, &[]), Format::Tiff);
        assert_eq!(default_output_format(Format::Webp, &[]), Format::Webp);
        assert_eq!(
            default_output_format(Format::Webp, &["webp.lossy"]),
            Format::Png
        );
        assert_eq!(
            default_output_format(Format::Tiff, &["tiff.low_depth"]),
            Format::Png
        );
        assert_eq!(default_output_format(Format::Bmp, &[]), Format::Png);
    }
}
