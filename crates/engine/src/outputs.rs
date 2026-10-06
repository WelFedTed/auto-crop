// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The finished bytes of an image's outputs, without saving anything. This is the same pixel
//! production as the engine's own saves ([`crate::Engine::save_items`]): the lossless JPEG path
//! when it is exact, else decode once, render each quad with the pixel cap, and encode through
//! `output::encode_raster` (quality from the source's tables, EXIF written once, the metadata and
//! ICC policy of [`EngineOptions`]). A front end that writes copies itself (`auto-crop render`,
//! `process --output`) calls this so its files equal what an in-place save would write.

use crate::error::{ErrKind, Result, codec_err};
use crate::lossless::{Backend, try_lossless};
use crate::output::{EngineOptions, SourceMeta, encode_raster};
use auto_crop_codecs::{Format, decode_with, sniff};
use auto_crop_core::QuadWarp;
use auto_crop_imgproc::render::{Limits, render_quad};
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
}

/// The format of the outputs of a source in `source`: the source's own when this build can write
/// it, JPEG for HEIC (the conversion target of PLAN 3.5), PNG for everything else.
pub fn default_output_format(source: Format) -> Format {
    if source.is_encodable() {
        source
    } else if source == Format::Heic {
        Format::Jpeg
    } else {
        Format::Png
    }
}

/// Renders one output per quad of `source` (the bytes of the file). `want` picks the format
/// (`None`: [`default_output_format`]); only JPEG and PNG can be written. A panic in a codec is an
/// error, not an unwind.
pub fn render_outputs(
    source: &[u8],
    quads: &[QuadWarp],
    want: Option<Format>,
    opts: &EngineOptions,
) -> Result<Vec<RenderedOutput>> {
    crate::run_isolated(std::panic::AssertUnwindSafe(|| {
        render_inner(source, quads, want, opts)
    }))
    .unwrap_or(Err(ErrKind::InternalPanic))
}

fn render_inner(
    source: &[u8],
    quads: &[QuadWarp],
    want: Option<Format>,
    opts: &EngineOptions,
) -> Result<Vec<RenderedOutput>> {
    if quads.is_empty() {
        return Err(ErrKind::NoCrop);
    }
    let src_format = sniff(source);
    // The exact lossless path: one JPEG to one JPEG.
    if let (1, Some(Format::Jpeg)) = (quads.len(), src_format)
        && want.is_none_or(|f| f == Format::Jpeg)
    {
        let meta = SourceMeta::read(Format::Jpeg, source, None);
        if let Some(l) = try_lossless(&mut Backend::new(), source, &quads[0], &meta, opts) {
            return Ok(vec![RenderedOutput {
                bytes: l.bytes,
                width: l.dims.0,
                height: l.dims.1,
                format: Format::Jpeg,
                lossless: true,
            }]);
        }
    }
    let decoded = decode_with(source, &opts.limits()).map_err(codec_err)?;
    let fmt = want.unwrap_or_else(|| default_output_format(decoded.format));
    if !fmt.is_encodable() {
        return Err(ErrKind::UnsupportedOutput);
    }
    let meta = SourceMeta::read(decoded.format, source, decoded.icc.map(Arc::new));
    quads
        .iter()
        .map(|q| {
            let out = render_quad(&decoded.raster, q, Limits::pixels(opts.max_pixels))
                .map_err(|_| ErrKind::NoCrop)?;
            let enc = encode_raster(&out, fmt, &meta, opts)?;
            Ok(RenderedOutput {
                bytes: enc.bytes,
                width: out.width,
                height: out.height,
                format: fmt,
                lossless: false,
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
            render_outputs(&src, &[quad()], Some(Format::Webp), &o).unwrap_err(),
            ErrKind::UnsupportedOutput
        );
        assert!(render_outputs(b"not an image", &[quad()], None, &o).is_err());
    }

    #[test]
    fn the_default_format_follows_the_source() {
        assert_eq!(default_output_format(Format::Jpeg), Format::Jpeg);
        assert_eq!(default_output_format(Format::Heic), Format::Jpeg);
        assert_eq!(default_output_format(Format::Tiff), Format::Png);
    }
}
