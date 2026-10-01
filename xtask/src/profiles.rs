// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `cargo xtask check-profiles`: panic = "unwind" guard (ROADMAP M0.31).
//!
//! Decoder glue relies on `catch_unwind`, so no profile may set
//! `panic = "abort"`, and `[profile.release]` must say `panic = "unwind"`.

use std::fs;
use std::path::Path;

/// Checks one manifest's text; returns violations.
pub fn check_manifest(name: &str, text: &str, require_release_unwind: bool) -> Vec<String> {
    let mut out = Vec::new();
    let mut section = String::new();
    let mut release_unwind = false;
    for (i, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') && line.ends_with(']') {
            section = line
                .trim_matches(|c| c == '[' || c == ']')
                .trim()
                .to_owned();
            continue;
        }
        let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
        if compact == "panic=\"abort\"" {
            out.push(format!(
                "{name}:{}: panic = \"abort\" in [{section}]",
                i + 1
            ));
        }
        if compact == "panic=\"unwind\"" && section == "profile.release" {
            release_unwind = true;
        }
    }
    if require_release_unwind && !release_unwind {
        out.push(format!(
            "{name}: [profile.release] must set panic = \"unwind\""
        ));
    }
    out
}

pub fn run(_args: &[String]) -> Result<(), String> {
    let mut violations = Vec::new();
    let root =
        fs::read_to_string("Cargo.toml").map_err(|e| format!("cannot read Cargo.toml: {e}"))?;
    violations.extend(check_manifest("Cargo.toml", &root, true));
    for dir in ["crates", "xtask"] {
        let base = Path::new(dir);
        let manifests: Vec<_> = if base.join("Cargo.toml").exists() {
            vec![base.join("Cargo.toml")]
        } else {
            fs::read_dir(base)
                .map_err(|e| format!("cannot read {dir}: {e}"))?
                .filter_map(Result::ok)
                .map(|d| d.path().join("Cargo.toml"))
                .filter(|p| p.exists())
                .collect()
        };
        for m in manifests {
            let text = fs::read_to_string(&m).map_err(|e| format!("{}: {e}", m.display()))?;
            violations.extend(check_manifest(&m.display().to_string(), &text, false));
        }
    }
    if violations.is_empty() {
        println!("check-profiles: all profiles unwind");
        Ok(())
    } else {
        Err(violations.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str =
        "[profile.dev]\npanic = \"unwind\"\n\n[profile.release]\npanic = \"unwind\"\n";

    #[test]
    fn good_manifest_passes() {
        assert!(check_manifest("Cargo.toml", GOOD, true).is_empty());
    }

    #[test]
    fn planted_abort_in_release_fails() {
        let bad = GOOD.replace(
            "[profile.release]\npanic = \"unwind\"",
            "[profile.release]\npanic = \"abort\"",
        );
        let v = check_manifest("Cargo.toml", &bad, true);
        assert!(v.iter().any(|m| m.contains("abort")), "{v:?}");
        assert!(v.iter().any(|m| m.contains("must set")), "{v:?}");
    }

    #[test]
    fn missing_release_unwind_fails() {
        let v = check_manifest("Cargo.toml", "[profile.dev]\npanic = \"unwind\"\n", true);
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn comments_are_ignored() {
        let t = format!("{GOOD}# panic = \"abort\" is forbidden\n");
        assert!(check_manifest("Cargo.toml", &t, true).is_empty());
    }
}
