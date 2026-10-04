# Golden-set workflow (local, one repository)

Policy: [golden-set.md](golden-set.md). Label format: [golden-label.schema.json](golden-label.schema.json). Harness: [eval-harness.md](eval-harness.md). Roadmap items: M1.39 to M1.44, M1.51, M1.52, M1.83 to M1.85 (adapted, see "What changed" at the end). Code: `crates/eval/src/golden.rs`, `xtask/src/golden/`, `xtask/src/label/`.

Owner decision 2026-10-04: **one repository only**. The golden set (images, labels, splits, per-image results) lives on your machine under the gitignored `_data/` and is evaluated locally. Only aggregate numbers are ever published. No tool here makes a network call, copies an image anywhere but a backup folder you name, or writes inside tracked directories.

## The layout

```
_data/                          your images, top level only (sub-folders such as _out are ignored)
_data/golden/labels/            one <image file name>.json per image, plus _state.json and logs
_data/golden/splits.lock.json   written by `golden lock`
_data/golden/eval-log.jsonl     append-only evaluation log (+ eval-log.head)
_data/golden/results/           full per-image results: LOCAL ONLY, never publish
_data/golden/aggregates/        aggregate-only metrics: the only files fit to publish
_data/golden/noise-floor.json   optional, from `eval noise-floor`
```

Every command takes `--data DIR` (default `_data`), `--images DIR` (default: the data folder) and `--labels DIR` (default `<data>/golden/labels`). A command that would write into a git repository refuses unless the folder is ignored by it.

## Step by step

1. **Put images in `_data/`.** Only your own documents, redacted specimens or images you have consent for (golden-set.md). Do not edit an image after labelling it: the label stores its SHA-256.
2. **Label.** From the repository root:

   ```
   cargo xtask label _data --open
   ```

   It prints an address like `http://127.0.0.1:PORT/?t=<random token>` (and opens it with `--open`). The token changes every run; keep the tab on that address. HEIC and AVIF only appear in a build with the native decoders: `cargo xtask build-native`, then `cargo run -p xtask --features heif -- label _data`. Files this build cannot decode are listed at start-up and left out.

   In the page:
   - Click the four corners of each document: **top-left of the upright item first, then clockwise** (top-right, bottom-right, bottom-left). The yellow edge is the top edge. Corners may lie outside the picture for partial frames. Drag a handle (48 px touch targets), or select one (click it, or keys 1 to 4) and use the arrow keys (Shift = 10 px, Alt = fine). `Top edge` (R) rotates which edge is the top; `Flip direction` fixes a counter-clockwise order.
   - Several items in one image: `+ Item` (N), each with its own flags (partial frame, curved, touching, hand held, folded; partial frame is ticked for you when a corner leaves the picture).
   - Tick at least one **slice tag** (see the quotas below), or `No document here` for a negative.
   - Zoom with the wheel or a two-finger pinch, pan by dragging. `,` / `.` are previous / next, Enter saves and goes next, G jumps to the next unlabelled image, S skips, `?` lists every key.
   - It saves by itself as soon as the label is valid (the status line says what is missing otherwise). The page shows no model output. The separate box at the bottom, "Show detector suggestion for comparison", is off by default; if you ever turn it on for an image, that label is marked `assisted: true` (the server records it itself, also across restarts) and is **excluded** from `golden lock` and `golden eval`. Start the labeller with `--no-suggestions` to remove the box altogether (use that for the second annotator).
   - Labelling time per save and skip goes to `_data/golden/labels/_labelling-log.jsonl`; each label also carries the summed `labelling_seconds`.
3. **Check.** `cargo xtask check-labels _data/golden/labels --images _data` validates every label (finite, simple, clockwise, convex quads; slice tags present; `negative` exactly when there are no items; no duplicate ids; no two labels for one image; no label without an image; the image's SHA-256 unchanged) and reports images without a label (add `--strict` to make that, and warnings, fail).
4. **See where you are.** `cargo xtask golden status` prints labelled images per slice against the v0 / v1 / v2 quotas, the median labelling time and the hours still needed.
5. **Scenes.** Without scene metadata, one file is one scene. If several photos show the same document or the same background (so a detector could learn one from the other), give them the same **Scene id** in the labeller before locking, otherwise they may land on both sides of the dev/locked split. Identical image files with different scene ids are an error.
6. **Lock the splits.** `cargo xtask golden lock` writes `splits.lock.json`: a scene-disjoint split of PROVISIONAL 30% `dev` and 70% `locked` (assigned deterministically by a hash of the scene id, balancing the image counts), with the SHA-256 of every image and label. Running it again only adds new images; an existing assignment never moves, and a locked image or label that was edited makes it stop with an error. `golden lock --withdraw <image file name>` removes an image on request (B21 withdrawal) and records that it happened; delete the files and every backup copy yourself.
   - Use **dev** for tuning thresholds. Use **locked** to confirm a result once per release candidate. If a locked result ever guides a change, retire those images and replace them (golden-set.md).
7. **Check the lock any time.** `cargo xtask golden check` fails if a locked image or label was edited or deleted, a scene is in both splits, a label is invalid, or the evaluation log was rewritten (a dev edit is only a warning: run `golden lock` to refresh it).
8. **Evaluate.**

   ```
   cargo xtask golden eval --set dev --predictor detector
   cargo xtask golden eval --set dev --predictor multi            # several items per image
   cargo xtask golden eval --set dev --predictor jsonl:preds.jsonl # your own predictions, ids = image file names
   cargo xtask golden eval --set locked --predictor detector --reason "RC1 confirmation"
   ```

   The harness runs in-process; nothing is networked. Full per-image results (with file names and quads) go to `_data/golden/results/` and are never printed. What reaches the terminal and `_data/golden/aggregates/` goes through `PublishableMetrics` (or `PublishableMultiMetrics`): no per-image field, every slice with fewer than 30 images withheld, 30 to 79 marked advisory, a leak check on the text. `--set locked` needs `--reason`; every run, dev or locked, is appended to `eval-log.jsonl` (who, when, commit, whether the tree was dirty, set, predictor, count, hashes) **before** its numbers are shown, and the output says how many times the locked set has been evaluated. The single-quad predictors score images with exactly one item; negatives, assisted labels and (for them) multi-item images are counted and left out, as the output says.
9. **Publish, deliberately.** `cargo xtask golden report` prints the Markdown report; `cargo xtask golden report --write` writes `docs/perf/golden-baseline.md`, the one place numbers from the private set become public. It refuses if the text contains any image name, id or scene id of the set.
10. **Back up.** `cargo xtask golden backup <dest>` (see below). Do it after labelling sessions and after every `lock`.

### How many images, per slice

From golden-set.md: v0 needs **150 images in total with at least 25 per slice** (M1 first gate); v1 500 and 50 per slice (before G2); v2 800 locked and 80 per gated slice (before 1.0); plus about 300 dev-tier images. Slices overlap, so one image counts in each of its tags. The ten slices: `receipt-long` (aspect above 4:1), `thermal-fade`, `partial-frame`, `touching-items`, `phone-document`, `flatbed-single`, `flatbed-multi`, `general-photo`, `heic-device`, `negative`. `golden status` shows the gap per slice.

Important consequence of the split: the numbers you can publish come from the set you evaluate, and **a slice needs 30 images in that set to be shown at all** (80 to be a gate). With 70% of the images locked and 30% dev, a slice of 25 has about 17 locked images, so at v0 most slices are suppressed on the locked set and only the totals are reported. That is the intended behaviour, not a bug: keep labelling, and do not weaken the 30 / 80 rules.

### How to read the report

- *mean IoU* is the average overlap of the predicted and the labelled page after warping the label to a unit square; *failure rate* counts images below IoU 0.90 (the silent-failure line); *auto-accepted ... silent failure(s)* is the number that matters most: images the app would have cropped without asking whose crop was wrong, with a one-sided 95% upper bound. With 100 images a zero count still allows a bound of about 3%.
- Corner error is a percentage of the image diagonal; skew is in degrees.
- A slice marked *advisory (n < 80)* is never a gate; slices that are not shown had fewer than 30 images.
- *Annotator noise floor*: no accuracy target may be tighter than the 95th-percentile disagreement between two people. It is "not measured" until a second person labels a random 20% (at least 30 images) blind: `cargo xtask label _data --labels _data/golden/labels-b --annotator B --no-suggestions`, then `cargo xtask eval noise-floor --a _data/golden/labels --b _data/golden/labels-b --out _data/golden/noise-floor.json`, then re-render the report. `noise-floor` reads the label folders directly (blank-quad, single-item images only). Fallback in the roadmap: your own re-label after at least 7 days (weaker).
- Everything is computed on your own documents: indicative for your use, not a population estimate.

## Backup and restore (M1.85)

Images live only on your encrypted disk and are never uploaded (golden-set.md). `cargo xtask golden backup <dest>` writes a **plain copy plus a SHA-256 manifest** of every labelled image, every label and file in the labels folder, the lock, the evaluation log and the aggregates (not `results/`, which can be regenerated). It deliberately contains no cryptography. Put `<dest>` on a **second disk that is itself encrypted**: a BitLocker volume on Windows (or VeraCrypt, or FileVault on macOS), not a cloud folder. A destination inside the data folder, or inside a git repository without being ignored, is refused.

- `cargo xtask golden restore-check <dest>` re-hashes every file of the backup, checks that **every image and label hash in the backed-up `splits.lock.json` matches** (the M1.85 acceptance test), and compares with the live data (it says how many files differ; `--require-current` turns that into a failure, `--no-live` skips it).
- `cargo xtask golden restore <dest> --to <empty folder>` restores into a fresh data folder, verifying every hash and the lock, ready to use with `--data <folder>`.
- Run `restore-check` after every backup and once for real (restore into a scratch folder) before relying on it.

## What changed relative to the roadmap items

ROADMAP.md was not edited (the owner keeps it); this is the list to apply when ticking.

| Item | Original | Now |
|---|---|---|
| M1.40 | `xtask check-labels` on `labels/<image_id>.json` in the private repo | `cargo xtask check-labels <dir>`; labels in `_data/golden/labels/`. Quads are **normalised** (like the harness manifest), where the first schema draft said source pixels; the schema was extended with `id`, `image`, `width`, `height`, per-item flags (`touching`, `hand_held`, `folded`), `assisted`, `annotator`, `labelling_seconds`; `tier` became optional (the lock decides). |
| M1.41 | Label Studio or `tools/labeler/` with npm | `cargo xtask label`: one HTML page, no npm, no lockfile, std-only server. Adds the optional, off-by-default suggestion toggle with the `assisted` mark (the roadmap said never a suggestion). Acceptance (10 images at median <= 60 s) is measured with your own labelling log, not yet. |
| M1.42, M1.43 | (unchanged) | `golden status` shows quotas and times; `noise-floor` now also reads label folders. The second annotator is still needed. |
| M1.44 | `golden/splits.lock.json` in the private repo | `_data/golden/splits.lock.json` via `golden lock` / `golden check`; the evaluation log is hash-chained (tamper-evident, not tamper-proof: it is your own disk). Default scene = file; optional shared `scene_id`. |
| M1.51, M1.83, M1.84 | private repo `auto-crop-golden`, self-hosted runner, network-less container, nightly / dispatch triggers | Dropped by the one-repository decision. Replaced by `golden eval` run locally by you, the `--reason` + log rule for the locked set (the "once per release candidate" discipline, enforced by visibility rather than by a trigger), and the report you publish by hand. No token, no runner, no workflow. M1.83's planted-`build.rs` acceptance does not apply; the network claim is covered instead by `ci-guards` plus a test (below). |
| M1.52 | publish only from main nightlies and releases; scratch-fork PR re-run | `PublishableMetrics` (and the new multi-item twin) kept as the only publishable shape, leak tests kept; the workflow and fork test do not exist. |
| M1.85 | encrypted backup on a second disk | plain copy + SHA-256 manifest + `restore-check` / `restore`; encryption by BitLocker / VeraCrypt volume (no crypto written here). |

The network claim: the evaluator and `xtask` link no HTTP, TLS or socket-framework crate. `cargo xtask ci-guards` bans them in the shipped crates and in every manifest, and the test `golden::evalrun::tests::no_network_crate_is_linked_into_the_harness_or_xtask` resolves the dependency closure of `auto-crop-eval` and `xtask` with `cargo metadata` and runs the same banned-crate check. The labeller's server uses only `std::net::TcpListener` on 127.0.0.1.

## Safety notes on the labeller

127.0.0.1 only; a random 128-bit token per run (constant-time compare); `Host` must be this server (DNS rebinding), `Origin` and `Sec-Fetch-Site` are checked, a POST needs JSON; `Cache-Control: no-store`, `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`, a nonce-based Content-Security-Policy that allows no external source, no framing; requests name an image by its index in the start-up list, never by path, and any `..`, `%` or backslash in a path is refused; the browser only ever receives re-encoded, downscaled previews and tiles. Anything on your machine that can read the terminal output can read the token, so do not paste the address anywhere.
