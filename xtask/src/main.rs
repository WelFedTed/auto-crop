// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Repository automation. Run as `cargo xtask <command>`.

mod dco;
mod deny_selftest;
mod deps;
mod doctor;
mod identity;
mod licenses;
mod profiles;
mod provenance;
mod roadmap;

use std::process::ExitCode;

const HELP: &str = "\
cargo xtask <command>

Commands:
  roadmap-check [--write] [--baseline <git-ref>]
        Validate ROADMAP.md (checkbox syntax, IDs, GATE lines, progress table).
        --write regenerates the progress table; --baseline <ref> also fails if
        an ID present in <ref> was dropped without being listed in
        docs/roadmap/retired.txt.
  check-deps
        Enforce crate dependency direction rules (core, imgproc, cli, shell).
  check-identity
        Fail if any io.github.* identifier other than the one in identity.toml appears.
  doctor
        Check the native build toolchain (git, CMake, Ninja, NASM, Node, C/C++) and print
        install commands for what is missing.
  provenance [--render]
        Validate the provenance log; --render regenerates the CSV/MD views (otherwise they
        must be up to date).
  licenses --check
        Require about.toml accepted licences to equal the deny.toml allow-list.
  check-dco [<rev-range>]
        Require a matching Signed-off-by trailer on every non-merge commit in the range
        (default HEAD^..HEAD); bot authors are skipped.
  deny-selftest
        Run cargo-deny over planted AGPL/GPL/non-commercial/banned fixtures; they must
        fail while the clean control passes.
  check-profiles
        Fail on panic = \"abort\" and require panic = \"unwind\" in release.
";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let cmd = args.next().unwrap_or_default();
    let rest: Vec<String> = args.collect();
    let result = match cmd.as_str() {
        "roadmap-check" => roadmap::run(&rest),
        "check-deps" => deps::run(&rest),
        "check-identity" => identity::run(&rest),
        "doctor" => doctor::run(&rest),
        "provenance" => provenance::run(&rest),
        "licenses" => licenses::run(&rest),
        "check-dco" => dco::run(&rest),
        "deny-selftest" => deny_selftest::run(&rest),
        "check-profiles" => profiles::run(&rest),
        "" | "help" | "--help" | "-h" => {
            print!("{HELP}");
            Ok(())
        }
        other => Err(format!("unknown xtask command: {other}\n\n{HELP}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
