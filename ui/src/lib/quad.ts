// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// Pure geometry for the crop editor. All quads are normalised (0..1 of the displayed source image) and
// ordered TL, TR, BR, BL, exactly as in the contract (`Edit.quad`).

import type { Edit, Pt } from './types.ts';

export type Quad = [Pt, Pt, Pt, Pt];
export type HandleKind = 'corner' | 'edge' | 'grip';

export const clamp = (v: number, lo: number, hi: number): number => Math.min(hi, Math.max(lo, v));

export function cloneQuad(q: Quad): Quad {
  return q.map((p) => ({ x: p.x, y: p.y })) as Quad;
}

export function cloneEdit(e: Edit): Edit {
  return { quad: cloneQuad(e.quad), quarterTurns: e.quarterTurns, fineDeg: e.fineDeg };
}

export function mid(a: Pt, b: Pt): Pt {
  return { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 };
}

export function centroid(q: Quad): Pt {
  return {
    x: (q[0].x + q[1].x + q[2].x + q[3].x) / 4,
    y: (q[0].y + q[1].y + q[2].y + q[3].y) / 4,
  };
}

/** The indices a handle moves: a corner moves one point, an edge two, the grip all four. */
export function movedIndices(kind: HandleKind, idx: number): number[] {
  if (kind === 'corner') return [idx];
  if (kind === 'edge') return [idx, (idx + 1) % 4];
  return [0, 1, 2, 3];
}

/**
 * Moves a handle by (dx, dy), in normalised units, from the quad `start`. The delta is clamped as a whole,
 * so the shape never distorts against the image border and the grip keeps the quad's shape.
 */
export function moveQuad(start: Quad, kind: HandleKind, idx: number, dx: number, dy: number): Quad {
  const idxs = movedIndices(kind, idx);
  let ddx = dx;
  let ddy = dy;
  for (const i of idxs) {
    ddx = clamp(ddx, -start[i].x, 1 - start[i].x);
    ddy = clamp(ddy, -start[i].y, 1 - start[i].y);
  }
  return start.map((p, i) => (idxs.includes(i) ? { x: p.x + ddx, y: p.y + ddy } : { x: p.x, y: p.y })) as Quad;
}

/** Sets one coordinate of one corner from a percent value (the WCAG 2.5.7 number fields). */
export function setCornerPercent(start: Quad, corner: number, axis: 'x' | 'y', percent: number): Quad {
  const q = cloneQuad(start);
  q[corner][axis] = clamp(percent, 0, 100) / 100;
  return q;
}

export function toPercent(v: number): string {
  return (v * 100).toFixed(1);
}

/** Parses a percent string; null for anything that is not a finite number. */
export function parsePercent(text: string): number | null {
  const v = Number.parseFloat(text.replace(',', '.'));
  return Number.isFinite(v) ? v : null;
}

export function quadsEqual(a: Quad, b: Quad, eps = 1e-6): boolean {
  return a.every((p, i) => Math.abs(p.x - b[i].x) < eps && Math.abs(p.y - b[i].y) < eps);
}

/** Rounds to 0.1 degree and clamps to the contract range. */
export function normaliseAngle(deg: number): number {
  return clamp(Math.round(deg * 10) / 10, -45, 45) + 0; // + 0 turns -0 into 0
}

/** Ruler detent: values close to 0 stick to 0 unless `disable` (Alt held). */
export function snapAngle(deg: number, disable: boolean, captured: boolean): { value: number; captured: boolean } {
  if (disable) return { value: deg, captured: false };
  if (captured) return Math.abs(deg) > 0.8 ? { value: deg, captured: false } : { value: 0, captured: true };
  if (Math.abs(deg) < 0.5) return { value: 0, captured: true };
  return { value: deg, captured: false };
}

export function turnQuarter(q: number, dir: 1 | -1): number {
  return (((q + dir) % 4) + 4) % 4;
}

/** Shows a quad inset by `fraction` of each side, the "Draw crop" start shape. */
export function insetQuad(fraction: number): Quad {
  return [
    { x: fraction, y: fraction },
    { x: 1 - fraction, y: fraction },
    { x: 1 - fraction, y: 1 - fraction },
    { x: fraction, y: 1 - fraction },
  ];
}

/** Zoom about a screen point: keeps the image point under (cx, cy) fixed. */
export function zoomAbout(
  view: { z: number; px: number; py: number },
  factor: number,
  cx: number,
  cy: number,
  min = 0.5,
  max = 10,
): { z: number; px: number; py: number } {
  const z = clamp(view.z * factor, min, max);
  const k = z / view.z;
  return { z, px: cx - (cx - view.px) * k, py: cy - (cy - view.py) * k };
}
