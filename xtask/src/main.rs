// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Repository automation. Run as `cargo xtask <command>`.

mod alloc_count;
mod check_native;
mod dco;
mod deny_selftest;
mod deps;
mod doctor;
mod eval;
mod hostile;
mod identity;
mod licenses;
mod native;
mod native_watch;
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
  check-native [--prefix <dir>]
        Inspect the built native libraries: no x265/x264 symbols, dependencies on the
        allow-list, libjpeg-turbo >= 3.1.4, no embedded-libheif.
  doctor
        Check the native build toolchain (git, CMake, Ninja, NASM, Node, C/C++) and print
        install commands for what is missing.
  provenance [--render]
        Validate the provenance log; --render regenerates the CSV/MD views (otherwise they
        must be up to date).
  build-native [--only a,b]
        Fetch the pinned native libraries (SHA-256 verified, wrong hash refused) and build
        libde265, libjpeg-turbo and libheif with CMake into target/native/prefix.
  native-watch [--fail-on-stale] [--fake name=version]
        Compare each pinned native library with the newest upstream release and print STALE lines.
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
  make-hostile [--out <dir>] [--only <substring>] [--no-run]
        Generate the hostile-file corpus (60000x60000 and 100 MP headers, zlib bombs, IFD
        floods, truncations, cyclic EXIF) and decode each file in a subprocess; fails on a
        panic, abort, hang, or a time or heap budget miss. Files go to target/hostile.
  synth --suite smoke|full [--out DIR] [--seed N] [--count N] [--max-edge N]
        Write a STAND-IN synthetic suite (images + manifest.jsonl) under target/synth/<suite> by
        default. Never committed. Wraps `auto-crop-eval synth`.
  eval <auto-crop-eval args>
        Run the accuracy harness (run, compare, noise-floor, publish, validate-manifest,
        self-check). Example: cargo xtask eval self-check
";

#[global_allocator]
static ALLOC: alloc_count::Counting = alloc_count::Counting;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let cmd = args.next().unwrap_or_default();
    let rest: Vec<String> = args.collect();
    let result = match cmd.as_str() {
        "roadmap-check" => roadmap::run(&rest),
        "check-deps" => deps::run(&rest),
        "check-identity" => identity::run(&rest),
        "check-native" => check_native::run(&rest),
        "doctor" => doctor::run(&rest),
        "provenance" => provenance::run(&rest),
        "licenses" => licenses::run(&rest),
        "build-native" => native::run(&rest),
        "native-watch" => native_watch::run(&rest),
        "check-dco" => dco::run(&rest),
        "deny-selftest" => deny_selftest::run(&rest),
        "check-profiles" => profiles::run(&rest),
        "make-hostile" => hostile::run(&rest),
        "hostile-run" => hostile::run_child(&rest),
        "synth" => eval::run_synth(&rest),
        "eval" => eval::run_eval(&rest),
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
