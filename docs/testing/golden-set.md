# Golden-set policy (public; no images)

Decision references: B21, A-8. Design: [PLAN 7.5](../plan/07-performance-accuracy-quality.md). Roadmap: M0.50 and M0.51 (policy, strata and sizes), M0.52 and M0.53 (CI dry run), M1.39 onward (machinery). In this project "golden" means the **private** set only; checked-in regression outputs are called reference images.

## Policy

- **Private.** Real, hand-labelled photos and scans live only on the maintainer's **encrypted disk**, outside any directory an AI coding tool can read. They are never committed, never put in Git LFS and never uploaded to CI. There is no separate private repository (owner decision 2026-10-04, one repository only): images, labels, splits and per-image results live under the gitignored `_data/` and are evaluated locally; only aggregates are published ([golden-workflow.md](golden-workflow.md)).
- **Never trained or tuned on.** If golden results ever guide a change, those images are retired and replaced.
- **Consented sources only:** the maintainer's own documents, redacted specimens, or images with the owner's consent. Document the source and consent per image.
- **Redaction:** redact identifiers (names, account numbers, addresses) but **never the paper edges**. Strip GPS and device serial numbers on ingest, except the recorded exception for the private HEIC corpus, which stays on the maintainer's Mac.
- **Withdrawal:** anyone whose document is in the set can ask for removal; the image, label and hash are deleted and the set is re-versioned.
- **Labelling:** with the blank-quad labeller `cargo xtask label` (no npm; it shows no model output by default, and a label made after switching on its optional suggestion view is marked `assisted` and excluded from the golden evaluation), never from the app's own suggestion, to avoid anchoring on the model. A 20% subset is double-labelled to set a noise floor.
- **Scene-disjoint** across training, dev and golden sets via a `scene_id` check.
- **Publishing:** only aggregates (counts, means, percentiles, per-slice numbers, calibration bins) and only for slices with n >= 30. No per-image rows, paths, thumbnails or OCR text.

## Staging

| Version | Size | When |
|---|---|---|
| v0 | >= 150 images, >= 25 per slice | M1 (first gate) |
| v1 | >= 500 images, >= 50 per slice | before G2 (M4) |
| v2 | >= 800 locked images, >= 80 per gated slice | before 1.0 (M13) |
| dev tier | about 300 extra images, unlocked | thresholds and calibration |

Slices overlap, so one image can count in several. That is about 1,100 labelled images in total; a first estimate is 40-80 hours of labelling, replaced by a measured rate in M1.

## Strata (slices)

1. Receipts, long and narrow (aspect above 4:1)
2. Thermal fade and low contrast
3. Partial frames
4. Touching or overlapping items
5. Phone photos of documents (clutter, white-on-white, low light, tilt above 30 degrees)
6. Flatbed single scans
7. Flatbed multi-photo scans
8. General photos (horizon and border crop, EXIF rotation)
9. HEIC/HEIF device files (iPhone and Android, Display P3, HDR or gain map, Live Photo, burst, grid)
10. No-document negatives

## Label format

See [golden-label.schema.json](golden-label.schema.json) (JSON Schema, draft 2020-12): one JSON file per image with the blank-quad labels (normalised to the EXIF-oriented image), orientation, per-item flags and slice tags. Step-by-step use: [golden-workflow.md](golden-workflow.md).

## Safe-run rules (one repository, local runs; the older design in PLAN 7.5 and 8.3.5 is superseded)

CI never sees the set: there is no private repository, no self-hosted runner and no workflow that touches it. The owner runs `cargo xtask golden eval` on their own machine; the evaluator makes no network call (no HTTP crate is linked, `ci-guards` plus a test check it), writes per-image results only under `_data/`, and publishes only `PublishableMetrics` aggregates (slices n >= 30; n < 80 advisory) through `golden report --write`. The locked split is evaluated only with a stated reason and every evaluation is appended to a hash-chained log, so repeated peeking is visible; it is meant for one confirmation per release candidate. Backups are a plain copy with a SHA-256 manifest on a second, encrypted disk (BitLocker or VeraCrypt).
