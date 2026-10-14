// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// The matted sample of the browser mock: the same print on a wide white mat as `print_on_white_mat.jpg` that "Try sample
// images" writes in the real app (crates/engine/src/samples.rs), at half its size, and the mock's stand-in for
// `Engine::trim_to_content`. NOT a detector: the mock answers with the picture's known outline when the crop is (about)
// the paper edge, and with nothing for every other page, the way the engine answers a page without a picture in a border.
// Pure maths and no canvas, so it runs under `node --test`.

import { applyH, solveHomography } from './homography.ts';
import type { Quad } from './quad.ts';
import type { Pt, TrimProposal } from './types.ts';

export const MATTED_W = 900;
export const MATTED_H = 675;

/** The paper (the outer edge of the mat), TL TR BR BL, normalised. The same numbers as `MATTED_PAPER` in the engine. */
export const MATTED_PAPER: Quad = [
  { x: 0.27, y: 0.09 },
  { x: 0.73, y: 0.07 },
  { x: 0.74, y: 0.93 },
  { x: 0.26, y: 0.91 },
];

/** The picture inside the mat as shares of the paper: left, top, right, bottom (the engine's `MATTED_PICTURE`). */
export const MATTED_PICTURE_SHARE = { l: 0.17, t: 0.13, r: 0.83, b: 0.78 } as const;

const UNIT = [
  { x: 0, y: 0 },
  { x: 1, y: 0 },
  { x: 1, y: 1 },
  { x: 0, y: 1 },
];

/** Page coordinates (0..1 across and down the paper) to the picture, normalised. */
export function pagePoint(u: number, v: number): Pt {
  const h = solveHomography(UNIT, MATTED_PAPER.map((p) => ({ x: p.x * MATTED_W, y: p.y * MATTED_H })));
  const p = applyH(h, { x: u, y: v });
  return { x: p.x / MATTED_W, y: p.y / MATTED_H };
}

/** The picture's corners, TL TR BR BL, normalised. */
export function mattedPicture(): Quad {
  const { l, t, r, b } = MATTED_PICTURE_SHARE;
  return [pagePoint(l, t), pagePoint(r, t), pagePoint(r, b), pagePoint(l, b)];
}

/** A landscape at (u, v) of the picture (0..1): sky shading to a pale horizon, a low sun, two ranges of hills. */
export function landscape(u: number, v: number): [number, number, number] {
  const hill = (amp: number, freq: number, phase: number, base: number) =>
    base + amp * (Math.sin(u * freq + phase) * 0.6 + Math.sin(u * freq * 2.3 + phase * 1.7) * 0.4);
  const far = hill(0.035, 5.5, 0.4, 0.58);
  const near = hill(0.05, 3.2, 2.1, 0.74);
  let c: [number, number, number];
  if (v > near) c = [34, 44, 36];
  else if (v > far) {
    const k = Math.min(1, Math.max(0, (v - far) / (near - far)));
    c = [74 - 22 * k, 90 - 24 * k, 78 - 22 * k];
  } else {
    const k = Math.min(1, Math.max(0, v / far));
    c = [64 + 86 * k, 92 + 72 * k, 142 + 44 * k];
  }
  const d = Math.hypot((u - 0.68) / 0.12, (v - 0.42) / 0.16);
  const sun = Math.exp(-d * d * 2.2);
  const glow = [252, 236, 190];
  return c.map((ch, i) => ch + (glow[i] - ch) * sun * 0.85) as [number, number, number];
}

/** How far (normalised, per corner) the crop may be from the paper edge and still be trimmed to the picture. */
export const MOCK_TRIM_CAPTURE = 0.06;

const dist = (a: Pt, b: Pt): number => Math.hypot(a.x - b.x, a.y - b.y);

/**
 * The mock's `trimToContent`: the picture when `corners` (the crop's quad) are about the paper's, else `null` (a crop
 * that is already the picture, or any other page, has no picture inside to trim to).
 */
export function mockTrim(matted: boolean, corners: readonly Pt[]): TrimProposal | null {
  if (!matted || corners.length !== 4) return null;
  if (!corners.every((p, i) => dist(p, MATTED_PAPER[i]) <= MOCK_TRIM_CAPTURE)) return null;
  return { quad: mattedPicture().map((p) => ({ ...p })) as TrimProposal['quad'], confidence: 0.91, reason: 'mat' };
}
