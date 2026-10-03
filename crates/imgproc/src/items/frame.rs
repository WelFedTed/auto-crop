// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Uniform frames around a picture: a white margin, a scanner border, a platen shadow strip
//! (ROADMAP M10.02). They are cut off before the bed is modelled, so a thin frame does not
//! become "the bed" and the dark table inside it "an item".

use crate::Raster;

/// A row (or column) is frame-like when it is nearly one colour.
const MAX_STD: f32 = 7.0;
/// The most that is ever trimmed from one side.
const MAX_FRAC: f32 = 0.10;

fn line_stats(r: &Raster, px: impl Iterator<Item = [u8; 3]>) -> ([f32; 3], f32) {
    let (mut n, mut sum, mut sq) = (0.0f32, [0.0f32; 3], [0.0f32; 3]);
    for p in px {
        n += 1.0;
        for c in 0..3 {
            let v = f32::from(p[c]);
            sum[c] += v;
            sq[c] += v * v;
        }
    }
    let _ = r;
    let mean = sum.map(|s| s / n.max(1.0));
    let var = (0..3)
        .map(|c| (sq[c] / n.max(1.0) - mean[c] * mean[c]).max(0.0))
        .sum::<f32>()
        / 3.0;
    (mean, var.sqrt())
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// How many rows or columns (at most 10% of the side) at the start of `line(i)` form a uniform
/// frame that differs clearly from what follows. `n` is the side length in lines.
fn depth(
    n: usize,
    line: &dyn Fn(usize) -> ([f32; 3], f32),
    band: &dyn Fn(usize, usize) -> [f32; 3],
) -> usize {
    let limit = ((MAX_FRAC * n as f32) as usize).max(2);
    let (first_mean, first_std) = line(0);
    if first_std > MAX_STD {
        return 0;
    }
    let mut d = 1;
    while d < limit {
        let (m, s) = line(d);
        if s > MAX_STD || dist(m, first_mean) > 12.0 {
            break;
        }
        d += 1;
    }
    if d >= limit {
        // Uniform all the way: that is a bed, not a frame.
        return 0;
    }
    // The next line must differ from the frame (a different colour or not uniform at all).
    let (m, s) = line(d);
    if s <= MAX_STD && dist(m, first_mean) <= 15.0 {
        return 0;
    }
    // What lies beyond must not be the same colour as the frame: a margin of empty bed in front of
    // some items is no frame, a white margin around a dark table is.
    let beyond = band(d, (d + (0.08 * n as f32) as usize + 4).min(n));
    if dist(beyond, first_mean) < 25.0 {
        return 0;
    }
    d
}

/// The depth of the uniform frame on each side: top, right, bottom, left (0 = none). Frames
/// shallower than 2 px are ignored.
pub fn find(r: &Raster) -> [usize; 4] {
    let (w, h) = (r.width as usize, r.height as usize);
    if w < 64 || h < 64 {
        return [0; 4];
    }
    let px = |x: usize, y: usize| r.pixel(x as u32, y as u32);
    let median3 = |mut v: [Vec<u8>; 3]| -> [f32; 3] {
        std::array::from_fn(|c| {
            v[c].sort_unstable();
            v[c].get(v[c].len() / 2).map_or(0.0, |x| f32::from(*x))
        })
    };
    let band_rows = |from_top: bool| {
        move |a: usize, b: usize| -> [f32; 3] {
            let mut v: [Vec<u8>; 3] = Default::default();
            for i in a..b {
                let y = if from_top { i } else { h - 1 - i };
                for x in (0..w).step_by(2) {
                    let p = px(x, y);
                    (0..3).for_each(|c| v[c].push(p[c]));
                }
            }
            median3(v)
        }
    };
    let band_cols = |from_left: bool| {
        move |a: usize, b: usize| -> [f32; 3] {
            let mut v: [Vec<u8>; 3] = Default::default();
            for i in a..b {
                let x = if from_left { i } else { w - 1 - i };
                for y in (0..h).step_by(2) {
                    let p = px(x, y);
                    (0..3).for_each(|c| v[c].push(p[c]));
                }
            }
            median3(v)
        }
    };
    let top = depth(
        h,
        &|i| line_stats(r, (0..w).map(|x| px(x, i))),
        &band_rows(true),
    );
    let bottom = depth(
        h,
        &|i| line_stats(r, (0..w).map(|x| px(x, h - 1 - i))),
        &band_rows(false),
    );
    let left = depth(
        w,
        &|i| line_stats(r, (0..h).map(|y| px(i, y))),
        &band_cols(true),
    );
    let right = depth(
        w,
        &|i| line_stats(r, (0..h).map(|y| px(w - 1 - i, y))),
        &band_cols(false),
    );
    let mut t = [top, right, bottom, left];
    // A real frame is at least 1.2% of its side (a few pixels of bed above a print are not one),
    // unless three sides have one.
    let sides = [h, w, h, w];
    let raw = t;
    let strong = |i: usize| raw[i] as f32 >= 0.012 * sides[i] as f32 && raw[i] >= 3;
    let n_frames = t.iter().filter(|v| **v >= 2).count();
    #[allow(clippy::needless_range_loop)]
    for i in 0..4 {
        if t[i] < 2 || (n_frames < 3 && !strong(i)) {
            t[i] = 0;
        } else {
            t[i] += 1; // one more line, to clear the blurred inner edge of the frame
        }
    }
    t
}

pub fn crop(r: &Raster, x0: usize, y0: usize, w: usize, h: usize) -> Raster {
    let mut out = Raster::new(w as u32, h as u32);
    for y in 0..h {
        let src = ((y0 + y) * r.width as usize + x0) * 3;
        let dst = y * w * 3;
        out.data[dst..dst + w * 3].copy_from_slice(&r.data[src..src + w * 3]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_white_margin_around_a_dark_picture_is_found_and_a_plain_bed_is_not() {
        let mut r = Raster::filled(200, 150, [250, 250, 250]);
        for y in 6..144 {
            for x in 4..194 {
                let v = 20 + ((x * 7 + y * 13) % 30) as u8;
                r.set_pixel(x as u32, y as u32, [v, v, v]);
            }
        }
        let t = find(&r);
        assert!(
            t[0] >= 6 && t[0] <= 8 && t[1] >= 6 && t[1] <= 8 && t[2] >= 6 && t[3] >= 4,
            "{t:?}"
        );
        let plain = Raster::filled(200, 150, [240, 240, 240]);
        assert_eq!(find(&plain), [0; 4]);
        let c = crop(&r, t[3], t[0], 200 - t[1] - t[3], 150 - t[0] - t[2]);
        assert!(c.pixel(0, 0)[0] < 100);
    }
}
