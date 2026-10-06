// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  areaFraction,
  boxQuad,
  convexHull,
  cutFromDrag,
  cutFromSliders,
  cutLine,
  cutPieces,
  cutProblem,
  halvesCut,
  hitCrop,
  mergedQuad,
  minAreaRect,
  pointInPolygon,
  slidersFromCut,
} from './geometry.ts';
import type { Quad } from './quad.ts';

const rect = (x0: number, y0: number, x1: number, y1: number): Quad => [
  { x: x0, y: y0 },
  { x: x1, y: y0 },
  { x: x1, y: y1 },
  { x: x0, y: y1 },
];

const near = (a: number, b: number, eps = 1e-6) => assert.ok(Math.abs(a - b) < eps, `${a} vs ${b}`);

test('point in polygon, including a rotated quad', () => {
  const q = rect(0.2, 0.2, 0.6, 0.6);
  assert.ok(pointInPolygon({ x: 0.4, y: 0.4 }, q));
  assert.ok(!pointInPolygon({ x: 0.7, y: 0.4 }, q));
  const diamond: Quad = [
    { x: 0.5, y: 0.1 },
    { x: 0.9, y: 0.5 },
    { x: 0.5, y: 0.9 },
    { x: 0.1, y: 0.5 },
  ];
  assert.ok(pointInPolygon({ x: 0.5, y: 0.5 }, diamond));
  assert.ok(!pointInPolygon({ x: 0.15, y: 0.15 }, diamond));
});

test('a tap hits the selected crop first, then the smallest crop that contains the point', () => {
  const big = { id: 1, quad: rect(0.1, 0.1, 0.9, 0.9), include: true };
  const small = { id: 2, quad: rect(0.4, 0.4, 0.5, 0.5), include: true };
  const gone = { id: 3, quad: rect(0.6, 0.6, 0.8, 0.8), include: false };
  const p = { x: 0.45, y: 0.45 };
  assert.equal(hitCrop(p, [big, small, gone], null, false), 2);
  assert.equal(hitCrop(p, [big, small, gone], 1, false), 1, 'the selected crop wins overlaps');
  assert.equal(hitCrop({ x: 0.7, y: 0.7 }, [big, small, gone], null, false), 1, 'an excluded crop is not hit');
  assert.equal(hitCrop({ x: 0.7, y: 0.7 }, [big, small, gone], null, true), 3, 'a ghost is hit when ghosts count, smallest first');
  assert.equal(hitCrop({ x: 0.95, y: 0.95 }, [big, small, gone], null, true), null);
});

test('convex hull drops interior and collinear points', () => {
  const h = convexHull([
    [0, 0],
    [4, 0],
    [4, 4],
    [0, 4],
    [2, 2],
    [2, 0],
  ]);
  assert.equal(h.length, 4);
});

test('the minimum-area rectangle of an axis-aligned union is its bounding box', () => {
  const r = minAreaRect([
    [0, 0],
    [10, 0],
    [10, 5],
    [0, 5],
    [3, 2],
  ]);
  assert.ok(r);
  const xs = r.map((p) => p[0]);
  const ys = r.map((p) => p[1]);
  near(Math.min(...xs), 0);
  near(Math.max(...xs), 10);
  near(Math.min(...ys), 0);
  near(Math.max(...ys), 5);
});

test('a tilted rectangle is recovered exactly (the rotated fit beats the bounding box)', () => {
  const th = (20 * Math.PI) / 180;
  const c = Math.cos(th);
  const s = Math.sin(th);
  const pts = [
    [0, 0],
    [100, 0],
    [100, 40],
    [0, 40],
  ].map(([x, y]) => [x * c - y * s + 50, x * s + y * c + 50] as [number, number]);
  const r = minAreaRect(pts)!;
  const side = (a: [number, number], b: [number, number]) => Math.hypot(a[0] - b[0], a[1] - b[1]);
  const sides = [side(r[0], r[1]), side(r[1], r[2])].sort((a, b) => a - b);
  near(sides[0], 40, 1e-6);
  near(sides[1], 100, 1e-6);
});

test('merging two halves of a split photo restores the whole photo (IoU about 1)', () => {
  const left = rect(0.1, 0.1, 0.3, 0.5);
  const right = rect(0.3, 0.1, 0.5, 0.5);
  const m = mergedQuad([left, right], 2000, 1500)!;
  near(m[0].x, 0.1, 1e-6);
  near(m[0].y, 0.1, 1e-6);
  near(m[2].x, 0.5, 1e-6);
  near(m[2].y, 0.5, 1e-6);
  near(areaFraction(m, 2000, 1500), 0.4 * 0.4, 1e-9);
});

test('cut pieces follow the engine: vertical ends on the top and bottom edges', () => {
  const q = rect(0.2, 0.2, 0.8, 0.6);
  const [l, r] = cutPieces(q, halvesCut('vertical'))!;
  near(l[1].x, 0.5);
  near(l[1].y, 0.2);
  near(l[2].x, 0.5);
  near(l[2].y, 0.6);
  assert.deepEqual(r[0], l[1]);
  assert.deepEqual(r[3], l[2]);
  const [t, b] = cutPieces(q, halvesCut('horizontal'))!;
  near(t[2].y, 0.4);
  near(t[3].y, 0.4);
  assert.deepEqual(b[0], t[3]);
  assert.equal(cutPieces(q, { axis: 'vertical', t0: 1.5, t1: 0.5 }), null);
});

test('a cut that leaves a piece under 2% of the scan is refused', () => {
  const q = rect(0.2, 0.2, 0.5, 0.5);
  assert.equal(cutProblem(q, halvesCut('vertical'), 1000, 1000), null);
  assert.equal(cutProblem(q, { axis: 'vertical', t0: 0.02, t1: 0.02 }, 1000, 1000), 'small');
  assert.equal(cutProblem(q, { axis: 'vertical', t0: 2, t1: 0.5 }, 1000, 1000), 'bad');
});

test('sliders and cuts round-trip and clamp', () => {
  const c = cutFromSliders('vertical', 0.5, 0.2);
  near(c.t0, 0.6);
  near(c.t1, 0.4);
  const s = slidersFromCut(c);
  near(s.pos, 0.5);
  near(s.tilt, 0.2);
  const edge = cutFromSliders('horizontal', 0.98, 0.2);
  assert.ok(edge.t0 <= 1 && edge.t1 >= 0);
});

test('a drag across a crop makes a cut on the nearer axis and snaps near-straight lines', () => {
  const q = rect(0.2, 0.2, 0.8, 0.6);
  const v = cutFromDrag(q, { x: 0.5, y: 0.25 }, { x: 0.5, y: 0.55 })!;
  assert.equal(v.axis, 'vertical');
  near(v.t0, 0.5);
  near(v.t1, 0.5);
  const h = cutFromDrag(q, { x: 0.25, y: 0.4 }, { x: 0.75, y: 0.4 })!;
  assert.equal(h.axis, 'horizontal');
  near(h.t0, 0.5);
  const tilted = cutFromDrag(q, { x: 0.4, y: 0.2 }, { x: 0.6, y: 0.6 })!;
  assert.equal(tilted.axis, 'vertical');
  assert.ok(tilted.t0 < tilted.t1, 'the line leans right as it goes down');
  assert.equal(cutFromDrag(q, { x: 0.5, y: 0.4 }, { x: 0.5, y: 0.41 }), null, 'too short');
  const [a, b] = cutLine(q, v);
  near(a.x, 0.5);
  near(b.x, 0.5);
});

test('a drawn box is a normalised rectangle and tiny boxes are ignored', () => {
  const q = boxQuad({ x: 0.6, y: 0.7 }, { x: 0.2, y: 0.3 })!;
  near(q[0].x, 0.2);
  near(q[2].y, 0.7);
  assert.equal(boxQuad({ x: 0.5, y: 0.5 }, { x: 0.51, y: 0.51 }), null);
  const clipped = boxQuad({ x: -0.5, y: 0.1 }, { x: 0.5, y: 1.9 })!;
  near(clipped[0].x, 0);
  near(clipped[2].y, 1);
});
