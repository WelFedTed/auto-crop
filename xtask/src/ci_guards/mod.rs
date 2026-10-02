// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `cargo xtask ci-guards [--selftest]` (ROADMAP M1.72, M1.80).
//!
//! CI policy guards that extend `check-profiles` (M0.31, which owns the `panic = "abort"` ban):
//!
//! * [`unsafe_guard`]: `unsafe` only inside `ffi/` and `simd/` module directories (plus an
//!   explicit allow-list), always with a `// SAFETY:` comment.
//! * [`network_guard`]: no HTTP, TLS or socket crates in the shipped build (B18, C4).
//! * [`corpus_guard`]: no dataset bytes in the repository (archives, scans, RAW files, big files,
//!   dataset directories) and the cache directories stay in `.gitignore` (M1.36, B21).
//! * [`workflow_guard`]: workflow hygiene (SHA pins, least privilege, no dangerous triggers,
//!   required jobs always report, no release machinery yet).
//!
//! Every guard has a red test: the planted violations in each module's `CASES` must be detected
//! and the controls must pass. `--selftest` runs them from the command line (CI does), and the
//! same cases run under `cargo test`. The allow-lists and the reasoning are in
//! `docs/policy/ci-guards.md`.

mod corpus_guard;
mod network_guard;
mod rust_lex;
mod unsafe_guard;
mod workflow_guard;

use std::path::Path;

fn selftest() -> Result<(), String> {
    let mut problems = unsafe_guard::selftest();
    problems.extend(workflow_guard::selftest());
    problems.extend(corpus_guard::selftest());
    let base = std::env::temp_dir().join(format!("auto-crop-ci-guards-{}", std::process::id()));
    let net = network_guard::selftest(&base);
    let _ = std::fs::remove_dir_all(&base);
    problems.extend(net?);
    if problems.is_empty() {
        println!(
            "ci-guards selftest: {} unsafe, {} manifest, {} workflow, {} network and {} corpus plants/controls behaved",
            unsafe_guard::CASES.len() + unsafe_guard::MANIFEST_CASES.len(),
            network_guard::MANIFEST_CASES.len(),
            workflow_guard::cases().len(),
            network_guard::CASES.len(),
            corpus_guard::cases().len() + corpus_guard::gitignore_cases().len()
        );
        Ok(())
    } else {
        Err(problems.join("\n"))
    }
}

pub fn run(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("--selftest") => return selftest(),
        Some(other) => return Err(format!("unknown ci-guards argument: {other}")),
        None => {}
    }
    let root = Path::new(".");
    let mut violations = Vec::new();
    violations.extend(unsafe_guard::check_tree(root)?);
    violations.extend(network_guard::check_manifests(root)?);
    violations.extend(network_guard::check_workspace(
        None,
        network_guard::SHIPPED_ROOTS,
        network_guard::TRIPLES,
    )?);
    violations.extend(workflow_guard::check_tree(root)?);
    violations.extend(corpus_guard::check_tree(root)?);
    if violations.is_empty() {
        println!(
            "ci-guards: unsafe, network, workflow and corpus policies hold ({} allow-listed unsafe file(s))",
            unsafe_guard::ALLOWED_FILES.len()
        );
        Ok(())
    } else {
        Err(violations.join("\n"))
    }
}
