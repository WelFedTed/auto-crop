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
- A curve parameter `t` in [0, 1] is chosen by ARC LENGTH (computed on a 64-samples-per-segment polyline), so
  a point at `t = 0.5` is halfway along the edge, whatever the control point spacing is.
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
source pixels (arc length on the full-resolution image), then capped like every other output (pixel cap,
memory budget). The output pixel `(x, y)` samples the source at `S(x / (W-1), y / (H-1))` with Lanczos3 (the
same kernel as the homography path), strip-wise, never a full-image f32 copy. Where the grid maps outside
the source the pixel is the paper-edge colour or transparent as the existing warp does (follow the quad
path's behaviour). The straight-quad result must equal the existing homography result within 1 LSB when all
four curves are straight and the quad is a rectangle in the image plane; for a perspective quad the Coons path
is NOT a homography (it is bilinear): that is acceptable and documented, and straight quads keep using the
homography path.

## Persistence

`EditState` stores the CURVE CONTROL POINTS (editable), not only the derived grid: a new geometry variant
(`Geometry::Curved(CurveWarp)`), with schema migration (EditState v3), serde names in camelCase, caps (32
points per curve), and `Geometry::Grid` (the dense grid of the learned model, M12.27) remains separate. A
curved item is HELD for review in every triage mode unless the user accepted it (it is never auto-written),
and it never takes the lossless JPEG path.
