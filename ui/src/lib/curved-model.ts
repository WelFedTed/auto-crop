// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// The curved sample page of the browser mock: the same analytic model as `receipt_curved.jpg` that "Try sample
// images" writes in the real app (crates/engine/src/samples.rs), at half its size. A flat receipt is bent by two
// terms, so its four edges are known exactly:
//
//   x = X + s W + BULGE 4t(1-t) (2s-1)      the sides bulge outwards
//   y = Y + t H + SAG   4s(1-s)             top and bottom dip in the middle
//
// with `s` across and `t` down the flat page (0..1). Pure maths and no canvas, so it runs under `node --test`.

import type { Pt } from './types.ts';

export const CURVED_W = 900;
export const CURVED_H = 675;
export const PAGE_X = 280;
export const PAGE_Y = 75;
export const PAGE_W = 340;
export const PAGE_H = 500;
export const SAG = 22;
export const BULGE = 11;

/** Where flat-page coordinates (s, t) land on the picture, in pixels. */
export function curvedPagePoint(s: number, t: number): { x: number; y: number } {
  return {
    x: PAGE_X + s * PAGE_W + BULGE * 4 * t * (1 - t) * (2 * s - 1),
    y: PAGE_Y + t * PAGE_H + SAG * 4 * s * (1 - s),
  };
}

/** The flat-page coordinates of a picture pixel (the inverse of `curvedPagePoint`, by fixed-point iteration). */
export function curvedPageCoords(x: number, y: number): { s: number; t: number } {
  let s = (x - PAGE_X) / PAGE_W;
  let t = (y - PAGE_Y) / PAGE_H;
  for (let i = 0; i < 14; i++) {
    t = (y - PAGE_Y - SAG * 4 * s * (1 - s)) / PAGE_H;
    s = (x - PAGE_X - BULGE * 4 * t * (1 - t) * (2 * s - 1)) / PAGE_W;
  }
  return { s, t };
}

const PAPER: [number, number, number] = [247, 244, 236];
const INK: [number, number, number] = [60, 64, 76];

const hash = (k: number): number => Math.abs(((Math.sin(k * 12.9898) * 43758.5453) % 1 + 1) % 1);

/** What is printed on the flat page at (s, t): cream paper, a shop name, rows of text and a barcode. */
export function receiptInk(s: number, t: number): [number, number, number] {
  const rows = 26;
  const r = Math.floor(t * rows);
  const along = (t * rows) % 1;
  if (t > 0.04 && t < 0.075 && s >= 0.2 && s < 0.8) return INK;
  if (t > 0.1 && t < 0.9 && along < 0.46 && s > 0.1 && s < 0.1 + 0.8 * (0.45 + 0.5 * hash(r))) return INK;
  if (t > 0.92 && t < 0.97 && s >= 0.15 && s < 0.85 && Math.floor(s * 90) % 3 !== 1) return INK;
  return PAPER;
}

/**
 * The four edges as a person would mark them, `n` points each at equal steps of the page parameter, normalised to
 * the picture: top (TL to TR), right (TR to BR), bottom (BR to BL), left (BL to TL).
 */
export function curvedEdges(n: number): [Pt[], Pt[], Pt[], Pt[]] {
  const at = (s: number, t: number): Pt => {
    const p = curvedPagePoint(s, t);
    return { x: p.x / CURVED_W, y: p.y / CURVED_H };
  };
  const steps = (f: (u: number) => Pt): Pt[] => Array.from({ length: n }, (_, i) => f(i / (n - 1)));
  return [steps((u) => at(u, 0)), steps((u) => at(1, u)), steps((u) => at(1 - u, 1)), steps((u) => at(0, 1 - u))];
}

/**
 * The quad a plain detector proposes for this page: the four true corners pulled in a little (it follows the dip of
 * the top and bottom edges), so the person still has something to adjust.
 */
export function curvedDetectedQuad(): [Pt, Pt, Pt, Pt] {
  const corners = [curvedPagePoint(0, 0), curvedPagePoint(1, 0), curvedPagePoint(1, 1), curvedPagePoint(0, 1)];
  const cx = corners.reduce((a, p) => a + p.x, 0) / 4;
  const cy = corners.reduce((a, p) => a + p.y, 0) / 4;
  return corners.map((p) => ({ x: (p.x + (cx - p.x) * 0.03) / CURVED_W, y: (p.y + (cy - p.y) * 0.03) / CURVED_H })) as [Pt, Pt, Pt, Pt];
}
