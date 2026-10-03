// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Decoder check on the generator's format, EXIF orientation and colour-space variants
//! (ROADMAP M1.34 acceptance: "the M1 decoders reproduce the upright reference for every
//! variant"; feeds M1.17).
//!
//! `python -m synth variants --out DIR` writes, for two pictures, every format (JPEG, PNG, TIFF,
//! WebP lossy and lossless) in each EXIF orientation 1 to 8 and in sRGB and Display P3, with the
//! *stored* pixels pre-turned so a reader that honours the tag recovers the upright picture, plus
//! the upright reference PNGs and `variants.jsonl` naming what each file must decode to. This
//! module decodes every variant with `auto_crop_codecs::decode` and checks that
//!
//! * the decode succeeds and reports the orientation that was written,
//! * the oriented size equals the reference size,
//! * the embedded ICC profile comes back byte for byte (SHA-256 in the list), or is absent when
//!   none was written,
//! * the pixels equal the reference exactly for lossless variants, and within the listed mean
//!   absolute error for lossy ones (the listed bound is twice what libjpeg or libwebp's own
//!   decoder reaches, at least 3 grey levels).
//!
//! The Rust decoders do not colour-manage, so a Display P3 variant decodes to its stored P3 values,
//! which are what the P3 reference holds.

use crate::manifest::sha256_hex;
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize)]
struct Variant {
    file: String,
    format: String,
    orientation: u8,
    colorspace: String,
    lossless: bool,
    reference: String,
    width: u32,
    height: u32,
    icc_sha256: Option<String>,
    max_mean_abs_err: f64,
}

#[derive(Debug, Default)]
pub struct VariantReport {
    pub checked: usize,
    pub lossless: usize,
    /// One line per failed variant.
    pub failures: Vec<String>,
    /// Largest mean absolute error seen over lossy variants.
    pub worst_lossy_error: f64,
}

fn mean_abs_err(a: &[u8], b: &[u8]) -> f64 {
    let sum: u64 = a
        .iter()
        .zip(b)
        .map(|(x, y)| u64::from(x.abs_diff(*y)))
        .sum();
    sum as f64 / a.len().max(1) as f64
}

/// Checks every variant listed in `dir/variants.jsonl`.
pub fn check_dir(dir: &Path) -> Result<VariantReport, String> {
    let list = dir.join("variants.jsonl");
    let text = std::fs::read_to_string(&list)
        .map_err(|e| format!("cannot read {}: {e}", list.display()))?;
    let mut report = VariantReport::default();
    let mut refs: std::collections::BTreeMap<String, auto_crop_codecs::Decoded> =
        Default::default();
    for (no, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let v: Variant = serde_json::from_str(line)
            .map_err(|e| format!("variants.jsonl line {}: {e}", no + 1))?;
        report.checked += 1;
        if v.lossless {
            report.lossless += 1;
        }
        let tag = format!(
            "{} ({} o{} {})",
            v.file, v.format, v.orientation, v.colorspace
        );
        if !refs.contains_key(&v.reference) {
            let bytes = std::fs::read(dir.join(&v.reference))
                .map_err(|e| format!("cannot read reference {}: {e}", v.reference))?;
            let d = auto_crop_codecs::decode(&bytes)
                .map_err(|e| format!("reference {} does not decode: {e}", v.reference))?;
            refs.insert(v.reference.clone(), d);
        }
        let reference = &refs[&v.reference];
        let bytes =
            std::fs::read(dir.join(&v.file)).map_err(|e| format!("cannot read {}: {e}", v.file))?;
        let d = match auto_crop_codecs::decode(&bytes) {
            Ok(d) => d,
            Err(e) => {
                report.failures.push(format!("{tag}: decode failed: {e}"));
                continue;
            }
        };
        if d.exif_orientation != v.orientation {
            report.failures.push(format!(
                "{tag}: decoder read EXIF orientation {}, written {}",
                d.exif_orientation, v.orientation
            ));
        }
        if (d.raster.width, d.raster.height) != (v.width, v.height) {
            report.failures.push(format!(
                "{tag}: oriented size {}x{}, expected {}x{}",
                d.raster.width, d.raster.height, v.width, v.height
            ));
            continue;
        }
        match (&v.icc_sha256, &d.icc) {
            (Some(want), Some(got)) if sha256_hex(got) == *want => {}
            (Some(_), Some(_)) => report
                .failures
                .push(format!("{tag}: ICC profile bytes changed")),
            (Some(_), None) => report.failures.push(format!("{tag}: ICC profile lost")),
            (None, Some(_)) => report
                .failures
                .push(format!("{tag}: ICC profile appeared from nowhere")),
            (None, None) => {}
        }
        if reference.raster.data.len() != d.raster.data.len() {
            report.failures.push(format!(
                "{tag}: pixel buffer size differs from the reference"
            ));
            continue;
        }
        let err = mean_abs_err(&reference.raster.data, &d.raster.data);
        if v.lossless {
            if err != 0.0 {
                report.failures.push(format!(
                    "{tag}: lossless variant differs from the upright reference (mean abs error {err:.4})"
                ));
            }
        } else {
            report.worst_lossy_error = report.worst_lossy_error.max(err);
            if err > v.max_mean_abs_err {
                report.failures.push(format!(
                    "{tag}: mean abs error {err:.3} exceeds {:.3}",
                    v.max_mean_abs_err
                ));
            }
        }
    }
    if report.checked == 0 {
        return Err(format!("{} lists no variants", list.display()));
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use auto_crop_imgproc::Raster;

    #[test]
    fn mean_error_is_the_average_absolute_difference() {
        assert_eq!(mean_abs_err(&[0, 10, 20], &[0, 10, 20]), 0.0);
        assert!((mean_abs_err(&[0, 0], &[3, 5]) - 4.0).abs() < 1e-12);
    }

    fn write_png(path: &Path, w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 3]) -> Raster {
        let mut data = Vec::new();
        for y in 0..h {
            for x in 0..w {
                data.extend_from_slice(&f(x, y));
            }
        }
        let r = Raster::from_raw(w, h, data).expect("raster");
        let bytes =
            auto_crop_codecs::encode(&r, auto_crop_codecs::Format::Png, 0, None).expect("encode");
        std::fs::write(path, bytes).expect("write");
        r
    }

    /// A one-variant directory built here (PNG, orientation 1) proves the checker accepts a good
    /// variant and names a bad one; the real variants come from the Python generator in CI.
    #[test]
    fn a_matching_png_passes_and_a_changed_pixel_is_reported() {
        let dir = std::env::temp_dir().join(format!("ac-variants-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let up = |x: u32, y: u32| [(x * 7) as u8, (y * 11) as u8, ((x + y) * 3) as u8];
        write_png(&dir.join("ref.png"), 16, 8, up);
        write_png(&dir.join("v.png"), 16, 8, up);
        let item = |file: &str| {
            format!(
                "{{\"file\":\"{file}\",\"format\":\"png\",\"orientation\":1,\"colorspace\":\"srgb\",\"lossless\":true,\"reference\":\"ref.png\",\"width\":16,\"height\":8,\"icc_sha256\":null,\"max_mean_abs_err\":0.0}}\n"
            )
        };
        std::fs::write(dir.join("variants.jsonl"), item("v.png")).expect("list");
        let good = check_dir(&dir).expect("checks");
        assert_eq!((good.checked, good.lossless), (1, 1));
        assert!(good.failures.is_empty(), "{:?}", good.failures);
        write_png(&dir.join("bad.png"), 16, 8, |x, y| {
            if (x, y) == (3, 3) {
                [0, 0, 1]
            } else {
                up(x, y)
            }
        });
        std::fs::write(dir.join("variants.jsonl"), item("bad.png")).expect("list");
        let bad = check_dir(&dir).expect("checks");
        assert_eq!(bad.failures.len(), 1, "{:?}", bad.failures);
        assert!(bad.failures[0].contains("differs from the upright reference"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_list_or_an_empty_one_is_an_error() {
        let dir = std::env::temp_dir().join(format!("ac-variants-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        assert!(check_dir(&dir).is_err());
        std::fs::write(dir.join("variants.jsonl"), "\n").expect("list");
        assert!(check_dir(&dir).expect_err("empty").contains("no variants"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
