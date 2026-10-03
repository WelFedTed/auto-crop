# tools/synth: synthetic test-data generator

Roadmap M1.30 to M1.35. Design: [PLAN 7.5](../../docs/plan/07-performance-accuracy-quality.md). Developer tool, never shipped, never imports or runs anything from the application (a CI guard enforces it).

It renders known-text pages and receipts, photographs them with a pinhole camera over procedural desks, degrades the picture like a phone camera would, and writes the files plus a `manifest.jsonl` (v1) that the accuracy harness (`auto-crop-eval`) reads. The ground-truth quad is **analytic** (the four page corners through the camera matrix in float64), not measured from the pixels, and the transcript of every page is known. Synthetic data only detects regressions; it never backs a real-world accuracy claim (B6).

## Use

```
cargo xtask synth-setup                     # once: target/synth-venv from requirements.lock (--require-hashes), Python >= 3.12
cargo xtask synth --suite smoke             # 200 images -> target/synth/smoke (about 4.8 MB of images)
cargo xtask synth --suite full              # 5,200 images -> target/synth/full (never archived, never committed)
cargo xtask synth --suite multi-smoke       # 160 multi-item scenes -> target/synth/multi-smoke (about 4.5 MB)
cargo xtask synth --suite multi-full        # 1,200 multi-item scenes -> target/synth/multi-full (never archived)
cargo xtask synth --suite smoke --generator rust   # the old Rust STAND-IN writer, as a fallback
cargo xtask check-splits                    # a scene or seed in two splits fails
cargo xtask synth-check                     # unit tests, inverse-warp SSIM, decoder variants, Tesseract CER
```

Directly (from `tools/synth` with the environment active): `python -m synth --seed S --count N --out DIR` (the same seed gives identical manifests and image bytes in a pinned environment, at any `--jobs`). Other options: `--suite smoke|full` (preset seed, count, size and format mix), `--max-edge`, `--truth none|text|full` (transcript JSON per image; `full` adds the clean binary render), `--backend builtin` (NumPy degradations instead of Augraphy), `--pin AXIS=VALUE` (repeatable, pins a tag for single-factor experiments). Subcommands: `check-geometry`, `check-ocr`, `variants`, `quotas`, `contact-sheet`, `licences`.

## What is rendered

| Layer | What | Module |
|---|---|---|
| Page | Letter and A4 documents (prose letter, invoice with generated line items, QR and Code 128, form, two-column report); receipts 58 to 80 mm wide with a dot-like font, generated items and prices, totals, EAN-13 and QR, any length up to 11.5:1 | `page.py`, `textgen.py`, `barcodes.py`, `fonts.py` |
| Ink and paper | white, cream and coloured paper, four inks, thermal fade (ink contrast 10 to 60%, blotchy, with print-head banding), paper grain, ink bleed, rare stains | `degrade.py` (Augraphy behind the seam) |
| Camera | pitch and yaw up to 45 degrees each, roll to 180, distance and zoom, tagged partial framing, tagged curl (cylinder, edge, corner) | `camera.py` |
| Desk | procedural wood, fabric, stone, plain, tile, dark mat and white desk; clutter that sits under the page (books, boxes, cups, keyboards, pens, extra white sheets), contact shadow | `backgrounds.py`, `scene.py` |
| Photo | five lighting classes (normal, dim, low-contrast, harsh shadow, colour cast), blur, sensor noise | `scene.py`, `degrade.py` |
| File | JPEG q40-95, PNG, TIFF, WebP; EXIF orientation 1 to 8 with pre-rotated pixels; sRGB and Display P3 (own ICC profile) | `encode.py`, `icc.py` |

Tags (each tag value is a slice; every value has at least 468 images in the full suite, floor 200 by M1.35): `aspect` (document, receipt 2:1 to 4:1, long 4:1 to 8:1, strip over 8:1), `paper`, `background`, `ink` (normal, faded), `lighting`, `clutter`, `tilt` (max of pitch and yaw: 0-10, 10-30, 30-45), `rotation` (upright to 15, tilted to 45, any), `framing` (full, partial), `curl`, `blur`, `noise`, `format`, `exif` (1 to 8), `colorspace`. Quotas are exact (balanced shuffles, `plan.py`), and `python -m synth quotas MANIFEST` checks them.

## Multi-item scenes (M10.51)

`synth/multi_item.py` places 2 to 8 items on one picture: photographic prints (3:2, 4:3, 5:4 and square, with white, Polaroid-style or no borders, procedural content with a wandering horizon), receipts (the known-text receipts of `page.py`, up to 6:1) and ID-1 cards. The bed is a flatbed lid (white, grey or black, with a vignette, platen frame shadows, 1 to 5 px edge lines, dust and the odd hair) or a desk photographed from slightly off-axis (wood, stone, fabric, dark mat; a keystone homography). Items cast soft shadows and carry a thin darker rim. Every item has an **analytic ground-truth quad** (TL, TR, BR, BL of the upright item, clockwise, y down, normalised; for desks through the plane-to-picture homography), listed in `items`; `quad` is the first item in reading order, so a single-item reader still gets a valid quad. Items are scored against their visible part when the frame clips them.

Tags: `count` (2, 3, 4, 5-6, 7-8), `separation` of the closest pair (`separated` at least 3.5% of the shorter side apart, `close` 1.5% to 3.2%, `touching` 0 to 0.8%, `overlap`; a layout whose closest pair falls between the bands is redrawn, and a test recomputes the class from the quads), `bed`, `kind` (photos, receipts, cards, mixed), `clip` (none, partial), `contrast` (`low` when the bed is within 28 grey levels of the items' outer edge colour, computed from the pixels), `rotation`, `format`. Quotas are exact (balanced shuffles); splits are by scene hash. Extra fields (ignored by the harness): `item_count`, `visible_fractions`, `min_gap_pct`, `item_kinds`, `angles_deg`, `bed_luma`.

Not rendered: hands and fingers, glare, items stacked in more than one layer, curl, text-bearing photos, real scanner banding. White-on-white is covered only as white-bordered prints and white receipts on a white lid. Smoke: 160 scenes at 640 px, JPEG, 4.5 MB; full: 1,200 at 800 px. Develop on one seed and check on others (`--seed N --name NAME --out DIR`); the same seed gives identical bytes at any `--jobs` (tested).

## Manifest

`id`, `image`, `scene_id`, `split`, `width`, `height` (of the EXIF-oriented picture), `quad` (TL, TR, BR, BL of the upright page, normalised, clockwise, y down, may leave the frame), `tags` are the harness fields. Extra fields, ignored by the harness: `scene_seed`, `image_seed`, `exif_orientation`, `rotation_deg`, `pitch_deg`, `yaw_deg`, `crop_box`, `item_count`, `visible_fraction`, `page_mm`, `page_aspect`, `jpeg_quality`, `background {kind, licence}`, `clutter_objects`, `degrade_backend`, `licence`, `source`, `truth`. Splits are by scene hash (30% `dev`, 70% `test`); a scene's page, background and seed never cross.

## Determinism

Every random choice comes from a stream named by `(seed, keys)` (BLAKE2b into NumPy PCG64), so an image depends on the suite seed and its own index only. Augraphy draws from Python's `random` and NumPy's global generator, so each call is wrapped in a reseed. ICC profiles are generated here (LittleCMS stamps the time into its own). The streams are stable within a pinned NumPy, and Pillow, OpenCV and Augraphy are pinned by hash in `requirements.lock`: that is what "same seed, identical bytes" means here. `tests/test_suite.py` generates a suite in-process and with two spawned workers and compares every byte. Across operating systems nothing is promised (FreeType, libjpeg and libm differ), but the ground truth (which uses only float64 NumPy) is compared to 1e-6 by `tests/golden_quads.json`. In practice the Windows dev machine and the Linux CI runner produced the same smoke manifest (`2c9a7c04...`) and the same detector aggregate. One non-obvious source of difference was found and fixed: libtiff leaves an alignment byte before the IFD uninitialised when a strip has an odd length (7 of 20 smoke TIFFs differed between two Linux runs), so `encode.zero_tiff_padding` zeroes it; CI regenerates the smoke suite with another worker count and diffs every file.

## Checks (`cargo xtask synth-check`)

| Check | What it proves | Status here |
|---|---|---|
| `tests/` (unittest) | plan quotas, camera golden quads (CI fails if ground truth drifts), SSIM with teeth, encode and EXIF round trips, ICC validity, independence from the app, byte-identical regeneration | runs locally and in CI |
| `check-geometry` | unwarping a render with only its quad matches the clean page, SSIM >= 0.98 (PROVISIONAL, band-limited: both sides are low-passed, sigma 1.5 px; see `verify.unwarp_ssim`) | min 0.994 over 48 renders |
| `variants` + `auto-crop-eval check-variants` | the repo's Rust decoders return the upright reference for every format x orientation x colour space, ICC byte-exact | 160 of 160 |
| `check-ocr` | Tesseract 5 character error rate <= 5% on the clean render at 10 px/mm (254 dpi, passed as `--dpi`), text layer only (rules, boxes, bar and QR codes carry no transcript; separator lines are dropped from both sides as in `tools/ocr_oracle`), 24 pages cycling the four document layouts and the receipt lengths, `--oem 1`, `--psm 3` for the two-column report and 6 otherwise (PROVISIONAL) | **not run on the Windows dev machine (no tesseract there); runs in CI** (`.github/workflows/synth.yml`). Linux run 37121957733, Tesseract 5.5.1: mean CER document 0.87%, receipt 0.95%, long 0.06%, strip 0.15%, all 24 pages 0.51% (worst page 5.3%, a receipt) |

## Supply chain

`requirements.txt` lists the direct pins; `requirements.lock` is `uv pip compile --universal --generate-hashes` of it with `overrides.txt` (Augraphy asks for `opencv-python`, which would overwrite `opencv-python-headless`). Install with `pip install --require-hashes --no-deps -r requirements.lock`. Augraphy 8.2.6 (MIT, 2023 release) is imported in `degrade.py` only. **AlbumentationsX (AGPL-3.0) is banned**: the lock must not contain it (tested), and `python -m synth licences` fails on any GPL, AGPL or non-commercial package. Fonts come from the pinned `matplotlib` wheel (DejaVu, STIX); backgrounds are procedural (DTD is excluded); text, names and prices are generated from word lists written for this project. See [docs/provenance.md](../../docs/provenance.md).

## What it showed about the detector

The classical detector scores mean IoU 0.98 and 1.85% failures on the Rust stand-in suite and 0.63 and 44% failures on this generator's 5,200-image suite (31 silent failures among 1,456 auto-accepted, 2.1%), with nothing changed in the detector. The failures sit in partial framing, long and strip receipts and look-alike desks with distractor sheets, compound, and do not depend on EXIF, format, colour space, tilt or blur. Details and the one-factor sweep: [docs/perf/detector-baseline.md](../../docs/perf/detector-baseline.md).

## Known gaps

No negatives (pages absent), no hands or fingers, no glare, no thermal-paper dot structure at pixel level, no handwriting, no real photographs or real scanner artefacts, curl is a smooth lift with no self-occlusion, one page per picture, backgrounds are flat-on to the camera. A detector that does well here is not thereby good on photographs.
