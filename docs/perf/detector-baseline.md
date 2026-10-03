# Classical page detector on the synthetic suites: baseline, the line-based rewrite, and the check on an independent generator

Two measurements of the classical page detector (`auto_crop_imgproc::detect::detect`) with the accuracy harness (`crates/eval`, ROADMAP M1.45-M1.49), both on Windows 11 x86-64, release build:

- **Before**: the first detector (global brightness threshold, best connected region, hull fitted with four lines), measured 2026-10-02 at the harness commit `f518f40`.
- **After**: the same entry point rebuilt around edge lines (this change), measured 2026-10-02. What changed is in "What changed" below.

## Read this first

- **Synthetic data only.** Every image is produced by the stand-in generator in `crates/eval/src/synth.rs` (labelled STAND-IN: it is not the M1.30 Python/Augraphy generator). It is built on the same Rust scene renderer the detector's own unit tests use, so the detector and the data share a lineage.
- **These numbers detect regressions between two builds. They say nothing about real-world accuracy** and never back the B6 silent-failure claim (PLAN 7.4: "Synthetic data only detects regressions and never satisfies B6"). The golden set (M1.39 onward) is where real accuracy gets measured; none of it exists yet. No roadmap gate (G-numbers, B6) is claimed met by anything on this page.
- Large synthetic gains can come from fitting the generator's habits. The constants of the new detector were chosen on two seeds, checked on a third, and then run once on a fourth that did not exist until they were frozen (see "Other seeds and sizes"); it was also run on other image sizes. That guards against tuning to one seed; it cannot rule out a quirk shared by every seed of the generator, which is why the caveats below matter more than the numbers.
- The confidence is still an uncalibrated heuristic; the Good/Check/Failed mapping (Good at score >= 0.90 with no forced band, Failed below 0.60 or when it reports no quad) is the interim Balanced cutoff from PLAN 7.4, not a calibrated operating point.
- Aggregates only. No per-image rows are recorded here.
- **The 2026-10-04 update below changes the detector for long thin receipts; the 2026-10-03 update after it measures the detector as it was before that change.** The 2026-10-03 update measures the same detector on the independent Python generator and the numbers do not carry over. Everything after it ("Method" onwards) was measured on the Rust stand-in generator.**

## Update 2026-10-04: long thin receipts and pages whose short ends are faint, cut or hidden

The Python suites showed the detector failing most on long and strip receipts (aspect 4:1 to 11:1), pages cut by the frame and look-alike desks; the owner's own photos showed the same (long receipts on wood or textured desks, receipts held in a hand where a table or banner inside the receipt was returned instead of the receipt). This change targets the first two without touching the constants the stand-in tuning chose. Base is commit `ec20442`; the head is this change. Same harness, same metrics, same machine (Windows 11, release build). **Aggregates only; the real-file numbers rest on approximate hand labels (see below).**

### What changed (`crates/imgproc/src/detect.rs`, `detect/strip.rs`, `detect/edges.rs`)

1. **Strip candidates** (`detect/strip.rs`, new). Diagnosis first: on the 21 failing full-frame `aspect=long` images of the Python smoke set, both long edges were found as lines for all of them (0.1 to 4 px from the truth); the two short ends were not (a short side of a thin receipt has under a few dozen edge points, below the Hough and inlier minimums, and the desk texture lifts the edge threshold). So a pair of near-parallel long lines (within 12 degrees, covered by edge points along at least 1.8 widths) now proposes a page without needing its ends as lines: the strip between them is compared with the colour just beyond each long edge all along its length (the interior of each long edge is judged by the strip centre or by the margin just inside it, whichever differs more, so a dark logo or a dense row of print does not break the run); the page is the run where that contrast holds, bridged over short dips, never extended more than 0.15 widths beyond the stretch where both edges carry edge points (past a receipt's end the contrast usually continues into a neighbouring sheet) unless the run reaches the frame. Ends are sharpened to the strongest inside/outside colour change and confirmed by that contrast instead of by an edge. A run that reaches the frame is a page cut by it: the corners are where the long lines leave the frame, the candidate is flagged `PartialFrame`, and both ends may be cut (a receipt running out of the top and the bottom), which the four-line quads could not express. Only the 16 best covered pairs get a colour profile (cost).
2. **Conservative scoring of strips.** A strip candidate is never auto-accepted (score capped at 0.8, forced Check), competes at 85% of its rank, and is not a "rival" that holds a clean quad. Without these the held answers on the stand-in changed for the worse (a text block found as a strip beat a worse wrong answer; 15 of 1,728 correct Good results became Check).
3. **Candidate selection ignores candidates that could never be answered** (score under 0.6 or ImplausibleQuad). Before, the top-ranked candidate by `score x (0.4 + area)` could be such a one and the detector then reported "no quad" although a usable candidate was in the list (the case in real photos of long receipts).
4. **A patch inside a bigger page.** When the chosen quad is already held for review and a bigger viable quad (score >= 0.6 and >= 0.75 of the chosen score) contains it, that one is chosen (still held). This is the table, banner or text block chosen instead of the receipt. It is not applied to a chosen quad that is clean (a page next to distractors that happen to bridge to a bigger quad: that cost 7 correct results on the stand-in until restricted).
5. **A container of several items is held.** Two or more separate, well supported (>= 0.9) quads of 12 to 60% of its area inside the chosen one turn a Good into Check (three photos on a mat were auto-accepted as one page). Multi-item splitting stays M10; this only keeps such an image from being accepted as a single page.
6. Small: the minimum separation of the two long sides drops from 7% to 3% of the long frame edge (a 12 to 25 px wide strip at 320 to 512 px), the odd-aspect hold moves from 12:1 to 20:1, `trace`/`trace_lines` expose the candidates for the new `manifest_report` example (`crates/eval/examples`, a local contact sheet with ground truth, candidates and lines drawn).

### Results (aggregates; base -> head)

`eval compare` base vs head is PASS on every set below. Silent failures = auto-accepted with IoU < 0.90.

| Set | Images | Mean IoU | Failure rate | Auto-accepted | Silent failures |
|---|---|---|---|---|---|
| Python full (seed default, 512 px) | 5,200 | 0.632 -> 0.696 | 44.4% -> 40.5% | 1,456 -> 1,468 | 31 -> 31 |
| Python mid-d (seed 424242, 1,000, made during development) | 1,000 | 0.626 -> 0.689 | 46.4% -> 43.4% | 288 -> 290 | 7 -> 7 |
| Python smoke (default) | 200 | 0.604 -> 0.696 | 45.5% -> 41.5% | 39 -> 39 | 0 -> 0 |
| Python smoke-b (seed 1234567) | 200 | 0.635 -> 0.725 | 46.0% -> 40.5% | 53 -> 53 | 2 -> 2 |
| Python smoke-c (seed 987654321) | 200 | 0.598 -> 0.705 | 47.5% -> 42.5% | 49 -> 50 | 1 -> 1 |
| Rust stand-in full | 5,184 | 0.979 -> 0.980 | 1.85% -> 1.72% | 4,558 -> 4,556 | 6 -> 6 |
| Rust stand-in mid-b (seed 1234567) | 1,728 | 0.978 -> 0.979 | 1.91% -> 1.79% | 1,531 -> 1,530 | 1 -> 1 |
| Rust stand-in smoke / smoke-b | 200 / 200 | 0.977 -> 0.981 / 0.979 -> 0.979 | 1.5% -> 0.5% / 2.5% -> 2.5% | 175 -> 175 / 182 -> 182 | 0 -> 0 / 0 -> 0 |

The stand-in suites (the previous tuning) are not worse on any gated slice. Python full by slice (mean IoU, failure rate; the document and normal-light slices were good before and are not worse):

| Slice | n | Base | Head | | Slice | n | Base | Head |
|---|---|---|---|---|---|---|---|---|
| aspect = document | 2,082 | 0.781, 29.1% | 0.793, 28.5% | | background = wood | 831 | 0.744, 31.2% | 0.793, 27.9% |
| aspect = receipt | 1,560 | 0.675, 39.9% | 0.724, 37.2% | | background = stone | 729 | 0.670, 39.0% | 0.754, 32.4% |
| aspect = long | 1,090 | 0.456, 63.8% | 0.582, 55.5% | | background = white-desk | 831 | 0.467, 60.8% | 0.569, 56.0% |
| aspect = strip | 468 | 0.237, 82.1% | 0.440, 69.2% | | background = tile | 624 | 0.478, 68.9% | 0.539, 66.2% |
| framing = full | 4,576 | 0.694, 36.8% | 0.757, 32.5% | | lighting = normal | 1,768 | 0.640, 44.0% | 0.702, 40.3% |
| framing = partial | 624 | 0.177, 99.8% | 0.249, 98.9% | | lighting = low-contrast | 832 | 0.648, 41.8% | 0.726, 38.2% |

Aspect long / strip on the other Python sets (failure rate): smoke 59.5% / 94.4% -> 52.4% / 77.8%; smoke-b 61.9% / 94.4% -> 52.4% / 72.2%; smoke-c 69.0% / 88.9% -> 61.9% / 72.2%; mid-d 71.2% / 84.4% -> 64.4% / 73.3%.

**Real photos (35 files in a private folder, never committed, no ground truth).** 15 single-page files were labelled by eye (approximate visual estimates, about +-1 to 2% of the image size and worse for pages held in a hand; multi-item files skipped), so these numbers are indicative only: mean IoU 0.537 -> 0.650, IoU >= 0.90 on 4 -> 5 of 15, answered with no quad at all 4 -> 1, auto-accepted 3 -> 3 (one of them at IoU 0.89 against an approximate label). Over all 35 files: the three-photo scan that was auto-accepted as one page is now held, and five files that returned no quad now return a held quad (three of them are among the labelled ones). The remaining wrong answers are held, none is newly Good.

**Speed**, single thread, whole `eval run` per image (decode included, noisy machine, three alternating runs each): 480 px stand-in 36.5 to 50.4 ms -> 47.4 to 73.8 ms, 320 px Python 17.9 to 22.7 ms -> 22.2 to 30.2 ms. Roughly +25 to 35% (the strip search: line pairs, colour profile of the 16 best). An earlier version without the pair limit cost 3x on the 320 px set.

### What did not work

- **Ranking by extent (longer diagonal) instead of area**, so a thin page is not outranked by a small fat distractor: mean IoU up on Python, but the stand-in failure rate rose (1.5% -> 2.0% and 2.5% -> 4.0%) and Python smoke failures rose. Dropped; the tuned ranking stays.
- **Looser paper-colour match** (4 -> 7 grey levels) to recognise "paper beyond" edges inside a page: low-contrast slices got worse on both generators (stand-in 4.5% -> 6.1%). Reverted.
- **Another solid item next to the chosen page makes it Check** (score >= 0.6 to 0.8, area >= 25%): no change in silent failures on Python, 3 to 10 fewer Good on the stand-in. Dropped; the container rule (item 5) with strict children costs nothing on the stand-in (looser children, 0.7 and 6%: 175 -> 165 Good).
- **Promoting the bigger containing quad at looser thresholds** (0.5 and 0.5): Python failure rate up by 2 to 6 points. Without the "chosen quad is already held" condition: 7 correct stand-in results lost.
- **Widening each line's covered stretch to the span of all its runs**: slightly worse on Python and no gain on the real photos.
- Not tried: skin-coloured blobs as occlusion, a paper-colour region segmentation for textured desks, a higher-resolution or anisotropic proxy for thin pages (the 640 px proxy was not what failed: the long edges were found), a model-based end detector.

### Remaining weak spots and caveats

- **The numbers are synthetic and share assumptions with the detector** (the strip search uses flat paper against a desk; generator strips are 12 to 25 px wide at 320 to 512 px, far thinner in pixels than a real receipt in a phone photo). Nothing here claims a G-gate or B6.
- Pages cut by the frame (`framing = partial`) remain 98.9% "failures" by definition (the truth leaves the frame; an in-frame answer scores the visible share). The detector now returns the visible part of a long receipt cut at both ends as a held `PartialFrame` quad where it used to return nothing.
- Of the 100 failing full-frame `aspect = long` images of the mid-d set, 26 had a candidate at IoU >= 0.90 that was not chosen (ranking), 35 had one at 0.75 to 0.90 (mostly one end of the strip placed 30 to 90 px too far, into a neighbouring sheet or into clutter, with sub-pixel long sides), 39 had none (faint receipts on dark mat, white desk and tile).
- White desk and tile stay above 55% failures; strips at 8:1 and beyond stay near 70%.
- Real receipts held in a hand: the receipt is now usually among the held candidates; the wrong one (a table or text block, or a quad that stops at a finger) is still sometimes chosen.
- The 200-image gate (`eval compare` at n = 200) is sensitive to one image in a gated slice; all nine comparisons above pass, but an earlier variant failed it on a single held image (-0.33 points on the png slice).

## Update 2026-10-03: the same detector on an independent generator (bad news)

The classical detector above was developed and checked on the Rust stand-in suites, whose scenes come from the same renderer the detector's unit tests use. The Python generator (`tools/synth`, M1.30 to M1.35; Augraphy, known-text pages and receipts, a pinhole camera written separately, procedural desks, partial framing, long strips) shares no code with either, and nothing in the detector was changed or tuned for it: it is the detector of the "after" column below (commit `5ede426`). Same harness, same metrics, same day. **The detector's headline numbers did not transfer.**

| | Stand-in smoke (200, 480 px) | Python smoke (200, 320 px) | Stand-in full (5,184, 480 px) | Python full (5,200, 512 px) |
|---|---|---|---|---|
| Mean IoU (95% bootstrap CI) | 0.9772 (0.9680-0.9839) | 0.6039 (0.5416-0.6630) | 0.9792 (0.9777-0.9806) | **0.6323 (0.6201-0.6450)** |
| Failure rate (IoU < 0.90, incl. no quad) | 1.50% (3) | 45.50% (91) | 1.85% (96) | **44.37% (2,307)** |
| Success at IoU >= 0.95 / >= 0.98 | 96.5% / 76.0% | 49.0% / 33.5% | 96.5% / 81.4% | 52.3% / 39.9% |
| No quad found | 0 | 53 | 3 | 1,137 (21.9%) |
| Corner error, % of diagonal, answered, p50 / p95 / p99 | 0.175 / 0.80 / 8.0 | 0.64 / 47.4 / 57.1 | 0.166 / 0.86 / 5.7 | 0.51 / 57.1 / 72.4 |
| Skew degrees, answered, p50 / p95 / p99 | 0.137 / 0.62 / 0.80 | 0.45 / 33.2 / 41.8 | 0.113 / 0.69 / 1.16 | 0.24 / 25.5 / 48.4 |
| Auto-accepted (Good) | 175 | 39 | 4,558 | 1,456 |
| Silent failures among auto-accepted | 0 (bound 1.70%) | 0 of 39 (bound 7.39%) | 6 (0.13%, bound 0.26%) | **31 (2.13%, bound 2.86%)** |
| Flag rate (Check or Failed) | 12.5% | 80.5% | 12.1% | 72.0% |
| Calibration (reported, not gated): ECE / Brier / AUROC | 0.019 / 0.012 / 0.985 | 0.187 / 0.169 / 0.921 | 0.017 / 0.015 / 0.908 | 0.196 / 0.179 / 0.913 |

The stand-in rows re-measured today are identical to the "after" columns of the table further down (the stand-in numbers are reproducible). The Python rows are a different, harder distribution, so the comparison is "how does it fare on data it was not built against", not a regression. The silent-failure rate, the number the plan cares about, is 16 times the stand-in's; a statement like "below 0.3% on this generator" holds for the stand-in and not for this one. **Nothing here satisfies or fails B6** (that is measured on the golden set), but it does show that a good score on a self-made generator says little. Also note that the detector's own behaviour is conservative: it holds 72% of the images for review (it did not guess), and only 28% reach Good.

### Where it fails (full suite, every slice has at least 468 images, so all are gated-size)

| Slice | n | Mean IoU | Failure rate | | Slice | n | Mean IoU | Failure rate |
|---|---|---|---|---|---|---|---|---|
| aspect = document | 2,082 | 0.782 | 29.1% | | framing = full | 4,576 | 0.694 | 36.8% |
| aspect = receipt (2:1 to 4:1) | 1,560 | 0.675 | 39.9% | | **framing = partial** | 624 | **0.177** | **99.8%** |
| aspect = long (4:1 to 8:1) | 1,090 | 0.456 | 63.8% | | curl = flat / curled | 4,680 / 520 | 0.632 / 0.638 | 44.2% / 45.6% |
| **aspect = strip (over 8:1)** | 468 | **0.237** | **82.1%** | | ink = normal / faded | 3,796 / 1,404 | 0.668 / 0.537 | 41.2% / 52.9% |
| background = wood | 831 | 0.744 | 31.2% | | lighting = normal | 1,768 | 0.640 | 44.0% |
| background = plain | 729 | 0.736 | 31.8% | | lighting = dim | 832 | 0.598 | 47.1% |
| background = stone / fabric | 729 / 729 | 0.670 / 0.664 | 39.0% / 40.5% | | lighting = harsh-shadow | 936 | 0.636 | 45.2% |
| background = dark-mat | 727 | 0.652 | 41.5% | | lighting = colour-cast | 832 | 0.631 | 44.0% |
| background = white-desk | 831 | 0.467 | 60.8% | | lighting = low-contrast | 832 | 0.648 | 41.8% |
| **background = tile** | 624 | **0.478** | **68.9%** | | clutter = none / light / heavy | 1,560 / 1,820 / 1,820 | 0.696 / 0.641 / 0.570 | 37.2% / 43.1% / 51.8% |
| tilt = 0-10 / 10-30 / 30-45 | 1,768 / 1,716 / 1,716 | 0.649 / 0.635 / 0.613 | 43.0% / 43.9% / 46.3% | | blur = sharp / soft / blurry | 2,860 / 1,560 / 780 | 0.635 / 0.643 / 0.600 | 44.8% / 41.7% / 48.2% |
| rotation = upright / tilted / any | 2,600 / 1,560 / 1,040 | 0.644 / 0.616 / 0.628 | 43.7% / 45.7% / 44.1% | | noise = low / medium / high | 2,600 / 1,560 / 1,040 | 0.650 / 0.624 / 0.600 | 42.5% / 45.8% / 46.8% |
| EXIF orientation 1 to 8 | 490 to 1,768 each | 0.624 to 0.644 | 43.3% to 45.3% | | format = jpeg / png / tiff / webp | 2,860 / 520 / 520 / 1,300 | 0.633 / 0.669 / 0.652 / 0.608 | 44.6% / 39.6% / 42.5% / 46.6% |
| colour space = srgb / display-p3 | 4,056 / 1,144 | 0.628 / 0.648 | 44.8% / 43.0% | | paper = white / cream / coloured | 3,118 / 1,041 / 1,041 | 0.635 / 0.642 / 0.615 | 44.3% / 43.0% / 45.8% |

Worst slice by mean IoU and by failure rate: `framing = partial`. What the table says:

- **Flat across EXIF orientation, file format, colour space, lighting class, tilt, rotation, curl, blur and noise** (within a few points of the 44% overall rate). That is the useful part of the independence check: the decode path (JPEG, PNG, TIFF, WebP, orientation 1 to 8, Display P3) does not distort the geometry, and the detector is not fragile to the photographic degradations of this generator. The one-factor sweep below agrees.
- **Partial framing (624 images, 99.8% failed).** The ground-truth quad leaves the frame; 347 of the 624 get no quad, 274 are held (Check) and 3 are auto-accepted. Part of this is structural: a quad clipped to the frame scores IoU equal to the visible fraction at best, which is 0.55 to 0.93 here (only 8.7% of these images are visible to 0.90 or more), so no in-frame answer can pass the 0.90 line. The metric counts them as failures; the behaviour (never trusting such a page) is the safe one. Whether partial pages should be extrapolated is a product decision for M2/M4, not something this table settles.
- **Long and strip receipts (aspect over 4:1): 63.8% and 82.1% failed** (58.7% and 79.8% with the page fully in frame). The plan's long-receipt slice is the detector's weakest area, and the stand-in's receipts (about 3.1:1) never exercised it.
- **Look-alike surfaces: white desk 60.8%, tile 68.9%** (and dark mat 41.5%): a tile grout grid or a near-white desk gives many competing edges or no contrast. 13 of the 31 silent failures are on a white desk and 10 on tile.
- **Confident wrong answers.** 19 of the 31 silent failures have IoU below 0.05 with confidence 0.92 to 0.997: the answer does not overlap the page at all. The harness does not dump predicted quads, so this is inferred, not observed: in the four of these images looked at (ground truth drawn on the picture) another white sheet from the generator's clutter lies on the desk, in some cases larger than the thin strip that was the target, which is the likely explanation. If so it is a multi-item situation in product terms, but still a Good verdict on the wrong object. Why the confidence stays high for it was not investigated.
- **Faded thermal ink: 52.9% against 41.2%** for normal ink on the same receipts-and-documents mix.
- None of the 31 silent failures is outside the cases above: all have at least one of long/strip, white desk or tile, heavy clutter, low contrast, partial framing or faded ink.

Failures compound. Counting the "hard" factors an image has (long or strip, a white-desk, tile or dark-mat desk, heavy clutter, low-contrast lighting, partial framing, faded ink), the failure rate climbs steeply with the count:

| Hard factors present | 0 | 1 | 2 | 3 | 4 | 5 or 6 |
|---|---|---|---|---|---|---|
| Images | 797 | 1,765 | 1,540 | 848 | 221 | 29 |
| Mean IoU | 0.960 | 0.729 | 0.556 | 0.379 | 0.254 | 0.168 |
| Failure rate | 5.8% | 34.6% | 54.0% | 71.7% | 84.2% | 89.7% |
| Silent failures / auto-accepted | 0 / 543 | 5 / 527 | 17 / 293 | 7 / 78 | 1 / 14 | 1 / 1 |

(With full framing only the same pattern holds: 5.8%, 29.8%, 46.8%, 63.7%, 73.1%, 78.6%.) With none of them the detector is at 0.960 mean IoU and 5.8% failures, still above the stand-in's 1.85%, and the 44% overall is mostly the product of many images carrying two or three of them.

**One-factor sweep from an easy base.** To see what each factor costs on its own, 12 small suites (150 to 360 images, seed 777, 512 px) pin every tag to an easy value (document, white paper, plain desk, normal light, no clutter, tilt 0-10, upright, full frame, flat, sharp, low noise, PNG, EXIF 1, sRGB) and free one axis at a time (`--pin AXIS=VALUE`). The easy base itself scores mean IoU 0.992 with 0 of 150 failures, so the generator is not broken and the detector is fine on easy pictures of this generator. Failure rate and (mean IoU) per freed value, n in brackets; every other value in these sweeps scored 0% to 2.4% failures:

| Freed axis | Value (n): failure rate (mean IoU) |
|---|---|
| aspect | document (144) 0.0% (0.993); receipt (108) 0.0% (0.989); **long (75) 8.0% (0.922)**; **strip (33) 30.3% (0.705)** |
| background | plain, fabric, stone, wood (51 to 57 each) 0.0% (0.992 to 0.997); **dark-mat (51) 17.6% (0.884)**; **white-desk (57) 10.5% (0.934)**; **tile (42) 31.0% (0.824)** |
| lighting | normal, dim, harsh-shadow, colour-cast (57 to 122) 0.0% (0.992 to 0.993); **low-contrast (58) 8.6% (0.952)** |
| clutter | none (108) 0.0% (0.993); light (126) 2.4% (0.989); **heavy (126) 7.9% (0.972)** |
| framing | full (317) 0.0% (0.992); **partial (43) 100% (0.169)** |
| ink (receipts) | normal (198) 0.5% (0.986); faded (162) 0.6% (0.983) |
| paper | white, cream (72 to 216) 0.0% (0.990 to 0.992); coloured (72) 1.4% (0.989) |
| tilt, rotation, curl, blur, noise | every value 0.0% to 0.9% (0.986 to 0.996) |

Read together with the table above: alone, only partial framing, strips, long receipts, tile, dark mat, white desk, low contrast and heavy clutter hurt, and tilt, rotation, curl, blur, noise and (alone) faded ink do not; it is the combinations (a faded long receipt on a tile desk with clutter) that take the failure rate to 44%. The stand-in could not show this because it had none of the first group and its other factors were all mild.

### What this does and does not mean

- It does **not** show the detector is bad on photographs: this generator is harder in several ways than a typical phone photo (extra white sheets as deliberate distractors, 8:1 strips on tile, 27% of images with a faded thermal receipt, 12% partial framing), and real photographs have things this one does not. It shows that **the 0.98 IoU / 1.85% failure / 0.13% silent-failure claim was an artefact of the stand-in**, and that the detector has at least three weak areas (long and strip receipts, look-alike surfaces with distractor sheets, partial pages) that the plan cares about. The golden set (M1.39 onward) is the only place this gets decided.
- The generator was not tuned against the detector: no generator parameter was changed in response to a detector result. The detector saw a 12-image integration run while the generator was being built; after the first 200-image run only the smoke suite's picture size and format mix (for the 5 MB archive budget), the `--pin` option for the sweep and a worker-death retry were added. The look of the pictures (contact sheet of 100 smoke images) was reviewed by eye, not by detector score.
- The previous section's seed-hygiene protocol (develop on some seeds, hold one back) does not apply to a different generator, but the same caution does: the Python generator is now a development target too, and the next detector change should be judged on both generators and on seeds it has not seen. One seed of each was run here.
- Reproduce: `cargo xtask synth-setup`, then `cargo xtask synth --suite full` (5,200 images, about 11 minutes with 10 workers and 413 MB under `target/synth/full`, never committed), then `cargo xtask eval run --manifest target/synth/full/manifest.jsonl --predictor detector --out full-py.json --suite full-py` (80 s). The stand-in: `cargo xtask synth --suite full --generator rust`. The sweep: `python -m synth --seed 777 --count 360 --pin aspect=document --pin background=plain ... --out DIR` with one axis left unpinned.

## Method

```
cargo xtask synth --generator rust --suite smoke   # stand-in: 200 images, 480 px long edge, seed 0x5EEDA070C20B0001 (target/synth/smoke-rust)
cargo xtask synth --generator rust --suite full    # 5,184 images, same generator and seed (its first 200 are the smoke set)
cargo xtask synth --generator rust --suite full --count 1728 --seed 1234567 --out target/synth/mid-b      # other seeds: balanced, 1,728 images
cargo xtask synth --generator rust --suite smoke --max-edge 1024 --out target/synth/smoke-1024             # other sizes
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
