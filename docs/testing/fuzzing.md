# Fuzzing: cargo-fuzz targets, CI and crash replay

Roadmap: M1.70 (targets), M1.71 (CI and replay); later targets are M2.61 and M6.63, and X.21 is the standing cadence. Code: `fuzz/` (own `[workspace]`, never shipped), workflow `.github/workflows/fuzz.yml`, replay test `xtask/tests/fuzz_regressions.rs`.

## Targets

Each target is a function `fn(&[u8])` in `fuzz/src/lib.rs`; `fuzz/fuzz_targets/<name>.rs` only forwards libFuzzer's input to it, so libFuzzer and the replay test run identical code. A function **panics** when it finds a bug: a panic anywhere below it (a decoder panic that the guard caught comes back as `CodecError::InternalPanic` and is re-raised by the target), a violated invariant, or a result that contradicts the limits it was given.

| Target | Input | Checks |
|---|---|---|
| `probe` | any bytes | `probe_with` under the default limits never panics, is deterministic, and agrees with `sniff` (format, non-zero size, orientation 1..=8, at least one frame and scan); the EXIF reader on the raw bytes returns 1..=8 or nothing |
| `limits` | 8 bytes choosing a `DecodeLimits` (pixels up to 262,144; metadata, scans, frames, file and memory caps from 0 to generous), then a file | probe and full decode under those limits: no panic; a successful decode respects every cap (pixels, file size, frames, ICC size), its raster has `w*h*3` bytes, and probe and decode agree on format, orientation and size |
| `metadata` | a carrier byte, then an EXIF, ICC or XMP blob | the blob is wrapped in a valid 8x8 JPEG (APP1 Exif, APP2 ICC, two-chunk ICC, APP1 XMP), PNG (`eXIf`, `iCCP` with a valid and with a raw stream), WebP (`EXIF`, `ICCP`) or TIFF (ICC tag, orientation); probe and decode under the default and a 64-byte metadata cap. This reaches ICC reassembly, `iCCP` inflation and the EXIF reader on every execution |
| `editstate_json` | any bytes | `serde_json::from_slice::<EditState>` and `engine::migrate::migrate_str` never panic and never return `ErrKind::Internal`; what migrate accepts serialises and migrates back to an equal state; `validate`, `render_hash` and the accessors never panic |

The libFuzzer inputs are capped at 1 MiB (`-max_len`). AddressSanitizer is cargo-fuzz's default and is on for all targets, but it only matters where there is `unsafe` or C: the codecs are `forbid(unsafe_code)` and every decoder in the tree is safe Rust, so today ASan adds nothing beyond cargo-fuzz's debug assertions and overflow checks (a dependency's `unsafe` SIMD code, such as zune-jpeg's AVX2 IDCT, is covered). **The turbojpeg targets (ROADMAP M1.18, M1.19) do not exist at the time of writing**: when the `turbojpeg` feature lands in `crates/codecs`, add it to `fuzz/Cargo.toml` (enabled on the `limits` and `metadata` targets, which then decode through it) and rerun the 5-minute acceptance with ASan, which is where the sanitizer starts to matter.

## Seeds

`cargo run --manifest-path fuzz/Cargo.toml --bin make-seeds -- fuzz/seeds` writes about 420 seeds (nothing binary is committed): the generated fixtures of every format (`codecs::fixtures`: baseline, progressive, 4:2:0, CMYK, restart, ICC and EXIF JPEGs; PNG in every colour type, Adam7, APNG; TIFF with each compression, BigTIFF, G4; WebP lossless, lossy, extended, animated; the recognised-only stubs), the hostile corpus of M1.69 (files over 1 MiB are skipped), the EXIF and ICC blobs inside every carrier, and the `EditState` fixtures of the engine. `cargo test -p xtask --test fuzz_regressions` replays all seeds on every OS.

## Fuzzing without libFuzzer (Windows, macOS)

`cargo run --release --manifest-path fuzz/Cargo.toml --no-default-features --bin mutate -- <rounds> <seed> [target]` mutates every seed `rounds` times (bit flips, byte and field edits, inserts, deletes, truncation, splices, interesting values) and runs the entry functions. It has no coverage feedback, so it is a smoke test, but it found the first bug below in seconds on Windows before the first CI run. Failing inputs go to `fuzz/artifacts-local/`.

## CI (`.github/workflows/fuzz.yml`)

- **PR smoke**: 60 s per target in a matrix (one job each, always reporting). The path filter is a step inside the job (a diff against the base for `crates/codecs/`, `crates/core/`, `fuzz/`, the workflow itself); when nothing matches, the remaining steps are skipped and the job is green. Reason: a required check skipped by a `paths:` filter never reports (see [ci-guards](../policy/ci-guards.md) section 3).
- **Nightly**: 1 h per target; the corpus is restored from and saved to the Actions cache so the hours accumulate (the roadmap wants at least 72 h per target before 1.0; the run time of every job is in its log).
- **Manual**: `workflow_dispatch` with `seconds` (default 300, the M1.70 acceptance) and `targets`.
- The fuzzers build against the same dependency versions as the product: the workflow copies the root `Cargo.lock` to `fuzz/Cargo.lock` before building. Nightly is pinned to a date (`FUZZ_NIGHTLY`), cargo-fuzz to 0.13.
- A crash fails the job and uploads `fuzz/artifacts/` (the crashing input and its minimised form are in the log as hex and base64).

## From a crash to a test

1. Download the artifact (or take the base64 from the log) and reproduce: `cargo +nightly fuzz run <target> fuzz/artifacts/<target>/crash-...` on Linux, or call the entry function from a test on any OS.
2. Fix the bug in `crates/` with a unit test of its own where one fits.
3. Minimise (`cargo +nightly fuzz tmin <target> <file>`) and save the input as `fuzz/regressions/<target>/<what>-<sha256 first 8>`.
4. `cargo test -p xtask --test fuzz_regressions` replays it on Windows, macOS and Linux with the normal test run (`cargo nextest run --workspace`). The test also fails if `fuzz/fuzz_targets/`, the target table in `fuzz/src/lib.rs` or the `[[bin]]` entries drift apart, and a planted panicking target proves the replay reports failures.

## What the fuzzers found

| Date | Target | Finding | Fix |
|---|---|---|---|
| 2026-10-03 | `limits` (found with `mutate`) | zune-jpeg 0.5.15 panics (`Option::unwrap` on a short output row in its AVX2 IDCT, `src/idct/avx2.rs`; an `assert!` in the AVX2 upsampler for tiny sizes) on a sequential JPEG whose scans carry one component each when the image is subsampled (4:2:0). The panic was caught by the guard and reported as `InternalPanic`, so no data was at risk, but it violates the "0 panics" gate of M1.69 and means such a file could never decode | `parse/jpeg.rs` refuses a sequential subsampled JPEG with one scan per component as `UnsupportedFeature("non-interleaved subsampled JPEG ...")` before decoding (4:4:4 files with one scan per component decode correctly and are untouched); unit test `a_non_interleaved_sequential_jpeg_is_an_unsupported_feature_not_a_decoder_panic`; input in `fuzz/regressions/limits/` |
| 2026-10-03 | `editstate_json` (CI run 37117026606, first 5-minute run, 2.4 M executions) | `serde_json` without its `float_roundtrip` feature parses a 17-digit coordinate one ulp off, so `serialize(parse(x))` parsed again was not equal to `parse(x)`: a saved `EditState` could change when reloaded, and `render_hash` (which hashes the float bits) with it. Latent in `cargo build -p auto-crop-engine`, hidden whenever the `eval` crate (which enabled the feature) was built in the same invocation | `serde_json = { version = "1", features = ["float_roundtrip"] }` in the workspace dependencies; unit test `coordinates_survive_a_save_and_load_exactly` (red without the feature under `cargo test -p auto-crop-engine`); input in `fuzz/regressions/editstate_json/` |
