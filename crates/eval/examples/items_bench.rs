// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Latency of `detect_items` per stage over the images of a manifest (decoded once beforehand).
//!
//! ```text
//! RAYON_NUM_THREADS=1 cargo run --release -p auto-crop-eval --example items_bench -- <manifest.jsonl> [images [passes]]
//! ```
//!
//! Prints the median and p95 total in milliseconds and the median time of each stage. Aggregates
//! only; run it on an otherwise idle machine.

use auto_crop_codecs::decode;
use auto_crop_imgproc::items::{ItemsOptions, detect_items_timed};
use std::path::Path;

fn pct(v: &[f64], p: f64) -> f64 {
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    s[((s.len() - 1) as f64 * p).round() as usize]
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let manifest = args
        .get(1)
        .expect("usage: items_bench <manifest.jsonl> [images [passes]]");
    let n: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(60);
    let passes: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(3);
    let base = Path::new(manifest).parent().unwrap_or(Path::new("."));
    let text = std::fs::read_to_string(manifest).expect("manifest");
    let rasters: Vec<_> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(n)
        .map(|l| {
            let row: serde_json::Value = serde_json::from_str(l).expect("row");
            let bytes =
                std::fs::read(base.join(row["image"].as_str().expect("image"))).expect("read");
            decode(&bytes).expect("decode").raster
        })
        .collect();
    let opts = ItemsOptions::default();
    let mut totals = Vec::new();
    let mut stages: Vec<(&'static str, Vec<f64>)> = Vec::new();
    for _ in 0..passes {
        for r in &rasters {
            let mut tm = Vec::new();
            let _ = detect_items_timed(r, &opts, &mut tm);
            totals.push(tm.last().map_or(0.0, |t| t.1));
            let mut prev = 0.0;
            for (i, (name, t)) in tm.iter().enumerate() {
                if stages.len() <= i {
                    stages.push((name, Vec::new()));
                }
                stages[i].1.push(t - prev);
                prev = *t;
            }
        }
    }
    let (w, h) = (rasters[0].width, rasters[0].height);
    println!(
        "{} images of about {w}x{h}, {passes} passes: median {:.1} ms, p95 {:.1} ms (to the end of the residual stage)",
        rasters.len(),
        pct(&totals, 0.5),
        pct(&totals, 0.95)
    );
    for (name, v) in &stages {
        println!("  {name:<10} median {:6.1} ms", pct(v, 0.5));
    }
}
