// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Local sanity run of the curve fitter on a folder of real pictures with golden labels.
//!
//! ```text
//! cargo run --release -p auto-crop-eval --example curves_folder -- _data --labels _data/golden/labels \
//!     [--out _data/_curves_out] [--verbose]
//! ```
//!
//! Every labelled item (a label file has one or more `items`, each with a `quad` and optionally
//! `curves`) is fitted from its labelled quad and scored: boundary distance to the labelled outline,
//! IoU against the straight quad, `looks_curved` on pages whose labels have no bent edge. For the
//! single-item pictures the fit is also started from the quad the classical detector finds.
//!
//! Only aggregates go to stdout (n, means, rates). `--verbose` adds one line per item (file names
//! included): local reading only. `--out DIR` writes an overlay PNG per item (labelled outline white,
//! fitted outline red, input quad green); keep DIR inside the private `_data/` folder. The labels and
//! pictures are the owner's, evaluation-only data: nothing here tunes a constant on them, and none
//! of this output belongs in a commit.

use auto_crop_codecs::{Format, decode, encode};
use auto_crop_eval::curve_fit_eval::{Row, evaluate, summarise};
use auto_crop_eval::curves::Curves;
use auto_crop_eval::geom::{P, Quad};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn draw(o: &mut auto_crop_imgproc::Raster, pts: &[P], c: [u8; 3]) {
    for i in 0..pts.len() {
        let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
        let n = ((b[0] - a[0]).abs().max((b[1] - a[1]).abs()))
            .ceil()
            .max(1.0) as usize;
        for k in 0..=n {
            let t = k as f64 / n as f64;
            let (x, y) = (a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t);
            if x >= 0.0 && y >= 0.0 && (x as u32) < o.width && (y as u32) < o.height {
                o.set_pixel(x as u32, y as u32, c);
            }
        }
    }
}

/// No labels: the detector finds a quad, the fitter curves it, one line per picture (local reading
/// only) and an overlay when `out` is given. The "truth" of the scoring is meaningless here.
fn unlabelled(dir: &Path, out: Option<&Path>) {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("read the folder")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    files.sort();
    let full: Quad = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    let no_tags = BTreeMap::new();
    for p in files {
        let Ok(bytes) = std::fs::read(&p) else {
            continue;
        };
        let Ok(decoded) = decode(&bytes) else {
            continue;
        };
        let name = p
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let row = evaluate(
            &decoded.raster,
            &name,
            &no_tags,
            &full,
            None,
            true,
            out.is_some(),
        );
        println!(
            "{name}: skipped={} looks_curved={} curvature={:?} conf={:?} gain={:?}",
            row.skipped, row.looks_curved, row.curv, row.conf, row.gain
        );
        if let (Some(dir), Some((img, _, fit, q))) = (out, &row.overlay) {
            let mut o = img.clone();
            draw(&mut o, q, [0, 255, 0]);
            draw(&mut o, fit, [255, 0, 0]);
            if let Ok(png) = encode(&o, Format::Png, 90, None) {
                let _ = std::fs::write(dir.join(format!("{name}.png")), png);
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mut dir, mut labels, mut out, mut verbose) = (None, None, None, false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--labels" => labels = it.next().map(PathBuf::from),
            "--out" => out = it.next().map(PathBuf::from),
            "--verbose" => verbose = true,
            other => dir = Some(PathBuf::from(other)),
        }
    }
    let Some(dir) = dir else {
        eprintln!("usage: curves_folder IMAGES_DIR [--labels LABELS_DIR] [--out DIR] [--verbose]");
        std::process::exit(2);
    };
    let Some(labels) = labels else {
        unlabelled(&dir, out.as_deref());
        return;
    };
    if let Some(o) = &out {
        std::fs::create_dir_all(o).expect("create the output folder");
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(&labels)
        .expect("read the labels folder")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|e| e == "json")
                && !p
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with('_'))
        })
        .collect();
    files.sort();
    let mut gt_rows: Vec<Row> = Vec::new();
    let mut det_rows: Vec<Row> = Vec::new();
    for lf in &files {
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(lf).expect("read label")).expect("json");
        let Some(image) = v["image"].as_str() else {
            continue;
        };
        let path: &Path = &dir.join(image);
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let Ok(decoded) = decode(&bytes) else {
            continue;
        };
        let items = v["items"].as_array().cloned().unwrap_or_default();
        // The label's slices become tags ("slice:NAME" = "1") for the per-slice summary.
        let tags: BTreeMap<String, String> = v["slices"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str())
                    .map(|s| (format!("slice:{s}"), "1".to_owned()))
                    .collect()
            })
            .unwrap_or_default();
        for (k, item) in items.iter().enumerate() {
            let Ok(quad) = serde_json::from_value::<Quad>(item["quad"].clone()) else {
                continue;
            };
            let curves: Option<Curves> = item
                .get("curves")
                .and_then(|c| serde_json::from_value(c.clone()).ok());
            let id = format!("{image}#{k}");
            let want_overlay = out.is_some();
            let row = evaluate(
                &decoded.raster,
                &id,
                &tags,
                &quad,
                curves.as_ref(),
                false,
                want_overlay,
            );
            if verbose {
                println!(
                    "{id}: curved_label={} looks_curved={} curvature={:?} conf={:?} gain={:?} boundary {:.1}px (straight {:.1}px)",
                    row.curved,
                    row.looks_curved,
                    row.curv,
                    row.conf,
                    row.gain,
                    row.bd_fit_px,
                    row.bd_straight_px
                );
            }
            if let (Some(dir), Some((img, gt, fit, q))) = (&out, &row.overlay) {
                let mut o = img.clone();
                draw(&mut o, q, [0, 255, 0]);
                draw(&mut o, gt, [255, 255, 255]);
                draw(&mut o, fit, [255, 0, 0]);
                if let Ok(png) = encode(&o, Format::Png, 90, None) {
                    let name = format!("{}_{k}.png", image.replace(['/', '\\'], "_"));
                    let _ = std::fs::write(dir.join(name), png);
                }
            }
            gt_rows.push(row);
            if items.len() == 1 {
                det_rows.push(evaluate(
                    &decoded.raster,
                    &id,
                    &tags,
                    &quad,
                    curves.as_ref(),
                    true,
                    false,
                ));
            }
        }
    }
    println!(
        "{} labelled items ({} with bent edges) from {} label files",
        gt_rows.len(),
        gt_rows.iter().filter(|r| r.curved).count(),
        files.len()
    );
    for (title, rows) in [
        ("fitted from the labelled quad", &gt_rows),
        (
            "fitted from the detector's quad (single-item pictures)",
            &det_rows,
        ),
    ] {
        println!("== {title}");
        let all: Vec<&Row> = rows.iter().collect();
        let curved: Vec<&Row> = rows.iter().filter(|r| r.curved).collect();
        let flat: Vec<&Row> = rows.iter().filter(|r| !r.curved).collect();
        summarise("all", &all);
        if !curved.is_empty() {
            summarise("curved labels", &curved);
        }
        summarise("flat labels", &flat);
    }
    println!("== per slice (fitted from the labelled quad; slices with at least 5 items)");
    let mut slices: BTreeMap<String, Vec<&Row>> = BTreeMap::new();
    for r in &gt_rows {
        for k in r.tags.keys() {
            slices.entry(k.clone()).or_default().push(r);
        }
    }
    for (k, rows) in slices {
        if rows.len() >= 5 {
            summarise(&k, &rows);
        }
    }
}
