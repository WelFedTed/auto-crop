// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! ROADMAP M1.69 gate on every OS: the whole hostile-file corpus, each file decoded in its own
//! subprocess, with 0 panics, aborts, hangs or budget misses.

use std::process::Command;

fn xtask() -> Command {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
}

fn scratch(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("auto-crop-hostile-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[test]
fn the_whole_hostile_corpus_is_typed_or_bounded() {
    let out = scratch("all");
    let run = xtask()
        .args(["make-hostile", "--out"])
        .arg(&out)
        .output()
        .expect("run xtask");
    let text = String::from_utf8_lossy(&run.stdout);
    let _ = std::fs::remove_dir_all(&out);
    assert!(
        run.status.success(),
        "hostile corpus failed:\n{text}\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(text.contains(" 0 failed"), "{text}");
    // The corpus is not allowed to shrink silently below the families the gate names.
    for needle in [
        "png-60000x60000",
        "png-100mp-header-truncated-data",
        "png-zlib-bomb-excess-data",
        "png-iccp-bomb",
        "tiff-ifd-chain-20000",
        "jpeg-exif-cyclic-ifd",
        "jpeg-scan-bomb-500-scans",
        "webp-vp8l-16384x16384",
        "png-truncated-50pct",
    ] {
        assert!(text.contains(needle), "missing case {needle}");
    }
}

#[test]
fn the_child_refuses_a_missing_file_and_the_harness_option_parser_rejects_junk() {
    let r = xtask()
        .args(["hostile-run", "definitely-not-a-file.bin"])
        .output()
        .unwrap();
    assert!(!r.status.success());
    let r = xtask().args(["make-hostile", "--bogus"]).output().unwrap();
    assert!(!r.status.success());
}
