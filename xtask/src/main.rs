// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Repository automation. Run as `cargo xtask <command>`.

mod alloc_count;
mod check_native;
mod ci_guards;
mod corpus;
mod dco;
mod deny_selftest;
mod deps;
mod doctor;
mod eval;
mod golden_set;
mod hostile;
mod identity;
mod label;
mod licenses;
mod native;
mod native_watch;
mod perf;
mod profiles;
mod provenance;
mod register;
mod roadmap;
mod synth;

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
  doctor [--strict]
        Check the native build toolchain (git, CMake, Ninja, NASM, Node, C/C++) and print
        install commands for what is missing, never install. Also checks the M1 dev tools
        (Valgrind on Linux, Python 3.12+, Tesseract 5.x, ImageMagick, unpaper); with --strict a
        missing dev tool fails too (the devcontainer uses that).
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
  ci-guards [--selftest]
        CI policy guards: `unsafe` only in ffi/ and simd/ module directories with // SAFETY:
        comments, no HTTP/TLS/socket crates in the shipped build (B18, C4), and workflow hygiene
        (SHA pins, least privilege, no pull_request_target, no path-filtered PR triggers, no
        release machinery). --selftest runs the planted violations, which must all be detected.
  make-hostile [--out <dir>] [--only <substring>] [--no-run]
        Generate the hostile-file corpus (60000x60000 and 100 MP headers, zlib bombs, IFD
        floods, truncations, cyclic EXIF) and decode each file in a subprocess; fails on a
        panic, abort, hang, or a time or heap budget miss. Files go to target/hostile.
  fetch-corpus [--sample] [--lock FILE] [--cache DIR] [--no-ingest] <name>
  fetch-corpus --record-hash [--sample] <name>
  fetch-corpus --list
        Download a public corpus pinned in corpus.lock.toml (URL, size, SHA-256, SPDX licence,
        attribution), verify it (a mismatch is refused and the file deleted), extract it into
        the cache OUTSIDE the repository ($AUTOCROP_CORPUS_CACHE, --cache, else a per-user
        cache directory) and write harness manifests next to the data. --sample picks the small
        sample file of corpora that have one. A lock entry whose size or sha256 is the
        placeholder TODO-first-fetch, or whose licence is not cleared, is refused with the reason.
        --record-hash downloads into a quarantine, prints the size and SHA-256 to paste into the
        lock (trust on first use), extracts nothing and deletes the file. Uses `curl`.
        Adapter options: [--every N] [--scene-by clip|document] [--dev-percent N]
        [--contact-sheet N]. See docs/testing/corpora.md.
  corpus-ingest <name> --src DIR [--out DIR] [--lock FILE] [adapter options]
        Run only the manifest adapter over an already extracted tree.
  perf <host|gen|stages|memory|batch>
        Performance harness for the pipeline skeleton (M1.60, M1.63): per-stage p50/p95 against
        the PROVISIONAL budgets, peak heap, batch throughput. Release builds only:
        cargo run --release -p xtask -- perf stages --mp 12
        (stages [--mp 12,48,100] [--runs N] [--threads all|N] [--json F]; memory; batch
        [--count 200] [--workers 1,..,8]; gen; host. Run `perf` alone for the full list.)
  synth --suite smoke|full [--generator python|rust] [--out DIR] [--seed N] [--count N]
        [--max-edge N] [--jobs N] [--truth none|text|full]
        Write a synthetic suite (images + manifest.jsonl) under target/synth/<suite>; never
        committed. The default generator is the Python tool in tools/synth (known-text pages and
        receipts, pinhole camera, procedural backgrounds, Augraphy degradations, JPEG/PNG/TIFF/WebP,
        EXIF 1-8, sRGB and Display P3). --generator rust is the old STAND-IN writer in
        auto-crop-eval (output under target/synth/<suite>-rust).
  synth-setup
        Create target/synth-venv and install tools/synth/requirements.lock with
        --require-hashes (needs Python 3.12 or newer). Nothing else installs Python packages.
  synth-check [tests|geometry|variants|ocr|all] [args]
        The generator's acceptance checks: Python unit tests, inverse-warp SSIM of the ground
        truth, every format x EXIF x colour-space variant decoded by the Rust decoders, and the
        Tesseract CER on the clean render (skipped, loudly, where tesseract is missing; pass
        `ocr --require` to fail instead).
  check-splits [MANIFEST...]
        Fail when a scene_id, scene_seed or similar group appears in two splits (default:
        every target/synth/*/manifest.jsonl).
  check-labels <labels dir> [--images DIR] [--strict] [--no-hash]
        Validate golden-set label files (docs/testing/golden-label.schema.json): finite, simple,
        clockwise, convex quads; slice tags; no duplicate ids; labels without an image and images
        without a label (the latter an error only with --strict).
  label <image dir> [--labels DIR] [--open] [--port N] [--annotator NAME] [--no-suggestions]
        The blank-quad labeller: a local page on 127.0.0.1 with a per-run token (offline, no
        dependencies). Autosaves one JSON label per image. Detector suggestions are off unless the
        labeller turns them on, and such labels are marked `assisted`. See docs/testing/golden-workflow.md.
  golden <status|lock|check|eval|report|backup|restore-check|restore> ...
        The local golden-set workflow (splits lock, local evaluation, aggregate-only report, backup).
        `cargo xtask golden help` lists the options.
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
        "ci-guards" => ci_guards::run(&rest),
        "make-hostile" => hostile::run(&rest),
        "hostile-run" => hostile::run_child(&rest),
        "fetch-corpus" => corpus::run_fetch(&rest),
        "corpus-ingest" => corpus::run_ingest(&rest),
        "perf" => perf::run(&rest),
        "synth" => synth::run_synth(&rest),
        "synth-setup" => synth::run_setup(&rest),
        "synth-check" => synth::run_check(&rest),
        "check-splits" => synth::run_check_splits(&rest),
        "check-labels" => golden_set::check::run_check_labels(&rest),
        "label" => label::run(&rest),
        "golden" => golden_set::run(&rest),
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
