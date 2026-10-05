// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `cargo xtask check-labels <dir>` (ROADMAP M1.40) and `cargo xtask golden status` (the
//! quota-count part of M1.39's `golden-status`).

use super::common::{Args, DATA_OPTS, Paths};
use auto_crop_eval::golden::{self, CheckOptions, CheckReport, QUOTAS, SLICES};

const CHECK_OPTS: [&str; 1] = ["--images"];

/// `cargo xtask check-labels <labels dir> [--images DIR] [--strict] [--no-hash]`.
pub fn run_check_labels(args: &[String]) -> Result<(), String> {
    let a = Args::new(args);
    a.reject_unknown(&["--strict", "--no-hash"], &CHECK_OPTS)?;
    let pos = a.positionals(&CHECK_OPTS);
    let [dir] = pos.as_slice() else {
        return Err(
            "usage: check-labels <labels dir> [--images DIR] [--strict] [--no-hash]".to_owned(),
        );
    };
    let labels = std::path::PathBuf::from(dir);
    let images = a
        .value("--images")?
        .map_or_else(|| labels.clone(), std::path::PathBuf::from);
    let rep = golden::check_dir(
        &labels,
        &images,
        CheckOptions {
            no_hash: a.flag("--no-hash"),
        },
    )?;
    print_report(&rep);
    let strict = a.flag("--strict");
    let bad = !rep.errors.is_empty()
        || (strict && (!rep.warnings.is_empty() || !rep.images_without_label.is_empty()));
    if bad {
        Err(format!(
            "{} error(s){}",
            rep.errors.len(),
            if strict {
                format!(
                    ", {} warning(s), {} image(s) without a label (--strict)",
                    rep.warnings.len(),
                    rep.images_without_label.len()
                )
            } else {
                String::new()
            }
        ))
    } else {
        Ok(())
    }
}

/// Prints errors, warnings, unlabelled images and slice counts.
pub fn print_report(rep: &CheckReport) {
    for e in &rep.errors {
        println!("ERROR   {e}");
    }
    for w in &rep.warnings {
        println!("warning {w}");
    }
    if !rep.images_without_label.is_empty() {
        println!(
            "{} image(s) without a label (not an error unless --strict):",
            rep.images_without_label.len()
        );
        for n in rep.images_without_label.iter().take(25) {
            println!("  {n}");
        }
        if rep.images_without_label.len() > 25 {
            println!("  ... and {} more", rep.images_without_label.len() - 25);
        }
    }
    println!(
        "check-labels: {} label file(s), {} error(s), {} warning(s), {} assisted (excluded from the golden evaluation), {} image(s) without a label",
        rep.labels,
        rep.errors.len(),
        rep.warnings.len(),
        rep.assisted,
        rep.images_without_label.len()
    );
}

fn median(v: &mut [f64]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let m = v.len() / 2;
    Some(if v.len() % 2 == 1 {
        v[m]
    } else {
        (v[m - 1] + v[m]) / 2.0
    })
}

/// `cargo xtask golden status`: how far the labelling is against the quotas.
pub fn run_status(args: &[String]) -> Result<(), String> {
    let a = Args::new(args);
    a.reject_unknown(&["--no-hash"], &DATA_OPTS)?;
    let p = Paths::from_args(&a)?;
    let files = golden::read_label_dir(&p.labels)?;
    let skipped = skipped_count(&p.labels);
    let (mut eligible, mut assisted, mut bad) = (0usize, 0usize, 0usize);
    let mut per_slice = std::collections::BTreeMap::<&str, usize>::new();
    let mut seconds = Vec::new();
    for lf in &files {
        let Ok(l) = &lf.label else {
            bad += 1;
            continue;
        };
        if !golden::validate_label(l, Some(&lf.stem)).errors.is_empty() {
            bad += 1;
            continue;
        }
        if l.assisted {
            assisted += 1;
            continue;
        }
        eligible += 1;
        for s in &l.slices {
            if let Some(name) = SLICES.iter().find(|n| **n == s.as_str()) {
                *per_slice.entry(name).or_default() += 1;
            }
        }
        if let Some(s) = l.labelling_seconds {
            seconds.push(s);
        }
    }
    let total_images = golden::list_images(&p.images).map(|v| v.len()).unwrap_or(0);
    println!(
        "labels in {}: {eligible} usable, {assisted} assisted (excluded), {bad} invalid; {skipped} skipped; {total_images} image(s) in {}",
        p.labels.display(),
        p.images.display()
    );
    println!(
        "{:<16} {:>6} {:>8} {:>8} {:>8}",
        "slice", "n", "v0 (25)", "v1 (50)", "v2 (80)"
    );
    for s in SLICES {
        let n = per_slice.get(s).copied().unwrap_or(0);
        let cell = |q: usize| {
            if n >= q {
                "ok".to_owned()
            } else {
                format!("-{}", q - n)
            }
        };
        println!(
            "{s:<16} {n:>6} {:>8} {:>8} {:>8}",
            cell(QUOTAS[0].1),
            cell(QUOTAS[1].1),
            cell(QUOTAS[2].1)
        );
    }
    println!(
        "totals: v0 needs >= 150 images ({eligible} usable), v1 >= 500, v2 >= 800 locked (see docs/testing/golden-set.md)"
    );
    if let Some(m) = median(&mut seconds) {
        let mean = seconds.iter().sum::<f64>() / seconds.len() as f64;
        println!(
            "labelling time over {} image(s): median {m:.0} s, mean {mean:.0} s; {:.1} h so far; {:.1} h for 150 images at the median",
            seconds.len(),
            seconds.iter().sum::<f64>() / 3600.0,
            m * 150.0 / 3600.0
        );
    } else {
        println!("no labelling times recorded yet");
    }
    match super::lock::read_lock(&p.lock()) {
        Ok(Some(l)) => {
            let dev = l.entries.iter().filter(|e| e.split == "dev").count();
            println!(
                "lock: version {}, {} dev + {} locked image(s)",
                l.version,
                dev,
                l.entries.len() - dev
            );
        }
        Ok(None) => println!("lock: none yet (cargo xtask golden lock)"),
        Err(e) => println!("lock: unreadable: {e}"),
    }
    Ok(())
}

/// How many images are marked skipped in `_state.json`.
fn skipped_count(labels: &std::path::Path) -> usize {
    std::fs::read_to_string(labels.join("_state.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get("skipped").and_then(|s| s.as_array().map(Vec::len)))
        .unwrap_or(0)
}
