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
  in a **scaled space** `(sx, sy)` (`(1, 1)` = normalised units; the engine passes `(image width, image height)`
  so the parameter is uniform in source pixels whatever the aspect; the scale changes the parameterisation,
  never the shape). `at(t)`: `target = t * total` (`t` clamped to 0..1); `k` = the last polyline index with
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

Output size: width = max(arc length of top, bottom) and height = max(arc length of left, right) measured in
source pixels (arc length on the full-resolution image, scale `(W_src, H_src)`), then capped like every other
output (pixel cap, memory budget). **Pixel convention (changed from the first draft, which said
`x / (W-1)`):** the output pixel `(x, y)` of the flat page, whose pixel CENTRES are at integer coordinates like
the homography path, samples the source at `S((x + 0.5) / W, (y + 0.5) / H)`, i.e. the outer pixel edges map
to the page edges. This is what makes a straight rectangle equal the homography result to within 1 LSB.
`to_grid(cols, rows)` is different on purpose: node `(i, j)` is `S(i / (cols-1), j / (rows-1))`, the patch
corner to corner. Resampling is Lanczos3 (the same kernel as the homography path), strip-wise, never a
full-image f32 copy. Pixels that map outside the source are zero, exactly as the quad path does. Quarter turns
and mirror act on the flat page (mirror first, then the clockwise turns), as for a quad. The straight-quad
result must equal the existing homography result within 1 LSB when all four curves are straight and the quad is
a rectangle in the image plane; for a perspective quad the Coons path is NOT a homography (it is bilinear):
that is acceptable and documented, and straight quads keep using the homography path.

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
