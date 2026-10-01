// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `cargo xtask deny-selftest` (ROADMAP M0.19).
//!
//! Runs `cargo deny` with the real `deny.toml` over tiny fixture crates outside
//! the shipping workspace: planted AGPL, GPL, non-commercial and banned-name
//! crates must each FAIL, and the clean control must PASS. This proves the
//! policy is enforced mechanically, not by convention.

use std::fs;
use std::path::Path;
use std::process::Command;

const FIXTURES: &str = "xtask/fixtures/deny-selftest";

/// A fixture directory named `control` must pass; every other one must fail.
pub fn expects_failure(dir_name: &str) -> bool {
    dir_name != "control"
}

/// Text that must appear in cargo-deny's output for a fixture to count as failing
/// for the RIGHT reason (not, say, a mistyped flag).
pub fn expected_marker(dir_name: &str) -> Option<&'static str> {
    match dir_name {
        "agpl" => Some("AGPL-3.0"),
        "gpl" => Some("GPL-3.0"),
        "ccnc" => Some("CC-BY-NC"),
        "heic" => Some("heic"),
        _ => None,
    }
}

pub fn run(_args: &[String]) -> Result<(), String> {
    let mut dirs: Vec<_> = fs::read_dir(FIXTURES)
        .map_err(|e| format!("cannot read {FIXTURES}: {e}"))?
        .filter_map(Result::ok)
        .filter(|d| d.path().join("Cargo.toml").exists())
        .collect();
    dirs.sort_by_key(std::fs::DirEntry::file_name);
    if dirs.is_empty() {
        return Err(format!("no fixtures found in {FIXTURES}"));
    }
    let deny_toml = fs::canonicalize("deny.toml").map_err(|e| format!("deny.toml: {e}"))?;
    let mut problems = Vec::new();
    for d in &dirs {
        let name = d.file_name().to_string_lossy().into_owned();
        let manifest = d.path().join("Cargo.toml");
        let out = Command::new("cargo")
            .args(["deny", "--manifest-path"])
            .arg(&manifest)
            .arg("--config")
            .arg(&deny_toml)
            .arg("check")
            .args(["licenses", "bans", "sources"])
            .output()
            .map_err(|e| format!("cannot run cargo deny (is cargo-deny installed?): {e}"))?;
        let output_text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let failed = !out.status.success();
        if failed
            && let Some(m) = expected_marker(&name)
            && !output_text.contains(m)
        {
            problems.push(format!(
                "fixture `{name}` failed, but not for the expected reason (no `{m}` in output):
{output_text}"
            ));
        }
        let want_fail = expects_failure(&name);
        let verdict = if failed == want_fail { "ok" } else { "WRONG" };
        println!(
            "deny-selftest: {name:<8} expected {:<4} got {:<4} -> {verdict}",
            if want_fail { "FAIL" } else { "PASS" },
            if failed { "FAIL" } else { "PASS" }
        );
        if failed != want_fail {
            problems.push(format!(
                "fixture `{name}` expected {} but cargo deny {}:\n{}",
                if want_fail { "failure" } else { "success" },
                if failed { "failed" } else { "passed" },
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        // Clean up generated lock files and targets so the tree stays tidy.
        let _ = fs::remove_file(Path::new(&d.path()).join("Cargo.lock"));
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_exist_for_every_failing_fixture() {
        for n in ["agpl", "gpl", "ccnc", "heic"] {
            assert!(expected_marker(n).is_some());
        }
        assert!(expected_marker("control").is_none());
    }

    #[test]
    fn only_the_control_is_expected_to_pass() {
        assert!(!expects_failure("control"));
        for n in ["agpl", "gpl", "ccnc", "heic"] {
            assert!(expects_failure(n));
        }
    }
}
