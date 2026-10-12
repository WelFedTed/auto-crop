# Enhancement: the engine surface for the UI

For the GUI session that builds the enhance panel (ROADMAP M7.33 to M7.48). Everything below is in
`crates/imgproc/src/enhance`, `crates/core` and `crates/engine`, and is additive: no existing method,
type or JSON field changed its meaning. The shell exposes it as Tauri commands and an `acimg` route
(`crates/shell`); the TypeScript contract in `ui/src/lib/types.ts`, the mock and the Svelte side are not
written. Names follow `docs/dev/multi-item-engine.md` (ids only, no pixels over IPC, no paths).

Design: [PLAN 5](../plan/05-enhancement.md). Measured numbers: [perf/enhance.md](../perf/enhance.md).

## The rule (B13)

Enhancement is **suggested, never forced**. `EditState.enhance` is `{"mode":"original"}` until a person
chooses another value, and nothing in the engine applies a suggestion by itself. The only automatic
application anywhere is the CLI's opt-in `process --enhance auto` (`docs/cli.md`).

## Modes

| `mode` | What it does | Output |
|---|---|---|
| `original` | Nothing. The pixels are never touched; a JPEG may take the lossless path. | as the source |
| `clean` | Flatten the lighting (paper estimate, gain), whiten the paper, set the levels. Colour kept. | RGB, source format, source ICC profile |
| `document` | Flatten, local threshold (Sauvola) with a soft ramp: black on white. | one 8-bit grey channel |
| `receipt` | Flatten, stretch the contrast, local threshold for faint thermal print (NICK). | one 8-bit grey channel |

`strength` (whole percent, 0 to 100) blends the result with the input; 0 is `original`. `Enhance` JSON:
`{"mode": "original"}` or `{"mode": "document", "strength": 100}` (an absent strength is 100).

What each mode measured on the synthetic OCR matrix is in [perf/enhance.md](../perf/enhance.md). The short
version the panel should reflect: **Clean is the recommendation** (it never made a slice worse and read
best); Document and Receipt are looks the person chooses. Document on faded print is handled (the levels
stretch is applied when the print is faded), but a faded receipt reads best as Receipt.

## State (schema v4)

* `EditState.enhance`: the default for every crop. `Item.enhance_override`: one crop's own value (the
  per-crop scope of the panel). Effective value = override, else default.
* `EDIT_STATE_VERSION` is 4. v3 to v4 only raises the version (lossless); a v3 document means `original`
  everywhere. `render_hash` and the per-crop `renderKey` include the effective enhancement, so history,
  the picture cache and "accepted" see every change. Strength 0 keys like `original`, strength above 100
  like 100.
* Setting an enhancement is not a geometry edit: the crop's band, provenance and re-detection behaviour do
  not change, and an override survives a re-detection of the page.

## Views

`ItemView.enhance` (the default) and, per crop, `CropView.enhance` (effective value) and
`CropView.enhanceOverride` (`null` when the crop follows the default: show a "customised" dot when it is not).
All `serde(default)`, camelCase.

## Edit calls (each ONE undo step, each returns the new `ItemView`)

| Tauri command | Engine call | Notes |
|---|---|---|
| `set_enhance(id, crop, mode, strength, phase, label, gesture)` | `set_enhance` | `strength` 0 to 1. `phase: "live"` records nothing (a drag in progress); the gesture id (combined with the crop id) coalesces a drag into one step. An unknown crop is `ITEM_OP`. |
| `clear_enhance(id, crop)` | `clear_enhance` | the crop follows the default again |
| `set_default_enhance(id, mode, strength, phase, label, gesture)` | `set_default_enhance` | all crops without an override ("All" scope) |
| `suggest_enhance(id, crop)` | `suggest_enhance` | see below; async (blocking pool) |

`undo`, `redo` and `reset_to_auto` cover enhancement (`reset_to_auto` returns to the analysed state, which
is `original`).

## The suggestion

`suggest_enhance(id, crop) -> EnhanceSuggestion | null`. `null` means leave the crop alone (a photograph, a
page that is already clean black on white, an estimate that could not be trusted) and also "image not
ready". It records and applies nothing; show a chip, and on the person's tap call `set_enhance`.

```jsonc
{
  "mode": "clean",              // what is suggested; never "original", never a binarising mode
  "strength": 100,
  "reason": "uneven_light",     // main reason; copy by code: uneven_light | dull_paper | colour_cast | faded_print
  "reasons": ["uneven_light", "dull_paper"],
  "blackAndWhite": "document",  // the B&W look that fits, an OPTION: "receipt" for a long thin page or faded print
  "blackAndWhiteHint": { "gray": true, "recommendPng": true },
  "hint": { "gray": false, "recommendPng": false },   // for `mode`
  "measures": { "aspect": 1.4, "paperFraction": 0.74, "inkFraction": 0.24, "shading": 0.54, "paperLuma": 213,
                "cast": 0.01, "colourFraction": 0.0, "textComponents": 1390, "concentration": 0.98,
                "glyphInkShare": 0.99, "inkContrast": 195, "faded": false }
}
```

* It fires when the crop **looks like print on paper** (paper is the dominant background, at least 40 glyph-sized
  components of one size carrying most of the ink: `measures.*`) **and** enhancement would visibly change
  it (shading of 8% or more, grey paper, a colour cast, or faded print). Thresholds and their evidence:
  `crates/imgproc/src/enhance/measures.rs`, `docs/perf/enhance.md`. They are PROVISIONAL and were fitted on
  synthetic pages and synthetic "photographs": on the dev set 28 of 28 pages fire and 0 of 36 other images
  do; on a held-out set (other seeds) 21 of 21 and 0 of 27. There is no evidence on real photographs yet.
* The page is measured as it would be saved with enhancement off, on a 2 MP rendering of the crop, and the
  answer is cached by (image, crop page), so asking again is free and an enhancement edit does not change
  it. The first call takes a few tens of milliseconds: do not call it for every grid tile at once.
* Banner copy by code (B20): `enhance.reason.<code>`.

## Pictures

`acimg` routes (token and ids as before):

* `/<token>/<id>/crop/<crop>/<thumb|result>?k=<renderKey>`: the committed crop, **enhanced with its
  effective value** (the key already includes it). An enhanced Result is rendered at the display proxy's
  2048 px long edge (a plain one at 1400) so text stays legible; the thumbnail is enhanced too.
* `/<token>/<id>/crop/<crop>/enhance-<thumb|result>?mode=<original|clean|document|receipt>&s=<0..100>`: a
  **candidate** (the slider in motion): nothing is committed or cached. Strict query, 400 for anything else.
  A newer request of the same kind supersedes an older one (204 No Content): only the last one finishes.
  Engine: `preview_enhance(id, crop, Enhance, CropImage, &dyn Cancel)`, cancellable per 64-row band.
* `/<token>/<id>/result|thumb` (whole image): the first included crop, enhanced the same way.

At zoom of 100% or more the preview (rendered from the 2048 px proxy) is not the export: a thresholded mode
does not agree pixel for pixel on thin strokes between resolutions (ROADMAP M7.25/M7.45, `render_tile`, is
not built). Say "preview" in the UI.

## Saving

`save_items` is unchanged; the effective enhancement of each crop is applied inside it, for a single crop and
for each file of a split:

* Order: geometry (quad, curved page or grid: ONE resample from the full-resolution source) -> flatten ->
  threshold -> encode. Estimates come from the crop's own pixels (and, when a quad leaves the frame, only
  from the pixels the geometry took from inside it); with several items each is rendered and enhanced on its
  own, so enhancing item 2 changes only item 2's file.
* The lossless JPEG path (`jpeg.lossless`) is for `original` (and strength 0) only. An enhanced JPEG save is
  re-encoded: one generation, like any other edit. The idempotency no-op rule (`is_noop`) does not treat an
  enhanced crop as a no-op.
* `clean` keeps the source's colour space and ICC profile byte for byte (the wide-gamut conversion to sRGB of
  ROADMAP M7.26 is not done). `document` and `receipt` write ONE 8-bit grey channel and drop the colour profile.
* `SaveOutcome.notices`: `enhance.icc_dropped` (the source had a profile and a grey file carries none) and
  `enhance.png_recommended` (a grey JPEG: PNG is lossless and keeps faint marks). A grey JPEG is never
  written below quality 90. **PNG is the recommended format for document and receipt**: use
  `hint.recommendPng` / `blackAndWhiteHint.recommendPng` to default the export format, and show the notice
  text from the codes. A first-save dialog (M7.44) is the UI's.
* Safety is unchanged: verified temp, automatic backup, atomic replace, `restore` byte for byte, the
  journal and crash recovery; the enhanced output is the "processed" file for the already-processed guard.
  Tests: `crates/engine/tests/enhance.rs`.
* Memory admission: `Engine::save_weight(id)` adds the enhancement by OUTPUT pixels (two bytes per pixel for
  the grey modes plus the 1.5 MP proxy). Peak extra heap measured at 12 MP: about 17 MB for Clean and for
  Document (no full-image float copy).

## What is not built (do not promise it in the UI)

Stage cache for sub-16 ms slider ticks on the display proxy (each preview request re-runs the analysis: tens
of ms at 2 MP; `imgproc::enhance::analyse` and `apply` are split so a cache can be added without touching
the algorithm), full-resolution tiles at 100% zoom (M7.25), the five fine-tune sliders and the Advanced
switches (M7.20 to M7.23: despeckle, colour marks, CLAHE), 1-bit output (M7.28 and following),
readability-risk holds in triage (M7.24; the enhancer's `Report` already carries `faded`, `low_contrast` and
`unreliable`), wide-gamut conversion (M7.26), SIMD (M7.13).
