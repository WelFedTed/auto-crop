// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! ROADMAP M1.34 acceptance: the repository's decoders reproduce the upright reference for every
//! format x EXIF orientation x colour-space variant the Python generator writes.
//!
//! The variants come from `python -m synth variants` (tools/synth), so this test needs the
//! generator's environment: `cargo xtask synth-setup` makes `target/synth-venv`, or point
//! `AUTO_CROP_SYNTH_PYTHON` at an interpreter that has the locked requirements. Without one the
//! test says so and passes, because the Rust unit tests of the checker itself still run; CI sets
//! the environment up and runs this through `cargo xtask synth-check variants` as well.

use std::path::{Path, PathBuf};
use std::process::Command;

fn python() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("AUTO_CROP_SYNTH_PYTHON")
        && !p.is_empty()
    {
        return Some(PathBuf::from(p));
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    ["target/synth-venv/Scripts/python.exe", "target/synth-venv/bin/python"]
        .iter()
        .map(|rel| root.join(rel))
        .find(|p| p.exists())
}

#[test]
fn every_variant_decodes_to_the_upright_reference() {
    let Some(py) = python() else {
        eprintln!(
            "SKIPPED: no tools/synth Python environment (run `cargo xtask synth-setup`); the variant decoder check did NOT run"
        );
        return;
    };
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = std::env::temp_dir().join(format!("ac-synth-variants-{}", std::process::id()));
    let out = Command::new(&py)
        .current_dir(&root)
        .env("PYTHONPATH", root.join("tools/synth"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .args(["-m", "synth", "variants", "--out"])
        .arg(&dir)
        .output()
        .expect("runs python");
    assert!(
        out.status.success(),
        "generator failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = auto_crop_eval::variants::check_dir(&dir).expect("checks");
    std::fs::remove_dir_all(&dir).ok();
    assert!(report.failures.is_empty(), "{:#?}", report.failures);
    // 2 pictures x (JPEG, PNG, TIFF, WebP lossy and lossless) x 8 orientations x 2 colour spaces
    assert_eq!(report.checked, 160);
    assert_eq!(report.lossless, 96);
}
