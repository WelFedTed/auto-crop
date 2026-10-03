// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Multi-item metrics and the `run --multi` path (ROADMAP M10.56, PLAN 4.15 G4).
//!
//! A scene holds several items; a predictor returns a set of quads and says whether it would
//! auto-accept the scan. Per scan: the item count (exact or not), how many predicted items match
//! a ground-truth item at IoU 0.9 (greedy matching by descending IoU), and whether the scan was
//! auto-accepted. Aggregated: item recall and precision (micro, over items), the exact-count
//! rate, the **silent wrong split** (an auto-accepted scan whose item set is wrong: another count
//! or an item below IoU 0.9) with its one-sided 95% Clopper-Pearson bound, and the routing rate
//! (touching or overlapping scans that were held). Slices follow the harness rules: n < 30 is
//! suppressed, 30 to 79 advisory, 80 and up gated.
//!
//! This is the metric core of the multi-item path and, like `geom`, `metrics` and `stats`, it
//! imports no project crate: it must not share code with the detector it measures (M1.46). The
//! predictor adapter that runs the real detector is in [`crate::detector`].
//!
//! Ground-truth items are clipped to the frame before matching (a clipped item is scored on its
//! visible part), and so are predictions. IoU is affine-invariant, so it is computed in
//! normalised coordinates.

use crate::geom::{self, P, Quad};
use crate::manifest::{Manifest, ManifestItem};
use crate::predictor::{PredictError, PredictInput};
use crate::report::SliceStatus;
use crate::stats::{clopper_pearson_upper, wilson_95};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};

pub const MULTI_SCHEMA: &str = "auto-crop-eval-multi-results/1";
/// An item matches a ground-truth item at this IoU (the silent-failure line of PLAN 4.9).
pub const MATCH_IOU: f64 = 0.9;

#[derive(Debug, Clone, PartialEq)]
pub struct MultiPrediction {
    /// Every item the predictor found, TL, TR, BR, BL in 0..1 EXIF-oriented coordinates.
    pub items: Vec<Quad>,
    /// The predictor would write this scan without asking.
    pub accepted: bool,
    /// Hold reason codes (scan-level and item-level), for the report.
    pub reasons: Vec<String>,
    pub confidence: Option<f64>,
}

pub trait MultiPredictor: Send + Sync {
    fn name(&self) -> String;
    fn predict(&self, input: &PredictInput) -> Result<MultiPrediction, PredictError>;
}

/// Returns the ground truth and accepts everything: scores perfectly, or the harness is wrong.
pub struct MultiOracle {
    gt: BTreeMap<String, Vec<Quad>>,
}

impl MultiOracle {
    pub fn from_manifest(m: &Manifest) -> Self {
        Self {
            gt: m
                .items
                .iter()
                .map(|i| (i.id.clone(), i.item_quads()))
                .collect(),
        }
    }
}

impl MultiPredictor for MultiOracle {
    fn name(&self) -> String {
        "multi-oracle".to_owned()
    }

    fn predict(&self, input: &PredictInput) -> Result<MultiPrediction, PredictError> {
        let items = self.gt.get(&input.id).ok_or(PredictError::Missing)?.clone();
        Ok(MultiPrediction {
            items,
            accepted: true,
            reasons: Vec::new(),
            confidence: Some(1.0),
        })
    }
}

fn clipped_to_frame(q: &Quad) -> Vec<P> {
    let frame: [P; 4] = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    geom::clip_convex(q, &frame)
}

/// IoU of two convex polygons (either winding), 0 when degenerate.
pub fn polygon_iou(a: &[P], b: &[P]) -> f64 {
    if a.len() < 3 || b.len() < 3 {
        return 0.0;
    }
    let (aa, ab) = (geom::area(a), geom::area(b));
    if aa <= 0.0 || ab <= 0.0 {
        return 0.0;
    }
    let inter = geom::clip_convex(a, b);
    let ai = if inter.len() >= 3 {
        geom::area(&inter)
    } else {
        0.0
    };
    ai / (aa + ab - ai)
}

/// Greedy matching by descending IoU between ground truth and predictions; returns the IoUs of
/// the matched pairs at or above `thr` (each ground-truth item and each prediction used once).
pub fn match_items(gt: &[Quad], pred: &[Quad], thr: f64) -> Vec<f64> {
    let g: Vec<Vec<P>> = gt.iter().map(clipped_to_frame).collect();
    let p: Vec<Vec<P>> = pred.iter().map(clipped_to_frame).collect();
    let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
    for (i, gi) in g.iter().enumerate() {
        for (j, pj) in p.iter().enumerate() {
            let iou = polygon_iou(gi, pj);
            if iou >= thr {
                pairs.push((iou, i, j));
            }
        }
    }
    pairs.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let (mut gu, mut pu) = (vec![false; g.len()], vec![false; p.len()]);
    let mut out = Vec::new();
    for (iou, i, j) in pairs {
        if !gu[i] && !pu[j] {
            gu[i] = true;
            pu[j] = true;
            out.push(iou);
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanStatus {
    Ok,
    Missing,
    Crashed,
}

/// One scored scan. Local results only: never a publishable artefact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScanResult {
    pub id: String,
    pub status: ScanStatus,
    pub n_gt: usize,
    pub n_pred: usize,
    pub matched: usize,
    pub exact_count: bool,
    pub accepted: bool,
    /// Auto-accepted and the item set is wrong (other count, or an item under IoU 0.9).
    pub silent_wrong: bool,
    pub mean_matched_iou: Option<f64>,
    /// For each ground-truth item, the best IoU of any prediction with it (diagnostics, 4 decimals).
    #[serde(default)]
    pub gt_best_iou: Vec<f64>,
    pub confidence: Option<f64>,
    pub reasons: Vec<String>,
    pub tags: BTreeMap<String, String>,
    pub note: Option<String>,
}

/// The best IoU any prediction reaches with each ground-truth item (clipped to the frame).
pub fn best_ious(gt: &[Quad], pred: &[Quad]) -> Vec<f64> {
    let p: Vec<Vec<P>> = pred.iter().map(clipped_to_frame).collect();
    gt.iter()
        .map(|g| {
            let g = clipped_to_frame(g);
            let b = p.iter().map(|q| polygon_iou(&g, q)).fold(0.0, f64::max);
            (b * 1e4).round() / 1e4
        })
        .collect()
}

pub fn score_scan(it: &ManifestItem, pred: &MultiPrediction) -> ScanResult {
    let gt = it.item_quads();
    let m = match_items(&gt, &pred.items, MATCH_IOU);
    let matched = m.len();
    let exact = pred.items.len() == gt.len();
    let wrong = !(exact && matched == gt.len());
    ScanResult {
        id: it.id.clone(),
        status: ScanStatus::Ok,
        n_gt: gt.len(),
        n_pred: pred.items.len(),
        matched,
        exact_count: exact,
        accepted: pred.accepted,
        silent_wrong: pred.accepted && wrong,
        mean_matched_iou: (!m.is_empty()).then(|| m.iter().sum::<f64>() / m.len() as f64),
        gt_best_iou: best_ious(&gt, &pred.items),
        confidence: pred
            .confidence
            .filter(|c| c.is_finite())
            .map(|c| c.clamp(0.0, 1.0)),
        reasons: pred.reasons.clone(),
        tags: it.tags.clone(),
        note: None,
    }
}

fn failed_scan(it: &ManifestItem, status: ScanStatus, note: Option<String>) -> ScanResult {
    ScanResult {
        id: it.id.clone(),
        status,
        n_gt: it.item_quads().len(),
        n_pred: 0,
        matched: 0,
        exact_count: false,
        accepted: false,
        silent_wrong: false,
        mean_matched_iou: None,
        gt_best_iou: Vec::new(),
        confidence: None,
        reasons: Vec::new(),
        tags: it.tags.clone(),
        note,
    }
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    let s = payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic".to_owned());
    s.chars().take(200).collect()
}

/// Scores one scan; a panicking or failing predictor becomes a scan with no items (not accepted).
pub fn evaluate_scan(m: &Manifest, it: &ManifestItem, p: &dyn MultiPredictor) -> ScanResult {
    let input = PredictInput::of(m, it);
    match catch_unwind(AssertUnwindSafe(|| p.predict(&input))) {
        Err(payload) => failed_scan(it, ScanStatus::Crashed, Some(panic_text(payload.as_ref()))),
        Ok(Err(PredictError::Missing)) => failed_scan(it, ScanStatus::Missing, None),
        Ok(Err(PredictError::Failed(msg))) => failed_scan(
            it,
            ScanStatus::Crashed,
            Some(msg.chars().take(200).collect()),
        ),
        Ok(Ok(pred)) => score_scan(it, &pred),
    }
}

/// Counts and rates over a set of scans.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MultiSummary {
    pub scans: usize,
    pub items_gt: usize,
    pub items_pred: usize,
    pub items_matched: usize,
    pub exact_count_scans: usize,
    /// Share of scans with exactly the right number of items.
    pub exact_count_rate: Option<f64>,
    /// Matched over ground-truth items (at IoU 0.9).
    pub item_recall: Option<f64>,
    /// Matched over predicted items.
    pub item_precision: Option<f64>,
    pub mean_matched_iou: Option<f64>,
    /// Scans where every item matched and no extra item was predicted.
    pub perfect_scans: usize,
    pub accepted: usize,
    pub accept_rate: Option<f64>,
    pub silent_wrong: usize,
    /// Silent wrong splits over auto-accepted scans, and its one-sided 95% bound.
    pub silent_wrong_risk: Option<f64>,
    pub silent_wrong_ub95: Option<f64>,
    /// Scans that were not auto-accepted although the item set was exactly right.
    pub held_but_right: usize,
    pub crashed: usize,
    pub missing: usize,
    pub with_cluster_hold: usize,
}

fn rate(k: usize, n: usize) -> Option<f64> {
    (n > 0).then(|| k as f64 / n as f64)
}

pub fn summarise(rows: &[&ScanResult]) -> MultiSummary {
    let n = rows.len();
    let sum = |f: &dyn Fn(&ScanResult) -> usize| rows.iter().map(|r| f(r)).sum::<usize>();
    let items_gt = sum(&|r| r.n_gt);
    let items_pred = sum(&|r| r.n_pred);
    let items_matched = sum(&|r| r.matched);
    let accepted = rows.iter().filter(|r| r.accepted).count();
    let silent = rows.iter().filter(|r| r.silent_wrong).count();
    let ious: Vec<f64> = rows
        .iter()
        .filter_map(|r| r.mean_matched_iou.map(|m| m * r.matched as f64))
        .collect();
    let perfect = |r: &ScanResult| r.exact_count && r.matched == r.n_gt;
    MultiSummary {
        scans: n,
        items_gt,
        items_pred,
        items_matched,
        exact_count_scans: rows.iter().filter(|r| r.exact_count).count(),
        exact_count_rate: rate(rows.iter().filter(|r| r.exact_count).count(), n),
        item_recall: rate(items_matched, items_gt),
        item_precision: rate(items_matched, items_pred),
        mean_matched_iou: (items_matched > 0)
            .then(|| ious.iter().sum::<f64>() / items_matched as f64),
        perfect_scans: rows.iter().filter(|r| perfect(r)).count(),
        accepted,
        accept_rate: rate(accepted, n),
        silent_wrong: silent,
        silent_wrong_risk: rate(silent, accepted),
        silent_wrong_ub95: (accepted > 0)
            .then(|| clopper_pearson_upper(silent as u64, accepted as u64, 0.05)),
        held_but_right: rows.iter().filter(|r| !r.accepted && perfect(r)).count(),
        crashed: rows
            .iter()
            .filter(|r| r.status == ScanStatus::Crashed)
            .count(),
        missing: rows
            .iter()
            .filter(|r| r.status == ScanStatus::Missing)
            .count(),
        with_cluster_hold: rows
            .iter()
            .filter(|r| {
                r.reasons
                    .iter()
                    .any(|c| c == "TOUCHING_ITEMS" || c == "OVERLAPPING_ITEMS")
            })
            .count(),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MultiSlice {
    pub key: String,
    pub axis: String,
    pub value: String,
    pub n: usize,
    pub status: SliceStatus,
    pub summary: MultiSummary,
    /// Share of scans in the slice that were not auto-accepted (the routing rate for the
    /// touching and overlap slices).
    pub held_rate: Option<f64>,
}

pub fn slices(rows: &[ScanResult]) -> Vec<MultiSlice> {
    let mut by: BTreeMap<(String, String), Vec<&ScanResult>> = BTreeMap::new();
    for r in rows {
        for (a, v) in &r.tags {
            by.entry((a.clone(), v.clone())).or_default().push(r);
        }
    }
    by.into_iter()
        .map(|((axis, value), rs)| {
            let summary = summarise(&rs);
            MultiSlice {
                key: format!("{axis}={value}"),
                n: rs.len(),
                status: SliceStatus::of(rs.len()),
                held_rate: rate(rs.iter().filter(|r| !r.accepted).count(), rs.len()),
                axis,
                value,
                summary,
            }
        })
        .collect()
}

/// Touching and overlapping scans together: how many were held (the G4 routing number).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Routing {
    pub n: usize,
    pub held: usize,
    pub rate: Option<f64>,
    /// Wilson 95% interval of the rate.
    pub wilson95: Option<[f64; 2]>,
}

pub fn routing(rows: &[ScanResult]) -> Routing {
    let hard: Vec<&ScanResult> = rows
        .iter()
        .filter(|r| {
            matches!(
                r.tags.get("separation").map(String::as_str),
                Some("touching" | "overlap")
            )
        })
        .collect();
    let held = hard.iter().filter(|r| !r.accepted).count();
    let w = (!hard.is_empty()).then(|| wilson_95(held as u64, hard.len() as u64));
    Routing {
        n: hard.len(),
        held,
        rate: rate(held, hard.len()),
        wilson95: w.map(|(a, b)| [a, b]),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MultiHeader {
    pub schema: String,
    pub eval_version: String,
    pub commit: String,
    pub os: String,
    pub arch: String,
    pub suite: String,
    pub split: String,
    pub predictor: String,
    pub manifest_sha256: String,
    pub match_iou: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MultiResults {
    pub header: MultiHeader,
    pub summary: MultiSummary,
    pub routing: Routing,
    pub slices: Vec<MultiSlice>,
    pub scans: Vec<ScanResult>,
}

#[derive(Debug, Clone)]
pub struct MultiRunConfig {
    pub threads: usize,
    pub commit: String,
    pub suite: String,
    pub split: String,
}

/// Runs a multi-item predictor over every scan (sorted by id, parallel, aggregated sequentially so
/// the output is byte-identical at any thread count).
pub fn run(
    m: &Manifest,
    p: &dyn MultiPredictor,
    cfg: &MultiRunConfig,
) -> Result<MultiResults, String> {
    let mut items: Vec<&ManifestItem> = m.items.iter().collect();
    items.sort_by(|a, b| a.id.cmp(&b.id));
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(cfg.threads)
        .build()
        .map_err(|e| format!("cannot build thread pool: {e}"))?;
    let scans: Vec<ScanResult> =
        pool.install(|| items.par_iter().map(|it| evaluate_scan(m, it, p)).collect());
    let refs: Vec<&ScanResult> = scans.iter().collect();
    Ok(MultiResults {
        header: MultiHeader {
            schema: MULTI_SCHEMA.to_owned(),
            eval_version: env!("CARGO_PKG_VERSION").to_owned(),
            commit: cfg.commit.clone(),
            os: std::env::consts::OS.to_owned(),
            arch: std::env::consts::ARCH.to_owned(),
            suite: cfg.suite.clone(),
            split: cfg.split.clone(),
            predictor: p.name(),
            manifest_sha256: m.sha256.clone(),
            match_iou: MATCH_IOU,
        },
        summary: summarise(&refs),
        routing: routing(&scans),
        slices: slices(&scans),
        scans,
    })
}

pub fn to_json(r: &MultiResults) -> String {
    let mut s = serde_json::to_string_pretty(r).expect("results serialise");
    s.push('\n');
    s
}

fn pct(v: Option<f64>) -> String {
    v.map_or_else(|| "n/a".to_owned(), |x| format!("{:.2}%", x * 100.0))
}

fn line(label: &str, s: &MultiSummary) -> String {
    format!(
        "{label:<28} n={:<5} exact {:>7} recall {:>7} precision {:>7} accepted {:>7} silent {}/{} (bound {})\n",
        s.scans,
        pct(s.exact_count_rate),
        pct(s.item_recall),
        pct(s.item_precision),
        pct(s.accept_rate),
        s.silent_wrong,
        s.accepted,
        pct(s.silent_wrong_ub95),
    )
}

/// Terminal summary: aggregates only (no ids), safe to paste into a log. The G4 lines are
/// reported for information; nothing here claims a gate is met.
pub fn summary_text(r: &MultiResults) -> String {
    let h = &r.header;
    let mut o = format!(
        "predictor {} | suite {} ({}) | commit {} | host {}-{} | match IoU {}\n",
        h.predictor, h.suite, h.split, h.commit, h.os, h.arch, h.match_iou
    );
    o.push_str(&line("all scans", &r.summary));
    let s = &r.summary;
    o.push_str(&format!(
        "items: {} ground truth, {} predicted, {} matched; mean matched IoU {}; perfect scans {}; held although right {}; crashed {} missing {}\n",
        s.items_gt,
        s.items_pred,
        s.items_matched,
        s.mean_matched_iou.map_or_else(|| "n/a".to_owned(), |v| format!("{v:.4}")),
        s.perfect_scans,
        s.held_but_right,
        s.crashed,
        s.missing
    ));
    o.push_str(&format!(
        "touching or overlapping scans routed to review: {}/{} = {}{}\n",
        r.routing.held,
        r.routing.n,
        pct(r.routing.rate),
        r.routing.wilson95.map_or_else(String::new, |w| format!(
            " (Wilson 95% [{:.1}%, {:.1}%])",
            w[0] * 100.0,
            w[1] * 100.0
        )),
    ));
    o.push_str("slices (n<30 suppressed, 30-79 advisory):\n");
    for sl in &r.slices {
        let tag = match sl.status {
            SliceStatus::Suppressed => {
                continue;
            }
            SliceStatus::Advisory => " [advisory]",
            SliceStatus::Gated => "",
        };
        o.push_str(&line(&format!("  {}{tag}", sl.key), &sl.summary));
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::parse;
    use std::path::Path;

    fn sq(x: f64, y: f64, w: f64, h: f64) -> Quad {
        [[x, y], [x + w, y], [x + w, y + h], [x, y + h]]
    }

    fn manifest(n: usize) -> Manifest {
        let mut text = String::new();
        for i in 0..n {
            let items = vec![
                sq(0.05, 0.05, 0.4, 0.4),
                sq(0.55, 0.1, 0.35, 0.3),
                sq(0.2, 0.55, 0.5, 0.35),
            ];
            let it = ManifestItem {
                v: 1,
                id: format!("m{i:04}"),
                image: format!("images/m{i:04}.jpg"),
                scene_id: format!("s{i:04}"),
                split: Some(if i % 10 < 3 { "dev" } else { "test" }.to_owned()),
                width: 400,
                height: 300,
                quad: items[0],
                items,
                tags: [
                    (
                        "separation".to_owned(),
                        ["separated", "touching", "overlap"][i % 3].to_owned(),
                    ),
                    ("bed".to_owned(), ["white", "black"][i % 2].to_owned()),
                ]
                .into_iter()
                .collect(),
            };
            text.push_str(&serde_json::to_string(&it).expect("serialises"));
            text.push('\n');
        }
        parse(&text, Path::new(".")).expect("valid")
    }

    fn cfg(threads: usize) -> MultiRunConfig {
        MultiRunConfig {
            threads,
            commit: "t".to_owned(),
            suite: "unit".to_owned(),
            split: "all".to_owned(),
        }
    }

    #[test]
    fn the_oracle_is_perfect() {
        let m = manifest(90);
        let r = run(&m, &MultiOracle::from_manifest(&m), &cfg(2)).expect("runs");
        let s = &r.summary;
        assert_eq!((s.scans, s.perfect_scans, s.silent_wrong), (90, 90, 0));
        assert_eq!(s.exact_count_rate, Some(1.0));
        assert_eq!((s.item_recall, s.item_precision), (Some(1.0), Some(1.0)));
        assert!(s.mean_matched_iou.expect("matched") > 0.999999);
        // Everything accepted: no touching or overlapping scan was routed to review.
        assert_eq!(r.routing.n, 60);
        assert_eq!(r.routing.held, 0);
    }

    #[test]
    fn matching_is_one_to_one_and_needs_iou_90() {
        let gt = [sq(0.0, 0.0, 0.4, 0.4), sq(0.5, 0.5, 0.4, 0.4)];
        // One prediction covering both halves of a ground-truth item twice must match once.
        let pred = [sq(0.0, 0.0, 0.4, 0.4), sq(0.0, 0.0, 0.4, 0.4)];
        assert_eq!(match_items(&gt, &pred, 0.9).len(), 1);
        // 5% too small on each side is IoU 0.81: no match.
        let small = [sq(0.02, 0.02, 0.36, 0.36)];
        assert!(match_items(&gt, &small, 0.9).is_empty());
        let near = [sq(0.004, 0.004, 0.392, 0.392)];
        assert_eq!(match_items(&gt, &near, 0.9).len(), 1);
    }

    #[test]
    fn a_clipped_ground_truth_item_is_scored_on_its_visible_part() {
        let gt = [sq(0.8, 0.2, 0.4, 0.3)]; // half outside the frame
        let visible = [sq(0.8, 0.2, 0.2, 0.3)];
        assert_eq!(match_items(&gt, &visible, 0.9).len(), 1);
    }

    #[test]
    fn silent_wrong_splits_need_acceptance_and_a_wrong_set() {
        let m = manifest(1);
        let it = &m.items[0];
        let right = MultiPrediction {
            items: it.item_quads(),
            accepted: true,
            reasons: vec![],
            confidence: None,
        };
        assert!(!score_scan(it, &right).silent_wrong);
        let mut merged = right.clone();
        merged.items = vec![sq(0.05, 0.05, 0.85, 0.85)];
        let r = score_scan(it, &merged);
        assert!(r.silent_wrong && !r.exact_count && r.matched == 0);
        merged.accepted = false;
        assert!(!score_scan(it, &merged).silent_wrong);
        let mut extra = right.clone();
        extra.items.push(sq(0.9, 0.9, 0.05, 0.05));
        let r = score_scan(it, &extra);
        assert!(r.silent_wrong && r.matched == 3 && !r.exact_count);
    }

    #[test]
    fn results_are_byte_identical_at_one_and_eight_threads_and_crashes_are_counted() {
        struct Flaky(MultiOracle);
        impl MultiPredictor for Flaky {
            fn name(&self) -> String {
                "flaky".to_owned()
            }
            fn predict(&self, input: &PredictInput) -> Result<MultiPrediction, PredictError> {
                if input.id.ends_with('7') {
                    panic!("planted multi predictor crash");
                }
                if input.id.ends_with('3') {
                    return Err(PredictError::Missing);
                }
                self.0.predict(input)
            }
        }
        let m = manifest(100);
        let p = Flaky(MultiOracle::from_manifest(&m));
        let default = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let a = to_json(&run(&m, &p, &cfg(1)).expect("runs"));
        let b = to_json(&run(&m, &p, &cfg(8)).expect("runs"));
        std::panic::set_hook(default);
        assert_eq!(a, b);
        let r: MultiResults = serde_json::from_str(&a).expect("parses");
        assert_eq!((r.summary.crashed, r.summary.missing), (10, 10));
        assert!(
            r.scans
                .iter()
                .filter(|s| s.status != ScanStatus::Ok)
                .all(|s| !s.accepted)
        );
    }

    #[test]
    fn slices_follow_the_n_rules_and_text_hides_small_ones() {
        let m = manifest(200);
        let r = run(&m, &MultiOracle::from_manifest(&m), &cfg(2)).expect("runs");
        let sep = r
            .slices
            .iter()
            .find(|s| s.key == "separation=separated")
            .expect("slice");
        assert_eq!((sep.n, sep.status), (67, SliceStatus::Advisory));
        let bed = r
            .slices
            .iter()
            .find(|s| s.key == "bed=white")
            .expect("slice");
        assert_eq!(bed.status, SliceStatus::Gated);
        let text = summary_text(&r);
        assert!(text.contains("separation=separated") && text.contains("[advisory]"));
        let small = manifest(20);
        let r = run(&small, &MultiOracle::from_manifest(&small), &cfg(1)).expect("runs");
        assert!(!summary_text(&r).contains("separation=separated"));
    }
}
