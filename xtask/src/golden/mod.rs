// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The local golden-set tooling (ROADMAP M1.39 to M1.44, M1.51, M1.52, M1.83 to M1.85, adapted to
//! the one-repository decision of 2026-10-04: the golden set never leaves the owner's machine).
//! Guide: docs/testing/golden-workflow.md.
//!
//! * `cargo xtask check-labels <dir>`: validate label files ([`check`]).
//! * `cargo xtask label <image dir>`: the blank-quad labeller (module `label`).
//! * `cargo xtask golden status|lock|check|eval|report|backup|restore-check|restore`.

pub mod backup;
pub mod check;
pub mod common;
pub mod evalrun;
pub mod lock;
pub mod log;
pub mod report;

pub const HELP: &str = "\
cargo xtask golden <command> [--data DIR] [--images DIR] [--labels DIR]
        --data defaults to _data (gitignored, private); --images to --data; --labels to <data>/golden/labels.
  status
        Labelled images per slice against the v0/v1/v2 quotas, labelling times, lock state.
  lock [--withdraw ID]
        Write or extend <data>/golden/splits.lock.json: scene-disjoint dev/locked split (PROVISIONAL
        30/70), SHA-256 of every image and label. Never changes an existing assignment; a locked
        image or label that was edited is an error. --withdraw removes an image on request.
  check [--no-hash]
        Fail if a locked image or label was edited, a scene is in both splits, a label is invalid,
        or the evaluation log was rewritten.
  eval [--set dev|locked] [--predictor detector|multi|jsonl:FILE] [--reason TEXT] [--threads N]
        Run the harness locally. Full per-image results go under <data>/golden/results/ only;
        aggregate numbers (slices n >= 30 only, n < 80 advisory) go to <data>/golden/aggregates/ and
        the terminal. The locked set needs --reason; every run is appended to eval-log.jsonl.
  report [--write] [--out FILE]
        Render the aggregates as docs/perf/golden-baseline.md (printed unless --write).
  backup DEST
        Copy images, labels and metadata to DEST with a SHA-256 manifest (no encryption: use a
        BitLocker/VeraCrypt volume on a second disk).
  restore-check DEST [--require-current] [--no-live]
        Verify every backed-up file and the lock's hashes; compare with the live data.
  restore DEST --to EMPTYDIR
        Restore a backup into a fresh data folder, verifying every hash.
";

/// `cargo xtask golden <command>`.
pub fn run(args: &[String]) -> Result<(), String> {
    let (cmd, rest) = args
        .split_first()
        .map_or(("", &[][..]), |(c, r)| (c.as_str(), r));
    match cmd {
        "status" => check::run_status(rest),
        "lock" => lock::run_lock(rest),
        "check" => lock::run_check(rest),
        "eval" => evalrun::run_eval(rest),
        "report" => report::run_report(rest),
        "backup" => backup::run_backup(rest),
        "restore-check" => backup::run_restore_check(rest),
        "restore" => backup::run_restore(rest),
        "" | "help" | "--help" | "-h" => {
            print!("{HELP}");
            Ok(())
        }
        other => Err(format!("unknown golden command: {other}\n\n{HELP}")),
    }
}
