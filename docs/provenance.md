# Provenance register

Every tool, font, dataset and weight Auto Crop uses, with its SPDX licence, what it is used for and where it comes from. Roadmap: M1.74 (extended by M7.69, the dataset and tool licence register). Decisions: B2 (licences), B12 (native libraries), B21 (test data).

This register is written by hand and **checked by `cargo xtask provenance`** (CI job `repo checks`): every native library in `native-deps.toml`, every package pinned under `tools/*/requirements.txt`, every `@fontsource/*` font in `ui/package.json`, every entry of the provenance log and every crate banned in `deny.toml` must have a row here. A row is a table line whose first cell is the name in backticks.

It does not repeat what other documents own; follow the links instead:

| Topic | Where |
|---|---|
| Datasets and weights (licence URL, SHA-256, status per source) | the provenance log [docs/policy/provenance/](policy/provenance/README.md) (`provenance.jsonl` is the source of truth; from M1.75 the authoritative log moves to the `auto-crop-models` repo) |
| Weights rules (OSI licence, training-data statement, banned classes) | [docs/policy/model-weights.md](policy/model-weights.md) |
| Rust crates | `cargo deny check` gates the licences, `cargo about` writes `THIRD_PARTY_NOTICES.md` ([CI policy guards](policy/ci-guards.md)) |
| The private golden set | [docs/testing/golden-set.md](testing/golden-set.md) (never in this repository) |
| Native library pins and hashes | [native-deps.toml](../native-deps.toml) |

"Checked" says how a licence was established: **verified** means read from this repository's own files or from the package's published metadata on the date shown; **to verify** means taken from the project's public licence and still to be confirmed against the pinned release before it ships.

## In use today

### Native libraries (built from source from pinned, SHA-256-verified releases)

LGPL libraries are separate, dynamically linked, replaceable shared libraries (B12). None of the GPL encoders is ever built.

| Name | Kind | SPDX | Use | Source | Checked |
|---|---|---|---|---|---|
| `libde265` | native library | LGPL-3.0-or-later | HEVC decoding plugin for libheif; decoder only (B12) | https://github.com/strukturag/libde265 | verified 2026-10-02 (native-deps.toml) |
| `libheif` | native library | LGPL-3.0-or-later | HEIF and HEIC container decoding; decode-only build, no x265 | https://github.com/strukturag/libheif | verified 2026-10-02 (native-deps.toml) |
| `libjpeg-turbo` | native library | IJG AND BSD-3-Clause AND Zlib | fast JPEG decode, scaled decode, lossless transforms (M1); IJG acknowledgement in the third-party notices | https://github.com/libjpeg-turbo/libjpeg-turbo | verified 2026-10-02 (native-deps.toml) |
| `dav1d` | native library | BSD-2-Clause | AV1 decoding for AVIF; pinned, built from M6 | https://code.videolan.org/videolan/dav1d | verified 2026-10-02 (native-deps.toml) |
| `libjxl` | native library | BSD-3-Clause | JPEG XL; pinned, built from M11 | https://github.com/libjxl/libjxl | verified 2026-10-02 (native-deps.toml) |
| `libwebp` | native library | BSD-3-Clause | WebP encoding; pinned, built from M11 | https://github.com/webmproject/libwebp | verified 2026-10-02 (native-deps.toml) |
| `onnxruntime-win-x64` | native library (prebuilt) | MIT | model inference runtime for ort; not yet shipped | https://github.com/microsoft/onnxruntime | verified 2026-10-02 (native-deps.toml) |
| `onnxruntime-linux-x64` | native library (prebuilt) | MIT | as above | https://github.com/microsoft/onnxruntime | verified 2026-10-02 (native-deps.toml) |
| `onnxruntime-osx-arm64` | native library (prebuilt) | MIT | as above | https://github.com/microsoft/onnxruntime | verified 2026-10-02 (native-deps.toml) |

### Python packages for the dev oracles (`tools/imgproc-oracles`, `tools/ocr_oracle`, never shipped)

Pinned in `tools/imgproc-oracles/requirements.txt`, hash-locked in `requirements.lock` (Python 3.12 or newer). They generate the committed numeric oracle fixtures under `crates/imgproc/tests/fixtures/`; the fixtures are data computed by this project, not copies of any library.

| Name | Kind | SPDX | Use | Source | Checked |
|---|---|---|---|---|---|
| `numpy` | Python package | BSD-3-Clause AND 0BSD AND MIT AND Zlib AND CC0-1.0 | float64 reference Lanczos3 warp (`warp_numpy_lanczos3.json`) | https://pypi.org/project/numpy/ | verified 2026-10-02 (wheel metadata, 2.5.3) |
| `opencv-python-headless` | Python package | Apache-2.0 | homography and warp reference (`cv2.getPerspectiveTransform`, `cv2.warpPerspective`) used only to produce fixtures; the wheel bundles third-party libraries (including LGPL FFmpeg) that are never redistributed by this project | https://pypi.org/project/opencv-python-headless/ | verified 2026-10-02 (wheel metadata, 5.0.0.93) for the package; bundled libraries to verify |
| `pillow` | Python package | MIT-CMU | renders the synthetic receipts of the OCR oracle (`tools/ocr_oracle`, M1.66; pinned in `tools/ocr_oracle/requirements.txt`, hash-locked, Python 3.10 or newer); the images are generated at run time and never committed | https://pypi.org/project/pillow/ | verified 2026-10-03 (package metadata, 12.3.0) |

### Developer and CI tools (run, never linked, never shipped)

GPL-licensed tools below are executed as separate programs for measurement and comparison. Their code is not linked into, copied into or distributed with Auto Crop, and their output is numbers, not derived code (B2).

| Name | Kind | SPDX | Use | Source | Checked |
|---|---|---|---|---|---|
| `rust-toolchain` | compiler | MIT OR Apache-2.0 | build; pinned in `rust-toolchain.toml` | https://github.com/rust-lang/rust | to verify |
| `cargo-deny` | CI tool | MIT OR Apache-2.0 | licence, ban, source and advisory gate (`deny.toml`) | https://github.com/EmbarkStudios/cargo-deny | to verify |
| `cargo-about` | CI tool | MIT OR Apache-2.0 | `THIRD_PARTY_NOTICES.md` | https://github.com/EmbarkStudios/cargo-about | to verify |
| `cargo-audit` | CI tool | MIT OR Apache-2.0 | daily RustSec advisory check | https://github.com/rustsec/rustsec | to verify |
| `cargo-nextest` | test runner | MIT OR Apache-2.0 | workspace tests on three OSes | https://github.com/nextest-rs/nextest | to verify |
| `zizmor` | CI tool | MIT | workflow security lint (version pinned in `ci.yml`) | https://github.com/zizmorcore/zizmor | verified 2026-10-02 (package metadata, 1.30.1) |
| `reuse` | CI tool | Apache-2.0 AND CC0-1.0 AND CC-BY-SA-4.0 AND GPL-3.0-or-later | SPDX and REUSE compliance lint (run as a separate program; version pinned in `ci.yml`) | https://github.com/fsfe/reuse-tool | verified 2026-10-02 (package metadata, 6.2.0) |
| `uv` | dev tool | MIT OR Apache-2.0 | regenerates `requirements.lock` only | https://github.com/astral-sh/uv | verified 2026-10-02 (package metadata, 0.12.22) |
| `cmake` | build tool | BSD-3-Clause | builds the native libraries | https://cmake.org | to verify |
| `ninja` | build tool | Apache-2.0 | native build backend | https://ninja-build.org | to verify |
| `nasm` | build tool | BSD-2-Clause | SIMD assembly for libjpeg-turbo on x86 | https://www.nasm.us | to verify |
| `node` | build tool | MIT | builds the UI (Vite, Svelte) | https://nodejs.org | to verify |
| `valgrind` | dev tool | GPL-2.0-or-later | heap and leak profiling on Linux (M1.60), run only | https://valgrind.org | to verify |
| `gungraun` / `gungraun-runner` | dev tool (crate, CI binary) | Apache-2.0 OR MIT | Valgrind instruction-count benchmarks and the PR gate (M1.58); `gungraun` is a dev-dependency of `crates/imgproc-bench`, `gungraun-runner` 0.20.0 is installed by `perf-gate.yml`; neither is shipped | https://github.com/gungraun/gungraun | verified 2026-10-03 (crates.io metadata, 0.20.0) |
| `tesseract` | dev tool | Apache-2.0 | OCR oracle for the enhancement CER metric (M1.66), 5.x, run only; OCR is not a product feature (B19); the CI job installs 5.5.1 from `ppa:alex-p/tesseract-ocr5` with `eng.traineddata` (tessdata_fast, Apache-2.0) | https://github.com/tesseract-ocr/tesseract | version verified in CI run 37117596234; licence to verify |
| `imagemagick` | dev tool | ImageMagick | reference conversions and comparisons, run only | https://imagemagick.org | to verify |
| `unpaper` | dev tool | GPL-2.0-or-later | deskew and border comparison baseline, run only, Linux | https://github.com/unpaper/unpaper | to verify |

### Fonts

| Name | Kind | SPDX | Use | Source | Checked |
|---|---|---|---|---|---|
| `@fontsource/ibm-plex-sans` | font (npm package) | OFL-1.1 | UI text; bundled through the npm package (imported in `ui/src/main.ts`), no font CDN | https://fontsource.org/fonts/ibm-plex-sans | to verify. The OFL text and copyright notice must ship with the app: open item for the third-party notices (M13). |
| `@fontsource/ibm-plex-mono` | font (npm package) | OFL-1.1 | UI monospace text and numerals | https://fontsource.org/fonts/ibm-plex-mono | to verify, same notice duty |
| Hershey vector fonts (`cv2.putText`) | font | Apache-2.0 | glyphs in the throwaway `spikes/strips` receipt generator; not used by shipped code | part of OpenCV | to verify |
| DejaVu Sans Mono (`fonts-dejavu-core`) | font (Ubuntu package) | Bitstream-Vera AND LicenseRef-DejaVu-public-domain-changes | glyphs of the synthetic receipts rendered by `tools/ocr_oracle` in the CI job (M1.66); the rendered images are generated at run time, never committed or shipped | https://dejavu-fonts.github.io | to verify |

### Datasets, test images and weights

Nothing is downloaded or committed today. Weights: none used. The public corpora (SmartDoc 2015 Ch.1, CORD, MIDV-500, DIBCO, raw.pixls.us CC0-only) are pinned in [corpus.lock.toml](../corpus.lock.toml) with URL, SPDX licence and attribution, but every size and SHA-256 is still the placeholder `TODO-first-fetch`, which `cargo xtask fetch-corpus` refuses; they are fetched into a cache outside the repository and never committed ([corpora guide](testing/corpora.md)). Datasets and images:

| Name | Kind | SPDX | Use | Source | Checked |
|---|---|---|---|---|---|
| `stand-in synthetic suites` | generated data | MIT OR Apache-2.0 | `cargo xtask synth` writes smoke and full suites from a seed under `target/`; never committed (M1.50) | this repository | verified 2026-10-02 |
| `hostile corpus` | generated data | MIT OR Apache-2.0 | `cargo xtask make-hostile`: bombs and corruptions written under `target/hostile` (M1.69) | this repository | verified 2026-10-02 |
| `engine EditState fixtures` | test data | MIT OR Apache-2.0 | `crates/engine/fixtures/editstate/*.json`, hand-written | this repository | verified 2026-10-02 |
| `imgproc oracle fixtures` | test data | MIT OR Apache-2.0 | `crates/imgproc/tests/fixtures/*.json`, numbers produced by `tools/imgproc-oracles` | this repository | verified 2026-10-02 |
| `libheif conformance_window_padding.heic` | test image | to verify (libheif source tree, LGPL-3.0-or-later) | HEIC decode check in the native CI job; read from the extracted libheif release archive, never committed | libheif release archive in `native-deps.toml` | to verify the per-file licence before any redistribution |
| `private golden set` | real photos and scans | not applicable (private) | accuracy evaluation; lives only on the maintainer's encrypted disk and in the private `auto-crop-golden` repo; only aggregate metrics leave it | [golden-set policy](testing/golden-set.md) | B21 |

### Rust crates and npm code packages

Not repeated here. Rust: `cargo deny check` (allow-list in `deny.toml`) and `THIRD_PARTY_NOTICES.md` from `cargo about`. npm: dependencies are listed in `ui/package.json`; there is no automated licence gate for them yet, so the fonts above are checked by hand until one exists.

## Cleared datasets and weights not yet in use

Source of truth: the provenance log. Listed so the register stays complete.

| Id | Kind | SPDX | Use | Status |
|---|---|---|---|---|
| `dataset:smartdoc-2015-ch1` | dataset | CC-BY-4.0 | training and evaluation, from M4 | cleared |
| `dataset:cord` | dataset | CC-BY-4.0 | training and evaluation, from M4 | cleared |

Pending audit (no use until cleared): `dataset:midv-500`, `weights:makeacopy-docquadnet-256`.

## Excluded

Never used, or blocked until the owner records an exception (A-7). A reason is given for each class; the log has the licence URLs.

| Id | Kind | Why excluded |
|---|---|---|
| `weights:doctr` | weights | non-commercial licence (DocTr) |
| `weights:docgeonet` | weights | non-commercial licence (DocGeoNet) |
| `weights:docentr` | weights | non-commercial licence (DocEnTr) |
| `weights:docaligner` | weights | no weight licence stated, so all rights reserved; blocked |
| `weights:pp-lcnet-doc-ori` | weights | training data undisclosed; blocked pending audit |
| `weights:imagenet-backbones` | weights | ImageNet terms unsettled for derived weights; blocked pending an exception or a from-scratch run |
| `dssim-core` | crate | AGPL (B2); `deny.toml` ban |
| `heic` | crate | AGPL; `deny.toml` ban |
| `heic-decoder` | crate | AGPL; `deny.toml` ban |
| `jpegxl-rs` | crate | GPL-3.0 wrapper; `deny.toml` ban |
| `jpegxl-sys` | crate | GPL-3.0 wrapper; `deny.toml` ban |
| `birdcage` | crate | GPL-3.0; `deny.toml` ban |
| `x264` | crate | GPL encoder, never linked (B12); `deny.toml` ban |
| `x265` | crate | GPL encoder, never linked (B12); `deny.toml` ban |
| `x264-sys` | crate | GPL encoder; `deny.toml` ban |
| `x265-sys` | crate | GPL encoder; `deny.toml` ban |
| `opencv` | crate | the Rust bindings are not allowed in the workspace; OpenCV is a Python dev oracle only; `deny.toml` ban |
| `purecv` | crate | LGPL-2.1-or-later, statically linked by Rust; `deny.toml` ban (M1.73) |

Also excluded, by policy rather than by a tooling entry:

- **Datasets and weights with non-commercial, research-only or share-alike terms**, and models such as DIS5K, RMBG and DE-GAN ([weights policy](policy/model-weights.md), rule 2). SROIE is excluded as a receipt dataset (roadmap M7.69).
- **Any AGPL, GPL or non-commercial code or weights as a dependency** (B2). GPL programs may be run as separate dev tools (above), never linked or shipped.
- **Real personal photos and documents** in this repository, in pull requests, issues or CI logs (B21): see the [data policy in CONTRIBUTING](../CONTRIBUTING.md#test-data-and-personal-images-b21).
- **Network downloads at run time**: no model or dataset is fetched by the app (B18).

## Adding an entry

1. A tool, font, library or fixture: add a row to the right table above, with its SPDX id, use and source, in the same change that introduces it. `cargo xtask provenance` fails until a native library, pinned Python package or font package has its row.
2. A dataset or weight: add a `pending` row to the provenance log first, audit it, then set its status (see the [weights policy](policy/model-weights.md)).
3. A ban: add it to `deny.toml` and to the excluded table. `cargo xtask deny-selftest` plants it automatically.
