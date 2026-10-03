# Imaging kernel measurements (M1.22-M1.27, M1.56, M1.57)

Roadmap items: M1.22 pyramid, M1.23 homography, M1.24 warp, M1.25 warp oracles, M1.26 minification guard, M1.27 thresholds, M1.56 resize benchmark, M1.57 kernel benchmark. Code: `crates/imgproc`, benches in `crates/imgproc-bench`. Design: [PLAN 7](../plan/07-performance-accuracy-quality.md), [ADR-0002](../adr/0002-lanczos-warp.md).

## Host and honesty statement

| | |
|---|---|
| Host label | **LUNCHBOX, desktop, indicative** (see [hardware.md](hardware.md)): Windows 11 Pro 10.0.26200, Intel Core i7-8700K 6C/12T 3.7 GHz (AVX2), 31.9 GB, rustc 1.98.1, `cargo bench` release profile (LTO off, default `x86-64` target, no `target-cpu`), criterion 0.8.2, rayon pools of 1 and 8 threads |
| Load during these runs | **Not idle.** An unrelated `ffmpeg` job held about 9-10 of the 12 hardware threads for the whole session (plus one Python process and other agents' compiles). Absolute times below are therefore pessimistic, by an unmeasured factor that is probably 1.3-2x (the frozen legacy kernel measured 648 ms here against the 452 ms of the ADR-0002 spike on the same code and size), and 8-thread cells are noisier than 1-thread cells |
| What to trust | **Ratios between kernels measured in the same run** (legacy vs new), not nanoseconds. Re-run on an idle machine before quoting any absolute number: `cargo bench -p auto-crop-imgproc-bench` |
| Gate status | **The M2.15 gate (`rectify` <= 90 ms at 12 MP, 8 threads) is NOT claimed.** The warp measured 123 ms for 12 MP to 2480 x 3508 on this loaded host; scaled by the legacy kernel's load factor that would be about 90 ms on an idle machine, which is an estimate, not a measurement. Not a Tier-M laptop result either |

## Warp (M1.24, M1.26, M1.57)

Source: synthetic 4:3 document photo (gradient, text-like rectangles, noise), tilted-quad homography to the output, RGB8. `legacy` is the frozen ADR-0002 spike kernel (`crates/imgproc-bench/src/legacy_warp.rs`), `plain` the new kernel, `guarded` the new kernel with the pyramid guard. Median of criterion samples (10 per cell).

| Source to output | Threads | legacy | plain | guarded | plain vs legacy |
|---|---|---:|---:|---:|---:|
| 12 MP to 2480 x 3508 (8.7 MP) | 8 | 648 ms | 123 ms | 124 ms | 5.3x |
| 12 MP to 2480 x 3508 | 1 | 3.76 s | 815 ms | 825 ms | 4.6x |
| 48 MP to 2480 x 3508 | 8 | 533 ms | 124 ms (aliases, scale 2.3) | 139 ms | 4.3x |
| 100 MP to 2480 x 3508 | 8 | 908 ms | 129-234 ms (aliases, scale 3.4) | 176 ms | 3.9-7x |
| 12 MP to 12 MP (ADR-0002 protocol) | 8 | 776 ms | 172-225 ms | n/a | 3.5-4.5x |
| 12 MP to 12 MP | 1 | 5.2 s | 1.11 s | n/a | 4.7x |
| 12 MP to 1024 x 768 (heavy minification) | 8 | 72 ms | 27 ms | 87 ms | |
| 48 MP to 1024 x 768 | 8 | 62 ms | 16.5 ms | 36 ms | |
| 100 MP to 1024 x 768 | 8 | 63 ms | 18 ms | 62 ms | |

Reading the table:

- **Speed-up over the spike kernel is about 4-5x at equal load** (same process, same machine, round-robin runs in `crates/imgproc-bench/examples/tune.rs` gave 0.19-0.21 of legacy for RGB8 single-thread). Where it comes from, each step measured: no libm software `fmaf` (the spike called `f32::mul_add` on a baseline x86-64 target without FMA, by far the largest cost), a vertical-first blend of the six source rows into one f32 vector (plain mul and add the compiler turns into SSE2 vectors) followed by a four-lane horizontal pass, `N` padded to a multiple of four so the blend is whole vectors, fixed-point (1/65536) source positions computed in f64 per row with a mantissa trick instead of saturating float-to-int casts, the border path kept out of line, and exact whole-pixel positions copied instead of convolved. Two ideas did **not** pay and were dropped: balanced (tree) sums to shorten the dependency chain, and `-C target-cpu=native` (autovectorised AVX2 only gained about 15%, which does not justify an `unsafe` `simd/` module under M1.72; a hand-written AVX2 kernel might, and is the next step if the idle-machine number misses 90 ms).
- 16-bit (RGB16) costs about the same as RGB8 (80 vs 87 ns per pixel in the tuning run); Gray8 about half.
- **The guard is not free**: it builds the reduced levels (cascaded 2 x 2 halving, `minify::reduce_half`, about 1.3 passes over the source) before warping. It pays for itself in correctness, not time: unguarded 48 and 100 MP to A4 alias (zone-plate test below), and the first version of the guard (exact one-pass box reduction with u64 sums) cost 536 ms at 48 MP before `reduce_half` brought it to 139 ms. For a 12 MP source at roughly 1:1 the guard changes nothing (identical bytes to `plain`, tested).
- ADR-0002 open items still open: AVX2/NEON with runtime dispatch, per-strip affine approximation, fewer taps when minifying (now handled structurally by the guard).

### Memory (M1.24) - `tests/warp_memory.rs`

Counting allocator (`peak_alloc`), peak heap above the heap in use at call start:

| Case | Extra peak | Output | Budget (output + 16 MB + band scratch) |
|---|---:|---:|---:|
| 12 MP to 2480 x 3508, 1 thread | 26.14 MB | 26.10 MB | 43.2 MB |
| 12 MP to 2480 x 3508, 8 threads | 26.43 MB | 26.10 MB | 43.5 MB |
| 100 MP (11547 x 8660) to 2480 x 3508, 8 threads (`--ignored`) | 26.42 MB | 26.10 MB | 43.5 MB |

The scratch beyond the output is under 0.4 MB (two i64 position rows per band). No full-frame f32 copy exists.

### Accuracy (M1.25) - `tests/oracles.rs`, `crates/imgproc-bench/tests/ssim_oracles.rs`

Fixtures are generated offline by `tools/imgproc-oracles/gen_oracles.py` (NumPy 2.5.3, OpenCV 5.0.0.93 pinned in `requirements.txt`) and checked in (`crates/imgproc/tests/fixtures`, 0.9 MB).

| Check | Result | Bar |
|---|---|---|
| Identity, integer shifts, 90/180/270 turns, Gray8 / RGB8 / RGBA8 / Gray16 / RGB16 | bit-exact | exact |
| 27 sub-pixel shift / perspective / scale / 3-degree rotation cases vs a NumPy float64 Lanczos3 (exact weights), Gray8, RGB8, RGB16 | worst difference 1 LSB (8-bit) and 1 LSB (16-bit) | <= 1 LSB |
| PSNR vs `cv2.warpPerspective` INTER_LANCZOS4 (OpenCV has no Lanczos3), photo-like content (blur sigma 1 and 1.5) | 51.3 dB and 52.9 dB | >= 45 dB (PROVISIONAL) |
| Same, **hard one-pixel edges everywhere** (`blur0`) | **41.8 dB** | below 45 |
| MSSIM (`image-compare`) vs NumPy reference, 9 RGB8 cases | >= 0.99999 | >= 0.99 |
| MSSIM vs OpenCV, 3 cases | 0.9965 (blur0), 0.9988, 0.9994 | >= 0.99 |

Plan/reality note: the 45 dB bar holds for photo-like content but not for synthetic hard edges, because OpenCV's 8-tap kernel and its 1/32-pixel coordinate quantisation differ most from a 6-tap kernel there (ADR-0002 measured 49.3 dB on denser, softer content). The test keeps the `blur0` case as a regression guard at 40 dB and asserts 45 dB on the others; the bar is not relaxed, but **the M2.15 acceptance should state the content the 45 dB applies to** (owner decision). SSIM across OSes (>= 0.98) is unmeasured until CI runs the oracle on three OSes.

### Minification guard (M1.26) - `tests/minify.rs`

Alias energy: mean squared deviation from mid-grey over the part of a zone plate (1000 x 1000, frequency reaching Nyquist at the corners) whose local frequency is above the output's Nyquist; the ideal output there is flat. Relative to an exact area average:

| Scale | guarded | unguarded 6-tap | note |
|---|---:|---:|---|
| 2x | +0.00 dB | +6.35 dB | roadmap case |
| 4x | +0.00 dB | +14.71 dB | roadmap case |
| 2.5x | +3.82 dB | +11.57 dB | residual 1.25 after level 1 |
| 3x | **+7.27 dB** | +14.18 dB | residual exactly 1.5 |
| 5x | +1.56 dB | +16.17 dB | residual 1.25 after level 2 |

The roadmap cases (2x, 4x) are within 3 dB (they are exact box averages). **Finding:** with the PROVISIONAL threshold of 1.5, scales that are not powers of two leave up to +7.3 dB of alias energy (worst just at residual 1.5). The calibration printout (`cargo test --release -p auto-crop-imgproc --test minify -- --ignored --nocapture`) shows threshold 1.0 keeps every tested scale at or below 0 dB of the area average (it over-smooths slightly instead) and 1.25 fixes scale 3 but not 2.5. Not changed: the threshold is a roadmap value, so this goes to the owner with the numbers.

Seam test: a perspective map whose local scale runs from about 1 to above 16 switches through 5 levels (0-4); the output deviates from the analytic smooth image by at most 1.45 LSB everywhere (bar: <= 2 LSB). The cascade's rounding offset alternates 1, 2 in a checkerboard; plain round-half-up drifted the deep levels by about a level (2.35 LSB, failing) before that fix.

## Homography (M1.23) - `tests/homography_props.rs`, `tests/oracles.rs`

Normalised DLT (Hartley) in f64 with complete-pivot elimination (no SVD crate). 4000 proptest cases (300,000 run once during development) of random convex quad pairs in a 11,000 x 8,000 frame, aspect 0.1 to 10: corners map within 1e-9 px and `inverse` round-trips within 1e-9 px (bar: < 1e-9). 48 cv2 `getPerspectiveTransform` fixtures: worst relative entry difference 5.4e-8 (bar: 1e-6; cv2 takes float32 input, the inputs are float32-exact). Duplicate, collinear and non-finite input returns `HomographyError::Degenerate` (converts to core's `ErrKind::Degenerate`); never a panic.

## Pyramid and tiles (M1.22) - `src/pyramid.rs`

256 px thumbnail, 1024 px detection, about 1.5 MP analysis (long edge <= 3072), about 3 MP display (long edge <= 4096), never upscaled, each level cascaded from the next larger one. Tests: sizes for 12 / 24 / 48 / 100 MP and a 10:1 strip, small sources unscaled and sharing, tiles at 64 / 512 / 1000 / 4096 rebuild every level bit-exactly (plus a proptest over arbitrary sizes and tile sizes), same bytes at 1 and 8 threads. `resize_area` is now parallel and **byte-identical** to the serial original (the original is kept in the tests as the reference).

## Thresholds (M1.27) - `src/threshold.rs`

`u64` column sums per 256-row strip, O(width) scratch per worker (the 12 MP extra peak is 13.5 MB: the 12 MB output plus about 1.5 MB, against 192 MB for two whole-image u64 integral planes), O(1) per pixel for any window. Equal to the naive window scan at windows 1, 3, 9, 31, 51, 101, 400 and strip heights 1 to 1000; same bytes at 1 and 8 threads; a 19.4 MP image of 255 with window 4001 (window sums over 10^9, squares over 10^12, both beyond u32) stays white for Sauvola and NICK; Otsu counts of 2 x 10^10 do not overflow. **Not measured:** "within 0.5 F-measure of Doxa on 3 DIBCO images" - no DIBCO images are available offline here; the item stays open for that clause.

12 MP grey page (4000 x 3000), median:

| Kernel | window | 8 threads | 1 thread |
|---|---:|---:|---:|
| Otsu + binarise | global | 5.5 ms | 22.8 ms |
| Sauvola | 31 / 51 / 101 | 62.7 / 62.2 / 61.5 ms | 355 / 355 / 357 ms |
| NICK | 31 / 51 / 101 | 38.3 / 39.5 / 37.9 ms | 212 / 214 / 219 ms |

Time is flat in the window size (O(1) per pixel). Sauvola is dominated by the per-pixel `sqrt`; the `enhance` budget (PLAN 7.1: 120-250 ms) holds on this loaded host with room to spare, but the 48 and 100 MP rows are not yet measured.

## Resize (M1.56) - `benches/resize.rs`, `examples/resize_quality.rs`

Median time, 12 MP source (loaded host):

| Resizer | to 1024 x 768, 1 thread | 8 threads | to 2000 x 1500, 1 thread | 8 threads |
|---|---:|---:|---:|---:|
| `imgproc::scale::resize_area` | 93 ms | 15.4 ms | 116 ms | 17.4 ms |
| `fast_image_resize` 6.1 Box | 8.6 ms | 4.0 ms | 17.8 ms | 15-30 ms (noisy) |
| `fast_image_resize` Bilinear | 10.8 ms | 5.1 ms | 18.6 ms | 7.9 ms |
| `fast_image_resize` Lanczos3 | 24.3 ms | 7.5 ms | 32.4 ms | 10.3 ms |
| `image` Triangle (1 thread only) | 243 ms | | 387 ms | |
| `image` Lanczos3 (1 thread only) | 546 ms | | 584 ms | |

48 MP to 1024 x 768: own 261 ms (1t) / 39.5 ms (8t); fir Box 24.4 / 10.9 ms; fir Lanczos3 103 / 21.5 ms; `image` Triangle 547 ms, Lanczos3 1.58 s.

**The in-tree area resize is 4-11x slower than `fast_image_resize` per thread** (the library is SIMD with runtime dispatch). The roadmap's `proxy` line (Table A: 25 ms at 12 MP) is met by the 8-thread own resize (15-17 ms here) but not single-threaded (93-116 ms), and batch mode runs jobs single-threaded. Recommendation for the M1.56 ADR: use `fast_image_resize` for proxies, after checking that its output is identical across SIMD levels (determinism rule) - `pic-scale` was not benchmarked (not in the tree).

Quality against an exact f64 area average (PSNR / light 8 x 8 luma SSIM; `cargo run --release -p auto-crop-imgproc-bench --example resize_quality`), 12 MP to 1024 x 768, blurred page: own area 100 dB / 1.0000 (it *is* the reference up to rounding), fir Box 41.1 dB / 0.9967, fir Bilinear 40.4 / 0.9957, fir Lanczos3 39.6 / 0.9937, `image` Triangle 40.4 / 0.9957, `image` Lanczos3 39.6 / 0.9938; the sharp synthetic page scores 2-3 dB lower for the libraries (35.8-38.3 dB). These measure agreement with area averaging, not perceived quality, and the reference favours the box-type filters; the linear-light rule is decided in [ADR-0008](../adr/0008-codec-kernel-choices.md) (gamma space), with `pic-scale` and CI-runner timings.

## Plan and reality conflicts recorded

1. `check-deps` forbids `image` (any dependency kind) in `auto-crop-imgproc`, so the library comparisons, `image-compare` SSIM and all criterion benches live in the new workspace crate `crates/imgproc-bench` (not shipped, `publish = false`).
2. `auto-crop-core` now has `CancelToken` and `ErrKind::Degenerate`; `imgproc` takes a small `Cancel` trait that core's token implements, and `HomographyError` converts to `ErrKind::Degenerate`.
3. M1.26: the 1.5 threshold leaves +7.3 dB alias energy at scale 3 (above).
4. M1.25/M2.15: 45 dB vs OpenCV holds for photo-like content, 41.8 dB for hard-edged synthetic content (above).
5. M1.27: the Doxa/DIBCO F-measure clause is unmeasured.
6. M1.57/M2.15: the 90 ms warp gate is unmeasured on idle hardware (above); the 1-thread resize misses the 25 ms proxy line.
