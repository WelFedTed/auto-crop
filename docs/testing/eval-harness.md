# Accuracy harness (`auto-crop-eval`)

Roadmap: M1.45-M1.50 (and the type-level part of M1.52). Design: [PLAN 7.2, 7.4, 7.6, 7.8](../plan/07-performance-accuracy-quality.md). Code: `crates/eval`. Wrapped by `cargo xtask eval ...` and `cargo xtask synth ...`. (The fuller developer guide is M1.82; this page is the working reference until then.)

The harness runs a **predictor** over a **manifest** of images with ground-truth page quads and reports geometry metrics. Synthetic data only detects regressions between builds; it never backs a real-world accuracy claim (B6).

## Quick start

```
cargo xtask eval self-check                       # no data needed; every-PR validation of the harness itself
cargo xtask synth-setup                           # once: Python environment for tools/synth from the hashed lock
cargo xtask synth --suite smoke                   # 200 images from the Python generator -> target/synth/smoke (ignored by version control)
cargo xtask synth --suite smoke --generator rust  # the old Rust STAND-IN writer -> target/synth/smoke-rust
cargo xtask eval run --manifest target/synth/smoke/manifest.jsonl --predictor detector --out base.json
cargo xtask eval compare --base base.json --head head.json   # exit 1 = regression gate failed
```

**Validating a detector change on more than one seed.** Both generators take `--seed`, `--count` and `--max-edge` (the examples below use the Rust stand-in, hence `--generator rust`), so a change can be developed on one seed and checked on others it never saw (and at other image sizes) before it is trusted even as a regression signal:

```
cargo xtask synth --generator rust --suite full --count 1728 --seed 1234567 --out target/synth/mid-b   # balanced: 1,728 = 2^6 * 27, every tag value exactly even
cargo xtask synth --generator rust --suite smoke --seed 31415926 --out target/synth/smoke-d
cargo xtask synth --generator rust --suite smoke --max-edge 1024 --out target/synth/smoke-1024
```

Keep one seed back until the constants are frozen and run it once. Compare base and head with `eval compare` on each set, not only on the default smoke set; `docs/perf/detector-baseline.md` records the protocol and its limits.

**Looking at what the detector does.** `cargo run --release -p auto-crop-eval --example manifest_report -- MANIFEST OUTDIR [key=value ...] [--failures] [--silent] [--limit N] [--lines] [--err]` writes a contact sheet (ground truth white, detection coloured by verdict) and a list that says, per image, the IoU and whether any candidate the detector scored had IoU >= 0.90 (a ranking problem) or none did (a candidate-generation problem); `id=a,b` selects images. `folder_report` does the same for a folder without ground truth. Both write only into the directory you name; point them at a private folder only inside that folder's own gitignored area.

## Commands

| Command | What it does |
|---|---|
| `synth --suite smoke\|full --out DIR [--seed N] [--count N] [--max-edge N]` | Writes the Rust STAND-IN suite (below). `cargo xtask synth` defaults to the Python generator instead; this binary command is the `--generator rust` path. |
| `run --manifest F --predictor SPEC --out F [--split dev\|test\|all] [--threads N] [--commit SHA] [--tier T] [--suite NAME]` | Scores a predictor. SPEC: `full-frame`, `oracle`, `detector[:GOOD_THRESHOLD]`, `jitter:SHIFT[:SEED]`, `jsonl:PATH`. Writes results (local, with per-image rows) and `<out>.timings.json`. |
| `compare --base F --head F [--waiver] [--out F] [--min-gate-n N]` | Paired regression gate (below). Exit 0 pass or waived, 1 fail, 2 error. |
| `noise-floor --a F --b F` | Disagreement between two annotators' label files (JSON lines of `id`, `width`, `height`, `quad`): IoU mean, median and p5, corner error median and p95, skew. No target may be tighter than the p95 disagreement. |
| `publish --results F --out F` | Writes the publishable aggregate view and leak-checks it (below). |
| `validate-manifest F` | Valid quads, unique ids, relative paths, scene-disjoint splits. |
| `check-splits F...` | `cargo xtask check-splits`: fails when a `scene_id`, `scene_seed`, `group_id`, `document_id` or `background_seed` appears in two splits, in one manifest or across several (an item without `split` counts as its file name). Works on the raw lines, so generator-specific fields are seen too. |
| `check-variants --dir D` | Decodes every format x EXIF orientation x colour-space variant written by `python -m synth variants` with the repo's decoders and requires the upright reference back (exact for lossless, bounded for lossy), the orientation tag, and the ICC profile byte for byte. |
| `self-check` | Harness self-validation (below). |

## Manifest (`manifest.jsonl`)

One JSON object per line:

```json
{"v":1,"id":"smoke-00033","image":"images/smoke-00033.png","scene_id":"smoke-s0016","split":"dev",
 "width":480,"height":360,
 "quad":[[0.237,0.161],[0.574,0.136],[0.603,0.762],[0.267,0.792]],
 "tags":{"lighting":"normal","clutter":"heavy","tilt":"0-10","aspect":"document","format":"png"}}
```

`quad` is the ground-truth page outline: TL, TR, BR, BL of the upright item, clockwise in a y-down frame, normalised to the EXIF-oriented image (x by width, y by height); it may leave the frame. `image` is relative to the manifest directory (absolute paths and `..` are rejected). Each tag key becomes a slice axis. A `scene_id` must not appear in two splits. Unknown fields are ignored.

## Multi-item scenes (`run --multi`, M10.56)

A manifest row may carry `items`, a list of quads in the same convention as `quad` (which must then equal the first of them; single-item manifests are unchanged). `run --multi` scores a **multi-item predictor** (`items[:CUTOFF]` is `imgproc::items::detect_items` with the receipts profile; `oracle` returns the truth) and writes `auto-crop-eval-multi-results/1`:

```
cargo xtask synth --suite multi-smoke                      # 160 scenes (tools/synth/multi_item.py)
cargo run --release -p auto-crop-eval -- run --multi --manifest target/synth/multi-smoke/manifest.jsonl \
    --predictor items --out multi.json --suite multi-smoke
```

Per scan: the predictor's items are matched to the ground-truth items one to one, greedily by descending IoU, at IoU 0.9 (both clipped to the frame, so a clipped item is scored on its visible part). Reported over scans and over items: **exact count** (share of scans with the right number of items), **item recall** (matched over ground-truth items) and **precision** (matched over predicted items), mean IoU of matched pairs, `perfect` scans (every item matched, no extra), the auto-accept rate, `held_but_right` (perfect scans that were not auto-accepted), and the **silent wrong split**: an auto-accepted scan whose item set is wrong (another count, or an item below IoU 0.9), as k over the auto-accepted scans with a one-sided 95% Clopper-Pearson bound. **Routing** is the share of `touching` and `overlap` scans that were not auto-accepted (Wilson 95% interval). Slices follow the usual rules (n < 30 suppressed, 30 to 79 advisory, 80 and up gated). The per-scan rows carry `gt_best_iou` (the best IoU of any prediction with each ground-truth item) for diagnostics; they are local only. Results are byte-identical at 1 and 8 threads (tested with the oracle and, through files on disk, with the real detector). The metric module (`multi.rs`) imports no project crate, like `geom` and `metrics`.

Local aids (never publish their output when the input is the owner's `_data/`): `cargo run --release -p auto-crop-eval --example multi_report -- <folder or manifest.jsonl> <out dir>` draws every detected item (and the ground truth when there is a manifest) on a preview and writes `index.html`; `--example items_debug -- <image> [mask.png]` prints the detector's diagnostics and writes a picture of what the flood fill saw.

## Predictor protocol

The Rust trait is `Predictor { name, predict(&PredictInput) -> Result<Prediction, PredictError> }`; a predictor sees the image path and size, never the ground truth. Any tool can instead write a **JSON-lines predictions file** and be scored with `--predictor jsonl:preds.jsonl`:

```json
{"id":"smoke-00033","quad":[[0.24,0.16],[0.57,0.14],[0.60,0.76],[0.27,0.79]],"confidence":0.93,"state":"good"}
{"id":"smoke-00034","quad":null,"state":"failed"}
```

Coordinates as in the manifest. `confidence` is the probability the result is acceptable (0..1, optional). `state` is `good` (auto-accepted), `check` (held for review) or `failed`; without it every answer counts as auto-accepted. **Missing ids, predictor panics or errors, `quad: null` and unscorable quads (non-finite, bow-tie, beyond the page plane's horizon) all count as failures with IoU 0**; a crash never aborts the run and cannot improve any percentile. Malformed lines are skipped and counted in the result header notes.

## Metric definitions

- **IoU after canonical warp.** The unique homography taking the ground-truth quad to the unit square is applied to the predicted quad; IoU is the Jaccard index of the result and the square. It is independent of coordinate scaling and of which corner the list starts at. The homography solver and the polygon clipper (Sutherland-Hodgman against a convex clip) are the harness's own and share no code with `imgproc` (a test fails if the metric modules import a project crate).
- **Corner error**: mean of the four corner distances in pixels, as a percentage of the image diagonal, in the order the predictor gave. **Orientation class**: which cyclic relabelling (or mirroring) of the predicted corners best matches the truth; counted among predictions with IoU >= 0.90, since the IoU cannot see it.
- **Skew**: absolute difference between the rotations of the predicted and true page, where a page's rotation is the mean direction of its four edges (each turned to point rightward), measured after undoing the best corner relabelling. This is an edge-direction measure, not a text-line skew; text-skew metrics belong with M2.
- **Failure**: IoU < 0.90 (the silent-failure line, PLAN 7.4). **Success levels**: IoU >= 0.95 and >= 0.98. Corner-error and skew percentiles (p50, p95, p99; linear interpolation as in NumPy) are reported over all images, with worst-case penalties (100% of the diagonal, 90 degrees) for unanswered ones, and again over answered images only.
- **Silent failure**: an auto-accepted result with IoU < 0.90. Risk = silent failures / auto-accepted images, with the one-sided 95% Clopper-Pearson upper bound; also silent failures over all images, the flag rate and the share of flagged images that were fine.
- **Slices.** Each tag value is a slice. n < 30: reported nowhere public (`suppressed`); 30 <= n < 80: `advisory`; n >= 80: `gated`. The worst reportable slice by mean IoU and by failure rate is listed. Mean IoU has a seeded percentile-bootstrap 95% interval (2,000 resamples) for n >= 30.
- **Calibration (reported, not gated until M4).** Risk-coverage curve over distinct confidence thresholds, reliability diagram with 10 equal-mass bins and Wilson 95% intervals, ECE, Brier score and AUROC of confidence against failure. Missing predictions enter as confidence 0 and a failure.

## Determinism

A result is a pure function of the manifest, the predictor and the build. Rows are scored in parallel but collected in id order and aggregated sequentially, so the file is **byte-identical at any thread count and across runs** (tested at 1 and 8 threads). The header records the commit (`--commit`, `AUTO_CROP_COMMIT`, else `git rev-parse HEAD`), host OS and architecture, optional tier, predictor, suite, split, manifest SHA-256 and the tool version. Wall time is deliberately **not** in the result (it would break byte-identity) and goes to `<out>.timings.json`; PLAN 7.2's "every eval result carries per-stage timings" is therefore met by the sidecar until the stage spans of M1.54 exist.

## Regression gate (`compare`, M1.50)

Two results over the same manifest (same SHA-256; same image ids) are paired by id. The gate **fails** when the mean IoU falls by 0.3 points or more, or the failure rate rises by 0.5 points or more (one image is exactly 0.5 points at n = 200, so one new failure on the smoke set blocks). Slices with n >= 80 are gated the same way; smaller slices are listed and never block. The report lists every image whose pass/fail state changed and a paired bootstrap interval for the mean IoU change. `--waiver` (the `accuracy-waiver` label) turns a failure into `WAIVED`, keeping the reasons on record. Different manifests or image sets are an error, never a pass. The CI workflow is `.github/workflows/accuracy-smoke.yml`.

## Self-check (M1.49)

`auto-crop-eval self-check` (and the unit test that runs it) needs no images. On parallelogram ground truths, which are affine images of the canonical square so every expected value is a closed form: the oracle scores IoU 1.0 with 0 failures; a jittered oracle follows the analytic curve `i / (2 - i)` with `i = (1 - shift)^2` to 1e-9; `FullFrame` equals the area fraction; planted crashes are counted as failures and the run completes; results are byte-identical at 1 and 8 threads; the gate fails a jittered head and passes an identical one; ECE reads near 0 for a calibrated predictor.

## Publishing guard (M1.52, type-level part)

`PublishableMetrics` is the only shape that may leave the machine: aggregates, slices with n >= 30 only, calibration bins, and a count (not the names) of withheld slices. It has no per-image field, no id, path, quad, tag or note; `publish` serialises it and runs a leak check (forbidden keys anywhere, any image id anywhere in the text). The workflow side (publish only from main nightlies and releases, the fork test) is not built and no longer planned: with one repository the private set is evaluated locally and the owner publishes by hand (`golden report --write`). `PublishableMultiMetrics` is the same guard for `run --multi` results (scans instead of images; the routing figure is withheld below 30 hard scans).

## Public corpora

Real public datasets (SmartDoc 2015 Ch.1, CORD, MIDV-500, DIBCO, raw.pixls.us CC0) are fetched, verified and turned into manifests of this format by `cargo xtask fetch-corpus`; see [corpora.md](corpora.md). Manifest lines written by the adapters carry extra `licence`, `attribution` and `source` fields, which the harness ignores.

## The synthetic suites

Two generators write this manifest format. **The Python generator (`tools/synth`, M1.30 to M1.35) is the default of `cargo xtask synth`**; the Rust writer below is kept behind `--generator rust` as a fallback and because the per-PR accuracy gate (`accuracy-smoke.yml`) still generates its smoke set with it. Synthetic numbers detect regressions; they never back a real-world accuracy claim (B6).

### Python generator (`tools/synth`)

Known-text pages (Letter and A4 letters, invoices with line items, forms, reports) and receipts (58 to 80 mm, thermal fade, any length to 11.5:1, EAN-13 and QR), a pinhole camera (pitch and yaw to 45 degrees, any roll, tagged partial framing and curl) with an analytic ground-truth quad from the float64 matrix, procedural backgrounds with clutter, five lighting classes, blur and noise, Augraphy paper and ink degradations behind a seam, and JPEG, PNG, TIFF and WebP output with EXIF orientation 1 to 8 and sRGB or Display P3. Guide: [tools/synth/README.md](../../tools/synth/README.md).

- Tags (15 axes, each value a slice): `aspect`, `paper`, `background`, `ink`, `lighting`, `clutter`, `tilt`, `rotation`, `framing`, `curl`, `blur`, `noise`, `format`, `exif`, `colorspace`. `full` has 5,200 images in 1,734 three-image scenes and every tag value has at least 468 images (the M1.35 floor is 200); `smoke` has 200.
- Splits are by scene hash (30% `dev`, 70% `test`); a scene's page, background and seed never cross (`cargo xtask check-splits`).
- **Size.** The smoke archive is under 5 MB as a `.tgz` (about 6.2 MB on disk with the transcripts) at a 320 px long edge and 12% lossless files; the full suite is 512 px, never archived. Both are regenerated from the seed.
- Deterministic: the same seed gives identical manifest and image bytes within the pinned environment, at any worker count (tested with 1 and 2 workers). Nothing is promised across operating systems; the ground truth is checked against golden values to 1e-6.
- Checks: `cargo xtask synth-check` (unit tests including the golden quads, SSIM of the unwarped render against the clean page, the Rust decoders on every variant, Tesseract CER where installed). See the README for the numbers and what is not run locally.
- Known gaps: no multi-item scenes, no pages absent (negatives), no hands, glare or handwriting, curl without self-occlusion, backgrounds flat-on to the camera.

### Rust STAND-IN (`--generator rust`)

`synth` here is a minimal M1.35 writer, **not** the M1.30 Python/Augraphy generator. It extends the Rust scene renderer in `auto-crop-imgproc::synth` with a pinhole camera (roll and tilt up to 45 degrees; analytic ground truth), distractor clutter that never touches the page, three lighting conditions and JPEG/PNG output.

- Tags: `lighting` (normal, dim, low-contrast), `clutter` (none, light, heavy), `tilt` (0-10, 10-30, 30-45 degrees; the larger of the in-plane roll and the plane's tilt), `aspect` (document about 1.4:1, receipt about 3.1:1), `format` (jpeg q40-95, png). Axes are exactly balanced; `full` has 5,184 images (every tag value 1,728-2,592, above the 200 floor of M1.35), `smoke` 200.
- Two images share a scene (same paper texture and colours); splits are by scene hash, about 30% `dev` and 70% `test`.
- Deterministic for a given suite and seed on one platform; f64 trigonometry means cross-OS bit-exactness is not promised.
- Known gaps against M1.30-M1.34: no real fonts or text, no CC0 backgrounds, no curl, no partial frames, no multi-item scenes, no negatives, EXIF orientation 1 only, JPEG and PNG only, receipt aspect 3.1:1 instead of > 4:1.
- **Size.** The smoke suite is about 18.5 MB, above the <= 5 MB archive of M1.35 (noisy photographs do not compress; PNG variants are the bulk). CI regenerates it from the seed instead of storing an archive. Recorded as a plan deviation (the Python smoke suite is inside the budget).
- The detector under test and this generator share a lineage (its unit tests use the same renderer), so the numbers on it are the least independent ones; `docs/perf/detector-baseline.md` compares both generators.
- Output is never committed: the default output is `target/synth/<suite>-rust`, and `/synth-out/` and `/eval-out/` are ignored by version control too.

## Not done here (so nobody assumes it is)

Shapely and SciPy reference fixtures (M1.46/M1.47 acceptance mentions them) were not generated: neither is installed on the dev machine and installing them was out of scope. The clipper is instead checked against hand-computed areas (including concave subjects) and a brute-force raster, the Clopper-Pearson bound against its closed form (k = 0), its defining equation and PLAN 7.4's worked examples, quantiles against NumPy's documented definition, and the SplitMix64 generator against its published sequence. Adding fixtures generated once with SciPy and shapely would still be a worthwhile independent cross-check. Also not built: the `metrics` branch and dashboard (M1.64). The private-golden workflow was redesigned for one repository and exists as local commands (`cargo xtask golden ...`, [golden-workflow.md](golden-workflow.md)); `noise-floor` reads directories of golden label files as well as JSON lines (`--a DIR --b DIR`); multi-item results have an aggregate-only `PublishableMultiMetrics`. Still not built: the second annotator's labels (M1.43 needs a person), and scoring negatives (the no-document images are counted, not yet scored as false accepts).
