// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `run`: a predictor over a manifest, scored into [`Results`] (ROADMAP M1.45, M1.49).
//!
//! Images are scored in parallel but collected in id order, and every aggregate is a sequential
//! function of those rows, so the serialised result is byte-identical at any thread count. Wall
//! times are deliberately not part of the result (they would break that); the CLI writes them to a
//! sidecar file.

use crate::manifest::{Manifest, ManifestItem};
use crate::metrics::{self, FAILURE_IOU};
use crate::predictor::{PredictError, PredictInput, Predictor, Verdict};
use crate::report::{
    Header, Host, ImageResult, RESULTS_SCHEMA, Results, Status, calibration_of, slice, summarise,
    worst_slice,
};
use rayon::prelude::*;
use std::panic::{AssertUnwindSafe, catch_unwind};

#[derive(Debug, Clone)]
pub struct RunConfig {
    /// Worker threads (0 = all cores).
    pub threads: usize,
    pub commit: String,
    pub suite: String,
    pub split: String,
    pub tier: Option<String>,
}

impl Default for RunConfig {
    fn default() -> Self {
        Self {
            threads: 0,
            commit: "unknown".to_owned(),
            suite: "adhoc".to_owned(),
            split: "all".to_owned(),
            tier: None,
        }
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

/// Scores one image. Never panics: a panicking predictor becomes a `Crashed` failure.
pub fn evaluate_one(m: &Manifest, it: &ManifestItem, predictor: &dyn Predictor) -> ImageResult {
    let input = PredictInput::of(m, it);
    let blank = |status: Status, note: Option<String>| ImageResult {
        id: it.id.clone(),
        status,
        verdict: None,
        confidence: None,
        iou: 0.0,
        corner_err_pct: None,
        skew_deg: None,
        orientation: None,
        invalid: None,
        failure: true,
        accepted: false,
        tags: it.tags.clone(),
        note,
    };
    let outcome = catch_unwind(AssertUnwindSafe(|| predictor.predict(&input)));
    let pred = match outcome {
        Err(payload) => return blank(Status::Crashed, Some(panic_text(payload.as_ref()))),
        Ok(Err(PredictError::Missing)) => return blank(Status::Missing, None),
        Ok(Err(PredictError::Failed(msg))) => {
            return blank(Status::Crashed, Some(msg.chars().take(200).collect()));
        }
        Ok(Ok(p)) => p,
    };
    let confidence = pred
        .confidence
        .filter(|c| c.is_finite())
        .map(|c| c.clamp(0.0, 1.0));
    let with_pred = |mut r: ImageResult| {
        r.verdict = pred.verdict;
        r.confidence = confidence;
        r
    };
    let Some(quad) = pred.quad else {
        return with_pred(blank(Status::NoQuad, None));
    };
    // Held (Check) and Failed results are flagged, not auto-accepted.
    let accepted = matches!(pred.verdict, None | Some(Verdict::Good));
    match metrics::score(&it.quad, &quad, it.width, it.height) {
        Err(inv) => {
            let mut r = blank(Status::Invalid, None);
            r.invalid = Some(inv);
            r.accepted = accepted;
            with_pred(r)
        }
        Ok(s) => with_pred(ImageResult {
            id: it.id.clone(),
            status: Status::Ok,
            verdict: None,
            confidence: None,
            iou: s.iou,
            corner_err_pct: Some(s.corner_err_pct),
            skew_deg: Some(s.skew_deg),
            orientation: Some(s.orientation),
            invalid: None,
            failure: s.iou < FAILURE_IOU,
            accepted,
            tags: it.tags.clone(),
            note: None,
        }),
    }
}

/// Runs `predictor` over every item (sorted by id) and aggregates.
pub fn run(
    manifest: &Manifest,
    predictor: &dyn Predictor,
    cfg: &RunConfig,
) -> Result<Results, String> {
    let mut items: Vec<&ManifestItem> = manifest.items.iter().collect();
    items.sort_by(|a, b| a.id.cmp(&b.id));
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(cfg.threads)
        .build()
        .map_err(|e| format!("cannot build thread pool: {e}"))?;
    let images: Vec<ImageResult> = pool.install(|| {
        items
            .par_iter()
            .map(|it| evaluate_one(manifest, it, predictor))
            .collect()
    });
    let refs: Vec<&ImageResult> = images.iter().collect();
    let summary = summarise(&refs, true);
    let slices = slice(&images);
    let worst = worst_slice(&slices);
    Ok(Results {
        header: Header {
            schema: RESULTS_SCHEMA.to_owned(),
            eval_version: env!("CARGO_PKG_VERSION").to_owned(),
            commit: cfg.commit.clone(),
            host: Host {
                os: std::env::consts::OS.to_owned(),
                arch: std::env::consts::ARCH.to_owned(),
                tier: cfg.tier.clone(),
            },
            suite: cfg.suite.clone(),
            split: cfg.split.clone(),
            predictor: predictor.name(),
            manifest_sha256: manifest.sha256.clone(),
            failure_iou: FAILURE_IOU,
            notes: predictor.notes(),
        },
        summary,
        slices,
        worst_slice: worst,
        calibration: calibration_of(&images),
        images,
    })
}

/// Canonical serialisation: pretty JSON with a trailing newline. Struct field order and the
/// sorted maps make it stable.
pub fn to_json(results: &Results) -> String {
    let mut s = serde_json::to_string_pretty(results).expect("results serialise");
    s.push('\n');
    s
}

pub fn from_json(text: &str) -> Result<Results, String> {
    serde_json::from_str(text).map_err(|e| format!("not an eval results file: {e}"))
}

#[cfg(test)]
pub(crate) mod testing {
    pub use crate::selfcheck::parallelogram_manifest;
}

#[cfg(test)]
mod tests {
    use super::testing::parallelogram_manifest;
    use super::*;
    use crate::predictor::{Crashing, FullFrame, Jittered, Oracle, Prediction};

    fn cfg(threads: usize) -> RunConfig {
        RunConfig {
            threads,
            commit: "test-commit".to_owned(),
            suite: "unit".to_owned(),
            ..RunConfig::default()
        }
    }

    #[test]
    fn oracle_scores_perfectly() {
        let m = parallelogram_manifest(120, 1);
        let r = run(&m, &Oracle::from_manifest(&m), &cfg(2)).expect("runs");
        assert_eq!(r.summary.failures, 0);
        assert_eq!(r.summary.n, 120);
        assert!(r.images.iter().all(|i| (i.iou - 1.0).abs() < 1e-12));
        assert!(r.summary.corner_err_pct.p99.expect("n > 0") < 1e-9);
        assert!(r.summary.skew_deg.p99.expect("n > 0") < 1e-6);
        assert_eq!(r.summary.orientation.upright, 120);
        assert_eq!(r.header.commit, "test-commit");
        assert_eq!(r.header.manifest_sha256, m.sha256);
    }

    #[test]
    fn crashes_and_missing_predictions_count_as_failures_and_do_not_abort_the_run() {
        let m = parallelogram_manifest(200, 2);
        let c = Crashing::from_manifest(&m, 7);
        let planted = m.items.iter().filter(|i| c.crashes(&i.id)).count();
        assert!(planted > 5);
        let r = run(&m, &c, &cfg(4)).expect("runs despite panics");
        assert_eq!(r.summary.n_crashed, planted);
        assert_eq!(r.summary.failures, planted);
        assert!(
            r.images
                .iter()
                .filter(|i| i.status == Status::Crashed)
                .all(|i| {
                    i.failure && !i.accepted && i.note.as_deref() == Some("planted predictor crash")
                })
        );
        let expected_mean = (200 - planted) as f64 / 200.0;
        assert!((r.summary.mean_iou.expect("n > 0") - expected_mean).abs() < 1e-12);
        // An empty predictor is all failures.
        struct Nothing;
        impl Predictor for Nothing {
            fn name(&self) -> String {
                "nothing".to_owned()
            }
            fn predict(&self, _: &PredictInput) -> Result<Prediction, PredictError> {
                Err(PredictError::Missing)
            }
        }
        let r = run(&m, &Nothing, &cfg(1)).expect("runs");
        assert_eq!(
            (r.summary.n_missing, r.summary.failure_rate),
            (200, Some(1.0))
        );
        assert_eq!(r.summary.mean_iou, Some(0.0));
    }

    #[test]
    fn full_frame_iou_is_the_area_fraction_for_affine_ground_truth() {
        let m = parallelogram_manifest(150, 3);
        let r = run(&m, &FullFrame, &cfg(2)).expect("runs");
        for (img, it) in r.images.iter().zip({
            let mut v: Vec<_> = m.items.iter().collect();
            v.sort_by(|a, b| a.id.cmp(&b.id));
            v
        }) {
            let fraction = crate::geom::area(&it.quad);
            assert!(
                (img.iou - fraction).abs() < 1e-12,
                "{}: {} vs {fraction}",
                it.id,
                img.iou
            );
        }
    }

    #[test]
    fn jittered_oracle_follows_the_analytic_curve() {
        let m = parallelogram_manifest(100, 4);
        for shift in [0.0, 0.005, 0.02, 0.05, 0.1, 0.2, 0.4] {
            let r = run(&m, &Jittered::from_manifest(&m, shift, 9), &cfg(2)).expect("runs");
            let want = Jittered::analytic_iou(shift);
            for img in &r.images {
                assert!(
                    (img.iou - want).abs() < 1e-9,
                    "shift {shift}: {} vs {want}",
                    img.iou
                );
            }
            let expect_failures = if want < FAILURE_IOU { 100 } else { 0 };
            assert_eq!(r.summary.failures, expect_failures, "shift {shift}");
        }
    }

    #[test]
    fn results_are_byte_identical_across_runs_and_thread_counts() {
        let m = parallelogram_manifest(300, 5);
        let p = Jittered::from_manifest(&m, 0.03, 1);
        let one = to_json(&run(&m, &p, &cfg(1)).expect("runs"));
        let eight = to_json(&run(&m, &p, &cfg(8)).expect("runs"));
        let again = to_json(&run(&m, &p, &cfg(1)).expect("runs"));
        assert_eq!(one, eight);
        assert_eq!(one, again);
        // The header records commit and host.
        let back = from_json(&one).expect("round trips");
        assert_eq!(back.header.commit, "test-commit");
        assert_eq!(back.header.host.os, std::env::consts::OS);
        assert_eq!(back.header.host.arch, std::env::consts::ARCH);
        assert_eq!(to_json(&back), one, "serialisation is canonical");
    }

    #[test]
    fn unscorable_quads_are_invalid_failures_and_held_results_are_not_silent() {
        let m = parallelogram_manifest(40, 6);
        struct Odd;
        impl Predictor for Odd {
            fn name(&self) -> String {
                "odd".to_owned()
            }
            fn predict(&self, i: &PredictInput) -> Result<Prediction, PredictError> {
                let n: usize = i.id[1..].parse().expect("numeric id");
                Ok(match n % 4 {
                    // Bow-tie, auto-accepted: a silent failure.
                    0 => Prediction {
                        quad: Some([[0.1, 0.1], [0.5, 0.5], [0.5, 0.1], [0.1, 0.5]]),
                        confidence: Some(0.9),
                        verdict: Some(Verdict::Good),
                    },
                    // Wrong but held for review: a failure, not a silent one.
                    1 => Prediction {
                        quad: Some([[0.0, 0.0], [0.1, 0.0], [0.1, 0.1], [0.0, 0.1]]),
                        confidence: Some(0.4),
                        verdict: Some(Verdict::Check),
                    },
                    // Honest "no page".
                    2 => Prediction {
                        quad: None,
                        confidence: Some(0.1),
                        verdict: Some(Verdict::Failed),
                    },
                    _ => Prediction {
                        quad: Some(crate::predictor::FULL_FRAME_QUAD),
                        confidence: Some(0.8),
                        verdict: Some(Verdict::Good),
                    },
                })
            }
        }
        let r = run(&m, &Odd, &cfg(2)).expect("runs");
        assert_eq!(r.summary.n_invalid, 10);
        assert_eq!(r.summary.n_no_quad, 10);
        let silent = r.images.iter().filter(|i| i.accepted && i.failure).count();
        // Invalid+Good (10) plus the full-frame answers that fail (accepted, IoU below 0.9).
        let ff_fail = r
            .images
            .iter()
            .filter(|i| i.status == Status::Ok && i.accepted && i.failure)
            .count();
        assert_eq!(r.summary.accepted.silent_failures, silent);
        assert_eq!(silent, 10 + ff_fail);
        let held = r
            .images
            .iter()
            .filter(|i| i.verdict == Some(Verdict::Check))
            .count();
        assert_eq!(held, 10);
        assert!(
            r.images
                .iter()
                .filter(|i| i.verdict == Some(Verdict::Check))
                .all(|i| !i.accepted)
        );
        assert!(
            r.calibration.is_some(),
            "every answered row carries a confidence"
        );
    }
}
