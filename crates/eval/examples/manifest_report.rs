// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Local debugging aid: runs the classical detector over (a filtered part of) a manifest and writes
//! a contact sheet with the ground truth (white) and the detected quad (green = Good, amber =
//! Check, red = Failed) drawn on each tile, plus a text list in the same order.
//!
//! ```text
//! cargo run --release -p auto-crop-eval --example manifest_report -- \
//!     target/synth/smoke/manifest.jsonl target/report aspect=long framing=full --failures --limit 24
//! ```
//!
//! Arguments after the output directory: `key=value` tag filters, `--failures` (only IoU < 0.9),
//! `--silent` (only auto-accepted failures), `--limit N`, `--skip N`, `--cols N`, `--tile PIXELS`.
//! Output stays in the directory you name; nothing is uploaded.

use auto_crop_codecs::{Format, decode, encode};
use auto_crop_core::Forced;
use auto_crop_eval::manifest;
use auto_crop_eval::metrics::canonical_iou;
use auto_crop_imgproc::Raster;
use auto_crop_imgproc::detect::{detect, trace, trace_lines};
use auto_crop_imgproc::scale::resize_to_fit;
use std::path::Path;

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

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: manifest_report <manifest.jsonl> <out dir> [key=value ...] [flags]");
        std::process::exit(2);
    }
    let m = manifest::load(Path::new(&args[0])).expect("manifest");
    let out = Path::new(&args[1]);
    std::fs::create_dir_all(out).expect("out dir");
    let (mut filters, mut failures, mut silent, mut lines, mut sides) =
        (Vec::new(), false, false, false, false);
    let mut err = false;
    let (mut limit, mut skip, mut cols, mut tile) = (24usize, 0usize, 6usize, 300u32);
    let mut it = args[2..].iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--failures" => failures = true,
            "--silent" => silent = true,
            "--lines" => lines = true,
            "--sides" => sides = true,
            "--err" => err = true,
            "--limit" => limit = it.next().and_then(|v| v.parse().ok()).unwrap_or(limit),
            "--skip" => skip = it.next().and_then(|v| v.parse().ok()).unwrap_or(skip),
            "--cols" => cols = it.next().and_then(|v| v.parse().ok()).unwrap_or(cols),
            "--tile" => tile = it.next().and_then(|v| v.parse().ok()).unwrap_or(tile),
            kv => {
                if let Some((k, v)) = kv.split_once('=') {
                    filters.push((k.to_owned(), v.to_owned()));
                }
            }
        }
    }
    let mut tiles: Vec<Raster> = Vec::new();
    let mut listing = String::new();
    let mut seen = 0usize;
    let (mut total, mut nfail, mut in_list, mut in_list75) = (0usize, 0usize, 0usize, 0usize);
    for item in &m.items {
        if !filters.iter().all(|(k, v)| {
            if k == "id" {
                v.split(',').any(|x| x == item.id)
            } else {
                item.tags.get(k).is_some_and(|t| t == v)
            }
        }) {
            continue;
        }
        let bytes = std::fs::read(m.resolve(item)).expect("read image");
        let decoded = decode(&bytes).expect("decode");
        let det = detect(&decoded.raster);
        let score = f64::from(det.confidence.score);
        let state = match (det.quad.is_some(), det.confidence.forced) {
            (false, _) | (_, Some(Forced::Failed)) => "failed",
            (_, Some(Forced::Check)) => "check",
            _ if score >= 0.9 => "good",
            _ if score >= 0.6 => "check",
            _ => "failed",
        };
        let pred = det.quad.map(|q| std::array::from_fn(|i| [q[i].x, q[i].y]));
        let iou = pred
            .and_then(|p| canonical_iou(&item.quad, &p).ok())
            .unwrap_or(0.0);
        let fail = iou < 0.9;
        // The best candidate the detector had, whatever it ranked it: tells ranking problems from
        // candidate-generation problems.
        let cands = trace(&decoded.raster);
        let (mut best_c, mut best_rank) = (0.0f64, usize::MAX);
        for (r, c) in cands.iter().enumerate() {
            let q = std::array::from_fn(|i| [c.quad[i].x, c.quad[i].y]);
            if let Ok(v) = canonical_iou(&item.quad, &q)
                && v > best_c
            {
                (best_c, best_rank) = (v, r);
            }
        }
        if std::env::var("AC_CANDS").is_ok() {
            for (r, c) in cands.iter().enumerate() {
                let q = std::array::from_fn(|i| [c.quad[i].x, c.quad[i].y]);
                let v = canonical_iou(&item.quad, &q).unwrap_or(0.0);
                println!(
                    "   cand {r} score {:.2} forced {:?} rank {:.2} iou {v:.2}",
                    c.score, c.forced, c.rank
                );
            }
        }
        total += 1;
        if fail {
            nfail += 1;
            if best_c >= 0.9 {
                in_list += 1;
            }
            if best_c >= 0.75 {
                in_list75 += 1;
            }
        }
        if (failures && !fail) || (silent && !(fail && state == "good")) {
            continue;
        }
        seen += 1;
        if seen <= skip || tiles.len() >= limit {
            continue;
        }
        let mut pic = resize_to_fit(&decoded.raster, tile);
        let (w, h) = (f64::from(pic.width), f64::from(pic.height));
        let pts = |q: &[[f64; 2]; 4]| -> Vec<(f64, f64)> {
            q.iter().map(|p| (p[0] * w, p[1] * h)).collect()
        };
        if sides && fail {
            let (segs, _) = trace_lines(&decoded.raster);
            let (iw, ih) = (
                f64::from(decoded.raster.width),
                f64::from(decoded.raster.height),
            );
            let mut line_note = String::new();
            for k in 0..4 {
                let (a, b) = (item.quad[k], item.quad[(k + 1) % 4]);
                let (a, b) = ((a[0] * iw, a[1] * ih), (b[0] * iw, b[1] * ih));
                let ang = (b.1 - a.1).atan2(b.0 - a.0);
                let len = (b.0 - a.0).hypot(b.1 - a.1);
                let mut best = (f64::MAX, 0.0);
                for sg in &segs {
                    let (p, q) = ((sg[0].x * iw, sg[0].y * ih), (sg[1].x * iw, sg[1].y * ih));
                    let la = (q.1 - p.1).atan2(q.0 - p.0);
                    let mut d = (la - ang).rem_euclid(std::f64::consts::PI);
                    if d > std::f64::consts::FRAC_PI_2 {
                        d = std::f64::consts::PI - d;
                    }
                    // Distance of the side midpoint to the infinite line.
                    let m = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
                    let nx = -(q.1 - p.1);
                    let ny = q.0 - p.0;
                    let nn = nx.hypot(ny).max(1e-9);
                    let dist = ((m.0 - p.0) * nx + (m.1 - p.1) * ny).abs() / nn;
                    let cost = dist + d.to_degrees() * 1.5;
                    if d.to_degrees() < 6.0 && cost < best.0 {
                        best = (cost, d.to_degrees());
                        let _ = dist;
                    }
                }
                let _ = std::fmt::Write::write_fmt(
                    &mut line_note,
                    format_args!(
                        " side{k}(len {len:.0}): {}",
                        if best.0 < f64::MAX {
                            format!("line cost {:.1}", best.0)
                        } else {
                            "none".to_owned()
                        }
                    ),
                );
            }
            println!("    {} sides:{line_note}", item.id);
        }
        if lines {
            let (segs, edge_pts) = trace_lines(&decoded.raster);
            for p in &edge_pts {
                put(&mut pic, (p.x * w) as i64, (p.y * h) as i64, [255, 0, 255]);
            }
            for sg in &segs {
                line(
                    &mut pic,
                    (sg[0].x * w, sg[0].y * h),
                    (sg[1].x * w, sg[1].y * h),
                    [0, 200, 255],
                    0,
                );
            }
        }
        let t = pts(&item.quad);
        for i in 0..4 {
            line(&mut pic, t[i], t[(i + 1) % 4], [255, 255, 255], 1);
        }
        if let Some(p) = pred {
            let colour = match state {
                "good" => [40, 230, 40],
                "check" => [255, 190, 0],
                _ => [240, 50, 50],
            };
            let p = pts(&p);
            for i in 0..4 {
                line(&mut pic, p[i], p[(i + 1) % 4], colour, 0);
            }
        }
        if err && let Some(pq) = pred {
            let (iw, ih) = (
                f64::from(decoded.raster.width),
                f64::from(decoded.raster.height),
            );
            if (0.7..0.9).contains(&iou) {
                let g = |q: &[[f64; 2]; 4]| -> Vec<(f64, f64)> {
                    q.iter().map(|p| (p[0] * iw, p[1] * ih)).collect()
                };
                let (t, p) = (g(&item.quad), g(&pq));
                let mut note = String::new();
                for k in 0..4 {
                    let (a, b) = (t[k], t[(k + 1) % 4]);
                    let len = (b.0 - a.0).hypot(b.1 - a.1);
                    let ang = (b.1 - a.1).atan2(b.0 - a.0);
                    let m = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
                    let mut best = (f64::MAX, 0.0, 0.0);
                    for j in 0..4 {
                        let (c, d) = (p[j], p[(j + 1) % 4]);
                        let la = (d.1 - c.1).atan2(d.0 - c.0);
                        let mut da = (la - ang).rem_euclid(std::f64::consts::PI);
                        if da > std::f64::consts::FRAC_PI_2 {
                            da -= std::f64::consts::PI;
                        }
                        let nx = -(d.1 - c.1);
                        let ny = d.0 - c.0;
                        let nn = nx.hypot(ny).max(1e-9);
                        let off = ((m.0 - c.0) * nx + (m.1 - c.1) * ny) / nn;
                        let plen = (d.0 - c.0).hypot(d.1 - c.1);
                        let cost = off.abs() + da.abs().to_degrees() * 3.0;
                        if da.abs() < 0.6 && cost < best.0 {
                            best = (cost, off, plen / len);
                        }
                    }
                    note.push_str(&format!(
                        " [len {:.0} off {:+.1}px ratio {:.2}]",
                        len, best.1, best.2
                    ));
                }
                println!("    {} err:{note}", item.id);
            }
        }
        let area = |q: &[[f64; 2]; 4]| {
            let mut a = 0.0;
            for i in 0..4 {
                let (p, r) = (q[i], q[(i + 1) % 4]);
                a += p[0] * r[1] - r[0] * p[1];
            }
            a.abs() / 2.0
        };
        let area_ratio = pred.map_or(0.0, |p| area(&p) / area(&item.quad));
        listing.push_str(&format!(
            "{:>3} {} iou {:.3} area {:.2} cand {:.3}@{} {} {:.2} {:?} tags {:?}\n",
            tiles.len(),
            item.id,
            iou,
            area_ratio,
            best_c,
            best_rank as i64,
            state,
            score,
            det.confidence
                .reasons
                .iter()
                .map(|r| r.code)
                .collect::<Vec<_>>(),
            item.tags
                .iter()
                .filter(|(k, _)| matches!(
                    k.as_str(),
                    "aspect" | "background" | "framing" | "lighting" | "clutter"
                ))
                .map(|(_, v)| v.as_str())
                .collect::<Vec<_>>()
        ));
        tiles.push(pic);
    }
    let cols = cols.max(1).min(tiles.len().max(1));
    let rows = tiles.len().div_ceil(cols);
    let mut sheet = Raster::filled(
        cols as u32 * (tile + 4),
        rows as u32 * (tile + 4),
        [30, 30, 30],
    );
    for (n, t) in tiles.iter().enumerate() {
        let (ox, oy) = (
            (n % cols) as u32 * (tile + 4),
            (n / cols) as u32 * (tile + 4),
        );
        for y in 0..t.height {
            for x in 0..t.width {
                let i = ((y * t.width + x) * 3) as usize;
                put(
                    &mut sheet,
                    i64::from(ox + x),
                    i64::from(oy + y),
                    [t.data[i], t.data[i + 1], t.data[i + 2]],
                );
            }
        }
    }
    if !tiles.is_empty() {
        let png = encode(&sheet, Format::Png, 90, None).expect("encode");
        std::fs::write(out.join("sheet.png"), png).expect("write sheet");
    }
    std::fs::write(out.join("list.txt"), &listing).expect("write list");
    print!("{listing}");
    println!(
        "matched {total}, failures {nfail}; a candidate with IoU >= 0.90 existed for {in_list} of them, >= 0.75 for {in_list75}"
    );
}
