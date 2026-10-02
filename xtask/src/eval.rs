// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `cargo xtask eval ...` and `cargo xtask synth ...`: thin wrappers that build and run the
//! accuracy harness binary (`auto-crop-eval`, ROADMAP M1.45 and M1.35). Everything after the
//! command name is passed through unchanged, so `cargo xtask eval run --help`-style usage matches
//! `auto-crop-eval` exactly. `synth` defaults `--out` to `target/synth/<suite>`, which is ignored
//! by git; synthetic suites are never committed.

use std::process::Command;

fn harness(args: &[String]) -> Result<(), String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let status = Command::new(cargo)
        .args([
            "run",
            "--quiet",
            "--release",
            "--package",
            "auto-crop-eval",
            "--",
        ])
        .args(args)
        .status()
        .map_err(|e| format!("cannot run cargo: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("auto-crop-eval exited with {status}"))
    }
}

/// `cargo xtask eval <args>`: passes `<args>` to `auto-crop-eval`.
pub fn run_eval(args: &[String]) -> Result<(), String> {
    harness(args)
}

/// The arguments `auto-crop-eval synth` should get for `cargo xtask synth <args>`.
pub fn synth_args(args: &[String]) -> Result<Vec<String>, String> {
    let value_of = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let suite = value_of("--suite").ok_or("synth needs --suite smoke|full")?;
    let mut out = vec!["synth".to_owned()];
    out.extend(args.iter().cloned());
    if value_of("--out").is_none() {
        out.push("--out".to_owned());
        out.push(format!("target/synth/{suite}"));
    }
    Ok(out)
}

/// `cargo xtask synth --suite smoke|full [--out DIR] [--seed N] [--count N] [--max-edge N]`.
pub fn run_synth(args: &[String]) -> Result<(), String> {
    harness(&synth_args(args)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_owned()).collect()
    }

    #[test]
    fn synth_defaults_the_output_directory_to_an_ignored_path() {
        let a = synth_args(&s(&["--suite", "smoke"])).expect("valid");
        assert_eq!(
            a,
            s(&["synth", "--suite", "smoke", "--out", "target/synth/smoke"])
        );
    }

    #[test]
    fn synth_keeps_an_explicit_output_and_requires_a_suite() {
        let a = synth_args(&s(&["--suite", "full", "--out", "x"])).expect("valid");
        assert_eq!(a, s(&["synth", "--suite", "full", "--out", "x"]));
        assert!(synth_args(&s(&["--out", "x"])).is_err());
    }
}
