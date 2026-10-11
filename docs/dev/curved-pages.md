# Curved pages: the boundary-curve model (shared spec)

Owner request (2026-10-06): label and correct pages whose EDGES are curved (a crumpled receipt photographed
by hand, a book page), not only straight-edged quads. This is the weight-free first version of the plan's
M12 dewarp (items M12.27 to M12.39): the page is described by four editable boundary curves, and the page is
flattened from them. It does not fix wrinkles inside the page (that needs the learned model, M12.x).

All of the labeller (`xtask/src/label`, JavaScript), the label schema and validator, `crates/core`,
`crates/imgproc`, `crates/engine`, the shell and the UI follow this one definition.

## Curves

A curved page has a straight quad (the four corners, as today) plus four curves. Coordinates are normalised
to the EXIF-oriented image (0..1, y down), like every other quad. Corner order is TL, TR, BR, BL (clockwise).

| Curve | Runs from | to |
|---|---|---|
| `top` | TL | TR |
| `right` | TR | BR |
| `bottom` | BR | BL |
| `left` | BL | TL |

- A curve is a list of 2 to 32 points. The first and last point of each curve ARE the quad corners (they
  must be equal to the quad's corners within 1e-6); the points in between lie ON the curve, in order along it.
  Two points = a straight edge, so a straight quad is a curved page with no interior points.
- Between points the curve is a **centripetal Catmull-Rom spline** (alpha 0.5), evaluated per segment with
  the standard Barry-Goldman form. The first and last segments use a phantom point reflected through the end
  point (`P-1 = 2 P0 - P1`, `Pn+1 = 2 Pn - Pn-1`). With 2 points the curve is the straight segment.
- Knots: the interval between neighbouring points (including the phantom ones) is `max(sqrt(distance), 1e-9)`,
  distances measured in the normalised coordinates. With exactly 2 points the curve is the straight lerp.
  Neighbouring points closer than 1e-9 are refused.
- A curve parameter `t` in [0, 1] is chosen by ARC LENGTH. The polyline has 64 steps per segment
  (`s = k / 64`, the knots themselves exact, so `64 * segments + 1` points); its cumulative length is measured
  in a **scaled space** `(sx, sy)` (`(1, 1)` = normalised units; pass `(image width, image height)` to make
  the parameter uniform in source pixels whatever the aspect; the scale changes the parameterisation, never
  the shape). The ENGINE's flattening measures it on the polyline moved into the page's rectified space instead
  (see Flattening); `Curve::arc(scale)` is the plain version the shared vectors check. `at(t)`: `target = t * total` (`t` clamped to 0..1); `k` = the last polyline index with
  `cum[k] <= target`, clamped to `n - 2`; the point is the lerp of polyline points `k` and `k + 1` by
  `(target - cum[k]) / (cum[k+1] - cum[k])` (0 if that span is 0). A curve of zero length returns its first point.
  A point at `t = 0.5` is halfway along the edge, whatever the control point spacing is.
- Curves may leave the 0..1 frame slightly (a page cut by the frame; the validator allows up to one frame
  width or height beyond each side, normalised coordinates in -1..=2) but must be finite and must not
  self-intersect (validated: a coarse polyline test).

## Label format (golden labels and manifests)

An item (or the single page of an image) gains an optional field next to `quad`:

```json
"quad": [[0.12,0.03],[0.88,0.02],[0.9,0.97],[0.1,0.98]],
"curves": {
  "top":    [[0.12,0.03],[0.5,0.025],[0.88,0.02]],
  "right":  [[0.88,0.02],[0.905,0.5],[0.9,0.97]],
  "bottom": [[0.9,0.97],[0.5,0.985],[0.1,0.98]],
  "left":   [[0.1,0.98],[0.085,0.5],[0.12,0.03]]
}
```

Each of the four edge keys is optional: an ABSENT edge is straight (its two corners). The labeller writes only the edges that are bent, and writes no `curves` key at all for a page whose edges are all straight; every reader must therefore treat a missing edge as `[corner, next corner]` (`auto_crop_eval::curves::Curves::full_edge`). The spline is evaluated on the NORMALISED coordinates exactly as stored (the labeller's JavaScript and `crates/eval` both do this), so the shape does not depend on the image size.

`quad` stays required (older tools and the quad metrics keep working); `curves` is optional and, when present,
must agree with the quad at the corners. A label without `curves` is a straight page.

## Flattening

`CurveWarp::to_grid(cols, rows)` builds the source-space grid with a **Coons patch** (bilinearly blended
boundary interpolation). With `u` along the page width (0 = left, 1 = right) and `v` down the height
(0 = top, 1 = bottom), and the four curves re-parameterised so they all run in the direction of increasing
`u` or `v` (`B(u) = bottom(1-u)`, `L(v) = left(1-v)`):

```
S(u,v) = (1-v) T(u) + v B(u) + (1-u) L(v) + u R(v)
       - [ (1-u)(1-v) TL + u(1-v) TR + (1-u) v BL + u v BR ]
```

**The engine flattens in the page's rectified space (corner homography first, then the curve flow;
ROADMAP M12.29).** The patch above is exact for a page seen square on, but a bilinear patch is not a
perspective map: through a bare Coons patch a straight quad with a 5% keystone scored SSIM 0.57 against the
flat page (0.997 for the homography). So the renderer composes the two:

1. The four corners (`TL, TR, BR, BL`, source pixel-centre coordinates `p * size - 0.5`) define the
   homography `H` of the plain quad crop: a rectangle `wq` x `hq` onto the quad, with `wq` the mean of
   `|TL TR|` and `|BL BR|` and `hq` the mean of `|TL BL|` and `|TR BR|` (the size `render_quad` gives).
2. Each curve is evaluated in the IMAGE as drawn (the spline and its 64-step polyline above, so what the
   person sees is what is used), then every polyline point is moved into the rectangle with `H^-1`. The
   arc-length parameter is measured on that rectified polyline (scale `(1, 1)`, rectified pixels).
3. `S` above is evaluated on the rectified edges with the rectangle's corners `(0,0), (wq,0), (wq,hq), (0,hq)`,
   and the source position is `H(S(u, v))`.

With four straight edges `S` is the identity on the rectangle, so the result IS the homography crop for any
perspective quad (0 LSB difference in the tests). `CurveWarp::to_grid` stays the plain image-space patch (the
definition the shared test vectors check); `auto_crop_imgproc::curved::coons_grid_px` is the same grid computed
the way the renderer does, in source pixels.

Output size: width = max(arc length of top, bottom) and height = max(arc length of left, right), measured on
the rectified edges (so a straight quad gets exactly the size a plain crop gets), then capped like every other
output (pixel cap, memory budget). **Pixel convention (changed from the first draft, which said
`x / (W-1)`):** the output pixel `(x, y)` of the flat page, whose pixel CENTRES are at integer coordinates like
the homography path, samples the patch at `((x + 0.5) / W, (y + 0.5) / H)`, i.e. the outer pixel edges map
to the page edges. This is what makes a straight quad equal the homography result to within 1 LSB.
`to_grid(cols, rows)` is different on purpose: node `(i, j)` is `S(i / (cols-1), j / (rows-1))`, the patch
corner to corner. Resampling is Lanczos3 (the same kernel as the homography path), strip-wise, never a
full-image f32 copy. Pixels that map outside the source are zero, exactly as the quad path does. Quarter turns
and mirror act on the flat page (mirror first, then the clockwise turns), as for a quad.

Shared test vectors: `docs/dev/curved-pages-vectors.json` is produced by `crates/core/src/curve.rs`
(`AUTOCROP_REGEN_VECTORS=1 cargo test -p auto-crop-core vectors` rewrites it; a normal run compares). It holds
curve control points, spline values per segment (`eval`), arc-length points and lengths (`at`, `length`, for
unit and pixel scales) and small Coons grids (`nodes`, `arcLengths`, `flatSize`). Any other implementation
(the labeller's JavaScript) must match them to 1e-9.

## Persistence

`EditState` stores the CURVE CONTROL POINTS (editable), not only the derived grid: a new geometry variant
(`Geometry::Curved(CurveWarp)`), with schema migration (EditState v3), serde names in camelCase, caps (32
points per curve), and `Geometry::Grid` (the dense grid of the learned model, M12.27) remains separate. A
curved item is HELD for review in every triage mode unless the user accepted it (it is never auto-written),
and it never takes the lossless JPEG path.

## Engine API

Everything below exists in `crates/core`, `crates/imgproc` and `crates/engine` (Rust; the shell and the UI
only have to expose it). Names are the Rust ones; JSON is camelCase.

### Types and persistence

* `core::Curve` is `Vec<Pt>` of 2 to 32 finite points (serde: a bare array of `{x, y}`; loading a shorter or
  longer array is an error, so a document can never hold an illegal curve). `core::CurveWarp` is
  `{ top, right, bottom, left, quarterTurns, mirror }` (`quarterTurns` 0..=3 and `mirror` default to 0 and
  false; unknown fields are refused). The curves' end points are the corners: `top` TL to TR, `right` TR to BR,
  `bottom` BR to BL, `left` BL to TL. There is no fine angle on a curved page (the curves already say how it
  lies); `curve_from_quad` bakes a quad's fine angle into the corners.
* `Geometry::Curved(CurveWarp)` serialises as `{"type": "curved", "top": [...], "right": [...], "bottom": [...],
  "left": [...], "quarterTurns": 0, "mirror": false}`. Helpers: `Geometry::curves()`, `is_curved()`,
  `outline_quad()` (the straight quad through the corners, with the turns and mirror: what a view shows as the
  page outline) and `EditState::has_curved()`.
* `EditState` schema is now **v3**. v2 to v3 only raises the version (lossless; the bump makes an older build
  refuse a document with curves as `SCHEMA_TOO_NEW` instead of half-reading it). `render_hash` and
  `item_render_hash` include the control points, the turns and the mirror, so history, the per-crop cache key
  (`CropView.renderKey`) and "accepted" see every change.
* `CurveWarp::validate()` (called by every edit, and by the renderer): finite, within -1..=2 of the frame,
  2..=32 points, neighbouring
  points more than 1e-9 apart, the four end-point pairs meet within 1e-6, the outline has an area, and a
  coarse polyline test (8 samples per segment) finds no crossing. `validate_with_quad(&QuadWarp)` also
  compares with a quad. Any failure is `ErrKind::Degenerate` (`DEGENERATE`).

### Edit API (`Engine`, all addressed by image id and crop id)

| Call | What it does |
|---|---|
| `curve_from_quad(id, crop)` | A quad crop becomes a curved page with four straight edges (corners unchanged, fine angle baked in); idempotent on a curved crop. One undo step "Curve edges". |
| `set_curves(id, crop, &CurveWarp, live, label, gesture)` | Replaces the curves of a quad or curved crop. `live = true` validates and returns the view the UI would show for a drag in progress without recording anything; otherwise one undo step per `gesture` id (the usual coalescing). An auto crop becomes `autoThenEdited`, so a re-detection keeps it. The corners move with the end points. |
| `clear_curves(id, crop)` | A curved crop becomes the straight quad through its corners (turns and mirror kept). One undo step "Straighten edges". |
| `preview_curves(id, crop, &CurveWarp, CropImage, &dyn Cancel)` | JPEG bytes of a candidate curve set rendered from the display proxy at preview size, without committing or caching (a drag preview). `Cancelled` if the token fires. |
| `crop_image_bytes(id, crop, kind)` / `crop_image_bytes_cancellable(.., &dyn Cancel)` | The committed crop image (the `acimg` per-crop path) now renders a curved crop through the curved resampler at preview resolution; the cache key is the crop's render hash. |
| `image_bytes(id, kind)` | The whole-image result and thumbnail do the same for the first included crop. |
| `save_weight(id) -> Option<u64>` and `curved_job_weight(src_px, out_px)` | Memory admission: `pixels * 9 + 64 MiB` of the source plus 6 bytes per OUTPUT pixel of the largest curved crop (its size comes from its arc lengths). A caller that uses a `MemoryBudget` admits a save with it. PROVISIONAL. |

Item operations on a curved crop (`core::items`): turn and flip are handled (only `quarterTurns` and `mirror`
change, the curves stay); the corner edit (`set_crop_edit`, `set_edit`), the fine angle, merge and cut are
REFUSED with `ErrKind::ItemOp` (`ITEM_OP`) and change nothing, so a curved crop can never silently lose its
curves to a quad operation. `reset_to_auto` is an explicit undoable step and may drop curves (undo brings
them back). A curved crop is never created by analysis: the detector stays quad-based.

### Views for the UI

* `CropView.curves: Option<CurveWarp>` carries the control points (null for a quad). `CropView.edit` is then
  the straight outline through the corners (so the quad handles keep working), `CropView.mirror` follows the
  curves' mirror. (The task text called this `crops[].geometry`; the field is `curves`.)
* `CropView.band` of a curved crop is `check` until the scan is accepted, then `good`. `ItemView.split.triage`
  is `heldForReview` and `split.accepted` is false until `accept_scan`.
* `ItemView.edit` (the first included crop) is the outline of a curved crop too.

### Hold, save and restore

* A curved crop is held in every triage mode (`scan_triage` never approves it). There is no separate flag: it
  reuses the multi-item acceptance (`accept_scan` / `unaccept_scan`, bound to the render hash, so any later edit
  needs a new acceptance).
* Save as Replace without that acceptance fails with `HELD_FOR_REVIEW`, notice `curved.held` (single crop) or
  `split.held` (several crops), writes nothing and makes no backup. Save as Copy needs no acceptance (nothing
  is overwritten).
* A curved crop never takes the lossless JPEG path; it is decoded once (EXIF orientation applied once),
  resampled once from the full-resolution source by the same resampler as the preview, encoded and verified
  through the usual commit, backup and journal; "Restore original" gives the original back byte for byte.
* Cancellation: the resampler polls the token once per 64-row band (previews). `save_items` itself has no
  token today for any geometry.

### Measured (see `crates/imgproc/tests/curved_oracles.rs`, `curved_memory.rs`)

Straight quads (axis-aligned, rotated, perspective, all four turns) vs the homography crop: 0 LSB. A page
bowed like a cylinder, a bowed page photographed through a keystone camera, and a waving tilted page, with
analytic edges and a printed grid, flattened from 9, 11 and 13 points: SSIM 0.994, 0.994 and 0.974 against the
flat page, rule straightness residual 0.35, 0.25 and 0.49 px, rule position error 0.40, 0.26 and 0.69 px. With
3 points on the wave: SSIM 0.46, straightness 9 px (the model is only as good as the points). 12 MP source to
a 3582 x 2623 page: extra heap = output + 0.3 MB; 170 ms on 8 threads and 1008 ms on 1, against 160 ms and
802 ms for the plain homography warp of the same size on the same (loaded) machine (release); bytes
identical at 1 and 8 threads. The dense-grid warp (`warp_grid_image`, 41 x 33 nodes) is within 1 LSB of the
exact render.

### Limits of a boundary-only model (measured, not hidden)

* It fixes only distortion that the four edges show. A page bent about an axis whose edges stay straight (a
  book open at the spine, photographed square on) is NOT corrected: SSIM 0.57 and 25 px of rule position error
  in the test. Wrinkles inside the page need the learned model (M12.x).
* The arc-length parameter is shared by top and bottom (and by left and right): if the real material is
  stretched differently along the two edges the interior is off by the difference.
* Perspective is modelled by the corner homography, not by the curves: a page that is both bent and seen at
  an angle is right to the extent that "the quad's perspective, then in-plane edge curvature" describes it
  (the tests with a keystone camera: SSIM 0.994). Foreshortening that varies along a bent surface is not
  recovered.
* The rectified frame is built from the corner quad, so a very slanted chord (corners that are not
  a rectangle in the page) shears the arc-length parameter a little: the wave-and-tilt test, whose chord is
  tilted, gives SSIM 0.974 where the symmetric bowl gives 0.994.
* Downscaling by more than 2x aliases like the plain quad path (no pre-shrink guard); previews are rendered
  from the display proxy, saves at the source's own scale.
* The CLI does not read curves (`--curves FILE` is out of scope). `render --edit` of a state with a curved
  crop ignores that crop.

## Suggestion API (automatic curves)

Today a person bends each edge by hand. `Engine::suggest_curves` proposes the four curves from the picture
(`crates/imgproc/src/curves_fit.rs`, classical, no weights, offline). **It only proposes**: nothing is recorded,
cached or applied, so a straight page is never changed by it; the person (or the UI on their click) applies the
result with `set_curves`, which is the one way a crop becomes curved. Numbers and caveats:
[docs/perf/curve-fit-baseline.md](../perf/curve-fit-baseline.md).

### Calls

| Call | What it does |
|---|---|
| `suggest_curves(id, crop) -> Result<Option<CurveSuggestion>>` | Fits the crop's outline (a quad with its fine angle baked in, or the corners of a curved crop) in the display proxy. `Ok(None)` while the image is not ready; `Err(NoCrop)` for an unknown crop or one without an outline; the usual `Err` for an unknown image. 5 to 25 ms of one thread (release, 512 px of work picture: about 8 ms; a large photo is reduced to 640 px first): call it from a worker, on demand. |
| `curve_hint(id, crop) -> Result<Option<CurveHint>>` | `suggest_curves` reduced to `{curvature, looksCurved}` for a chip. **Computed on demand, not during analysis**: the task allowed an additive `curveHint` in the crop view only if it cost under 3 ms per image at the detection proxy; the fit costs 5 to 10 ms (measured), so there is no `curveHint` field in `CropView` and analysis is unchanged. |

`CurveSuggestion` (serde camelCase; edge arrays are in the order top, right, bottom, left):

| Field | Meaning |
|---|---|
| `quad` | The four corners TL, TR, BR, BL, normalised. They may differ from the crop's by a few pixels: the fit intersects the fitted end tangents of the two edges at each corner and moves it by at most 4% of the shorter side. They ARE the end points of `curves`. |
| `curves` | A `CurveWarp`: 2 to 9 points per edge, `validate()` has passed, carries the crop's `quarterTurns` and `mirror`. Pass it as it is to `set_curves` (the engine refuses corner edits on a curved crop, so corners and curves travel together). Edges that are not bent by more than the fit's noise (under 2 px of the chord, or no better supported by edge evidence than a straight line) come back straight (two points). |
| `confidence[4]` | 0..1 per edge, the mean station confidence (contrast above the picture's own texture level). Low where a finger, a hard shadow or a desk that matches the paper hides the edge; those stretches are bridged by interpolation. |
| `curvature[4]` | The CURVATURE score: the largest distance of the fitted edge from its chord, in percent of the chord length (0 for an edge left straight). |
| `bulge[4]` | The same deviation in percent of the page's SHORTER side (a short edge is noisy in percent of its own length). |
| `supportGain[4]` | How much better the fitted curve is backed by edge evidence than the best straight line through the same search windows (0 = no better, about 1 = a full-contrast edge's worth at every station). This is what keeps flat pages flat. |
| `maxCurvature` | The largest `curvature` among the edges that pass every `looksCurved` gate (0 if none). |
| `looksCurved` | Some edge passes all of: `confidence >= 0.7`, chord length >= 80 px of the 640 px working picture, deviation >= 3 px, `supportGain >= 0.35`, `curvature >= 1.0`. |
| `fellBack` | The fit was refused (non-finite or degenerate input, a result that fails `CurveWarp::validate`, a bend over 35% of the shorter side, an area change beyond 0.6..1.6): `curves` are the crop's straight edges, confidences and curvatures are 0. |

The UI is expected to show the suggestion as a PREVIEW (the `curve-result` route flattens any `CurveWarp` without
committing it), let the person accept or discard it, and only then call `set_curves` (`gesture` as for any edit,
so it is one undo step). `looksCurved` is a hint for a chip, not a decision: see the real-photo caveat in the
baseline (on the owner's labelled receipts it fires on pages that were labelled straight far more often than on
the synthetic suites).

### How the fit works

1. The fit works on a picture of at most 640 px on the long edge. For each of the four chords it lays 48 to 64
   stations; at each station it samples the colour along the chord's normal (three points one pixel apart along the
   edge averaged), over a window of about +-6% of the shorter side (more for a long edge: 3.5% of its length,
   at most 25% of the shorter side), never beyond the frame.
2. **Edge response** at offset `d`: the colour distance (Euclidean over R, G, B, in grey levels) between the mean of
   `d-3..d-1` and of `d+1..d+3`, times "the near side is paper" (a Gaussian of the distance of the near colour from the
   paper colour) times "the far side is not paper" (colours 6 and 11 px beyond must not match the paper). **Paper** is
   a model from inside the quad: the bright end (pale paper) or dark end (dark paper; decided by comparing the page's
   dominant colour with the colour just outside) of a band deeper inside than the window reaches, pooled over 17
   neighbouring stations, so dense text, a title bar or a shadow across the page do not become the paper; a local
   colour more than 1.8 times brighter or 2.6 times dimmer than the page's dominant colour is refused.
3. **Viterbi** over the stations: one offset per station, a cost of -contrast (saturated at 40 grey levels), a slope
   penalty (6 per squared slope) and a weak pull to the chord. If more than 10% of the stations sit at the window's
   edge the window is re-centred on the result and the search repeated (4 passes at most).
4. Per station confidence from the contrast above the picture's texture floor; stretches below 0.3 are bridged
   linearly; an edge with under 25% reliable stations is left straight. A robust (Tukey) degree-5 polynomial trend pulls
   isolated detours back; a [1 2 1] smoothing removes jitter.
5. Corners refined, polylines simplified (Ramer-Douglas-Peucker at 0.4% of the shorter side, then points are
   added where the Catmull-Rom spline strays more than 1.5 tolerances, 9 points at most), validated, curvature scored.

The fit does not know about the page's inside: a wrinkle inside the page, a fold, curvature that the four edges do not
show. It follows the strongest paper-to-not-paper edge near the chord, so a quad that is wrong by more than a few
windows cannot be repaired by it, and an object abutting the page can lead it astray.

### Thresholds

The five `looksCurved` gates are constants in `curves_fit.rs` (`LOOKS_CURVED_*`). They were chosen on the DEV split of
synthetic suites only (flat `smoke`, `smoke-b`, `smoke-c` and the curved suite), one gate at a time, with a margin:
no flat page of the dev split passed at `supportGain >= 0.3`; the shipped 0.35 and confidence 0.7 sit one step
above. They were never tuned on the owner's real files. See the baseline for the numbers on the held-out splits.

## The editor (shell and UI)

`crates/shell`: the commands `curve_from_quad(id, crop)`, `set_curves(id, crop, curves, phase, label, gesture)`
(`phase: "live"` validates and records nothing; anything else is one undo step per gesture id) and
`clear_curves(id, crop)`; `curves` is the `CurveSet` of `ui/src/lib/types.ts` (the JSON of `CurveWarp`). The flattened
preview of a candidate set is NOT an IPC call: it is the `acimg` route
`/<token>/<id>/crop/<crop>/curve-<thumb|result>?t=..&r=..&b=..&l=..&q=<turns>&m=<0|1>` (flat lists of normalised
`x,y,x,y...` per edge, digits, commas, dots and minus signs only; `parse_curve_query` is strict). `thumb` is the small
fast one for a drag in progress, `result` the full preview size (the straight-crop comparison); a newer request of the
same kind cancels an older one (`Superseded`, answered 204), a shape the renderer refuses is answered 422. Nothing is
committed or cached; the committed picture is the ordinary per-crop image (`crop/<crop>/result?k=<renderKey>`).

`ui/src`: `curve.ts` is the spline, arc length, validation and Coons patch in TypeScript, held to
`curved-pages-vectors.json` to 1e-9 (`curve.test.ts`); it draws the outline, picks handles and refuses what the engine
would refuse, and never computes pixels (only the browser mock does, `mock-scene.ts`). `curve-edit.ts` is the pure
state machine of the handles (`curve-edit.test.ts`). Behaviour:

* Straight | Curved per crop. Curved calls `curve_from_quad`; Straight (or Back to straight) calls `clear_curves`
  (Undo brings the curves back). Corner handles of a curved page move the end points of the two curves that meet
  there and the interior points follow the chord (the labeller's rule), through `set_curves`; the quad operations
  the engine refuses (`set_crop_edit`, angle, merge, cut) are disabled with the reason, and an `ITEM_OP` on a curved
  page is explained.
* One handle per corner and per interior point, and one hollow handle in the middle of an edge that has none (it
  becomes a point when dragged or on Enter). 16 px dot in a 48 px hit area; double-click or press and hold on an edge
  adds a point, double-click on a point or Delete removes it (an edge keeps its two corners, at most 32 points);
  roving tab stop with `[` `]` / Page keys / Home / End, arrows nudge 1 px, Shift 10 px, Alt a quarter pixel; Esc
  deselects; Add point, Remove, Straighten and Reset buttons; the selected point has x and y fields. A drag is one
  `set_curves` with a fresh gesture id; a burst of nudges shares one (`GestureClock`, 500 ms).
* The picture: while dragging, the inset asks for the `curve-thumb` preview about every 90 ms; when the drag lands it
  keeps that picture until the committed crop image has loaded. Flattened | Straight crop compares with the same
  corners and straight edges (`curve-result` of the reset set).
* A curved page is held: `CropView.band` is `check` until `accept_scan`, the banner says 'Curved page: review, then
  accept', Replace original is off with the reason until then (the engine answers `HELD_FOR_REVIEW`, notice
  `curved.held`), Save as copy always works, Save all never writes it unaccepted.
* Try sample images writes `receipt_curved.jpg`: a receipt bent by an analytic model (`samples::curved_page_point`) whose
  edges are known exactly (`curved_sample_edges`); the plain detector gets its bottom edge wrong (Check, weak edge),
  which is a fair picture of what the editor is for.

### Suggest curves (UI)

The shell command `suggest_curves(id, crop)` (`async`, on the blocking pool like `redetect`: the fit is 5 to 25 ms but the
main thread is never held) returns `Engine::suggest_curves` as it is (`CurveSuggestion | null` in `ui/src/lib/types.ts`;
`null` while the image is not ready). `crates/shell/tests/suggest_json.rs` pins the wire shape on the real engine and
`ui/src/lib/suggest.test.ts` pins it against the Rust struct and two real answers (`ui/src/lib/fixtures/`, regenerate with
`AUTOCROP_REGEN_FIXTURES=1 cargo test -p auto-crop-shell --test suggest_json`). The browser mock answers with the curved
sample's own edges (a weaker bottom edge, as the sample's detector flags it too) when the corners are near the page, and
with a `fellBack` answer for any other page (`mock-suggest.ts`).

* **Where.** A "Suggest curves" button sits in the curve bar in both modes. In Curved it asks at once; in Straight it first
  runs `curve_from_quad` (one undo step, "Curve edges", and the first-use sheet if it has not been seen), then asks. If the
  answer is "nothing to suggest" the page stays Curved with straight edges: Back to straight (or Undo) returns.
* **Preview, never auto-applied.** The answer is a PREVIEW: the suggested outline in orange (a solid line with a dark casing,
  so it is told apart from the dashed blue current outline by more than colour) over the picture, and the flattened page in the
  inset (the `curve-result` route with the suggestion's curves as the query, bordered orange and captioned "Suggested").
  "Apply" calls `set_curves` with the suggestion's `curves` WHOLE, `label` "Suggest curves", no gesture id: ONE undo step. The
  crop is then an ordinary curved page, held for review until accepted, exactly as after a hand edit. "Dismiss" (or Esc) drops it
  and records nothing. Apply is always available, including after a refused fit.
* **Honest words** (`suggest.ts`, `strings.ts`): `fellBack`, or four edges that all came back straight, is "No clear curved edge
  found: the page may be flat" (and the panel says there is nothing to suggest); an edge with `confidence < 0.7` is "Not sure about
  the right edge: check it" (the edges are named, "any of the edges" for all four); a suggestion whose largest bend is under 1.5%
  is "This edge is nearly straight: the suggestion may not help". The largest bend is the maximum of `curvature[]` and
  `maxCurvature`: `maxCurvature` is 0 when no edge passes every `looksCurved` gate, which would otherwise call a clearly bent
  edge nearly straight.
* **Stale answers.** A suggestion belongs to (image, crop, the crop's render key). Switching crop or image, a committed edit, an
  undo or a redo, and the start of a handle drag drop a loading or previewed suggestion (announced once: "The suggestion was
  dropped because the page changed"); an answer for a request that was dismissed, replaced or dropped is ignored; Apply on a
  picture that has changed does nothing. The state machine is `suggest.ts` (idle, loading, preview, applied, dismissed, error),
  tested in `suggest.test.ts`.
* **Accessibility.** The result is a `role="status"` `aria-live="polite"` region that is always in the page (empty when idle).
  Focus moves to Apply when a preview arrives and back to the Suggest button after Apply or Dismiss. The spinner stops under
  `prefers-reduced-motion`. The panel wraps at 375 px.
* **No "looks curved" chip, badge or filter, on purpose.** `looksCurved` is not read anywhere in the UI (a test scans the
  sources). On the owner's labelled receipts it fired on 13% of the pages labelled flat, and it fires on a flat page of the app's
  own samples as well (`docs/perf/curve-fit-baseline.md`, "Real files"); it also misses about half of the pages that are curved by
  1.5% or more even on synthetic data. Suggesting curves is something the person asks for.
* **First use.** The first time Curved is entered (by the Curved button, or through Suggest curves from Straight) a one-time sheet
  says what it fixes (bowed edges, perspective), what it does not (wrinkles and folds inside the page) and that nothing is written
  until the page is accepted. "Got it" remembers it in the engine setting `curvedIntroAck` (`settings.toml`; the whole settings
  object is sent back as for every setting; an older file loads as "not seen") and carries on; "Not now" changes nothing and the
  sheet is shown again next time.
