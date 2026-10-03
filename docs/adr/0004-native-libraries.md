# 0004 - Native libraries: pinned CMake builds, libde265 as a plugin, libjpeg-turbo pin

- **Status:** accepted
- **Date:** 2026-10-01
- **Roadmap items:** M0.39, M0.40, M0.41, M0.42, M0.44, M0.45, M1.18 (update below)
- **Decision log links:** B12, B2, D4, D6
- **Time box:** 4 days (PROVISIONAL, with the HEIC binding in ADR-0005); actual about a day of CI iteration. Code: `native-deps.toml`, `xtask build-native`, `xtask check-native`, `.github/workflows/native.yml`, `spikes/turbojpeg/`.

## Context

HEIC/HEIF, JPEG scaled decode and lossless transforms need C libraries. Decision B12 bundles libde265 with a decode-only libheif as separate, dynamically linked, replaceable libraries; no x265/x264; no `libheif-sys` embedded mode and no vcpkg. The `turbojpeg` crate vendors an old libjpeg-turbo (3.1.0, which has a known double free fixed in 3.1.4), so we link our own.

## Decision

- **One manifest, hash-pinned.** `native-deps.toml` pins libheif 1.23.5, libde265 1.1.3, libjpeg-turbo 3.2.0, dav1d 1.5.4, libjxl 0.12.0, libwebp 1.6.0 and ONNX Runtime 1.28.2 (three prebuilt platform archives) by URL and SHA-256 with licence, build system and flags. `cargo xtask build-native` downloads with `curl`, **refuses any file whose SHA-256 differs** (unit-tested), extracts with `tar` and builds libde265, libjpeg-turbo and libheif with CMake into `target/native/prefix`. Version floors (libheif >= 1.23.5, libde265 >= 1.1.3, libjpeg-turbo >= 3.1.4) are enforced by the manifest check.
- **libheif is decode-only.** Documentation, examples, gdk-pixbuf and libsharpyuv are off, and **every encoder and every unneeded codec is switched off explicitly** (`WITH_X265`, `WITH_X264`, `WITH_KVAZAAR`, VVC, AOM, rav1e, SVT, JPEG, OpenJPEG, FFmpeg ...). libheif's defaults enable the GPL x264 and x265 encoders if they are found on the machine, so the explicit switches are what keeps B12's "never x265/x264" true.
- **libde265 is a separate plugin on every OS** (`WITH_LIBDE265_PLUGIN=ON`): `heif-libde265.dll` in `lib/libheif/` on Windows, the corresponding `libheif-libde265` plugin under `lib/libheif/` on Linux and macOS. libheif loads it in `heif_init` from its default plugin directory and from `LIBHEIF_PLUGIN_PATH`. Linked directly would also work (CMake option), but the plugin layout keeps libde265 replaceable (LGPL relinking) and gives a clean `no-hevc` variant: simply do not ship the plugin.
- **`check-native` enforces it** on the built libraries: no `x265_`/`x264_` symbols, every dependency on `packaging/allowed-libs.txt` (system and runtime libraries plus our own), libjpeg-turbo's `jconfig.h` at or above 3.1.4, and no `embedded-libheif` anywhere. CI proves it with two **negative tests**: a library with a planted `x265_` symbol is rejected, and a copy of the install with `jconfig.h` edited to version 3.1.0 is rejected, each for the right reason. (The 3.1.0 case is a header edit, not a real 3.1.0 build.)
- **libjpeg-turbo override.** We build our own libjpeg-turbo with CMake (NASM required on x86, `REQUIRE_SIMD=ON`, position-independent code) and link it through our own thin TurboJPEG 3 binding. The spike's build script reads `TURBOJPEG_VERSION_NUMBER` and refuses to link anything below 3.1.4. Fallback if the C build ever becomes unacceptable: `zune-jpeg` (pure Rust, but no DCT-domain scaling).

## Results

All on GitHub-hosted runners, 2026-10-01. Required = Windows, macOS, Linux (B9).

| Cell | libde265 + libheif + libjpeg-turbo build | check-native | libjpeg-turbo spike | HEIC probe (ADR-0005) |
|---|---|---|---|---|
| windows-2025 x64 (required) | pass | pass | pass | pass |
| macos-latest arm64 (required) | pass | pass | pass | pass |
| ubuntu-22.04 x64 (required, glibc floor) | pass | pass | pass | pass |
| ubuntu-24.04 x64 | pass | pass | pass | pass |
| windows-11-arm | pass | pass | pass | pass |
| ubuntu-22.04-arm | pass (needs the job cap `AUTOCROP_BUILD_JOBS=2`; unlimited parallelism got the runner killed mid-compile) | pass | pass | pass |
| fedora (digest-pinned container) | pass after one fix: the bundled static zlib inside libjpeg-turbo 3.2.0 was linked into the shared library without `-fPIC` (Ubuntu's gcc default hid it); fixed with `CMAKE_POSITION_INDEPENDENT_CODE=ON` | pass | not run (build and check only) | not run (build and check only) |
| Intel Mac (`macos-26-intel`) | build smoke only (nightly workflow), no native build yet | UNMEASURED | UNMEASURED | UNMEASURED |

`heif_security_limits` (the decoder safety limits API) is installed with libheif 1.23.5 (`heif_security.h`).

**libjpeg-turbo spike results (all three required OSes plus the extra cells, identical):**

- 1/4 scaled decode: dimensions 64x48 give 16x12; mean absolute difference to a 4x4 box filter of the full decode is 0.64 levels.
- Lossless 90-degree rotate into a **caller-owned buffer** (`TJPARAM_NOREALLOC`): the library writes into our buffer, and a too-small buffer is an error, not a reallocation. One rotation decodes within 2 levels of rotating the decoded pixels (integer inverse-DCT rounding is not perfectly symmetric); **four rotations return the exact original pixels**, which proves no coefficient was lost.
- MCU-aligned crop (4:2:0, 16x16 MCU): exact against the same region of the decoded original.
- `PERFECT` rotation of a 70x50 image (not MCU aligned) is refused: `Transform is not perfect`.

## Consequences

- **Engine policy:** a 90-degree rotate or crop is lossless only when the image dimensions (and the crop origin) are MCU-aligned. Otherwise the engine must either trim the partial edge MCUs, with the user's consent, or fall back to a re-encode at the policy quality (PLAN 3.8, 02 §2.7). This matches the plan's "lossless fast path for MCU-aligned operations".
- **M6** promotes this build to the release recipe (Windows installer layout, plugin directory next to the executable, `LIBHEIF_PLUGIN_PATH` or a relative plugin path, notices for LGPL relinking). The `no-hevc` variant is the same build without the libde265 plugin.
- **Security:** libheif and libde265 advisories are tracked by the daily security workflow and the `native-deps.toml` watch (M0.10, still open); the SLA is in PLAN 8.2.3.
- **Open items:** ONNX Runtime, dav1d, libjxl and libwebp are pinned but not yet built (M4, M6, M11). `heif-libde265` plugin discovery on a relocated install is a packaging item (M6). Intel Mac native builds are best-effort until 1.0.
- **Revisit trigger:** a libheif or libde265 security release (bump the pin), or a maintained pure-Rust HEVC decoder (re-evaluate in about six months, D4).

## Update 2026-10-03 (M1.18): the `turbojpeg` crate or our own FFI

M1.18 was written as "enable the `turbojpeg` 1.5.x crate". Before wiring it into `codecs` the crate was read against the questions that matter here (sources of `turbojpeg` 1.5.1 and `turbojpeg-sys` 1.2.0 from crates.io; not built locally because the only place the native library is built is CI):

| Question | `turbojpeg` 1.5.1 + `turbojpeg-sys` 1.2.0 | Own `ffi/` module (this repo) |
|---|---|---|
| Finds our pinned build without pkg-config or vcpkg on Windows? | Only through `TURBOJPEG_SOURCE=explicit` plus `TURBOJPEG_LIB_DIR`, `TURBOJPEG_INCLUDE_DIR` and `TURBOJPEG_DYNAMIC=1` (otherwise it links statically), set in the environment of every cargo invocation (IDE, `cargo test`, CI). The default features (`cmake`, `pkg-config`) build the vendored copy or ask pkg-config | `build.rs` reads `AUTOCROP_NATIVE_PREFIX`, default `target/native/prefix`, so `cargo test --features turbojpeg` works after `cargo xtask build-native` with no environment |
| Which libjpeg-turbo can it end up with? | The vendored source is **3.1.0** (`set(VERSION 3.1.0)` in the bundled `CMakeLists.txt`), the release with the known double free; a missing variable silently builds it. It has no version check at all | `build.rs` refuses anything below 3.1.4 (`jconfig.h` and `turbojpeg.h` both read) and never builds or searches anything else |
| Licence | Unlicense OR MIT (acceptable), bundles 7 MB of libjpeg-turbo source in the crate | no new crate |
| `unsafe` | about 30 `unsafe` sites in the crate (third party, so outside `ci-guards`) | about 20 sites, all in `crates/codecs/src/turbo/ffi/` with `// SAFETY:` comments, checked by `ci-guards` |
| API coverage for our needs | scaled decode, `tj3Transform` into a caller slice, compress: yes | the same dozen functions, plus `tj3SetICCProfile` and the density parameters |
| Constants | pregenerated from the 3.1.0 header | checked against the pinned `turbojpeg.h` by a unit test (`constants_match_the_pinned_header`); all of them also agree with the crate's pregenerated 3.1.0 bindings |

**Decision: keep the hand-written binding** (as ADR-0004 and ADR-0005 already say for libheif). The crate would still need our own build script for the version floor, an environment dance on every machine, and would carry the vulnerable source in the tree; the binding is about 15 functions that CI has exercised on three operating systems since the M0.44 spike. The cost is first-party `unsafe` (confined to one directory, SAFETY-commented, guarded) and the `codecs` crate lint moving from `forbid(unsafe_code)` to `deny` with a single `#![allow(unsafe_code)]` inside `ffi/`. The default build (`turbojpeg` feature off) compiles no `unsafe` and needs no C toolchain.

Revisit if the crate gains a version floor and a prefix-aware build (or a maintained fork does), or if the binding grows beyond about 25 functions.

The version check is enforced three ways, each with a negative test: `xtask check-native` (header edited to 3.1.0 is rejected, `native.yml`), `build.rs` (`cargo build --features turbojpeg` against a header edited to 3.1.0 fails with "older than 3.1.4", `turbojpeg.yml`, unit-tested in `native_header.rs`), and the manifest check (`native-deps.toml` below its `min_version` is rejected). A real libjpeg-turbo 3.1.0 build is not exercised: the pin is 3.2.0, and an edited header proves the gate, not the library.

## Update 2026-10-04: dav1d is built and linked (ADR-0009)

`cargo xtask build-native` now also builds **dav1d 1.5.4** (`build = "meson"`, shared; needs meson, ninja and NASM on x86) before libheif, which is configured with `WITH_DAV1D=ON` and no other AV1 codec. `check-native` allows `dav1d` and fails on AV1 encoder symbols (`aom_codec_av1_cx`, `rav1e_context_new`, `svt_av1_enc_init`) and on a libheif that does not link dav1d (a unit-tested rule plus a planted-symbol negative test in `native.yml`). The AVIF decoder is therefore no longer an "open item"; ONNX Runtime, libjxl and libwebp still are.
