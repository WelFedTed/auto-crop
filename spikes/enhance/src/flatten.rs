// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The flatten prototype (PLAN 5.3.1 steps 3-4), luma only.
//!
//! 1. Blocks of `block` px at half-block stride; per block a 256-bin histogram gives the
//!    `percentile` luma `p` (default 88th) and `m`, the mean of the pixels at or above `p`.
//! 2. A block is rejected when it is mostly dark relative to its neighbours (black band,
//!    reverse-video header: `p < reject_dark x` the median `p` of the surrounding valid blocks) or
//!    when more than `reject_ink` of its pixels are ink (dense barcode or QR code).
//! 3. Rejected cells are filled by push-pull diffusion on the coarse grid, then a 3x3 median and a
//!    Gaussian of one block smooth the map.
//! 4. Gain `W / M(x, y)` (bilinear from the grid), clamped to `[1, max_gain]`, white level `W`.

use std::time::Instant;

#[derive(Clone)]
pub struct Gray {
    pub w: usize,
    pub h: usize,
    pub data: Vec<u8>,
}

#[derive(Clone, Copy, Debug)]
pub struct Params {
    pub block: usize,
    pub stride: usize,
    pub percentile: f32,
    /// Reject when `p` is below this fraction of the local median `p` (PLAN: 0.60).
    pub reject_dark: f32,
    /// Reject when more than this fraction of the block is ink (PLAN: 0.35).
    pub reject_ink: f32,
    /// Second stage: a cell next to rejected cells is rejected when its `p` is below this fraction
    /// of the valid cells around it.
    pub reject_dim: f32,
    /// A pixel is ink when it is below this fraction of the block's own `p`.
    pub ink_level: f32,
    /// Radius (in cells) of the neighbourhood whose median `p` the dark test uses.
    pub median_radius: usize,
    /// Output paper level.
    pub white: f32,
    pub max_gain: f32,
    /// Ablation switch: with `false` no block is rejected (the negative control).
    pub reject: bool,
    /// Estimate-and-apply rounds (see `flatten`).
    pub passes: usize,
    /// Radius (cells) of the plane fit that fills rejected cells (0 = push-pull only).
    pub plane_radius: usize,
    /// Average this many pixels per axis before measuring (1 = off).
    pub pre: usize,
    /// Gaussian sigma of the final map smoothing, in cells (PLAN: one block = 2 cells).
    pub smooth: f32,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            block: 32,
            stride: 16,
            percentile: 0.88,
            reject_dark: 0.60,
            reject_ink: 0.35,
            reject_dim: 0.93,
            ink_level: 0.70,
            median_radius: 3,
            white: 235.0,
            max_gain: 3.0,
            reject: true,
            passes: 2,
            plane_radius: 8,
            pre: 2,
            smooth: 2.0,
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct Stats {
    pub cells: usize,
    pub rejected_dark: usize,
    pub rejected_ink: usize,
    pub max_gain: f32,
    pub grid_ms: f64,
    pub apply_ms: f64,
}

struct Grid {
    gw: usize,
    gh: usize,
    v: Vec<f32>,
}

fn percentile_of(hist: &[u32; 256], n: u32, q: f32) -> u8 {
    let target = ((n as f32) * q).ceil().max(1.0) as u32;
    let mut acc = 0;
    for (v, &c) in hist.iter().enumerate() {
        acc += c;
        if acc >= target {
            return v as u8;
        }
    }
    255
}

fn median(v: &mut [f32]) -> f32 {
    v.sort_by(|a, b| a.total_cmp(b));
    v[v.len() / 2]
}

/// The coarse illumination map (before inpainting): per cell the mean above the percentile, and
/// whether the cell is valid.
///
/// Rejection needs a paper-level reference that follows the lighting, so it uses the median
/// percentile `p` of the cells around (`median_radius`): a cell is dark when its own `p` is far
/// below it, and a pixel is ink when it is far below it. Ink is counted on the full-resolution
/// pixels of the cell's ground area (a fine barcode averages to a uniform mid-grey whose own
/// percentile looks like dim paper; only the unaveraged pixels and the neighbours' paper level
/// give it away).
fn block_grid(
    img: &Gray,
    full: &Gray,
    f: usize,
    p: &Params,
    stats: &mut Stats,
) -> (Grid, Vec<bool>) {
    let gw = (img.w.saturating_sub(p.block)) / p.stride + 1;
    let gh = (img.h.saturating_sub(p.block)) / p.stride + 1;
    let mut m = vec![0.0f32; gw * gh];
    let mut pv = vec![0.0f32; gw * gh];
    let mut fhist = vec![[0u32; 256]; gw * gh];
    let mut fcount = vec![0u32; gw * gh];
    for gy in 0..gh {
        for gx in 0..gw {
            let (x0, y0) = (gx * p.stride, gy * p.stride);
            let (x1, y1) = ((x0 + p.block).min(img.w), (y0 + p.block).min(img.h));
            let mut hist = [0u32; 256];
            for y in y0..y1 {
                for &b in &img.data[y * img.w + x0..y * img.w + x1] {
                    hist[b as usize] += 1;
                }
            }
            let n = ((x1 - x0) * (y1 - y0)) as u32;
            let pc = percentile_of(&hist, n, p.percentile);
            let (mut s, mut c) = (0u64, 0u32);
            for (v, &cnt) in hist.iter().enumerate().skip(pc as usize) {
                s += v as u64 * u64::from(cnt);
                c += cnt;
            }
            let i = gy * gw + gx;
            let (fx0, fy0) = (x0 * f, y0 * f);
            let (fx1, fy1) = ((x1 * f).min(full.w), (y1 * f).min(full.h));
            for y in fy0..fy1 {
                for &b in &full.data[y * full.w + fx0..y * full.w + fx1] {
                    fhist[i][b as usize] += 1;
                }
            }
            fcount[i] = ((fx1 - fx0) * (fy1 - fy0)) as u32;
            m[i] = s as f32 / c.max(1) as f32;
            pv[i] = f32::from(pc);
        }
    }
    stats.cells = gw * gh;
    let mut valid = vec![true; gw * gh];
    if p.reject {
        let r = p.median_radius as isize;
        for gy in 0..gh {
            for gx in 0..gw {
                let i = gy * gw + gx;
                let mut nb = Vec::new();
                for dy in -r..=r {
                    for dx in -r..=r {
                        let (x, y) = (gx as isize + dx, gy as isize + dy);
                        if x >= 0 && y >= 0 && (x as usize) < gw && (y as usize) < gh {
                            nb.push(pv[y as usize * gw + x as usize]);
                        }
                    }
                }
                let local = median(&mut nb);
                let ink_cut = (p.ink_level * local).floor().clamp(0.0, 255.0) as usize;
                let ink_n: u32 = fhist[i][..ink_cut].iter().sum();
                let ink = ink_n as f32 / fcount[i].max(1) as f32;
                if pv[i] < p.reject_dark * local {
                    valid[i] = false;
                    stats.rejected_dark += 1;
                } else if ink > p.reject_ink {
                    valid[i] = false;
                    stats.rejected_ink += 1;
                }
            }
        }
        // Second stage: a surviving cell that is dimmer than the valid paper around it and borders
        // rejected cells is distrusted too. Fine barcodes leave a few sparse columns whose own
        // percentile (about 85% of paper) and ink share both look innocent; their neighbours give
        // them away.
        let stage1 = valid.clone();
        for gy in 0..gh {
            for gx in 0..gw {
                let i = gy * gw + gx;
                if !stage1[i] {
                    continue;
                }
                let (mut bad, mut around) = (0, Vec::new());
                for dy in -r..=r {
                    for dx in -r..=r {
                        let (x, y) = (gx as isize + dx, gy as isize + dy);
                        if x < 0 || y < 0 || x as usize >= gw || y as usize >= gh {
                            continue;
                        }
                        let j = y as usize * gw + x as usize;
                        if stage1[j] {
                            around.push(pv[j]);
                        } else if dx.abs() <= 1 && dy.abs() <= 1 {
                            bad += 1;
                        }
                    }
                }
                if bad >= 2 && !around.is_empty() && pv[i] < p.reject_dim * median(&mut around) {
                    valid[i] = false;
                    stats.rejected_ink += 1;
                }
            }
        }
    }
    (Grid { gw, gh, v: m }, valid)
}

/// Push-pull fill: coarse levels average the valid cells, then each level fills its holes from the
/// level above (bilinear). Valid cells keep their value.
fn push_pull(g: &mut Grid, valid: &[bool]) {
    struct Level {
        w: usize,
        h: usize,
        v: Vec<f32>,
        wt: Vec<f32>,
    }
    let mut levels = vec![Level {
        w: g.gw,
        h: g.gh,
        v: g.v.clone(),
        wt: valid.iter().map(|&b| if b { 1.0 } else { 0.0 }).collect(),
    }];
    while levels.last().map(|l| l.w > 1 || l.h > 1).unwrap_or(false) {
        let f = levels.last().unwrap();
        let (cw, ch) = (f.w.div_ceil(2), f.h.div_ceil(2));
        let mut v = vec![0.0; cw * ch];
        let mut wt = vec![0.0; cw * ch];
        for cy in 0..ch {
            for cx in 0..cw {
                let (mut sv, mut sw, mut n) = (0.0f32, 0.0f32, 0.0f32);
                for dy in 0..2 {
                    for dx in 0..2 {
                        let (x, y) = (cx * 2 + dx, cy * 2 + dy);
                        if x < f.w && y < f.h {
                            n += 1.0;
                            sv += f.v[y * f.w + x] * f.wt[y * f.w + x];
                            sw += f.wt[y * f.w + x];
                        }
                    }
                }
                v[cy * cw + cx] = if sw > 0.0 { sv / sw } else { 0.0 };
                wt[cy * cw + cx] = (sw / n).min(1.0);
            }
        }
        levels.push(Level {
            w: cw,
            h: ch,
            v,
            wt,
        });
    }
    // Coarsest level: if nothing was valid anywhere, there is nothing to fill from.
    if levels.last().unwrap().wt[0] == 0.0 {
        g.v.iter_mut().for_each(|x| *x = 0.0);
        return;
    }
    for li in (0..levels.len() - 1).rev() {
        let (lo, hi) = levels.split_at_mut(li + 1);
        let fine = &mut lo[li];
        let coarse = &hi[0];
        for y in 0..fine.h {
            for x in 0..fine.w {
                let i = y * fine.w + x;
                if fine.wt[i] >= 1.0 {
                    continue;
                }
                let up = sample(
                    &coarse.v,
                    coarse.w,
                    coarse.h,
                    (x as f32 - 0.5) / 2.0,
                    (y as f32 - 0.5) / 2.0,
                );
                fine.v[i] = fine.wt[i] * fine.v[i] + (1.0 - fine.wt[i]) * up;
                fine.wt[i] = 1.0;
            }
        }
    }
    g.v = levels.swap_remove(0).v;
}

/// Refines the push-pull fill: each rejected cell gets the value of a weighted least-squares
/// *quadratic* surface (`1, x, y, x^2, xy, y^2`) through the valid cells within `radius` cells
/// (Gaussian weights). Push-pull averages, so across a large hole it flattens any gradient; a
/// plane would keep a linear trend exact, and the quadratic term also follows the curvature of a
/// shadow edge or vignette. A small ridge on the non-constant terms keeps one-sided fits tame.
/// Cells with too few valid neighbours keep the push-pull value.
fn plane_fill(g: &mut Grid, valid: &[bool], radius: usize) {
    const K: usize = 6;
    let r = radius as isize;
    let sigma2 = (radius as f32 * 0.6).powi(2);
    let src = g.v.clone();
    let basis = |dx: f64, dy: f64| [1.0, dx, dy, dx * dx, dx * dy, dy * dy];
    for gy in 0..g.gh {
        for gx in 0..g.gw {
            if valid[gy * g.gw + gx] {
                continue;
            }
            let mut a = [[0.0f64; K]; K];
            let mut b = [0.0f64; K];
            let (mut n, mut sw) = (0, 0.0f64);
            for dy in -r..=r {
                for dx in -r..=r {
                    let (x, y) = (gx as isize + dx, gy as isize + dy);
                    if x < 0 || y < 0 || x as usize >= g.gw || y as usize >= g.gh {
                        continue;
                    }
                    let j = y as usize * g.gw + x as usize;
                    if !valid[j] {
                        continue;
                    }
                    let w = f64::from((-((dx * dx + dy * dy) as f32) / (2.0 * sigma2)).exp());
                    let phi = basis(dx as f64, dy as f64);
                    let z = f64::from(src[j]);
                    n += 1;
                    sw += w;
                    for p in 0..K {
                        b[p] += w * phi[p] * z;
                        for q in 0..K {
                            a[p][q] += w * phi[p] * phi[q];
                        }
                    }
                }
            }
            if n < 14 {
                continue;
            }
            // Ridge scaled to each term's own magnitude (x^2 terms are ~r^2 larger than 1).
            for p in 1..K {
                a[p][p] += 0.02 * a[p][p] + 1e-6 * sw;
            }
            if let Some(c) = solve(a, b) {
                g.v[gy * g.gw + gx] = c[0] as f32;
            }
        }
    }
}

/// Gaussian elimination with partial pivoting.
fn solve<const K: usize>(mut a: [[f64; K]; K], mut b: [f64; K]) -> Option<[f64; K]> {
    for col in 0..K {
        let piv = (col..K).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[piv][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        for row in col + 1..K {
            let f = a[row][col] / a[col][col];
            for k in col..K {
                a[row][k] -= f * a[col][k];
            }
            b[row] -= f * b[col];
        }
    }
    let mut x = [0.0f64; K];
    for row in (0..K).rev() {
        let s: f64 = (row + 1..K).map(|k| a[row][k] * x[k]).sum();
        x[row] = (b[row] - s) / a[row][row];
    }
    Some(x)
}

fn sample(v: &[f32], w: usize, h: usize, x: f32, y: f32) -> f32 {
    let x = x.clamp(0.0, (w - 1) as f32);
    let y = y.clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let a = v[y0 * w + x0] * (1.0 - fx) + v[y0 * w + x1] * fx;
    let b = v[y1 * w + x0] * (1.0 - fx) + v[y1 * w + x1] * fx;
    a * (1.0 - fy) + b * fy
}

fn median3(g: &Grid) -> Vec<f32> {
    let mut out = g.v.clone();
    for y in 0..g.gh {
        for x in 0..g.gw {
            let mut nb = Vec::with_capacity(9);
            for dy in -1isize..=1 {
                for dx in -1isize..=1 {
                    let (xx, yy) = (x as isize + dx, y as isize + dy);
                    if xx >= 0 && yy >= 0 && (xx as usize) < g.gw && (yy as usize) < g.gh {
                        nb.push(g.v[yy as usize * g.gw + xx as usize]);
                    }
                }
            }
            out[y * g.gw + x] = median(&mut nb);
        }
    }
    out
}

/// Value at index `i` of a line of `n` values with *odd reflection* about the ends
/// (`v[-k] = 2 v[0] - v[k]`), which keeps a linear trend linear across the border where clamping
/// would flatten it (and bias every cell within a few sigma of the page edge).
fn odd(at: impl Fn(usize) -> f32, i: isize, n: usize) -> f32 {
    let last = n as isize - 1;
    if i < 0 {
        2.0 * at(0) - at((-i).min(last) as usize)
    } else if i > last {
        2.0 * at(last as usize) - at((2 * last - i).max(0) as usize)
    } else {
        at(i as usize)
    }
}

/// Bilinear sample that continues the outermost cells linearly beyond the grid (the outer half
/// block of the page lies outside the first and last cell centres).
fn sample_extrap(v: &[f32], w: usize, h: usize, x: f32, y: f32) -> f32 {
    let xi = x.floor().clamp(0.0, (w as f32 - 2.0).max(0.0)) as usize;
    let yi = y.floor().clamp(0.0, (h as f32 - 2.0).max(0.0)) as usize;
    let (x1, y1) = ((xi + 1).min(w - 1), (yi + 1).min(h - 1));
    let (fx, fy) = (x - xi as f32, y - yi as f32);
    let a = v[yi * w + xi] * (1.0 - fx) + v[yi * w + x1] * fx;
    let b = v[y1 * w + xi] * (1.0 - fx) + v[y1 * w + x1] * fx;
    a * (1.0 - fy) + b * fy
}

fn gaussian(g: &mut Grid, sigma: f32) {
    let r = (sigma * 3.0).ceil() as isize;
    let k: Vec<f32> = (-r..=r)
        .map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp())
        .collect();
    let ks: f32 = k.iter().sum();
    let mut tmp = g.v.clone();
    for y in 0..g.gh {
        for x in 0..g.gw {
            let mut a = 0.0;
            for (j, kv) in k.iter().enumerate() {
                let xx = x as isize + j as isize - r;
                a += kv * odd(|i| g.v[y * g.gw + i], xx, g.gw);
            }
            tmp[y * g.gw + x] = a / ks;
        }
    }
    for y in 0..g.gh {
        for x in 0..g.gw {
            let mut a = 0.0;
            for (j, kv) in k.iter().enumerate() {
                let yy = y as isize + j as isize - r;
                a += kv * odd(|i| tmp[i * g.gw + x], yy, g.gh);
            }
            g.v[y * g.gw + x] = a / ks;
        }
    }
}

/// The illumination map on the coarse grid (cell `i` is centred at `i * stride + block / 2`).
fn illumination(img: &Gray, p: &Params, stats: &mut Stats) -> Grid {
    let t = Instant::now();
    // Averaging f x f pixels first cuts sensor noise by f (a high percentile of noisy pixels reads
    // high, and by more in dark areas where the gain, hence the amplified noise, is larger) and
    // turns fine barcodes into the grey they are for the percentile.
    let f = p.pre.max(1);
    let small;
    let (src, q) = if f > 1 {
        small = downscale(img, f);
        let q = Params {
            block: p.block / f,
            stride: p.stride / f,
            // The same ground distance in (smaller) cells.
            median_radius: p.median_radius * f,
            ..*p
        };
        (&small, q)
    } else {
        (img, *p)
    };
    let (mut g, valid) = block_grid(src, img, f, &q, stats);
    push_pull(&mut g, &valid);
    if p.reject && p.plane_radius > 0 {
        plane_fill(&mut g, &valid, p.plane_radius);
    }
    g.v = median3(&g);
    gaussian(&mut g, p.smooth);
    stats.grid_ms += t.elapsed().as_secs_f64() * 1e3;
    g
}

fn downscale(img: &Gray, f: usize) -> Gray {
    let (w, h) = (img.w / f, img.h / f);
    let mut data = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0u32;
            for dy in 0..f {
                for dx in 0..f {
                    s += u32::from(img.data[(y * f + dy) * img.w + x * f + dx]);
                }
            }
            data[y * w + x] = ((s + (f * f / 2) as u32) / (f * f) as u32) as u8;
        }
    }
    Gray { w, h, data }
}

/// Debug view of the first pass: (grid width, grid height, raw cell value, valid flag) at the cell
/// centres, plus the final smoothed map value per cell.
pub fn debug_cells(img: &Gray, p: &Params) -> (usize, usize, Vec<f32>, Vec<bool>, Vec<f32>) {
    let mut stats = Stats::default();
    let f = p.pre.max(1);
    let small = downscale(img, f);
    let q = Params {
        block: p.block / f,
        stride: p.stride / f,
        median_radius: p.median_radius * f,
        ..*p
    };
    let (g, valid) = block_grid(&small, img, f, &q, &mut stats);
    let raw = g.v.clone();
    let fin = illumination(img, p, &mut stats);
    (g.gw, g.gh, raw, valid, fin.v)
}

/// Runs `p.passes` rounds. A high percentile inside a block is biased towards the brighter side of
/// the block when light falls off across it, so round 1 under-corrects steep gradients; later
/// rounds measure the (much smaller) remaining gradient on the already flattened image and
/// multiply their gain in. The final gain is applied to the original once.
pub fn flatten(img: &Gray, p: &Params) -> (Gray, Stats) {
    let mut stats = Stats::default();
    let n = img.data.len();
    let mut total = vec![1.0f32; n];
    let mut cur = img.clone();
    let half = p.block as f32 / 2.0;
    for _ in 0..p.passes.max(1) {
        let g = illumination(&cur, p, &mut stats);
        let t = Instant::now();
        for y in 0..img.h {
            let gy = (y as f32 - half) / p.stride as f32;
            for x in 0..img.w {
                let gx = (x as f32 - half) / p.stride as f32;
                let m = sample_extrap(&g.v, g.gw, g.gh, gx, gy).max(1.0);
                let i = y * img.w + x;
                total[i] *= (p.white / m).clamp(0.5, p.max_gain);
                let v = f32::from(img.data[i]) * total[i].clamp(1.0, p.max_gain);
                cur.data[i] = (v + 0.5).clamp(0.0, 255.0) as u8;
            }
        }
        stats.apply_ms += t.elapsed().as_secs_f64() * 1e3;
    }
    stats.max_gain = total.iter().fold(1.0f32, |a, &g| a.max(g.min(p.max_gain)));
    (cur, stats)
}
