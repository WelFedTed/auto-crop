# Classical page detector on the synthetic stand-in suites: baseline and the line-based rewrite

Two measurements of the classical page detector (`auto_crop_imgproc::detect::detect`) with the accuracy harness (`crates/eval`, ROADMAP M1.45-M1.49), both on Windows 11 x86-64, release build:

- **Before**: the first detector (global brightness threshold, best connected region, hull fitted with four lines), measured 2026-10-02 at the harness commit `f518f40`.
- **After**: the same entry point rebuilt around edge lines (this change), measured 2026-10-02. What changed is in "What changed" below.

## Read this first

- **Synthetic data only.** Every image is produced by the stand-in generator in `crates/eval/src/synth.rs` (labelled STAND-IN: it is not the M1.30 Python/Augraphy generator). It is built on the same Rust scene renderer the detector's own unit tests use, so the detector and the data share a lineage.
- **These numbers detect regressions between two builds. They say nothing about real-world accuracy** and never back the B6 silent-failure claim (PLAN 7.4: "Synthetic data only detects regressions and never satisfies B6"). The golden set (M1.39 onward) is where real accuracy gets measured; none of it exists yet. No roadmap gate (G-numbers, B6) is claimed met by anything on this page.
- Large synthetic gains can come from fitting the generator's habits. The constants of the new detector were chosen on two seeds, checked on a third, and then run once on a fourth that did not exist until they were frozen (see "Other seeds and sizes"); it was also run on other image sizes. That guards against tuning to one seed; it cannot rule out a quirk shared by every seed of the generator, which is why the caveats below matter more than the numbers.
- The confidence is still an uncalibrated heuristic; the Good/Check/Failed mapping (Good at score >= 0.90 with no forced band, Failed below 0.60 or when it reports no quad) is the interim Balanced cutoff from PLAN 7.4, not a calibrated operating point.
- Aggregates only. No per-image rows are recorded here.

## Method

```
cargo xtask synth --suite smoke          # 200 images, 480 px long edge, seed 0x5EEDA070C20B0001
cargo xtask synth --suite full           # 5,184 images, same generator and seed (its first 200 are the smoke set)
cargo xtask synth --suite full --count 1728 --seed 1234567 --out target/synth/mid-b      # other seeds: balanced, 1,728 images
cargo xtask synth --suite smoke --max-edge 1024 --out target/synth/smoke-1024             # other sizes
cargo xtask eval run --manifest target/synth/<suite>/manifest.jsonl --predictor detector \
    --out target/synth/<suite>-detector.json --suite <suite>
cargo xtask eval compare --base base.json --head head.json
```

Ground truth is the analytic page quad from the generator's pinhole camera (TL, TR, BR, BL, 0..1 coordinates of the upright, EXIF-oriented image). Metrics follow `docs/testing/eval-harness.md`: IoU after canonical warp with the harness's own homography and polygon clipper, corner error as a percentage of the image diagonal, skew, failure at IoU < 0.90. A missing quad ("no page found") counts as a failure with IoU 0. Results are byte-identical at any thread count. "Before" files were produced by the old detector's build on the same manifests (same manifest SHA-256), so `eval compare` pairs them image by image.

## Aggregate results (smoke and full, default seed)

| | Smoke before | Smoke after | Full before | Full after |
|---|---|---|---|---|
| Images | 200 | 200 | 5,184 | 5,184 |
| Mean IoU (95% bootstrap CI) | 0.7236 (0.6618-0.7792) | 0.9772 (0.9680-0.9839) | 0.7090 (0.6976-0.7206) | 0.9792 (0.9777-0.9806) |
| Failure rate (IoU < 0.90, incl. no quad) | 29.00% (58) | 1.50% (3) | 31.00% (1,607) | 1.85% (96) |
| Success at IoU >= 0.95 / >= 0.98 | 71.0% / 61.0% | 96.5% / 76.0% | 68.8% / 61.3% | 96.5% / 81.3% |
| No quad found | 38 | 0 | 1,094 | 3 |
| Corner error, % of diagonal, answered images, p50 / p95 / p99 | 0.210 / 27.5 / 36.7 | 0.175 / 0.80 / 8.0 | 0.215 / 26.3 / 36.8 | 0.166 / 0.86 / 5.7 |
| Skew degrees, answered images, p50 / p95 / p99 | 0.207 / 0.97 / 27.3 | 0.137 / 0.62 / 0.80 | 0.211 / 1.21 / 8.9 | 0.113 / 0.69 / 1.16 |
| Auto-accepted (Good) images | 101 | 175 | 2,350 | 4,558 |
| Silent failures among auto-accepted | 12 (11.9%) | 0 (0.0%, one-sided 95% bound 1.70%) | 271 (11.5%, bound 12.7%) | 6 (0.13%, bound 0.26%) |
| Flag rate (Check or Failed) | 49.5% | 12.5% | 54.7% | 12.1% |
| Share of flagged images that were fine (IoU >= 0.90) | 53.5% | 88.0% | 52.9% | 85.6% |
| Calibration (reported, not gated): ECE / Brier / AUROC | 0.160 / 0.138 / 0.747 | 0.019 / 0.012 / 0.985 | 0.158 / 0.133 / 0.809 | 0.017 / 0.015 / 0.908 |

The "silent failure" figure is the number the plan cares about, and the "bound" is the one-sided 95% Clopper-Pearson upper bound. With zero or a handful of silent failures the bound on 200 images is still above 1.5%; only the 5,184-image suite has the sample size to say "below 0.3% on this generator". **That is a statement about this generator, not about B6** (which is measured on the golden set).

The flag rate fell from 55% to 12%, but 86% of the flagged images are still fine: the detector now errs on the side of review. That is deliberate (a held item costs the user a look, a silent failure costs a wrong overwritten original), and it is the cost of the conservative rules below.

Orientation of the geometrically right answers (IoU >= 0.90), full suite: before 3,536 upright, 22 rot90, 19 rot270; after 5,082 upright, 1 rot90, 5 rot270. See "Corner order" below.

`eval compare` (the regression gate) base vs head: PASS on smoke and on full. On full every one of the 13 slices improves; two of the 5,184 images newly fail (full-00758 IoU 0.983 to 0.897, full-02810 0.991 to 0.889) and 1,513 newly pass. On the 200-image smoke set the advisory slice `lighting=normal` is 0.28 points lower in mean IoU (not gated at n < 80, and under the 0.3-point threshold anyway).

## Per-slice results (full suite, every slice n >= 1,728 so all are gated-size)

| Slice | n | Mean IoU before -> after | Failure rate before -> after | Silent failures / auto-accepted before -> after |
|---|---|---|---|---|
| lighting = normal | 1,728 | 0.9467 -> 0.9846 | 4.05% -> 0.00% | 7/1058 -> 0/1616 |
| lighting = dim | 1,728 | 0.9347 -> 0.9868 | 5.15% -> 0.00% | 7/1035 -> 0/1617 |
| lighting = low-contrast | 1,728 | 0.2454 -> 0.9661 | 83.80% -> 5.56% | 257/257 -> 6/1325 |
| clutter = none | 1,728 | 0.7566 -> 0.9806 | 27.84% -> 1.91% | 62/745 -> 2/1565 |
| clutter = light | 1,728 | 0.7090 -> 0.9793 | 30.90% -> 1.79% | 96/792 -> 2/1520 |
| clutter = heavy | 1,728 | 0.6613 -> 0.9776 | 34.26% -> 1.85% | 113/813 -> 2/1473 |
| tilt = 0-10 deg | 1,728 | 0.7302 -> 0.9801 | 27.55% -> 1.50% | 142/784 -> 0/1444 |
| tilt = 10-30 deg | 1,728 | 0.7140 -> 0.9778 | 30.44% -> 2.26% | 86/814 -> 4/1528 |
| tilt = 30-45 deg | 1,728 | 0.6827 -> 0.9796 | 35.01% -> 1.79% | 43/752 -> 2/1586 |
| aspect = document | 2,592 | 0.7088 -> 0.9819 | 33.99% -> 2.16% | 246/917 -> 5/2341 |
| aspect = receipt | 2,592 | 0.7091 -> 0.9764 | 28.01% -> 1.54% | 25/1433 -> 1/2217 |
| format = jpeg | 2,592 | 0.7080 -> 0.9789 | 31.02% -> 1.77% | 138/1179 -> 3/2230 |
| format = png | 2,592 | 0.7099 -> 0.9795 | 30.98% -> 1.93% | 133/1171 -> 3/2328 |

Worst slice by mean IoU and by failure rate, before and after: `lighting=low-contrast`. All 96 remaining failures on the full suite are low-contrast scenes (normal and dim lighting now have none), and 90 of them are held or reported as failed rather than silently accepted. The remaining low-contrast failures are mostly scenes where one page edge is genuinely invisible: the generator's desk shades across the paper's brightness, so along part of an edge the desk is as bright as the paper and no edge exists to find.

## Other seeds and sizes

Development used the default seed (smoke, full) and seed 1234567 (smoke-b, mid-b). Seed 987654321 was checked a few times while the constants were being chosen (a sweep of the gradient threshold gain and floor, and the final run). Seed 31415926 (smoke-d) and seed 20261002 (mid-d) were generated after the constants were frozen and were run once on the frozen build. The 1024 px and 320 px sets reuse the default seed at other image sizes, which exercises the downscale to the 640 px proxy and the opposite direction.

| Set | Role | Mean IoU before -> after | Failure rate before -> after | No quad before -> after | Silent failures / auto-accepted before -> after | Flag rate before -> after |
|---|---|---|---|---|---|---|
| smoke-b (seed 1234567, 200) | dev | 0.6938 -> 0.9787 | 33.00% -> 2.50% | 42 -> 0 | 12/88 (13.64%, bound 21.16%) -> 0/182 (bound 1.63%) | 56.0% -> 9.0% |
| mid-b (seed 1234567, 1,728) | dev | 0.7090 -> 0.9777 | 31.31% -> 1.91% | 361 -> 0 | 88/760 (11.58%, bound 13.67%) -> 1/1531 (0.07%, bound 0.31%) | 56.0% -> 11.4% |
| smoke-c (seed 987654321, 200) | checked | 0.7037 -> 0.9802 | 30.50% -> 2.00% | 46 -> 0 | 10/86 (11.63%, bound 18.93%) -> 0/175 (bound 1.70%) | 57.0% -> 12.5% |
| mid-c (seed 987654321, 1,728) | checked | 0.7130 -> 0.9790 | 30.32% -> 1.85% | 360 -> 0 | 98/788 (12.44%, bound 14.54%) -> 1/1521 (0.07%, bound 0.31%) | 54.4% -> 12.0% |
| smoke-d (seed 31415926, 200) | held out | 0.6724 -> 0.9841 | 35.00% -> 0.50% | 48 -> 0 | 12/92 (13.04%, bound 20.28%) -> 0/176 (bound 1.69%) | 54.0% -> 12.0% |
| mid-d (seed 20261002, 1,728) | held out | 0.7087 -> 0.9771 | 30.79% -> 1.85% | 378 -> 3 | 74/766 (9.66%, bound 11.60%) -> 0/1489 (bound 0.20%) | 55.7% -> 13.8% |
| smoke-1024 (default seed, 1024 px long edge) | size | 0.5795 -> 0.9814 | 42.00% -> 3.00% | 64 -> 1 | 18/68 (26.47%, bound 36.68%) -> 0/177 (bound 1.68%) | 66.0% -> 11.5% |
| smoke-320 (default seed, 320 px long edge) | size | 0.7067 -> 0.9727 | 34.50% -> 3.00% | 35 -> 0 | 10/115 (8.70%, bound 14.30%) -> 1/174 (0.57%, bound 2.70%) | 42.5% -> 13.0% |

The silent-failure count did not rise on any set and fell by an order of magnitude or more on all of them. The held-out seed behaves like the development seeds.

## Engine sample scenes (`crates/engine/src/samples.rs`)

The engine writes eleven samples (the "Try sample images" set; the flow tests open all of them). Against the known corners the specs were rendered from:

| Sample | Before (verdict, IoU) | After (verdict, IoU) |
|---|---|---|
| receipt_1_tilted, receipt_2_straight, receipt_3_leaning, receipt_4_blurry | Good, 0.986-0.992 | Good, 0.984-0.995 |
| document_1_perspective, document_2_slight_tilt, document_3_dark_page_light_desk | Good, 0.974-0.998 | Good, 0.974-0.998 |
| document_4_fills_frame | Check, 0.988 | Good, 0.998 |
| hard_page_cut_by_frame | Check, 0.900 | Check, 0.900 |
| hard_low_contrast | Failed (no quad) | Good, 0.998 |
| hard_white_on_white | Failed (no quad) | Check, IoU 0.07 (a wrong quad, held) |
| hard_no_document (no page in the frame) | Failed | Failed |

`hard_low_contrast` was written as a case that "should land in Check or Failed"; the new detector simply finds it, so the sample no longer shows that tier (the flow test that checks the three tiers still passes through the other samples). `hard_white_on_white` is now held with a wrong quad instead of reported as Failed; it is never written either way. The samples' own behaviour is a demo concern, not an accuracy result.

## Speed

Single thread, detection only (images decoded once beforehand, mean of 3 passes over 60 images, 11 for the samples), release build, this machine. The detector works on a proxy of at most 640 px on the long edge. Three builds: the original detector, the original detector with only the blur and morphology rewrite (the separate `perf(imgproc)` commit), and the new detector.

| Input | Original, ms per image (p95) | Original + fast blur and morphology | New detector, ms per image (p95) |
|---|---|---|---|
| 320x240 | 27.7 (29.5) | not measured | 12.8 (18.7) |
| 480x360 (the suites) | 60.0 (64.1) | 11.3 (12.7) | 29.3 (40.9) |
| 1024x768 | 111.7 (120.9) | 23.2 (26.8) | 57.0 (70.8) |
| 1800x1350 (engine samples) | 112.1 (115.9) | not measured | 66.7 (78.5) |

So the new detector is about twice as fast as the one it replaces, but only because the blur and the 3x3 morphology were rewritten (separable passes with the multiply-add over contiguous rows); the new algorithm itself costs about 2.5 times what the old algorithm costs once both have the fast kernels (edge field, Hough search, line combinations, and scoring of every candidate). The line combinations use bit masks over the (at most 64) lines; the worst pictures (heavy clutter with many near-parallel stripes) are the slow ones, which is where the p95 comes from. Part of the 1024 and 1800 px time is the resize to the proxy. There is room left (the edge field is the largest single stage), and none of it was needed for the "stay fast" bar.

## What changed

`crates/imgproc/src/detect.rs` and the new `crates/imgproc/src/detect/edges.rs`. The public function `detect` and its `Detection` type are unchanged.

1. **Edge lines instead of a brightness threshold.** A colour-fused gradient (the strongest of the red, green and blue gradients at each pixel, so a white receipt on a coloured desk shows even at equal grey level) is thresholded at 1.5 times the image's own median gradient (clamped to 1.0-6.0 grey levels per pixel), thinned, and given sub-pixel positions by the centre of mass of the gradient across the ridge (a blurred edge is flat on top and its maximum wanders). Edge points vote for lines perpendicular to their gradient with the same weight, so a faint paper edge counts as much as an ink edge. Lines are refined by least squares on their inliers, and four-line combinations (two near-parallel pairs crossing at 50 degrees or more) are ranked by how much of each side carries edge points. The polarity of an edge may flip along a side: that is exactly what the low-contrast scenes do, and why a brightness threshold cannot find them.
2. **The old region candidates stay as a second source**, because a page cut by the frame has no edge along the frame to find. Every candidate from both sources is scored by the same rule.
3. **One scoring rule, built to be conservative.** For each side, the share of samples that have an aligned edge of consistent polarity within two pixels (the page's own colour is not used as a threshold any more). A side whose edge has paper behind it (an edge in the page, such as the text block, a picture frame, a fold) gets no support at all: the far side of a true page edge is desk. A result is auto-acceptable only if every side has at least 0.90 support (the old rule averaged), the edge contrast is not low, nothing cuts it off, and no rival exists.
4. **Rival check.** If another quad that is at least 10% bigger has a score of at least 0.75, or at least three solid sides, the choice is held (`Check`, reason `WeakEdge` on the side the rival grows past). This is what catches the text block chosen instead of a page whose far edge is faint, and a paper-white distractor chosen instead of a low-contrast receipt. A smaller distractor next to a clear page is not a reason to doubt the page.
5. **Corner order.** The metric's corner-order handling is right (`orientation_classes` in `crates/eval/src/metrics.rs` checks all four rotations and the mirror in closed form). The 41 wrong-start answers on the full suite came from the detector: it listed the corners starting at the edge closest to horizontal, which is a coin flip for a page rolled near 45 degrees (all 41 are in the 30-45 degree tilt bucket). Now a page whose closest edge is more than 30 degrees off horizontal, with the perpendicular edge within 60, is taken portrait-up (short edge on top); anything nearer to upright keeps the old rule, so a landscape page that is upright stays landscape. This is a portrait-page prior (receipts and most documents are portrait, and so is every page the generator draws); a landscape page rolled past 30 degrees would be listed rotated by 90 degrees, which the later orientation stage (PLAN 4.5.1) is meant to settle. Line quads also came out counter-clockwise at first (44 mirrored answers in the first experiment); every quad is now forced clockwise before the rotation is chosen.

## What did not work (and was dropped)

- **Region candidates alone, scored by edge support (with the old rectangularity term)**: the low-contrast failure rate stayed at 83%. The segmentation, not the scoring, was the problem there.
- **Keeping the old rectangularity term for region candidates**: they then outranked better line quads (mean IoU 0.955 against 0.973 for lines alone on mid-b). It was dropped; all candidates are scored on edge evidence only.
- **A higher edge threshold (3 times the median, floor 1.5)**: the first setting tried. Faint short page ends (a receipt's top and bottom) fell under it; low-contrast no-quad answers went up. 1.5 and 2.0 times the median gave similar results.
- **A high minimum vote count in the Hough search, and ranking lines by raw votes**: faint edges scatter their votes (their gradient direction is noisy), so the true top edge of a faint receipt scored below text-row lines and never made the list. The minimum is low now and lines are ranked by the inliers they collect after refinement.
- **Requiring both lines of a corner to carry edge points near it**: tidy, and faster, but it removed true corners (no quad on 108 of 1,728 images against 6 without it, at 40 lines). Dropped; the enumeration is made cheap with bit masks instead.
- **A paper-colour test with a loose tolerance, taking the page colour from the interior** (any side whose far side was within 7 grey levels of the interior median lost those samples): it penalised real low-contrast pages (23% low-contrast failures). With a 4-level tolerance, and the page colour taken from the margin strip inside the four sides rather than the interior median (large picture blocks fool the median), it helps and is kept.
- **Requiring the strip just inside every side to be paper** (page margins are blank): no net gain, more no-quad answers, and a risky assumption for full-bleed pages. Dropped.
- **Calling a quad ambiguous whenever any different quad scored well**: flagged 69% of images. Restricting rivals to bigger quads brought it back to about 12%.
- A parabolic sub-pixel refinement of the edge position had the wrong sign in the first draft; the centre-of-mass replacement is both simpler and flat-top safe.
- **Not tried**: CLAHE or local contrast normalisation, Lab or saturation channels, Canny hysteresis, morphological closing of the edge map, and a line segment detector. The per-channel gradient fusion and the saturated Hough votes cover the cases the synthetic data shows, so there was nothing to measure these against; they remain candidates for the real-photo golden set.

## Caveats and deliberate limits

- The generator draws one flat desk with a brightness gradient and faint stripes, soft shadows, rectangular distractors (a third of them paper-white), two paper kinds (a receipt at about 3.1:1 and a page at about 1.4:1, not the plan's > 4:1), JPEG (q40-95) and PNG, and EXIF orientation 1 only. It has no curl, no partial frames, no multi-item scenes, no real-world degradations, no wood grain or printed desk patterns, no text in a real font, no coloured paper and no no-document negatives. A detector that is good on this data is not thereby good on photographs.
- Low-contrast scenes were made deliberately hard (paper minus 16-30 grey levels, with blur, noise and clutter), and their failure mechanism (the desk crossing the paper's brightness) is a property of this generator. The new detector's advantage over a brightness threshold there is real in the sense that it handles a polarity flip; how often real photographs do that is unknown.
- Several constants are tuned numbers (the gradient gain 1.5 and its bounds, the 0.90 per-side support, the 0.75 and 0.7 rival thresholds, the 4 grey-level paper match, the 6 px probe). They were chosen on the seeds named above; real photographs may want different values, and the rules that hold items for review (`GOOD_SIDE_SUPPORT`, rival check) are the ones most likely to over-flag on noisy real edges. Over-flagging is the safe direction but costs review time.
- The paper-colour contradiction test assumes the margin just inside a page edge is blank paper. A page with a printed border or a full-bleed picture at its edge will lose support on that side and be held.
- The rival check can only report `WeakEdge` (the registry's `DETECTORS_DISAGREE`-style code for "another plausible outline" does not exist in `auto_crop_core::ReasonCode` yet); the chip text would read "edge unclear on the right side" for what is really "another possible page". A dedicated reason code is a follow-up in `crates/core`.
- The smoke set is 18.5 MB (JPEG and PNG, noise and clutter included), not the <= 5 MB of M1.35; CI regenerates it from the seed instead of storing it (see `docs/testing/eval-harness.md`).
- Bit-exact regeneration across operating systems is not promised (f64 trigonometry in the camera model); numbers are comparable within one platform and between `main` and a PR built on the same runner.
