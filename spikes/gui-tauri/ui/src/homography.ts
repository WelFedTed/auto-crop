// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// Four-point homography for the drag preview (PLAN 2.5 / ROADMAP M3.27). Spike code: the shipping
// version must agree with the Rust warp within 0.05 px on shared vectors.

export type Pt = { x: number; y: number };

/** Row-major 3x3 with h[8] fixed to 1, so `h` has 9 entries. */
export type H = [number, number, number, number, number, number, number, number, number];

/** Solves the homography taking each `src[i]` to `dst[i]` (Gaussian elimination, partial pivoting). */
export function solveHomography(src: Pt[], dst: Pt[]): H {
  const a: number[][] = [];
  for (let i = 0; i < 4; i++) {
    const { x, y } = src[i];
    const { x: u, y: v } = dst[i];
    a.push([x, y, 1, 0, 0, 0, -u * x, -u * y, u]);
    a.push([0, 0, 0, x, y, 1, -v * x, -v * y, v]);
  }
  const n = 8;
  for (let col = 0; col < n; col++) {
    let pivot = col;
    for (let r = col + 1; r < n; r++) if (Math.abs(a[r][col]) > Math.abs(a[pivot][col])) pivot = r;
    if (Math.abs(a[pivot][col]) < 1e-12) throw new Error('degenerate quad');
    [a[col], a[pivot]] = [a[pivot], a[col]];
    for (let r = 0; r < n; r++) {
      if (r === col) continue;
      const f = a[r][col] / a[col][col];
      for (let c = col; c <= n; c++) a[r][c] -= f * a[col][c];
    }
  }
  const h = a.map((row, i) => row[n] / row[i]);
  return [h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7], 1];
}

export function applyH(h: H, p: Pt): Pt {
  const w = h[6] * p.x + h[7] * p.y + h[8];
  return { x: (h[0] * p.x + h[1] * p.y + h[2]) / w, y: (h[3] * p.x + h[4] * p.y + h[5]) / w };
}

/** CSS `matrix3d()` (column-major) for use with `transform-origin: 0 0`. */
export function toMatrix3d(h: H): string {
  const m = [h[0], h[3], 0, h[6], h[1], h[4], 0, h[7], 0, 0, 1, 0, h[2], h[5], 0, h[8]];
  return `matrix3d(${m.map((v) => (Number.isFinite(v) ? v : 0).toFixed(8)).join(',')})`;
}

/** Quad corners in order TL, TR, BR, BL. Returns the homography mapping the quad onto 0..w x 0..h. */
export function quadToRect(quad: Pt[], w: number, h: number): H {
  const rect: Pt[] = [
    { x: 0, y: 0 },
    { x: w, y: 0 },
    { x: w, y: h },
    { x: 0, y: h },
  ];
  return solveHomography(quad, rect);
}

export function quadSize(q: Pt[]): { w: number; h: number } {
  const d = (a: Pt, b: Pt) => Math.hypot(a.x - b.x, a.y - b.y);
  return { w: (d(q[0], q[1]) + d(q[3], q[2])) / 2, h: (d(q[0], q[3]) + d(q[1], q[2])) / 2 };
}
