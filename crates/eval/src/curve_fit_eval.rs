// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Scoring of the automatic curve fitter (uto_crop_imgproc::curves_fit) against ground-truth page
//! outlines (boundary distance, IoU of rasterised outlines, looks_curved rates, time). Shared by the
//! examples curves_eval (a manifest) and curves_folder (a folder with golden labels). Only
//! aggregates are meant to leave the machine; a Row is per image and local.

use crate::curves::{Curves, boundary_samples, chamfer_distance, raster_iou};
use crate::geom::{P, Quad};
use auto_crop_core::Pt;
use auto_crop_imgproc::Raster;
use auto_crop_imgproc::curves_fit::fit_curves;
use auto_crop_imgproc::detect::detect;
use serde_json::json;
use std::collections::BTreeMap;
use std::time::Instant;

/// What a debug dump draws: the picture, the true outline, the fitted one and the input quad.
pub type Overlay = (auto_crop_imgproc::Raster, Vec<P>, Vec<P>, Vec<P>);

pub struct Row {
    pub tags: BTreeMap<String, String>,
    pub curved: bool,
    pub gt_curv: f64,
    pub skipped: bool,
    pub fell_back: bool,
    pub looks_curved: bool,
    pub bd_fit_px: f64,
    pub bd_straight_px: f64,
    pub short_px: f64,
    pub iou_fit: f64,
    pub iou_straight: f64,
    pub det_iou: f64,
    pub ms: f64,
    pub max_curv: f64,
    pub conf_min: f64,
    pub bias_px: f64,
    pub id: String,
    pub curv: [f32; 4],
    pub conf: [f32; 4],
    pub bulge: [f32; 4],
    pub gain: [f32; 4],
    pub overlay: Option<Overlay>,
}

fn px(q: &Quad, w: f64, h: f64) -> Vec<P> {
    q.iter().map(|p| [p[0] * w, p[1] * h]).collect()
}

fn scale(v: &[P], w: f64, h: f64) -> Vec<P> {
    v.iter().map(|p| [p[0] * w, p[1] * h]).collect()
}

fn edge_curvature(samples: &[P], per: usize, e: usize) -> f64 {
    let n = samples.len();
    let a = samples[e * per];
    let b = samples[(e * per + per) % n];
    let len = (b[0] - a[0]).hypot(b[1] - a[1]);
    if len < 1e-9 {
        return 0.0;
    }
    (e * per..e * per + per)
        .map(|i| {
            let p = samples[i];
            ((b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])).abs() / len
        })
        .fold(0.0, f64::max)
        / len
        * 100.0
}

/// Mean signed distance of the fitted samples from the true outline: positive when outside it.
fn signed_bias(fit: &[P], truth: &[P]) -> f64 {
    let inside = |x: f64, y: f64| -> bool {
        let mut c = false;
        let mut j = truth.len() - 1;
        for i in 0..truth.len() {
            let (pi, pj) = (truth[i], truth[j]);
            if (pi[1] > y) != (pj[1] > y)
                && x < (pj[0] - pi[0]) * (y - pi[1]) / (pj[1] - pi[1]) + pi[0]
            {
                c = !c;
            }
            j = i;
        }
        c
    };
    fit.iter()
        .map(|p| {
            let d = truth
                .iter()
                .map(|q| (p[0] - q[0]).hypot(p[1] - q[1]))
                .fold(f64::MAX, f64::min);
            if inside(p[0], p[1]) { -d } else { d }
        })
        .sum::<f64>()
        / fit.len() as f64
}

pub fn quantile(v: &mut [f64], q: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let pos = q * (v.len() - 1) as f64;
    let (lo, hi) = (pos.floor() as usize, pos.ceil() as usize);
    v[lo] + (v[hi] - v[lo]) * (pos - lo as f64)
}

pub fn mean(v: &[f64]) -> f64 {
    if v.is_empty() {
        f64::NAN
    } else {
        v.iter().sum::<f64>() / v.len() as f64
    }
}

/// Fits the curves of one image and scores them against the ground truth `gt_quad` + `gt_curves`.
/// The fitter is given the ground-truth quad, or the quad the detector finds when `use_detector`
/// (`Row::skipped` when it finds none).
pub fn evaluate(
    raster: &Raster,
    id: &str,
    tags: &BTreeMap<String, String>,
    gt_quad: &Quad,
    gt_curves: Option<&Curves>,
    use_detector: bool,
    keep_overlay: bool,
) -> Row {
    let (w, h) = (f64::from(raster.width), f64::from(raster.height));
    let gt_curves = gt_curves.cloned().unwrap_or_default();
    let curved = gt_curves.any_bent();
    let gt_samples = boundary_samples(gt_quad, Some(&gt_curves), 100);
    let gt_px = scale(&gt_samples, w, h);
    let gt_curv = (0..4)
        .map(|e| edge_curvature(&gt_px, 100, e))
        .fold(0.0, f64::max);
    let gtq = px(gt_quad, w, h);
    let short_px = {
        let d = |a: usize, b: usize| (gtq[a][0] - gtq[b][0]).hypot(gtq[a][1] - gtq[b][1]);
        ((d(0, 1) + d(2, 3)) / 2.0).min((d(1, 2) + d(3, 0)) / 2.0)
    };
    let mut row = Row {
        tags: tags.clone(),
        curved,
        gt_curv,
        skipped: false,
        fell_back: false,
        looks_curved: false,
        bd_fit_px: f64::NAN,
        bd_straight_px: f64::NAN,
        short_px,
        iou_fit: f64::NAN,
        iou_straight: f64::NAN,
        det_iou: f64::NAN,
        ms: 0.0,
        max_curv: 0.0,
        conf_min: 0.0,
        bias_px: f64::NAN,
        id: id.to_owned(),
        curv: [0.0; 4],
        conf: [0.0; 4],
        bulge: [0.0; 4],
        gain: [0.0; 4],
        overlay: None,
    };
    let quad: [Pt; 4] = if use_detector {
        match detect(raster).quad {
            Some(q) => q,
            None => {
                row.skipped = true;
                return row;
            }
        }
    } else {
        std::array::from_fn(|i| Pt::new(gt_quad[i][0], gt_quad[i][1]))
    };
    let t0 = Instant::now();
    let fit = fit_curves(raster, quad);
    row.ms = t0.elapsed().as_secs_f64() * 1e3;
    row.fell_back = fit.fell_back;
    row.looks_curved = fit.looks_curved;
    row.max_curv = f64::from(fit.max_curvature);
    row.curv = fit.curvature;
    row.conf = fit.confidence;
    row.bulge = fit.bulge;
    row.gain = fit.support_gain;
    row.conf_min = fit.confidence.iter().copied().fold(1.0f32, f32::min).into();

    let in_q: Quad = std::array::from_fn(|i| [quad[i].x, quad[i].y]);
    let to_curves = |c: &auto_crop_core::CurveWarp| -> Curves {
        let g = |cv: &auto_crop_core::Curve| Some(cv.points().iter().map(|p| [p.x, p.y]).collect());
        Curves {
            top: g(&c.top),
            right: g(&c.right),
            bottom: g(&c.bottom),
            left: g(&c.left),
        }
    };
    let fit_q: Quad = std::array::from_fn(|i| [fit.quad[i].x, fit.quad[i].y]);
    let fit_px = scale(
        &boundary_samples(&fit_q, Some(&to_curves(&fit.curves)), 100),
        w,
        h,
    );
    let straight_px = scale(&boundary_samples(&in_q, None, 100), w, h);
    row.bd_fit_px = chamfer_distance(&fit_px, &gt_px);
    row.bias_px = signed_bias(&fit_px, &gt_px);
    if keep_overlay {
        row.overlay = Some((
            raster.clone(),
            gt_px.clone(),
            fit_px.clone(),
            px(&in_q, w, h),
        ));
    }
    row.bd_straight_px = chamfer_distance(&straight_px, &gt_px);
    let (gw, gh) = (raster.width, raster.height);
    row.iou_fit = raster_iou(&fit_px, &gt_px, gw, gh);
    row.iou_straight = raster_iou(&straight_px, &gt_px, gw, gh);
    if use_detector {
        row.det_iou = row.iou_straight;
    }
    row
}

pub fn summarise(label: &str, rows: &[&Row]) -> serde_json::Value {
    let used: Vec<&&Row> = rows.iter().filter(|r| !r.skipped).collect();
    let n = used.len();
    if n == 0 {
        return json!({"label": label, "n": 0});
    }
    let pct = |f: fn(&Row) -> f64, s: fn(&Row) -> f64| -> Vec<f64> {
        used.iter().map(|r| 100.0 * f(r) / s(r)).collect()
    };
    let mut fit_pct = pct(|r| r.bd_fit_px, |r| r.short_px);
    let mut st_pct = pct(|r| r.bd_straight_px, |r| r.short_px);
    let mut fit_px: Vec<f64> = used.iter().map(|r| r.bd_fit_px).collect();
    let mut st_px: Vec<f64> = used.iter().map(|r| r.bd_straight_px).collect();
    let iou_fit: Vec<f64> = used.iter().map(|r| r.iou_fit).collect();
    let iou_st: Vec<f64> = used.iter().map(|r| r.iou_straight).collect();
    let mut ms: Vec<f64> = used.iter().map(|r| r.ms).collect();
    let looks = used.iter().filter(|r| r.looks_curved).count();
    let fell = used.iter().filter(|r| r.fell_back).count();
    let better = used
        .iter()
        .filter(|r| r.iou_fit > r.iou_straight + 1e-3)
        .count();
    let worse = used
        .iter()
        .filter(|r| r.iou_fit < r.iou_straight - 1e-3)
        .count();
    let v = json!({
        "label": label,
        "n": n,
        "boundary_px": {"fit_mean": mean(&fit_px), "fit_p50": quantile(&mut fit_px, 0.5), "fit_p95": quantile(&mut fit_px, 0.95),
                         "straight_mean": mean(&st_px), "straight_p50": quantile(&mut st_px, 0.5), "straight_p95": quantile(&mut st_px, 0.95)},
        "boundary_pct_short": {"fit_mean": mean(&fit_pct), "fit_p50": quantile(&mut fit_pct, 0.5), "fit_p95": quantile(&mut fit_pct, 0.95),
                         "straight_mean": mean(&st_pct), "straight_p50": quantile(&mut st_pct, 0.5), "straight_p95": quantile(&mut st_pct, 0.95)},
        "iou": {"fit_mean": mean(&iou_fit), "straight_mean": mean(&iou_st), "better": better, "worse": worse},
        "looks_curved": looks,
        "looks_curved_rate": looks as f64 / n as f64,
        "fell_back": fell,
        "ms": {"mean": mean(&ms), "p50": quantile(&mut ms, 0.5), "p95": quantile(&mut ms, 0.95)},
    });
    println!(
        "{label:<28} n={n:<5} boundary px mean {:.2} p50 {:.2} p95 {:.2} (straight {:.2}) | % short mean {:.2} p50 {:.2} p95 {:.2} (straight {:.2}) | IoU {:.4} vs straight {:.4} (better {better}, worse {worse}) | looks_curved {looks}/{n} ({:.1}%) fell_back {fell} | {:.1} ms (p95 {:.1})",
        mean(&fit_px),
        quantile(&mut fit_px, 0.5),
        quantile(&mut fit_px, 0.95),
        mean(&st_px),
        mean(&fit_pct),
        quantile(&mut fit_pct, 0.5),
        quantile(&mut fit_pct, 0.95),
        mean(&st_pct),
        mean(&iou_fit),
        mean(&iou_st),
        100.0 * looks as f64 / n as f64,
        mean(&ms),
        quantile(&mut ms, 0.95),
    );
    v
}
