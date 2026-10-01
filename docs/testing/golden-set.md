# Golden-set policy (public; no images)

Decision references: B21, A-8. Design: [PLAN 7.5](../plan/07-performance-accuracy-quality.md). Roadmap: M0.50 and M0.51 (policy, strata and sizes), M0.52 and M0.53 (CI dry run), M1.39 onward (machinery). In this project "golden" means the **private** set only; checked-in regression outputs are called reference images.

## Policy

- **Private.** Real, hand-labelled photos and scans live only on the maintainer's **encrypted disk**, outside any directory an AI coding tool can read. They are never committed, never put in Git LFS and never uploaded to public CI. The private repo `auto-crop-golden` holds workflows, labels and hashes only.
- **Never trained or tuned on.** If golden results ever guide a change, those images are retired and replaced.
- **Consented sources only:** the maintainer's own documents, redacted specimens, or images with the owner's consent. Document the source and consent per image.
- **Redaction:** redact identifiers (names, account numbers, addresses) but **never the paper edges**. Strip GPS and device serial numbers on ingest, except the recorded exception for the private HEIC corpus, which stays on the maintainer's Mac.
- **Withdrawal:** anyone whose document is in the set can ask for removal; the image, label and hash are deleted and the set is re-versioned.
- **Labelling:** with a blank-quad labeller (Label Studio or `tools/labeler/`), never from the app's own suggestion, to avoid anchoring on the model. A 20% subset is double-labelled to set a noise floor.
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

See [golden-label.schema.json](golden-label.schema.json) (JSON Schema, draft 2020-12): one JSON file per image with the blank-quad labels, orientation, item count and slice tags.

## Safe-run rules (summary; details in PLAN 7.5 and 8.3.5)

Hosted runners only build; a self-hosted runner attached to the private repo runs the built evaluator in a network-less container with the set mounted read-only; triggers are nightly (dev tier), manual dispatch (locked set, once per release candidate) and release dispatch, never per push, pull request or fork; no third-party code runs beside the data.
