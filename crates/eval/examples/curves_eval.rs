// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Evaluation of the automatic curve fitter (`auto_crop_imgproc::curves_fit`) over a manifest.
//!
//! ```text
//! cargo run --release -p auto-crop-eval --example curves_eval -- MANIFEST \
//!     [--quad gt|detector] [--split dev|test|all] [--limit N] [--json OUT.json] [--by TAG,TAG]
//! ```
//!
//! For every image the fitter is run on a straight quad: the ground-truth quad (`--quad gt`, the
//! default: it isolates the fitter) or the quad the classical detector finds (`--quad detector`: the
//! end-to-end case, images without a detection are counted and skipped). Reported, over the images
//! whose ground truth has bent edges ("curved") and over those that have none ("flat"):
//!
//! * mean boundary distance between the fitted outline and the ground-truth outline, in pixels and
//!   as a percentage of the shorter side of the page (p50 and p95 too), next to the same figure for
//!   the straight input quad;
//! * IoU of the fitted outline and of the straight quad with the ground-truth outline (rasterised,
//!   so a curved outline is scored as it is);
//! * how often `looks_curved` fires (false positives on flat pages, recall on curved ones, by
//!   curvature bucket);
//! * milliseconds per fit (the whole call, single thread).
//!
//! Only aggregates are printed or written. Run it on private images only with output inside
//! `_data/`, and never publish the figures of a private set per image.

use auto_crop_eval::curve_fit_eval::{Row, evaluate, mean, quantile, summarise};
use auto_crop_eval::geom::P;
use auto_crop_eval::manifest::{self, ManifestItem};
use serde_json::json;
use std::collections::BTreeMap;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut manifest_path = None;
    let (mut quad_mode, mut split, mut limit, mut json_out) =
        ("gt".to_owned(), "all".to_owned(), usize::MAX, None);
    let mut ids: Vec<String> = Vec::new();
    let mut csv_out: Option<String> = None;
    let mut by: Vec<String> = vec!["bend".into(), "curvature".into(), "aspect".into()];
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--quad" => quad_mode = it.next().cloned().unwrap_or_default(),
            "--split" => split = it.next().cloned().unwrap_or_default(),
            "--limit" => limit = it.next().and_then(|s| s.parse().ok()).unwrap_or(usize::MAX),
            "--json" => json_out = it.next().cloned(),
            "--csv" => csv_out = it.next().cloned(),
            "--ids" => {
                ids = it
                    .next()
                    .map(|s| s.split([',']).map(str::to_owned).collect())
                    .unwrap_or_default()
            }
            "--by" => {
                by = it
                    .next()
                    .map(|s| s.split(',').map(str::to_owned).collect())
                    .unwrap_or_default();
            }
            other => manifest_path = Some(other.to_owned()),
        }
    }
    let Some(manifest_path) = manifest_path else {
        eprintln!(
            "usage: curves_eval MANIFEST [--quad gt|detector] [--split S] [--limit N] [--json OUT] [--by TAGS]"
        );
        std::process::exit(2);
    };
    let mut m = manifest::load(std::path::Path::new(&manifest_path)).expect("manifest");
    if split != "all" {
        m = m.filter_split(&split);
    }
    let use_detector = quad_mode == "detector";
    let items: Vec<ManifestItem> = m
        .items
        .iter()
        .filter(|i| ids.is_empty() || ids.contains(&i.id))
        .take(limit)
        .cloned()
        .collect();
    let rows: Vec<Row> = items
        .iter()
        .map(|it| {
            let bytes = std::fs::read(m.resolve(it)).expect("read image");
            let raster = auto_crop_codecs::decode(&bytes).expect("decode").raster;
            evaluate(
                &raster,
                &it.id,
                &it.tags,
                &it.quad,
                it.curves.as_ref(),
                use_detector,
                std::env::var_os("CURVES_DUMP").is_some(),
            )
        })
        .collect();
    let skipped = rows.iter().filter(|r| r.skipped).count();
    println!(
        "manifest {} images ({} {}), quad = {quad_mode}, {skipped} without a detection",
        rows.len(),
        split,
        if rows.iter().any(|r| r.curved) {
            "with curved pages"
        } else {
            "flat"
        }
    );
    let mut out = vec![];
    let all: Vec<&Row> = rows.iter().collect();
    let curved: Vec<&Row> = rows.iter().filter(|r| r.curved).collect();
    let flat: Vec<&Row> = rows.iter().filter(|r| !r.curved).collect();
    out.push(summarise("all", &all));
    if !curved.is_empty() {
        out.push(summarise("curved", &curved));
    }
    if !flat.is_empty() {
        out.push(summarise("flat", &flat));
        if use_detector {
            let good: Vec<&Row> = flat.iter().copied().filter(|r| r.det_iou >= 0.9).collect();
            out.push(summarise("flat, detector IoU >= 0.9", &good));
        }
    }
    for tag in &by {
        let mut groups: BTreeMap<String, Vec<&Row>> = BTreeMap::new();
        for r in &curved {
            if let Some(v) = r.tags.get(tag) {
                groups.entry(v.clone()).or_default().push(r);
            }
        }
        for (v, g) in groups {
            if g.len() >= 8 {
                out.push(summarise(&format!("curved {tag}={v}"), &g));
            }
        }
    }
    // looks_curved against the ground-truth curvature of the page, in buckets.
    let edges = [0.0, 0.3, 0.6, 1.0, 1.5, 2.5, 4.0, 100.0];
    for k in 0..edges.len() - 1 {
        let g: Vec<&Row> = rows
            .iter()
            .filter(|r| !r.skipped && r.gt_curv >= edges[k] && r.gt_curv < edges[k + 1])
            .collect();
        if !g.is_empty() {
            let looks = g.iter().filter(|r| r.looks_curved).count();
            println!(
                "ground-truth curvature {:>4.1}-{:<5.1}% n={:<4} looks_curved {:>5.1}%  mean fitted max curvature {:.2}%",
                edges[k],
                edges[k + 1],
                g.len(),
                100.0 * looks as f64 / g.len() as f64,
                g.iter().map(|r| r.max_curv).sum::<f64>() / g.len() as f64,
            );
        }
    }
    let mut bias: Vec<f64> = rows
        .iter()
        .filter(|r| !r.skipped)
        .map(|r| r.bias_px)
        .collect();
    println!(
        "signed boundary bias px (positive = fitted outline outside the truth): mean {:.2} p50 {:.2} p5 {:.2} p95 {:.2}",
        mean(&bias),
        quantile(&mut bias, 0.5),
        quantile(&mut bias, 0.05),
        quantile(&mut bias, 0.95)
    );
    let confident_flat = flat.iter().filter(|r| !r.skipped).count();
    println!("flat pages used: {confident_flat}");
    if let Some(dir) = std::env::var_os("CURVES_DUMP") {
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).expect("dump dir");
        let mut order: Vec<&Row> = rows.iter().filter(|r| r.overlay.is_some()).collect();
        let key = |r: &Row| {
            if std::env::var_os("CURVES_DUMP_ERR").is_some() {
                r.bd_fit_px / r.short_px
            } else {
                r.max_curv
            }
        };
        order.sort_by(|a, b| {
            key(b)
                .partial_cmp(&key(a))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        for r in order.iter().take(24) {
            let (img, gt, fit, q) = r.overlay.as_ref().expect("overlay");
            let mut o = auto_crop_imgproc::scale::resize_area(img, img.width * 2, img.height * 2);
            let draw = |o: &mut auto_crop_imgproc::Raster, pts: &[P], c: [u8; 3]| {
                for i in 0..pts.len() {
                    let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
                    let n = ((b[0] - a[0]).abs().max((b[1] - a[1]).abs()) * 2.0)
                        .ceil()
                        .max(1.0) as usize;
                    for k in 0..=n {
                        let t = k as f64 / n as f64;
                        let (x, y) = (
                            ((a[0] + (b[0] - a[0]) * t) * 2.0) as i64,
                            ((a[1] + (b[1] - a[1]) * t) * 2.0) as i64,
                        );
                        if x >= 0 && y >= 0 && (x as u32) < o.width && (y as u32) < o.height {
                            o.set_pixel(x as u32, y as u32, c);
                        }
                    }
                }
            };
            draw(&mut o, q, [0, 255, 0]);
            draw(&mut o, gt, [255, 255, 255]);
            draw(&mut o, fit, [255, 0, 0]);
            let bytes = auto_crop_codecs::encode(&o, auto_crop_codecs::Format::Png, 90, None)
                .expect("encode");
            std::fs::write(dir.join(format!("{:05.2}_{}.png", r.max_curv, r.id)), bytes)
                .expect("write");
        }
    }
    if let Some(p) = csv_out {
        let mut s = String::from(
            "id,bend,curvature_tag,aspect,gt_curv,curved,fell_back,bd_fit_px,bd_straight_px,short_px,iou_fit,iou_straight,c0,c1,c2,c3,k0,k1,k2,k3,b0,b1,b2,b3,g0,g1,g2,g3\n",
        );
        for r in rows.iter().filter(|r| !r.skipped) {
            let tag = |k: &str| r.tags.get(k).cloned().unwrap_or_default();
            s += &format!(
                "{},{},{},{},{:.3},{},{},{:.3},{:.3},{:.1},{:.4},{:.4},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.3},{:.3},{:.3},{:.3}\n",
                r.id,
                tag("bend"),
                tag("curvature"),
                tag("aspect"),
                r.gt_curv,
                r.curved,
                r.fell_back,
                r.bd_fit_px,
                r.bd_straight_px,
                r.short_px,
                r.iou_fit,
                r.iou_straight,
                r.curv[0],
                r.curv[1],
                r.curv[2],
                r.curv[3],
                r.conf[0],
                r.conf[1],
                r.conf[2],
                r.conf[3],
                r.bulge[0],
                r.bulge[1],
                r.bulge[2],
                r.bulge[3],
                r.gain[0],
                r.gain[1],
                r.gain[2],
                r.gain[3]
            );
        }
        std::fs::write(p, s).expect("csv");
    }
    if let Some(p) = json_out {
        let text = serde_json::to_string_pretty(
            &json!({"quad": quad_mode, "split": split, "groups": out}),
        )
        .expect("json");
        std::fs::write(p, text).expect("write json");
    }
}
