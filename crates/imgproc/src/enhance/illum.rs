// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The illumination (paper) estimate on the analysis proxy (PLAN 5.3.1 step 3; the production form
//! of the `spikes/enhance` prototype).
//!
//! 1. **Cells.** The proxy is covered by a grid of cells about half a block wide. Per cell, a
//!    256-bin histogram of the block around it gives the high luma percentile (90th by default)
//!    and the mean RGB of the pixels at or above it: the colour and brightness of the paper there.
//!    Pixels outside the validity mask take no part, so an item's estimate never sees its
//!    neighbours or the table.
//! 2. **Rejection.** A cell is not paper when it is much darker than the cells around it (black
//!    band, reverse-video header), when more than a third of its pixels are ink (barcode, QR code,
//!    dense print), when its colour is far from the paper colour around it (stamp, coloured
//!    header), or (second stage) when it is dimmer than the valid cells around it and touches
//!    two rejected ones (the sparse columns of a fine barcode).
//! 3. **Fill and smooth.** Rejected cells are filled by push-pull diffusion, refined by a weighted
//!    quadratic least-squares surface through the valid cells within a radius (diffusion alone
//!    flattens every gradient across a hole), then a 3 x 3 median and a Gaussian of one block.
//!
//! Everything is computed per cell row in parallel from immutable inputs, so the result does not
//! depend on the thread count.

// The grid code indexes three colour channels and small fixed matrices by position; iterator
// forms of those loops are harder to read, not safer.
#![allow(clippy::needless_range_loop)]

use super::Proxy;
use crate::cancel::{Cancel, Cancelled};
use rayon::prelude::*;

/// Knobs of the estimate (`Tuning` holds the user-visible subset).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct IllumParams {
    /// Edge of the measuring block, in proxy pixels.
    pub block: usize,
    pub percentile: f32,
    /// Reject when the cell's percentile is below this share of the local median (0.60).
    pub reject_dark: f32,
    /// Reject when more than this share of the block is ink (0.35).
    pub reject_ink: f32,
    /// A pixel is ink below this share of the local paper level (0.70).
    pub ink_level: f32,
    /// A mostly-ink cell is kept when its percentile reads at least this share of the local
    /// median (0.92): paper shows through.
    pub keep_pc: f32,
    /// Second stage: dimmer than this share of the valid cells around (0.93).
    pub reject_dim: f32,
    /// Largest Cb/Cr distance from the local paper colour, in 8-bit units (12).
    pub chroma_limit: f32,
    /// Radius, in cells, of the neighbourhood whose median is the local reference (3).
    pub median_radius: usize,
    /// Radius, in cells, of the quadratic fit that fills rejected cells (8).
    pub fill_radius: usize,
    /// Gaussian sigma of the final smoothing, in cells (2: one block).
    pub smooth: f32,
}

impl IllumParams {
    pub fn with_block(block: usize, percentile: f32) -> Self {
        Self {
            block: block.max(8),
            percentile,
            reject_dark: 0.60,
            reject_ink: 0.35,
            ink_level: 0.70,
            keep_pc: 0.92,
            reject_dim: 0.93,
            chroma_limit: 12.0,
            median_radius: 3,
            fill_radius: 8,
            smooth: 2.0,
        }
    }
}

/// The smoothed paper colour on a coarse grid. Cell `(i, j)` is centred at
/// `((i + 0.5) * sx, (j + 0.5) * sy)` proxy pixels.
#[derive(Debug, Clone)]
pub(crate) struct Grid {
    pub gw: usize,
    pub gh: usize,
    pub v: Vec<[f32; 3]>,
}

/// What the estimate found.
#[derive(Debug, Clone)]
pub(crate) struct Estimate {
    pub grid: Grid,
    /// Cells inside the page that were rejected, as a share of those cells.
    pub rejected: f32,
    /// Cells inside the page.
    pub cells: usize,
    /// Per cell: inside the page (cells outside it hold extrapolated values).
    pub inside: Vec<bool>,
}

#[derive(Debug, Clone, Copy, Default)]
struct Cell {
    /// Percentile luma.
    pc: f32,
    /// Mean RGB of the pixels at or above the percentile.
    rgb: [f32; 3],
    /// Inside the page and measurable.
    ok: bool,
}

fn luma_to_cbcr(rgb: [f32; 3]) -> (f32, f32) {
    let y = 0.299 * rgb[0] + 0.587 * rgb[1] + 0.114 * rgb[2];
    (0.564 * (rgb[2] - y), 0.713 * (rgb[0] - y))
}

fn median_of(buf: &mut [f32]) -> f32 {
    buf.sort_unstable_by(f32::total_cmp);
    buf[buf.len() / 2]
}

/// Estimates the paper colour of `proxy`. `None` when no cell is measurable (an empty mask).
pub(crate) fn estimate(
    proxy: &Proxy,
    valid: Option<&[u8]>,
    p: &IllumParams,
    cancel: &dyn Cancel,
) -> Result<Option<Estimate>, Cancelled> {
    let (pw, ph) = (proxy.w, proxy.h);
    let stride = (p.block / 2).max(4);
    let gw = pw.div_ceil(stride).max(1);
    let gh = ph.div_ceil(stride).max(1);
    let (sx, sy) = (pw as f32 / gw as f32, ph as f32 / gh as f32);
    let half = p.block as f32 / 2.0;
    let window = |gx: usize, gy: usize| {
        let (cx, cy) = ((gx as f32 + 0.5) * sx, (gy as f32 + 0.5) * sy);
        let x0 = (cx - half).floor().max(0.0) as usize;
        let x1 = ((cx + half).ceil() as usize).min(pw).max(x0 + 1).min(pw);
        let y0 = (cy - half).floor().max(0.0) as usize;
        let y1 = ((cy + half).ceil() as usize).min(ph).max(y0 + 1).min(ph);
        (x0.min(pw - 1), x1, y0.min(ph - 1), y1)
    };

    // Phase 1: percentile and paper colour per cell.
    let mut cells = vec![Cell::default(); gw * gh];
    cells.par_chunks_mut(gw).enumerate().for_each(|(gy, row)| {
        if cancel.is_cancelled() {
            return;
        }
        let mut hist = [0u32; 256];
        for (gx, cell) in row.iter_mut().enumerate() {
            let (x0, x1, y0, y1) = window(gx, gy);
            hist.fill(0);
            let mut n = 0u32;
            for y in y0..y1 {
                let lrow = &proxy.luma[y * pw + x0..y * pw + x1];
                match valid {
                    None => {
                        for v in lrow {
                            hist[usize::from(*v)] += 1;
                        }
                        n += lrow.len() as u32;
                    }
                    Some(m) => {
                        for (v, mv) in lrow.iter().zip(&m[y * pw + x0..y * pw + x1]) {
                            if *mv != 0 {
                                hist[usize::from(*v)] += 1;
                                n += 1;
                            }
                        }
                    }
                }
            }
            let area = ((x1 - x0) * (y1 - y0)) as u32;
            if n == 0 || n * 2 < area {
                continue;
            }
            let target = ((n as f32 * p.percentile).ceil() as u32).clamp(1, n);
            let mut acc = 0u32;
            let mut pc = 255usize;
            for (v, c) in hist.iter().enumerate() {
                acc += c;
                if acc >= target {
                    pc = v;
                    break;
                }
            }
            let (mut sr, mut sg, mut sb, mut cnt) = (0u64, 0u64, 0u64, 0u64);
            for y in y0..y1 {
                let lrow = &proxy.luma[y * pw + x0..y * pw + x1];
                let crow = &proxy.rgb[(y * pw + x0) * 3..(y * pw + x1) * 3];
                for (i, v) in lrow.iter().enumerate() {
                    if usize::from(*v) >= pc && valid.is_none_or(|m| m[y * pw + x0 + i] != 0) {
                        sr += u64::from(crow[i * 3]);
                        sg += u64::from(crow[i * 3 + 1]);
                        sb += u64::from(crow[i * 3 + 2]);
                        cnt += 1;
                    }
                }
            }
            let cnt = cnt.max(1) as f32;
            *cell = Cell {
                pc: pc as f32,
                rgb: [sr as f32 / cnt, sg as f32 / cnt, sb as f32 / cnt],
                ok: true,
            };
        }
    });
    if cancel.is_cancelled() {
        return Err(Cancelled);
    }
    let inside = cells.iter().filter(|c| c.ok).count();
    if inside == 0 {
        return Ok(None);
    }

    // Phase 2: rejection against the cells around.
    let r = p.median_radius as isize;
    let around = |gx: usize, gy: usize, f: &mut dyn FnMut(usize)| {
        for dy in -r..=r {
            for dx in -r..=r {
                let (x, y) = (gx as isize + dx, gy as isize + dy);
                if x >= 0 && y >= 0 && (x as usize) < gw && (y as usize) < gh {
                    f(y as usize * gw + x as usize);
                }
            }
        }
    };
    let mut valid_cell = vec![false; gw * gh];
    valid_cell
        .par_chunks_mut(gw)
        .enumerate()
        .for_each(|(gy, row)| {
            let mut buf = [0f32; 64];
            let mut cb = [0f32; 64];
            let mut cr = [0f32; 64];
            for (gx, out) in row.iter_mut().enumerate() {
                let c = cells[gy * gw + gx];
                if !c.ok {
                    continue;
                }
                let mut n = 0;
                around(gx, gy, &mut |j| {
                    if cells[j].ok && n < 64 {
                        buf[n] = cells[j].pc;
                        let (b, rr) = luma_to_cbcr(cells[j].rgb);
                        cb[n] = b;
                        cr[n] = rr;
                        n += 1;
                    }
                });
                let local = median_of(&mut buf[..n]);
                let (mcb, mcr) = (median_of(&mut cb[..n]), median_of(&mut cr[..n]));
                // Dark relative to the cells around.
                if c.pc < p.reject_dark * local {
                    continue;
                }
                // Colour far from the paper colour around (stamps, coloured headers).
                let (ccb, ccr) = luma_to_cbcr(c.rgb);
                if (ccb - mcb).abs().max((ccr - mcr).abs()) > p.chroma_limit {
                    continue;
                }
                // Ink share of the block (counted on the proxy against the local paper level).
                let (x0, x1, y0, y1) = window(gx, gy);
                let cut = (p.ink_level * local).floor().clamp(0.0, 255.0) as u8;
                let (mut ink, mut tot) = (0u32, 0u32);
                for y in y0..y1 {
                    for (i, v) in proxy.luma[y * pw + x0..y * pw + x1].iter().enumerate() {
                        if valid.is_none_or(|m| m[y * pw + x0 + i] != 0) {
                            tot += 1;
                            ink += u32::from(*v < cut);
                        }
                    }
                }
                // A cell that is mostly ink is not paper, unless its own percentile still reads the
                // paper level of its neighbours: then paper shows through (the edge of a band, a
                // coarse barcode) and the percentile is a fair measurement. A blurred fine barcode
                // reads dimmer than that and is rejected.
                if tot > 0 && ink as f32 / tot as f32 > p.reject_ink && c.pc < p.keep_pc * local {
                    continue;
                }
                *out = true;
            }
        });
    if cancel.is_cancelled() {
        return Err(Cancelled);
    }
    // Second stage: a surviving cell dimmer than the valid cells around it that touches two
    // rejected cells is distrusted too.
    let stage1 = valid_cell.clone();
    valid_cell
        .par_chunks_mut(gw)
        .enumerate()
        .for_each(|(gy, row)| {
            let mut buf = [0f32; 64];
            for (gx, out) in row.iter_mut().enumerate() {
                if !*out {
                    continue;
                }
                let (mut bad, mut n) = (0, 0);
                for dy in -r..=r {
                    for dx in -r..=r {
                        let (x, y) = (gx as isize + dx, gy as isize + dy);
                        if x < 0 || y < 0 || x as usize >= gw || y as usize >= gh {
                            continue;
                        }
                        let j = y as usize * gw + x as usize;
                        if stage1[j] {
                            if n < 64 {
                                buf[n] = cells[j].pc;
                                n += 1;
                            }
                        } else if cells[j].ok && dx.abs() <= 1 && dy.abs() <= 1 {
                            bad += 1;
                        }
                    }
                }
                if bad >= 2 && n > 0 {
                    let m = median_of(&mut buf[..n]);
                    if cells[gy * gw + gx].pc < p.reject_dim * m {
                        *out = false;
                    }
                }
            }
        });
    let kept = valid_cell.iter().filter(|v| **v).count();
    if kept == 0 {
        return Ok(None);
    }
    let rejected = 1.0 - kept as f32 / inside as f32;

    // Fill and smooth.
    let mut v: Vec<[f32; 3]> = cells.iter().map(|c| c.rgb).collect();
    push_pull(&mut v, &valid_cell, gw, gh);
    if kept < gw * gh && p.fill_radius > 0 {
        quad_fill(&mut v, &valid_cell, gw, gh, p.fill_radius);
    }
    if cancel.is_cancelled() {
        return Err(Cancelled);
    }
    let med = median3(&v, gw, gh);
    let mut grid = Grid { gw, gh, v: med };
    gaussian(&mut grid, p.smooth);
    Ok(Some(Estimate {
        grid,
        rejected,
        cells: inside,
        inside: cells.iter().map(|c| c.ok).collect(),
    }))
}

fn sample(v: &[[f32; 3]], w: usize, h: usize, x: f32, y: f32) -> [f32; 3] {
    let x = x.clamp(0.0, (w - 1) as f32);
    let y = y.clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let mut out = [0f32; 3];
    for (c, o) in out.iter_mut().enumerate() {
        let a = v[y0 * w + x0][c] * (1.0 - fx) + v[y0 * w + x1][c] * fx;
        let b = v[y1 * w + x0][c] * (1.0 - fx) + v[y1 * w + x1][c] * fx;
        *o = a * (1.0 - fy) + b * fy;
    }
    out
}

/// Push-pull fill: coarse levels average the valid cells, then each level fills its holes from the
/// level above. Valid cells keep their value.
fn push_pull(v: &mut [[f32; 3]], valid: &[bool], gw: usize, gh: usize) {
    struct Level {
        w: usize,
        h: usize,
        v: Vec<[f32; 3]>,
        wt: Vec<f32>,
    }
    let mut levels = vec![Level {
        w: gw,
        h: gh,
        v: v.to_vec(),
        wt: valid.iter().map(|&b| f32::from(u8::from(b))).collect(),
    }];
    while levels.last().is_some_and(|l| l.w > 1 || l.h > 1) {
        let f = levels.last().expect("non-empty");
        let (cw, ch) = (f.w.div_ceil(2), f.h.div_ceil(2));
        let mut nv = vec![[0f32; 3]; cw * ch];
        let mut nwt = vec![0f32; cw * ch];
        for cy in 0..ch {
            for cx in 0..cw {
                let (mut sv, mut sw, mut n) = ([0f32; 3], 0f32, 0f32);
                for dy in 0..2 {
                    for dx in 0..2 {
                        let (x, y) = (cx * 2 + dx, cy * 2 + dy);
                        if x < f.w && y < f.h {
                            let i = y * f.w + x;
                            n += 1.0;
                            sw += f.wt[i];
                            for c in 0..3 {
                                sv[c] += f.v[i][c] * f.wt[i];
                            }
                        }
                    }
                }
                let o = cy * cw + cx;
                if sw > 0.0 {
                    nv[o] = [sv[0] / sw, sv[1] / sw, sv[2] / sw];
                }
                nwt[o] = (sw / n).min(1.0);
            }
        }
        levels.push(Level {
            w: cw,
            h: ch,
            v: nv,
            wt: nwt,
        });
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
                let a = fine.wt[i];
                for c in 0..3 {
                    fine.v[i][c] = a * fine.v[i][c] + (1.0 - a) * up[c];
                }
                fine.wt[i] = 1.0;
            }
        }
    }
    v.copy_from_slice(&levels[0].v);
}

/// Refines the push-pull fill: each rejected cell takes the value of a weighted least-squares
/// quadratic surface through the valid cells within `radius` cells (Gaussian weights). Push-pull
/// averages, so across a large hole it flattens every gradient; the quadratic keeps a shadow edge
/// or a vignette going through the hole. Cells with too few valid neighbours keep the push-pull
/// value.
fn quad_fill(v: &mut [[f32; 3]], valid: &[bool], gw: usize, gh: usize, radius: usize) {
    const K: usize = 6;
    let r = radius as isize;
    let sigma2 = (radius as f32 * 0.6).powi(2);
    let src: Vec<[f32; 3]> = v.to_vec();
    let holes: Vec<usize> = (0..gw * gh).filter(|&i| !valid[i]).collect();
    let fills: Vec<Option<[f32; 3]>> = holes
        .par_iter()
        .map(|&i| {
            let (gx, gy) = ((i % gw) as isize, (i / gw) as isize);
            let mut a = [[0f64; K]; K];
            let mut b = [[0f64; 3]; K];
            let (mut n, mut sw) = (0, 0f64);
            for dy in -r..=r {
                for dx in -r..=r {
                    let (x, y) = (gx + dx, gy + dy);
                    if x < 0 || y < 0 || x as usize >= gw || y as usize >= gh {
                        continue;
                    }
                    let j = y as usize * gw + x as usize;
                    if !valid[j] {
                        continue;
                    }
                    let w = f64::from((-((dx * dx + dy * dy) as f32) / (2.0 * sigma2)).exp());
                    let (fx, fy) = (dx as f64, dy as f64);
                    let phi = [1.0, fx, fy, fx * fx, fx * fy, fy * fy];
                    n += 1;
                    sw += w;
                    for p in 0..K {
                        for c in 0..3 {
                            b[p][c] += w * phi[p] * f64::from(src[j][c]);
                        }
                        for q in 0..K {
                            a[p][q] += w * phi[p] * phi[q];
                        }
                    }
                }
            }
            if n < 14 {
                return None;
            }
            // Ridge scaled to each term's own magnitude.
            for p in 1..K {
                a[p][p] += 0.02 * a[p][p] + 1e-6 * sw;
            }
            solve(a, b).map(|s| [s[0][0] as f32, s[0][1] as f32, s[0][2] as f32])
        })
        .collect();
    for (i, f) in holes.into_iter().zip(fills) {
        if let Some(f) = f {
            v[i] = f;
        }
    }
}

/// Gaussian elimination with partial pivoting; three right-hand sides.
fn solve<const K: usize>(mut a: [[f64; K]; K], mut b: [[f64; 3]; K]) -> Option<[[f64; 3]; K]> {
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
            for c in 0..3 {
                b[row][c] -= f * b[col][c];
            }
        }
    }
    let mut x = [[0f64; 3]; K];
    for row in (0..K).rev() {
        for c in 0..3 {
            let s: f64 = (row + 1..K).map(|k| a[row][k] * x[k][c]).sum();
            x[row][c] = (b[row][c] - s) / a[row][row];
        }
    }
    Some(x)
}

fn median3(v: &[[f32; 3]], gw: usize, gh: usize) -> Vec<[f32; 3]> {
    let mut out = v.to_vec();
    out.par_chunks_mut(gw).enumerate().for_each(|(y, row)| {
        let mut nb = [0f32; 9];
        for (x, o) in row.iter_mut().enumerate() {
            for c in 0..3 {
                let mut n = 0;
                for dy in -1isize..=1 {
                    for dx in -1isize..=1 {
                        let (xx, yy) = (x as isize + dx, y as isize + dy);
                        if xx >= 0 && yy >= 0 && (xx as usize) < gw && (yy as usize) < gh {
                            nb[n] = v[yy as usize * gw + xx as usize][c];
                            n += 1;
                        }
                    }
                }
                o[c] = median_of(&mut nb[..n]);
            }
        }
    });
    out
}

/// Value at index `i` of a line of `n` values with odd reflection about the ends
/// (`v[-k] = 2 v[0] - v[k]`), which keeps a linear trend linear across the border where clamping
/// would flatten it and bias every cell within a few sigma of the page edge.
fn odd(at: impl Fn(usize) -> [f32; 3], i: isize, n: usize) -> [f32; 3] {
    let last = n as isize - 1;
    if i < 0 {
        let (a, b) = (at(0), at((-i).min(last) as usize));
        [2.0 * a[0] - b[0], 2.0 * a[1] - b[1], 2.0 * a[2] - b[2]]
    } else if i > last {
        let (a, b) = (at(last as usize), at((2 * last - i).max(0) as usize));
        [2.0 * a[0] - b[0], 2.0 * a[1] - b[1], 2.0 * a[2] - b[2]]
    } else {
        at(i as usize)
    }
}

fn gaussian(g: &mut Grid, sigma: f32) {
    if sigma <= 0.0 {
        return;
    }
    let (gw, gh) = (g.gw, g.gh);
    let r = (sigma * 3.0).ceil() as isize;
    let k: Vec<f32> = (-r..=r)
        .map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp())
        .collect();
    let ks: f32 = k.iter().sum();
    let src = g.v.clone();
    let mut tmp = src.clone();
    tmp.par_chunks_mut(gw).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let mut a = [0f32; 3];
            for (j, kv) in k.iter().enumerate() {
                let xx = x as isize + j as isize - r;
                let s = odd(|i| src[y * gw + i], xx, gw);
                for c in 0..3 {
                    a[c] += kv * s[c];
                }
            }
            *o = [a[0] / ks, a[1] / ks, a[2] / ks];
        }
    });
    g.v.par_chunks_mut(gw).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let mut a = [0f32; 3];
            for (j, kv) in k.iter().enumerate() {
                let yy = y as isize + j as isize - r;
                let s = odd(|i| tmp[i * gw + x], yy, gh);
                for c in 0..3 {
                    a[c] += kv * s[c];
                }
            }
            *o = [a[0] / ks, a[1] / ks, a[2] / ks];
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_pull_fills_a_hole_with_the_surrounding_level() {
        let (gw, gh) = (9, 9);
        let mut v = vec![[200.0f32; 3]; gw * gh];
        let mut valid = vec![true; gw * gh];
        for y in 3..6 {
            for x in 3..6 {
                valid[y * gw + x] = false;
                v[y * gw + x] = [0.0; 3];
            }
        }
        push_pull(&mut v, &valid, gw, gh);
        for y in 3..6 {
            for x in 3..6 {
                assert!((v[y * gw + x][0] - 200.0).abs() < 1.0);
            }
        }
    }

    #[test]
    fn the_quadratic_fill_keeps_a_gradient_through_a_hole() {
        let (gw, gh) = (24, 24);
        let truth = |x: usize, y: usize| 120.0 + 4.0 * x as f32 + 2.0 * y as f32;
        let mut v: Vec<[f32; 3]> = (0..gw * gh).map(|i| [truth(i % gw, i / gw); 3]).collect();
        let mut valid = vec![true; gw * gh];
        for y in 8..16 {
            for x in 8..16 {
                valid[y * gw + x] = false;
                v[y * gw + x] = [0.0; 3];
            }
        }
        push_pull(&mut v, &valid, gw, gh);
        let pp_err = (v[12 * gw + 12][0] - truth(12, 12)).abs();
        quad_fill(&mut v, &valid, gw, gh, 8);
        let q_err = (v[12 * gw + 12][0] - truth(12, 12)).abs();
        assert!(q_err < 1.0, "quadratic error {q_err}");
        assert!(q_err < pp_err + 1e-3, "push-pull {pp_err} vs quad {q_err}");
    }

    #[test]
    fn odd_reflection_keeps_a_linear_ramp_linear() {
        let mut g = Grid {
            gw: 20,
            gh: 1,
            v: (0..20).map(|i| [10.0 * i as f32; 3]).collect(),
        };
        gaussian(&mut g, 2.0);
        for i in 0..20 {
            assert!((g.v[i][0] - 10.0 * i as f32).abs() < 0.5, "cell {i}");
        }
    }
}
