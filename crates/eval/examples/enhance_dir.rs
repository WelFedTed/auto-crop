// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Runs the enhancement pipeline over a folder of images (dev tool and OCR-oracle front end).
//!
//! ```text
//! cargo run --release -p auto-crop-eval --example enhance_dir -- \
//!     --in DIR --out DIR --mode original|clean|document|receipt [--strength 1.0] [--spec k=v,k=v]
//! ```
//!
//! Every PNG or JPEG in `--in` is decoded, enhanced and written as PNG under the same stem in
//! `--out` (8-bit grey for the thresholded modes). `--spec` overrides single [`Tuning`] fields for
//! sweeps: `window` (factor, both modes), `block` (factor, the mode's own), `sauvola_k`, `nick_k`,
//! `method` (`sauvola` or `nick`), `rho`, `edge` (`hard` or `soft`), `percentile`, `passes`,
//! `cap`, `clamp_lo`, `clamp_hi`, `nick_hi`, `white`. The per-image report is printed as one JSON
//! line, so a CI log carries the measures next to the OCR numbers.

use auto_crop_codecs::{Format, decode, encode};
use auto_crop_core::CancelToken;
use auto_crop_core::OutputSpec;
use auto_crop_core::ports::{EncodeMeta, Encoder, PixelFormat, RasterView, SampleSlice};
use auto_crop_imgproc::cancel::NeverCancel;
use auto_crop_imgproc::enhance::{Binariser, Edge, Enhanced, Mode, Params, Tuning, enhance};
use std::path::{Path, PathBuf};
use std::time::Instant;

fn parse_spec(spec: &str, mode: Mode, t: &mut Tuning) -> Result<(), String> {
    for kv in spec.split(',').filter(|s| !s.is_empty()) {
        let (k, v) = kv
            .split_once('=')
            .ok_or_else(|| format!("bad spec item `{kv}`"))?;
        let f = || v.parse::<f32>().map_err(|e| format!("{k}: {e}"));
        match k {
            "window" => {
                t.window_document = f()?;
                t.window_receipt = f()?;
            }
            "block" => match mode {
                Mode::Clean => t.block_clean = f()?,
                Mode::Document => t.block_document = f()?,
                _ => t.block_receipt = f()?,
            },
            "sauvola_k" => t.sauvola_k = f()?,
            "nick_k" => t.nick_k = f()?,
            "method" => {
                let m = match v {
                    "sauvola" => Binariser::Sauvola,
                    "nick" => Binariser::Nick,
                    _ => return Err(format!("method: `{v}`")),
                };
                t.document_method = m;
                t.receipt_method = m;
            }
            "rho" => t.rho = f()?,
            "edge" => {
                t.edge = match v {
                    "hard" => Edge::Hard,
                    "soft" => Edge::Soft,
                    _ => return Err(format!("edge: `{v}`")),
                }
            }
            "percentile" => t.percentile = f()?,
            "passes" => t.passes = f()? as u8,
            "cap" => t.gain_cap = f()?,
            "clamp_lo" => t.clamp_lo = f()?,
            "clamp_hi" => t.clamp_hi = f()?,
            "nick_hi" => t.nick_hi = f()?,
            "white" => t.tone_white = f()?,
            _ => return Err(format!("unknown spec key `{k}`")),
        }
    }
    Ok(())
}

fn write_png(path: &Path, out: &Enhanced) -> Result<(), String> {
    let bytes = match out {
        Enhanced::Rgb(r) => encode(r, Format::Png, 90, None).map_err(|e| format!("{e:?}"))?,
        Enhanced::Gray(g) => {
            let view = RasterView::new(
                g.width,
                g.height,
                PixelFormat::Gray8,
                g.width as usize,
                SampleSlice::U8(&g.data),
            )
            .ok_or("view")?;
            auto_crop_codecs::encoders::PngEncoder
                .encode(
                    &view,
                    &OutputSpec::default(),
                    &EncodeMeta {
                        icc: None,
                        dpi: Some((300, 300)),
                    },
                    &CancelToken::never(),
                )
                .map_err(|e| format!("{e:?}"))?
        }
    };
    std::fs::write(path, bytes).map_err(|e| e.to_string())
}

fn main() -> Result<(), String> {
    let (mut input, mut output, mut mode, mut strength, mut spec) =
        (None, None, "document".to_owned(), 1.0f32, String::new());
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut next = || args.next().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--in" => input = Some(PathBuf::from(next()?)),
            "--out" => output = Some(PathBuf::from(next()?)),
            "--mode" => mode = next()?,
            "--strength" => strength = next()?.parse().map_err(|e| format!("{e}"))?,
            "--spec" => spec = next()?,
            _ => return Err(format!("unknown argument {a}")),
        }
    }
    let (input, output) = (
        input.ok_or("--in is required")?,
        output.ok_or("--out is required")?,
    );
    let mode = match mode.as_str() {
        "original" => Mode::Original,
        "clean" => Mode::Clean,
        "document" => Mode::Document,
        "receipt" => Mode::Receipt,
        other => return Err(format!("unknown mode `{other}`")),
    };
    let mut params = Params::new(mode, strength);
    parse_spec(&spec, mode, &mut params.tuning)?;
    std::fs::create_dir_all(&output).map_err(|e| e.to_string())?;
    let mut files: Vec<PathBuf> = std::fs::read_dir(&input)
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg"))
        })
        .collect();
    files.sort();
    let mut total_ms = 0.0;
    for f in &files {
        let bytes = std::fs::read(f).map_err(|e| e.to_string())?;
        let img = decode(&bytes)
            .map_err(|e| format!("{}: {e:?}", f.display()))?
            .raster;
        let t = Instant::now();
        let (out, rep) =
            enhance(img, &params, None, &NeverCancel).map_err(|_| "cancelled".to_owned())?;
        let ms = t.elapsed().as_secs_f64() * 1e3;
        total_ms += ms;
        let name = f.file_stem().and_then(|s| s.to_str()).unwrap_or("image");
        write_png(&output.join(format!("{name}.png")), &out)?;
        println!(
            "{{\"image\":\"{name}\",\"ms\":{ms:.1},\"text_h\":{:.1},\"window\":{},\"block\":{},\"rejected\":{:.3},\"max_gain\":{:.2},\"shading\":{:.3},\"paper_fraction\":{:.3},\"ink_fraction\":{:.3},\"ink_contrast\":{:.0},\"faded\":{},\"low_contrast\":{}}}",
            rep.text_h,
            rep.window,
            rep.block,
            rep.rejected,
            rep.max_gain,
            rep.shading,
            rep.paper_fraction,
            rep.ink_fraction,
            rep.ink_contrast,
            rep.faded,
            rep.low_contrast
        );
    }
    eprintln!("{} images, {total_ms:.0} ms in the enhancer", files.len());
    Ok(())
}
