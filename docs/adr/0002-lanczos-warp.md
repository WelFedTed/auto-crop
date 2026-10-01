# 0002 - Lanczos perspective warp: own strip-wise u8 kernel

- **Status:** accepted
- **Date:** 2026-10-01
- **Roadmap items:** M0.36, M0.37, M0.38 (feeds M1.23-M1.26, M2.14, M2.15)
- **Decision log links:** D6 (pure-Rust kernel gaps)
- **Time box:** 3 days (PROVISIONAL); actual about half a day. Code: `spikes/warp/` (throwaway, outside the workspace).

## Context

Auto Crop needs one high-quality resample of the full-resolution image through a perspective (homography) map. `imageproc` has no Lanczos warp, and a general f32 warp keeps whole-image f32 buffers, which is costly at 48-100 MP. The spike compares three implementations on identical input.

## Options considered

All three use Lanczos3 (6 taps per axis, weights normalised, clamped edges), integer-centre pixel convention, destination to source mapping, zero outside the source.

| Option | Description |
|---|---|
| **ref** | f64 reference, per output pixel (authoritative for PSNR) |
| **kornia** | `kornia-imgproc` 0.2.0 `warp_perspective` (f32, `InterpolationMode::Lanczos`, Apache-2.0). Needs an f32 copy of the source and destination, so the measured path is u8 to f32, warp, then u8 |
| **own** | strip-wise u8 kernel: 64-row rayon strips, 1024-entry Lanczos3 weight table, f32 accumulation, u8 in and out, no full-frame f32 buffer |

## Results

Protocol per [spikes.md](spikes.md), with deviations listed below. Machine: dev desktop (Windows 11, Intel Core i7-8700K 6C/12T, 32 GB, AC power), `cargo build --release` with the default x86-64 target (no AVX2, no SIMD tuning), 8-thread and 1-thread rayon pools, first run dropped as warm-up. Synthetic RGB image (gradient plus 6,000 text-like rectangles plus noise), output equals source size, tilted-receipt quadrilateral.

**Accuracy (PSNR against the f64 reference, higher is better; bar >= 45 dB, PROVISIONAL):**

| Output | own | kornia | own vs kornia |
|---|---|---|---|
| 12 MP (4000x3000) | 71.42 dB | 75.76 dB | 70.31 dB |
| 48 MP (8000x6000) | 71.79 dB | 75.42 dB | 70.84 dB |

OpenCV oracle (dev-only, Python `cv2` 5.0.0, 12 MP, `warpPerspective` with the same matrix): Lanczos4 vs reference 49.27 dB, bicubic 47.68 dB, bilinear 38.02 dB. OpenCV has no Lanczos3, so the PSNR is limited by the kernel difference; the point is that the geometry and coordinate convention agree. `cargo tree -i opencv` is empty (the oracle is Python only).

**Speed (median / p95 ms; bar for 12 MP at 8 threads: <= 90 ms, PROVISIONAL):**

| Output | Threads | Samples | own | kornia (warp only, f32 source ready) | kornia (u8 to f32, warp, u8) |
|---|---|---|---|---|---|
| 12 MP | 8 | 20 | 452 / 547 | 1004 / 1032 | 994 / 1009 |
| 12 MP | 1 | 10 | 2578 / 2599 | 5353 / 5395 | 5400 / 5531 |
| 48 MP | 8 | 10 | 2155 / 2972 | 3899 / 4132 | 4095 / 4627 |
| 48 MP | 1 | 3 | 10249 / 10252 | 21276 / 21445 | 21412 / 21831 |

**Extra peak heap above the source and output buffers (allocator-counted, not process RSS):** own 0.1 MB at both sizes; kornia full path 324 MB at 12 MP and 1296 MB at 48 MP (about 2.6 GB expected at 100 MP).

**Licences:** kornia-imgproc, kornia-image: Apache-2.0 (allowed). The own kernel has no new dependencies beyond `rayon`.

**Protocol deviations:** fewer samples than the 20-sample rule for 1-thread and 48 MP cells (time box); the own 8-thread cells show a p95 21% (12 MP) and 38% (48 MP) above the median, above the 5% variation rule, probably because light `gh` API calls ran during those samples; all other cells are within 5%. Numbers are for a desktop CPU and are labelled "desktop, indicative", not a laptop result. Memory is allocator peak, not RSS.

## Decision

Use the **own strip-wise u8/u16 Lanczos3 warp** as the engine kernel. It matches the f64 reference at 71 dB, is about 2.2x faster than kornia at the same thread count, and needs almost no scratch memory, which is decisive for 48-100 MP images (kornia's f32 path would need over 2 GB of extra memory at 100 MP). kornia-imgproc stays a dev-only comparison.

**GO:** adopt the own strip-wise u8/u16 Lanczos3 warp (accuracy bar met). **The speed bar is NOT met**: 452 ms at 12 MP and 8 threads against the 90 ms provisional bar, about 5x away without SIMD.

## Consequences

- **M1.23-M1.26 and M2.14/M2.15** implement the kernel from this spike: strip-wise rows, weight table, u8 and u16 in and out, a coordinate provider for homography now and a dense map later (dewarp, M12), clamped output against ringing, tests against an f64 reference and the dev-only OpenCV oracle, with **pinned golden outputs in CI** (the oracle never runs in CI).
- **Open items (speed):** this spike did no SIMD tuning. Next: AVX2/NEON tap loops with runtime dispatch (the default build targets baseline SSE2), fixed-point weights, per-strip affine approximation of the homography, skipping fully outside pixels, and fewer taps when the map downscales. A typical document crop outputs 2-4 MP, so the same kernel should cost roughly a third to a quarter of the 12 MP numbers.
- **Budget risk:** at 452 ms the warp alone is 65% of the provisional 700 ms end-to-end budget for a 12 MP image (PLAN 7.1). If SIMD tuning cannot bring it to the order of 100-150 ms at 12 MP, the kernel gate (PLAN 7.7: warp <= 90 ms) and the Table A budget must be re-baselined with measured numbers. Both are provisional engineering numbers, not owner decisions.
- **Other open items:** linear-light resampling, and a final decision on u16 handling, are M1 questions.
- **Revisit trigger:** measured warp time on Tier-M laptop hardware (M0.47) after SIMD, or a new maintained Rust crate offering a strip-wise Lanczos warp.
