// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `cargo xtask eval ...`: a thin wrapper that builds and runs the accuracy harness binary
//! (`auto-crop-eval`, ROADMAP M1.45). Everything after the command name is passed through
//! unchanged, so `cargo xtask eval run --help`-style usage matches `auto-crop-eval` exactly.
//! `cargo xtask synth` lives in `synth.rs` (the Python generator, with the Rust stand-in behind
//! `--generator rust`); synthetic suites are never committed.

use std::process::Command;

/// Builds (release) and runs `auto-crop-eval <args>`.
pub fn harness(args: &[String]) -> Result<(), String> {
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
