// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! ROADMAP M1.71: crashers found by the fuzzers become plain test inputs on all three OSes.
//!
//! `fuzz/regressions/<target>/<file>` is run through the target's entry function (the same function
//! libFuzzer calls, see `fuzz/src/lib.rs`) without libFuzzer, so this runs on Windows and macOS
//! too. The generated seed corpus is replayed as well, which keeps the entry points and the seeds
//! from rotting between fuzz runs.

use auto_crop_fuzz::{Entry, TARGETS, replay, seeds};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn fuzz_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../fuzz")
}

fn names_in(dir: &Path, ext: Option<&str>) -> BTreeSet<String> {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| match ext {
                    Some(x) => p.extension().is_some_and(|e| e == x),
                    None => p.is_dir(),
                })
                .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn every_regression_file_passes_its_target() {
    let root = fuzz_dir().join("regressions");
    let mut failures = Vec::new();
    let mut count = 0;
    for target in names_in(&root, None) {
        let f = auto_crop_fuzz::entry(&target)
            .unwrap_or_else(|| panic!("fuzz/regressions/{target}/ has no target of that name"));
        let mut files: Vec<_> = std::fs::read_dir(root.join(&target))
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .collect();
        files.sort();
        for p in files {
            count += 1;
            let bytes = std::fs::read(&p).unwrap();
            if let Err(e) = replay(f, &bytes) {
                failures.push(format!(
                    "{target}/{}: {e}",
                    p.file_name().unwrap().to_string_lossy()
                ));
            }
        }
    }
    eprintln!("replayed {count} regression file(s)");
    assert!(
        failures.is_empty(),
        "fuzz regressions fail again:\n{}",
        failures.join("\n")
    );
}

#[test]
fn the_target_table_matches_the_files_and_the_manifest() {
    let table: BTreeSet<String> = TARGETS.iter().map(|(n, _)| (*n).to_owned()).collect();
    assert_eq!(table.len(), TARGETS.len(), "duplicate target name");
    assert_eq!(
        names_in(&fuzz_dir().join("fuzz_targets"), Some("rs")),
        table,
        "fuzz/fuzz_targets/*.rs and fuzz::TARGETS differ"
    );
    let manifest = std::fs::read_to_string(fuzz_dir().join("Cargo.toml")).unwrap();
    for name in &table {
        assert!(
            manifest.contains(&format!("name = \"{name}\"")),
            "fuzz/Cargo.toml has no [[bin]] for {name}"
        );
        let wrapper =
            std::fs::read_to_string(fuzz_dir().join(format!("fuzz_targets/{name}.rs"))).unwrap();
        assert!(
            wrapper.contains(&format!("auto_crop_fuzz::{name}(data)")),
            "fuzz_targets/{name}.rs does not call its entry function"
        );
    }
    // Regression directories may only exist for known targets.
    for d in names_in(&fuzz_dir().join("regressions"), None) {
        assert!(table.contains(&d), "fuzz/regressions/{d}/ is not a target");
    }
}

#[test]
fn every_seed_passes_its_target_quickly() {
    let all = seeds::seeds();
    for (target, _) in TARGETS {
        assert!(
            all.iter().any(|s| s.target == *target),
            "target {target} has no seeds"
        );
    }
    let mut failures = Vec::new();
    for s in &all {
        let f: Entry = auto_crop_fuzz::entry(s.target).expect("seed for an unknown target");
        let t = std::time::Instant::now();
        let r = replay(f, &s.bytes);
        let took = t.elapsed();
        if let Err(e) = r {
            failures.push(format!("{}/{}: {e}", s.target, s.name));
        }
        // A seed that takes seconds would make every fuzz execution slow (PROVISIONAL bound).
        if took.as_secs() >= 10 {
            failures.push(format!("{}/{} took {took:?}", s.target, s.name));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A guard that is not proven red is not a guard: a target that panics must be reported with its
/// message, and one that returns must pass.
#[test]
fn replay_reports_a_panicking_target() {
    fn bad(data: &[u8]) {
        assert!(data.is_empty(), "planted failure on {} bytes", data.len());
    }
    fn good(_: &[u8]) {}
    let err = replay(bad, b"abc").unwrap_err();
    assert!(err.contains("planted failure on 3 bytes"), "{err}");
    assert!(replay(bad, b"").is_ok());
    assert!(replay(good, b"abc").is_ok());
}
