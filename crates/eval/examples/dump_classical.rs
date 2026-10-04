// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Local aid for the learned-detector study (ADR 0010): runs the classical detector over every
//! image of a manifest and writes one JSON line per image with its answer (quad, confidence,
//! state, the same Good/Check/Failed mapping as the harness) and the best-ranked candidates it
//! scored, so a hybrid selector can be tried offline in Python without re-implementing the detector.
//!
//! ```text
//! cargo run --release -p auto-crop-eval --example dump_classical -- MANIFEST OUT.jsonl [CANDIDATES]
//! ```
//!
//! The file names no image path, only the manifest ids. When the manifest is the owner's private
//! labels (`_data/labels.jsonl`, B21) write the output inside `_data/` and publish aggregates only.

use auto_crop_core::Forced;
use auto_crop_eval::detector::{DEFAULT_GOOD_THRESHOLD, FAILED_BELOW};
use auto_crop_eval::manifest;
use auto_crop_imgproc::detect::{detect, trace};
use rayon::prelude::*;
use serde_json::json;
use std::path::Path;

fn quad_json(q: &[auto_crop_core::Pt; 4]) -> serde_json::Value {
    json!([
        [q[0].x, q[0].y],
        [q[1].x, q[1].y],
        [q[2].x, q[2].y],
        [q[3].x, q[3].y]
    ])
}

fn forced_name(f: Option<Forced>) -> &'static str {
    match f {
        None => "none",
        Some(Forced::Check) => "check",
        Some(Forced::Failed) => "failed",
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: dump_classical MANIFEST OUT.jsonl [CANDIDATES=8]");
        std::process::exit(2);
    }
    let keep: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(8);
    let m = manifest::load(Path::new(&args[1])).expect("manifest");
    let lines: Vec<String> = m
        .items
        .par_iter()
        .map(|it| {
            let path = m.resolve(it);
            let decoded = std::fs::read(&path)
                .ok()
                .and_then(|b| auto_crop_codecs::decode(&b).ok());
            let Some(decoded) = decoded else {
                return json!({"id": it.id, "quad": null, "state": "failed", "error": "decode"})
                    .to_string();
            };
            let t0 = std::time::Instant::now();
            let det = detect(&decoded.raster);
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            let score = f64::from(det.confidence.score);
            let state = match (det.quad.is_some(), det.confidence.forced) {
                (false, _) | (_, Some(Forced::Failed)) => "failed",
                (_, Some(Forced::Check)) => "check",
                _ if score >= DEFAULT_GOOD_THRESHOLD => "good",
                _ if score >= FAILED_BELOW => "check",
                _ => "failed",
            };
            let cands: Vec<serde_json::Value> = trace(&decoded.raster)
                .iter()
                .take(keep)
                .map(|c| {
                    json!({
                        "quad": quad_json(&c.quad),
                        "score": c.score,
                        "forced": forced_name(c.forced),
                        "rank": c.rank,
                    })
                })
                .collect();
            json!({
                "id": it.id,
                "quad": det.quad.as_ref().map(quad_json),
                "confidence": score.clamp(0.0, 1.0),
                "state": state,
                "ms": ms,
                "candidates": cands,
            })
            .to_string()
        })
        .collect();
    std::fs::write(&args[2], lines.join("\n") + "\n").expect("write output");
    eprintln!("wrote {} lines to {}", lines.len(), args[2]);
}
