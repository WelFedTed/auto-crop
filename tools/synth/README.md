# tools/synth: synthetic test-data generator

Roadmap M1.30 to M1.35. Design: [PLAN 7.5](../../docs/plan/07-performance-accuracy-quality.md). Developer tool, never shipped, never imports or runs anything from the application (a CI guard enforces it).

It renders known-text pages and receipts, photographs them with a pinhole camera over procedural desks, degrades the picture like a phone camera would, and writes the files plus a `manifest.jsonl` (v1) that the accuracy harness (`auto-crop-eval`) reads. The ground-truth quad is **analytic** (the four page corners through the camera matrix in float64), not measured from the pixels, and the transcript of every page is known. Synthetic data only detects regressions; it never backs a real-world accuracy claim (B6).

## Use

```
cargo xtask synth-setup                     # once: target/synth-venv from requirements.lock (--require-hashes), Python >= 3.12
cargo xtask synth --suite smoke             # 200 images -> target/synth/smoke (about 4.8 MB of images)
cargo xtask synth --suite full              # 5,200 images -> target/synth/full (never archived, never committed)
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

## Manifest

`id`, `image`, `scene_id`, `split`, `width`, `height` (of the EXIF-oriented picture), `quad` (TL, TR, BR, BL of the upright page, normalised, clockwise, y down, may leave the frame), `tags` are the harness fields. Extra fields, ignored by the harness: `scene_seed`, `image_seed`, `exif_orientation`, `rotation_deg`, `pitch_deg`, `yaw_deg`, `crop_box`, `item_count`, `visible_fraction`, `page_mm`, `page_aspect`, `jpeg_quality`, `background {kind, licence}`, `clutter_objects`, `degrade_backend`, `licence`, `source`, `truth`. Splits are by scene hash (30% `dev`, 70% `test`); a scene's page, background and seed never cross.

## Determinism

Every random choice comes from a stream named by `(seed, keys)` (BLAKE2b into NumPy PCG64), so an image depends on the suite seed and its own index only. Augraphy draws from Python's `random` and NumPy's global generator, so each call is wrapped in a reseed. ICC profiles are generated here (LittleCMS stamps the time into its own). The streams are stable within a pinned NumPy, and Pillow, OpenCV and Augraphy are pinned by hash in `requirements.lock`: that is what "same seed, identical bytes" means here. `tests/test_suite.py` generates a suite in-process and with two spawned workers and compares every byte. Across operating systems nothing is promised (FreeType, libjpeg and libm differ), but the ground truth (which uses only float64 NumPy) is compared to 1e-6 by `tests/golden_quads.json`.

## Checks (`cargo xtask synth-check`)

| Check | What it proves | Status here |
|---|---|---|
| `tests/` (unittest) | plan quotas, camera golden quads (CI fails if ground truth drifts), SSIM with teeth, encode and EXIF round trips, ICC validity, independence from the app, byte-identical regeneration | runs locally and in CI |
| `check-geometry` | unwarping a render with only its quad matches the clean page, SSIM >= 0.98 (PROVISIONAL, band-limited: both sides are low-passed, sigma 1.5 px; see `verify.unwarp_ssim`) | min 0.994 over 48 renders |
| `variants` + `auto-crop-eval check-variants` | the repo's Rust decoders return the upright reference for every format x orientation x colour space, ICC byte-exact | 160 of 160 |
| `check-ocr` | Tesseract 5 character error rate <= 5% on the clean render at 10 px/mm (254 dpi, passed as `--dpi`), text layer only (rules, boxes, bar and QR codes carry no transcript; separator lines are dropped from both sides as in `tools/ocr_oracle`), 24 pages cycling the four document layouts and the receipt lengths, `--oem 1`, `--psm 3` for the two-column report and 6 otherwise (PROVISIONAL) | **not run locally (no tesseract on the dev machine); runs in CI** (`.github/workflows/synth.yml`) |

## Supply chain

`requirements.txt` lists the direct pins; `requirements.lock` is `uv pip compile --universal --generate-hashes` of it with `overrides.txt` (Augraphy asks for `opencv-python`, which would overwrite `opencv-python-headless`). Install with `pip install --require-hashes --no-deps -r requirements.lock`. Augraphy 8.2.6 (MIT, 2023 release) is imported in `degrade.py` only. **AlbumentationsX (AGPL-3.0) is banned**: the lock must not contain it (tested), and `python -m synth licences` fails on any GPL, AGPL or non-commercial package. Fonts come from the pinned `matplotlib` wheel (DejaVu, STIX); backgrounds are procedural (DTD is excluded); text, names and prices are generated from word lists written for this project. See [docs/provenance.md](../../docs/provenance.md).

## Known gaps

No multi-item scenes (M10.51), no negatives (pages absent), no hands or fingers, no glare, no thermal-paper dot structure at pixel level, no handwriting, no real photographs or real scanner artefacts, curl is a smooth lift with no self-occlusion, one page per picture, backgrounds are flat-on to the camera. A detector that does well here is not thereby good on photographs.
