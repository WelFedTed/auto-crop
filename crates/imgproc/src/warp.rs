// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Strip-wise Lanczos3 perspective warp on u8 RGB (ADR-0002): 64-row rayon strips, a Lanczos3
//! weight table, f32 accumulation, no full-frame f32 buffer. Integer-centre convention: pixel
//! `(i, j)` has its centre at `(i, j)`. Output outside the source is black.

use crate::Raster;
use rayon::prelude::*;
use std::sync::OnceLock;

const STRIP: usize = 64;
const LUT_N: usize = 1024;

fn lanczos3(x: f64) -> f64 {
    if x == 0.0 {
        return 1.0;
    }
    if x.abs() >= 3.0 {
        return 0.0;
    }
    let px = std::f64::consts::PI * x;
    3.0 * px.sin() * (px / 3.0).sin() / (px * px)
}

fn lut() -> &'static [[f32; 6]] {
    static LUT: OnceLock<Vec<[f32; 6]>> = OnceLock::new();
    LUT.get_or_init(|| {
        (0..LUT_N)
            .map(|i| {
                let f = i as f64 / LUT_N as f64;
                let mut w = [0.0f64; 6];
                let mut sum = 0.0;
                for (k, wk) in w.iter_mut().enumerate() {
                    *wk = lanczos3(f - (k as f64 - 2.0));
                    sum += *wk;
                }
                let mut out = [0.0f32; 6];
                for k in 0..6 {
                    out[k] = (w[k] / sum) as f32;
                }
                out
            })
            .collect()
    })
}

/// Resamples `src` through `dst_to_src` (a homography from output pixel centres to source pixel
/// centres) into a new `out_w` x `out_h` image. Returns an all-black image for an empty source.
pub fn warp_perspective(src: &Raster, dst_to_src: &[f64; 9], out_w: u32, out_h: u32) -> Raster {
    let mut out = Raster::new(out_w, out_h);
    let (sw, sh) = (src.width as usize, src.height as usize);
    if sw == 0 || sh == 0 || out_w == 0 || out_h == 0 {
        return out;
    }
    let lut = lut();
    let h: [f32; 9] = std::array::from_fn(|i| dst_to_src[i] as f32);
    let dw = out_w as usize;
    let data = &src.data;
    out.data
        .par_chunks_mut(dw * 3 * STRIP)
        .enumerate()
        .for_each(|(strip, chunk)| {
            let v0 = strip * STRIP;
            for (r, row) in chunk.chunks_mut(dw * 3).enumerate() {
                let vf = (v0 + r) as f32;
                let (nx0, ny0, nd0) = (h[1] * vf + h[2], h[4] * vf + h[5], h[7] * vf + h[8]);
                for u in 0..dw {
                    let uf = u as f32;
                    let d = h[6] * uf + nd0;
                    let x = (h[0] * uf + nx0) / d;
                    let y = (h[3] * uf + ny0) / d;
                    // Half a pixel of slack so edge pixels sample cleanly.
                    if !(x >= -0.5 && x < sw as f32 - 0.5 && y >= -0.5 && y < sh as f32 - 0.5) {
                        continue;
                    }
                    let (xf, yf) = (x.floor(), y.floor());
                    let wx = &lut[(((x - xf) * LUT_N as f32) as usize).min(LUT_N - 1)];
                    let wy = &lut[(((y - yf) * LUT_N as f32) as usize).min(LUT_N - 1)];
                    let (x0, y0) = (xf as i64, yf as i64);
                    let mut xi = [0usize; 6];
                    for (k, v) in xi.iter_mut().enumerate() {
                        *v = ((x0 + k as i64 - 2).clamp(0, sw as i64 - 1)) as usize * 3;
                    }
                    let mut acc = [0.0f32; 3];
                    for (k, wyv) in wy.iter().enumerate() {
                        let yi = ((y0 + k as i64 - 2).clamp(0, sh as i64 - 1)) as usize;
                        let base = yi * sw * 3;
                        let (mut r0, mut r1, mut r2) = (0.0f32, 0.0f32, 0.0f32);
                        for (j, wxv) in wx.iter().enumerate() {
                            let p = base + xi[j];
                            r0 = wxv.mul_add(f32::from(data[p]), r0);
                            r1 = wxv.mul_add(f32::from(data[p + 1]), r1);
                            r2 = wxv.mul_add(f32::from(data[p + 2]), r2);
                        }
                        acc[0] = wyv.mul_add(r0, acc[0]);
                        acc[1] = wyv.mul_add(r1, acc[1]);
                        acc[2] = wyv.mul_add(r2, acc[2]);
                    }
                    let o = u * 3;
                    row[o] = (acc[0] + 0.5).clamp(0.0, 255.0) as u8;
                    row[o + 1] = (acc[1] + 0.5).clamp(0.0, 255.0) as u8;
                    row[o + 2] = (acc[2] + 0.5).clamp(0.0, 255.0) as u8;
                }
            }
        });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: u32, h: u32) -> Raster {
        let mut r = Raster::new(w, h);
        for y in 0..h {
            for x in 0..w {
                r.set_pixel(x, y, [(x * 255 / w) as u8, (y * 255 / h) as u8, 90]);
            }
        }
        r
    }

    #[test]
    fn identity_warp_reproduces_the_source() {
        let src = gradient(40, 30);
        let id = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
        let out = warp_perspective(&src, &id, 40, 30);
        let max_diff = out
            .data
            .iter()
            .zip(&src.data)
            .map(|(a, b)| (i32::from(*a) - i32::from(*b)).abs())
            .max()
            .unwrap();
        assert!(max_diff <= 1, "max diff {max_diff}");
    }

    #[test]
    fn an_integer_shift_moves_pixels() {
        let src = gradient(40, 30);
        // Output (u, v) samples source (u + 5, v + 3).
        let shift = [1.0, 0.0, 5.0, 0.0, 1.0, 3.0, 0.0, 0.0, 1.0];
        let out = warp_perspective(&src, &shift, 20, 20);
        let (a, b) = (out.pixel(4, 4), src.pixel(9, 7));
        for c in 0..3 {
            assert!((i32::from(a[c]) - i32::from(b[c])).abs() <= 1);
        }
    }

    #[test]
    fn outside_the_source_is_black_and_empty_inputs_do_not_panic() {
        let src = Raster::filled(8, 8, [200, 200, 200]);
        let far = [1.0, 0.0, 1000.0, 0.0, 1.0, 1000.0, 0.0, 0.0, 1.0];
        assert!(
            warp_perspective(&src, &far, 4, 4)
                .data
                .iter()
                .all(|b| *b == 0)
        );
        let id = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
        assert_eq!(
            warp_perspective(&Raster::new(0, 0), &id, 4, 4).data.len(),
            48
        );
    }
}
