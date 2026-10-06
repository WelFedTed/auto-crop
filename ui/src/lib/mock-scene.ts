// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// Canvas drawing for the browser mock: synthetic receipt and document photos (desk, rotated and
// perspective-distorted paper, text-like bars), and a software perspective warp used as the mock's "engine"
// result. Browser only (needs canvas); the real app never loads this.

import { applyH, solveHomography, type H, type Pt as PxPt } from './homography.ts';
import { insetQuad, type Quad } from './quad.ts';
import { mulberry32 } from './rng.ts';
import type { Edit, Side } from './types.ts';

export interface SceneSpec {
  kind: 'receipt' | 'document';
  /** Source proxy size in pixels (EXIF-oriented). */
  w: number;
  h: number;
  /** Ground-truth paper corners, normalised, TL TR BR BL (may lie outside 0..1 when the paper is cut off). */
  truth: Quad;
  /** Desk gradient colours and paper colour. */
  bg: [string, string];
  paper: string;
  /** Paper nearly invisible against the desk: edges are hard to see. */
  lowContrast: boolean;
  /** A scene with no paper edge to find at all. */
  invisible: boolean;
  seed: number;
}

export { mulberry32 };

function makeCanvas(w: number, h: number): HTMLCanvasElement {
  const c = document.createElement('canvas');
  c.width = Math.max(1, Math.round(w));
  c.height = Math.max(1, Math.round(h));
  return c;
}

function fillPoly(g: CanvasRenderingContext2D, pts: PxPt[], style: string | CanvasGradient): void {
  g.beginPath();
  g.moveTo(pts[0].x, pts[0].y);
  for (let i = 1; i < pts.length; i++) g.lineTo(pts[i].x, pts[i].y);
  g.closePath();
  g.fillStyle = style;
  g.fill();
}

/** Builds a plausible scene spec: where the paper sits, how it is tilted and bent by perspective. */
export function makeSpec(
  rnd: () => number,
  kind: 'receipt' | 'document',
  opts: { aspect?: number; lowContrast?: boolean; invisible?: boolean; cutOffBottom?: boolean; seed: number },
): SceneSpec {
  const portrait = kind === 'receipt' || rnd() < 0.7;
  const w = portrait ? 900 : 1200;
  const h = portrait ? 1200 : 900;
  const aspect = opts.aspect ?? (kind === 'receipt' ? 0.36 + rnd() * 0.12 : 0.707);
  const ph = (kind === 'receipt' ? 0.8 : 0.84) * h;
  let pw = ph * aspect;
  const maxW = w * 0.82;
  const k = pw > maxW ? maxW / pw : 1;
  pw *= k;
  const phh = ph * k;
  const theta = ((rnd() - 0.5) * 2 * (kind === 'receipt' ? 9 : 7) * Math.PI) / 180;
  const cx = w * (0.5 + (rnd() - 0.5) * 0.05);
  let cy = h * (0.5 + (rnd() - 0.5) * 0.04);
  if (opts.cutOffBottom) cy += phh * 0.17;
  const jitter = Math.min(w, h) * 0.018;
  const corners: [number, number][] = [
    [-pw / 2, -phh / 2],
    [pw / 2, -phh / 2],
    [pw / 2, phh / 2],
    [-pw / 2, phh / 2],
  ];
  const truth = corners.map(([x, y]) => {
    const rx = x * Math.cos(theta) - y * Math.sin(theta);
    const ry = x * Math.sin(theta) + y * Math.cos(theta);
    return { x: (cx + rx + (rnd() - 0.5) * 2 * jitter) / w, y: (cy + ry + (rnd() - 0.5) * 2 * jitter) / h };
  }) as Quad;
  const warm = rnd() < 0.5;
  const bgs: [string, string][] = warm
    ? [
        ['#8a7968', '#5e5045'],
        ['#7b6a58', '#4e4238'],
      ]
    : [
        ['#6f7a86', '#444c56'],
        ['#7e8a78', '#4d574a'],
      ];
  let bg = bgs[Math.floor(rnd() * bgs.length)];
  let paper = kind === 'receipt' ? '#f6f4ee' : '#f3f1ea';
  if (opts.lowContrast) {
    bg = ['#cfcdc6', '#bdbab2'];
    paper = '#efede7';
  }
  if (opts.invisible) {
    bg = ['#e9e6de', '#dedbd2'];
    paper = '#e9e6de';
  }
  return {
    kind,
    w,
    h,
    truth,
    bg,
    paper,
    lowContrast: !!opts.lowContrast,
    invisible: !!opts.invisible,
    seed: opts.seed,
  };
}

/** Draws the full scene (desk plus paper with content) and returns the source-proxy canvas. */
export function drawScene(spec: SceneSpec): HTMLCanvasElement {
  const c = makeCanvas(spec.w, spec.h);
  const g = c.getContext('2d', { willReadFrequently: true })!;
  const rnd = mulberry32(spec.seed * 7919 + 13);

  const grad = g.createLinearGradient(0, 0, spec.w, spec.h);
  grad.addColorStop(0, spec.bg[0]);
  grad.addColorStop(1, spec.bg[1]);
  g.fillStyle = grad;
  g.fillRect(0, 0, spec.w, spec.h);
  // wood-grain streaks
  for (let i = 0; i < 70; i++) {
    g.fillStyle = rnd() < 0.5 ? 'rgba(0,0,0,0.045)' : 'rgba(255,255,255,0.035)';
    g.fillRect(0, rnd() * spec.h, spec.w, 1 + rnd() * 3);
  }

  const quad: PxPt[] = spec.truth.map((p) => ({ x: p.x * spec.w, y: p.y * spec.h }));
  g.save();
  if (!spec.invisible) {
    g.shadowColor = spec.lowContrast ? 'rgba(0,0,0,0.12)' : 'rgba(0,0,0,0.38)';
    g.shadowBlur = 20;
    g.shadowOffsetX = 5;
    g.shadowOffsetY = 11;
  }
  fillPoly(g, quad, spec.paper);
  g.restore();

  // Paper content in local coordinates (pw x 100), mapped through the homography to the quad.
  const aspect =
    Math.hypot(quad[1].x - quad[0].x, quad[1].y - quad[0].y) / Math.max(1, Math.hypot(quad[3].x - quad[0].x, quad[3].y - quad[0].y));
  const lw = 100 * aspect;
  const lh = 100;
  const H = solveHomography(
    [
      { x: 0, y: 0 },
      { x: lw, y: 0 },
      { x: lw, y: lh },
      { x: 0, y: lh },
    ],
    quad,
  );
  const bar = (x: number, y: number, w: number, hh: number, style: string) => {
    fillPoly(
      g,
      [
        applyH(H, { x, y }),
        applyH(H, { x: x + w, y }),
        applyH(H, { x: x + w, y: y + hh }),
        applyH(H, { x, y: y + hh }),
      ],
      style,
    );
  };
  const ink = 'rgba(38,42,52,0.78)';
  const soft = 'rgba(38,42,52,0.42)';
  const m = lw * 0.09;
  const inner = lw - 2 * m;
  if (spec.kind === 'receipt') {
    bar(lw / 2 - inner * 0.28, 7, inner * 0.56, 4.2, ink);
    bar(lw / 2 - inner * 0.36, 13.5, inner * 0.72, 1.2, soft);
    bar(lw / 2 - inner * 0.3, 16.5, inner * 0.6, 1.2, soft);
    let y = 23;
    for (let i = 0; i < 17 && y < 80; i++) {
      bar(m, y, inner * (0.28 + rnd() * 0.3), 1.35, ink);
      bar(lw - m - inner * 0.17, y, inner * 0.17, 1.35, ink);
      y += 3.6;
    }
    bar(m, y + 1, inner, 0.5, soft);
    bar(m, y + 4, inner * 0.3, 2.1, ink);
    bar(lw - m - inner * 0.24, y + 4, inner * 0.24, 2.1, ink);
    // barcode
    let bx = m + inner * 0.12;
    const by = Math.min(92, y + 11);
    while (bx < lw - m - inner * 0.12) {
      const bw = 0.5 + rnd() * 1.3;
      bar(bx, by, bw, 5.5, ink);
      bx += bw + 0.5 + rnd() * 1.3;
    }
  } else {
    bar(m, 8, inner * 0.55, 3.8, ink);
    bar(m, 14, inner * 0.3, 1.2, soft);
    let y = 22;
    for (let i = 0; i < 20 && y < 78; i++) {
      const para = i % 6 === 5;
      bar(m, y, para ? inner * (0.2 + rnd() * 0.5) : inner, 1.1, ink);
      y += para ? 5.2 : 2.9;
    }
    // small table
    for (let r = 0; r < 3; r++) {
      bar(m, y + 2 + r * 3.2, inner * 0.9, 0.5, soft);
    }
    bar(lw - m - inner * 0.36, 92, inner * 0.36, 0.7, ink);
  }
  return c;
}

/** Detected quad for a scene, with controlled noise. `weak` pushes one side outward (the WEAK_EDGE case). */
export function detectQuad(spec: SceneSpec, rnd: () => number, noise: number, weak?: Side): Quad {
  const clamp01 = (v: number) => Math.min(1, Math.max(0, v));
  const q = spec.truth.map((p) => ({
    x: clamp01(p.x + (rnd() - 0.5) * 2 * noise),
    y: clamp01(p.y + (rnd() - 0.5) * 2 * noise),
  })) as Quad;
  if (weak) {
    const push = 0.035 + rnd() * 0.02;
    const idx: Record<Side, [number, number]> = { top: [0, 1], right: [1, 2], bottom: [2, 3], left: [3, 0] };
    for (const i of idx[weak]) {
      if (weak === 'top') q[i].y = clamp01(q[i].y - push);
      if (weak === 'bottom') q[i].y = clamp01(q[i].y + push);
      if (weak === 'left') q[i].x = clamp01(q[i].x - push);
      if (weak === 'right') q[i].x = clamp01(q[i].x + push);
    }
  }
  return q;
}

export function skinnyQuad(): Quad {
  return [
    { x: 0.42, y: 0.1 },
    { x: 0.56, y: 0.12 },
    { x: 0.55, y: 0.5 },
    { x: 0.41, y: 0.46 },
  ];
}

export { insetQuad };

/** Output pixel size of a warped quad on the source canvas, scaled so the long edge is at most `maxLong`. */
export function quadOutputSize(src: { width: number; height: number }, q: Quad, maxLong: number): { w: number; h: number } {
  const d = (a: number, b: number) => Math.hypot((q[a].x - q[b].x) * src.width, (q[a].y - q[b].y) * src.height);
  let w = (d(0, 1) + d(3, 2)) / 2;
  let h = (d(0, 3) + d(1, 2)) / 2;
  const long = Math.max(w, h);
  if (long > maxLong) {
    w = (w * maxLong) / long;
    h = (h * maxLong) / long;
  }
  return { w: Math.max(8, Math.round(w)), h: Math.max(8, Math.round(h)) };
}

/** Software perspective warp: the quad of `src` onto an outW x outH rectangle, bilinear sampled. */
export function warpQuad(src: HTMLCanvasElement, q: Quad, outW: number, outH: number): HTMLCanvasElement {
  const sctx = src.getContext('2d', { willReadFrequently: true })!;
  const sd = sctx.getImageData(0, 0, src.width, src.height);
  const sw = src.width;
  const sh = src.height;
  const quadPx: PxPt[] = q.map((p) => ({ x: p.x * sw, y: p.y * sh }));
  let H: H;
  try {
    H = solveHomography(
      [
        { x: 0, y: 0 },
        { x: outW, y: 0 },
        { x: outW, y: outH },
        { x: 0, y: outH },
      ],
      quadPx,
    );
  } catch {
    H = [sw / outW, 0, 0, 0, sh / outH, 0, 0, 0, 1];
  }
  const out = makeCanvas(outW, outH);
  const octx = out.getContext('2d')!;
  const od = octx.createImageData(outW, outH);
  const s = sd.data;
  const o = od.data;
  for (let y = 0; y < outH; y++) {
    for (let x = 0; x < outW; x++) {
      const px = x + 0.5;
      const py = y + 0.5;
      const wgt = H[6] * px + H[7] * py + H[8];
      let sx = (H[0] * px + H[1] * py + H[2]) / wgt - 0.5;
      let sy = (H[3] * px + H[4] * py + H[5]) / wgt - 0.5;
      sx = Math.min(sw - 1.001, Math.max(0, sx));
      sy = Math.min(sh - 1.001, Math.max(0, sy));
      const x0 = sx | 0;
      const y0 = sy | 0;
      const fx = sx - x0;
      const fy = sy - y0;
      const i00 = (y0 * sw + x0) * 4;
      const i10 = i00 + 4;
      const i01 = i00 + sw * 4;
      const i11 = i01 + 4;
      const oi = (y * outW + x) * 4;
      for (let ch = 0; ch < 3; ch++) {
        const top = s[i00 + ch] * (1 - fx) + s[i10 + ch] * fx;
        const bot = s[i01 + ch] * (1 - fx) + s[i11 + ch] * fx;
        o[oi + ch] = top * (1 - fy) + bot * fy;
      }
      o[oi + 3] = 255;
    }
  }
  octx.putImageData(od, 0, 0);
  return out;
}

/** The mock "engine" result: warp, then rotate by quarter turns plus fine degrees on a white background. */
export function renderResult(src: HTMLCanvasElement, edit: Edit, maxLong = 760): HTMLCanvasElement {
  const { w, h } = quadOutputSize(src, edit.quad, maxLong);
  const warped = warpQuad(src, edit.quad, w, h);
  const theta = ((edit.quarterTurns * 90 + edit.fineDeg) * Math.PI) / 180;
  if (Math.abs(theta) < 1e-9) return warped;
  const cos = Math.abs(Math.cos(theta));
  const sin = Math.abs(Math.sin(theta));
  const bw = Math.ceil(w * cos + h * sin);
  const bh = Math.ceil(w * sin + h * cos);
  const out = makeCanvas(bw, bh);
  const g = out.getContext('2d')!;
  g.fillStyle = '#ffffff';
  g.fillRect(0, 0, bw, bh);
  g.translate(bw / 2, bh / 2);
  g.rotate(theta);
  g.drawImage(warped, -w / 2, -h / 2);
  return out;
}

export function scaleToLongEdge(src: HTMLCanvasElement, longEdge: number): HTMLCanvasElement {
  const k = Math.min(1, longEdge / Math.max(src.width, src.height));
  const out = makeCanvas(src.width * k, src.height * k);
  const g = out.getContext('2d')!;
  g.imageSmoothingQuality = 'high';
  g.drawImage(src, 0, 0, out.width, out.height);
  return out;
}

/**
 * Encodes a canvas to a blob: URL. Uses the synchronous toDataURL path on purpose: canvas.toBlob is
 * scheduled asynchronously and can take a second per image in a background or throttled browser tab.
 */
export function canvasToBlobUrl(c: HTMLCanvasElement, quality = 0.86): Promise<string> {
  const data = c.toDataURL('image/jpeg', quality);
  const bin = atob(data.slice(data.indexOf(',') + 1));
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return Promise.resolve(URL.createObjectURL(new Blob([bytes], { type: 'image/jpeg' })));
}

/** A source canvas from a user-chosen file: EXIF-oriented, long edge at most `maxLong`. */
export async function canvasFromFile(file: File, maxLong = 1600): Promise<{ canvas: HTMLCanvasElement; width: number; height: number }> {
  const bmp = await createImageBitmap(file);
  const k = Math.min(1, maxLong / Math.max(bmp.width, bmp.height));
  const c = makeCanvas(bmp.width * k, bmp.height * k);
  const g = c.getContext('2d', { willReadFrequently: true })!;
  g.imageSmoothingQuality = 'high';
  g.drawImage(bmp, 0, 0, c.width, c.height);
  const width = bmp.width;
  const height = bmp.height;
  bmp.close();
  return { canvas: c, width, height };
}

export function defaultEdit(q: Quad): Edit {
  return { quad: q, quarterTurns: 0, fineDeg: 0 };
}

export function failedPlaceholderEdit(): Edit {
  return defaultEdit(insetQuad(0.05));
}

// ------------------------------------------------------------------------------------------ bed scans
// Several items on a scanner bed (the multi-item mock scenes): photos and receipts on a light bed, some touching,
// a speck of dust the detector rejects. Each item is drawn through the same homography as the single scenes.

export interface BedItem {
  quad: Quad;
  kind: 'photo' | 'receipt';
  hue: number;
  /** Items with the same group touch: the detector finds one cluster crop for them and flags it. */
  group?: number;
  score: number;
}

export interface BedSpec {
  w: number;
  h: number;
  bg: string;
  items: BedItem[];
  /** Specks of dust the detector looks at and rejects (excluded candidates). */
  dust: Quad[];
  seed: number;
}

/** A rotated rectangle given in pixels around a normalised centre, as a normalised quad TL TR BR BL. */
export function rotatedBox(W: number, H: number, cx: number, cy: number, wpx: number, hpx: number, deg: number): Quad {
  const th = (deg * Math.PI) / 180;
  const c = Math.cos(th);
  const s = Math.sin(th);
  const pts: [number, number][] = [
    [-wpx / 2, -hpx / 2],
    [wpx / 2, -hpx / 2],
    [wpx / 2, hpx / 2],
    [-wpx / 2, hpx / 2],
  ];
  return pts.map(([x, y]) => ({ x: (cx * W + x * c - y * s) / W, y: (cy * H + x * s + y * c) / H })) as Quad;
}

function circlePts(cx: number, cy: number, r: number, n = 18): PxPt[] {
  return Array.from({ length: n }, (_, i) => ({ x: cx + r * Math.cos((i / n) * 2 * Math.PI), y: cy + r * Math.sin((i / n) * 2 * Math.PI) }));
}

export function drawBed(spec: BedSpec): HTMLCanvasElement {
  const c = makeCanvas(spec.w, spec.h);
  const g = c.getContext('2d', { willReadFrequently: true })!;
  const rnd = mulberry32(spec.seed * 31 + 7);
  g.fillStyle = spec.bg;
  g.fillRect(0, 0, spec.w, spec.h);
  for (let i = 0; i < 40; i++) {
    g.fillStyle = rnd() < 0.5 ? 'rgba(0,0,0,0.018)' : 'rgba(255,255,255,0.05)';
    g.fillRect(0, rnd() * spec.h, spec.w, 1 + rnd() * 2);
  }
  spec.items.forEach((it, idx) => {
    const quad: PxPt[] = it.quad.map((p) => ({ x: p.x * spec.w, y: p.y * spec.h }));
    g.save();
    g.shadowColor = 'rgba(0,0,0,0.3)';
    g.shadowBlur = 8;
    g.shadowOffsetX = 2;
    g.shadowOffsetY = 4;
    fillPoly(g, quad, it.kind === 'receipt' ? '#f7f5ef' : '#ffffff');
    g.restore();
    const lw = 100 * (Math.hypot(quad[1].x - quad[0].x, quad[1].y - quad[0].y) / Math.max(1, Math.hypot(quad[3].x - quad[0].x, quad[3].y - quad[0].y)));
    const lh = 100;
    const H = solveHomography(
      [
        { x: 0, y: 0 },
        { x: lw, y: 0 },
        { x: lw, y: lh },
        { x: 0, y: lh },
      ],
      quad,
    );
    const poly = (pts: PxPt[], style: string) =>
      fillPoly(
        g,
        pts.map((p) => applyH(H, p)),
        style,
      );
    const rect = (x: number, y: number, w: number, h: number, style: string) =>
      poly(
        [
          { x, y },
          { x: x + w, y },
          { x: x + w, y: y + h },
          { x, y: y + h },
        ],
        style,
      );
    if (it.kind === 'photo') {
      const m = 3.5;
      rect(m, m, lw - 2 * m, lh - 2 * m, `hsl(${it.hue} 55% 72%)`);
      rect(m, lh * 0.62, lw - 2 * m, lh * 0.38 - m, `hsl(${(it.hue + 45) % 360} 38% 36%)`);
      poly(circlePts(lw * 0.24, lh * 0.26, lh * 0.11), '#fff3b8');
      poly(
        [
          { x: lw * 0.4, y: lh * 0.62 },
          { x: lw * 0.62, y: lh * 0.28 },
          { x: lw * 0.86, y: lh * 0.62 },
        ],
        `hsl(${(it.hue + 20) % 360} 30% 46%)`,
      );
      // tally marks so each photo is recognisable
      for (let k = 0; k <= idx % 4; k++) rect(lw * 0.12 + k * 7, lh * 0.7, 3.6, 12, 'rgba(255,255,255,0.85)');
    } else {
      const inner = lw - 14;
      rect(lw / 2 - inner * 0.3, 8, inner * 0.6, 4.2, 'rgba(38,42,52,0.78)');
      for (let y = 22, i = 0; y < 86; y += 4.2, i++) {
        rect(7, y, inner * (0.4 + ((i * 37) % 30) / 100), 1.6, 'rgba(38,42,52,0.7)');
        rect(lw - 7 - inner * 0.17, y, inner * 0.17, 1.6, 'rgba(38,42,52,0.7)');
      }
    }
  });
  for (const d of spec.dust) {
    fillPoly(
      g,
      d.map((p) => ({ x: p.x * spec.w, y: p.y * spec.h })),
      'rgba(70,64,58,0.8)',
    );
  }
  return c;
}

export interface BedDetection {
  quad: Quad;
  score: number;
  reasons: { code: 'TOUCHING_ITEMS' | 'ITEMS_TOO_CLOSE' }[];
}

/** What the detector would find: one crop per item, one flagged cluster crop per touching group. */
export function bedDetections(spec: BedSpec): BedDetection[] {
  const out: BedDetection[] = [];
  const done = new Set<number>();
  spec.items.forEach((it, i) => {
    if (done.has(i)) return;
    if (it.group === undefined) {
      out.push({ quad: it.quad, score: it.score, reasons: [] });
      return;
    }
    const members = spec.items.map((x, k) => ({ x, k })).filter((m) => m.x.group === it.group);
    members.forEach((m) => done.add(m.k));
    const xs = members.flatMap((m) => m.x.quad.map((p) => p.x));
    const ys = members.flatMap((m) => m.x.quad.map((p) => p.y));
    const [x0, x1, y0, y1] = [Math.min(...xs), Math.max(...xs), Math.min(...ys), Math.max(...ys)];
    out.push({
      quad: [
        { x: x0, y: y0 },
        { x: x1, y: y0 },
        { x: x1, y: y1 },
        { x: x0, y: y1 },
      ],
      score: 0.82,
      reasons: [{ code: 'TOUCHING_ITEMS' }],
    });
  });
  return out;
}

/** The single-item reading of a bed: one crop around everything on it (what "Treat as one item" gives). */
export function bedSingle(spec: BedSpec): Quad {
  const pts = spec.items.flatMap((i) => i.quad);
  const [x0, x1] = [Math.min(...pts.map((p) => p.x)) - 0.02, Math.max(...pts.map((p) => p.x)) + 0.02];
  const [y0, y1] = [Math.min(...pts.map((p) => p.y)) - 0.02, Math.max(...pts.map((p) => p.y)) + 0.02];
  const c = (v: number) => Math.min(1, Math.max(0, v));
  return [
    { x: c(x0), y: c(y0) },
    { x: c(x1), y: c(y0) },
    { x: c(x1), y: c(y1) },
    { x: c(x0), y: c(y1) },
  ];
}

/** A named bed layout of the mock samples. */
export function bedLayout(kind: 'albums' | 'receipts' | 'two' | 'locked'): BedSpec {
  const W = 1500;
  const H = 1100;
  const box = (cx: number, cy: number, w: number, h: number, deg: number) => rotatedBox(W, H, cx, cy, w, h, deg);
  const bg = '#e9e9e4';
  switch (kind) {
    case 'albums':
      return {
        w: W,
        h: H,
        bg,
        seed: 3,
        items: [
          { quad: box(0.26, 0.27, 540, 390, 3), kind: 'photo', hue: 205, score: 0.985 },
          { quad: box(0.74, 0.26, 560, 380, -2), kind: 'photo', hue: 25, score: 0.98 },
          { quad: box(0.23, 0.75, 380, 440, 1.5), kind: 'photo', hue: 120, score: 0.99 },
          { quad: box(0.6, 0.76, 330, 430, 0), kind: 'photo', hue: 330, group: 1, score: 0.95 },
          { quad: box(0.835, 0.76, 330, 430, 0), kind: 'photo', hue: 275, group: 1, score: 0.95 },
        ],
        dust: [
          [
            { x: 0.455, y: 0.5 },
            { x: 0.47, y: 0.5 },
            { x: 0.47, y: 0.52 },
            { x: 0.455, y: 0.52 },
          ],
        ],
      };
    case 'receipts':
      return {
        w: W,
        h: H,
        bg,
        seed: 5,
        items: [
          { quad: box(0.2, 0.5, 300, 900, 2), kind: 'receipt', hue: 0, score: 0.985 },
          { quad: box(0.5, 0.5, 300, 860, -1.5), kind: 'receipt', hue: 0, score: 0.99 },
          { quad: box(0.8, 0.5, 300, 920, 1), kind: 'receipt', hue: 0, score: 0.987 },
        ],
        dust: [],
      };
    case 'two':
    case 'locked':
      return {
        w: W,
        h: H,
        bg,
        seed: kind === 'two' ? 8 : 9,
        items: [
          { quad: box(0.27, 0.5, 560, 760, 2), kind: 'photo', hue: kind === 'two' ? 190 : 45, score: 0.99 },
          { quad: box(0.73, 0.5, 560, 760, -2), kind: 'photo', hue: kind === 'two' ? 300 : 160, score: 0.985 },
        ],
        dust: [],
      };
  }
}
