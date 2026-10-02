// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! End-to-end tests of the `auto-crop-eval` binary on a tiny generated suite: synth, run with the
//! real detector, determinism across thread counts, the compare gate's exit codes (the M1.50
//! mutation test: jittered fails, identical passes) and the publishing guard.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_auto-crop-eval"))
}

fn run(args: &[&str]) -> Output {
    bin().args(args).output().expect("binary runs")
}

fn ok(args: &[&str]) -> String {
    let o = run(args);
    assert!(
        o.status.success(),
        "{args:?} failed: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ac-eval-cli-{}-{name}", std::process::id()));
    std::fs::remove_dir_all(&d).ok();
    std::fs::create_dir_all(&d).expect("scratch dir");
    d
}

fn s(p: &Path) -> &str {
    p.to_str().expect("utf-8 path")
}

/// 40 small images: enough for tags to repeat, small enough to run in a second.
fn suite(dir: &Path) -> PathBuf {
    ok(&[
        "synth",
        "--suite",
        "smoke",
        "--count",
        "40",
        "--max-edge",
        "200",
        "--out",
        s(dir),
    ]);
    dir.join("manifest.jsonl")
}

#[test]
fn synth_run_compare_and_publish_end_to_end() {
    let dir = scratch("e2e");
    let manifest = suite(&dir);
    ok(&["validate-manifest", s(&manifest)]);

    let (r1, r8, rf, rj) = (
        dir.join("detector-1.json"),
        dir.join("detector-8.json"),
        dir.join("full-frame.json"),
        dir.join("jitter.json"),
    );
    let common = |predictor: &str, out: &Path, threads: &str| {
        ok(&[
            "run",
            "--manifest",
            s(&manifest),
            "--predictor",
            predictor,
            "--out",
            s(out),
            "--suite",
            "cli-test",
            "--commit",
            "deadbeef",
            "--threads",
            threads,
        ])
    };
    let summary = common("detector", &r1, "1");
    common("detector", &r8, "8");
    common("full-frame", &rf, "2");
    common("jitter:0.01", &rj, "2");

    // The detector actually finds pages on its own lineage of synthetic images.
    assert!(summary.contains("mean IoU"), "{summary}");
    let results: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&r1).expect("results")).expect("json");
    let mean_iou = results["summary"]["mean_iou"].as_f64().expect("mean IoU");
    assert!(
        mean_iou > 0.3,
        "detector mean IoU {mean_iou} on easy-ish synthetic images"
    );
    assert_eq!(results["header"]["commit"], "deadbeef");
    assert_eq!(results["summary"]["n"], 40);

    // Byte-identical at 1 and 8 threads.
    assert_eq!(
        std::fs::read(&r1).expect("a"),
        std::fs::read(&r8).expect("b"),
        "results must not depend on the thread count"
    );

    // Compare: identical passes (exit 0), a jittered head fails (exit 1) and the waiver label
    // turns the failure into an override (exit 0).
    let pass = run(&["compare", "--base", s(&r1), "--head", s(&r8)]);
    assert_eq!(pass.status.code(), Some(0));
    let oracle = dir.join("oracle.json");
    common("oracle", &oracle, "2");
    let fail = run(&["compare", "--base", s(&oracle), "--head", s(&rj)]);
    assert_eq!(
        fail.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&fail.stdout)
    );
    assert!(String::from_utf8_lossy(&fail.stdout).contains("FAIL"));
    let waived = run(&[
        "compare",
        "--base",
        s(&oracle),
        "--head",
        s(&rj),
        "--waiver",
    ]);
    assert_eq!(waived.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&waived.stdout).contains("WAIVED"));
    // Comparing results over different manifests is an error (exit 2), never a pass.
    let other = scratch("e2e-other");
    let other_manifest = other.join("manifest.jsonl");
    ok(&[
        "synth",
        "--suite",
        "smoke",
        "--count",
        "40",
        "--max-edge",
        "200",
        "--seed",
        "99",
        "--out",
        s(&other),
    ]);
    let rother = other.join("r.json");
    ok(&[
        "run",
        "--manifest",
        s(&other_manifest),
        "--predictor",
        "full-frame",
        "--out",
        s(&rother),
    ]);
    let mismatch = run(&["compare", "--base", s(&rf), "--head", s(&rother)]);
    assert_eq!(mismatch.status.code(), Some(2));

    // Publish: a leak-checked aggregate view with no image ids.
    let published = dir.join("published.json");
    ok(&["publish", "--results", s(&r1), "--out", s(&published)]);
    let text = std::fs::read_to_string(&published).expect("published");
    assert!(!text.contains("smoke-0"), "an image id leaked: {text}");
    assert!(text.contains("auto-crop-metrics/1"));

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&other).ok();
}

#[test]
fn a_jsonl_predictor_is_scored_and_missing_lines_are_failures() {
    let dir = scratch("jsonl");
    let manifest = suite(&dir);
    // Predict the full frame for the first 10 images only.
    let text = std::fs::read_to_string(&manifest).expect("manifest");
    let preds: String = text
        .lines()
        .take(10)
        .map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).expect("row");
            format!(
                "{{\"id\":{},\"quad\":[[0,0],[1,0],[1,1],[0,1]],\"confidence\":0.5}}\n",
                v["id"]
            )
        })
        .collect();
    let pf = dir.join("preds.jsonl");
    std::fs::write(&pf, preds).expect("write");
    let out = dir.join("r.json");
    let spec = format!("jsonl:{}", s(&pf));
    ok(&[
        "run",
        "--manifest",
        s(&manifest),
        "--predictor",
        &spec,
        "--out",
        s(&out),
    ]);
    let r: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&out).expect("r")).expect("json");
    assert_eq!(r["summary"]["n_missing"], 30);
    assert_eq!(r["summary"]["n_ok"], 10);
    assert!(r["summary"]["failures"].as_u64().expect("failures") >= 30);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn bad_usage_exits_with_an_error_not_a_panic() {
    assert_eq!(run(&["frobnicate"]).status.code(), Some(2));
    assert_eq!(run(&["run"]).status.code(), Some(2));
    assert_eq!(
        run(&["synth", "--suite", "enormous", "--out", "x"])
            .status
            .code(),
        Some(2)
    );
    assert_eq!(run(&["--help"]).status.code(), Some(0));
    assert_eq!(run(&["self-check"]).status.code(), Some(0));
}
