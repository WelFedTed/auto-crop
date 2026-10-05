// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `cargo xtask golden eval` (ROADMAP M1.51, M1.52, M1.83 and M1.84, redesigned for one repository).
//!
//! The original items assumed a private repository with a self-hosted runner. Owner decision
//! 2026-10-04: one repository only. The golden set stays on the owner's disk under the gitignored
//! `_data/`, is evaluated here, in this process, and only aggregates ever leave it:
//!
//! * the full per-image results go to `_data/golden/results/` (local, with ids and quads);
//! * the aggregate numbers go through [`PublishableMetrics`] (or its multi-item twin), which has
//!   no per-image field, withholds every slice with n < 30 and marks n < 80 as advisory, and are
//!   leak-checked before they are written to `_data/golden/aggregates/`;
//! * nothing here makes a network call (no HTTP crate is linked; `cargo xtask ci-guards` and a
//!   test in this module check it);
//! * the locked split can be evaluated only with `--reason`, and every evaluation, dev or locked,
//!   is appended to the hash-chained `eval-log.jsonl` before its numbers are shown.

use super::common::{
    Args, DATA_OPTS, Paths, ensure_private, git_commit, sha256_bytes, utc_compact, utc_now, who,
    write_atomic,
};
use super::lock::{self, check_all};
use super::log::{self, LogEntry};
use auto_crop_eval::detector::{DetectorPredictor, ItemsDetectorPredictor};
use auto_crop_eval::golden::{self, GoldenLabel};
use auto_crop_eval::manifest::Manifest;
use auto_crop_eval::multi::{self, MultiRunConfig};
use auto_crop_eval::predictor::{JsonLines, Predictor};
use auto_crop_eval::publish::{PublishableMetrics, check_no_leak};
use auto_crop_eval::publish_multi::{
    PublishableMultiMetrics, check_no_leak as check_no_leak_multi,
};
use auto_crop_eval::report::summary_text;
use auto_crop_eval::run::{RunConfig, run, to_json};
use std::path::Path;

/// What a run produced, for tests and for the report.
#[derive(Debug, Clone)]
#[cfg_attr(not(test), allow(dead_code))] // the paths are read by the tests; the CLI prints `printed`
pub struct Outcome {
    pub n: usize,
    pub aggregate_path: std::path::PathBuf,
    pub results_path: std::path::PathBuf,
    pub printed: String,
}

fn safe_name(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_owned()
}

/// The single-quad predictors of a spec: `detector`, or `jsonl:<file>`.
fn single_predictor(spec: &str) -> Result<Box<dyn Predictor>, String> {
    match spec.split_once(':') {
        None if spec == "detector" => Ok(Box::new(DetectorPredictor::default())),
        Some(("jsonl", path)) => Ok(Box::new(JsonLines::load(Path::new(path))?)),
        _ => Err(format!(
            "unknown predictor `{spec}` (detector | multi | jsonl:<file>)"
        )),
    }
}

fn load_labels(p: &Paths, ids: &[String]) -> Result<Vec<GoldenLabel>, String> {
    let mut out = Vec::new();
    for id in ids {
        let path = p.labels.join(golden::label_file_name(id));
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        out.push(golden::parse_label(&text).map_err(|e| format!("{}: {e}", path.display()))?);
    }
    Ok(out)
}

fn left_out_text(l: &golden::LeftOut) -> String {
    format!(
        "left out of this run: {} negative(s), {} assisted, {} multi-item (single-quad predictors only)",
        l.negatives, l.assisted, l.multi_item
    )
}

/// Runs one evaluation. `print` receives the text meant for the terminal.
pub fn evaluate(
    p: &Paths,
    set: &str,
    spec: &str,
    reason: Option<&str>,
    threads: usize,
) -> Result<Outcome, String> {
    if set != "dev" && set != "locked" {
        return Err(format!("--set must be dev or locked, not {set:?}"));
    }
    ensure_private(&p.golden)?;
    if set == "locked" && reason.is_none_or(|r| r.trim().is_empty()) {
        return Err(
            "the locked split is for confirming a result once per release candidate; say why with --reason \"...\" (it is written to the evaluation log)"
                .to_owned(),
        );
    }
    let errors = check_all(p, true)?;
    if errors > 0 {
        return Err(format!(
            "golden check failed with {errors} error(s); an evaluation never runs on data that changed after locking"
        ));
    }
    let lock = lock::read_lock(&p.lock())?.ok_or("no lock: run `cargo xtask golden lock`")?;
    let state = log::verify(p)?;
    let ids: Vec<String> = lock
        .entries
        .iter()
        .filter(|e| e.split == set)
        .map(|e| e.id.clone())
        .collect();
    if ids.is_empty() {
        return Err(format!("the {set} split has no images"));
    }
    let labels = load_labels(p, &ids)?;
    let multi_mode = spec == "multi";
    let (items, left) = golden::manifest_items(&labels, &|_| Some(set.to_owned()), !multi_mode);
    if items.is_empty() {
        return Err(format!(
            "no scorable image in the {set} split ({})",
            left_out_text(&left)
        ));
    }
    let manifest = Manifest {
        sha256: golden::manifest_sha256(&items),
        base: p.images.clone(),
        items,
    };
    let (commit, dirty) = git_commit();
    let stamp = utc_compact();
    let name = safe_name(spec);
    let results_path = p.results().join(format!("{stamp}-{set}-{name}.json"));
    let aggregate_path = p.aggregates().join(format!("{set}-{name}.json"));
    let manifest_path = p
        .results()
        .join(format!("{stamp}-{set}-{name}.manifest.jsonl"));

    // Run, then build both the full local result and the publishable aggregate.
    let (full_json, aggregate_json, mut printed) = if multi_mode {
        let cfg = MultiRunConfig {
            threads,
            commit: commit.clone(),
            suite: "golden".to_owned(),
            split: set.to_owned(),
        };
        let r = multi::run(&manifest, &ItemsDetectorPredictor::default(), &cfg)?;
        let agg = PublishableMultiMetrics::from_results(&r).to_json();
        check_no_leak_multi(&agg, &r)?;
        let text = multi::summary_text(&r);
        (multi::to_json(&r), agg, text)
    } else {
        let predictor = single_predictor(spec)?;
        let cfg = RunConfig {
            threads,
            commit: commit.clone(),
            suite: "golden".to_owned(),
            split: set.to_owned(),
            tier: Some("golden".to_owned()),
        };
        let r = run(&manifest, predictor.as_ref(), &cfg)?;
        let agg = PublishableMetrics::from_results(&r).to_json();
        check_no_leak(&agg, &r)?;
        (to_json(&r), agg, summary_text(&r))
    };
    // The text for the terminal must not name an image either.
    if let Some(it) = manifest
        .items
        .iter()
        .find(|it| printed.contains(&format!("\"{}\"", it.id)) || printed.contains(&it.image))
    {
        return Err(format!(
            "internal error: the summary names an image ({}); nothing was written",
            it.id
        ));
    }

    // Log first: a result is never shown without its entry in the log.
    log::append(
        p,
        LogEntry {
            seq: 0,
            prev: String::new(),
            at: utc_now(),
            who: who(),
            commit,
            dirty,
            set: set.to_owned(),
            predictor: spec.to_owned(),
            n: manifest.items.len(),
            reason: reason.map(str::to_owned),
            lock_sha256: sha256_bytes(&std::fs::read(p.lock()).map_err(|e| e.to_string())?),
            aggregate_sha256: sha256_bytes(aggregate_json.as_bytes()),
        },
    )?;
    write_atomic(&results_path, full_json.as_bytes())?;
    write_atomic(
        &manifest_path,
        golden::manifest_text(&manifest.items).as_bytes(),
    )?;
    write_atomic(&aggregate_path, aggregate_json.as_bytes())?;

    let header = format!(
        "golden eval: set {set} ({} image(s), lock version {}), predictor {spec}\n{}\n",
        manifest.items.len(),
        lock.version,
        left_out_text(&left)
    );
    let peeks = state.locked_evaluations() + usize::from(set == "locked");
    let footer = format!(
        "\nslices with n < 30 are withheld from the aggregate file, n < 80 are advisory (never a gate).\n\
         aggregate (the only publishable file): {}\n\
         per-image results (local, never publish): {}\n\
         the locked split has now been evaluated {peeks} time(s) in total (see eval-log.jsonl)\n",
        aggregate_path.display(),
        results_path.display(),
    );
    printed = format!("{header}{printed}{footer}");
    Ok(Outcome {
        n: manifest.items.len(),
        aggregate_path,
        results_path,
        printed,
    })
}

/// `cargo xtask golden eval [--set dev|locked] --predictor detector|multi|jsonl:<file> [--reason R]`.
pub fn run_eval(args: &[String]) -> Result<(), String> {
    let a = Args::new(args);
    let opts = [
        "--data",
        "--images",
        "--labels",
        "--set",
        "--predictor",
        "--reason",
        "--threads",
    ];
    a.reject_unknown(&[], &opts)?;
    let p = Paths::from_args(&a)?;
    let set = a.value("--set")?.unwrap_or_else(|| "dev".to_owned());
    let spec = a
        .value("--predictor")?
        .unwrap_or_else(|| "detector".to_owned());
    let threads = a
        .value("--threads")?
        .map(|t| t.parse::<usize>().map_err(|_| format!("bad --threads {t}")))
        .transpose()?
        .unwrap_or(0);
    let _ = DATA_OPTS;
    let out = evaluate(&p, &set, &spec, a.value("--reason")?.as_deref(), threads)?;
    print!("{}", out.printed);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::golden_set::lock::tests::{GOOD, add_label, synthetic_set};
    use auto_crop_codecs::{Format, encode};
    use auto_crop_eval::golden::{GoldenItem, label_to_json, new_label};
    use auto_crop_imgproc::Raster;

    /// A real (synthetic) PNG of a bright page on a dark background, and its label.
    fn add_real(p: &Paths, name: &str, quad: [[f64; 2]; 4]) {
        let (w, h) = (160u32, 120u32);
        let mut r = Raster::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let inside = f64::from(x) / f64::from(w) > quad[0][0]
                    && f64::from(x) / f64::from(w) < quad[1][0]
                    && f64::from(y) / f64::from(h) > quad[0][1]
                    && f64::from(y) / f64::from(h) < quad[2][1];
                let v = if inside { 235 } else { 40 };
                let i = ((y * w + x) * 3) as usize;
                r.data[i..i + 3].copy_from_slice(&[v, v, v]);
            }
        }
        // Distinct bytes per image (identical images are a labelling error), in a corner pixel.
        let digest = sha256_bytes(name.as_bytes());
        for (k, b) in r.data.iter_mut().take(3).enumerate() {
            *b = u8::from_str_radix(&digest[2 * k..2 * k + 2], 16).expect("hex");
        }
        let png = encode(&r, Format::Png, 90, None).expect("encodes");
        std::fs::write(p.images.join(name), &png).expect("write");
        let mut l = new_label(name, &sha256_bytes(&png), w, h);
        l.slices = vec!["flatbed-single".to_owned()];
        l.items = vec![GoldenItem::new(quad)];
        std::fs::write(p.labels.join(format!("{name}.json")), label_to_json(&l)).expect("write");
    }

    fn lock_args(p: &Paths) -> Vec<String> {
        [
            "--data",
            &p.data.display().to_string(),
            "--images",
            &p.images.display().to_string(),
            "--labels",
            &p.labels.display().to_string(),
        ]
        .map(str::to_owned)
        .to_vec()
    }

    #[test]
    fn locked_needs_a_reason_logs_every_run_and_writes_only_aggregates_for_publishing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = synthetic_set(dir.path(), 0);
        for i in 0..12 {
            add_real(
                &p,
                &format!("page{i:02}.png"),
                [[0.2, 0.2], [0.8, 0.2], [0.8, 0.8], [0.2, 0.8]],
            );
        }
        lock::run_lock(&lock_args(&p)).expect("locks");
        // Locked without a reason is refused and leaves no log entry.
        assert!(evaluate(&p, "locked", "detector", None, 1).is_err());
        assert!(log::verify(&p).expect("log").entries.is_empty());
        let dev = evaluate(&p, "dev", "detector", None, 1).expect("dev evaluates");
        assert!(dev.n >= 1);
        let locked =
            evaluate(&p, "locked", "detector", Some("first look"), 1).expect("locked evaluates");
        assert!(
            locked.printed.contains("evaluated 1 time"),
            "{}",
            locked.printed
        );
        let second =
            evaluate(&p, "locked", "detector", Some("again"), 1).expect("locked evaluates again");
        assert!(
            second.printed.contains("evaluated 2 time"),
            "{}",
            second.printed
        );
        let s = log::verify(&p).expect("log intact");
        assert_eq!(s.entries.len(), 3);
        assert_eq!(s.locked_evaluations(), 2);
        assert_eq!(s.entries[1].reason.as_deref(), Some("first look"));
        // The aggregate file is the publishable shape: no ids, no per-image keys.
        let agg = std::fs::read_to_string(&locked.aggregate_path).expect("read");
        assert!(!agg.contains("page0"), "{agg}");
        let v: serde_json::Value = serde_json::from_str(&agg).expect("json");
        assert_eq!(v["schema"], "auto-crop-metrics/1");
        assert!(v.get("images").is_none());
        // Small slices are withheld: every slice here has n < 30.
        assert_eq!(v["slices"].as_object().expect("slices").len(), 0);
        // The full results stay under _data and do carry ids.
        assert!(locked.results_path.starts_with(&p.golden));
        assert!(
            std::fs::read_to_string(&locked.results_path)
                .expect("read")
                .contains("page")
        );
        // Terminal text names no image.
        assert!(!locked.printed.contains(".png"), "{}", locked.printed);
        // Tampering with a locked label blocks the evaluation.
        // Tampering with a locked label blocks every later evaluation, dev included.
        let lock = lock::read_lock(&p.lock()).expect("r").expect("l");
        let victim = lock
            .entries
            .iter()
            .find(|e| e.split == "locked")
            .expect("a locked image")
            .id
            .clone();
        let lp = p.labels.join(format!("{victim}.json"));
        let t = std::fs::read_to_string(&lp).expect("read");
        std::fs::write(&lp, t.replace("0.8", "0.7")).expect("write");
        assert!(evaluate(&p, "dev", "detector", None, 1).is_err());
        assert_eq!(
            log::verify(&p).expect("log").entries.len(),
            3,
            "a refused run leaves no log entry"
        );
    }

    #[test]
    fn multi_and_jsonl_predictors_run_and_negatives_are_counted_not_scored() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = synthetic_set(dir.path(), 0);
        for i in 0..10 {
            add_real(
                &p,
                &format!("a{i}.png"),
                [[0.2, 0.2], [0.8, 0.2], [0.8, 0.8], [0.2, 0.8]],
            );
        }
        // A negative and a two-item image.
        add_label(&p, "neg.png", None);
        let np = p.labels.join("neg.png.json");
        let t = std::fs::read_to_string(&np).expect("read");
        let mut l: GoldenLabel = golden::parse_label(&t).expect("parse");
        l.items.clear();
        l.slices = vec!["negative".to_owned()];
        std::fs::write(&np, label_to_json(&l)).expect("write");
        lock::run_lock(&lock_args(&p)).expect("locks");
        let lock = lock::read_lock(&p.lock()).expect("r").expect("l");
        let set = "dev";
        let n_dev = lock.entries.iter().filter(|e| e.split == set).count();
        assert!(n_dev >= 1);
        let out = evaluate(&p, set, "multi", None, 1).expect("multi runs");
        assert!(out.aggregate_path.to_string_lossy().contains("dev-multi"));
        let agg: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&out.aggregate_path).expect("read"))
                .expect("json");
        assert_eq!(agg["schema"], "auto-crop-metrics-multi/1");
        // A jsonl predictor file is accepted (its answers here are deliberately useless).
        let preds = p.data.join("preds.jsonl");
        let mut text = String::new();
        for e in lock
            .entries
            .iter()
            .filter(|e| e.split == set && e.id != "neg.png")
        {
            text.push_str(&format!(
                "{{\"id\":\"{}\",\"quad\":{},\"confidence\":0.99,\"state\":\"good\"}}\n",
                e.id,
                serde_json::to_string(&GOOD.map(|_| [0.2, 0.2])).expect("json")
            ));
        }
        std::fs::write(&preds, text).expect("write");
        let spec = format!("jsonl:{}", preds.display());
        let out = evaluate(&p, set, &spec, None, 1).expect("jsonl runs");
        assert!(
            out.printed.contains("left out of this run"),
            "{}",
            out.printed
        );
        assert!(evaluate(&p, set, "nonsense", None, 1).is_err());
        assert!(evaluate(&p, "all", "detector", None, 1).is_err());
    }

    #[test]
    fn no_network_crate_is_linked_into_the_harness_or_xtask() {
        // The shipped-crate ban of ci-guards covers cli and shell; this adds the evaluator and the
        // dev tool itself (host triple only, to keep it fast).
        let found = crate::ci_guards::network_guard::check_workspace(
            None,
            &["auto-crop-eval", "xtask"],
            &[host_triple()],
        )
        .expect("cargo metadata");
        assert!(found.is_empty(), "{found:?}");
    }

    fn host_triple() -> &'static str {
        if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
            "x86_64-pc-windows-msvc"
        } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            "aarch64-apple-darwin"
        } else {
            "x86_64-unknown-linux-gnu"
        }
    }
}
