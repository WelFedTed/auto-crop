// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Lab planes, Gaussian blur and gradient fields on the detection proxy.

use crate::Raster;
use rayon::prelude::*;

/// CIE Lab (D65) planes of an image, row-major, one `f32` per pixel.
pub struct Lab {
    pub w: usize,
    pub h: usize,
    pub l: Vec<f32>,
    pub a: Vec<f32>,
    pub b: Vec<f32>,
}

fn srgb_lut() -> [f32; 256] {
    let mut t = [0.0f32; 256];
    for (i, v) in t.iter_mut().enumerate() {
        let c = i as f32 / 255.0;
        *v = if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        };
    }
    t
}

fn f_lab(t: f32) -> f32 {
    if t > 0.008_856 {
        t.cbrt()
    } else {
        7.787 * t + 16.0 / 116.0
    }
}

pub fn to_lab(r: &Raster) -> Lab {
    let (w, h) = (r.width as usize, r.height as usize);
    let lut = srgb_lut();
    // The cube root as a table (4096 steps over 0..1.2, linear in between): the same Lab within
    // 0.01, several times faster than three `cbrt` calls per pixel.
    let table: Vec<f32> = (0..=4097)
        .map(|i| f_lab(i as f32 * (1.2 / 4096.0)))
        .collect();
    let flut = |t: f32| -> f32 {
        let u = (t.clamp(0.0, 1.2) * (4096.0 / 1.2)).min(4096.0);
        let i = u as usize;
        let f = u - i as f32;
        table[i] + (table[i + 1] - table[i]) * f
    };
    let px = r.data.as_chunks::<3>().0;
    let lab: Vec<[f32; 3]> = px
        .par_chunks((w * 8).max(1))
        .flat_map_iter(|chunk| {
            chunk.iter().map(|px| {
                let (rl, gl, bl) = (
                    lut[px[0] as usize],
                    lut[px[1] as usize],
                    lut[px[2] as usize],
                );
                let x = (0.412_456_4 * rl + 0.357_576_1 * gl + 0.180_437_5 * bl) / 0.950_47;
                let y = 0.212_672_9 * rl + 0.715_152_2 * gl + 0.072_175 * bl;
                let z = (0.019_333_9 * rl + 0.119_192 * gl + 0.950_304_1 * bl) / 1.088_83;
                let (fx, fy, fz) = (flut(x), flut(y), flut(z));
                [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
            })
        })
        .collect();
    let (mut l, mut a, mut b) = (
        Vec::with_capacity(w * h),
        Vec::with_capacity(w * h),
        Vec::with_capacity(w * h),
    );
    for p in &lab {
        l.push(p[0]);
        a.push(p[1]);
        b.push(p[2]);
    }
    Lab { w, h, l, a, b }
}

/// Separable Gaussian blur with replicated borders.
pub fn blur(src: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    if sigma < 0.2 {
        return src.to_vec();
    }
    let radius = (sigma * 3.0).ceil() as usize;
    let mut k: Vec<f32> = (0..=2 * radius)
        .map(|i| {
            let d = i as f32 - radius as f32;
            (-(d * d) / (2.0 * sigma * sigma)).exp()
        })
        .collect();
    let s: f32 = k.iter().sum();
    for v in &mut k {
        *v /= s;
    }
    let mut tmp = vec![0.0f32; w * h];
    let mut pad = vec![0.0f32; w + 2 * radius];
    for y in 0..h {
        let row = &src[y * w..(y + 1) * w];
        pad[..radius].fill(row[0]);
        pad[radius..radius + w].copy_from_slice(row);
        pad[radius + w..].fill(row[w - 1]);
        let out = &mut tmp[y * w..(y + 1) * w];
        for (j, kv) in k.iter().enumerate() {
            for (o, v) in out.iter_mut().zip(&pad[j..j + w]) {
                *o += kv * v;
            }
        }
    }
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        let dst = &mut out[y * w..(y + 1) * w];
        for (j, kv) in k.iter().enumerate() {
            let yy = (y + j).saturating_sub(radius).min(h - 1);
            for (o, v) in dst.iter_mut().zip(&tmp[yy * w..(yy + 1) * w]) {
                *o += kv * v;
            }
        }
    }
    out
}

impl Lab {
    pub fn blurred(&self, sigma: f32) -> Lab {
        let (w, h) = (self.w, self.h);
        let (l, (a, b)) = rayon::join(
            || blur(&self.l, w, h, sigma),
            || rayon::join(|| blur(&self.a, w, h, sigma), || blur(&self.b, w, h, sigma)),
        );
        Lab { w, h, l, a, b }
    }

    /// Gradient magnitude at a coarse scale: the planes are halved, blurred at `sigma` (in
    /// full-resolution pixels), differentiated and brought back up. About four times cheaper than
    /// blurring at full size, and the scale is wide enough that nothing is lost.
    pub fn coarse_gradient(&self, sigma: f32) -> Vec<f32> {
        let (w, h) = (self.w, self.h);
        let (hw, hh) = (w.div_ceil(2), h.div_ceil(2));
        let half = |p: &[f32]| -> Vec<f32> {
            let mut o = vec![0.0f32; hw * hh];
            for y in 0..hh {
                let (y0, y1) = (2 * y, (2 * y + 1).min(h - 1));
                for x in 0..hw {
                    let (x0, x1) = (2 * x, (2 * x + 1).min(w - 1));
                    o[y * hw + x] =
                        0.25 * (p[y0 * w + x0] + p[y0 * w + x1] + p[y1 * w + x0] + p[y1 * w + x1]);
                }
            }
            o
        };
        let small = Lab {
            w: hw,
            h: hh,
            l: half(&self.l),
            a: half(&self.a),
            b: half(&self.b),
        };
        let g = small.blurred(sigma / 2.0).gradient();
        // The half-size gradient is per half-size pixel: halve it for per-pixel units.
        let mut out = vec![0.0f32; w * h];
        out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            let sy = (y / 2).min(hh - 1);
            for (x, o) in row.iter_mut().enumerate() {
                *o = 0.5 * g[sy * hw + (x / 2).min(hw - 1)];
            }
        });
        out
    }

    /// Gradient magnitude of the three planes together (central differences; Lab units per pixel).
    pub fn gradient(&self) -> Vec<f32> {
        let (w, h) = (self.w, self.h);
        let mut g = vec![0.0f32; w * h];
        let at = |p: &[f32], x: usize, y: usize| p[y * w + x];
        g.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            let (y0, y1) = (y.saturating_sub(1), (y + 1).min(h - 1));
            for (x, out) in row.iter_mut().enumerate() {
                let (x0, x1) = (x.saturating_sub(1), (x + 1).min(w - 1));
                let mut s = 0.0f32;
                for p in [&self.l, &self.a, &self.b] {
                    let dx = (at(p, x1, y) - at(p, x0, y)) / (x1 - x0).max(1) as f32;
                    let dy = (at(p, x, y1) - at(p, x, y0)) / (y1 - y0).max(1) as f32;
                    s += dx * dx + dy * dy;
                }
                *out = s.sqrt();
            }
        });
        g
    }

    /// Bilinear sample of one plane.
    pub fn sample(plane: &[f32], w: usize, h: usize, x: f64, y: f64) -> f32 {
        let x = x.clamp(0.0, (w - 1) as f64);
        let y = y.clamp(0.0, (h - 1) as f64);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (fx, fy) = ((x - x0 as f64) as f32, (y - y0 as f64) as f32);
        let top = plane[y0 * w + x0] * (1.0 - fx) + plane[y0 * w + x1] * fx;
        let bot = plane[y1 * w + x0] * (1.0 - fx) + plane[y1 * w + x1] * fx;
        top * (1.0 - fy) + bot * fy
    }
}

/// CIE76 colour difference.
pub fn de76(p: [f32; 3], q: [f32; 3]) -> f32 {
    let (a, b, c) = (p[0] - q[0], p[1] - q[1], p[2] - q[2]);
    (a * a + b * b + c * c).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn white_is_l100_and_black_is_l0() {
        let mut r = Raster::new(2, 1);
        r.set_pixel(0, 0, [255, 255, 255]);
        let lab = to_lab(&r);
        assert!((lab.l[0] - 100.0).abs() < 0.1 && lab.a[0].abs() < 0.2 && lab.b[0].abs() < 0.3);
        assert!(lab.l[1].abs() < 0.1);
    }

    #[test]
    fn blur_keeps_a_constant_image_and_gradient_finds_a_step() {
        let (w, h) = (20, 10);
        let flat = vec![7.0f32; w * h];
        assert!(
            blur(&flat, w, h, 2.0)
                .iter()
                .all(|v| (v - 7.0).abs() < 1e-4)
        );
        let mut lab = Lab {
            w,
            h,
            l: vec![0.0; w * h],
            a: vec![0.0; w * h],
            b: vec![0.0; w * h],
        };
        for y in 0..h {
            for x in 10..w {
                lab.l[y * w + x] = 50.0;
            }
        }
        let g = lab.gradient();
        assert!(g[5 * w + 10] > 20.0 && g[5 * w + 3] == 0.0);
    }
}
