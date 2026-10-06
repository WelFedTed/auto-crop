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
- Curves may leave the 0..1 frame slightly (a page cut by the frame) but must be finite and must not
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
* `CurveWarp::validate()` (called by every edit, and by the renderer): finite, 2..=32 points, neighbouring
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
