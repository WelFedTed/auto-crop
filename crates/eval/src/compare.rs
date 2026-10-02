// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `compare`: the paired regression gate (ROADMAP M1.47, M1.50; PLAN 7.8).
//!
//! Two results over the same manifest are paired by image id. The gate fails when the mean IoU
//! drops by 0.3 points or more, or the failure rate rises by 0.5 points or more (one image is
//! 0.5 points at n = 200, so a single new failure on the smoke set blocks). Slices with n >= 80 are
//! gated the same way; smaller slices are listed and never block. A waiver turns a failure into a
//! recorded override, never into a pass.

use crate::report::{ImageResult, Results, SliceStatus};
use crate::stats::{BOOTSTRAP_RESAMPLES, BOOTSTRAP_SEED, MIN_GATE_N, bootstrap_mean_ci, mean};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 0.3 points of mean IoU (IoU is a fraction, so points are hundredths).
pub const MAX_MEAN_IOU_DROP: f64 = 0.003;
/// 0.5 points of failure rate.
pub const MAX_FAILURE_RISE: f64 = 0.005;
/// Floating-point slack so "exactly one image at n = 200" lands on the failing side.
const EPS: f64 = 1e-9;

#[derive(Debug, Clone)]
pub struct CompareConfig {
    pub max_mean_iou_drop: f64,
    pub max_failure_rise: f64,
    pub min_gate_n: usize,
    /// The `accuracy-waiver` label was set.
    pub waiver: bool,
}

impl Default for CompareConfig {
    fn default() -> Self {
        Self {
            max_mean_iou_drop: MAX_MEAN_IOU_DROP,
            max_failure_rise: MAX_FAILURE_RISE,
            min_gate_n: MIN_GATE_N,
            waiver: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Pass,
    Fail,
    /// Would have failed; overridden by the waiver label.
    Waived,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Delta {
    pub base: f64,
    pub head: f64,
    pub delta: f64,
}

impl Delta {
    fn new(base: f64, head: f64) -> Self {
        Self {
            base,
            head,
            delta: head - base,
        }
    }
}

/// An image whose pass/fail state differs between the two runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Changed {
    pub id: String,
    pub base_iou: f64,
    pub head_iou: f64,
    pub base_failed: bool,
    pub head_failed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SliceDelta {
    pub key: String,
    pub n: usize,
    pub status: SliceStatus,
    pub gated: bool,
    pub mean_iou: Delta,
    pub failure_rate: Delta,
    pub regressed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Comparison {
    pub verdict: Verdict,
    pub reasons: Vec<String>,
    pub n_paired: usize,
    pub base_commit: String,
    pub head_commit: String,
    pub mean_iou: Delta,
    /// 95% bootstrap interval of the paired mean IoU difference (head - base).
    pub mean_iou_delta_ci95: Option<[f64; 2]>,
    pub failure_rate: Delta,
    pub newly_failing: Vec<Changed>,
    pub newly_passing: Vec<Changed>,
    pub slices: Vec<SliceDelta>,
}

fn by_id(r: &Results) -> BTreeMap<&str, &ImageResult> {
    r.images.iter().map(|i| (i.id.as_str(), i)).collect()
}

/// Compares `head` against `base`. Errors when the two are not over the same images, because an
/// unpaired comparison would not be a regression test.
pub fn compare(base: &Results, head: &Results, cfg: &CompareConfig) -> Result<Comparison, String> {
    if base.header.manifest_sha256 != head.header.manifest_sha256 {
        return Err(format!(
            "results are over different manifests ({} vs {})",
            &base.header.manifest_sha256[..base.header.manifest_sha256.len().min(12)],
            &head.header.manifest_sha256[..head.header.manifest_sha256.len().min(12)]
        ));
    }
    let (b, h) = (by_id(base), by_id(head));
    if b.len() != base.images.len() || h.len() != head.images.len() {
        return Err("duplicate image ids in a results file".to_owned());
    }
    if b.keys().ne(h.keys()) {
        let only_base = b.keys().filter(|k| !h.contains_key(*k)).count();
        let only_head = h.keys().filter(|k| !b.contains_key(*k)).count();
        return Err(format!(
            "results cover different images ({only_base} only in base, {only_head} only in head)"
        ));
    }
    if b.is_empty() {
        return Err("no images to compare".to_owned());
    }

    let paired: Vec<(&ImageResult, &ImageResult)> = b.iter().map(|(k, bi)| (*bi, h[k])).collect();
    let n = paired.len();
    let base_ious: Vec<f64> = paired.iter().map(|p| p.0.iou).collect();
    let head_ious: Vec<f64> = paired.iter().map(|p| p.1.iou).collect();
    let diffs: Vec<f64> = paired.iter().map(|p| p.1.iou - p.0.iou).collect();
    let fails = |side: usize| {
        paired
            .iter()
            .filter(|p| if side == 0 { p.0.failure } else { p.1.failure })
            .count() as f64
            / n as f64
    };

    let mean_iou = Delta::new(mean(&base_ious), mean(&head_ious));
    let failure_rate = Delta::new(fails(0), fails(1));
    let mut newly_failing = Vec::new();
    let mut newly_passing = Vec::new();
    for (bi, hi) in &paired {
        if bi.failure != hi.failure {
            let c = Changed {
                id: bi.id.clone(),
                base_iou: bi.iou,
                head_iou: hi.iou,
                base_failed: bi.failure,
                head_failed: hi.failure,
            };
            if hi.failure {
                newly_failing.push(c);
            } else {
                newly_passing.push(c);
            }
        }
    }

    let mut reasons = Vec::new();
    let breach = |d: &Delta, limit: f64, drop: bool| {
        if drop {
            d.delta <= -limit + EPS
        } else {
            d.delta >= limit - EPS
        }
    };
    if breach(&mean_iou, cfg.max_mean_iou_drop, true) {
        reasons.push(format!(
            "mean IoU fell {:.2} pt ({:.4} to {:.4}); limit {:.1} pt",
            -mean_iou.delta * 100.0,
            mean_iou.base,
            mean_iou.head,
            cfg.max_mean_iou_drop * 100.0
        ));
    }
    if breach(&failure_rate, cfg.max_failure_rise, false) {
        reasons.push(format!(
            "failure rate rose {:.2} pt ({:.2}% to {:.2}%); limit {:.1} pt",
            failure_rate.delta * 100.0,
            failure_rate.base * 100.0,
            failure_rate.head * 100.0,
            cfg.max_failure_rise * 100.0
        ));
    }

    let mut slices = Vec::new();
    for hs in &head.slices {
        let Some(bs) = base.slices.iter().find(|s| s.key == hs.key) else {
            continue;
        };
        let (Some(bm), Some(hm), Some(bf), Some(hf)) = (
            bs.summary.mean_iou,
            hs.summary.mean_iou,
            bs.summary.failure_rate,
            hs.summary.failure_rate,
        ) else {
            continue;
        };
        let (mi, fr) = (Delta::new(bm, hm), Delta::new(bf, hf));
        let gated = hs.n >= cfg.min_gate_n;
        let regressed =
            breach(&mi, cfg.max_mean_iou_drop, true) || breach(&fr, cfg.max_failure_rise, false);
        if gated && regressed {
            reasons.push(format!(
                "slice {} (n={}): mean IoU {:+.2} pt, failure rate {:+.2} pt",
                hs.key,
                hs.n,
                mi.delta * 100.0,
                fr.delta * 100.0
            ));
        }
        slices.push(SliceDelta {
            key: hs.key.clone(),
            n: hs.n,
            status: hs.status,
            gated,
            mean_iou: mi,
            failure_rate: fr,
            regressed,
        });
    }

    let verdict = match (reasons.is_empty(), cfg.waiver) {
        (true, _) => Verdict::Pass,
        (false, true) => Verdict::Waived,
        (false, false) => Verdict::Fail,
    };
    Ok(Comparison {
        verdict,
        reasons,
        n_paired: n,
        base_commit: base.header.commit.clone(),
        head_commit: head.header.commit.clone(),
        mean_iou,
        mean_iou_delta_ci95: bootstrap_mean_ci(&diffs, BOOTSTRAP_RESAMPLES, BOOTSTRAP_SEED, 0.95)
            .map(|(a, b)| [a, b]),
        failure_rate,
        newly_failing,
        newly_passing,
        slices,
    })
}

/// A short human-readable report (for the job log and a PR comment).
pub fn render_text(c: &Comparison) -> String {
    let mut s = String::new();
    let word = match c.verdict {
        Verdict::Pass => "PASS",
        Verdict::Fail => "FAIL",
        Verdict::Waived => "WAIVED (accuracy-waiver label)",
    };
    s.push_str(&format!(
        "accuracy compare: {word}\n  paired images: {}\n  mean IoU:      {:.4} -> {:.4} ({:+.2} pt)\n  failure rate:  {:.2}% -> {:.2}% ({:+.2} pt)\n",
        c.n_paired,
        c.mean_iou.base,
        c.mean_iou.head,
        c.mean_iou.delta * 100.0,
        c.failure_rate.base * 100.0,
        c.failure_rate.head * 100.0,
        c.failure_rate.delta * 100.0
    ));
    if let Some(ci) = c.mean_iou_delta_ci95 {
        s.push_str(&format!(
            "  paired mean IoU change 95% CI: [{:+.3}, {:+.3}] pt\n",
            ci[0] * 100.0,
            ci[1] * 100.0
        ));
    }
    for r in &c.reasons {
        s.push_str(&format!("  reason: {r}\n"));
    }
    for (label, list) in [
        ("newly failing", &c.newly_failing),
        ("newly passing", &c.newly_passing),
    ] {
        if !list.is_empty() {
            s.push_str(&format!("  {label} ({}):\n", list.len()));
            for ch in list.iter().take(20) {
                s.push_str(&format!(
                    "    {} IoU {:.3} -> {:.3}\n",
                    ch.id, ch.base_iou, ch.head_iou
                ));
            }
            if list.len() > 20 {
                s.push_str(&format!("    ... and {} more\n", list.len() - 20));
            }
        }
    }
    s.push_str("  slices (gated = n >= gate floor; others advisory):\n");
    for d in &c.slices {
        s.push_str(&format!(
            "    {:<24} n={:<5} IoU {:+.2} pt  fail {:+.2} pt  {}{}\n",
            d.key,
            d.n,
            d.mean_iou.delta * 100.0,
            d.failure_rate.delta * 100.0,
            if d.gated { "gated" } else { "advisory" },
            if d.regressed { "  REGRESSED" } else { "" }
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::predictor::{FullFrame, Jittered, Oracle};
    use crate::run::testing::parallelogram_manifest;
    use crate::run::{RunConfig, run};

    fn results(m: &crate::manifest::Manifest, p: &dyn crate::predictor::Predictor) -> Results {
        run(
            m,
            p,
            &RunConfig {
                threads: 2,
                commit: "c".to_owned(),
                ..RunConfig::default()
            },
        )
        .expect("runs")
    }

    #[test]
    fn identical_runs_pass() {
        let m = parallelogram_manifest(200, 11);
        let a = results(&m, &Jittered::from_manifest(&m, 0.02, 1));
        let b = results(&m, &Jittered::from_manifest(&m, 0.02, 1));
        let c = compare(&a, &b, &CompareConfig::default()).expect("comparable");
        assert_eq!(c.verdict, Verdict::Pass);
        assert!(c.reasons.is_empty() && c.newly_failing.is_empty() && c.newly_passing.is_empty());
        assert_eq!(c.mean_iou.delta, 0.0);
        assert_eq!(c.mean_iou_delta_ci95, Some([0.0, 0.0]));
    }

    #[test]
    fn a_jittered_head_fails_the_gate_mutation_test() {
        // M1.50's mutation test: jittered fails, identical passes.
        let m = parallelogram_manifest(200, 12);
        let base = results(&m, &Oracle::from_manifest(&m));
        let head = results(&m, &Jittered::from_manifest(&m, 0.01, 3));
        let c = compare(&base, &head, &CompareConfig::default()).expect("comparable");
        assert_eq!(c.verdict, Verdict::Fail);
        assert!(
            c.reasons.iter().any(|r| r.contains("mean IoU fell")),
            "{:?}",
            c.reasons
        );
        // A tiny jitter that costs well under 0.3 pt passes.
        let tiny = results(&m, &Jittered::from_manifest(&m, 0.0004, 3));
        assert_eq!(
            compare(&base, &tiny, &CompareConfig::default())
                .expect("comparable")
                .verdict,
            Verdict::Pass
        );
        // The waiver label overrides a failure and says so.
        let cfg = CompareConfig {
            waiver: true,
            ..CompareConfig::default()
        };
        let waived = compare(&base, &head, &cfg).expect("comparable");
        assert_eq!(waived.verdict, Verdict::Waived);
        assert!(!waived.reasons.is_empty(), "the reasons stay on record");
    }

    #[test]
    fn one_new_failure_in_200_images_is_exactly_the_failure_threshold() {
        let m = parallelogram_manifest(200, 13);
        let base = results(&m, &Oracle::from_manifest(&m));
        let mut head = base.clone();
        // Turn one image into a failure without moving the mean IoU past its own limit.
        head.images[7].iou = 0.89;
        head.images[7].failure = true;
        head.summary.failures = 1;
        head.summary.failure_rate = Some(1.0 / 200.0);
        head.summary.mean_iou = Some(head.images.iter().map(|i| i.iou).sum::<f64>() / 200.0);
        let c = compare(&base, &head, &CompareConfig::default()).expect("comparable");
        assert_eq!(c.verdict, Verdict::Fail);
        assert_eq!(c.newly_failing.len(), 1);
        assert_eq!(c.newly_failing[0].id, head.images[7].id);
        assert!(c.reasons.iter().any(|r| r.contains("failure rate rose")));
    }

    #[test]
    fn improvements_pass_and_are_listed() {
        let m = parallelogram_manifest(200, 14);
        let base = results(&m, &FullFrame);
        let head = results(&m, &Oracle::from_manifest(&m));
        let c = compare(&base, &head, &CompareConfig::default()).expect("comparable");
        assert_eq!(c.verdict, Verdict::Pass);
        assert!(c.mean_iou.delta > 0.0);
        assert!(!c.newly_passing.is_empty() && c.newly_failing.is_empty());
        let text = render_text(&c);
        assert!(text.contains("PASS") && text.contains("newly passing"));
    }

    #[test]
    fn only_slices_at_or_above_the_gate_floor_can_block() {
        let m = parallelogram_manifest(200, 15);
        let base = results(&m, &Oracle::from_manifest(&m));
        let head = results(&m, &Jittered::from_manifest(&m, 0.01, 3));
        // With the default floor of 80 no slice of a 200-image run with 2-3 values per axis is
        // below it except tilt thirds (67 images), which stay advisory.
        let c = compare(&base, &head, &CompareConfig::default()).expect("comparable");
        let tilt = c
            .slices
            .iter()
            .find(|s| s.key == "tilt=10-30")
            .expect("slice");
        assert!(!tilt.gated && tilt.regressed);
        let lighting = c
            .slices
            .iter()
            .find(|s| s.key == "lighting=dim")
            .expect("slice");
        assert!(lighting.gated);
        // A comparison whose only regression is in an advisory slice does not block: raise the
        // floor above every slice and lower the global limits out of the way.
        let cfg = CompareConfig {
            max_mean_iou_drop: 1.0,
            max_failure_rise: 1.0,
            min_gate_n: 1000,
            waiver: false,
        };
        assert_eq!(
            compare(&base, &head, &cfg).expect("comparable").verdict,
            Verdict::Pass
        );
    }

    #[test]
    fn mismatched_inputs_are_errors_not_passes() {
        let m = parallelogram_manifest(60, 16);
        let other = parallelogram_manifest(60, 17);
        let a = results(&m, &FullFrame);
        let b = results(&other, &FullFrame);
        assert!(
            compare(&a, &b, &CompareConfig::default())
                .expect_err("manifests differ")
                .contains("different manifests")
        );
        let mut c = a.clone();
        c.images.pop();
        assert!(
            compare(&a, &c, &CompareConfig::default())
                .expect_err("images differ")
                .contains("different images")
        );
    }
}
