// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Local check of the classical detector against golden labels of single-item pictures.
//!
//! ```text
//! cargo run --release -p auto-crop-eval --example labels_detect -- _data --labels _data/golden/labels \
//!     [--out _data/_out_labels] [--rows]
//! ```
//!
//! Only aggregates go to stdout: items scored, mean IoU, pictures below IoU 0.90, results the
//! detector would auto-accept (Good), silent failures among them, the held share and the detect
//! time. `--rows` adds one line per picture with its running number (no file names), the verdict,
//! the IoU and the reason codes, for local reading. `--out DIR` writes an overlay per picture
//! (label white, detection coloured by verdict) named by that number; keep DIR inside the private
//! `_data/`. The labels and pictures are the owner's evaluation-only data: nothing may be tuned on
//! them alone and none of this output belongs in a commit.

use auto_crop_codecs::{Format, decode, encode};
use auto_crop_core::Forced;
use auto_crop_eval::geom::Quad;
use auto_crop_eval::metrics::canonical_iou;
use auto_crop_imgproc::Raster;
use auto_crop_imgproc::detect::detect;
use auto_crop_imgproc::scale::resize_to_fit;
use std::path::{Path, PathBuf};

fn put(r: &mut Raster, x: i64, y: i64, c: [u8; 3]) {
    if x >= 0 && y >= 0 && (x as u32) < r.width && (y as u32) < r.height {
        let i = ((y as u32 * r.width + x as u32) * 3) as usize;
        r.data[i..i + 3].copy_from_slice(&c);
    }
}

fn line(r: &mut Raster, a: (f64, f64), b: (f64, f64), c: [u8; 3]) {
    let n = ((b.0 - a.0).abs().max((b.1 - a.1).abs()).ceil() as usize).max(1) * 2;
    for i in 0..=n {
        let t = i as f64 / n as f64;
        let (x, y) = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
        for d in -1..=1 {
            put(r, x as i64 + d, y as i64, c);
            put(r, x as i64, y as i64 + d, c);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mut dir, mut labels, mut out, mut rows) = (None, None, None, false);
    let mut trace_n: Option<usize> = None;
    let mut only: Option<usize> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--labels" => labels = it.next().map(PathBuf::from),
            "--out" => out = it.next().map(PathBuf::from),
            "--rows" => rows = true,
            "--only" => only = it.next().and_then(|v| v.parse().ok()),
            "--trace" => trace_n = it.next().and_then(|v| v.parse().ok()),
            other => dir = Some(PathBuf::from(other)),
        }
    }
    let (Some(dir), Some(labels)) = (dir, labels) else {
        eprintln!("usage: labels_detect IMAGES_DIR --labels LABELS_DIR [--out DIR] [--rows]");
        std::process::exit(2);
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

    let (mut n, mut below, mut good, mut silent, mut held, mut none) = (0, 0, 0, 0, 0, 0);
    let (mut sum_iou, mut sum_ms) = (0.0f64, 0.0f64);
    for lf in &files {
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(lf).expect("read label")).expect("json");
        let Some(image) = v["image"].as_str() else {
            continue;
        };
        let items = v["items"].as_array().cloned().unwrap_or_default();
        if items.len() != 1 || v["assisted"].as_bool().unwrap_or(false) {
            continue;
        }
        let Ok(truth) = serde_json::from_value::<Quad>(items[0]["quad"].clone()) else {
            continue;
        };
        let Ok(bytes) = std::fs::read(Path::new(&dir).join(image)) else {
            continue;
        };
        let Ok(decoded) = decode(&bytes) else {
            continue;
        };
        n += 1;
        if only.is_some_and(|o| o != n as usize) {
            continue;
        }
        let t0 = std::time::Instant::now();
        let det = detect(&decoded.raster);
        sum_ms += t0.elapsed().as_secs_f64() * 1000.0;
        let score = f64::from(det.confidence.score);
        let verdict = match (det.quad.is_some(), det.confidence.forced) {
            (false, _) | (_, Some(Forced::Failed)) => "Failed",
            (_, Some(Forced::Check)) => "Check",
            _ if score >= 0.90 => "Good",
            _ if score >= 0.60 => "Check",
            _ => "Failed",
        };
        let pred: Option<Quad> = det.quad.map(|q| std::array::from_fn(|i| [q[i].x, q[i].y]));
        let iou = pred
            .as_ref()
            .and_then(|p| canonical_iou(&truth, p).ok())
            .unwrap_or(0.0);
        sum_iou += iou;
        if iou < 0.90 {
            below += 1;
        }
        match verdict {
            "Good" => {
                good += 1;
                if iou < 0.90 {
                    silent += 1;
                }
            }
            "Check" => held += 1,
            _ => none += 1,
        }
        if trace_n == Some(n as usize) {
            for c in auto_crop_imgproc::detect::trace(&decoded.raster)
                .iter()
                .take(12)
            {
                let q: Vec<String> = c
                    .quad
                    .iter()
                    .map(|p| format!("({:.3},{:.3})", p.x, p.y))
                    .collect();
                println!(
                    "  cand rank {:.3} score {:.2} {:?} {}",
                    c.rank,
                    c.score,
                    c.forced,
                    q.join(" ")
                );
            }
            let t: Vec<String> = truth
                .iter()
                .map(|p| format!("({:.3},{:.3})", p[0], p[1]))
                .collect();
            println!("  truth {}", t.join(" "));
        }
        let reasons: Vec<String> = det
            .confidence
            .reasons
            .iter()
            .map(|r| format!("{:?}/{:?}", r.code, r.side))
            .collect();
        if rows {
            println!(
                "#{n:02} {verdict:6} score {score:.2} iou {iou:.3} {}",
                reasons.join(" ")
            );
        }
        if let Some(o) = &out {
            let mut preview = resize_to_fit(&decoded.raster, 900);
            let (w, h) = (f64::from(preview.width), f64::from(preview.height));
            let px =
                |q: &Quad| -> Vec<(f64, f64)> { q.iter().map(|p| (p[0] * w, p[1] * h)).collect() };
            let t = px(&truth);
            for i in 0..4 {
                line(&mut preview, t[i], t[(i + 1) % 4], [255, 255, 255]);
            }
            if let Some(p) = &pred {
                let c = match verdict {
                    "Good" => [60, 220, 60],
                    "Check" => [255, 190, 0],
                    _ => [235, 60, 60],
                };
                let p = px(p);
                for i in 0..4 {
                    line(&mut preview, p[i], p[(i + 1) % 4], c);
                }
            }
            if let Ok(png) = encode(&preview, Format::Png, 90, None) {
                let _ = std::fs::write(o.join(format!("{n:02}.png")), png);
            }
        }
    }
    if n == 0 {
        println!("no single-item labelled pictures found");
        return;
    }
    let nf = f64::from(n);
    println!(
        "items {n}  mean IoU {:.3}  below 0.90: {below}  none: {none}  auto-accepted: {good}  silent failures: {silent}  held (Check): {held}  detect {:.0} ms/picture",
        sum_iou / nf,
        sum_ms / nf
    );
}
