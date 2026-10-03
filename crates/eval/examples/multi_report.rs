// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Local-only contact sheet of the multi-item detector.
//!
//! ```text
//! cargo run --release -p auto-crop-eval --example multi_report -- <folder or manifest.jsonl> <out dir> [first [count]]
//! ```
//!
//! `MULTI_ONLY=id1,id2` restricts the run to those names.
//!
//! Every detected item is drawn on a preview (green = auto-accepted scan, amber = held, cyan
//! outline = a cluster); with a manifest the ground-truth items are drawn thin in red. Writes one
//! PNG per image, `index.html` and `results.json`. The output folder must stay private when the
//! input is the owner's `_data/`: nothing is uploaded, and the report names files.

use auto_crop_codecs::{Format, decode, encode};
use auto_crop_imgproc::Raster;
use auto_crop_imgproc::items::{ItemKind, ItemsOptions, detect_items};
use auto_crop_imgproc::scale::resize_to_fit;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

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

fn poly(r: &mut Raster, q: &[(f64, f64)], c: [u8; 3], thick: i64) {
    for i in 0..q.len() {
        line(r, q[i], q[(i + 1) % q.len()], c, thick);
    }
}

struct Entry {
    path: PathBuf,
    name: String,
    gt: Vec<Vec<(f64, f64)>>,
}

fn entries(input: &Path) -> Vec<Entry> {
    if input.extension().is_some_and(|e| e == "jsonl") {
        let base = input.parent().unwrap_or(Path::new("."));
        let text = std::fs::read_to_string(input).expect("read manifest");
        let mut v = Vec::new();
        for l in text.lines().filter(|l| !l.trim().is_empty()) {
            let row: serde_json::Value = serde_json::from_str(l).expect("manifest line");
            let quads = |val: &serde_json::Value| -> Vec<(f64, f64)> {
                val.as_array()
                    .map(|a| {
                        a.iter()
                            .map(|p| (p[0].as_f64().unwrap_or(0.0), p[1].as_f64().unwrap_or(0.0)))
                            .collect()
                    })
                    .unwrap_or_default()
            };
            let gt: Vec<Vec<(f64, f64)>> = match row.get("items").and_then(|i| i.as_array()) {
                Some(items) => items.iter().map(quads).collect(),
                None => vec![quads(&row["quad"])],
            };
            v.push(Entry {
                path: base.join(row["image"].as_str().unwrap_or_default()),
                name: row["id"].as_str().unwrap_or_default().to_owned(),
                gt,
            });
        }
        v
    } else {
        let mut e: Vec<_> = std::fs::read_dir(input)
            .expect("read input dir")
            .filter_map(Result::ok)
            .filter(|e| e.path().is_file())
            .collect();
        e.sort_by_key(|e| e.file_name());
        e.into_iter()
            .map(|e| Entry {
                name: e.file_name().to_string_lossy().into_owned(),
                path: e.path(),
                gt: Vec::new(),
            })
            .collect()
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: multi_report <folder or manifest.jsonl> <out dir> [first [count]]");
        std::process::exit(2);
    }
    let (input, out) = (Path::new(&args[1]), Path::new(&args[2]));
    let first: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
    let count: usize = args
        .get(4)
        .and_then(|s| s.parse().ok())
        .unwrap_or(usize::MAX);
    std::fs::create_dir_all(out).expect("create output dir");
    let mut html = String::from(
        "<!doctype html><meta charset=utf-8><title>Multi-item detector</title>\
         <style>body{font:14px system-ui;margin:16px;background:#222;color:#eee}\
         .g{display:grid;grid-template-columns:repeat(auto-fill,minmax(300px,1fr));gap:12px}\
         .c{background:#333;padding:8px;border-radius:6px}img{width:100%}\
         .acc{border-left:6px solid #3c3}.held{border-left:6px solid #fb0}small{color:#aaa}</style>\
         <h2>Multi-item detector</h2><p>Green = scan auto-accepted, amber = held, cyan = cluster, red thin = ground truth.</p><div class=g>",
    );
    let mut rows = Vec::new();
    let opts = ItemsOptions::default();
    let only: Vec<String> = std::env::var("MULTI_ONLY")
        .map(|v| v.split(',').map(str::to_owned).collect())
        .unwrap_or_default();
    for (n, e) in entries(input)
        .into_iter()
        .enumerate()
        .filter(|(_, e)| only.is_empty() || only.contains(&e.name))
        .skip(first)
        .take(count)
    {
        let Ok(bytes) = std::fs::read(&e.path) else {
            continue;
        };
        let Ok(decoded) = decode(&bytes) else {
            println!("{:3} {}: not decoded", n, e.name);
            continue;
        };
        let t0 = std::time::Instant::now();
        let det = detect_items(&decoded.raster, &opts);
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        let accepted = det.auto_accept(opts.good_cutoff);
        let mut preview = resize_to_fit(&decoded.raster, PREVIEW_EDGE);
        let (w, h) = (f64::from(preview.width), f64::from(preview.height));
        for g in &e.gt {
            let p: Vec<(f64, f64)> = g.iter().map(|p| (p.0 * w, p.1 * h)).collect();
            poly(&mut preview, &p, [235, 40, 40], 0);
        }
        for it in &det.items {
            let p: Vec<(f64, f64)> = it.quad.iter().map(|p| (p.x * w, p.y * h)).collect();
            let colour = match (it.kind, accepted, it.confidence.forced.is_some()) {
                (ItemKind::Cluster, _, _) => [0, 220, 230],
                (_, true, _) => [60, 220, 60],
                (_, false, true) => [255, 190, 0],
                _ => [200, 220, 60],
            };
            poly(&mut preview, &p, colour, 1);
        }
        let img_name = format!("{:03}.png", n + 1);
        std::fs::write(
            out.join(&img_name),
            encode(&preview, Format::Png, 90, None).expect("encode"),
        )
        .expect("write");
        let codes: Vec<String> = det
            .scan_confidence()
            .reasons
            .iter()
            .map(|r| format!("{:?}", r.code))
            .collect();
        let class = if accepted { "acc" } else { "held" };
        let _ = write!(
            html,
            "<div class='c {class}'><img src='{img_name}'><b>{}</b><br>{} items (truth {}) {} <small>{ms:.0} ms {}</small></div>",
            e.name,
            det.items.len(),
            e.gt.len(),
            if accepted { "ACCEPT" } else { "held" },
            codes.join(", ")
        );
        println!(
            "{:3} {:>24} items {} truth {} {} {ms:4.0} ms  bed_like={} noise={:.2} T={:.1} {}",
            n + 1,
            e.name.chars().take(24).collect::<String>(),
            det.items.len(),
            e.gt.len(),
            if accepted { "ACCEPT" } else { "held  " },
            det.scan_flags.bed_like,
            det.diagnostics.noise,
            det.diagnostics.edge_threshold,
            codes.join(",")
        );
        rows.push(serde_json::json!({
            "file": e.name, "items": det.items.len(), "truth": e.gt.len(), "accepted": accepted,
            "ms": ms, "reasons": codes,
            "quads": det.items.iter().map(|i| i.quad.iter().map(|p| [p.x, p.y]).collect::<Vec<_>>()).collect::<Vec<_>>(),
        }));
    }
    html.push_str("</div>");
    std::fs::write(out.join("index.html"), html).expect("write index.html");
    std::fs::write(
        out.join("results.json"),
        serde_json::to_string_pretty(&rows).expect("json"),
    )
    .expect("write results");
}
