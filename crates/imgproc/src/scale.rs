// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Area-average downscaling (box filter with fractional coverage), used for proxies and thumbnails.

use crate::Raster;
use rayon::prelude::*;

/// Output dimensions that fit `max_edge` on the long side, keeping the aspect ratio (never upscales,
/// never below 1).
pub fn fit_dimensions(width: u32, height: u32, max_edge: u32) -> (u32, u32) {
    let long = width.max(height);
    if long <= max_edge || long == 0 {
        return (width.max(1), height.max(1));
    }
    let s = f64::from(max_edge) / f64::from(long);
    (
        ((f64::from(width) * s).round() as u32).max(1),
        ((f64::from(height) * s).round() as u32).max(1),
    )
}

/// Axis weights: for each output index, the (start, weights) over source indices.
fn axis_weights(src: usize, dst: usize) -> Vec<(usize, Vec<f32>)> {
    let scale = src as f64 / dst as f64;
    (0..dst)
        .map(|i| {
            let (a, b) = (i as f64 * scale, (i as f64 + 1.0) * scale);
            let first = a.floor() as usize;
            let last = ((b.ceil() as usize).max(first + 1)).min(src);
            let mut w = Vec::with_capacity(last - first);
            let mut sum = 0.0;
            for s in first..last {
                let cover = (b.min(s as f64 + 1.0) - a.max(s as f64)).max(0.0);
                w.push(cover as f32);
                sum += cover;
            }
            if sum <= 0.0 {
                w = vec![1.0];
                sum = 1.0;
            }
            for v in &mut w {
                *v /= sum as f32;
            }
            (first.min(src - 1), w)
        })
        .collect()
}

/// Output rows per rayon task. Each task runs the horizontal pass for the source rows its output
/// rows need (a handful of rows are shared with the neighbouring task), so scratch memory is
/// `OUT_BAND * ceil(scale)` rows of `out_w * 3` f32, never a full-frame f32 image.
const OUT_BAND: usize = 8;

/// Resizes to exactly `out_w` x `out_h` with an area average (up- or downscaling; upscaling is
/// nearest-neighbour-like and only used for tiny inputs). Parallel over bands of output rows;
/// the result does not depend on the thread count.
pub fn resize_area(src: &Raster, out_w: u32, out_h: u32) -> Raster {
    if src.width == 0 || src.height == 0 || out_w == 0 || out_h == 0 {
        return Raster::new(out_w, out_h);
    }
    if src.width == out_w && src.height == out_h {
        return src.clone();
    }
    let (sw, sh) = (src.width as usize, src.height as usize);
    let (dw, dh) = (out_w as usize, out_h as usize);
    let wx = axis_weights(sw, dw);
    let wy = axis_weights(sh, dh);
    let mut out = Raster::new(out_w, out_h);
    out.data
        .par_chunks_mut(dw * 3 * OUT_BAND)
        .enumerate()
        .for_each(|(band, chunk)| {
            let y0 = band * OUT_BAND;
            let rows = chunk.len() / (dw * 3);
            // Source rows touched by this band (clamped like the vertical pass clamps).
            let lo = wy[y0].0.min(sh - 1);
            let hi = wy[y0..y0 + rows]
                .iter()
                .map(|(start, w)| (start + w.len() - 1).min(sh - 1))
                .max()
                .unwrap_or(lo);
            // Horizontal pass for source rows lo..=hi into f32.
            let mut tmp = vec![0.0f32; (hi - lo + 1) * dw * 3];
            for (i, trow) in tmp.chunks_exact_mut(dw * 3).enumerate() {
                let y = lo + i;
                let row = &src.data[y * sw * 3..(y + 1) * sw * 3];
                for (x, (start, w)) in wx.iter().enumerate() {
                    let mut acc = [0.0f32; 3];
                    for (k, wk) in w.iter().enumerate() {
                        let p = (start + k).min(sw - 1) * 3;
                        acc[0] += wk * f32::from(row[p]);
                        acc[1] += wk * f32::from(row[p + 1]);
                        acc[2] += wk * f32::from(row[p + 2]);
                    }
                    trow[x * 3..x * 3 + 3].copy_from_slice(&acc);
                }
            }
            // Vertical pass.
            for (r, orow) in chunk.chunks_exact_mut(dw * 3).enumerate() {
                let (start, w) = &wy[y0 + r];
                for x in 0..dw {
                    let mut acc = [0.0f32; 3];
                    for (k, wk) in w.iter().enumerate() {
                        let o = ((start + k).min(sh - 1) - lo) * dw * 3 + x * 3;
                        acc[0] += wk * tmp[o];
                        acc[1] += wk * tmp[o + 1];
                        acc[2] += wk * tmp[o + 2];
                    }
                    for (c, a) in acc.iter().enumerate() {
                        orow[x * 3 + c] = (a + 0.5).clamp(0.0, 255.0) as u8;
                    }
                }
            }
        });
    out
}

/// Downscales to fit `max_edge` on the long side; a copy if it already fits.
pub fn resize_to_fit(src: &Raster, max_edge: u32) -> Raster {
    let (w, h) = fit_dimensions(src.width, src.height, max_edge);
    resize_area(src, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The serial implementation the early engine slice shipped, kept as the bit-exact reference.
    fn resize_area_reference(src: &Raster, out_w: u32, out_h: u32) -> Raster {
        if src.width == 0 || src.height == 0 || out_w == 0 || out_h == 0 {
            return Raster::new(out_w, out_h);
        }
        if src.width == out_w && src.height == out_h {
            return src.clone();
        }
        let (sw, sh) = (src.width as usize, src.height as usize);
        let (dw, dh) = (out_w as usize, out_h as usize);
        let wx = axis_weights(sw, dw);
        let wy = axis_weights(sh, dh);
        let mut tmp = vec![0.0f32; dw * sh * 3];
        for y in 0..sh {
            let row = &src.data[y * sw * 3..(y + 1) * sw * 3];
            for (x, (start, w)) in wx.iter().enumerate() {
                let mut acc = [0.0f32; 3];
                for (k, wk) in w.iter().enumerate() {
                    let p = (start + k).min(sw - 1) * 3;
                    acc[0] += wk * f32::from(row[p]);
                    acc[1] += wk * f32::from(row[p + 1]);
                    acc[2] += wk * f32::from(row[p + 2]);
                }
                let o = (y * dw + x) * 3;
                tmp[o..o + 3].copy_from_slice(&acc);
            }
        }
        let mut out = Raster::new(out_w, out_h);
        for (y, (start, w)) in wy.iter().enumerate() {
            for x in 0..dw {
                let mut acc = [0.0f32; 3];
                for (k, wk) in w.iter().enumerate() {
                    let o = ((start + k).min(sh - 1) * dw + x) * 3;
                    acc[0] += wk * tmp[o];
                    acc[1] += wk * tmp[o + 1];
                    acc[2] += wk * tmp[o + 2];
                }
                let d = (y * dw + x) * 3;
                for (c, a) in acc.iter().enumerate() {
                    out.data[d + c] = (a + 0.5).clamp(0.0, 255.0) as u8;
                }
            }
        }
        out
    }

    fn noise(w: u32, h: u32) -> Raster {
        let mut r = Raster::new(w, h);
        let mut s = 0x1234_5679u32;
        for b in &mut r.data {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            *b = (s >> 7) as u8;
        }
        r
    }

    #[test]
    fn the_parallel_resize_is_byte_identical_to_the_serial_reference() {
        for (sw, sh, dw, dh) in [
            (97, 61, 40, 23),
            (64, 64, 64, 33),
            (301, 7, 100, 7),
            (50, 400, 13, 129),
            (333, 222, 111, 74),
            (10, 10, 25, 31),
            (1, 1, 5, 5),
            (5, 3, 1, 1),
        ] {
            let src = noise(sw, sh);
            assert_eq!(
                resize_area(&src, dw, dh),
                resize_area_reference(&src, dw, dh),
                "{sw}x{sh} -> {dw}x{dh}"
            );
        }
    }

    #[test]
    fn the_resize_is_identical_at_1_and_8_threads() {
        let src = noise(640, 480);
        let run = |n: usize| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(n)
                .build()
                .unwrap()
                .install(|| resize_area(&src, 171, 133))
        };
        assert_eq!(run(1), run(8));
    }

    #[test]
    fn dimensions_keep_the_aspect_and_never_upscale() {
        assert_eq!(fit_dimensions(4000, 3000, 2000), (2000, 1500));
        assert_eq!(fit_dimensions(100, 50, 2000), (100, 50));
        assert_eq!(fit_dimensions(5000, 1, 100), (100, 1));
    }

    #[test]
    fn a_flat_image_stays_flat_and_a_checker_averages_out() {
        let flat = Raster::filled(64, 64, [90, 120, 200]);
        let small = resize_area(&flat, 10, 7);
        assert!(small.data.chunks(3).all(|p| p == [90, 120, 200]));

        let mut checker = Raster::new(8, 8);
        for y in 0..8 {
            for x in 0..8 {
                let v = if (x + y) % 2 == 0 { 0 } else { 255 };
                checker.set_pixel(x, y, [v, v, v]);
            }
        }
        let one = resize_area(&checker, 1, 1);
        assert!((i32::from(one.data[0]) - 128).abs() <= 1);
    }
}
