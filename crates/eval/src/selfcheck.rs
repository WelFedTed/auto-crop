// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Harness self-validation (ROADMAP M1.49): the checks that prove the harness measures what it
//! claims, with no dataset needed, so they run on every PR (`auto-crop-eval self-check` and the
//! unit test that calls it).
//!
//! The ground truths are parallelograms, which are affine images of the canonical square, so
//! every expected value below is an exact closed form, not another estimate.

use crate::calib;
use crate::compare::{CompareConfig, Verdict, compare};
use crate::manifest::{Manifest, ManifestItem, parse};
use crate::metrics::FAILURE_IOU;
use crate::predictor::{Crashing, FullFrame, Jittered, Oracle, Predictor};
use crate::report::Status;
use crate::run::{RunConfig, run, to_json};
use crate::stats::SplitMix64;
use std::collections::BTreeMap;
use std::path::Path;

/// `n` parallelogram ground truths inside the frame, spread over two tag axes. No images exist:
/// the oracle-family predictors never open them.
pub fn parallelogram_manifest(n: usize, seed: u64) -> Manifest {
    let mut r = SplitMix64(seed);
    let mut text = String::new();
    for i in 0..n {
        let (w, h) = (0.15 + 0.5 * r.unit(), 0.15 + 0.5 * r.unit());
        let (x0, y0) = (
            0.1 + (0.7 - w) * 0.3 * r.unit(),
            0.05 + (0.9 - h) * 0.3 * r.unit(),
        );
        let shear = (r.unit() - 0.5) * 0.16;
        let quad = [
            [x0, y0],
            [x0 + w, y0],
            [x0 + w + shear, y0 + h],
            [x0 + shear, y0 + h],
        ];
        let mut tags = BTreeMap::new();
        tags.insert("lighting".to_owned(), ["normal", "dim"][i % 2].to_owned());
        tags.insert(
            "tilt".to_owned(),
            ["0-10", "10-30", "30-45"][i % 3].to_owned(),
        );
        let it = ManifestItem {
            v: 1,
            id: format!("p{i:05}"),
            image: format!("images/p{i:05}.jpg"),
            scene_id: format!("s{i:05}"),
            split: Some(if i % 10 < 3 { "dev" } else { "test" }.to_owned()),
            width: 640,
            height: 480,
            quad,
            items: Vec::new(),
            tags,
        };
        text.push_str(&serde_json::to_string(&it).expect("manifest item serialises"));
        text.push('\n');
    }
    parse(&text, Path::new(".")).expect("generated manifest is valid")
}

#[derive(Debug, Clone)]
pub struct Check {
    pub name: &'static str,
    pub outcome: Result<String, String>,
}

fn cfg(threads: usize) -> RunConfig {
    RunConfig {
        threads,
        commit: "self-check".to_owned(),
        suite: "self-check".to_owned(),
        ..RunConfig::default()
    }
}

fn exec(m: &Manifest, p: &dyn Predictor, threads: usize) -> Result<crate::report::Results, String> {
    run(m, p, &cfg(threads))
}

fn oracle_is_perfect(m: &Manifest) -> Result<String, String> {
    let r = exec(m, &Oracle::from_manifest(m), 2)?;
    let worst = r.images.iter().map(|i| i.iou).fold(1.0, f64::min);
    if r.summary.failures != 0 || worst < 1.0 - 1e-12 {
        return Err(format!(
            "failures {}, lowest IoU {worst}",
            r.summary.failures
        ));
    }
    Ok(format!("{} images, IoU 1.0, 0 failures", r.summary.n))
}

fn jitter_follows_the_analytic_curve(m: &Manifest) -> Result<String, String> {
    let mut worst = 0.0f64;
    for shift in [0.0, 0.01, 0.03, 0.05, 0.1, 0.2, 0.4] {
        let r = exec(m, &Jittered::from_manifest(m, shift, 17), 2)?;
        let want = Jittered::analytic_iou(shift);
        for i in &r.images {
            worst = worst.max((i.iou - want).abs());
        }
        let want_failures = if want < FAILURE_IOU { m.items.len() } else { 0 };
        if r.summary.failures != want_failures {
            return Err(format!(
                "shift {shift}: {} failures, expected {want_failures}",
                r.summary.failures
            ));
        }
    }
    if worst > 1e-9 {
        return Err(format!(
            "largest deviation from the analytic curve {worst:.3e}"
        ));
    }
    Ok(format!("7 shifts, max deviation {worst:.1e}"))
}

fn full_frame_is_the_area_fraction(m: &Manifest) -> Result<String, String> {
    let r = exec(m, &FullFrame, 2)?;
    let mut worst = 0.0f64;
    let mut sorted: Vec<&ManifestItem> = m.items.iter().collect();
    sorted.sort_by(|a, b| a.id.cmp(&b.id));
    for (img, it) in r.images.iter().zip(sorted) {
        worst = worst.max((img.iou - crate::geom::area(&it.quad)).abs());
    }
    if worst > 1e-12 {
        return Err(format!(
            "largest deviation from the area fraction {worst:.3e}"
        ));
    }
    Ok(format!("max deviation {worst:.1e}"))
}

fn crashes_are_counted(m: &Manifest) -> Result<String, String> {
    let c = Crashing::from_manifest(m, 7);
    let planted = m.items.iter().filter(|i| c.crashes(&i.id)).count();
    let r = exec(m, &c, 4)?;
    let counted = r.summary.n_crashed;
    let failed = r
        .images
        .iter()
        .filter(|i| i.status == Status::Crashed && i.failure && !i.accepted)
        .count();
    if planted == 0 || counted != planted || failed != planted || r.summary.n != m.items.len() {
        return Err(format!(
            "planted {planted}, counted {counted}, failing {failed}"
        ));
    }
    Ok(format!(
        "{planted} planted crashes counted as failures, run completed"
    ))
}

fn deterministic_across_threads(m: &Manifest) -> Result<String, String> {
    let p = Jittered::from_manifest(m, 0.03, 5);
    let one = to_json(&exec(m, &p, 1)?);
    let eight = to_json(&exec(m, &p, 8)?);
    let again = to_json(&exec(m, &p, 1)?);
    if one != eight || one != again {
        return Err("results differ between runs or thread counts".to_owned());
    }
    Ok(format!(
        "{} bytes identical at 1 and 8 threads and across runs",
        one.len()
    ))
}

fn gate_blocks_a_jittered_head_and_passes_an_identical_one(m: &Manifest) -> Result<String, String> {
    let base = exec(m, &Oracle::from_manifest(m), 2)?;
    let same = exec(m, &Oracle::from_manifest(m), 2)?;
    let jittered = exec(m, &Jittered::from_manifest(m, 0.01, 3), 2)?;
    let pass = compare(&base, &same, &CompareConfig::default())?;
    let fail = compare(&base, &jittered, &CompareConfig::default())?;
    if pass.verdict != Verdict::Pass {
        return Err(format!(
            "identical runs: {:?} {:?}",
            pass.verdict, pass.reasons
        ));
    }
    if fail.verdict != Verdict::Fail {
        return Err("a jittered head was not blocked".to_owned());
    }
    Ok(format!(
        "identical passes; jittered blocked ({:+.2} pt mean IoU)",
        fail.mean_iou.delta * 100.0
    ))
}

fn calibration_reads_near_zero_for_a_calibrated_predictor() -> Result<String, String> {
    let mut r = SplitMix64(11);
    let data: Vec<(f64, bool)> = (0..40_000)
        .map(|_| {
            let c = r.unit();
            (c, r.unit() < c)
        })
        .collect();
    let ece = calib::calibration(&data).ece;
    if ece > 0.01 {
        return Err(format!("ECE {ece:.4} for a calibrated predictor"));
    }
    Ok(format!("ECE {ece:.4} on 40k calibrated draws"))
}

/// Runs every check. A check that errors does not stop the others.
pub fn run_all() -> Vec<Check> {
    let m = parallelogram_manifest(200, 0x5E1F);
    let checks: [(&'static str, Result<String, String>); 7] = [
        ("oracle IoU 1.0, 0 failures", oracle_is_perfect(&m)),
        (
            "jittered oracle follows the analytic curve",
            jitter_follows_the_analytic_curve(&m),
        ),
        (
            "FullFrame equals the area fraction",
            full_frame_is_the_area_fraction(&m),
        ),
        ("crashes counted as failures", crashes_are_counted(&m)),
        (
            "byte-identical at 1 and 8 threads",
            deterministic_across_threads(&m),
        ),
        (
            "gate: jittered fails, identical passes",
            gate_blocks_a_jittered_head_and_passes_an_identical_one(&m),
        ),
        (
            "ECE near 0 for a calibrated predictor",
            calibration_reads_near_zero_for_a_calibrated_predictor(),
        ),
    ];
    checks
        .into_iter()
        .map(|(name, outcome)| Check { name, outcome })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_self_check_passes() {
        let checks = run_all();
        assert_eq!(checks.len(), 7);
        let bad: Vec<String> = checks
            .iter()
            .filter_map(|c| c.outcome.as_ref().err().map(|e| format!("{}: {e}", c.name)))
            .collect();
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[test]
    fn the_checks_would_catch_a_broken_harness() {
        // A manifest of perspective (non-affine) quads must not satisfy the closed forms: proof
        // that the checks have teeth and the parallelogram choice matters.
        let mut items = parallelogram_manifest(20, 3).items;
        for it in &mut items {
            it.quad[2] = [it.quad[2][0] + 0.05, it.quad[2][1] + 0.08];
        }
        let text: String = items
            .iter()
            .map(|i| serde_json::to_string(i).expect("serialises") + "\n")
            .collect();
        let m = parse(&text, Path::new(".")).expect("valid");
        assert!(full_frame_is_the_area_fraction(&m).is_err());
    }
}
