// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { applyH, quadSize, quadToRect, solveHomography, toMatrix3d, type Pt } from './homography.ts';

const quad: Pt[] = [
  { x: 350, y: 76 },
  { x: 557, y: 92 },
  { x: 520, y: 494 },
  { x: 315, y: 475 },
];

test('each quad corner lands on the matching rectangle corner', () => {
  const h = quadToRect(quad, 200, 400);
  const want: Pt[] = [
    { x: 0, y: 0 },
    { x: 200, y: 0 },
    { x: 200, y: 400 },
    { x: 0, y: 400 },
  ];
  quad.forEach((p, i) => {
    const got = applyH(h, p);
    assert.ok(Math.abs(got.x - want[i].x) < 1e-6 && Math.abs(got.y - want[i].y) < 1e-6, `corner ${i}`);
  });
});

test('an axis-aligned rectangle maps to a pure scale', () => {
  const rect: Pt[] = [
    { x: 10, y: 20 },
    { x: 110, y: 20 },
    { x: 110, y: 220 },
    { x: 10, y: 220 },
  ];
  const h = quadToRect(rect, 50, 100);
  const mid = applyH(h, { x: 60, y: 120 });
  assert.ok(Math.abs(mid.x - 25) < 1e-9 && Math.abs(mid.y - 50) < 1e-9);
});

test('a degenerate quad is rejected', () => {
  const flat: Pt[] = [0, 1, 2, 3].map((i) => ({ x: i, y: 0 }));
  assert.throws(() => solveHomography(flat, flat));
});

test('matrix3d is column-major with the projective terms in the right slots', () => {
  const h = quadToRect(quad, 200, 400);
  const nums = toMatrix3d(h).slice('matrix3d('.length, -1).split(',').map(Number);
  assert.equal(nums.length, 16);
  assert.ok(Math.abs(nums[3] - h[6]) < 1e-7 && Math.abs(nums[7] - h[7]) < 1e-7);
  assert.ok(Math.abs(nums[12] - h[2]) < 1e-5 && Math.abs(nums[13] - h[5]) < 1e-5);
});

test('quadSize averages opposite sides', () => {
  const s = quadSize(quad);
  assert.ok(s.w > 190 && s.w < 220 && s.h > 390 && s.h < 420);
});
