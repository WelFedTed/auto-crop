// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// The boundary-curve model of docs/dev/curved-pages.md in TypeScript: what the editor draws and validates while
// a handle is dragged. It is the same arithmetic as crates/core/src/curve.rs (the Rust spline) and the labeller's
// JavaScript block, and it is held to the shared vectors in docs/dev/curved-pages-vectors.json to 1e-9
// (curve.test.ts). The ENGINE still renders the flattened page (no pixels are computed here, PLAN 2.5); this file
// only draws the outline, picks handles and refuses a shape the engine would refuse.
//
// A page is four curves, top (TL to TR), right (TR to BR), bottom (BR to BL) and left (BL to TL). Each is a list
// of 2 to 32 points whose first and last point ARE the page corners. Between points the curve is a centripetal
// Catmull-Rom spline; a curve parameter is by arc length. Coordinates are normalised (0..1, y down).

import type { CurveSet, Pt } from './types.ts';
import type { Quad } from './quad.ts';

export type EdgeIndex = 0 | 1 | 2 | 3;
export const EDGE_KEYS = ['top', 'right', 'bottom', 'left'] as const;
export type EdgeKey = (typeof EDGE_KEYS)[number];

export const MIN_POINTS = 2;
export const MAX_POINTS = 32;
export const SAMPLES_PER_SEGMENT = 64;
const CHECK_SAMPLES = 8;
const MIN_KNOT = 1e-9;
/** The end points of the curves must agree with each other within this (normalised units). */
export const CORNER_TOLERANCE = 1e-6;
/** A point may lie this far outside the 0..1 frame (one frame width or height). */
export const FRAME_MARGIN = 1;

export type Scale = readonly [number, number];
const UNIT: Scale = [1, 1];

const dist = (a: Pt, b: Pt): number => Math.hypot(a.x - b.x, a.y - b.y);
const lerp = (a: Pt, b: Pt, t: number): Pt => ({ x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t });

// ------------------------------------------------------------------------------------------------ the spline

/** The controls of segment `seg` (between points `seg` and `seg + 1`): the phantom point reflected through an end. */
function segmentControls(p: readonly Pt[], seg: number): [Pt, Pt, Pt, Pt] {
  const n = p.length;
  const s = Math.min(seg, n - 2);
  const p0 = s === 0 ? { x: 2 * p[0].x - p[1].x, y: 2 * p[0].y - p[1].y } : p[s - 1];
  const p3 = s + 2 >= n ? { x: 2 * p[n - 1].x - p[n - 2].x, y: 2 * p[n - 1].y - p[n - 2].y } : p[s + 2];
  return [p0, p[s], p[s + 1], p3];
}

/**
 * The point at local parameter `s` (0..1, by the spline's own knots, NOT arc length) of segment `seg`:
 * centripetal Catmull-Rom (alpha 0.5), Barry-Goldman form. Two points are the straight lerp.
 */
export function evalSegment(p: readonly Pt[], seg: number, s: number): Pt {
  const n = p.length;
  const [p0, p1, p2, p3] = segmentControls(p, seg);
  if (n === 2) return lerp(p1, p2, s);
  const knot = (a: Pt, b: Pt) => Math.max(Math.sqrt(dist(a, b)), MIN_KNOT);
  const t0 = 0;
  const t1 = t0 + knot(p0, p1);
  const t2 = t1 + knot(p1, p2);
  const t3 = t2 + knot(p2, p3);
  const t = t1 + s * (t2 - t1);
  const mix = (a: Pt, b: Pt, ta: number, tb: number): Pt => {
    const d = tb - ta;
    const wa = (tb - t) / d;
    const wb = (t - ta) / d;
    return { x: wa * a.x + wb * b.x, y: wa * a.y + wb * b.y };
  };
  const a1 = mix(p0, p1, t0, t1);
  const a2 = mix(p1, p2, t1, t2);
  const a3 = mix(p2, p3, t2, t3);
  const b1 = mix(a1, a2, t0, t2);
  const b2 = mix(a2, a3, t1, t3);
  return mix(b1, b2, t1, t2);
}

/** The polyline of `per` steps per segment (`per * segments + 1` points; the knots themselves exact). */
export function polyline(p: readonly Pt[], per: number = SAMPLES_PER_SEGMENT): Pt[] {
  const segs = p.length - 1;
  const out: Pt[] = [{ x: p[0].x, y: p[0].y }];
  for (let seg = 0; seg < segs; seg++) {
    for (let k = 1; k <= per; k++) {
      out.push(k === per ? { x: p[seg + 1].x, y: p[seg + 1].y } : evalSegment(p, seg, k / per));
    }
  }
  return out;
}

/** A polyline prepared for arc-length lookups, lengths measured in `scale` space. */
export interface Arc {
  pts: Pt[];
  cum: number[];
}

export function arcOfPolyline(pts: Pt[], scale: Scale = UNIT): Arc {
  const cum = [0];
  let total = 0;
  for (let i = 1; i < pts.length; i++) {
    total += Math.hypot((pts[i].x - pts[i - 1].x) * scale[0], (pts[i].y - pts[i - 1].y) * scale[1]);
    cum.push(total);
  }
  return { pts, cum };
}

export function arcOf(p: readonly Pt[], scale: Scale = UNIT): Arc {
  return arcOfPolyline(polyline(p), scale);
}

export const arcLength = (a: Arc): number => a.cum[a.cum.length - 1];

/** The point at arc-length fraction `t` (clamped to 0..1): the polyline point, interpolated (Rust `ArcCurve::at`). */
export function arcAt(a: Arc, t: number): Pt {
  const total = arcLength(a);
  const n = a.pts.length;
  if (Number.isNaN(total) || !(total > 0)) return { ...a.pts[0] };
  const tt = Number.isNaN(t) ? 0 : Math.min(1, Math.max(0, t));
  const target = tt * total;
  // partition_point(cum <= target): the count of leading entries that are <= target
  let lo = 0;
  let hi = a.cum.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (a.cum[mid] <= target) lo = mid + 1;
    else hi = mid;
  }
  const k = Math.min(Math.max(lo - 1, 0), n - 2);
  const span = a.cum[k + 1] - a.cum[k];
  const f = span > 0 ? Math.min(1, Math.max(0, (target - a.cum[k]) / span)) : 0;
  return lerp(a.pts[k], a.pts[k + 1], f);
}

export function curveLength(p: readonly Pt[], scale: Scale = UNIT): number {
  return arcLength(arcOf(p, scale));
}

export function curveAt(p: readonly Pt[], t: number, scale: Scale = UNIT): Pt {
  return arcAt(arcOf(p, scale), t);
}

// ------------------------------------------------------------------------------------------------ the page

export const edgeOf = (c: CurveSet, e: number): Pt[] => c[EDGE_KEYS[e]];

export function cloneCurves(c: CurveSet): CurveSet {
  const k = (a: Pt[]) => a.map((p) => ({ x: p.x, y: p.y }));
  return { top: k(c.top), right: k(c.right), bottom: k(c.bottom), left: k(c.left), quarterTurns: c.quarterTurns, mirror: c.mirror };
}

/** The corners TL, TR, BR, BL: the first point of top, right, bottom and left. */
export function cornersOf(c: CurveSet): Quad {
  return [c.top[0], c.right[0], c.bottom[0], c.left[0]].map((p) => ({ x: p.x, y: p.y })) as Quad;
}

/** A page with four straight edges through `quad`; the turns and mirror are kept. */
export function curvesFromQuad(quad: Quad, quarterTurns = 0, mirror = false): CurveSet {
  const [tl, tr, br, bl] = quad.map((p) => ({ x: p.x, y: p.y }));
  return { top: [tl, tr], right: [tr, br], bottom: [br, bl], left: [bl, tl], quarterTurns, mirror };
}

/** True when the edge adds nothing to the chord: no interior point, or all of them lie on it in order. */
export function edgeIsStraight(p: readonly Pt[]): boolean {
  if (p.length <= 2) return true;
  const a = p[0];
  const b = p[p.length - 1];
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const l = Math.hypot(dx, dy);
  if (!(l > 1e-9)) return false;
  let last = 0;
  for (let i = 1; i < p.length - 1; i++) {
    const s = ((p[i].x - a.x) * dx + (p[i].y - a.y) * dy) / (l * l);
    const off = ((p[i].x - a.x) * dy - (p[i].y - a.y) * dx) / l;
    if (Math.abs(off) > 1e-4 * l || !(s > last) || !(s < 1)) return false;
    last = s;
  }
  return true;
}

export const bentEdges = (c: CurveSet): boolean[] => EDGE_KEYS.map((k) => !edgeIsStraight(c[k]));
export const isStraightPage = (c: CurveSet): boolean => EDGE_KEYS.every((k) => edgeIsStraight(c[k]));
/** Interior points over the four edges. */
export const pointCount = (c: CurveSet): number => EDGE_KEYS.reduce((n, k) => n + c[k].length - 2, 0);

export type CurveProblem =
  | 'points' // fewer than 2 or more than 32
  | 'nonFinite'
  | 'range'
  | 'coincident'
  | 'corners'
  | 'noArea'
  | 'crossing';

function polygonArea(p: readonly Pt[]): number {
  let s = 0;
  for (let i = 0; i < p.length; i++) {
    const a = p[i];
    const b = p[(i + 1) % p.length];
    s += a.x * b.y - b.x * a.y;
  }
  return s / 2;
}

const orient = (a: Pt, b: Pt, c: Pt): number => (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);

function segmentsCross(a: Pt, b: Pt, c: Pt, d: Pt): boolean {
  const o1 = orient(a, b, c);
  const o2 = orient(a, b, d);
  const o3 = orient(c, d, a);
  const o4 = orient(c, d, b);
  return ((o1 > 0 && o2 < 0) || (o1 < 0 && o2 > 0)) && ((o3 > 0 && o4 < 0) || (o3 < 0 && o4 > 0));
}

function selfIntersects(p: readonly Pt[]): boolean {
  const n = p.length;
  for (let i = 0; i < n; i++) {
    const a = p[i];
    const b = p[(i + 1) % n];
    for (let j = i + 2; j < n; j++) {
      if (i === 0 && j === n - 1) continue;
      if (segmentsCross(a, b, p[j], p[(j + 1) % n])) return true;
    }
  }
  return false;
}

/** The closed outline as a coarse polyline (top, right, bottom, left; each corner once). */
function outlinePolyline(c: CurveSet, per: number): Pt[] {
  const out: Pt[] = [];
  for (const k of EDGE_KEYS) {
    const p = polyline(c[k], per);
    p.pop();
    out.push(...p);
  }
  return out;
}

/** `CurveWarp::validate` of the Rust core: null when the engine would accept this curve set, else why not. */
export function validateCurves(c: CurveSet): CurveProblem | null {
  for (const k of EDGE_KEYS) {
    const p = c[k];
    if (p.length < MIN_POINTS || p.length > MAX_POINTS) return 'points';
    if (p.some((q) => !Number.isFinite(q.x) || !Number.isFinite(q.y))) return 'nonFinite';
    if (p.some((q) => q.x < -FRAME_MARGIN || q.x > 1 + FRAME_MARGIN || q.y < -FRAME_MARGIN || q.y > 1 + FRAME_MARGIN)) return 'range';
    for (let i = 1; i < p.length; i++) if (dist(p[i - 1], p[i]) < MIN_KNOT) return 'coincident';
  }
  const joins: [Pt, Pt][] = [
    [c.top[c.top.length - 1], c.right[0]],
    [c.right[c.right.length - 1], c.bottom[0]],
    [c.bottom[c.bottom.length - 1], c.left[0]],
    [c.left[c.left.length - 1], c.top[0]],
  ];
  if (joins.some(([a, b]) => Math.abs(a.x - b.x) > CORNER_TOLERANCE || Math.abs(a.y - b.y) > CORNER_TOLERANCE)) return 'corners';
  const poly = outlinePolyline(c, CHECK_SAMPLES);
  if (Math.abs(polygonArea(poly)) < 1e-9) return 'noArea';
  if (selfIntersects(poly)) return 'crossing';
  return null;
}

// ------------------------------------------------------------------------------------------------ the Coons patch

/**
 * `CurveWarp::to_grid_scaled`: the source-space Coons grid, node `(i, j)` = `S(i / (cols-1), j / (rows-1))` with the
 * edges walked by arc length in `scale` space. The product renders in Rust; this is for the shared vectors and for
 * the browser mock's stand-in renderer.
 */
export function coonsGrid(c: CurveSet, cols: number, rows: number, scale: Scale = UNIT): Pt[] {
  const top = arcOf(c.top, scale);
  const right = arcOf(c.right, scale);
  const bottom = arcOf(c.bottom, scale);
  const left = arcOf(c.left, scale);
  const [tl, tr, br, bl] = cornersOf(c);
  const out: Pt[] = [];
  const nc = Math.max(2, cols);
  const nr = Math.max(2, rows);
  for (let j = 0; j < nr; j++) {
    const v = j / (nr - 1);
    for (let i = 0; i < nc; i++) {
      const u = i / (nc - 1);
      out.push(coonsBlend([tl, tr, br, bl], u, v, arcAt(top, u), arcAt(bottom, 1 - u), arcAt(left, 1 - v), arcAt(right, v)));
    }
  }
  return out;
}

export function coonsBlend(corners: readonly [Pt, Pt, Pt, Pt], u: number, v: number, t: Pt, b: Pt, l: Pt, r: Pt): Pt {
  const [tl, tr, br, bl] = corners;
  const wtl = (1 - u) * (1 - v);
  const wtr = u * (1 - v);
  const wbl = (1 - u) * v;
  const wbr = u * v;
  return {
    x: (1 - v) * t.x + v * b.x + (1 - u) * l.x + u * r.x - (wtl * tl.x + wtr * tr.x + wbl * bl.x + wbr * br.x),
    y: (1 - v) * t.y + v * b.y + (1 - u) * l.y + u * r.y - (wtl * tl.y + wtr * tr.y + wbl * bl.y + wbr * br.y),
  };
}

export function arcLengths(c: CurveSet, scale: Scale = UNIT): [number, number, number, number] {
  return [curveLength(c.top, scale), curveLength(c.right, scale), curveLength(c.bottom, scale), curveLength(c.left, scale)];
}

/** The natural size of the flattened page before the turns: the longer of top and bottom by the longer of left and right. */
export function flatSize(c: CurveSet, scale: Scale = UNIT): [number, number] {
  const [t, r, b, l] = arcLengths(c, scale);
  return [Math.max(t, b), Math.max(l, r)];
}

// ------------------------------------------------------------------------------------------------ editing

const clamp01 = (v: number): number => Math.min(1, Math.max(0, v));
export const clampPt = (p: Pt): Pt => ({ x: clamp01(p.x), y: clamp01(p.y) });

/**
 * A corner moved: the interior points of an edge keep their place in the chord frame (X = P + a d + b perp(d),
 * d = Q - P), so the edge turns and scales with its chord. Same rule as the labeller (similarity P,Q -> P',Q').
 */
export function retargetInterior(interior: readonly Pt[], p0: Pt, q0: Pt, p1: Pt, q1: Pt): Pt[] {
  const dx = q0.x - p0.x;
  const dy = q0.y - p0.y;
  const l2 = dx * dx + dy * dy;
  if (!(l2 > 1e-18)) return interior.map((x) => ({ x: x.x + p1.x - p0.x, y: x.y + p1.y - p0.y }));
  const ex = q1.x - p1.x;
  const ey = q1.y - p1.y;
  const re = (ex * dx + ey * dy) / l2;
  const im = (ey * dx - ex * dy) / l2;
  return interior.map((x) => {
    const vx = x.x - p0.x;
    const vy = x.y - p0.y;
    return { x: p1.x + re * vx - im * vy, y: p1.y + re * vy + im * vx };
  });
}

/**
 * Moves corner `i` (0 TL, 1 TR, 2 BR, 3 BL) to `p`. The two curves that meet there end at the new corner and their
 * interior points follow with it (a chord similarity), so a bowed edge stays bowed the same way.
 */
export function moveCorner(c: CurveSet, i: number, p: Pt): CurveSet {
  const next = cloneCurves(c);
  const to = clampPt(p);
  const out = i; // the curve that starts at this corner
  const inn = (i + 3) % 4; // the curve that ends at it
  const A = edgeOf(next, out);
  const B = edgeOf(next, inn);
  const aOld = c[EDGE_KEYS[out]];
  const bOld = c[EDGE_KEYS[inn]];
  const aInner = retargetInterior(aOld.slice(1, -1), aOld[0], aOld[aOld.length - 1], to, aOld[aOld.length - 1]);
  const bInner = retargetInterior(bOld.slice(1, -1), bOld[0], bOld[bOld.length - 1], bOld[0], to);
  next[EDGE_KEYS[out]] = [{ ...to }, ...aInner.map(clampPt), A[A.length - 1]];
  next[EDGE_KEYS[inn]] = [B[0], ...bInner.map(clampPt), { ...to }];
  return next;
}

/** Moves interior point `m` (0-based among the interior points) of edge `e`. */
export function movePoint(c: CurveSet, e: number, m: number, p: Pt): CurveSet {
  const next = cloneCurves(c);
  const pts = edgeOf(next, e);
  if (m < 0 || m >= pts.length - 2) return next;
  pts[m + 1] = clampPt(p);
  return next;
}

/** Inserts a point so that it becomes interior point `at` of edge `e` (0 = right after the first corner). */
export function insertPointAt(c: CurveSet, e: number, at: number, p: Pt): CurveSet | null {
  if (edgeOf(c, e).length >= MAX_POINTS) return null;
  const next = cloneCurves(c);
  const list = edgeOf(next, e);
  list.splice(Math.min(Math.max(0, at), list.length - 2) + 1, 0, clampPt(p));
  return next;
}

/** Removes interior point `m` of edge `e`; the end points are never removed (a curve keeps at least 2). */
export function removePoint(c: CurveSet, e: number, m: number): CurveSet | null {
  const pts = edgeOf(c, e);
  if (pts.length <= MIN_POINTS || m < 0 || m >= pts.length - 2) return null;
  const next = cloneCurves(c);
  edgeOf(next, e).splice(m + 1, 1);
  return next;
}

/** One edge back to the straight chord between its corners. */
export function straightenEdge(c: CurveSet, e: number): CurveSet {
  const next = cloneCurves(c);
  const pts = edgeOf(c, e);
  next[EDGE_KEYS[e]] = [{ ...pts[0] }, { ...pts[pts.length - 1] }];
  return next;
}

/** All four edges straight, corners kept. */
export function resetCurves(c: CurveSet): CurveSet {
  return curvesFromQuad(cornersOf(c), c.quarterTurns, c.mirror);
}

/** The spline segment (0-based) of edge `e` that a point of the polyline lies on, and where. */
export interface Nearest {
  edge: EdgeIndex;
  /** The spline segment it lies on: a point inserted there becomes interior point `seg`. */
  seg: number;
  /** The closest point on the polyline. */
  p: Pt;
  /** Distance in the units of the `scale` given. */
  d: number;
}

/** The nearest point of one edge to `q`, distances measured in `scale` space (screen-like). */
export function nearestOnEdge(c: CurveSet, e: number, q: Pt, scale: Scale = UNIT): Nearest | null {
  const pts = edgeOf(c, e);
  const per = 24;
  const poly = polyline(pts, per);
  let best: Nearest | null = null;
  for (let i = 0; i + 1 < poly.length; i++) {
    const a = poly[i];
    const b = poly[i + 1];
    const dx = (b.x - a.x) * scale[0];
    const dy = (b.y - a.y) * scale[1];
    const l2 = dx * dx + dy * dy;
    const w = l2 > 0 ? Math.min(1, Math.max(0, (((q.x - a.x) * scale[0]) * dx + ((q.y - a.y) * scale[1]) * dy) / l2)) : 0;
    const p = { x: a.x + (b.x - a.x) * w, y: a.y + (b.y - a.y) * w };
    const d = Math.hypot((p.x - q.x) * scale[0], (p.y - q.y) * scale[1]);
    if (!best || d < best.d) best = { edge: e as EdgeIndex, seg: Math.min(Math.floor(i / per), pts.length - 2), p, d };
  }
  return best;
}

/** The nearest point over the four edges. */
export function nearestOnPage(c: CurveSet, q: Pt, scale: Scale = UNIT): Nearest | null {
  let best: Nearest | null = null;
  for (let e = 0; e < 4; e++) {
    const r = nearestOnEdge(c, e, q, scale);
    if (r && (!best || r.d < best.d)) best = r;
  }
  return best;
}

/** Inserts a point at the spot of `n` (a result of `nearestOnPage`); the new point is interior point `n.seg`. */
export function insertAtNearest(c: CurveSet, n: Nearest): { curves: CurveSet; m: number } | null {
  const pts = edgeOf(c, n.edge);
  if (pts.length >= MAX_POINTS) return null;
  const next = cloneCurves(c);
  edgeOf(next, n.edge).splice(n.seg + 1, 0, clampPt(n.p));
  return { curves: next, m: n.seg };
}

/** The halfway-by-arc-length point of an edge: where the hollow handle of a straight edge sits. */
export function edgeMidpoint(c: CurveSet, e: number, scale: Scale = UNIT): Pt {
  return curveAt(edgeOf(c, e), 0.5, scale);
}

/**
 * One handle of a curved page. A corner is `{kind: 'corner', e}`; a real point of an edge is `{kind: 'point', e, m}`;
 * an edge that is only its two corners shows one hollow `{kind: 'ghost', e, m: -1}` at its middle.
 */
export type CurveHandle = { kind: 'corner'; e: EdgeIndex; m: -1 } | { kind: 'point'; e: EdgeIndex; m: number } | { kind: 'ghost'; e: EdgeIndex; m: -1 };

export const handleKey = (h: CurveHandle): string => (h.kind === 'corner' ? `c${h.e}` : h.kind === 'ghost' ? `g${h.e}` : `p${h.e}.${h.m}`);

/** The handles in boundary order: a corner, then the points of the edge after it. The order the roving tab stop walks. */
export function curveHandles(c: CurveSet): CurveHandle[] {
  const out: CurveHandle[] = [];
  for (const e of [0, 1, 2, 3] as const) {
    out.push({ kind: 'corner', e, m: -1 });
    const n = edgeOf(c, e).length - 2;
    if (n <= 0) out.push({ kind: 'ghost', e, m: -1 });
    else for (let m = 0; m < n; m++) out.push({ kind: 'point', e, m });
  }
  return out;
}

/** Where a handle sits, in normalised coordinates. */
export function handlePos(c: CurveSet, h: CurveHandle, scale: Scale = UNIT): Pt {
  if (h.kind === 'corner') return cornersOf(c)[h.e];
  if (h.kind === 'ghost') return edgeMidpoint(c, h.e, scale);
  return edgeOf(c, h.e)[h.m + 1];
}

/** The same handle after the curve set changed shape (a point was added or removed before it). */
export function sameHandle(a: CurveHandle | null, b: CurveHandle | null): boolean {
  return !!a && !!b && handleKey(a) === handleKey(b);
}

/** Moves any handle to `p`. A ghost first becomes a real point at `p`. Returns the new set and the handle that moved. */
export function moveHandle(c: CurveSet, h: CurveHandle, p: Pt): { curves: CurveSet; handle: CurveHandle } | null {
  if (h.kind === 'corner') return { curves: moveCorner(c, h.e, p), handle: h };
  if (h.kind === 'point') return { curves: movePoint(c, h.e, h.m, p), handle: h };
  const pts = edgeOf(c, h.e);
  if (pts.length >= MAX_POINTS) return null;
  const next = cloneCurves(c);
  edgeOf(next, h.e).splice(1, 0, clampPt(p));
  return { curves: next, handle: { kind: 'point', e: h.e, m: 0 } };
}

/** Screen-space path data for an edge: `M x y L x y ...`, through `toScreen`. */
export function edgePath(c: CurveSet, e: number, toScreen: (p: Pt) => Pt): string {
  return polyline(edgeOf(c, e), 24)
    .map((p, i) => {
      const s = toScreen(p);
      return `${i === 0 ? 'M' : 'L'}${s.x.toFixed(1)} ${s.y.toFixed(1)}`;
    })
    .join('');
}

/** The closed outline path of the page through `toScreen`. */
export function outlinePath(c: CurveSet, toScreen: (p: Pt) => Pt): string {
  const parts: string[] = [];
  for (let e = 0; e < 4; e++) {
    const pl = polyline(edgeOf(c, e), 24).map(toScreen);
    parts.push(pl.map((s, i) => `${parts.length === 0 && i === 0 ? 'M' : 'L'}${s.x.toFixed(1)} ${s.y.toFixed(1)}`).join(''));
  }
  return `${parts.join('')}Z`;
}

/** Compact URL form of a curve set for the `acimg` preview route: only digits, commas, dots and minus signs. */
export function curveQuery(c: CurveSet): string {
  const f = (v: number) => v.toFixed(7).replace(/\.?0+$/, '') || '0';
  const list = (p: Pt[]) => p.map((q) => `${f(q.x)},${f(q.y)}`).join(',');
  return `t=${list(c.top)}&r=${list(c.right)}&b=${list(c.bottom)}&l=${list(c.left)}&q=${c.quarterTurns}&m=${c.mirror ? 1 : 0}`;
}
