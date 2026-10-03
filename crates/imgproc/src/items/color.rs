// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Lab planes, Gaussian blur and gradient fields on the detection proxy.

use crate::Raster;

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
    let (mut l, mut a, mut b) = (vec![0.0; w * h], vec![0.0; w * h], vec![0.0; w * h]);
    for (i, px) in r.data.as_chunks::<3>().0.iter().enumerate() {
        let (rl, gl, bl) = (
            lut[px[0] as usize],
            lut[px[1] as usize],
            lut[px[2] as usize],
        );
        let x = (0.412_456_4 * rl + 0.357_576_1 * gl + 0.180_437_5 * bl) / 0.950_47;
        let y = 0.212_672_9 * rl + 0.715_152_2 * gl + 0.072_175 * bl;
        let z = (0.019_333_9 * rl + 0.119_192 * gl + 0.950_304_1 * bl) / 1.088_83;
        let (fx, fy, fz) = (f_lab(x), f_lab(y), f_lab(z));
        l[i] = 116.0 * fy - 16.0;
        a[i] = 500.0 * (fx - fy);
        b[i] = 200.0 * (fy - fz);
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
        Lab {
            w: self.w,
            h: self.h,
            l: blur(&self.l, self.w, self.h, sigma),
            a: blur(&self.a, self.w, self.h, sigma),
            b: blur(&self.b, self.w, self.h, sigma),
        }
    }

    /// Gradient magnitude of the three planes together (central differences; Lab units per pixel).
    pub fn gradient(&self) -> Vec<f32> {
        let (w, h) = (self.w, self.h);
        let mut g = vec![0.0f32; w * h];
        let at = |p: &[f32], x: usize, y: usize| p[y * w + x];
        for y in 0..h {
            let (y0, y1) = (y.saturating_sub(1), (y + 1).min(h - 1));
            for x in 0..w {
                let (x0, x1) = (x.saturating_sub(1), (x + 1).min(w - 1));
                let mut s = 0.0f32;
                for p in [&self.l, &self.a, &self.b] {
                    let dx = (at(p, x1, y) - at(p, x0, y)) / (x1 - x0).max(1) as f32;
                    let dy = (at(p, x, y1) - at(p, x, y0)) / (y1 - y0).max(1) as f32;
                    s += dx * dx + dy * dy;
                }
                g[y * w + x] = s.sqrt();
            }
        }
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
