# Research: Performance and accuracy engineering and measurement  (key: perf)

## Summary
"Very fast, very accurate" becomes enforceable only as numeric budgets plus a benchmark and accuracy harness built before the GUI. Speed comes from three things: never touching full-resolution pixels until export, analysing a small proxy (≤1024 px long side), and using SIMD/rayon libraries. Library choice matters more than language: fast_image_resize is about 13x faster than the `image` crate's resizer on a 16 MP downscale (13.5 vs 176 ms, Lanczos3, single thread). HEIC is the speed outlier because HEVC has no DCT-style scaled decode, so it needs a thumbnail-first UX. Accuracy needs its own labelled corpus: public sets (SmartDoc, MIDV-500, CORD) are narrow, with licence caveats, so synthetic warps plus a small hand-labelled locked test set are essential. Tiered algorithms (fast path, refine, needs-review) with a calibrated confidence score reconcile speed and accuracy. Nearly all latency numbers below are extrapolated from desktop-class published benchmarks, so a week-1 spike must recalibrate them on real mid-range laptops.

## Recommendation
Build a headless `auto-crop-core` library plus CLI first, with a benchmark and evaluation harness, before any GUI work. Use libjpeg-turbo (turbojpeg) for scaled JPEG decode, libheif for HEIC (thumbnail-first UX, dynamically linked for LGPL), fast_image_resize plus rayon for resampling, and ONNX Runtime CPU as the default inference path. Run GPU/NPU providers (CoreML, DirectML, CUDA) only as benchmarked opt-ins. Gate CI on instruction counts (gungraun/CodSpeed simulation, Linux) and on per-slice accuracy on a deterministic smoke set. Add nightly wall-clock runs on a stable machine. Keep a hand-labelled locked test set, and report risk-coverage (failure rate among auto-accepted images) instead of accuracy alone. Adopt the budgets in the deliverable as provisional and re-baseline them after the week-1 spike on a real Tier-M laptop. Rust is justified mainly by safe SIMD, rayon, cargo packaging and the ort/fir ecosystem, not by raw speed over C++.

## Key findings
- Libraries dominate speed: fast_image_resize 6.1.0 (AVX2, single thread, Ryzen 9 5950X) resizes 16 MP to 852x567 in 3.55 ms (bilinear) or 13.5 ms (Lanczos3). The `image` crate takes 80/176 ms and libvips 5.6/17 ms. Proxy creation therefore costs single-digit to low-tens of ms.
- Scaled JPEG decode: zune-jpeg (the `image` crate's default decoder) has no DCT scaling. libjpeg-turbo via the `turbojpeg` crate does (set_scaling_factor) but needs an external libturbojpeg. `jpeg-decoder` has scale() for 1/8, 1/4, 1/2 but was last released 2025-06. Reduced decode removes IDCT/colour work but not Huffman decoding, so expect roughly 3-5x, not 8x (extrapolation).
- HEIC is slow and has no scaled-decode shortcut. The pure-Rust `heic` crate needs 451 ms sequential and 180 ms with parallel tiles for 12 MP on a Ryzen 9 7950X, so expect 2-3x more on a mid-range laptop. Use the embedded thumbnail for a draft preview and decode the full image in the background. The pure-Rust decoder is AGPL-3.0 or commercial; libheif is LGPL.
- ONNX Runtime via `ort` is still 2.0.0-rc.13 (2026-07-28, wrapping ORT ~1.30) although its docs call it production-ready. DirectML is in sustained engineering, with new Windows work moving to WinML. The ort docs flag the WebGPU EP as experimental. A CPU baseline is mandatory and GPU/NPU EPs must be benchmarked per model.
- A 256x256-class classifier or heatmap net runs in low-single-digit to low-tens of ms on CPU. PaddleOCR's 7 MB PP-LCNet doc-orientation model reports 99.06% top-1 at 3.24 ms on a Xeon 6271C. Orientation at 99%+ is realistic, but that number comes from a small self-built set.
- Public data is narrow and licence-tangled. SmartDoc Ch.1 is A4 pages on 5 backgrounds (CC BY 4.0). MIDV-500 is ID cards. CORD is 1,000 receipt photos (CC BY 4.0). The aggregated HF DocCornerDataset warns of research-only terms for parts of it. Fetch corpora by script and never vendor them; ship only synthetic and self-owned data.
- Wall-clock benchmarks on shared CI runners are too noisy (public runners: ubuntu 4 vCPU/16 GB, macos-latest 3 vCPU M1/7 GB). Gate on instruction counts (gungraun 0.20.0, Linux-only, Valgrind-based, or CodSpeed simulation, free for OSS) and run nightly wall-clock on a stable machine.
- The SmartDoc metric is Jaccard/IoU of the quad, with success defined as IoU ≥ 0.945. Adopting this protocol makes results comparable with the literature.

## Risks
- Budgets are extrapolated from Ryzen 9 5950X/7950X and old-i7 numbers, not mid-range laptops or Apple silicon. Real values may be 1.5-3x worse, so budgets are provisional until the week-1 spike.
- Public benchmarks are saturated or overfit (the DocAligner authors say so themselves), and none covers thermal-receipt photos well. The synthetic-to-real gap is the main accuracy risk, and a hand-labelled locked set is the only real guard.
- Dataset licence conflicts: the DocCornerDataset card claims research-only for SmartDoc/MIDV while the SmartDoc Zenodo record says CC BY 4.0 (plus a citation/email request). Mixing them into a permissive repo needs a licence audit.
- HEIC: libheif is LGPL (dynamic linking required), HEVC decode has patent-licensing ambiguity, and the pure-Rust `heic` decoder is AGPL. OS decoders (ImageIO, WIC) may be faster but are unbenchmarked here.
- ORT EP variance: CoreML and DirectML can silently partition or fall back for unsupported ops, and small nets often gain nothing from a GPU. Non-VNNI x86 int8 (U8S8) can saturate and lose accuracy, so quantised models need per-platform accuracy checks.
- Tooling licence traps: Albumentations was archived 2025-07-10 and its successor AlbumentationsX is AGPL-3.0, so avoid it in the generator. `std::simd` is still gated behind #![feature(portable_simd)] on the tracking issue (current status unconfirmed), so use `wide`, `pulp`, or fir/pic-scale.
- Synthetic generator and app share failure modes if they share code. Generate with independent tools (Python/OpenCV/Augraphy), not the app's own warp.

## Options evaluated

### fast_image_resize — recommended
SIMD Rust resampler (SSE4.1/AVX2/NEON, optional rayon) for proxy creation and export resizing.
- licence: Apache-2.0 OR MIT
- status: 6.1.0, 2026-07-21; about 4.5M recent downloads; actively maintained.
- pros: 16 MP to 0.5 MP in 3.5-13.5 ms single-threaded (5950X); About 13x faster than the `image` crate; Supports u8, u16, f32 pixel types
- cons: No AVX-512; Benchmarks are single-thread on a desktop CPU

### pic-scale — viable
Alternative SIMD scaler with linear-light, f16 and HDR-friendly paths, 30+ filters.
- licence: BSD-3-Clause OR Apache-2.0
- status: 0.7.12, 2026-09-10; active; benchmarks against fir not published.
- pros: AVX2, AVX-512, NEON, AVX-VNNI backends; Linear-light and 16-bit/f32 support for colour-correct resizing
- cons: Pre-1.0 API; No head-to-head numbers, so benchmark it against fir

### libjpeg-turbo via `turbojpeg` crate — recommended
Scaled JPEG decode (1/2, 1/4, 1/8) and fast encode.
- licence: Crate: Unlicense OR MIT; libjpeg-turbo: BSD/IJG/zlib
- status: turbojpeg 1.5.1 (2026-07-25); libjpeg-turbo 3.2.x.
- pros: DCT-domain scaling; Fast SIMD encode/decode; 12-bit/lossless support in 3.x
- cons: Needs an external or vendored C library and build tooling; Scaling only saves IDCT and colour work, not Huffman decoding

### zune-jpeg — fallback
Pure-Rust SIMD JPEG decoder (default in the `image` crate).
- licence: MIT OR Apache-2.0 OR Zlib
- status: 0.5.15 (0.5.16-rc2 on 2026-09-08); about 44M recent downloads.
- pros: Speed close to libjpeg-turbo (±10 ms by its own claim); Safe Rust; no_std capable
- cons: No DCT-scaled decode documented; Not bit-identical to libjpeg

### libheif (libheif-rs / libheif-sys) — recommended
HEIC/HEIF/AVIF decoding with thumbnails, tiled parallel decode and security limits.
- licence: libheif LGPL; libheif-sys MIT
- status: libheif-rs 3.0.0 (2026-08-18); libheif-sys 5.3.1 wrapping libheif 1.23.1.
- pros: Broadest HEIC/HEIF coverage; Embedded thumbnails for instant draft preview; Parallel tile decode
- cons: LGPL means dynamic linking and shipping compliance; HEVC patent-licensing ambiguity; Full-res decode is hundreds of ms for 12 MP

### ort (ONNX Runtime for Rust) — recommended
Inference for corner, orientation and enhancement nets, with CPU and optional CoreML, DirectML, CUDA, WebGPU providers.
- licence: Apache-2.0 OR MIT
- status: 2.0.0-rc.13, 2026-07-28; wraps ORT ~1.30; about 7.6M recent downloads.
- pros: Mature runtime with int8 QDQ quantisation; Model caching for CoreML; Cross-platform
- cons: Still an RC; DirectML is in sustained engineering; EP fallbacks and CoreML/GPU gains are model-dependent; U8S8 saturation on non-VNNI x86

### gungraun (successor to iai-callgrind) + CodSpeed simulation — recommended
Deterministic instruction-count regression gating in CI.
- licence: gungraun Apache-2.0 OR MIT; CodSpeed free for OSS
- status: gungraun 0.20.0 (2026-09-26); codspeed-criterion-compat 5.0.2 (2026-09-17).
- pros: Variance below 1% claimed for CPU simulation; Works on noisy shared runners; DHAT heap stats
- cons: gungraun is Linux-only; no Windows/macOS; Instruction counts miss memory-bandwidth and threading effects

### Augraphy (synthetic degradation) — viable
Python document-scan and paper-degradation augmentations with mask and keypoint ground truth.
- licence: MIT
- status: 8.2.6; release date unverified.
- pros: Realistic ink, paper, shadow and fold effects; Permissive licence; Keypoints preserved for spatial transforms
- cons: Python-only tool; Keypoints unsupported for some effects (e.g. InkShifter); Pair it with OpenCV for perspective warps

## Deliverable
**Reference machine "Tier-M":** 6C/12T x86-64 AVX2 laptop (Ryzen 5 7640U / Core i5-1335U class) or Apple M1 8C, 16 GB, iGPU. Limits on "Tier-L" (4C, 8 GB) are 2x these budgets. All figures are extrapolated unless cited.

**Speed and memory budgets (JPEG unless stated)**

| Operation | 12 MP | 48 MP | 100 MP |
|---|---|---|---|
| Cold start to usable UI | ≤1.0 s | – | – |
| First paint (scaled decode 1/4-1/8) | ≤60 ms | ≤120 ms | ≤250 ms |
| First preview with auto-quad overlay | ≤150 ms | ≤250 ms | ≤450 ms |
| HEIC draft (thumbnail) / final overlay | ≤150 ms / ≤700 ms | ≤400 ms / ≤3 s | n/a |
| Fast-path analysis on ≤1024 px proxy (corners+skew+orientation+confidence) | ≤40 ms at any size | | |
| Full-res auto-process → JPEG q90 (single image) | ≤400 ms | ≤1.5 s | ≤3.5 s |
| Batch throughput, 6C, JPEG→JPEG | ≥10 img/s | ≥2.5 img/s | ≥1 img/s |
| HEIC→JPG only | ≥5 img/s | | |
| Peak RSS per in-flight image (≤3x decoded RGB8 + 64 MB) | ≤170 MB | ≤500 MB | ≤1 GB |
| Corner-drag/rotate preview | ≤8 ms/frame (GPU homography on ~2 MP proxy) | | |
| Contrast/threshold slider | ≤16 ms on proxy | | |
| Undo/redo | ≤50 ms, parametric history | | |

The 12 MP budget breaks down as: decode ~85 ms (Google's benchmark shows turbojpeg at 84 ms for 6 MP on a 2.6 GHz i7, assumed ~2x faster now), analysis 40, warp 30, enhance 40, encode ~100, about 300 ms. Batch scaling efficiency ≥70% up to 8 cores, and the worker count is throttled to ≤50% of free RAM.

**Accuracy targets**

| Task | Metric | Release target |
|---|---|---|
| Quad | IoU after canonical warp (SmartDoc, success ≥0.945) | mean ≥0.985; failures ≤2% realistic, ≤6% hard slice |
| Corners | error / image diagonal | median ≤0.15%, p95 ≤0.5%; ≤2 px at 12 MP after full-res refine |
| Skew | abs angle error | median ≤0.1°, p95 ≤0.5° |
| Orientation | top-1 (0/90/180/270) | ≥99%; ≥99.5% at 95% coverage; EXIF/HEIF rotation 100% (no double-apply) |
| Crop | IoU vs hand box | mean ≥0.97, p5 ≥0.92; content-clipping ≤0.5% |
| Enhancement | Tesseract 5 CER before vs after; DIBCO F-measure | median ΔCER ≤0; ≥20% relative CER cut on low-contrast slice; worse by >1 pt on ≤2% of images; F within 1 pt of the Sauvola baseline |
| HEIC→JPG | ΔE2000 vs reference (ΔE ≤1 ≈ just noticeable) | mean ≤0.5, p99 ≤1.5, max ≤3; EXIF/ICC kept 100% |
| Confidence | ECE; risk-coverage | ECE ≤0.05; auto-accepted failures ≤1% at ≤10% review rate |
| Robustness | fuzz and malformed inputs | 0 crashes or hangs; pixel cap default 500 MP |

**Tiered pipeline:**
- T0: thumbnail draft, ≤100 ms.
- T1 (fast path): 256² net plus geometry checks; expected to handle about 85-90% of images at ≤40 ms.
- T2 (refine, on low confidence): 512² net, flip/rotate test-time augmentation, classical edge fallback, and sub-pixel line-fit on full-res corner ROIs, ≤250 ms.
- T3: below the lower confidence threshold, flag "needs review". Batch mode queues these instead of exporting.

Confidence combines heatmap peak sharpness, agreement between learned and classical quads, geometric plausibility (convexity, corner angles 60-120°, aspect, area), and TTA agreement. It is calibrated with isotonic or temperature scaling on held-out data.

**Corpus:**
- (a) Public sets: SmartDoc, MIDV-500/2019, CORD, DocUNet/UVDoc benchmarks, DIBCO. Fetched by script with checksums, never vendored.
- (b) About 5k synthetic images with exact ground truth. Render known-text pages/receipts, then apply random homography (tilt to 45°, any rotation), backgrounds, shadows and gradients, blur, noise, JPEG q40-95, thermal fade and curl, using Augraphy/OpenCV.
- (c) 300-500 hand-labelled real photos (HEIC/JPEG/scan, receipts, A4, cards, whiteboards) as a locked release-only test set, with two annotators on 20% to measure the noise floor.
- Report per-slice metrics (lighting, clutter, occlusion, tilt >30°, aspect >4:1, format) and gate on the worst slice.

**CI:**
- Per PR: 200-image deterministic accuracy smoke (block on mean IoU −0.3 pt or failure rate +0.5 pt) plus instruction-count gates (>5%) on decode, resize, warp and threshold kernels (Linux). Windows/macOS use interleaved A/B builds on the same runner.
- Nightly: full accuracy suite plus wall-clock e2e via hyperfine/criterion on a stable machine (issue on >10% p50 regression).
- Publish JSON to a metrics branch and render a Pages dashboard (per-slice trends, latency vs failure-rate Pareto).
- Profiling: samply (Win/mac/Linux) plus Tracy; Instruments or perf as needed; cargo-flamegraph; heaptrack/DHAT for memory; wgpu timestamp queries for GPU.

## Decision-critical claims (as researched)
- fast_image_resize 6.1.0 (Apache-2.0/MIT; SSE4.1/AVX2/NEON, no AVX-512) resizes RGB8 4928x3279 to 852x567 in 3.55 ms (bilinear) and 13.5 ms (Lanczos3) single-threaded on a Ryzen 9 5950X, versus 80 ms and 176 ms for the `image` crate and 5.6/17.3 ms for libvips. [https://github.com/Cykooz/fast_image_resize/blob/main/benchmarks-x86_64.md]
- zune-jpeg documents no DCT-scaled (reduced-size) decoding, whereas jpeg-decoder offers Decoder::scale() with 1/8, 1/4, 1/2 factors (last release 0.3.2, 2025-06) and the turbojpeg crate exposes set_scaling_factor via an external libturbojpeg. [https://docs.rs/jpeg-decoder/latest/jpeg_decoder/struct.Decoder.html]
- Full HEVC decode of a 12 MP (3024x4032, 48-tile) HEIC takes 451 ms sequential and 180 ms with parallel tiles on a Ryzen 9 7950X (pure-Rust `heic` crate, AGPL-3.0 or commercial), so the format has no cheap scaled-decode path and needs thumbnail-first UX; libheif (LGPL) offers thumbnails and parallel tile decoding. [https://github.com/imazen/heic]
- The `ort` crate is at 2.0.0-rc.13 (2026-07-28), not yet stable 2.0. DirectML is in 'sustained engineering' with new Windows work moving to WinML, and the ort docs call the WebGPU EP experimental. [https://onnxruntime.ai/docs/execution-providers/DirectML-ExecutionProvider.html]
- SmartDoc Challenge 1 evaluates quads by Jaccard index with a success threshold of 0.945. The dataset is CC BY 4.0 (citation and email to the organisers requested), and MIDV-500 source documents are public domain or openly licensed. The aggregated DocCornerDataset card says MIDV/SmartDoc are research-use-only. [https://zenodo.org/record/1230217]
- PaddleOCR's PP-LCNet_x1_0_doc_ori 4-class orientation model reports 99.06% top-1, 7 MB, and 3.24 ms CPU inference (Xeon Gold 6271C), trained on a self-built dataset. [https://www.paddleocr.ai/v3.3.2/version3.x/module_usage/doc_img_orientation_classification.html]
- Public-repo GitHub runners are free but small (ubuntu-latest 4 vCPU/16 GB; macos-latest arm64 3 vCPU M1/7 GB). gungraun (successor to iai-callgrind, v0.20.0) gives deterministic instruction counts but is Linux-only, and CodSpeed is free for OSS with simulation and 600 wall-time macro-runner minutes per month. [https://docs.github.com/en/actions/reference/runners/github-hosted-runners]
- Albumentations (MIT) was archived 2025-07-10 and its successor AlbumentationsX is dual AGPL-3.0/commercial, so it is unsuitable for a permissive repo. Augraphy (MIT) provides document-degradation augmentations with keypoint and mask ground-truth support. [https://github.com/albumentations-team/albumentations]

## Researcher questions for user
- What is the minimum hardware you want to commit to for the 'very fast' targets? — It sets the SIMD baseline (AVX2 vs SSE4.1 fallback), whether ARM64 Windows/Linux and Intel Macs are first-class, and which machines the budgets are measured on. (default: x86-64 with AVX2 plus Apple Silicon and Linux/Windows ARM64; Intel Macs best-effort)
- How cautious should batch mode be about silent mistakes? — It sets the auto-accept confidence threshold, which trades the share of images needing manual review against the rate of undetected bad crops. (default: Balanced: ≤1% silent failures, about 8-10% flagged)
- Do you accept shipping a small learned model (a 5-10 MB ONNX file plus the ONNX Runtime library) for corner detection and orientation, or must v1 be classical CV only? — Learned models are markedly more robust on cluttered or low-contrast backgrounds but need training data, a permissive model licence, and the ORT runtime (a pre-1.0 Rust binding), which adds package size and CI matrix cost. (default: Classical fast path plus a small ONNX model trained on synthetic and permissively licensed data)
- May the hand-labelled evaluation set be built from your own photos and published in the repo? — Real photos are the only reliable guard against the synthetic-to-real gap, but receipts and scans can contain personal data and the set must be locked and openly licensed. (default: Your own photos of specimen or redacted documents, published CC0)
- For HEIC/HEIF to JPG, should output preserve the source's wide-gamut colour (Display P3 with an embedded ICC profile) or convert to sRGB by default? — It defines the ΔE2000 reference and pass/fail test. It also decides whether HDR gain maps are dropped or converted to Ultra HDR JPEG, which changes file size and compatibility. (default: Preserve ICC/P3 by default, with an 'sRGB for compatibility' toggle)

## INDEPENDENT VERIFICATION (skeptic) — overrides the researcher where they differ
- [PARTLY-TRUE] 1. fast_image_resize 6.1.0 (Apache-2.0/MIT; SSE4.1/AVX2/NEON, no AVX-512) resizes RGB8 4928x3279 to 852x567 in 3.55 ms bilinear / 13.5 ms Lanczos3 single-threaded on a Ryzen 9 5950X vs 80 ms / 176 ms for `image` and 5.6/17.3 ms for libvips.
  CORRECTION: The numbers match the benchmarks-x86_64.md table exactly (fir avx2 3.55/13.52; image 80.24/176.09; libvips 5.58/17.31; fir sse4.1 5.83/15.91; fir scalar 15.62/37.26). The file is stamped with fir version 6.0.1, not 6.1.0, so the figures were not re-run for 6.1.0. The 5950X is the CPU and the runs are single-threaded. crates.io confirms 6.1.0 was published 2026-07-21 under MIT OR Apache-2.0, with about 4.48M recent downloads. The README lists SSE4.1, AVX2, NEON and Wasm SIMD128 and does not mention AVX-512. The 13x figure applies to Lanczos3 only; bilinear is about 22x. The 12-bit-safe gap is real, but treat these as desktop-class numbers.
- [PARTLY-TRUE] 2. zune-jpeg documents no DCT-scaled decoding, whereas jpeg-decoder offers Decoder::scale() with 1/8, 1/4, 1/2 (last release 0.3.2, 2025-06) and the turbojpeg crate exposes set_scaling_factor via an external libturbojpeg.
  CORRECTION: Confirmed: jpeg-decoder 0.3.2 was published 2025-06-21 and Decoder::scale() supports 1/8, 1/4, 1/2 and 1. Confirmed: neither the zune-jpeg docs nor its README mention scaled decode. Confirmed: turbojpeg 1.5.1 (2026-07-25) has set_scaling_factor. Two corrections. (a) turbojpeg-sys builds libjpeg-turbo from bundled source by default via cmake and needs NASM or Yasm (the default errors without it). It bundles libjpeg-turbo 3.1.0, not the 3.2.x that upstream now ships (3.2.0, 2026-06-30). A system library is an opt-in pkg-config feature. (b) jpeg-decoder is in maintenance mode, and image-rs says it is moving to zune-jpeg. It is a weak long-term fallback for scaled decode.
- [CONFIRMED] 3. Full HEVC decode of a 12 MP (3024x4032, 48-tile) HEIC takes 451 ms sequential and 180 ms parallel on a Ryzen 9 7950X (pure-Rust `heic` crate, AGPL-3.0 or commercial); no cheap scaled-decode path so thumbnail-first UX; libheif (LGPL) offers thumbnails and parallel tile decoding.
  CORRECTION: All of it holds. The imazen/heic README lists 451 ms sequential and 180 ms parallel for 3024x4032 with 48 tiles on a 7950X. The crate is AGPL-3.0-only OR a commercial licence, v0.1.6, with only about 9.5k recent downloads, so it is immature and copyleft and not a serious default. The README also states that no patent rights are granted. libheif is LGPL and supports thumbnails, and its ENABLE_PARALLEL_TILE_DECODING CMake option defaults to ON. That is a build-time switch, so a distro or vcpkg build may differ. libheif-rs 3.0.0 (2026-08-18) and libheif-sys 5.3.1+1.23.1 (2026-08-13) are confirmed.
- [CONFIRMED] 4. The `ort` crate is at 2.0.0-rc.13 (2026-07-28), not yet stable 2.0; DirectML is in 'sustained engineering' with new Windows work moving to WinML; ort docs call the WebGPU EP experimental.
  CORRECTION: crates.io shows max version 2.0.0-rc.13, published 2026-07-28, no stable release, and about 7.59M recent downloads. The ONNX Runtime DirectML page says DirectML is in sustained engineering with new feature work moved to WinML. The ort execution-providers page calls WebGPU experimental and says it may give wrong results or crash. One related error: the summary says ort wraps ORT ~1.30, but the rc.13 release notes, its docs.rs README and ort's version-mapping page all say ONNX Runtime 1.28.0. The 1.30 badge is only on the GitHub master README, so it is unreleased.
- [PARTLY-TRUE] 5. SmartDoc Challenge 1 evaluates quads by Jaccard index with a success threshold of 0.945; dataset is CC BY 4.0 (citation and email to organisers requested); MIDV-500 source documents public domain or openly licensed; the aggregated DocCornerDataset card says MIDV/SmartDoc are research-use-only.
  CORRECTION: Confirmed: the Zenodo record is CC BY 4.0 and asks users to cite the ICDAR2015 SmartDoc paper and email icdar.smartdoc@gmail.com. Confirmed: the official evaluation script (jchazalon/smartdoc15-ch1-eval) computes Jaccard index after a homography into a reference frame, which matches 'IoU after canonical warp'. Confirmed: the MIDV-500 paper says all source images are public domain or under public copyright licences. Confirmed: the DocCornerDataset card marks MIDV and SmartDoc as research use only, which conflicts with SmartDoc's CC BY on Zenodo, so avoid that aggregate. NOT verified: the 0.945 success threshold. It is absent from the Zenodo record and from the official eval scripts, and the original paper could not be retrieved. Treat 0.945 as an unsourced number and either cite it from the paper or drop it. The target 'failures = Jaccard < 0.945' is then the project's own definition.
- [PARTLY-TRUE] 6. PaddleOCR's PP-LCNet_x1_0_doc_ori 4-class orientation model reports 99.06% top-1, 7 MB, 3.24 ms CPU inference (Xeon Gold 6271C), trained on a self-built dataset.
  CORRECTION: Numbers confirmed: 4 classes (0/90/180/270), 99.06% top-1, 7 MB, 3.24 ms in regular mode and 1.19 ms in high-performance mode on an Intel Xeon Gold 6271C at 2.6 GHz. Correction: the self-built set is the TEST set (1,000 images of ID and document scenes), listed under the docs' 'Testing Environment', not described as training data. The page does not disclose the training data. So 99.06% is measured on only 1,000 ID and document images, and it will not transfer to receipts, whiteboards or cluttered photos. It is a reference point and not evidence for the project's >=99% orientation target.
- [PARTLY-TRUE] 7. Public-repo GitHub runners are free but small (ubuntu-latest 4 vCPU/16 GB; macos-latest arm64 3 vCPU M1/7 GB). gungraun (successor to iai-callgrind, v0.20.0) gives deterministic instruction counts but is Linux-only, and CodSpeed is free for OSS with simulation and 600 wall-time macro-runner minutes per month.
  CORRECTION: Confirmed on GitHub docs: standard runners are free and unlimited on public repos. Ubuntu is 4 vCPU/16 GB x64, Windows is 4 vCPU/16 GB, and macos-latest is arm64 at 3 vCPU M1/7 GB. There is also a 1 vCPU ubuntu-slim and a 4 vCPU/14 GB Intel macOS option. gungraun 0.20.0 was published 2026-09-26 and is Apache-2.0 OR MIT. Its README says it is forked from iai and offers an iai-callgrind migration checklist. 'Linux-only' is slightly off: the docs say it cannot run on Windows or on targets with neither Valgrind nor Linux perf, and the install guide covers Linux and FreeBSD. macOS is not documented as supported, and Valgrind does not work on current Apple Silicon. It also has a Linux-perf-only mode. CodSpeed's pricing page shows 600 macro-runner min/month on the free plan, with unlimited seats for OSS and 3-month history. It lists a CPU Simulation instrument with no free-tier restriction. Whether gungraun benchmarks run under CodSpeed was not verified.
- [CONFIRMED] 8. Albumentations (MIT) was archived 2025-07-10 and its successor AlbumentationsX is dual AGPL-3.0/commercial, so it is unsuitable for a permissive repo. Augraphy (MIT) provides document-degradation augmentations with keypoint and mask ground-truth support.
  CORRECTION: Confirmed: the Albumentations repo is archived (2025-07-10) and points to AlbumentationsX. AlbumentationsX is AGPL-3.0-only or commercial, and its README warns that AGPL is incompatible with MIT, Apache-2.0 and BSD projects. Nuance: if it is used only as an offline dev tool for data generation and no code is vendored, the app itself is not AGPL-tainted. Only the ambiguity over generated data remains, so the 'unsuitable' verdict is a policy choice rather than a hard legal fact. Augraphy is MIT and supports masks and keypoints in its spatial augmentations, with exceptions such as InkShifter. Its pixel-level effects leave masks and keypoints unchanged. The 'release date unverified' gap is now filled: PyPI shows 8.2.6 was uploaded 2023-12-31 and the dev branch's last commit is July 2025. That is a maintenance risk to note before depending on it.

### Other errors spotted by skeptic
- The ort option row says it 'wraps ORT ~1.30'. The rc.13 release notes, its docs.rs README and the version-mapping page all say ONNX Runtime 1.28.0. The 1.30 badge is only on the unreleased GitHub master README.
- The libjpeg-turbo row cites '3.2.x', but the default vendored build in turbojpeg-sys bundles libjpeg-turbo 3.1.0 (the 3.2.0 release is dated 2026-06-30). The build needs cmake and NASM, or a system library through the pkg-config feature. Claim 2's 'external libturbojpeg' framing and the 'needs an external or vendored C library' con are therefore only half right.
- The Augraphy status 'release date unverified' can be resolved: 8.2.6 is from 2023-12-31, with no PyPI release for about 2.75 years and only sparse commits through July 2025. The option is rated 'viable' without any maintenance warning.
- The fast_image_resize benchmark table is stamped with fir 6.0.1, while the option text presents 6.1.0 as the version benchmarked.
- The deliverable uses 'SmartDoc success >=0.945' as an accuracy definition. No primary source found sets that threshold, and the official eval scripts do not define one.
- The claim that Google's benchmark shows turbojpeg decoding 6 MP in 84 ms on a 2.6 GHz i7 could not be located or verified. It feeds the decode line of the 12 MP budget, so treat that line as unverified until measured in the week-1 spike.
- The jpeg-decoder maintenance-mode status (image-rs is moving to zune-jpeg) is not in the options list. It weakens the idea of jpeg-decoder as a scaled-decode fallback.
- Verified with no issue: fir 6.1.0 (2026-07-21, about 4.48M downloads); pic-scale 0.7.12 (2026-09-10; README lists AVX2, AVX-512, AVX-VNNI, NEON, 30+ filters, no fir comparison); zune-jpeg 0.5.15 (2026-03-26) plus 0.5.16-rc2 (2026-09-08), about 44.5M downloads, README claims speed within 10 ms of libjpeg-turbo; turbojpeg 1.5.1 (2026-07-25); libheif-rs 3.0.0; libheif-sys 5.3.1+1.23.1; codspeed-criterion-compat 5.0.2 (2026-09-17). WebSearch quota ran out mid-check, so some claims were verified through direct primary-source fetches only.

## Sources
- https://github.com/Cykooz/fast_image_resize/blob/main/benchmarks-x86_64.md
- https://github.com/Cykooz/fast_image_resize
- https://docs.rs/jpeg-decoder/latest/jpeg_decoder/struct.Decoder.html
- https://docs.rs/turbojpeg/latest/turbojpeg/
- https://docs.rs/crate/zune-jpeg/latest
- https://github.com/google/decoder-benchmarks-for-rust
- https://github.com/imazen/heic
- https://github.com/strukturag/libheif
- https://github.com/pykeio/ort
- https://ort.pyke.io/perf/execution-providers
- https://onnxruntime.ai/docs/execution-providers/DirectML-ExecutionProvider.html
- https://onnxruntime.ai/docs/execution-providers/CoreML-ExecutionProvider.html
- https://onnxruntime.ai/docs/performance/model-optimizations/quantization.html
- https://zenodo.org/record/1230217
- https://arxiv.org/abs/1807.05786
- https://huggingface.co/datasets/mapo80/DocCornerDataset
- https://github.com/clovaai/cord
- https://github.com/tanguymagne/UVDoc
- https://github.com/tanguymagne/UVDoc-Dataset
- https://github.com/docsaidlab/DocAligner
- https://www.paddleocr.ai/v3.3.2/version3.x/module_usage/doc_img_orientation_classification.html
- https://arxiv.org/abs/2206.02136
- https://docs.github.com/en/actions/reference/runners/github-hosted-runners
- https://github.com/gungraun/gungraun
- https://codspeed.io/pricing
- https://codspeed.io/blog/benchmarks-in-ci-without-noise
- https://github.com/mstange/samply
- https://github.com/albumentations-team/albumentations
- https://github.com/sparkfish/augraphy
- https://github.com/awxkee/pic-scale
- https://github.com/awxkee/moxcms
- https://libjpeg-turbo.org/About/Performance
- https://crates.io/crates/ort
- https://crates.io/crates/fast_image_resize
- https://github.com/rust-lang/rust/issues/86656