# 0007 - Inference backend: ONNX Runtime via `ort` (load-dynamic), rten as the no-runtime fallback

- **Status:** accepted for M0 (revisit trigger below); the owner may veto
- **Date:** 2026-10-01
- **Roadmap items:** M0.33, M0.35 (M0.32 and M0.34 stay open, see "Not shown")
- **Decision log links:** B7, B2, B12
- **Time box:** 4 days (PROVISIONAL); actual about one day. Code: `spikes/inference/` (throwaway, outside the workspace). CI: `.github/workflows/inference.yml`.

## Context

B7 uses small bundled ONNX models (page-corner heatmaps, document orientation). M0 must choose the runtime that executes them on all three OSes and four architectures, and record what each candidate costs.

The spike uses **two stand-in nets with deterministic random weights** (no weights are downloaded or trained, per the model-weights policy): a MobileNetV3-small-like backbone with FPN and a 4-channel 64x64 corner head at 1x3x256x256 (stands in for DocQuadNet-256), and a PP-LCNet-like depthwise-separable classifier at 1x3x224x224 (stands in for the document-orientation model). Both are exported at opset 17 with BatchNorm folded, and quantised to static QDQ int8 with ONNX Runtime's own tool. Operators exercised: Conv (incl. depthwise), HardSwish, HardSigmoid, Relu, GlobalAveragePool, Mul, Add, Resize, Sigmoid, Softmax, Flatten, plus QuantizeLinear/DequantizeLinear. Random weights are fine for operator coverage, latency, size and backend agreement; they are NOT fine for accuracy.

Candidates, exact pins: `ort =2.0.0-rc.13` (ONNX Runtime 1.28), `rten =0.26.0`, `tract-onnx =0.23.8`.

## Results

Median ms per 256x256 pass, 4 threads, 30 timed runs after a warm-up (CI runners are shared and noisy; macOS runners showed p95 up to 320 ms). The local Windows run is an i7-8700K (6C/12T, AVX2, 32 GB), which qualifies as the Tier-M machine.

| Cell | quadnet fp32 ort / rten / tract | quadnet int8 ort / rten / tract |
|---|---|---|
| Windows x64, local i7-8700K | 2.8 / 6.7 / 17.7 | 3.9 / 8.3 / 25.9 |
| Windows x64 (windows-2025) | 6.6 / 13.4 / 20.6 | 6.7 / 17.6 / 32.9 |
| Windows ARM64 (windows-11-arm) | 7.0 / 8.6 / 15.9 | 5.7 / 14.1 / 21.8 |
| Linux x64 (ubuntu-22.04) | 3.5 / 9.7 / 13.8 | 3.8 / 12.2 / 23.2 |
| Linux ARM64 (ubuntu-22.04-arm) | 5.7 / 8.0 / 13.4 | 3.8 / 10.3 / 17.4 |
| macOS arm64 (macos-latest) | 11.3 / 12.4 / 23.4 | 4.6 / 25.6 / 29.5 |
| macOS x86_64 (macos-26-intel) | n/a / 16.2 / 21.3 | n/a / 22.3 / 35.6 |

The orientation net shows the same ordering (ort 2-7 ms, rten 4-9 ms, tract 11-21 ms at fp32). One thread: ort roughly 2x slower than at 4 threads, rten about 3x, tract barely scales (17.7 ms at 4 threads vs 19.9 at 1 on Windows).

| Property | ort (ONNX Runtime) | rten | tract |
|---|---|---|---|
| Loads and runs both nets, fp32 and int8, on every OS/arch tried | yes, except Intel Mac (no runtime) | yes, all cells | yes, all cells |
| fp32 max abs difference vs ONNX Runtime | reference | 4e-7 | 5e-7 |
| Corner peak difference vs ONNX Runtime (bar 0.1 px) | reference | <= 4e-5 px | <= 4e-5 px |
| int8 max abs difference vs ONNX Runtime int8 | reference | 0.011 (quadnet), 0.004 (orient) | 0.011 (quadnet), **0.757 on the orientation softmax** |
| Cold load | 25-100 ms | 3-14 ms | 36-205 ms |
| Binary (harness, one backend) | 0.75 MB exe + runtime library when loaded dynamically (Windows runtime DLL 15.8 MB) | 4.3-8.0 MB | 21-47 MB |
| Native dependency | yes (the pinned runtime) | none | none |

Findings that matter:

1. **tract's int8 is wrong on the orientation net** (softmax output off by 0.75 against both ONNX Runtime int8 and fp32), and it is slower in int8 than in fp32. It also scales poorly with threads and is the largest binary. tract is **rejected**.
2. **rten matches ONNX Runtime closely** (fp32 4e-7; int8 within one quantisation step) and is 2-4x slower, still far inside the PROVISIONAL bar of 25 ms median at 4 threads on Tier-M (6.7 ms measured locally). It is pure Rust, loads in milliseconds, and needs no native runtime.
3. **ort's default `download-binaries` mode is not usable for us:** on Ubuntu 22.04 the downloaded static runtime fails to link (`undefined reference ... _M_replace_cold`, needs a newer libstdc++); on Windows it statically links the runtime (21.6 MB exe) and copies a 18.5 MB `DirectML.dll`; and the files are not pinned by our hash policy (D4). The **`load-dynamic` feature with our own SHA-256-pinned runtime** (the `native-deps.toml` archives) works on Linux x64/ARM64 and Windows, and the Linux x64 archive hash matched the pin in CI.
4. **`load-dynamic` without an explicit path found a system `onnxruntime.dll` on Windows** (Windows ships one). The product must load the runtime by **absolute path** next to the executable and verify the version, never by search path (DLL planting and version-skew risk). Recorded as a requirement for M4.
5. **Intel Mac has no ONNX Runtime binary** (Microsoft's 1.28.2 release has no `osx-x86_64` asset, and Python `onnxruntime` has no macOS x86_64 wheels). rten and tract run there (rten quadnet 16 ms median).
6. Windows ARM64 works through ort's own download and Microsoft publishes `onnxruntime-win-arm64-1.28.2.zip`; Linux ARM64 works with `onnxruntime-linux-aarch64-1.28.2.tgz` (SHA-256 seen in CI: `f020b3d31106cc7db03889b4a5c21e7c38ce4a09ad26119c11d1ad6d3fa0ec04`, **not yet pinned**, to be added to `native-deps.toml` in M4). `windows-11-arm` and `ubuntu-22.04-arm` rows are best-effort cells.

## Decision

**GO: ONNX Runtime through `ort =2.0.0-rc.13` with `load-dynamic`** behind an `InferenceBackend` trait, **CPU by default**, running inside the sandboxed worker pool (ADR-0006). Reasons: it is the fastest everywhere, the reference for numeric agreement, supports any future model (including dewarping nets) and is the only candidate with opt-in GPU providers (CoreML, DirectML, CUDA, after their own benchmarks).

- **Fallback behind the same trait: rten** where no runtime is available (Intel Mac, which is best-effort until 1.0 per ADR-0004) or the pinned runtime fails its version check. Same models, same `models.lock` hashes; the golden-set gate must pass on every backend that ships, and the app reports which one ran.
- **Pins:** `ort =2.0.0-rc.13`, ONNX Runtime 1.28.2 from `native-deps.toml` (SHA-256 checked), `rten =0.26.0`. `tract` is not used.
- **Models:** only hash-checked models listed in `models.lock` (provenance policy); no network fetch.
- **Packaging:** the runtime library ships next to the executable and is loaded by absolute path with a version check (finding 4); default ort features `download-binaries`, `copy-dylibs` and `tls-native` stay off.
- **Revisit trigger:** if, with the real DocQuadNet-class model in M4, rten stays within 2x of ONNX Runtime and covers every operator, drop ONNX Runtime from the default build (one pure-Rust backend, no native runtime to sandbox, ship, or patch) and keep it as an opt-in GPU path. The `ort` crate is a release candidate; if it stalls before 2.0.0, that trigger fires too.

## Not shown (UNMEASURED, not passes)

- **M0.32 stays open:** the nets are stand-ins, `paddle2onnx` on a real PP-LCNet model was not run (PaddlePaddle is not installed), and int8 outputs differ from ONNX Runtime by 0.01, above the 1e-3 bar (a quantisation-rounding difference, not a failure, but the stated bar is not met).
- **M0.34 stays open:** the int8 accuracy bars (p95 corner-error rise, IoU delta) need a trained model and labelled images; random weights make argmax unstable (int8 vs fp32 peak differences of 60-128 px are an artefact of near-flat random heatmaps, not a result). Windows ARM64 and Intel Mac were rated for "works" only.
- Apple silicon numbers come from a noisy shared runner; the Tier-M bar is judged on the local i7-8700K only.
- Opt-in GPU providers were not benchmarked.

## Update 2026-10-05 (M1.55): the backend trait, the loader and the stand-ins in the workspace

- **Code:** `crates/infer` (`auto-crop-infer`): `InferenceBackend` trait, `OrtBackend` (feature `ort`, `ort =2.0.0-rc.13`, `default-features = false`, `load-dynamic`), `RtenBackend` (feature `rten`, `rten =0.26.0`). Both features are off by default; the default build has no ONNX Runtime, rten or imageproc code. Engine and CLI forward them as `standin-ort`, `standin-rten`, `standin-canny`.
- **Loader (finding 4 turned into code):** the runtime is loaded by **absolute path** only (`AUTOCROP_ORT_DYLIB`, else next to the executable); a bare name or relative path is refused. Before anything is loaded the file's SHA-256 must equal the pinned library inside the archive of `native-deps.toml` (`runtime::PINS`; ort's own check is only `>=` the minor version). Verified on the Windows host: the system `C:\Windows\System32\onnxruntime.dll` is **refused**, a pinned library with one byte changed is refused, a second different path after a load is refused, and `ORT_DYLIB_PATH` is never consulted. `ort::init_from` is called with the verified path and sessions use `commit_from_memory` on bytes that were hashed (`VerifiedModel`).
- **Fetching:** `cargo xtask fetch-ort` downloads the host's archive through the same SHA-256-verified downloader as `build-native`, refuses a wrong hash, checks the library inside against the loader pin and installs it into `target/native/prefix/lib`. It needs no compiler (works on the Windows dev machine without CMake).
- **Stand-in net:** `cargo xtask make-standin-net` runs `spikes/inference/gen_models.py --only-quadnet` (deterministic random weights, 2,694,177 bytes with onnx 1.23.1) into `target/standin` with a sidecar SHA-256; never committed.
- **Result:** ort and rten agree to 3.9e-7 on the stand-in net (bar 1e-3); numbers in [budgets.md](../perf/budgets.md), all STAND-IN. Pins added for the three required OSes; Windows ARM64, Linux ARM64 and Intel Mac still have none (rten there).
- **Dependency checks recorded:** `ort` (MIT OR Apache-2.0) with `default-features = false` pulls no HTTP or TLS crate (`ureq`, `native-tls`) into the graph, passes `cargo deny check` and the network guard, and a new `inference_guard` in `cargo xtask ci-guards` fixes its feature set (see docs/policy/ci-guards.md section 7). `rten` is MIT OR Apache-2.0, pure Rust.
