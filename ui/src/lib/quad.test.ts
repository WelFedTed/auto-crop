// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  cloneQuad,
  insetQuad,
  moveQuad,
  normaliseAngle,
  parsePercent,
  quadsEqual,
  setCornerPercent,
  snapAngle,
  toPercent,
  turnQuarter,
  zoomAbout,
  type Quad,
} from './quad.ts';

const q: Quad = [
  { x: 0.2, y: 0.2 },
  { x: 0.8, y: 0.2 },
  { x: 0.8, y: 0.9 },
  { x: 0.2, y: 0.9 },
];

test('a corner moves alone and clamps to the image', () => {
  const m = moveQuad(q, 'corner', 0, -0.5, 0.1);
  assert.equal(m[0].x, 0);
  assert.ok(Math.abs(m[0].y - 0.3) < 1e-9);
  assert.deepEqual(m[1], q[1]);
});

test('an edge moves its two corners together', () => {
  const m = moveQuad(q, 'edge', 1, 0.1, 0); // right edge: corners 1 and 2
  assert.ok(Math.abs(m[1].x - 0.9) < 1e-9 && Math.abs(m[2].x - 0.9) < 1e-9);
  assert.equal(m[0].x, 0.2);
});

test('the grip keeps the shape and stops at the border', () => {
  const m = moveQuad(q, 'grip', 0, 0.5, -0.5);
  // right-most x is 0.8, so dx is limited to 0.2; top-most y is 0.2 so dy is limited to -0.2
  assert.ok(Math.abs(m[1].x - 1) < 1e-9 && Math.abs(m[0].y - 0) < 1e-9);
  assert.ok(Math.abs(m[1].x - m[0].x - 0.6) < 1e-9);
});

test('moveQuad does not mutate its input', () => {
  const before = cloneQuad(q);
  moveQuad(q, 'grip', 0, 0.1, 0.1);
  assert.ok(quadsEqual(q, before));
});

test('percent helpers', () => {
  assert.equal(toPercent(0.1234), '12.3');
  assert.equal(parsePercent('12,5'), 12.5);
  assert.equal(parsePercent('abc'), null);
  const s = setCornerPercent(q, 2, 'x', 150);
  assert.equal(s[2].x, 1);
  assert.equal(setCornerPercent(q, 2, 'y', -4)[2].y, 0);
});

test('angles round to 0.1, clamp to +-45 and never produce -0', () => {
  assert.equal(normaliseAngle(0.04), 0);
  assert.ok(Object.is(normaliseAngle(-0.04), 0));
  assert.equal(normaliseAngle(12.345), 12.3);
  assert.equal(normaliseAngle(99), 45);
  assert.equal(normaliseAngle(-99), -45);
});

test('ruler detent captures near 0 and releases further out; Alt disables it', () => {
  assert.deepEqual(snapAngle(0.3, false, false), { value: 0, captured: true });
  assert.deepEqual(snapAngle(0.7, false, true), { value: 0, captured: true });
  assert.deepEqual(snapAngle(0.9, false, true), { value: 0.9, captured: false });
  assert.deepEqual(snapAngle(0.3, true, false), { value: 0.3, captured: false });
});

test('quarter turns wrap in both directions', () => {
  assert.equal(turnQuarter(3, 1), 0);
  assert.equal(turnQuarter(0, -1), 3);
});

test('inset quad is the Draw crop start shape', () => {
  const i = insetQuad(0.05);
  assert.deepEqual(i[0], { x: 0.05, y: 0.05 });
  assert.deepEqual(i[2], { x: 0.95, y: 0.95 });
});

test('zoom about a point keeps that point fixed and clamps', () => {
  const v = { z: 1, px: 10, py: 20 };
  const w = zoomAbout(v, 2, 110, 120);
  // image point under the cursor before: (110-10)/1 = 100 ; after: (110 - px') / z'
  assert.ok(Math.abs((110 - w.px) / w.z - 100) < 1e-9);
  assert.ok(Math.abs((120 - w.py) / w.z - 100) < 1e-9);
  assert.equal(zoomAbout(v, 1000, 0, 0).z, 10);
  assert.equal(zoomAbout(v, 0.0001, 0, 0).z, 0.5);
});
