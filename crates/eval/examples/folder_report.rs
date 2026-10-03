// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Local-only contact sheet: runs the classical detector over every image in a folder and writes a
//! preview PNG with the found quad drawn on it, a `results.json` and an `index.html`.
//!
//! ```text
//! cargo run --release -p auto-crop-eval --example folder_report -- _data _data/_out
//! ```
//!
//! The output folder must stay private (the owner's `_data/` is gitignored, B21): this tool never
//! uploads anything and the report names files, so do not publish it. No ground truth is used, so
//! there is no accuracy number here, only what the detector found and how sure it was.

use auto_crop_codecs::{Format, decode, encode};
use auto_crop_imgproc::Raster;
use auto_crop_imgproc::detect::detect;
use auto_crop_imgproc::scale::resize_to_fit;
use std::fmt::Write as _;
use std::path::Path;

const PREVIEW_EDGE: u32 = 900;

fn put(r: &mut Raster, x: i64, y: i64, c: [u8; 3]) {
    if x >= 0 && y >= 0 && (x as u32) < r.width && (y as u32) < r.height {
        let i = ((y as u32 * r.width + x as u32) * 3) as usize;
        r.data[i..i + 3].copy_from_slice(&c);
    }
}

fn line(r: &mut Raster, a: (f64, f64), b: (f64, f64), c: [u8; 3], thick: i64) {
    let n = ((b.0 - a.0).abs().max((b.1 - a.1).abs()).ceil() as usize).max(1) * 2;
    for i in 0..=n {
        let t = i as f64 / n as f64;
        let (x, y) = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
        for dy in -thick..=thick {
            for dx in -thick..=thick {
                put(r, x as i64 + dx, y as i64 + dy, c);
            }
        }
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: folder_report <input dir> <output dir>");
        std::process::exit(2);
    }
    let (input, out) = (Path::new(&args[1]), Path::new(&args[2]));
    std::fs::create_dir_all(out).expect("create output dir");
    let mut entries: Vec<_> = std::fs::read_dir(input)
        .expect("read input dir")
        .filter_map(Result::ok)
        .filter(|e| e.path().is_file())
        .collect();
    entries.sort_by_key(|e| e.file_name());

    let mut rows: Vec<serde_json::Value> = Vec::new();
    let mut html = String::from(
        "<!doctype html><meta charset=utf-8><title>Detector on _data</title>\
         <style>body{font:14px system-ui;margin:16px;background:#222;color:#eee}\
         .g{display:grid;grid-template-columns:repeat(auto-fill,minmax(300px,1fr));gap:12px}\
         .c{background:#333;padding:8px;border-radius:6px}img{width:100%}\
         .Good{border-left:6px solid #3c3}.Check{border-left:6px solid #fb0}.Failed{border-left:6px solid #e44}\
         .Skipped{border-left:6px solid #888}small{color:#aaa}</style><h2>Detector on _data</h2>\
         <p>Green = Good, amber = Check (held for review), red = Failed. Local only; no ground truth.</p><div class=g>",
    );
    for (n, e) in entries.iter().enumerate() {
        let path = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(err) => {
                rows.push(serde_json::json!({"file": name, "verdict": "Skipped", "note": format!("read: {err}")}));
                continue;
            }
        };
        let decoded = match decode(&bytes) {
            Ok(d) => d,
            Err(err) => {
                let note = format!("{err}");
                let _ = write!(
                    html,
                    "<div class='c Skipped'><b>{}</b><br><small>not decoded: {}</small></div>",
                    html_escape(&name),
                    html_escape(&note)
                );
                rows.push(serde_json::json!({"file": name, "verdict": "Skipped", "note": note}));
                continue;
            }
        };
        let t0 = std::time::Instant::now();
        let det = detect(&decoded.raster);
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        let score = f64::from(det.confidence.score);
        let verdict = match (det.quad.is_some(), det.confidence.forced) {
            (false, _) | (_, Some(auto_crop_core::Forced::Failed)) => "Failed",
            (_, Some(auto_crop_core::Forced::Check)) => "Check",
            _ if score >= 0.90 => "Good",
            _ if score >= 0.60 => "Check",
            _ => "Failed",
        };
        let colour = match verdict {
            "Good" => [60, 220, 60],
            "Check" => [255, 190, 0],
            _ => [235, 60, 60],
        };
        let mut preview = resize_to_fit(&decoded.raster, PREVIEW_EDGE);
        if let Some(q) = det.quad {
            let (w, h) = (f64::from(preview.width), f64::from(preview.height));
            let p: Vec<(f64, f64)> = q.iter().map(|p| (p.x * w, p.y * h)).collect();
            for i in 0..4 {
                line(&mut preview, p[i], p[(i + 1) % 4], colour, 2);
            }
            for pt in &p {
                line(&mut preview, *pt, *pt, [255, 255, 255], 5);
            }
        }
        let img_name = format!("{:03}.png", n + 1);
        let png = encode(&preview, Format::Png, 90, None).expect("encode preview");
        std::fs::write(out.join(&img_name), png).expect("write preview");
        let reasons: Vec<String> = det
            .confidence
            .reasons
            .iter()
            .map(|r| format!("{:?}", r.code))
            .collect();
        let _ = write!(
            html,
            "<div class='c {verdict}'><img src='{img_name}'><b>{}</b><br>{verdict} {score:.2} \
             <small>{}x{} {ms:.0} ms {}</small></div>",
            html_escape(&name),
            decoded.raster.width,
            decoded.raster.height,
            html_escape(&reasons.join(", "))
        );
        println!(
            "{verdict:6} {score:.2} {ms:5.0} ms  {name}  {}",
            reasons.join(",")
        );
        let quad = det
            .quad
            .map(|q| q.iter().map(|p| [p.x, p.y]).collect::<Vec<_>>());
        rows.push(serde_json::json!({
            "file": name, "verdict": verdict, "score": score, "ms": ms, "reasons": reasons, "quad": quad
        }));
    }
    html.push_str("</div>");
    std::fs::write(out.join("index.html"), html).expect("write index.html");
    std::fs::write(
        out.join("results.json"),
        serde_json::to_string_pretty(&rows).expect("serialise results"),
    )
    .expect("write results.json");
    println!("wrote {} entries to {}", rows.len(), out.display());
}
