// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The guard that keeps the test-only escape hooks out of a shipped worker.
//!
//! The hooks (attempt a file open, a TCP connect, a process spawn and a 1 GiB allocation after the
//! sandbox is applied; inject a crash, a hang or garbage; pretend Landlock is absent) exist only
//! with `--features escape-tests`. A shipped worker is built without that feature, and this test,
//! which runs in every build, fails when such a binary contains one of the hook's strings or when
//! a manifest other than the worker's and the sandbox crate's turns the feature on.

use std::path::Path;

const HOOK_STRINGS: &[&str] = &["AUTOCROP_ESCAPE", "AUTOCROP-FAULT:", "--no-landlock"];

#[test]
fn a_worker_built_without_the_feature_has_none_of_the_hooks() {
    let exe = Path::new(env!("CARGO_BIN_EXE_auto-crop-worker"));
    let bytes = std::fs::read(exe).unwrap();
    let find = |needle: &str| bytes.windows(needle.len()).any(|w| w == needle.as_bytes());
    if cfg!(feature = "escape-tests") {
        // The control: the feature really does put the strings in, so the check below means
        // something.
        for s in HOOK_STRINGS {
            assert!(
                find(s),
                "{s} should be in an escape-tests build (the guard is blind)"
            );
        }
        let out = std::process::Command::new(exe)
            .arg("--version")
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&out.stdout).contains("escape-tests=ON"));
    } else {
        for s in HOOK_STRINGS {
            assert!(!find(s), "a worker without the feature contains {s:?}");
        }
        let out = std::process::Command::new(exe)
            .arg("--version")
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&out.stdout).contains("escape-tests=off"));
    }
}

#[test]
fn no_other_manifest_enables_the_escape_hooks() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut seen = 0;
    for entry in std::fs::read_dir(crates).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let manifest = entry.path().join("Cargo.toml");
        let Ok(text) = std::fs::read_to_string(&manifest) else {
            continue;
        };
        seen += 1;
        if name == "worker" || name == "sandbox" {
            continue;
        }
        assert!(
            !text.contains("escape-tests"),
            "crates/{name}/Cargo.toml mentions escape-tests: only the worker and the sandbox crate may"
        );
    }
    assert!(seen >= 8, "the scan looked at {seen} manifests");
}
