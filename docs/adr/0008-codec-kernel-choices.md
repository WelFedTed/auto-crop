# 0008 - Codec kernel choices: decoder, resizer, JPEG encoder, linear-light rule

- **Status:** accepted
- **Date:** 2026-10-03
- **Roadmap items:** M1.56 (this ADR), M1.18, M1.19, M1.20, M1.21 (the code it measures)
- **Decision log links:** B12 (C parsers in the sandboxed pool), B2 (licences), B18 (offline), A-4 (gates go back to the owner)
- **Time box:** none (benchmarks over finished code). Code: `crates/codecs/benches/decode.rs`, `crates/imgproc-bench/benches/resize.rs`, `crates/imgproc-bench/examples/resize_quality.rs`, `.github/workflows/turbojpeg.yml`.

## Context

M1 has two JPEG decode paths (pure-Rust zune-jpeg, and libjpeg-turbo behind the `turbojpeg` feature, ADR-0004), three resizer candidates (the in-tree area resize, `fast_image_resize`, `pic-scale`, plus `image::imageops::resize` as the naive baseline), three JPEG encoders (the `image` crate's, `jpeg-encoder`, libjpeg-turbo) and an open question from PLAN 3.7 and ADR-0002: resample in gamma-encoded space or in linear light. The pipeline-skeleton measurements ([budgets.md](../perf/budgets.md)) flag `proxy` (the resize) and `encode` (the `image` encoder, 214 ms at 12 MP) as the stages that do not fit single-threaded.

## Method and noise

- **Where the numbers come from.** Decode, encode and resize timings are from **GitHub-hosted runners** (windows-2025, macos-latest, ubuntu-22.04), one criterion run per OS (`--warm-up-time 1 --measurement-time 3`, median of 10 to 15 samples), workflow run [37119220716](https://github.com/WelFedTed/auto-crop/actions/runs/37119220716). Shared 3 to 4 vCPU VMs: **NOISY** (differences under about 20% are not meaningful), but the candidates of one row ran back to back on the same machine, so ratios within an OS are the evidence. The 8-thread columns on a 4-core runner say "all cores", not 8x.
- **Dev machine** (Windows 11, i7-8700K, 12 threads) numbers are given only where a runner number does not exist (PSNR/SSIM tables are deterministic; its timings are **NOISY**: an unrelated process held about 1.3 cores and the CPU sat at 70 to 100% while it ran, so its 8-thread rows even came out slower than the 1-thread ones and are not quoted).
- **Inputs.** JPEG: 4000 x 3000, 4:2:0, q90, photo-like synthetic content (gradient, dark rectangles, noise; 1.7 MB). PNG, TIFF, WebP: 2000 x 2000. Resize: the 12 MP synthetic document page of `imgproc-bench` to 1024 x 768. Synthetic content stands in for the real-device corpus (M0.49, M6): entropy, and so decode time, will differ on real photos.

## Results

### JPEG decode, 12 MP (median, ms)

| Candidate | ubuntu-22.04 | macos-latest | windows-2025 |
|---|---:|---:|---:|
| zune-jpeg full (`codecs::decode`: pre-checks, decode, orientation) | 61.8 | 47.3 | 48.6 |
| libjpeg-turbo 3.2.0 full (`decode_scaled(Want::Full)`) | 48.9 | 30.2 | 37.1 |
| libjpeg-turbo 1/2 | 28.5 | 22.1 | 25.6 |
| libjpeg-turbo 1/4 | 23.8 | 20.0 | 19.9 |
| libjpeg-turbo 1/8 | 19.2 | 20.7 | 16.6 |
| zune-jpeg full, then area resize to 1/4 (what a build without the feature does) | 84.2 | 60.2 | 62.4 |

libjpeg-turbo is 1.3 to 1.6x faster for a full decode, and a 1/4-size result is **2.5 to 3.5x faster** than decoding in full and shrinking. The gain stops at about 20 ms because entropy (Huffman) decoding is not reduced by DCT scaling. Dev machine (NOISY): zune-jpeg full 105 ms, zune then block-average to 1/4 169 ms.

Accuracy (CI, all three OSes identical): libjpeg-turbo versus zune-jpeg, mean abs diff 0.064 LSB (4:2:0), 0.059 (progressive 4:2:0), 0.022 (4:2:2), 0.008 (4:4:4), 0.005 (gray). Scaled versus full decode plus block average, 40 fixtures: worst PSNR 45.5 dB at 1/2, 45.0 dB at 1/4, 36.5 dB at 1/8 (PROVISIONAL bound 35 dB), 52 dB on a 4001 x 3001 image. Details: [codecs-hostile-input.md](../testing/codecs-hostile-input.md).

### JPEG encode at q90, 12 MP RGB8 (median, ms; output bytes)

| Encoder | ubuntu-22.04 | macos-latest | windows-2025 | bytes |
|---|---:|---:|---:|---:|
| `image` crate (`codecs::encode`, used by the skeleton today) | 287.6 | 119.8 | 238.9 | 1,958,639 |
| `jpeg-encoder` 0.7 (pure Rust, IJG tables) | 116.7 | 66.1 | 100.9 | 1,738,524 |
| libjpeg-turbo `tj3Compress8` | 33.1 | 28.6 | 29.4 | 1,705,991 |

Round trip at q90 on a smooth gradient: libjpeg-turbo 47.8 dB, `jpeg-encoder` 47.4 dB (bound: 40 dB). Dev machine (NOISY): `image` 356 ms, `jpeg-encoder` 142 ms.

### PNG, TIFF, WebP decode, 4 MP (median, ms; no alternative candidates, the `image`-crate decoders are the only ones in the tree)

| Format | ubuntu-22.04 | macos-latest | windows-2025 |
|---|---:|---:|---:|
| PNG RGB8 (photo-like, written by our `PngEncoder`) | 49.9 | 43.5 | 45.5 |
| TIFF uncompressed | 3.2 | 1.6 | 8.5 |
| TIFF LZW | 73.0 | 62.5 | 62.1 |
| TIFF Deflate | 14.4 | 14.5 | 19.5 |
| WebP lossless | 7.6 | 6.1 | 15.0 |

TIFF LZW (about 60 to 75 ms per 4 MP, about 190 ms per 12 MP) is the slowest and only matters for scanner output.

### Resize, 12 MP to 1024 x 768 (median, ms; 1 thread / all cores of the runner)

| Resizer | ubuntu-22.04 | macos-latest | windows-2025 |
|---|---:|---:|---:|
| in-tree `resize_area` | 45.7 / 25.2 | 28.2 / 9.7 | 33.9 / 16.6 |
| `fast_image_resize` 6.1 Box | 4.5 / 3.1 | 6.8 / 5.3 | 3.5 / 2.2 |
| `fast_image_resize` Bilinear | 6.0 / 3.8 | 13.5 / 8.9 | 4.4 / 2.7 |
| `fast_image_resize` Lanczos3 | 12.8 / 8.5 | 34.8 / 17.4 | 10.2 / 6.4 |
| `pic-scale` 0.7.12 sRGB Bilinear | 5.8 / 3.9 | 6.2 / 3.4 | 4.5 / 7.5 |
| `pic-scale` sRGB Lanczos3 | 12.8 / 7.8 | 12.8 / 9.0 | 10.0 / 10.7 |
| `pic-scale` linear light (f32) Bilinear | 84.5 / 95.5 | 33.0 / 32.1 | 83.2 / 92.8 |
| `pic-scale` linear light (f32) Lanczos3 | 99.8 / 111.0 | 86.8 / 58.0 | 90.2 / 99.2 |
| `pic-scale` linear light (fixed point) Bilinear | 48.5 / 53.0 | 33.6 / 23.9 | 48.0 / 51.2 |
| `image::imageops` Triangle (1 thread only) | 111.5 | 67.7 | 79.0 |
| `image::imageops` Lanczos3 (1 thread only) | 232.2 | 155.0 | 174.0 |

`pic-scale`'s own threading helped its sRGB paths on two of three runners and its linear-light paths on none. The in-tree area resize is 4 to 10x slower than `fast_image_resize` Box on one thread.

### Resize quality (deterministic; dev machine; `cargo run --release -p auto-crop-imgproc-bench --example resize_quality`)

PSNR (dB) and light luma SSIM against an exact f64 area average of the sRGB code values ("gamma average"), and PSNR against the same average taken in linear light ("linear average"), 12 MP synthetic page to 1024 x 768:

| Resizer | sharp page: PSNR vs gamma avg / SSIM / PSNR vs linear avg | blurred page: PSNR vs gamma avg / SSIM / PSNR vs linear avg |
|---|---|---|
| in-tree area | 99.8 / 1.0000 / 32.1 | 100.1 / 1.0000 / 41.1 |
| fast_image_resize Box | 38.3 / 0.9937 / 31.1 | 41.1 / 0.9967 / 38.1 |
| fast_image_resize Bilinear | 36.4 / 0.9893 / 31.9 | 40.4 / 0.9957 / 39.3 |
| fast_image_resize Lanczos3 | 35.8 / 0.9853 / 30.0 | 39.6 / 0.9937 / 36.2 |
| pic-scale sRGB Bilinear | 36.6 / 0.9897 / 31.9 | 40.7 / 0.9960 / 39.5 |
| pic-scale sRGB Lanczos3 | 35.8 / 0.9853 / 30.0 | 39.6 / 0.9937 / 36.2 |
| pic-scale linear Bilinear | 29.1 / 0.9597 / 34.2 | 35.1 / 0.9903 / 39.0 |
| pic-scale linear Lanczos3 | 29.5 / 0.9721 / 32.1 | 33.3 / 0.9907 / 33.2 |
| image Triangle / Lanczos3 | 36.4 / 35.8 | 40.4 / 39.6 |

How much the colour space matters: the exact gamma-space and linear-light averages differ by **32.1 dB** (sharp page, 3.9x reduction; mean level 193.1 versus 194.8, the linear result is lighter by 1.7 levels as dark ink is averaged with paper), 35.7 dB at 2x, and 41.1 / 53.0 dB on the blurred page. The gamma-space average is the reference of the first column, so those PSNRs measure agreement with area averaging, not perceived quality, and favour box-type filters (kernels.md says the same).

## Decision

1. **Decoder.** `zune-jpeg` stays the default full-size JPEG decoder: it is pure safe Rust, needs no C toolchain, parses untrusted bytes in-process without a C parser (B12), agrees with libjpeg-turbo to 0.06 LSB, and is within 1.3 to 1.6x of it. **libjpeg-turbo (feature `turbojpeg`) is used for scaled decode** (`Want::Scaled`, previews and detection proxies, 2.5 to 3.5x faster), in the sandboxed decode worker of ADR-0006 as every C parser is, and for the lossless transform of progressive input; the safe-Rust transformer remains the fallback (both give identical pixels). Builds without the feature get the same sizes and geometry from the safe path.
2. **Resizer.** Adopt **`fast_image_resize`** for proxy and preview downscaling: Box where area semantics are wanted (the 1024 px detection proxy: 3.5 to 6.8 ms against 28 to 46 ms for the in-tree kernel on one thread), Lanczos3 where a sharper result is wanted. `pic-scale` is **not** adopted: its sRGB paths are no faster than `fast_image_resize` (ubuntu-22.04: 5.8 vs 6.0 ms Bilinear, 12.8 vs 12.8 ms Lanczos3 on one thread), and it adds three crates (`colorutils-rs`, `novtb`, `erydanos`); its only differentiator is linear light (rule 4). `image::imageops::resize` is never used in the pipeline (8 to 25x slower than `fast_image_resize` Bilinear). The in-tree `resize_area` stays as the exact, thread-count-independent reference for tests; swapping the engine's proxy to `fast_image_resize` is a follow-up that must keep the engine's "same bytes at 1, 3 and 8 threads" test green.
3. **JPEG encoder.** `codecs::encoders::default_jpeg_encoder()` is libjpeg-turbo when the build has it and `jpeg-encoder` otherwise. Encoders only see our own pixels, so the C library is not an untrusted-input risk. libjpeg-turbo is 2.3 to 3.5x faster than `jpeg-encoder` and 4 to 9x faster than the `image` encoder, with the smallest files; `jpeg-encoder` is 1.8 to 2.5x faster and 11% smaller than the `image` encoder, so even without the feature the skeleton's `encode` stage should move off the `image` encoder. Both embed ICC and density, and the `jpeg-encoder` tables carry the IJG attribution (`about.hbs`, native-library section).
4. **Linear-light rule.** **Resample in gamma-encoded space** (the PLAN 3.7 default). Linear light costs 5 to 18x more for pic-scale's f32 path (33 to 85 ms against 4.5 to 6.2 ms Bilinear at 1 thread) and 5 to 11x for its fixed-point path, shifts mean level by up to 1.7 levels (lighter ink, thinner text) on the sharp page, and the detectors and thresholds are tuned on sRGB proxies. Revisit only for a reduced-size *export*, where physical correctness may outweigh parity with other tools (a harness A/B, PLAN 5.14).

**GO:** keep zune-jpeg as the full decoder, use libjpeg-turbo for scaled decode, encode and progressive transforms where it is built, adopt `fast_image_resize` for proxies in gamma space, and drop `pic-scale` and linear-light resampling for 1.0.

## Consequences

- **Code.** `Want::Scaled` (`codecs::decode_scaled`), `turbo::Transformer`, the encoders and `default_jpeg_encoder()` exist and are tested (see [codecs-hostile-input.md](../testing/codecs-hostile-input.md)); the engine still has to call them (the `core::Decoder` port implementation is engine wiring), and the proxy resize still has to move to `fast_image_resize` (follow-up; it needs the dependency in `imgproc` or a `codecs` re-export, because `check-deps` keeps codec crates out of `imgproc`).
- **Build.** Official builds enable `turbojpeg` (CMake, NASM and a C compiler on the release machines, ADR-0004); contributors without them build and test the pure-Rust default.
- **Open.** The numbers are synthetic-content, runner-grade measurements: re-run on the real-device corpus and on the target hardware (docs/perf/hardware.md) before relying on a budget; `STOPONWARNING` strictness and CMYK fallback are recorded in codecs-hostile-input.md. `pic-scale` stays a dev-dependency of `imgproc-bench` only.
- **Revisit trigger:** zune-jpeg gaining DCT-domain scaling or becoming more than 1.5x slower than libjpeg-turbo on real photos; a `fast_image_resize` release that changes Box or Lanczos3 output; an export option for reduced-size output (linear-light question).
