# Automatic curve fitter: baseline

What it is: `auto_crop_imgproc::curves_fit::fit_curves` and `Engine::suggest_curves` ([docs/dev/curved-pages.md](../dev/curved-pages.md), "Suggestion API"). Given the picture and a straight quad it proposes four boundary curves and corners, with a confidence and a curvature per edge and a `looks_curved` hint. It never applies anything.

**Read this first.** The suites below are synthetic and share their assumptions with the fitter (edges are smooth arcs, a page is paper on a desk). Real-file evidence is n = 1 curved page and 68 flat labelled items from one person's folder; it is reported at the end and it is NOT reassuring for `looks_curved`. Nothing here is a claim about real photographs, and no constant was tuned on the owner's files.

All numbers: release build, one thread, Windows 11, `cargo xtask curves-eval` (the example `crates/eval/examples/curves_eval.rs`), the fitter started from the GROUND-TRUTH quad unless a row says detector. Boundary distance is the symmetric mean distance between the fitted and the true outline (100 samples per edge); "% short" is that distance over the shorter side of the true page. IoU is of the rasterised outlines. "Straight" is the same figure for the input quad with straight edges (the thing a person would otherwise get).

## Data

| Set | What | Used for |
|---|---|---|
| `curved-full` (seed `0x5A0C0C02`, 1,200 images, `bend` cylinder / wave / corner / bow, curvature buckets, all desks and aspects) | pages with analytic curved edges | thresholds from its **dev** split (342 pages, scenes hashed 30%); everything below from its **test** split (858 images, 852 with a bent edge) |
| `curved-smoke` (seed `0x5A0C0C01`, 150) | the same generator, another seed | never used for any choice: held-out check |
| flat `smoke`, `smoke-b`, `smoke-c` (Python generator, 200 each, hard: distractors, partial framing, low contrast) | false positives | dev (174 pages) for the thresholds, test (426) and all (600) for the numbers |
| flat `smoke` from the Rust stand-in (200) | false positives on a different renderer (soft shadows, abutting objects) | first run showed 21% false positives (the reason for the pooled paper reference); then included |

The default suites are untouched by the generator work: `smoke` and `smoke-c` regenerate to the same manifest SHA-256 and the same image bytes as before (`2c9a7c04...`, `3ceea663...`).

## Curved pages (test split, 852 pages with a bent edge, quad = ground truth)

Boundary distance, fitted vs straight quad: mean **1.54 px** vs 1.96, p50 1.33 vs 1.41, p95 3.33 vs 5.51. In % of the shorter side: mean 1.48 vs 1.88, p50 1.06 vs 1.13, p95 3.96 vs 6.54. IoU of the outline with the true one 0.957 vs 0.935. The fit is closer than the straight quad on 420 pages and farther on 333 (see below: it makes mild pages worse). Held-out `curved-smoke` (149): 1.54 px vs 2.03, IoU 0.953 vs 0.924. Time 8.8 ms mean, 12.6 ms p95 (512 px pictures, the machine was busy; 4 to 6 ms idle on 320 px pictures).

By how curved the page really is (largest edge, % of its length), boundary distance in px, fit vs straight, and IoU:

| True curvature | n | fit | straight | IoU fit / straight | `looks_curved` |
|---|---|---|---|---|---|
| slight (< 0.6%) | 78 | 0.90 | 0.28 | 0.975 / 0.988 | 0 of 78 |
| mild (0.6 to 1.5%) | 304 | 1.33 | 0.86 | 0.964 / 0.969 | 22 of 304 (7%) |
| moderate (1.5 to 3.5%) | 286 | 1.60 | 2.08 | 0.959 / 0.929 | 114 of 286 (40%) |
| strong (>= 3.5%) | 184 | 2.06 | 4.30 | 0.936 / 0.866 | 102 of 184 (55%) |

So the fitter earns its keep from about 1.5% of curvature up; below that its own noise floor (about 1 px, a systematic -0.6 px bias inward from the thin darker rim the generator draws, and edge wobble) is as large as the bend, and a ground-truth quad is already better than the fit. (Edges whose fitted bend is under 2 px, or that gain under 0.15 from curving, are returned straight for exactly this reason.) By bend: `bow` 1.67 px vs 2.81 (IoU 0.949 vs 0.901), `cylinder` 1.55 vs 1.86, `wave` 1.60 vs 1.44 (no gain), `corner` 1.06 vs 0.54 (worse: a corner curl bends a short stretch near a corner). By aspect, % of the shorter side fit vs straight: document 0.74 vs 0.87, receipt 1.27 vs 1.66, long 2.40 vs 3.12, strip 3.66 vs 4.32 (a strip is resolution limited).

The fitter fell back to the straight quad on 19 of 852 (2%) and refuses (not an error) inputs that are degenerate. Started from the DETECTOR's quad instead (`--quad detector`), the fit cannot repair a quad that is wrong by more than a few windows: 113 of 858 test images have no detection and the median boundary error is 2.1 px but the mean 21 px (p95 100 px); the straight quad is no worse (21.5 px mean). The curved suite is as hard for the detector as the default suites are.

## `looks_curved` and curvature thresholds

How they were decided. The curvature of an edge is the largest distance of the fitted curve from its chord, % of the chord length. In the first version of the fitter the raw curvature alone (>= 1.2%) fired on 38 to 49% of the flat dev pages (fit noise on short edges, a desk stripe, an object touching the page), so no threshold on curvature alone works. What separated the classes was the **support gain**: the mean saturated edge response along the fitted path minus that of the best straight line through the same windows (flat dev pages: p50 0.00, p99 0.25 per edge). The shipped gates (all must hold for an edge): confidence >= 0.7, chord >= 80 px (of the 640 px working picture), deviation >= 3 px, support gain >= 0.35, curvature >= 1.0%. Procedure: one gate at a time on the dev split, accepting the first value with no flat dev false positive and then moving one step further (at the final code state gain 0.3 has 1 of 174 flat dev pages firing and 0.25 has 2); the test split then gave the honest figure below. The constants are `LOOKS_CURVED_*` in `curves_fit.rs`.

| Gate set | Dev: flat false positives | Dev recall (true curvature 1.5-2.5 / 2.5-4 / 4+) | Test: flat false positives | Test recall (same buckets) |
|---|---|---|---|---|
| gain >= 0.25 | 2 of 174 (1.1%) | 51 / 72 / 60% | 7 of 426 (1.6%) | 56 / 58 / 62% |
| gain >= 0.30 | 1 of 174 (0.6%) | 46 / 65 / 56% | 7 of 426 (1.6%) | 50 / 53 / 59% |
| **gain >= 0.35 (shipped)** | 1 of 174 (0.6%) | 41 / 61 / 54% | 5 of 426 (1.2%) | 38 / 47 / 55% |
| gain >= 0.40 | 1 of 174 (0.6%) | 32 / 52 / 52% | 3 of 426 (0.7%) | 31 / 44 / 51% |
| gain >= 0.50 | 0 of 174 | 22 / 39 / 46% | 2 of 426 (0.5%) | 19 / 34 / 43% |

(False positives here are over the three Python flat suites; recall is over all of `curved-full`'s dev or test pages in the bucket; the other gates as shipped.) False positives of the shipped gates with the fitter started from the ground-truth quad, all pages of each flat suite: `smoke` 1 of 200, `smoke-b` 2 of 200, `smoke-c` 3 of 200, Rust stand-in 4 of 200; from the DETECTOR's quad: 2 of 178, 2 of 183, 3 of 187 and 5 of 200 pages that got a detection. About **1.25% of flat pages (10 of 800)** are called curved on synthetic data: rare, not negligible, and the margin over the dev optimum is small. `looks_curved` misses about half of the pages that really are curved by 1.5% or more (test recall 47% overall, 38 to 55% by bucket): it is conservative on purpose. A page curved by under 1.5% is called curved in 7% of cases or fewer.

Known failure modes seen in overlays: a strong edge adjacent to the page edge (a neighbouring object or its soft shadow) pulling the fit out by a few pixels over a stretch; a dark page with a pale title bar (fixed by choosing the paper colour from the right end of the band and refusing local colours far from the page's dominant one; test in `curves_fit.rs`); a blurred edge beside striped desk texture; a page whose edge is cut by the frame (the unseen stretch is straight); pages whose edge is hidden by a lighter or equal desk (confidence drops, the edge stays straight); fingers and hard shadows (bridged when brief).

## Corner refinement, time and memory

With the detector's quad on pages it found well (IoU >= 0.9 with the truth) the refined corners are NOT better than the detector's on flat pages: IoU 0.965 vs 0.975 on `smoke` (the fit moves corners by up to 4% of the shorter side to meet the fitted tangents, and the tangent of a wobbly edge end is noisy). `--predictor curves` reports exactly this on any manifest. Corner refinement matters only when edges are curved (the straight corner of a bent edge is not where the curves meet); do not use it as a corner polisher for flat pages.

Time (release, one thread, includes the reduction of a larger picture to 640 px): 4 to 6 ms on 320 px pictures, 7 to 10 ms on 512 px ones, up to 15 ms for strongly curved pages (more passes). No hint is therefore computed during analysis (the allowance was under 3 ms per image); `Engine::curve_hint` computes it on demand. Memory: the 640 px working picture (about 0.9 MB for a 640 x 480 RGB raster), one small vector per station, nothing proportional to the source size.

## Real files (n is tiny; one run, no tuning, aggregates only)

The owner's labelled pictures, with each labelled item fitted from its LABELLED quad: 69 items, of which 1 has labelled curved edges (the Safeway receipt). On that one the fit is not better than the straight quad (boundary 10.8 px, 3.3% of the shorter side, against 6.8 px for the labelled straight quad; IoU 0.922 vs 0.922): it follows the strongest edge, which on a crumpled real receipt is not always the paper edge. `looks_curved` fired on it. On the 68 flat labelled items it fired **9 times (13%)**; by slice: receipt-long 6 of 25 (24%; one of the 25 is the curved page, which should fire), general-photo 2 of 18, flatbed-multi 2 of 37, partial-frame 1 of 11, touching-items 1 of 13 (items count in each slice they belong to). Some of those may be real small waviness of the paper edge that a person labels straight; the data cannot say. Started from the detector's quad on the 16 single-item pictures: median boundary error 17 px, as for the detector alone. Consequence for the UI: `looksCurved` is not reliable enough on real receipts to switch anything on by itself or to show without a way to dismiss it; offer "Suggest curves" on demand, show the preview, let the person accept, and treat `looksCurved` as a hint at most. It is the number to improve first once the golden set has more curved receipts.

## What was not verified

Anything about phone photographs beyond the one folder above; pages with a hand or finger; pages cut by the frame at a curved edge; curved edges that are not smooth arcs (folds, tears); multi-item scans; colour spaces and EXIF (the pictures are decoded and oriented by `auto-crop-codecs` before they reach the fitter, as everywhere); behaviour at more than 640 px of working size (it is reduced first); and thresholds on any data but synthetic. The synthetic bends are not a physical model of paper (see `tools/synth/README.md`).
