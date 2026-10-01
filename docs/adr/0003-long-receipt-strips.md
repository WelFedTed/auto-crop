# 0003 - Long narrow receipts: classical baseline failure rates (M4 input)

- **Status:** accepted (partial: the ML half of the spike was skipped, see below)
- **Date:** 2026-10-01
- **Roadmap items:** M0.54, M0.55 (feeds M4.32, M4.49)
- **Decision log links:** B7, B21, A-7
- **Time box:** 2 days (PROVISIONAL); actual about half a day. Code: `spikes/strips/` (throwaway, Python and OpenCV, outside the workspace).

## Context

Receipts are the owner's headline example, and research flagged that a stride-4 corner-heatmap net (256x256 input) may struggle with long narrow receipts because the two short-end corner peaks merge. No receipt-specific benchmark exists, so M0 builds a synthetic long-strip set and measures the cheap classical detector on it. The numbers are an input to the ML work in M4 (elongated second pass, M4.32; the early real-receipt 8:1 test, M4.49).

## Method

- **Generator** (`spikes/strips/gen.py`, seeded, shares no code with the app): 500 images, 125 per aspect bucket (4:1, 6:1, 8:1, 10:1). A rendered paper strip (Hershey-font text lines, prices, separators, barcodes, varying ink darkness), random 3D pose (any in-plane rotation, pitch and yaw up to 30 degrees), a contact shadow, one of five background kinds (wood, dark, light, noise, cloth; light backgrounds are included on purpose), blur, noise and JPEG quality 40-95. Output 1600x1200 photos with exact ground-truth quads (clockwise from the top-left of the upright strip).
- **Spot check:** 20 images (every 25th) were drawn with their quads and inspected on a contact sheet; the ground truth lines up with the paper in all 20.
- **Detector** (`spikes/strips/baseline.py`): resize to 1024 px, blur, Canny with thresholds from the median, dilate, external contours, polygon approximation (epsilon 2% of perimeter), best convex four-point contour by area, falling back to the minimum-area rectangle of the largest contour. This is the cheap classical path only.
- **Metrics:** a failure is no quad found, or IoU against ground truth below 0.9 (the silent-failure line, PLAN 7.4). Corner error is the mean corner distance over the best cyclic alignment, as a percentage of the image diagonal.

## Results

Synthetic data, one run, one seed set; not a receipt benchmark.

| Aspect bucket | n | Failures (IoU < 0.9 or no quad) | Mean IoU | Median corner error (% of diagonal) |
|---|---|---|---|---|
| 4:1 | 125 | 20 (16.0%) | 0.849 | 0.26% |
| 6:1 | 125 | 26 (20.8%) | 0.815 | 0.32% |
| 8:1 | 125 | 41 (32.8%) | 0.748 | 0.35% |
| 10:1 | 125 | 52 (41.6%) | 0.750 | 0.38% |
| **Total** | 500 | **139 (27.8%)** | | |

The classical baseline degrades steadily with aspect: the failure rate is 2.6 times higher at 10:1 than at 4:1.

## What was skipped, and why

The ML half (DocQuadNet-256 at 256x256 and an elongated 512x128 variant) was **not run**. The MakeACopy weights licence has no answer yet (M0.26 is an unsent draft that needs the owner's approval, [makeacopy-licence-query.md](../legal/makeacopy-licence-query.md)) and the weights are never committed. Whether the plain 256x256 heatmap net fails on 8:1 strips is therefore still **UNMEASURED**; the early test in M4.49 and the elongated pass in M4.32 stay in the plan.

## Decision

**GO:** record the table above as the M4 input and keep the plan's treatment of strips above 4:1 as a hard slice (elongated second pass, early real-receipt test, a per-aspect-bucket line in the evaluation report). The classical detector alone is not enough for long receipts, which supports the hybrid design (B7) rather than classical-only.

## Consequences

- M4 gates must report the 6:1, 8:1 and 10:1 buckets separately and not average them away.
- The generator and baseline are a starting point for the M1 synthetic generator (`tools/synth`, which is allowed to reuse the ideas but must stay independent of the app's warp).
- The baseline's failure causes were not analysed per background kind; M1 should break the failures down (light backgrounds, stripes, shadows) before M4 tunes anything.
- Revisit trigger: an answer on the MakeACopy weights, or any other cleared pretrained corner net, then run the ML half on this set.
