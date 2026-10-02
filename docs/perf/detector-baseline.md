# Classical detector baseline on the synthetic stand-in suites

First measurement of the shipped classical page detector (`auto_crop_imgproc::detect::detect`) with the accuracy harness (`crates/eval`, ROADMAP M1.45-M1.49). Measured 2026-10-02 at commit `f518f40` (harness) on Windows 11 x86-64, release build.

## Read this first

- **Synthetic data only.** Every image is produced by the stand-in generator in `crates/eval/src/synth.rs` (labelled STAND-IN: it is not the M1.30 Python/Augraphy generator). It is built on the same Rust scene renderer the detector's own unit tests use, so the detector and the data share a lineage.
- **These numbers detect regressions between two builds. They say nothing about real-world accuracy** and never back the B6 silent-failure claim (PLAN 7.4: "Synthetic data only detects regressions and never satisfies B6"). The golden set (M1.39 onward) is where real accuracy gets measured; none of it exists yet.
- The detector is a deliberately simple baseline (its own module docs say so). Its confidence is an uncalibrated heuristic; the Good/Check/Failed mapping below (Good at score >= 0.90, Failed below 0.60 or when it reports no quad) is the interim Balanced cutoff from PLAN 7.4, not a calibrated operating point.
- Aggregates only. No per-image rows are recorded here.

## Method

```
cargo xtask synth --suite smoke          # 200 images, 480 px long edge, seed 0x5EEDA070C20B0001
cargo xtask synth --suite full           # 5,184 images, same generator and seed
cargo xtask eval run --manifest target/synth/<suite>/manifest.jsonl --predictor detector \
    --out target/synth/<suite>-detector.json --suite <suite>
```

Ground truth is the analytic page quad from the generator's pinhole camera (TL, TR, BR, BL, 0..1 coordinates of the upright, EXIF-oriented image). Metrics follow `docs/testing/eval-harness.md`: IoU after canonical warp with the harness's own homography and polygon clipper, corner error as a percentage of the image diagonal, skew, failure at IoU < 0.90. A missing quad ("no page found") counts as a failure with IoU 0. Results were byte-identical at 1 and 8 threads (`cmp` on the smoke result files, and the `self-check` and unit tests on every run).

## Aggregate results

| | Smoke (n = 200) | Full (n = 5,184) | FullFrame baseline, full |
|---|---|---|---|
| Mean IoU | 0.7236 (95% bootstrap CI 0.6618-0.7792) | 0.7090 (0.6976-0.7206) | 0.2619 (0.2592-0.2650) |
| Failure rate (IoU < 0.90, incl. no quad) | 29.00% (58) | 31.00% (1,607) | 100% |
| Success at IoU >= 0.95 | 71.00% | 68.83% | 0% |
| Success at IoU >= 0.98 | 61.00% | 61.28% | 0% |
| No quad found | 38 (19.0%) | 1,094 (21.1%) | 0 |
| Corner error, % of diagonal, answered images: p50 / p95 / p99 | 0.210 / 27.5 / 36.7 | 0.215 / 26.3 / 36.8 | 26.5 / 36.4 / 38.3 |
| Skew degrees, answered images: p50 / p95 / p99 | 0.207 / 0.968 / 27.3 | 0.211 / 1.207 / 8.94 | n/a |
| Corner error p50 over all images (no quad counted as 100%) | 0.236 | 0.246 | 26.5 |
| Auto-accepted (Good) images | 101 | 2,350 | 5,184 |
| Silent failures among auto-accepted | 12 (11.9%) | 271 (11.5%), one-sided 95% bound 12.7% | all |
| Flag rate (Check or Failed) | 49.5% | 54.7% | 0% |
| Calibration (reported, not gated): ECE / Brier / AUROC | 0.160 / 0.138 / 0.747 | 0.158 / 0.133 / 0.809 | n/a |

Corner-error and skew percentiles over all images hit the worst-case penalty values at p95 and p99 (100% of the diagonal, 90 degrees) because more than 5% of images have no quad; the "answered images" rows are the geometry quality of the answers actually given.

Orientation: of the answered images that are geometrically right (IoU >= 0.90), 3,536 list their corners upright, 22 start one corner along (rot90) and 19 start three along (rot270); none are rot180 or mirrored (smoke: 140, 1, 1). The IoU cannot see this, the corner error does.

## Per-slice results (full suite, every slice n >= 1,728 so all are gated-size)

| Slice | n | Mean IoU | Failure rate |
|---|---|---|---|
| lighting = normal | 1,728 | 0.9467 | 4.05% |
| lighting = dim | 1,728 | 0.9347 | 5.15% |
| lighting = low-contrast | 1,728 | 0.2454 | 83.80% |
| clutter = none | 1,728 | 0.7566 | 27.84% |
| clutter = light | 1,728 | 0.7090 | 30.90% |
| clutter = heavy | 1,728 | 0.6613 | 34.26% |
| tilt = 0-10 deg | 1,728 | 0.7302 | 27.55% |
| tilt = 10-30 deg | 1,728 | 0.7140 | 30.44% |
| tilt = 30-45 deg | 1,728 | 0.6827 | 35.01% |
| aspect = document | 2,592 | 0.7088 | 33.99% |
| aspect = receipt | 2,592 | 0.7091 | 28.01% |
| format = jpeg | 2,592 | 0.7080 | 31.02% |
| format = png | 2,592 | 0.7099 | 30.98% |

Worst slice by mean IoU and by failure rate: `lighting=low-contrast`. On the 200-image smoke set the lighting, clutter and tilt slices have n = 63-72 (advisory, n < 80) and the aspect and format slices n = 92-108 (gated size), so the per-slice gate has little power there and the PR gate leans on the aggregate thresholds. The smoke numbers track the full suite (the smoke mean IoU is within 1.5 points of the full one, inside its own CI).

## What the numbers say (about this detector on this generator)

- The detector is accurate when it finds the page: median corner error about 0.2% of the diagonal and median skew about 0.2 degrees on the 4,090 answered images.
- Almost all of the failures come from one slice. Low-contrast scenes (desk within 16-30 grey levels of the paper) account for 1,448 of 1,607 failures (952 no-quad, 496 wrong quad). Normal and dim lighting fail 4-5% of the time, mostly because no quad was reported.
- Of 2,350 auto-accepted images, 271 failed; 257 of them are low-contrast. The detector's score separates good from bad only moderately (AUROC 0.81) and is over-confident (ECE 0.16), as expected of an uncalibrated heuristic. The M4 calibration work, not this baseline, is where that is addressed.
- 41 geometrically correct answers list their corners starting at the wrong corner. That is a real orientation-ordering defect worth a ROADMAP item.

## Caveats and deliberate limits

- The generator draws one flat desk texture with soft shadows and rectangular distractors, two paper kinds (a receipt at about 3.1:1 and a page at about 1.4:1, not the plan's > 4:1), JPEG (q40-95) and PNG, and EXIF orientation 1 only. It has no curl, no partial frames, no multi-item scenes, no real-world degradations and no no-document negatives.
- Low-contrast scenes were made deliberately hard (paper minus 16-30 grey levels, with blur, noise and clutter); the 83.8% failure rate there reflects that choice as much as the detector.
- The smoke set is 18.5 MB (JPEG and PNG, noise and clutter included), not the <= 5 MB of M1.35; CI regenerates it from the seed instead of storing it (see `docs/testing/eval-harness.md`).
- Bit-exact regeneration across operating systems is not promised (f64 trigonometry in the camera model); numbers are comparable within one platform and between `main` and a PR built on the same runner.
