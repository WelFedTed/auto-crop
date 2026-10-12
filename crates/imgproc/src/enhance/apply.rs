// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Applying the gain field to pixels, in bands: the illumination flatten (PLAN 5.3.1 step 4),
//! the tone LUT, the strength blend and the grey conversion that feeds the threshold.
//!
//! The gain field is a coarse grid, never a full-resolution map. For each output row the grid is
//! interpolated vertically once (one gain per grid column), then each pixel interpolates
//! horizontally between two neighbours, so the cost is a few multiplications per pixel. Rows are
//! independent and the arithmetic of a row does not depend on the band it belongs to, so the
//! bytes do not depend on the thread count or the band height.

use super::illum::Grid;
use crate::cancel::{Cancel, Cancelled};
use rayon::prelude::*;

/// Output rows per rayon task and per cancel check (PLAN 2.8: every 64 rows).
pub(crate) const BAND_ROWS: usize = 64;

/// Per-channel multiplicative gain on a coarse grid, shared by every image size: a pixel at
/// `(x, y)` of a `W x H` image reads the field at `u = (x + 0.5) / W`, `v = (y + 0.5) / H`.
#[derive(Debug, Clone)]
pub struct GainField {
    pub(crate) gw: usize,
    pub(crate) gh: usize,
    pub(crate) g: Vec<[f32; 3]>,
    /// Largest value the field can reach after interpolation.
    pub(crate) cap: f32,
}

/// `(index, t)` such that the value at `pos` is `a[index] * (1 - t) + a[index + 1] * t`; `t` may
/// leave `0..1` so the outermost cells continue linearly (extrapolation).
fn tap(pos: f32, n: usize) -> (usize, f32) {
    if n < 2 {
        return (0, 0.0);
    }
    let i = pos.floor().clamp(0.0, (n - 2) as f32) as usize;
    (i, pos - i as f32)
}

impl GainField {
    /// A field of constant gain 1 (no flattening).
    pub fn identity() -> Self {
        Self {
            gw: 1,
            gh: 1,
            g: vec![[1.0; 3]],
            cap: 1.0,
        }
    }

    /// The per-cell gain `clamp(target / paper, 1, cap)` of a smoothed paper grid.
    pub(crate) fn from_paper(paper: &Grid, target: f32, cap: f32, luma_only: bool) -> Self {
        let g = paper
            .v
            .iter()
            .map(|p| {
                let p = if luma_only {
                    let y = 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2];
                    [y, y, y]
                } else {
                    *p
                };
                [
                    (target / p[0].max(1.0)).clamp(1.0, cap),
                    (target / p[1].max(1.0)).clamp(1.0, cap),
                    (target / p[2].max(1.0)).clamp(1.0, cap),
                ]
            })
            .collect();
        Self {
            gw: paper.gw,
            gh: paper.gh,
            g,
            cap,
        }
    }

    /// The product of two fields on the same grid (a second estimate-and-apply round).
    pub(crate) fn times(&self, other: &GainField) -> GainField {
        debug_assert_eq!((self.gw, self.gh), (other.gw, other.gh));
        let g = self
            .g
            .iter()
            .zip(&other.g)
            .map(|(a, b)| {
                [
                    (a[0] * b[0]).min(self.cap),
                    (a[1] * b[1]).min(self.cap),
                    (a[2] * b[2]).min(self.cap),
                ]
            })
            .collect();
        GainField {
            gw: self.gw,
            gh: self.gh,
            g,
            cap: self.cap,
        }
    }

    pub fn is_identity(&self) -> bool {
        self.g.iter().all(|g| *g == [1.0; 3])
    }

    pub fn max_gain(&self) -> f32 {
        self.g
            .iter()
            .flat_map(|g| g.iter().copied())
            .fold(1.0, f32::max)
    }

    /// Column taps for an image `width` pixels wide.
    fn x_taps(&self, width: usize) -> Vec<(u32, f32)> {
        (0..width)
            .map(|x| {
                let pos = (x as f32 + 0.5) * self.gw as f32 / width as f32 - 0.5;
                let (i, t) = tap(pos, self.gw);
                (i as u32, t)
            })
            .collect()
    }

    /// The gains of row `y` of a `height`-row image at every grid column (plus one repeated
    /// column so the last tap always has a neighbour).
    fn row(&self, y: usize, height: usize, out: &mut Vec<[f32; 3]>) {
        let pos = (y as f32 + 0.5) * self.gh as f32 / height as f32 - 0.5;
        let (j, t) = tap(pos, self.gh);
        let j1 = (j + 1).min(self.gh - 1);
        out.clear();
        for gx in 0..self.gw {
            let a = self.g[j * self.gw + gx];
            let b = self.g[j1 * self.gw + gx];
            out.push([
                a[0] * (1.0 - t) + b[0] * t,
                a[1] * (1.0 - t) + b[1] * t,
                a[2] * (1.0 - t) + b[2] * t,
            ]);
        }
        let last = *out.last().expect("non-empty");
        out.push(last);
    }
}

/// The three channel gains at one pixel from the row vector and the column tap.
#[inline]
fn gains_at(row: &[[f32; 3]], tap: (u32, f32), cap: f32) -> [f32; 3] {
    let (i, t) = (tap.0 as usize, tap.1);
    let (a, b) = (row[i], row[i + 1]);
    [
        (a[0] * (1.0 - t) + b[0] * t).clamp(1.0, cap),
        (a[1] * (1.0 - t) + b[1] * t).clamp(1.0, cap),
        (a[2] * (1.0 - t) + b[2] * t).clamp(1.0, cap),
    ]
}

#[inline]
fn flat(v: u8, g: f32) -> u8 {
    // `as u8` saturates at 255.
    (f32::from(v) * g + 0.5) as u8
}

/// Rec. 709 luma of an 8-bit pixel with integer weights summing to 256.
#[inline]
pub(crate) fn luma709(p: [u8; 3]) -> u8 {
    ((54 * u32::from(p[0]) + 183 * u32::from(p[1]) + 19 * u32::from(p[2]) + 128) >> 8) as u8
}

/// `t` (0..=1) as a blend weight out of 256 (1.0 is exactly 256: the enhanced value).
pub(crate) fn weight256(strength: f32) -> i32 {
    (strength.clamp(0.0, 1.0) * 256.0).round() as i32
}

#[inline]
fn blend(orig: u8, enh: u8, w: i32) -> u8 {
    if w >= 256 {
        return enh;
    }
    let d = i32::from(enh) - i32::from(orig);
    (i32::from(orig) + ((d * w + if d >= 0 { 128 } else { -128 }) / 256)).clamp(0, 255) as u8
}

/// Flatten + tone LUT + blend, in place on an RGB buffer (Clean mode).
pub(crate) fn clean_in_place(
    rgb: &mut [u8],
    w: usize,
    h: usize,
    field: &GainField,
    lut: &[u8; 256],
    weight: i32,
    cancel: &dyn Cancel,
) -> Result<(), Cancelled> {
    let taps = field.x_taps(w);
    rgb.par_chunks_mut(w * 3 * BAND_ROWS)
        .enumerate()
        .for_each(|(band, chunk)| {
            if cancel.is_cancelled() {
                return;
            }
            let y0 = band * BAND_ROWS;
            let mut grow = Vec::with_capacity(field.gw + 1);
            for (r, line) in chunk.chunks_exact_mut(w * 3).enumerate() {
                field.row(y0 + r, h, &mut grow);
                for (x, px) in line.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                    let g = gains_at(&grow, taps[x], field.cap);
                    for c in 0..3 {
                        let enh = lut[usize::from(flat(px[c], g[c]))];
                        px[c] = blend(px[c], enh, weight);
                    }
                }
            }
        });
    if cancel.is_cancelled() {
        Err(Cancelled)
    } else {
        Ok(())
    }
}

/// Flattened luma plane (for the threshold), through `lut`. Pixels outside `valid` are white.
/// `valid` is `(mask, mask_width, mask_height)`.
pub(crate) fn flat_luma(
    rgb: &[u8],
    w: usize,
    h: usize,
    field: &GainField,
    lut: &[u8; 256],
    valid: Option<(&[u8], usize, usize)>,
    cancel: &dyn Cancel,
) -> Result<Vec<u8>, Cancelled> {
    let taps = field.x_taps(w);
    let mut luma = vec![0u8; w * h];
    luma.par_chunks_mut(w * BAND_ROWS)
        .enumerate()
        .for_each(|(band, chunk)| {
            if cancel.is_cancelled() {
                return;
            }
            let y0 = band * BAND_ROWS;
            let mut grow = Vec::with_capacity(field.gw + 1);
            for (r, out) in chunk.chunks_exact_mut(w).enumerate() {
                let y = y0 + r;
                field.row(y, h, &mut grow);
                let src = &rgb[y * w * 3..(y + 1) * w * 3];
                let mrow = valid.map(|(m, mw, mh)| {
                    let my = (2 * y + 1) * mh / (2 * h);
                    &m[my * mw..(my + 1) * mw]
                });
                for (x, o) in out.iter_mut().enumerate() {
                    if let (Some(row), Some((_, mw, _))) = (mrow, valid)
                        && row[(2 * x + 1) * mw / (2 * w)] == 0
                    {
                        *o = 255;
                        continue;
                    }
                    let g = gains_at(&grow, taps[x], field.cap);
                    let p = [
                        flat(src[x * 3], g[0]),
                        flat(src[x * 3 + 1], g[1]),
                        flat(src[x * 3 + 2], g[2]),
                    ];
                    *o = lut[usize::from(luma709(p))];
                }
            }
        });
    if cancel.is_cancelled() {
        Err(Cancelled)
    } else {
        Ok(luma)
    }
}

/// Final pass of a thresholded mode: blends with the original luma (strength below 1) and whitens
/// the pixels outside the page. `rgb` is the original image.
pub(crate) fn finish_gray(
    out: &mut [u8],
    rgb: Option<&[u8]>,
    w: usize,
    h: usize,
    weight: i32,
    valid: Option<(&[u8], usize, usize)>,
    cancel: &dyn Cancel,
) -> Result<(), Cancelled> {
    if rgb.is_none() && valid.is_none() {
        return Ok(());
    }
    out.par_chunks_mut(w * BAND_ROWS)
        .enumerate()
        .for_each(|(band, chunk)| {
            if cancel.is_cancelled() {
                return;
            }
            let y0 = band * BAND_ROWS;
            for (r, line) in chunk.chunks_exact_mut(w).enumerate() {
                let y = y0 + r;
                let mrow = valid.map(|(m, mw, mh)| {
                    let my = (2 * y + 1) * mh / (2 * h);
                    &m[my * mw..(my + 1) * mw]
                });
                for (x, o) in line.iter_mut().enumerate() {
                    if let (Some(row), Some((_, mw, _))) = (mrow, valid)
                        && row[(2 * x + 1) * mw / (2 * w)] == 0
                    {
                        *o = 255;
                        continue;
                    }
                    if let Some(src) = rgb {
                        let i = (y * w + x) * 3;
                        let orig = luma709([src[i], src[i + 1], src[i + 2]]);
                        *o = blend(orig, *o, weight);
                    }
                }
            }
        });
    if cancel.is_cancelled() {
        Err(Cancelled)
    } else {
        Ok(())
    }
}

/// Applies a gain field to a small image (the proxy), returning the flattened copy.
pub(crate) fn flatten_copy(
    rgb: &[u8],
    luma_out: &mut Vec<u8>,
    w: usize,
    h: usize,
    field: &GainField,
) -> Vec<u8> {
    let taps = field.x_taps(w);
    let mut out = vec![0u8; w * h * 3];
    luma_out.clear();
    luma_out.resize(w * h, 0);
    out.par_chunks_mut(w * 3)
        .zip(luma_out.par_chunks_mut(w))
        .enumerate()
        .for_each(|(y, (line, lrow))| {
            let mut grow = Vec::with_capacity(field.gw + 1);
            field.row(y, h, &mut grow);
            for x in 0..w {
                let g = gains_at(&grow, taps[x], field.cap);
                let i = (y * w + x) * 3;
                let p = [
                    flat(rgb[i], g[0]),
                    flat(rgb[i + 1], g[1]),
                    flat(rgb[i + 2], g[2]),
                ];
                line[x * 3..x * 3 + 3].copy_from_slice(&p);
                lrow[x] = luma709(p);
            }
        });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cancel::NeverCancel;

    #[test]
    fn taps_extrapolate_linearly_beyond_the_outer_cells() {
        // 4 cells over 100 px: centres at 12.5, 37.5, 62.5, 87.5.
        let f = GainField {
            gw: 4,
            gh: 1,
            g: vec![[1.0; 3], [2.0; 3], [3.0; 3], [4.0; 3]],
            cap: 10.0,
        };
        let taps = f.x_taps(100);
        let mut row = Vec::new();
        f.row(0, 1, &mut row);
        let at = |x: usize| gains_at(&row, taps[x], 10.0)[0];
        assert!((at(37) - 2.0).abs() < 0.03, "{}", at(37));
        assert!((at(50) - 2.5).abs() < 0.03, "{}", at(50));
        // The outer half cell continues the ramp (slope 1 per 25 px).
        assert!((at(0) - 0.5).abs() < 0.05 || at(0) == 1.0, "{}", at(0));
        assert!((at(99) - 4.5).abs() < 0.05, "{}", at(99));
    }

    #[test]
    fn a_constant_field_scales_every_pixel_and_unit_gain_is_the_identity() {
        let (w, h) = (37, 21);
        let img: Vec<u8> = (0..w * h * 3).map(|i| (i % 251) as u8).collect();
        let lut: [u8; 256] = std::array::from_fn(|i| i as u8);
        let mut a = img.clone();
        clean_in_place(
            &mut a,
            w,
            h,
            &GainField::identity(),
            &lut,
            256,
            &NeverCancel,
        )
        .unwrap();
        assert_eq!(a, img);
        let f = GainField {
            gw: 1,
            gh: 1,
            g: vec![[2.0, 1.0, 1.5]],
            cap: 3.0,
        };
        let mut b = img.clone();
        clean_in_place(&mut b, w, h, &f, &lut, 256, &NeverCancel).unwrap();
        for (i, (x, y)) in img.iter().zip(&b).enumerate() {
            let g = [2.0, 1.0, 1.5][i % 3];
            assert_eq!(*y, (f32::from(*x) * g + 0.5).min(255.0) as u8);
        }
        // Half strength lands half way.
        let mut c = img.clone();
        clean_in_place(&mut c, w, h, &f, &lut, 128, &NeverCancel).unwrap();
        for i in 0..img.len() {
            let want = (i32::from(img[i]) + i32::from(b[i])) / 2;
            assert!((i32::from(c[i]) - want).abs() <= 1);
        }
    }

    #[test]
    fn blend_is_exact_at_the_ends() {
        for o in [0u8, 1, 100, 254, 255] {
            for e in [0u8, 7, 128, 255] {
                assert_eq!(blend(o, e, 256), e);
                assert_eq!(blend(o, e, 0), o);
            }
        }
    }
}
