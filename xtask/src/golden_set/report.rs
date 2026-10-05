// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `cargo xtask golden report [--write]`: renders the aggregate metrics files of
//! `_data/golden/aggregates/` as `docs/perf/golden-baseline.md`. Prints by default; the file is
//! written only when the owner passes `--write`, because it is the one place where numbers from
//! the private set leave `_data/`. The renderer reads only the aggregate types
//! (`PublishableMetrics`, `PublishableMultiMetrics`), so it has no access to a per-image row, and
//! it refuses to emit text that contains any image id, file name or scene id of the set.

use super::common::{Args, DATA_OPTS, Paths, pct, utc_now};
use super::lock;
use auto_crop_eval::golden::NON_LABEL_FILES;
use auto_crop_eval::multi::Routing;
use auto_crop_eval::noise::NoiseFloor;
use auto_crop_eval::publish::{METRICS_SCHEMA, PublishableMetrics};
use auto_crop_eval::publish_multi::{MULTI_METRICS_SCHEMA, PublishableMultiMetrics};
use auto_crop_eval::report::{Dist, SliceStatus};
use std::fmt::Write as _;

pub const DEFAULT_OUT: &str = "docs/perf/golden-baseline.md";

fn num(v: Option<f64>, digits: usize) -> String {
    v.map_or_else(|| "n/a".to_owned(), |x| format!("{x:.digits$}"))
}

fn dist(d: &Dist) -> String {
    format!(
        "p50 {}, p95 {}, p99 {}",
        num(d.p50, 3),
        num(d.p95, 3),
        num(d.p99, 3)
    )
}

fn status(s: SliceStatus) -> &'static str {
    match s {
        SliceStatus::Suppressed => "suppressed",
        SliceStatus::Advisory => "advisory (n < 80)",
        SliceStatus::Gated => "n >= 80",
    }
}

/// `slice-receipt-long=yes` becomes `receipt-long`; other keys are shown as they are.
fn slice_label(key: &str) -> String {
    key.strip_prefix("slice-")
        .and_then(|k| k.strip_suffix("=yes"))
        .map_or_else(|| key.to_owned(), str::to_owned)
}

fn render_single(o: &mut String, file: &str, m: &PublishableMetrics) {
    let s = &m.summary;
    let _ = writeln!(
        o,
        "### {file}\n\nPredictor `{}`, split `{}`, commit `{}`, host {}-{}, n = {}.\n",
        m.predictor, m.split, m.commit, m.host.os, m.host.arch, m.n
    );
    let ci = s.mean_iou_ci95.map_or_else(String::new, |c| {
        format!(" (95% interval {:.4} to {:.4})", c[0], c[1])
    });
    let a = &s.accepted;
    let _ = writeln!(
        o,
        "| metric | value |\n|---|---|\n\
         | mean IoU | {}{ci} |\n\
         | failure rate (IoU < 0.90) | {} ({} of {}) |\n\
         | IoU >= 0.95 / >= 0.98 | {} / {} |\n\
         | corner error, % of diagonal | {} |\n\
         | skew, degrees | {} |\n\
         | auto-accepted | {} images, {} silent failure(s), risk {} (one-sided 95% bound {}) |\n\
         | flag rate | {} |",
        num(s.mean_iou, 4),
        pct(s.failure_rate),
        s.failures,
        s.n,
        pct(s.success_95),
        pct(s.success_98),
        dist(&s.corner_err_pct),
        dist(&s.skew_deg),
        a.n,
        a.silent_failures,
        pct(a.risk),
        pct(a.risk_ub95),
        pct(Some(a.flag_rate)),
    );
    if let Some(c) = &m.calibration {
        let _ = writeln!(
            o,
            "\nCalibration (reported, not gated): ECE {:.4}, Brier {:.4}, AUROC {}.",
            c.ece,
            c.brier,
            num(c.auroc, 4)
        );
    }
    let _ = writeln!(
        o,
        "\n| slice | n | status | mean IoU | failure rate |\n|---|---|---|---|---|"
    );
    for (key, sl) in &m.slices {
        let _ = writeln!(
            o,
            "| {} | {} | {} | {} | {} |",
            slice_label(key),
            sl.n,
            status(sl.status),
            num(sl.summary.mean_iou, 4),
            pct(sl.summary.failure_rate)
        );
    }
    let _ = writeln!(
        o,
        "\n{} slice(s) withheld for having fewer than 30 images.\n",
        m.suppressed_slice_count
    );
}

fn routing_line(r: &Routing) -> String {
    if r.n < auto_crop_eval::stats::MIN_PUBLIC_N {
        return "withheld (fewer than 30 touching or overlapping scans)".to_owned();
    }
    format!(
        "{}/{} = {}{}",
        r.held,
        r.n,
        pct(r.rate),
        r.wilson95.map_or_else(String::new, |w| format!(
            " (Wilson 95% interval {:.1}% to {:.1}%)",
            w[0] * 100.0,
            w[1] * 100.0
        ))
    )
}

fn render_multi(o: &mut String, file: &str, m: &PublishableMultiMetrics) {
    let s = &m.summary;
    let _ = writeln!(
        o,
        "### {file}\n\nMulti-item predictor `{}`, split `{}`, commit `{}`, host {}-{}, {} scan(s).\n",
        m.predictor, m.split, m.commit, m.os, m.arch, m.n
    );
    let _ = writeln!(
        o,
        "| metric | value |\n|---|---|\n\
         | exact item count | {} |\n\
         | item recall / precision | {} / {} |\n\
         | mean matched IoU | {} |\n\
         | auto-accepted | {} scans, {} silent wrong split(s), risk {} (one-sided 95% bound {}) |\n\
         | touching or overlapping scans routed to review | {} |",
        pct(s.exact_count_rate),
        pct(s.item_recall),
        pct(s.item_precision),
        num(s.mean_matched_iou, 4),
        s.accepted,
        s.silent_wrong,
        pct(s.silent_wrong_risk),
        pct(s.silent_wrong_ub95),
        routing_line(&m.routing),
    );
    let _ = writeln!(
        o,
        "\n| slice | n | status | exact count | recall | precision |\n|---|---|---|---|---|---|"
    );
    for (key, sl) in &m.slices {
        let _ = writeln!(
            o,
            "| {} | {} | {} | {} | {} | {} |",
            slice_label(key),
            sl.n,
            status(sl.status),
            pct(sl.summary.exact_count_rate),
            pct(sl.summary.item_recall),
            pct(sl.summary.item_precision)
        );
    }
    let _ = writeln!(
        o,
        "\n{} slice(s) withheld for having fewer than 30 scans.\n",
        m.suppressed_slice_count
    );
}

/// Renders the report from `(file name, aggregate JSON text)` pairs and an optional noise floor.
pub fn render(
    files: &[(String, String)],
    noise: Option<&NoiseFloor>,
    generated: &str,
) -> Result<String, String> {
    let mut o = String::new();
    let _ = writeln!(
        o,
        "# Golden-set baseline\n\n\
         Generated {generated} by `cargo xtask golden report` from the maintainer's private golden set. \
         Aggregates only: no image, file name, scene or per-image number appears here (B21).\n\n\
         How to read it:\n\n\
         - **n is small.** The golden set starts at about 150 images (v0). A slice with fewer than 30 images is not shown at all; \
         a slice with 30 to 79 images is **advisory** and never a gate; only slices of 80 or more are comparable with the release gates. \
         Intervals are bootstrap (mean IoU) or Clopper-Pearson / Wilson (rates); with n this small they are wide.\n\
         - **dev** images may be used to tune thresholds. **locked** images are for confirming a result once per release candidate; \
         every evaluation of either split is recorded in a tamper-evident log on the maintainer's machine.\n\
         - Real numbers on one person's documents: indicative of the maintainer's own scans and photos, not a population estimate. \
         Synthetic suites only detect regressions and are reported elsewhere.\n\
         - Labels were drawn blank-quad (no detector suggestion shown); labels made with a suggestion are excluded.\n"
    );
    let _ = writeln!(o, "## Annotator noise floor\n");
    match noise {
        Some(n) => {
            let _ = writeln!(
                o,
                "Two annotators labelled {} images blind. Median IoU between them {}, 5th percentile {}; \
                 corner error median {} % of the diagonal, 95th percentile {} %; skew 95th percentile {} degrees. \
                 No target may be tighter than the 95th-percentile disagreement.\n",
                n.n,
                num(n.iou_median, 4),
                num(n.iou_p05, 4),
                num(n.corner_err_pct_median, 3),
                num(n.corner_err_pct_p95, 3),
                num(n.skew_deg_p95, 3)
            );
        }
        None => {
            let _ = writeln!(
                o,
                "**Not measured yet (ROADMAP M1.43).** It needs a second annotator labelling a random 20% (at least 30 images) blind; \
                 until then no accuracy target on this set is tighter than a human can reproduce. \
                 Run `cargo xtask eval noise-floor --a _data/golden/labels --b <second labels dir> --out _data/golden/noise-floor.json` \
                 and re-render this report.\n"
            );
        }
    }
    let _ = writeln!(o, "## Results\n");
    if files.is_empty() {
        let _ = writeln!(
            o,
            "No evaluation has been run yet (`cargo xtask golden eval --set dev --predictor detector`).\n"
        );
    }
    for (name, text) in files {
        let v: serde_json::Value =
            serde_json::from_str(text).map_err(|e| format!("{name}: {e}"))?;
        let title = name.trim_end_matches(".json");
        match v.get("schema").and_then(|s| s.as_str()) {
            Some(METRICS_SCHEMA) => {
                let m: PublishableMetrics =
                    serde_json::from_str(text).map_err(|e| format!("{name}: {e}"))?;
                render_single(&mut o, title, &m);
            }
            Some(MULTI_METRICS_SCHEMA) => {
                let m: PublishableMultiMetrics =
                    serde_json::from_str(text).map_err(|e| format!("{name}: {e}"))?;
                render_multi(&mut o, title, &m);
            }
            other => return Err(format!("{name}: unknown aggregate schema {other:?}")),
        }
    }
    Ok(o)
}

/// Fails if the text contains any identifier of the set.
pub fn check_no_identifiers(text: &str, lock: &lock::Lock) -> Result<(), String> {
    for e in &lock.entries {
        for needle in [&e.id, &e.image, &e.scene_id] {
            if needle.len() >= 3 && text.contains(needle.as_str()) {
                return Err(format!(
                    "the report text contains an identifier of the golden set ({needle}); refusing to write it"
                ));
            }
        }
    }
    Ok(())
}

/// `cargo xtask golden report [--write] [--out FILE]`.
pub fn run_report(args: &[String]) -> Result<(), String> {
    let a = Args::new(args);
    let mut opts = DATA_OPTS.to_vec();
    opts.push("--out");
    a.reject_unknown(&["--write"], &opts)?;
    let p = Paths::from_args(&a)?;
    let mut files = Vec::new();
    if let Ok(rd) = std::fs::read_dir(p.aggregates()) {
        let mut names: Vec<_> = rd
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|f| {
                f.extension().is_some_and(|e| e == "json")
                    && f.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| !NON_LABEL_FILES.contains(&n))
            })
            .collect();
        names.sort();
        for f in names {
            let name = f
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_owned();
            let text = std::fs::read_to_string(&f)
                .map_err(|e| format!("cannot read {}: {e}", f.display()))?;
            files.push((name, text));
        }
    }
    let noise: Option<NoiseFloor> = std::fs::read_to_string(p.noise_floor())
        .ok()
        .map(|t| serde_json::from_str(&t))
        .transpose()
        .map_err(|e| format!("noise-floor.json: {e}"))?;
    let text = render(&files, noise.as_ref(), &utc_now())?;
    if let Some(lock) = lock::read_lock(&p.lock())? {
        check_no_identifiers(&text, &lock)?;
    }
    if a.flag("--write") {
        let out =
            std::path::PathBuf::from(a.value("--out")?.unwrap_or_else(|| DEFAULT_OUT.to_owned()));
        super::common::write_atomic(&out, text.as_bytes())?;
        println!(
            "wrote {} ({} aggregate file(s))",
            out.display(),
            files.len()
        );
    } else {
        print!("{text}");
        eprintln!(
            "(printed only; pass --write to write {DEFAULT_OUT}, the one place numbers from the private set become public)"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::golden_set::evalrun::evaluate;
    use crate::golden_set::lock::tests::synthetic_set;

    #[test]
    fn the_report_renders_aggregates_with_caveats_and_the_noise_placeholder() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = synthetic_set(dir.path(), 0);
        for i in 0..8 {
            crate::golden_set::lock::tests::add_label(&p, &format!("secret-name-{i}.png"), None);
        }
        // Real pixels are not needed to render: evaluate with a predictor file that fails everything.
        let lock_args: Vec<String> = [
            "--data",
            &p.data.display().to_string(),
            "--images",
            &p.images.display().to_string(),
            "--labels",
            &p.labels.display().to_string(),
        ]
        .map(str::to_owned)
        .to_vec();
        crate::golden_set::lock::run_lock(&lock_args).expect("locks");
        let empty = p.data.join("none.jsonl");
        std::fs::write(&empty, "").expect("write");
        let spec = format!("jsonl:{}", empty.display());
        let out = evaluate(&p, "dev", &spec, None, 1).expect("evaluates");
        let files = vec![(
            "dev-x.json".to_owned(),
            std::fs::read_to_string(&out.aggregate_path).expect("read"),
        )];
        let text = render(&files, None, "2026-10-04T00:00:00Z").expect("renders");
        assert!(text.contains("# Golden-set baseline"));
        assert!(text.contains("Not measured yet (ROADMAP M1.43)"));
        assert!(text.contains("advisory"));
        assert!(text.contains("failure rate"));
        let lock = lock::read_lock(&p.lock()).expect("r").expect("l");
        check_no_identifiers(&text, &lock).expect("no identifiers");
        // A planted identifier is refused.
        let planted = format!("{text}\nsee {}\n", lock.entries[0].image);
        assert!(check_no_identifiers(&planted, &lock).is_err());
        // With a noise floor the placeholder is replaced.
        let nf = NoiseFloor {
            n: 31,
            only_in_a: 0,
            only_in_b: 0,
            unscorable: 0,
            iou_mean: Some(0.97),
            iou_median: Some(0.98),
            iou_p05: Some(0.9),
            corner_err_pct_median: Some(0.3),
            corner_err_pct_p95: Some(1.1),
            skew_deg_median: Some(0.2),
            skew_deg_p95: Some(1.0),
        };
        let with = render(&files, Some(&nf), "t").expect("renders");
        assert!(with.contains("labelled 31 images blind"));
        assert!(!with.contains("Not measured yet"));
        // Default is print-only: nothing was written under docs/.
        let written = dir.path().join("docs").join("golden-baseline.md");
        let mut a = lock_args.clone();
        run_report(&a).expect("prints");
        assert!(!written.exists());
        a.extend([
            "--write".to_owned(),
            "--out".to_owned(),
            written.display().to_string(),
        ]);
        run_report(&a).expect("writes");
        assert!(written.exists());
        // An unknown aggregate is refused.
        assert!(
            render(
                &[("x.json".to_owned(), "{\"schema\":\"nope\"}".to_owned())],
                None,
                "t"
            )
            .is_err()
        );
    }
}
