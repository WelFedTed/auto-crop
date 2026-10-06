// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// Pure geometry for multi-item scans: hit testing, the minimum-area rectangle of a union (the Merge
// preview), the pieces of a cut (the Cut preview) and the cut line a drag describes. The engine computes the
// real results (crates/core/src/items.rs); these are the same formulas, so a preview matches what Confirm
// produces. Quads are normalised (0..1 of the source image), TL, TR, BR, BL.

import { solveHomography, applyH } from './homography.ts';
import type { Quad } from './quad.ts';
import type { Cut, Pt } from './types.ts';

type P = [number, number];

/** A cut piece must cover at least this fraction of the scan (core MIN_PIECE_FRACTION). */
export const MIN_PIECE_FRACTION = 0.02;
/** Most items one scan may hold (core MAX_ITEMS). */
export const MAX_ITEMS = 32;

/** Pixel-space area of a quad on a `w` x `h` source. */
export function quadAreaPx(q: readonly Pt[], w: number, h: number): number {
  let s = 0;
  for (let i = 0; i < q.length; i++) {
    const a = q[i];
    const b = q[(i + 1) % q.length];
    s += a.x * w * (b.y * h) - b.x * w * (a.y * h);
  }
  return Math.abs(s) / 2;
}

/** Area as a fraction of the scan (the engine's `area_fraction`). */
export function areaFraction(q: readonly Pt[], w: number, h: number): number {
  return quadAreaPx(q, w, h) / Math.max(1, w * h);
}

/** Point in convex-or-simple polygon (ray casting). */
export function pointInPolygon(p: Pt, poly: readonly Pt[]): boolean {
  let inside = false;
  for (let i = 0, j = poly.length - 1; i < poly.length; j = i++) {
    const a = poly[i];
    const b = poly[j];
    if (a.y > p.y !== b.y > p.y && p.x < ((b.x - a.x) * (p.y - a.y)) / (b.y - a.y) + a.x) inside = !inside;
  }
  return inside;
}

export function centre(q: readonly Pt[]): Pt {
  return { x: q.reduce((s, p) => s + p.x, 0) / q.length, y: q.reduce((s, p) => s + p.y, 0) / q.length };
}

export function bounds(q: readonly Pt[]): { x0: number; y0: number; x1: number; y1: number } {
  return {
    x0: Math.min(...q.map((p) => p.x)),
    y0: Math.min(...q.map((p) => p.y)),
    x1: Math.max(...q.map((p) => p.x)),
    y1: Math.max(...q.map((p) => p.y)),
  };
}

/**
 * Which crop a tap hits: the selected one wins overlaps (its handles are on top), otherwise the smallest
 * crop that contains the point, so a small item lying on a big one stays reachable.
 */
export function hitCrop(
  p: Pt,
  crops: readonly { id: number; quad: readonly Pt[]; include: boolean }[],
  selectedId: number | null,
  includeExcluded: boolean,
  w = 1,
  h = 1,
): number | null {
  const hits = crops.filter((c) => (c.include || includeExcluded) && pointInPolygon(p, c.quad));
  if (hits.length === 0) return null;
  const sel = hits.find((c) => c.id === selectedId);
  if (sel) return sel.id;
  hits.sort((a, b) => quadAreaPx(a.quad, w, h) - quadAreaPx(b.quad, w, h) || a.id - b.id);
  return hits[0].id;
}

// ---------------------------------------------------------------------------------------- min-area rect
function cross(o: P, a: P, b: P): number {
  return (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
}

/** Convex hull (Andrew's monotone chain), counter-clockwise on a y-up plane; collinear points dropped. */
export function convexHull(points: readonly P[]): P[] {
  const pts = points
    .filter((p) => Number.isFinite(p[0]) && Number.isFinite(p[1]))
    .map((p) => [p[0], p[1]] as P)
    .sort((a, b) => a[0] - b[0] || a[1] - b[1]);
  const uniq = pts.filter((p, i) => i === 0 || p[0] !== pts[i - 1][0] || p[1] !== pts[i - 1][1]);
  if (uniq.length < 3) return uniq;
  const lower: P[] = [];
  for (const p of uniq) {
    while (lower.length >= 2 && cross(lower[lower.length - 2], lower[lower.length - 1], p) <= 0) lower.pop();
    lower.push(p);
  }
  const upper: P[] = [];
  for (let i = uniq.length - 1; i >= 0; i--) {
    const p = uniq[i];
    while (upper.length >= 2 && cross(upper[upper.length - 2], upper[upper.length - 1], p) <= 0) upper.pop();
    upper.push(p);
  }
  lower.pop();
  upper.pop();
  return lower.concat(upper);
}

/** The minimum-area enclosing rectangle of `points` (pixel space), or null for fewer than three hull points. */
export function minAreaRect(points: readonly P[]): [P, P, P, P] | null {
  const hull = convexHull(points);
  if (hull.length < 3) return null;
  let best: { area: number; c: [P, P, P, P] } | null = null;
  for (let i = 0; i < hull.length; i++) {
    const a = hull[i];
    const b = hull[(i + 1) % hull.length];
    const len = Math.hypot(b[0] - a[0], b[1] - a[1]);
    if (len < 1e-9) continue;
    const ux = (b[0] - a[0]) / len;
    const uy = (b[1] - a[1]) / len;
    let minU = Infinity;
    let maxU = -Infinity;
    let minV = Infinity;
    let maxV = -Infinity;
    for (const p of hull) {
      const u = (p[0] - a[0]) * ux + (p[1] - a[1]) * uy;
      const v = -(p[0] - a[0]) * uy + (p[1] - a[1]) * ux;
      minU = Math.min(minU, u);
      maxU = Math.max(maxU, u);
      minV = Math.min(minV, v);
      maxV = Math.max(maxV, v);
    }
    const area = (maxU - minU) * (maxV - minV);
    if (!best || area < best.area - 1e-9) {
      const at = (u: number, v: number): P => [a[0] + u * ux - v * uy, a[1] + u * uy + v * ux];
      best = { area, c: [at(minU, minV), at(maxU, minV), at(maxU, maxV), at(minU, maxV)] };
    }
  }
  return best ? best.c : null;
}

/** Orders four corners clockwise on screen (y down) starting with the corner nearest the top-left. */
export function orderTLTRBRBL(c: readonly P[]): [P, P, P, P] {
  const cx = c.reduce((s, p) => s + p[0], 0) / 4;
  const cy = c.reduce((s, p) => s + p[1], 0) / 4;
  const sorted = c.slice().sort((a, b) => Math.atan2(a[1] - cy, a[0] - cx) - Math.atan2(b[1] - cy, b[0] - cx));
  // atan2 on a y-down plane runs clockwise on screen. Start at the corner with the smallest x + y.
  let start = 0;
  for (let i = 1; i < 4; i++) if (sorted[i][0] + sorted[i][1] < sorted[start][0] + sorted[start][1]) start = i;
  return [sorted[start], sorted[(start + 1) % 4], sorted[(start + 2) % 4], sorted[(start + 3) % 4]];
}

/** The minimum-area rectangle of the union of `quads` on a `w` x `h` source, as a normalised quad. */
export function mergedQuad(quads: readonly (readonly Pt[])[], w: number, h: number): Quad | null {
  const pts: P[] = quads.flatMap((q) => q.map((p) => [p.x * w, p.y * h] as P));
  const r = minAreaRect(pts);
  if (!r) return null;
  const o = orderTLTRBRBL(r);
  return o.map(([x, y]) => ({ x: x / w, y: y / h })) as Quad;
}

// ----------------------------------------------------------------------------------------------- cut
const lerp = (a: Pt, b: Pt, t: number): Pt => ({ x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t });

/** The two pieces of `q` for `cut` (the engine's `split_item`), or null when the cut is not valid. */
export function cutPieces(q: Quad, cut: Cut): [Quad, Quad] | null {
  const ok = (t: number) => Number.isFinite(t) && t >= 0 && t <= 1;
  if (!ok(cut.t0) || !ok(cut.t1)) return null;
  if (cut.axis === 'vertical') {
    const top = lerp(q[0], q[1], cut.t0);
    const bottom = lerp(q[3], q[2], cut.t1);
    return [
      [q[0], top, bottom, q[3]],
      [top, q[1], q[2], bottom],
    ];
  }
  const left = lerp(q[0], q[3], cut.t0);
  const right = lerp(q[1], q[2], cut.t1);
  return [
    [q[0], q[1], right, left],
    [left, right, q[2], q[3]],
  ];
}

export type CutProblem = 'bad' | 'small';

/** Why a cut would be refused, or null when both pieces cover at least 2% of the scan. */
export function cutProblem(q: Quad, cut: Cut, w: number, h: number): CutProblem | null {
  const pieces = cutPieces(q, cut);
  if (!pieces) return 'bad';
  for (const p of pieces) if (areaFraction(p, w, h) < MIN_PIECE_FRACTION) return 'small';
  return null;
}

/** The cut line that splits `q` in halves (equal fractions on both edges). */
export function halvesCut(axis: Cut['axis']): Cut {
  return { axis, t0: 0.5, t1: 0.5 };
}

/** Position and tilt sliders to a cut: `pos` is the middle (0..1), `tilt` the difference t0 - t1. */
export function cutFromSliders(axis: Cut['axis'], pos: number, tilt: number): Cut {
  const c = (v: number) => Math.min(1, Math.max(0, v));
  return { axis, t0: c(pos + tilt / 2), t1: c(pos - tilt / 2) };
}

/** The sliders of a cut (inverse of `cutFromSliders` up to clamping). */
export function slidersFromCut(cut: Cut): { pos: number; tilt: number } {
  return { pos: (cut.t0 + cut.t1) / 2, tilt: cut.t0 - cut.t1 };
}

/**
 * The cut a drag across a crop describes. `a` and `b` are the two points of the drag in source space; they are
 * mapped into the crop's own frame (the quad as a unit square) and the line is extended to the two edges it
 * crosses. The nearer axis wins (a line closer to vertical makes a vertical cut) and a line within 3% of
 * straight snaps straight. Null when the drag is too short or the quad cannot be mapped.
 */
export function cutFromDrag(q: Quad, a: Pt, b: Pt): Cut | null {
  let H;
  try {
    H = solveHomography(
      q.map((p) => ({ x: p.x, y: p.y })),
      [
        { x: 0, y: 0 },
        { x: 1, y: 0 },
        { x: 1, y: 1 },
        { x: 0, y: 1 },
      ],
    );
  } catch {
    return null;
  }
  const la = applyH(H, a);
  const lb = applyH(H, b);
  const du = lb.x - la.x;
  const dv = lb.y - la.y;
  if (Math.hypot(du, dv) < 0.05) return null;
  const clamp = (v: number) => Math.min(1, Math.max(0, v));
  const snap = (t0: number, t1: number): [number, number] => (Math.abs(t0 - t1) < 0.03 ? [(t0 + t1) / 2, (t0 + t1) / 2] : [t0, t1]);
  if (Math.abs(du) <= Math.abs(dv)) {
    // closer to vertical: where does the line cross v = 0 and v = 1?
    const at = (v: number) => la.x + (du * (v - la.y)) / dv;
    const [t0, t1] = snap(clamp(at(0)), clamp(at(1)));
    return { axis: 'vertical', t0, t1 };
  }
  const at = (u: number) => la.y + (dv * (u - la.x)) / du;
  const [t0, t1] = snap(clamp(at(0)), clamp(at(1)));
  return { axis: 'horizontal', t0, t1 };
}

/** The two end points of a cut across `q` in source space (for drawing the preview line). */
export function cutLine(q: Quad, cut: Cut): [Pt, Pt] {
  if (cut.axis === 'vertical') return [lerp(q[0], q[1], cut.t0), lerp(q[3], q[2], cut.t1)];
  return [lerp(q[0], q[3], cut.t0), lerp(q[1], q[2], cut.t1)];
}

// ----------------------------------------------------------------------------------------------- misc
/** A box drawn by dragging from `a` to `b`, as a normalised quad; null when it is smaller than `minFrac` of the scan. */
export function boxQuad(a: Pt, b: Pt, minFrac = 0.01): Quad | null {
  const x0 = Math.min(a.x, b.x);
  const x1 = Math.max(a.x, b.x);
  const y0 = Math.min(a.y, b.y);
  const y1 = Math.max(a.y, b.y);
  const c = (v: number) => Math.min(1, Math.max(0, v));
  const q: Quad = [
    { x: c(x0), y: c(y0) },
    { x: c(x1), y: c(y0) },
    { x: c(x1), y: c(y1) },
    { x: c(x0), y: c(y1) },
  ];
  return (q[1].x - q[0].x) * (q[3].y - q[0].y) >= minFrac ? q : null;
}

/** Intersection over union of two axis-aligned bounding boxes of quads (cheap, for the mock). */
export function boxIou(a: readonly Pt[], b: readonly Pt[]): number {
  const A = bounds(a);
  const B = bounds(b);
  const iw = Math.max(0, Math.min(A.x1, B.x1) - Math.max(A.x0, B.x0));
  const ih = Math.max(0, Math.min(A.y1, B.y1) - Math.max(A.y0, B.y0));
  const inter = iw * ih;
  const union = (A.x1 - A.x0) * (A.y1 - A.y0) + (B.x1 - B.x0) * (B.y1 - B.y0) - inter;
  return union > 0 ? inter / union : 0;
}
