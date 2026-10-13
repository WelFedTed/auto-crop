// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// The browser mock's stand-in for the engine's enhancer (crates/imgproc/src/enhance): a small canvas version of
// "flatten the light, whiten the paper, optionally make it black and white", enough to SEE the panel work. It is not the
// real algorithm and says nothing about its quality. Browser only (needs canvas); the real app never loads this.

import type { EnhanceMode, EnhanceSuggestion } from './types.ts';

function canvas(w: number, h: number): HTMLCanvasElement {
  const c = document.createElement('canvas');
  c.width = Math.max(1, w);
  c.height = Math.max(1, h);
  return c;
}

/** The paper level per channel on a coarse grid: the 85th percentile of each cell (ink is the minority), smoothed, then sampled bilinearly. */
function background(d: Uint8ClampedArray, w: number, h: number): (x: number, y: number, ch: number) => number {
  const cell = Math.max(10, Math.round(Math.max(w, h) / 40));
  const gw = Math.ceil(w / cell);
  const gh = Math.ceil(h / cell);
  const grid = [new Float32Array(gw * gh), new Float32Array(gw * gh), new Float32Array(gw * gh)];
  for (let gy = 0; gy < gh; gy++) {
    for (let gx = 0; gx < gw; gx++) {
      for (let ch = 0; ch < 3; ch++) {
        const vals: number[] = [];
        for (let y = gy * cell; y < Math.min(h, (gy + 1) * cell); y += 2) {
          for (let x = gx * cell; x < Math.min(w, (gx + 1) * cell); x += 2) vals.push(d[(y * w + x) * 4 + ch]);
        }
        vals.sort((a, b) => a - b);
        grid[ch][gy * gw + gx] = vals[Math.min(vals.length - 1, Math.floor(vals.length * 0.85))] ?? 255;
      }
    }
  }
  for (const g of grid) {
    for (let pass = 0; pass < 2; pass++) {
      const copy = g.slice();
      for (let y = 0; y < gh; y++) {
        for (let x = 0; x < gw; x++) {
          let s = 0;
          let n = 0;
          for (let dy = -1; dy <= 1; dy++) {
            for (let dx = -1; dx <= 1; dx++) {
              const xx = x + dx;
              const yy = y + dy;
              if (xx >= 0 && yy >= 0 && xx < gw && yy < gh) {
                s += copy[yy * gw + xx];
                n++;
              }
            }
          }
          g[y * gw + x] = s / n;
        }
      }
    }
  }
  return (x, y, ch) => {
    const fx = Math.min(gw - 1, Math.max(0, x / cell - 0.5));
    const fy = Math.min(gh - 1, Math.max(0, y / cell - 0.5));
    const x0 = Math.floor(fx);
    const y0 = Math.floor(fy);
    const x1 = Math.min(gw - 1, x0 + 1);
    const y1 = Math.min(gh - 1, y0 + 1);
    const tx = fx - x0;
    const ty = fy - y0;
    const g = grid[ch];
    const top = g[y0 * gw + x0] * (1 - tx) + g[y0 * gw + x1] * tx;
    const bot = g[y1 * gw + x0] * (1 - tx) + g[y1 * gw + x1] * tx;
    return top * (1 - ty) + bot * ty;
  };
}

/** The mean of the luma in a square window around every pixel (integral image). */
function localMean(luma: Float32Array, w: number, h: number, r: number): Float32Array {
  const I = new Float64Array((w + 1) * (h + 1));
  for (let y = 0; y < h; y++) {
    let row = 0;
    for (let x = 0; x < w; x++) {
      row += luma[y * w + x];
      I[(y + 1) * (w + 1) + x + 1] = I[y * (w + 1) + x + 1] + row;
    }
  }
  const out = new Float32Array(w * h);
  for (let y = 0; y < h; y++) {
    const y0 = Math.max(0, y - r);
    const y1 = Math.min(h, y + r + 1);
    for (let x = 0; x < w; x++) {
      const x0 = Math.max(0, x - r);
      const x1 = Math.min(w, x + r + 1);
      const sum = I[y1 * (w + 1) + x1] - I[y0 * (w + 1) + x1] - I[y1 * (w + 1) + x0] + I[y0 * (w + 1) + x0];
      out[y * w + x] = sum / ((x1 - x0) * (y1 - y0));
    }
  }
  return out;
}

/**
 * `src` with `mode` applied at `strength` percent (0 to 100): Clean divides out the paper level per channel (even light,
 * white paper, colour kept); Document and Receipt then make it black and white with a local threshold. The result is blended
 * with the input by the strength, like the engine's. `src` itself is never touched.
 */
export function enhanceCanvas(src: HTMLCanvasElement, mode: EnhanceMode, strength: number): HTMLCanvasElement {
  const s = Math.min(100, Math.max(0, strength)) / 100;
  if (mode === 'original' || s <= 0) return src;
  const w = src.width;
  const h = src.height;
  const input = src.getContext('2d', { willReadFrequently: true })!.getImageData(0, 0, w, h);
  const d = input.data;
  const bg = background(d, w, h);
  const flat = new Float32Array(w * h * 3);
  const luma = new Float32Array(w * h);
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const i = y * w + x;
      for (let ch = 0; ch < 3; ch++) flat[i * 3 + ch] = Math.min(255, (d[i * 4 + ch] * 250) / Math.max(40, bg(x, y, ch)));
      luma[i] = 0.299 * flat[i * 3] + 0.587 * flat[i * 3 + 1] + 0.114 * flat[i * 3 + 2];
    }
  }
  const result = new Uint8ClampedArray(w * h * 4);
  if (mode === 'clean') {
    // set the levels: the darkest 1% (the ink) goes to near black, the paper to white
    const sorted = Float32Array.from(luma).sort();
    const black = Math.min(90, sorted[Math.floor(sorted.length * 0.01)] * 0.85);
    for (let i = 0; i < w * h; i++) {
      for (let ch = 0; ch < 3; ch++) result[i * 4 + ch] = Math.min(255, Math.max(0, ((flat[i * 3 + ch] - black) / (250 - black)) * 255));
      result[i * 4 + 3] = 255;
    }
  } else {
    let lo = 0;
    if (mode === 'receipt') {
      // stretch the contrast first: faded print sits close to the paper
      const sorted = Float32Array.from(luma).sort();
      lo = sorted[Math.floor(sorted.length * 0.02)];
    }
    const stretched = mode === 'receipt' ? luma.map((v) => Math.min(255, Math.max(0, ((v - lo) / Math.max(40, 250 - lo)) * 255))) : luma;
    const mean = localMean(stretched, w, h, Math.max(7, Math.round(Math.max(w, h) / 40)));
    const k = mode === 'receipt' ? 0.9 : 0.84;
    for (let i = 0; i < w * h; i++) {
      const t = mean[i] * k;
      const v = Math.min(255, Math.max(0, ((stretched[i] - (t - 7)) / 14) * 255));
      result[i * 4] = result[i * 4 + 1] = result[i * 4 + 2] = v;
      result[i * 4 + 3] = 255;
    }
  }
  for (let i = 0; i < w * h * 4; i++) if (i % 4 !== 3) result[i] = d[i] * (1 - s) + result[i] * s;
  const out = canvas(w, h);
  out.getContext('2d')!.putImageData(new ImageData(result, w, h), 0, 0);
  return out;
}

/** What the mock's `suggestEnhance` answers for a page it considers a shaded document or receipt (the numbers are made up). */
export function mockEnhanceSuggestion(kind: 'document' | 'receipt', faded: boolean): EnhanceSuggestion {
  const reasons: EnhanceSuggestion['reasons'] = faded ? ['uneven_light', 'faded_print'] : ['uneven_light', 'dull_paper'];
  return {
    mode: 'clean',
    strength: 100,
    reason: reasons[0],
    reasons,
    blackAndWhite: kind === 'receipt' || faded ? 'receipt' : 'document',
    blackAndWhiteHint: { gray: true, recommendPng: true },
    hint: { gray: false, recommendPng: false },
    measures: {
      aspect: kind === 'receipt' ? 3 : 1.4,
      paperFraction: 0.74,
      inkFraction: 0.24,
      shading: 0.4,
      paperLuma: 213,
      cast: 0.04,
      colourFraction: 0,
      textComponents: 900,
      concentration: 0.98,
      glyphInkShare: 0.99,
      inkContrast: faded ? 70 : 195,
      faded,
    },
  };
}
