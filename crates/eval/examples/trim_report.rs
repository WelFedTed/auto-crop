// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Local check of "Trim to content" (`auto_crop_imgproc::trim`) over a synthetic manifest: how often
//! a proposal is made on pages that must never be trimmed (documents, receipts: false triggers),
//! and, for the matted prints of the `real-cases` suite, how close the proposal is to the picture.
//!
//! ```text
//! cargo run --release -p auto-crop-eval --example trim_report -- target/synth/full/manifest.jsonl [--by TAG]
//! cargo run --release -p auto-crop-eval --example trim_report -- target/synth/real-cases-rust/manifest.jsonl --pictures
//! ```
//!
//! Each picture is run twice: on the detector's quad (what the app does) and on the ground-truth
//! quad. Only aggregates go to stdout. `--by TAG` groups by that manifest tag (default `aspect`),
//! `--show N` lists the first N pictures that got a proposal where none is expected. Pictures are
//! reduced to the engine's 2048 px display proxy first, like the app. `--out DIR` writes an overlay
//! (ground truth white, the detector's quad amber, the proposal cyan) of every proposal where none is
//! expected (so, not with `--pictures`).

use auto_crop_codecs::{Format, decode, encode};
use auto_crop_core::{Forced, Pt};
use auto_crop_eval::manifest;
use auto_crop_eval::synth::SuiteSpec;
use auto_crop_eval::synth_real::scene_with_picture;
use auto_crop_imgproc::Raster;
use auto_crop_imgproc::detect::detect;
use auto_crop_imgproc::scale::resize_to_fit;
use auto_crop_imgproc::trim::trim_to_content;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Default)]
struct Group {
    n: usize,
    detected: usize,
    on_detected: usize,
    on_truth: usize,
    on_good: usize,
    errors: Vec<f64>,
    ms: Vec<f64>,
}

fn corner_error(found: &[Pt; 4], truth: &[[f64; 2]; 4], w: f64, h: f64) -> f64 {
    (0..4)
        .map(|i| ((found[i].x - truth[i][0]) * w).hypot((found[i].y - truth[i][1]) * h))
        .fold(0.0, f64::max)
        / w.hypot(h)
}

fn put(r: &mut Raster, x: i64, y: i64, c: [u8; 3]) {
    if x >= 0 && y >= 0 && (x as u32) < r.width && (y as u32) < r.height {
        let i = ((y as u32 * r.width + x as u32) * 3) as usize;
        r.data[i..i + 3].copy_from_slice(&c);
    }
}

fn outline(r: &mut Raster, q: &[(f64, f64); 4], c: [u8; 3]) {
    for i in 0..4 {
        let (a, b) = (q[i], q[(i + 1) % 4]);
        let n = ((b.0 - a.0).abs().max((b.1 - a.1).abs()).ceil() as usize).max(1) * 2;
        for k in 0..=n {
            let t = k as f64 / n as f64;
            let (x, y) = (
                (a.0 + (b.0 - a.0) * t) as i64,
                (a.1 + (b.1 - a.1) * t) as i64,
            );
            for d in 0..2 {
                put(r, x + d, y, c);
                put(r, x, y + d, c);
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.first() else {
        eprintln!("usage: trim_report <manifest.jsonl> [--by TAG] [--pictures] [--show N]");
        std::process::exit(2);
    };
    let (mut by, mut pictures, mut show) = ("aspect".to_owned(), false, 0usize);
    let mut out: Option<std::path::PathBuf> = None;
    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--by" => by = it.next().cloned().unwrap_or(by),
            "--pictures" => pictures = true,
            "--out" => out = it.next().map(std::path::PathBuf::from),
            "--show" => show = it.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            _ => {}
        }
    }
    let m = manifest::load(Path::new(path)).expect("manifest");
    let spec = SuiteSpec::named("real-cases").expect("the real-cases spec");
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    let mut shown = 0usize;
    for (index, item) in m.items.iter().enumerate() {
        let Ok(bytes) = std::fs::read(m.resolve(item)) else {
            continue;
        };
        let Ok(decoded) = decode(&bytes) else {
            continue;
        };
        let img = resize_to_fit(&decoded.raster, 2048);
        let key = item
            .tags
            .get(&by)
            .cloned()
            .unwrap_or_else(|| "(untagged)".to_owned());
        let g = groups.entry(key).or_default();
        g.n += 1;
        let det = detect(&img);
        let good = det.confidence.forced.is_none() && det.confidence.score >= 0.9;
        let _ = Forced::Check;
        if let Some(q) = det.quad {
            g.detected += 1;
            let t0 = std::time::Instant::now();
            let r = trim_to_content(&img, q);
            g.ms.push(t0.elapsed().as_secs_f64() * 1000.0);
            if let Some(r) = &r {
                g.on_detected += 1;
                if good {
                    g.on_good += 1;
                }
                if pictures {
                    // The ground-truth picture of a matted print: regenerate the scene by its index.
                    let n: usize = item
                        .id
                        .rsplit('-')
                        .next()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(index);
                    if let Some(truth) = scene_with_picture(&spec, n).3 {
                        g.errors.push(corner_error(
                            &r.quad,
                            &truth,
                            f64::from(img.width),
                            f64::from(img.height),
                        ));
                    }
                } else if let Some(dir) = out.as_ref().filter(|_| shown < show.max(40)) {
                    shown += 1;
                    std::fs::create_dir_all(dir).expect("out dir");
                    let mut pic = resize_to_fit(&img, 800);
                    let (w, h) = (f64::from(pic.width), f64::from(pic.height));
                    let px = |q: &[Pt; 4]| -> [(f64, f64); 4] {
                        std::array::from_fn(|i| (q[i].x * w, q[i].y * h))
                    };
                    outline(
                        &mut pic,
                        &std::array::from_fn(|i| (item.quad[i][0] * w, item.quad[i][1] * h)),
                        [255, 255, 255],
                    );
                    outline(&mut pic, &px(&q), [255, 190, 0]);
                    outline(&mut pic, &px(&r.quad), [0, 220, 255]);
                    if let Ok(png) = encode(&pic, Format::Png, 90, None) {
                        let _ = std::fs::write(dir.join(format!("{}.png", item.id)), png);
                    }
                } else if shown < show {
                    shown += 1;
                    println!(
                        "  proposal on {} ({}) confidence {:.2} {:?}",
                        item.id, g.n, r.confidence, r.reason
                    );
                }
            }
        }
        let tq = item.quad.map(|p| Pt::new(p[0], p[1]));
        if trim_to_content(&img, tq).is_some() {
            g.on_truth += 1;
        }
    }
    println!(
        "{:<22} {:>5} {:>9} {:>14} {:>14} {:>14} {:>9}",
        by, "n", "detected", "Some(detector)", "Some(Good)", "Some(truth)", "ms/trim"
    );
    let (mut tn, mut td, mut ts, mut tg, mut tt) = (0, 0, 0, 0, 0);
    for (k, g) in &groups {
        let ms = g.ms.iter().sum::<f64>() / g.ms.len().max(1) as f64;
        println!(
            "{k:<22} {:>5} {:>9} {:>6} {:>6.1}% {:>7} {:>6.1}% {:>7} {:>6.1}% {ms:>9.1}",
            g.n,
            g.detected,
            g.on_detected,
            100.0 * g.on_detected as f64 / g.detected.max(1) as f64,
            g.on_good,
            100.0 * g.on_good as f64 / g.detected.max(1) as f64,
            g.on_truth,
            100.0 * g.on_truth as f64 / g.n.max(1) as f64,
        );
        if !g.errors.is_empty() {
            let mut e = g.errors.clone();
            e.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            println!(
                "  corner error vs the picture (share of the diagonal): n {} median {:.4} p90 {:.4} max {:.4}",
                e.len(),
                e[e.len() / 2],
                e[(e.len() * 9 / 10).min(e.len() - 1)],
                e[e.len() - 1]
            );
        }
        tn += g.n;
        td += g.detected;
        ts += g.on_detected;
        tg += g.on_good;
        tt += g.on_truth;
    }
    println!(
        "TOTAL {tn} pictures, {td} detected: Some on the detector's quad {ts} ({:.2}%), on Good ones {tg}, on the truth quad {tt} ({:.2}%)",
        100.0 * ts as f64 / td.max(1) as f64,
        100.0 * tt as f64 / tn.max(1) as f64
    );
}
