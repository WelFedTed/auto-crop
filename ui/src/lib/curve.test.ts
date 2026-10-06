// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import {
  arcLengths,
  arcOf,
  arcAt,
  coonsGrid,
  curveAt,
  curveHandles,
  curveLength,
  curveQuery,
  curvesFromQuad,
  edgeIsStraight,
  edgeOf,
  evalSegment,
  flatSize,
  handlePos,
  insertAtNearest,
  insertPointAt,
  moveCorner,
  moveHandle,
  movePoint,
  nearestOnPage,
  polyline,
  removePoint,
  resetCurves,
  straightenEdge,
  validateCurves,
  cornersOf,
  type Scale,
} from './curve.ts';
import type { Quad } from './quad.ts';
import type { CurveSet, Pt } from './types.ts';

type V = [number, number];
interface Vectors {
  tolerance: number;
  curves: {
    name: string;
    points: V[];
    scale: V;
    length: number;
    polylineLength: number;
    eval: { segment: number; s: number; point: V }[];
    at: { t: number; point: V }[];
  }[];
  coons: {
    name: string;
    top: V[];
    right: V[];
    bottom: V[];
    left: V[];
    cols: number;
    rows: number;
    scale: V;
    arcLengths: number[];
    flatSize: V;
    nodes: V[];
    valid: boolean;
  }[];
}

// The shared vectors are produced by crates/core/src/curve.rs; this implementation must match them to the
// tolerance the file states (1e-9), exactly as the labeller's JavaScript does.
const vectors = JSON.parse(readFileSync(new URL('../../../docs/dev/curved-pages-vectors.json', import.meta.url), 'utf8')) as Vectors;
const pts = (a: V[]): Pt[] => a.map(([x, y]) => ({ x, y }));
const near = (a: number, b: number, tol: number, what: string) => assert.ok(Math.abs(a - b) <= tol, `${what}: ${a} vs ${b}`);
const nearPt = (a: Pt, b: V, tol: number, what: string) => {
  near(a.x, b[0], tol, `${what} x`);
  near(a.y, b[1], tol, `${what} y`);
};

test('the vectors file is the one this test was written for', () => {
  assert.equal(vectors.tolerance, 1e-9);
  assert.ok(vectors.curves.length >= 7 && vectors.coons.length >= 4);
});

test('spline segments match the shared vectors', () => {
  for (const c of vectors.curves) {
    for (const e of c.eval) nearPt(evalSegment(pts(c.points), e.segment, e.s), e.point, vectors.tolerance, `${c.name} seg ${e.segment} s ${e.s}`);
  }
});

test('arc-length points, lengths and polyline sizes match the shared vectors, in unit and pixel space', () => {
  for (const c of vectors.curves) {
    const p = pts(c.points);
    const scale = c.scale as Scale;
    assert.equal(polyline(p).length, c.polylineLength, `${c.name} polyline size`);
    near(curveLength(p, scale), c.length, vectors.tolerance * Math.max(1, c.length), `${c.name} length`);
    const arc = arcOf(p, scale);
    for (const a of c.at) nearPt(arcAt(arc, a.t), a.point, vectors.tolerance, `${c.name} at ${a.t}`);
    nearPt(curveAt(p, 0.5, scale), c.at.find((a) => a.t === 0.5)!.point, vectors.tolerance, `${c.name} middle`);
  }
});

test('Coons grids, edge lengths and flat sizes match the shared vectors', () => {
  for (const g of vectors.coons) {
    const set: CurveSet = { top: pts(g.top), right: pts(g.right), bottom: pts(g.bottom), left: pts(g.left), quarterTurns: 0, mirror: false };
    const scale = g.scale as Scale;
    const scaleTol = vectors.tolerance * Math.max(1, ...g.arcLengths);
    assert.equal(validateCurves(set) === null, g.valid, `${g.name} validity`);
    arcLengths(set, scale).forEach((l, i) => near(l, g.arcLengths[i], scaleTol, `${g.name} arc ${i}`));
    const [fw, fh] = flatSize(set, scale);
    near(fw, g.flatSize[0], scaleTol, `${g.name} flat width`);
    near(fh, g.flatSize[1], scaleTol, `${g.name} flat height`);
    const nodes = coonsGrid(set, g.cols, g.rows, scale);
    assert.equal(nodes.length, g.nodes.length);
    nodes.forEach((n, i) => nearPt(n, g.nodes[i], vectors.tolerance, `${g.name} node ${i}`));
  }
});

// ------------------------------------------------------------------------------------------ editing and validation

const quad = (): Quad => [
  { x: 0.1, y: 0.1 },
  { x: 0.9, y: 0.1 },
  { x: 0.9, y: 0.9 },
  { x: 0.1, y: 0.9 },
];

test('a quad becomes four straight edges and back, keeping corners, turns and mirror', () => {
  const c = curvesFromQuad(quad(), 1, true);
  assert.deepEqual(cornersOf(c), quad());
  assert.equal(c.top.length + c.right.length + c.bottom.length + c.left.length, 8);
  assert.equal(c.quarterTurns, 1);
  assert.equal(c.mirror, true);
  assert.equal(validateCurves(c), null);
  const r = resetCurves(insertPointAt(c, 0, 0, { x: 0.5, y: 0.05 })!);
  assert.deepEqual(r, c);
});

test('a bent edge is a bent edge, and points on the chord are still a straight one', () => {
  const c = curvesFromQuad(quad());
  const on = insertPointAt(c, 0, 0, { x: 0.5, y: 0.1 })!;
  assert.ok(edgeIsStraight(on.top), 'a point on the chord adds no bend');
  const bent = movePoint(on, 0, 0, { x: 0.5, y: 0.04 });
  assert.ok(!edgeIsStraight(bent.top));
  assert.ok(edgeIsStraight(straightenEdge(bent, 0).top));
});

test('points keep their order, the maximum is 32 and the minimum is the two corners', () => {
  let c = curvesFromQuad(quad());
  for (let i = 0; i < 30; i++) c = insertPointAt(c, 0, i, { x: 0.12 + i * 0.025, y: 0.1 - 0.001 * i })!;
  assert.equal(c.top.length, 32);
  assert.equal(insertPointAt(c, 0, 5, { x: 0.5, y: 0.1 }), null, 'the 33rd point is refused');
  assert.equal(validateCurves(c), null);
  for (let i = 0; i < 30; i++) c = removePoint(c, 0, 0)!;
  assert.equal(c.top.length, 2);
  assert.equal(removePoint(c, 0, 0), null, 'the end points are never removed');
});

test('moving a corner carries the ends of both edges and the points between them follow the chord', () => {
  let c = curvesFromQuad(quad());
  c = movePoint(insertPointAt(c, 0, 0, { x: 0.5, y: 0.1 })!, 0, 0, { x: 0.5, y: 0.06 }); // top bows up by 0.04
  const moved = moveCorner(c, 1, { x: 0.8, y: 0.12 }); // TR
  assert.deepEqual(moved.top[moved.top.length - 1], { x: 0.8, y: 0.12 });
  assert.deepEqual(moved.right[0], { x: 0.8, y: 0.12 });
  assert.deepEqual(cornersOf(moved)[1], { x: 0.8, y: 0.12 });
  assert.equal(validateCurves(moved), null, 'the corners still meet');
  // the interior point stayed 0.04 off the chord, on the same side, at the same relative place along it
  const [a, b] = [moved.top[0], moved.top[moved.top.length - 1]];
  const p = moved.top[1];
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const len = Math.hypot(dx, dy);
  const along = ((p.x - a.x) * dx + (p.y - a.y) * dy) / (len * len);
  const off = ((p.x - a.x) * dy - (p.y - a.y) * dx) / len;
  near(along, 0.5, 1e-9, 'relative place');
  near(off, 0.04 * (len / 0.8), 1e-9, 'offset scales with the chord');
});

test('the validation refuses what the engine refuses', () => {
  const c = curvesFromQuad(quad());
  assert.equal(validateCurves(c), null);
  const tooFar: CurveSet = { ...c, top: [c.top[0], { x: 5, y: 0.1 }, c.top[1]] };
  assert.equal(validateCurves(tooFar), 'range');
  const same: CurveSet = { ...c, top: [c.top[0], { ...c.top[0] }, c.top[1]] };
  assert.equal(validateCurves(same), 'coincident');
  const mismatch: CurveSet = { ...c, right: [{ x: 0.91, y: 0.1 }, c.right[1]] };
  assert.equal(validateCurves(mismatch), 'corners');
  const nan: CurveSet = { ...c, top: [c.top[0], { x: Number.NaN, y: 0 }, c.top[1]] };
  assert.equal(validateCurves(nan), 'nonFinite');
  // the top edge dives through the bottom edge: the outline crosses itself
  const cross: CurveSet = { ...c, top: [c.top[0], { x: 0.5, y: 0.95 }, c.top[1]] };
  assert.equal(validateCurves(cross), 'crossing');
  const flat: CurveSet = curvesFromQuad([
    { x: 0.1, y: 0.1 },
    { x: 0.4, y: 0.4 },
    { x: 0.9, y: 0.9 },
    { x: 0.6, y: 0.6 },
  ]);
  assert.equal(validateCurves(flat), 'noArea');
});

test('handles run corner, then the edge after it; a straight edge has one hollow handle at its middle', () => {
  let c = curvesFromQuad(quad());
  let h = curveHandles(c);
  assert.deepEqual(
    h.map((x) => x.kind),
    ['corner', 'ghost', 'corner', 'ghost', 'corner', 'ghost', 'corner', 'ghost'],
  );
  const g = h[1];
  nearPt(handlePos(c, g), [0.5, 0.1], 1e-12, 'the hollow handle is at the middle');
  const moved = moveHandle(c, g, { x: 0.5, y: 0.05 })!;
  assert.deepEqual(moved.handle, { kind: 'point', e: 0, m: 0 }, 'touching it makes it a real point');
  c = moved.curves;
  h = curveHandles(c);
  assert.deepEqual(h.slice(0, 3).map((x) => x.kind), ['corner', 'point', 'corner']);
  assert.equal(edgeOf(c, 0).length, 3);
});

test('the nearest edge point puts a new point where it belongs among the others', () => {
  let c = curvesFromQuad(quad());
  c = insertPointAt(c, 0, 0, { x: 0.5, y: 0.1 })!;
  const n = nearestOnPage(c, { x: 0.75, y: 0.11 })!;
  assert.equal(n.edge, 0);
  assert.equal(n.seg, 1, 'on the second segment of the top edge');
  const r = insertAtNearest(c, n)!;
  assert.equal(r.m, 1);
  assert.equal(r.curves.top.length, 4);
  assert.ok(r.curves.top[2].x > 0.5 && r.curves.top[2].x < 0.9);
  const miss = nearestOnPage(c, { x: 0.5, y: 0.5 }, [1000, 1000])!;
  assert.ok(miss.d > 300, 'far from every edge, in the scale given');
});

test('the preview query is digits, commas and minus signs only and round-trips the numbers', () => {
  const c = insertPointAt(curvesFromQuad(quad(), 3, true), 0, 0, { x: 0.5, y: 0.0612345678 })!;
  const q = curveQuery(c);
  assert.match(q, /^t=[-0-9.,]+&r=[-0-9.,]+&b=[-0-9.,]+&l=[-0-9.,]+&q=3&m=1$/);
  const t = new URLSearchParams(q).get('t')!.split(',').map(Number);
  assert.equal(t.length, 6);
  near(t[3], 0.0612346, 1e-7, 'rounded to 7 digits');
  assert.ok(!q.includes('e'), 'no exponent form');
});
