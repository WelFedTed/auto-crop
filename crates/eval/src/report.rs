// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Result types and the aggregation of per-image rows into summaries, slices and calibration
//! (ROADMAP M1.46-M1.48). Everything here is a pure function of the rows, in row order, so a result
//! is byte-identical however the rows were produced.

use crate::calib::{self, Calibration};
use crate::metrics::{FAILURE_IOU, Invalid, Orientation, SUCCESS_IOU_95, SUCCESS_IOU_98};
use crate::predictor::Verdict;
use crate::stats::{
    BOOTSTRAP_RESAMPLES, BOOTSTRAP_SEED, MIN_GATE_N, MIN_PUBLIC_N, bootstrap_mean_ci,
    clopper_pearson_upper, mean, quantile_sorted,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const RESULTS_SCHEMA: &str = "auto-crop-eval-results/1";

/// Worst-case values that stand in for "no usable quad" in the corner-error and skew
/// distributions, so a predictor cannot improve its percentiles by failing to answer.
pub const MISSING_CORNER_ERR_PCT: f64 = 100.0;
pub const MISSING_SKEW_DEG: f64 = 90.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// A scorable quad.
    Ok,
    /// The predictor said there is no page (an honest Failed, still a failure here).
    NoQuad,
    /// No output for this image.
    Missing,
    /// A quad that cannot be scored (non-finite, bow-tie, beyond the horizon).
    Invalid,
    /// The predictor panicked or returned an error.
    Crashed,
}

/// One scored image. Local results only: this type never reaches a publishable artefact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageResult {
    pub id: String,
    pub status: Status,
    pub verdict: Option<Verdict>,
    pub confidence: Option<f64>,
    pub iou: f64,
    pub corner_err_pct: Option<f64>,
    pub skew_deg: Option<f64>,
    pub orientation: Option<Orientation>,
    pub invalid: Option<Invalid>,
    /// IoU below the failure line, or no scorable answer.
    pub failure: bool,
    /// Auto-accepted: the predictor answered with a quad and did not hold it for review.
    pub accepted: bool,
    pub tags: BTreeMap<String, String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Dist {
    pub p50: Option<f64>,
    pub p95: Option<f64>,
    pub p99: Option<f64>,
}

/// How the corners of geometrically right predictions (IoU at or above the failure line) are
/// listed: anything but `upright` is an orientation error the IoU cannot see.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OrientationCounts {
    pub upright: usize,
    pub rot90: usize,
    pub rot180: usize,
    pub rot270: usize,
    pub mirrored: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Accepted {
    /// Images the predictor auto-accepted.
    pub n: usize,
    /// Auto-accepted images that failed (silent failures).
    pub silent_failures: usize,
    /// Silent failures over auto-accepted images (the headline risk); `None` if none accepted.
    pub risk: Option<f64>,
    /// One-sided 95% Clopper-Pearson upper bound on `risk`.
    pub risk_ub95: Option<f64>,
    /// Silent failures over all images (always at most `risk`).
    pub silent_over_all: f64,
    /// Share of images held or failed rather than accepted.
    pub flag_rate: f64,
    /// Share of flagged images that were in fact fine.
    pub flagged_fine_share: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub n: usize,
    pub n_ok: usize,
    pub n_no_quad: usize,
    pub n_missing: usize,
    pub n_invalid: usize,
    pub n_crashed: usize,
    pub mean_iou: Option<f64>,
    /// 95% percentile-bootstrap interval of the mean IoU; only for `n >= 30`.
    pub mean_iou_ci95: Option<[f64; 2]>,
    pub failures: usize,
    pub failure_rate: Option<f64>,
    pub success_95: Option<f64>,
    pub success_98: Option<f64>,
    pub corner_err_pct: Dist,
    pub skew_deg: Dist,
    pub orientation: OrientationCounts,
    pub accepted: Accepted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SliceStatus {
    /// n below 30: never reported publicly.
    Suppressed,
    /// 30 <= n < 80: reported, never gated.
    Advisory,
    /// n of 80 or more.
    Gated,
}

impl SliceStatus {
    pub fn of(n: usize) -> Self {
        if n < MIN_PUBLIC_N {
            Self::Suppressed
        } else if n < MIN_GATE_N {
            Self::Advisory
        } else {
            Self::Gated
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SliceSummary {
    /// `axis=value`, for example `tilt=30-45`.
    pub key: String,
    pub axis: String,
    pub value: String,
    pub n: usize,
    pub status: SliceStatus,
    pub summary: Summary,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorstSlice {
    pub key: String,
    pub n: usize,
    pub status: SliceStatus,
    pub value: f64,
}

/// The weakest reportable slice (n >= 30) on each headline number.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Worst {
    pub by_mean_iou: Option<WorstSlice>,
    pub by_failure_rate: Option<WorstSlice>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Host {
    pub os: String,
    pub arch: String,
    pub tier: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Header {
    pub schema: String,
    pub eval_version: String,
    pub commit: String,
    pub host: Host,
    pub suite: String,
    pub split: String,
    pub predictor: String,
    pub manifest_sha256: String,
    pub failure_iou: f64,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Results {
    pub header: Header,
    pub summary: Summary,
    pub slices: Vec<SliceSummary>,
    pub worst_slice: Worst,
    /// `None` when the predictor reports no confidence.
    pub calibration: Option<Calibration>,
    pub images: Vec<ImageResult>,
}

fn dist(mut v: Vec<f64>) -> Dist {
    v.sort_by(f64::total_cmp);
    let q = |p: f64| (!v.is_empty()).then(|| quantile_sorted(&v, p));
    Dist {
        p50: q(0.50),
        p95: q(0.95),
        p99: q(0.99),
    }
}

fn rate(k: usize, n: usize) -> Option<f64> {
    (n > 0).then(|| k as f64 / n as f64)
}

/// Aggregates rows. `with_ci` adds the bootstrap interval (skipped below the public floor).
pub fn summarise(rows: &[&ImageResult], with_ci: bool) -> Summary {
    let n = rows.len();
    let count = |s: Status| rows.iter().filter(|r| r.status == s).count();
    let ious: Vec<f64> = rows.iter().map(|r| r.iou).collect();
    let failures = rows.iter().filter(|r| r.failure).count();
    let at_least = |t: f64| {
        rows.iter()
            .filter(|r| r.status == Status::Ok && r.iou >= t)
            .count()
    };

    let corner = rows
        .iter()
        .map(|r| r.corner_err_pct.unwrap_or(MISSING_CORNER_ERR_PCT))
        .collect();
    let skew = rows
        .iter()
        .map(|r| r.skew_deg.unwrap_or(MISSING_SKEW_DEG))
        .collect();

    let mut orientation = OrientationCounts::default();
    for r in rows.iter().filter(|r| r.status == Status::Ok && !r.failure) {
        match r.orientation {
            Some(Orientation::Upright) => orientation.upright += 1,
            Some(Orientation::Rot90) => orientation.rot90 += 1,
            Some(Orientation::Rot180) => orientation.rot180 += 1,
            Some(Orientation::Rot270) => orientation.rot270 += 1,
            Some(Orientation::Mirrored) => orientation.mirrored += 1,
            None => {}
        }
    }

    let accepted_n = rows.iter().filter(|r| r.accepted).count();
    let silent = rows.iter().filter(|r| r.accepted && r.failure).count();
    let flagged: Vec<&&ImageResult> = rows.iter().filter(|r| !r.accepted).collect();
    let flagged_fine = flagged.iter().filter(|r| !r.failure).count();
    let accepted = Accepted {
        n: accepted_n,
        silent_failures: silent,
        risk: rate(silent, accepted_n),
        risk_ub95: (accepted_n > 0)
            .then(|| clopper_pearson_upper(silent as u64, accepted_n as u64, 0.05)),
        silent_over_all: if n > 0 { silent as f64 / n as f64 } else { 0.0 },
        flag_rate: if n > 0 {
            flagged.len() as f64 / n as f64
        } else {
            0.0
        },
        flagged_fine_share: rate(flagged_fine, flagged.len()),
    };

    Summary {
        n,
        n_ok: count(Status::Ok),
        n_no_quad: count(Status::NoQuad),
        n_missing: count(Status::Missing),
        n_invalid: count(Status::Invalid),
        n_crashed: count(Status::Crashed),
        mean_iou: (n > 0).then(|| mean(&ious)),
        mean_iou_ci95: if with_ci && n >= MIN_PUBLIC_N {
            bootstrap_mean_ci(&ious, BOOTSTRAP_RESAMPLES, BOOTSTRAP_SEED, 0.95).map(|(a, b)| [a, b])
        } else {
            None
        },
        failures,
        failure_rate: rate(failures, n),
        success_95: rate(at_least(SUCCESS_IOU_95), n),
        success_98: rate(at_least(SUCCESS_IOU_98), n),
        corner_err_pct: dist(corner),
        skew_deg: dist(skew),
        orientation,
        accepted,
    }
}

/// Slices every image by every tag it carries, sorted by key.
pub fn slice(rows: &[ImageResult]) -> Vec<SliceSummary> {
    let mut groups: BTreeMap<(String, String), Vec<&ImageResult>> = BTreeMap::new();
    for r in rows {
        for (axis, value) in &r.tags {
            groups
                .entry((axis.clone(), value.clone()))
                .or_default()
                .push(r);
        }
    }
    groups
        .into_iter()
        .map(|((axis, value), members)| SliceSummary {
            key: format!("{axis}={value}"),
            n: members.len(),
            status: SliceStatus::of(members.len()),
            summary: summarise(&members, true),
            axis,
            value,
        })
        .collect()
}

/// The reportable slice (n >= 30) with the lowest mean IoU and the one with the highest failure
/// rate. Ties go to the earlier key.
pub fn worst_slice(slices: &[SliceSummary]) -> Worst {
    let reportable = || {
        slices
            .iter()
            .filter(|s| s.status != SliceStatus::Suppressed)
    };
    let pick = |value: &dyn Fn(&SliceSummary) -> Option<f64>, lowest: bool| {
        let mut best: Option<WorstSlice> = None;
        for s in reportable() {
            let Some(v) = value(s) else { continue };
            let better = best
                .as_ref()
                .is_none_or(|b| if lowest { v < b.value } else { v > b.value });
            if better {
                best = Some(WorstSlice {
                    key: s.key.clone(),
                    n: s.n,
                    status: s.status,
                    value: v,
                });
            }
        }
        best
    };
    Worst {
        by_mean_iou: pick(&|s| s.summary.mean_iou, true),
        by_failure_rate: pick(&|s| s.summary.failure_rate, false),
    }
}

/// Calibration over all images: a prediction with no confidence that was answered disables it;
/// images with no answer enter as confidence 0 and a failure.
pub fn calibration_of(rows: &[ImageResult]) -> Option<Calibration> {
    let answered =
        |r: &&ImageResult| matches!(r.status, Status::Ok | Status::Invalid | Status::NoQuad);
    if rows.is_empty() || rows.iter().filter(answered).any(|r| r.confidence.is_none()) {
        return None;
    }
    if !rows.iter().any(|r| r.confidence.is_some()) {
        return None;
    }
    let data: Vec<(f64, bool)> = rows
        .iter()
        .map(|r| (r.confidence.unwrap_or(0.0), !r.failure))
        .collect();
    Some(calib::calibration(&data))
}

fn pct(v: Option<f64>) -> String {
    v.map_or_else(|| "n/a".to_owned(), |x| format!("{:.2}%", x * 100.0))
}

fn num(v: Option<f64>, digits: usize) -> String {
    v.map_or_else(|| "n/a".to_owned(), |x| format!("{x:.digits$}"))
}

/// A human-readable summary for the terminal. Aggregates only: safe to paste into a log.
pub fn summary_text(r: &Results) -> String {
    let (h, s) = (&r.header, &r.summary);
    let mut o = String::new();
    o.push_str(&format!(
        "predictor {} | suite {} ({}) | commit {} | host {}-{}\n",
        h.predictor, h.suite, h.split, h.commit, h.host.os, h.host.arch
    ));
    o.push_str(&format!(
        "n={}  ok={} no_quad={} missing={} invalid={} crashed={}\n",
        s.n, s.n_ok, s.n_no_quad, s.n_missing, s.n_invalid, s.n_crashed
    ));
    let ci = s.mean_iou_ci95.map_or_else(String::new, |c| {
        format!("  95% CI [{:.4}, {:.4}]", c[0], c[1])
    });
    o.push_str(&format!("mean IoU {}{ci}\n", num(s.mean_iou, 4)));
    o.push_str(&format!(
        "failure (IoU<{FAILURE_IOU}) {} ({} images)  success>=0.95 {}  success>=0.98 {}\n",
        pct(s.failure_rate),
        s.failures,
        pct(s.success_95),
        pct(s.success_98)
    ));
    o.push_str(&format!(
        "corner error % diag  p50 {} p95 {} p99 {}\n",
        num(s.corner_err_pct.p50, 3),
        num(s.corner_err_pct.p95, 3),
        num(s.corner_err_pct.p99, 3)
    ));
    o.push_str(&format!(
        "skew deg             p50 {} p95 {} p99 {}\n",
        num(s.skew_deg.p50, 3),
        num(s.skew_deg.p95, 3),
        num(s.skew_deg.p99, 3)
    ));
    let a = &s.accepted;
    o.push_str(&format!(
        "auto-accepted {} ({} silent failures; risk {}, one-sided 95% bound {})  flag rate {}\n",
        a.n,
        a.silent_failures,
        pct(a.risk),
        pct(a.risk_ub95),
        pct(Some(a.flag_rate))
    ));
    if let Some(c) = &r.calibration {
        o.push_str(&format!(
            "calibration (reported, not gated): ECE {:.4}  Brier {:.4}  AUROC {}\n",
            c.ece,
            c.brier,
            num(c.auroc, 4)
        ));
    }
    o.push_str("slices (n<30 suppressed, 30-79 advisory):\n");
    for sl in &r.slices {
        o.push_str(&format!(
            "  {:<26} n={:<5} {:<10} IoU {}  fail {}\n",
            sl.key,
            sl.n,
            format!("{:?}", sl.status).to_lowercase(),
            num(sl.summary.mean_iou, 4),
            pct(sl.summary.failure_rate)
        ));
    }
    if let Some(w) = &r.worst_slice.by_mean_iou {
        o.push_str(&format!(
            "worst slice by mean IoU: {} (n={}, {:?}) {:.4}\n",
            w.key, w.n, w.status, w.value
        ));
    }
    if let Some(w) = &r.worst_slice.by_failure_rate {
        o.push_str(&format!(
            "worst slice by failure rate: {} (n={}, {:?}) {}\n",
            w.key,
            w.n,
            w.status,
            pct(Some(w.value))
        ));
    }
    for n in &h.notes {
        o.push_str(&format!("note: {n}\n"));
    }
    o
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    pub fn row(id: &str, iou: f64, tags: &[(&str, &str)]) -> ImageResult {
        ImageResult {
            id: id.to_owned(),
            status: Status::Ok,
            verdict: None,
            confidence: None,
            iou,
            corner_err_pct: Some((1.0 - iou) * 10.0),
            skew_deg: Some((1.0 - iou) * 5.0),
            orientation: Some(Orientation::Upright),
            invalid: None,
            failure: iou < FAILURE_IOU,
            accepted: true,
            tags: tags
                .iter()
                .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
                .collect(),
            note: None,
        }
    }

    pub fn failed_row(id: &str, status: Status) -> ImageResult {
        ImageResult {
            status,
            iou: 0.0,
            corner_err_pct: None,
            skew_deg: None,
            orientation: None,
            failure: true,
            accepted: false,
            ..row(id, 0.0, &[])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    #[test]
    fn summary_counts_rates_and_percentiles() {
        let mut rows: Vec<ImageResult> = (0..100)
            .map(|i| row(&format!("i{i}"), if i < 90 { 0.99 } else { 0.5 }, &[]))
            .collect();
        rows[0].iou = 0.96;
        let refs: Vec<&ImageResult> = rows.iter().collect();
        let s = summarise(&refs, true);
        assert_eq!(s.n, 100);
        assert_eq!(s.failures, 10);
        assert_eq!(s.failure_rate, Some(0.10));
        assert_eq!(s.success_95, Some(0.90));
        assert_eq!(s.success_98, Some(0.89));
        assert!(
            (s.mean_iou.expect("n > 0") - (0.96 + 89.0 * 0.99 + 10.0 * 0.5) / 100.0).abs() < 1e-12
        );
        let ci = s.mean_iou_ci95.expect("n >= 30");
        assert!(ci[0] < s.mean_iou.expect("n > 0") && s.mean_iou.expect("n > 0") < ci[1]);
        // All 100 are auto-accepted, 10 of them failed: risk 10%.
        assert_eq!(s.accepted.silent_failures, 10);
        assert_eq!(s.accepted.risk, Some(0.10));
        assert!(s.accepted.risk_ub95.expect("accepted > 0") > 0.10);
        assert_eq!(s.accepted.flag_rate, 0.0);
    }

    #[test]
    fn unanswered_images_are_failures_and_take_worst_case_percentiles() {
        let mut rows: Vec<ImageResult> = (0..40).map(|i| row(&format!("g{i}"), 1.0, &[])).collect();
        rows.push(failed_row("m", Status::Missing));
        rows.push(failed_row("c", Status::Crashed));
        let refs: Vec<&ImageResult> = rows.iter().collect();
        let s = summarise(&refs, false);
        assert_eq!((s.n_missing, s.n_crashed, s.failures), (1, 1, 2));
        assert_eq!(s.mean_iou_ci95, None, "no interval unless asked");
        // 2 of 42 are worst-case, so p99 is the penalty while p50 stays near zero.
        assert_eq!(s.corner_err_pct.p99, Some(MISSING_CORNER_ERR_PCT));
        assert_eq!(s.skew_deg.p99, Some(MISSING_SKEW_DEG));
        assert!(s.corner_err_pct.p50.expect("n > 0") < 1e-9);
        // The two non-answers are flagged, not silent; 40 accepted, all fine.
        assert_eq!(s.accepted.n, 40);
        assert_eq!(s.accepted.silent_failures, 0);
        assert!((s.accepted.flag_rate - 2.0 / 42.0).abs() < 1e-15);
        assert_eq!(s.accepted.flagged_fine_share, Some(0.0));
    }

    #[test]
    fn slices_carry_status_thresholds_and_the_worst_slice_ignores_tiny_ones() {
        let mut rows = Vec::new();
        for i in 0..100 {
            rows.push(row(&format!("a{i}"), 0.99, &[("lighting", "normal")]));
        }
        for i in 0..40 {
            rows.push(row(&format!("b{i}"), 0.93, &[("lighting", "dim")]));
        }
        for i in 0..5 {
            rows.push(row(&format!("c{i}"), 0.10, &[("lighting", "tiny")]));
        }
        let slices = slice(&rows);
        let by = |k: &str| slices.iter().find(|s| s.key == k).expect("slice exists");
        assert_eq!(by("lighting=normal").status, SliceStatus::Gated);
        assert_eq!(by("lighting=dim").status, SliceStatus::Advisory);
        assert_eq!(by("lighting=tiny").status, SliceStatus::Suppressed);
        assert_eq!(by("lighting=tiny").summary.mean_iou_ci95, None);
        assert!(by("lighting=dim").summary.mean_iou_ci95.is_some());
        let w = worst_slice(&slices);
        // The 5-image slice is far worse but is not reportable, so it can never be "worst".
        assert_eq!(w.by_mean_iou.expect("some slice").key, "lighting=dim");
        assert_eq!(w.by_failure_rate.expect("some slice").value, 0.0);
        assert!(slices.windows(2).all(|p| p[0].key < p[1].key));
    }

    #[test]
    fn slice_status_boundaries() {
        assert_eq!(SliceStatus::of(29), SliceStatus::Suppressed);
        assert_eq!(SliceStatus::of(30), SliceStatus::Advisory);
        assert_eq!(SliceStatus::of(79), SliceStatus::Advisory);
        assert_eq!(SliceStatus::of(80), SliceStatus::Gated);
    }

    #[test]
    fn calibration_needs_confidences_and_counts_missing_as_zero() {
        let mut rows: Vec<ImageResult> =
            (0..20).map(|i| row(&format!("i{i}"), 0.99, &[])).collect();
        assert!(calibration_of(&rows).is_none(), "no confidence anywhere");
        for r in &mut rows {
            r.confidence = Some(0.9);
        }
        rows.push(failed_row("m", Status::Missing));
        let c = calibration_of(&rows).expect("all answered rows have one");
        assert_eq!(c.n, 21);
        // One answered row losing its confidence disables the report.
        rows[0].confidence = None;
        assert!(calibration_of(&rows).is_none());
    }

    #[test]
    fn orientation_counts_only_geometrically_right_predictions() {
        let mut rows = [row("a", 0.99, &[]), row("b", 0.99, &[]), row("c", 0.2, &[])];
        rows[1].orientation = Some(Orientation::Rot180);
        rows[2].orientation = Some(Orientation::Rot90);
        let refs: Vec<&ImageResult> = rows.iter().collect();
        let o = summarise(&refs, false).orientation;
        assert_eq!((o.upright, o.rot180, o.rot90), (1, 1, 0));
    }
}
