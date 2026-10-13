# Enhancement: the engine surface for the UI

For the GUI session that builds the enhance panel (ROADMAP M7.33 to M7.48). Everything below is in
`crates/imgproc/src/enhance`, `crates/core` and `crates/engine`, and is additive: no existing method,
type or JSON field changed its meaning. The shell exposes it as Tauri commands and an `acimg` route
(`crates/shell`); the TypeScript contract in `ui/src/lib/types.ts`, the mock and the Svelte side are written
(see "The editor (shell and UI)" at the end). Names follow `docs/dev/multi-item-engine.md` (ids only, no pixels over IPC, no paths).

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

## The editor (shell and UI)

`crates/shell` exposes the engine calls above as the commands `set_enhance`, `clear_enhance`, `set_default_enhance`,
`suggest_enhance` (async, on the blocking pool) and, added with the UI, `set_scan_enhance` and `enhance_suggested`:

* `Engine::set_scan_enhance(id, mode, strength, live, label, gesture)` is the "All items in this scan" scope: it sets the
  default AND drops every crop's own override, as ONE undo step (Undo brings the crops' own values back).
  `set_default_enhance` keeps the overrides, as before; the UI does not call it.
* `Engine::enhance_suggested(ids) -> EnhanceBatch` is the grid's "Enhance all suggested": for every included crop of the
  images that has no look yet and has a recommendation, it applies that recommendation as the crop's own override, as ONE
  step of the session history (`session_undo`, label "Enhance suggested (N images)"). Crops that already have a look, crops
  with no recommendation and failures are counted, never touched. Only a click calls it.
* `Settings.suggestEnhance` (additive, default on, `settings.toml` key `suggest_enhance`) only decides whether a
  suggestion is SHOWN. It never applies anything. The CLI sets it off.
* "Try sample images" has two pages the suggestion fires on (`samples::SHADED_SAMPLES`: a text page in uneven light and a
  faded receipt under a warm lamp, both from the enhancer's own test pages). No other sample gets a suggestion
  (`exactly_the_shaded_samples_get_a_suggestion`).

The strength crosses the IPC as a fraction (0 to 1), as the engine takes it; the UI speaks whole percent
(`backend.ts` divides). `crates/shell/tests/enhance_json.rs` pins the wire shape on the real engine and writes
`ui/src/lib/fixtures/enhance-*.json` (`AUTOCROP_REGEN_FIXTURES=1`), which `ui/src/lib/enhance.test.ts` reads.

`ui/src`: `enhance.ts` is the pure logic (looks, strength, the preview query, counts and badges, the suggestion's life:
asked for a page, ready, previewing, dismissed), `EnhancePanel.svelte` the inspector section, `EnhanceBanner.svelte` the
suggestion, `SwapImage.svelte` the picture that never goes blank while a new address loads. Behaviour:

* **Looks.** Original, Clean, Document, Receipt, each with a plain line. A look keeps the strength it had; from Original it
  starts at 100. Strength 0 is Original (the engine keys it that way too). Choosing Original on a crop that follows an
  Original default calls `clear_enhance` (no override kept).
* **Live preview.** While the slider moves the picture comes from the `enhance-thumb` route (about every 90 ms, the older
  request is superseded by the shell); when it lands the commit is ONE `set_enhance` with a fresh gesture id (a burst of key
  presses shares one, `GestureClock`), and the picture settles to the `enhance-result` route and then to the committed crop
  image. Previews of the same kind supersede each other in the shell (one counter per kind, shared with the curve previews),
  so the panel never has two `result` requests in flight: the original for Compare is asked for only after the look has
  arrived, and a request answered with 204 is asked again twice at most.
* **Compare.** Press and hold (or hold Space or Enter) on "Show original"; a tap under 350 ms keeps it showing (the same
  rule as the editor's Compare). A split view puts the original on the left of a line on the picture itself.
* **Suggestion.** Asked for when the setting is on, the image is ready and the crop has no look, a moment (350 ms) after
  the last change of the crop or its page; the answer is cached by the engine. A gentle banner under the header says
  "This looks like a document: Clean makes it easier to read" (receipt for a long thin page) with the engine's reasons
  (`reasons[]`, copy by code), and Preview, Apply, Not now. Preview shows the result in the panel without recording
  anything (Esc or Stop preview ends it); Apply is `set_enhance` labelled "Enhance: Clean (suggested)", one undo step; Not now
  hides it for that crop until the app closes (`store.enhanceDismissed`, not saved). A change of crop, page or look drops it.
  The suggested look is tagged "Suggested" and the black-and-white look that fits "Fits this page" (an option).
* **Several items.** "This item" | "All items in this scan"; the count "Enhancement is on for 2 of 3 items"; a crop with its
  own look says so and can follow the scan look again (`clear_enhance`). The chip bar shows each item's look.
* **Held and accept.** Unchanged: enhancement alone never makes a crop held. The engine binds an acceptance to the render
  hash, which includes the look, so changing the look of a scan that was accepted (a split or a curved page) withdraws the
  acceptance, exactly like any other edit; the panel says so for held scans.
* **Output.** A black-and-white look on a JPEG source shows the PNG hint near Save (the UI has no output-format choice, so no
  one-click "Save as PNG": it points to the Convert screen) and the grey note (one grey channel, no colour profile); after a
  save the engine's `enhance.icc_dropped` and `enhance.png_recommended` notices are shown in plain language.
* **Grid.** "Enhance all suggested" (the whole batch) and "Enhance suggested" (the selection) call `enhance_suggested`, one
  session undo and a summary toast with Undo; tiles show "Enhanced" (or "Enhanced 2 of 3"). Settings has the switch
  "Suggest enhancement for documents and receipts".
* **Honest limits in the UI.** The help text says Clean was measured only on made-up test pages (OCR, never worse), that the
  black-and-white looks can lose faint marks (keep Original if unsure), and that the preview comes from a reduced copy.

The browser mock (`mock-enhance.ts`) is a canvas stand-in for the enhancer (flatten the paper level per channel, whiten,
threshold): it shows the panel working and says nothing about the real algorithm's quality.
