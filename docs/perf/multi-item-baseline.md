# Multi-item detection: first measured baseline (ROADMAP M10.01-M10.15, M10.51, M10.56)

`auto_crop_imgproc::items::detect_items` finds several photos, receipts or cards on one scan or table photo and returns one quad per item, holds the scan when it is unsure, and never merges or silently splits. This page records what it does on the synthetic multi-item suites and on ten labelled real pictures, how it was measured, what failed, and where it is weak. Measured 2026-10-04 on Windows 11 x86-64, release build.

## Read this first

- **Synthetic data detects regressions and guides development; it never backs a real-world claim** (PLAN 7.4, B6). The multi-item generator (`tools/synth/synth/multi_item.py`) draws procedural photographs, the known-text receipts and simple cards; it has no hands, glare, stacked layers or scanner banding, and its white-lid scenes are only white-bordered prints and white receipts.
- **The real slice is ten pictures with 53 hand-labelled items** (about +-2% label noise, 10 scenes: far below the 80 needed to gate anything, and below the 30 needed to report a slice publicly). They are the owner's private files; only aggregates appear here and nothing from `_data/` was copied, committed or uploaded.
- **Every threshold is PROVISIONAL** and was set by hand on the development seed, then checked on other seeds. No gate is claimed met: the G4 targets (exact count >= 97%, recall >= 95%, precision >= 97% on well-separated items; >= 90% of touching or overlapping scans routed to review; silent wrong splits <= 1.0%) are shown in the tables only so the distance is visible. They are **not met** overall; they are nearly met on some bed types and far from met on white and light-grey lids.
- The score is an uncalibrated heuristic (like the single-item detector's); auto-accept uses the strict preview rule of ROADMAP M10.29 (every item Good at 0.95 and no scan-level hold).

## API for the engine (stable)

```rust
// crates/imgproc/src/items/mod.rs
pub fn detect_items(src: &Raster, opts: &ItemsOptions) -> ItemsDetection;
pub fn detect_items_timed(src: &Raster, opts: &ItemsOptions, timings: &mut Vec<(&'static str, f64)>) -> ItemsDetection;

pub struct ItemsOptions { policy: SplitPolicy /*Auto|Always|Never*/, profile: SplitProfile /*Photos|Receipts*/,
    max_items: usize /*32*/, min_gap_frac: f32 /*0.015*/, min_area_frac: f32 /*0.01*/, proxy_edge: u32 /*1024*/,
    stability_check: bool, good_cutoff: f32 /*0.95*/, trust_textured_beds: bool /*false*/ }   // Default::default() is the preview setting
pub struct ItemsDetection { pub items: Vec<ItemCandidate>, pub scan_flags: ScanFlags, pub diagnostics: Diagnostics }
pub struct ItemCandidate { quad: [Pt; 4] /*TL,TR,BR,BL, normalised, reading order*/, kind: ItemKind /*Rect|Cluster*/,
    confidence: Confidence /*core type; reasons use core ReasonCode*/, fill: f32, partial_frame: bool,
    count_range: Option<(u8, u8)> /*clusters*/, signals: ItemSignals }
pub struct ScanFlags { bed_like: bool, outcome: Outcome /*NoItems|SingleItemRoute|One|Many(n)*/, reasons: Vec<Reason> /*scan-level holds*/,
    min_gap_frac: Option<f32>, stable: bool, unexplained_edge: f32 }
impl ItemsDetection { fn scan_confidence(&self) -> Confidence; fn auto_accept(&self, cutoff: f32) -> bool; fn to_items(&self, pipeline_ver: u32) -> Vec<core::Item>; }
impl ItemCandidate { fn to_item(&self, pipeline_ver: u32) -> core::Item; }
// items::handoff (M10.14): crop_for_item(src, quad, margin) and refine_with_detect(src, &cand) run detect::detect() on the item crop
```

`to_items` feeds `EditState::redetect`; `core::scan_triage` on the result agrees with `auto_accept` (tested). `Outcome::SingleItemRoute` (one item that is at least 95% of the picture, or `policy: Never`) and `NoItems` mean "use the single-item path or hold"; a `Cluster` is one flagged outline for touching or overlapping items and is never Good. New core reason codes (additive, registry names): `TOUCHING_ITEMS`, `OVERLAPPING_ITEMS`, `ITEMS_TOO_CLOSE`, `SPLIT_UNSTABLE`, `TOO_MANY_ITEMS`, `BED_UNCERTAIN`, `ANALYSIS_LIMIT`, `NO_DOCUMENT`. The function does no I/O, is deterministic (byte-identical at 1 and 8 threads, tested) and has no time cap yet (`ANALYSIS_LIMIT` fires only on the 2,000-component cap).

## What it does

1. **Frame trim** (`frame.rs`): a uniform white margin or scanner border that ends in a straight line across the whole side is cut off (a margin of empty bed in front of some items is not a frame, which was a regression found by the engine's own split test and fixed).
2. **Bed model** (`bed.rs`): Lab cells seen all around a ring inside the frame in at least three of 16 segments are the background; colour clusters under 40% as common as the commonest are dropped (prints lying on the frame). Pixels are bed (a background colour), soft (a little darker or chroma-shifted, never lighter) or foreground. Triage (M10.01): four 1.5% strips, bed-like when three sides agree within dE76 8; a scene that is not bed-like is held `BED_UNCERTAIN` (photographed desks never auto-accept).
3. **Flood fill from the frame**: spreads through bed pixels and through soft pixels that are not on a crisp edge (strong and narrow: gradient at sigma 1 at least `max(1.6, 4 x noise)` and at least 1.8 x the coarse gradient), so soft shadows join the bed and crisp outlines stop it. What is not reached is foreground; open, fill holes, 8-connected components, area >= 1%, thin shapes (hairs, lid edges, dust) rejected into `diagnostics.rejected`.
4. **Rectangles**: minimum-area rectangle of the component's hull, fill >= 0.9; every side snapped to the outermost crisp edge within 4 px of the outline (at least a quarter as strong as the strongest), robust line fit, corners by intersection; edge support (share of points on the line) and contrast across the side.
5. **Not a rectangle**: distance-transform watershed; the cut is kept only if both pieces are rectangles with supported sides, else one `Cluster` (`TOUCHING_ITEMS`, or `OVERLAPPING_ITEMS` below fill 0.8). A rectangle with two aligned crisp lines of opposite polarity across it (the white borders of two touching prints) is cut in the middle; a long straight crisp line that starts on the outline (another item on top) holds the item (`OVERLAPPING_ITEMS`).
6. **Holds**: `PARTIAL_FRAME` (clipped by the frame), `WEAK_EDGE{side}`, `LOW_CONTRAST_EDGE` (side contrast under dE 8, or crisp structure outside every item), `ITEMS_TOO_CLOSE` (clear gap under 1.5% of the shorter side), `ODD_ASPECT` (4:1 Photos, 12:1 Receipts), `TOO_MANY_ITEMS` (more than 32: the largest are kept), `SPLIT_UNSTABLE` (item count or outlines change at 0.7x and 1.4x the edge threshold; only run when the scan would otherwise be accepted), `NO_DOCUMENT`. Scan confidence is the minimum over items.

## Results

All numbers: items matched at IoU 0.9 (greedy, ground truth and prediction clipped to the frame), receipts profile, strict auto-accept. `exact` = scans with the right item count; `silent` = auto-accepted scans with a wrong item set, k over auto-accepted (one-sided 95% Clopper-Pearson bound).

### Synthetic suites (`cargo xtask synth --suite multi-smoke|multi-full`)

| Set | Role | Scans | Exact | Recall | Precision | Auto-accepted | Silent wrong (bound) | Touching/overlap routed to review |
|---|---|---|---|---|---|---|---|---|
| multi-smoke (seed 0x6D170001) | development | 160 | 57.5% | 65.6% | 75.4% | 25 | 1 / 25 (17.6%) | 61 / 61 |
| multi-smoke-b (seed 1111) | check | 160 | 60.0% | 68.6% | 77.1% | 21 | 0 / 21 (13.3%) | 61 / 61 |
| multi-smoke-c (seed 2222) | check | 160 | 64.4% | 70.2% | 78.9% | 18 | 0 / 18 (15.3%) | 61 / 61 |
| multi-val (seed 777), run once, constants frozen | held out | 400 | 62.0% | 69.5% | 78.7% | 52 | 1 / 52 (8.8%) | 152 / 152 (Wilson lower 97.5%) |
| multi-full (default seed, never tuned on) | full | 1,200 | 60.6% | 68.9% | 78.6% | 173 | 0 / 173 (1.72%) | 456 / 456 (Wilson lower 99.2%) |

By separation on multi-full (the closest pair decides the tag):

| Separation | n | Exact | Recall | Precision | Auto-accepted | Silent |
|---|---|---|---|---|---|---|
| separated (>= 3.5% of the shorter side) | 504 | 78.4% | 79.1% | 85.4% | 114 | 0 |
| close (1.5-3.2%) | 240 | 81.7% | 82.8% | 85.5% | 59 | 0 |
| touching (0-0.8%) | 240 | 40.4% | 57.0% | 70.0% | 0 | 0 |
| overlap | 216 | 18.1% | 41.8% | 58.3% | 0 | 0 |

The G4 comparison, for information only (separated slice): exact count 78% (target 97), recall 79% (95), precision 85% (97). By bed, on separated scans: flatbed-black 100% / 97.7% / 97.7%, fabric 94.6 / 94.7 / 97.5, wood 92.6 / 94.9 / 93.3, dark-mat 97.3 / 94.4 / 94.4, stone 80.8 / 89.8 / 91.5, flatbed-grey 62.1 / 83.9 / 88.1, **flatbed-white 37.6 / 27.3 / 39.4**. The suite's non-white beds therefore come close to the targets on separated items; the white and light-grey lids are the open problem. Receipts reach 96% precision but only 71% recall (long thin receipts on light beds are lost); partial (clipped) scans are never accepted by design.

Every slice above has n >= 30 on multi-full; slices at n < 80 are advisory (stone, dark-mat and fabric separated slices are 52 to 73 scans). 1 vs 8 threads: byte-identical results on multi-smoke (`cmp`).

### Before and after

The first version (commit `4dad8cd`) on the first version of the smoke suite (different generator: straight horizons inside prints, a half-pixel ground-truth offset): exact 23.1%, recall 36.3%, precision 32.0%, 3 of 160 auto-accepted (0 silent). The same suite cannot be regenerated, so the table above is the honest "after"; what moved the numbers, in order of effect: the internal-edge test learning to ignore a print's own horizon and a card's header band (recall 36% to 64%), seams only as two opposite lines, the edge snap no longer jumping to an inner edge, the frame trim and cluster-checked bed colours (real pictures), and fixing the generator's half-pixel ground-truth offset (about +2 points of recall at IoU 0.9). The pre-fix figures are not comparable with later ones because the data changed; they are listed so the direction is on record.

### Ten labelled real pictures (`_data/labels_multi.jsonl`, private)

| | Scans | Items | Exact | Recall | Precision | Auto-accepted | Routing of touching/overlap |
|---|---|---|---|---|---|---|---|
| first version | 10 | 53 | 20% | 17.0% | 30.0% | 0 | 4 / 4 |
| current | 10 | 53 | 30% | 39.6% | 63.6% | 1 (correct) | 4 / 4 |

Three scans are right (three prints on a dark table, accepted; six photos on a ribbed dark bed, held only for gaps under 1.5%; a before/after graphic with six photos clipped by the frame, held). Not solved: overlapping print piles (Polaroids, heavy overlap: `OVERLAPPING_ITEMS` clusters, as designed), a vector image of curled receipts on a transparency checkerboard (the paper is the checker's colour), a photo-album page (the bed model is uncertain), and a sheet of receipts on a light grey vector background (found, but the outlines sit on the printed text blocks). With n = 10 none of this is a rate.

### Speed

Single thread (`RAYON_NUM_THREADS=1 cargo run --release -p auto-crop-eval --example items_bench -- <manifest> 80 3`), 640x427 proxies from the smoke suite: median 63 ms, p95 137 ms (the p95 includes the two stability re-runs of scans that could be accepted); stages: Lab 11, bed model 6, planes 13, segmentation 16, components and per-item refinement 12, residual check 1 ms. The proxy is at most 1024 px, so a 12 MP picture costs roughly 2.5 to 4 times this (not measured with real 12 MP scans). The plan's budget is 40 ms p50: **not met**. The rest of the budget needs SIMD or fewer full-resolution float planes (the Lab conversion and the Gaussian blurs are the biggest stages); the machine was shared with other builds during part of this work, so treat the numbers as +-30%.

## What did not work (and was dropped)

- **A bright-lid cap** (every item on a bed with L* >= 88 held as low contrast, per the M10.60 idea): it held a clean engine test scene for no measured gain (0 silent wrong splits in 173 accepted without it), so it is gone. Cost: a print's white border on a white lid is invisible, so the quad sits on the picture edge (IoU about 0.88 to 0.95) and can be accepted. A unit test documents it.
- **Growing the outline to a faint outer edge** (white border on a white lid): +2 points of recall on the white lid, false positives from resampling and JPEG ringing on clean scenes, removed.
- **Rectangles enclosed by a closed faint outline** as a second source for white paper: dense receipt text fills the interior with crisp pixels, the outlines have gaps; removed unmerged.
- **A full-width straight line as a seam** (first version): split every borderless print with a straight horizon and every card with a header band; replaced by the two-opposite-lines rule and, for single lines, a hold.
- **A lower edge-threshold floor (3.0 to 1.6)**: neutral on the suites, kept for the white-lid rim edges.
- **The snap window looking only inward** (a step-count slip, found late): it happened to bias outlines inward, which suited the synthetic shadows; the correct two-sided search costs about one point of recall and 25 auto-accepts on multi-full and fixed real pictures with a white print border on a mid-grey bed.

## Weak spots (read before relying on it)

1. **White and light-grey lids**: white paper and white print borders are the colour of the bed. Recall on separated scans is 27% (white) and 84% (grey); almost nothing is auto-accepted there, which is the safe direction, but outlines are often missing or on the picture edge.
2. **Content the colour of the bed**: a dark picture with no border on a dark mat, a card header the colour of the bed, a pale sky next to a white border. The quad covers the rest; the scan can be accepted (the two silent wrong splits seen: a card whose header band was cut off, a photo whose dark half merged with a dark mat).
3. **Aligned, equal-size items that touch** form a rectangle: only the two-opposite-lines seam (white borders) or a single-line hold catches them.
4. **Textured or photographed desks** are held `BED_UNCERTAIN` by default; their outlines are decent (separated wood 95% recall) but never auto-accept.
5. **Overlaps** are flagged as clusters (count range only); nothing is solved. Clusters are sized by the minimum-area rectangle, which can be much bigger than the items.
6. No time cap (`ANALYSIS_LIMIT` is only the component cap); no calibration (M10.16); no per-profile ROI net pass (M10.59); the stability check is skipped on scans that are held anyway.

Reproduce: `cargo xtask synth-setup`, `cargo xtask synth --suite multi-smoke` (or `multi-full`; `--seed N --name NAME --out DIR` for other seeds), then `cargo run --release -p auto-crop-eval -- run --multi --manifest target/synth/multi-smoke/manifest.jsonl --predictor items --out multi.json --suite multi-smoke`. Local pictures of what the detector saw: `--example multi_report` and `--example items_debug` (never publish their output for real files).
