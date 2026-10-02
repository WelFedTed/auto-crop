// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Shared helpers for the benchmarks: deterministic test images and quality metrics.

use auto_crop_imgproc::Raster;

/// Deterministic xorshift generator (no `rand` dependency; benchmarks must be reproducible).
pub struct Rng(pub u64);

impl Rng {
    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x >> 32) as u32
    }
}

/// A document-like RGB image: paper gradient, many small dark rectangles ("text") and noise.
/// Same recipe as the ADR-0002 spike (gradient plus text-like rectangles plus noise).
pub fn synthetic_photo(width: u32, height: u32, seed: u64) -> Raster {
    let mut rng = Rng(seed | 1);
    let mut r = Raster::new(width, height);
    let (w, h) = (width as usize, height as usize);
    for y in 0..h {
        for x in 0..w {
            let base = 170 + (60 * x / w.max(1)) as i32 + (25 * y / h.max(1)) as i32;
            let i = (y * w + x) * 3;
            r.data[i] = base.clamp(0, 255) as u8;
            r.data[i + 1] = (base - 6).clamp(0, 255) as u8;
            r.data[i + 2] = (base - 20).clamp(0, 255) as u8;
        }
    }
    let rects = (w * h / 2000).max(8);
    for _ in 0..rects {
        let rw = 3 + (rng.next_u32() % 40) as usize;
        let rh = 2 + (rng.next_u32() % 9) as usize;
        let x0 = rng.next_u32() as usize % w.saturating_sub(rw).max(1);
        let y0 = rng.next_u32() as usize % h.saturating_sub(rh).max(1);
        let ink = (20 + rng.next_u32() % 70) as u8;
        for y in y0..(y0 + rh).min(h) {
            for x in x0..(x0 + rw).min(w) {
                let i = (y * w + x) * 3;
                r.data[i] = ink;
                r.data[i + 1] = ink;
                r.data[i + 2] = ink;
            }
        }
    }
    for b in &mut r.data {
        let n = (rng.next_u32() % 7) as i32 - 3;
        *b = (i32::from(*b) + n).clamp(0, 255) as u8;
    }
    r
}

/// Row-major homography taking a tilted-receipt quadrilateral inside the source frame to the
/// output rectangle (`dst_to_src`): the output pixel centres map into the source.
pub fn tilted_quad_homography(src_w: u32, src_h: u32, out_w: u32, out_h: u32) -> [f64; 9] {
    use auto_crop_imgproc::geometry::homography;
    let (sw, sh) = (f64::from(src_w), f64::from(src_h));
    let (ow, oh) = (f64::from(out_w), f64::from(out_h));
    let dst = [
        (-0.5, -0.5),
        (ow - 0.5, -0.5),
        (ow - 0.5, oh - 0.5),
        (-0.5, oh - 0.5),
    ];
    let quad = [
        (0.06 * sw, 0.10 * sh),
        (0.93 * sw, 0.05 * sh),
        (0.97 * sw, 0.92 * sh),
        (0.03 * sw, 0.95 * sh),
    ];
    homography(dst, quad).expect("tilted quad is non-degenerate")
}

/// PSNR in dB between two equally sized byte buffers (infinite if identical).
pub fn psnr(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len());
    let mse = a
        .iter()
        .zip(b)
        .map(|(x, y)| {
            let d = f64::from(*x) - f64::from(*y);
            d * d
        })
        .sum::<f64>()
        / a.len() as f64;
    if mse == 0.0 {
        f64::INFINITY
    } else {
        10.0 * (255.0f64 * 255.0 / mse).log10()
    }
}

/// Mean SSIM over non-overlapping 8x8 windows of the luma of two RGB images (uniform window, the
/// usual C1/C2 constants). A light-weight metric for comparing resizers, not the `image-compare`
/// reference used for pass/fail oracles.
pub fn ssim_luma_8x8(a: &Raster, b: &Raster) -> f64 {
    assert_eq!((a.width, a.height), (b.width, b.height));
    let luma = |r: &Raster, x: usize, y: usize| -> f64 {
        let i = (y * r.width as usize + x) * 3;
        0.299 * f64::from(r.data[i])
            + 0.587 * f64::from(r.data[i + 1])
            + 0.114 * f64::from(r.data[i + 2])
    };
    let (c1, c2) = ((0.01f64 * 255.0).powi(2), (0.03f64 * 255.0).powi(2));
    let (w, h) = (a.width as usize, a.height as usize);
    let (mut total, mut n) = (0.0, 0u64);
    for by in (0..h.saturating_sub(7)).step_by(8) {
        for bx in (0..w.saturating_sub(7)).step_by(8) {
            let (mut sa, mut sb, mut saa, mut sbb, mut sab) = (0.0, 0.0, 0.0, 0.0, 0.0);
            for y in by..by + 8 {
                for x in bx..bx + 8 {
                    let (p, q) = (luma(a, x, y), luma(b, x, y));
                    sa += p;
                    sb += q;
                    saa += p * p;
                    sbb += q * q;
                    sab += p * q;
                }
            }
            let (ma, mb) = (sa / 64.0, sb / 64.0);
            let (va, vb, cov) = (
                saa / 64.0 - ma * ma,
                sbb / 64.0 - mb * mb,
                sab / 64.0 - ma * mb,
            );
            total += ((2.0 * ma * mb + c1) * (2.0 * cov + c2))
                / ((ma * ma + mb * mb + c1) * (va + vb + c2));
            n += 1;
        }
    }
    if n == 0 { 1.0 } else { total / n as f64 }
}
pub mod legacy_warp;

/// Exact area-average downscale in f64 (the independent reference for resizer quality figures).
pub fn area_reference_f64(src: &Raster, out_w: u32, out_h: u32) -> Raster {
    let axis = |sn: usize, dn: usize| -> Vec<(usize, Vec<f64>)> {
        let scale = sn as f64 / dn as f64;
        (0..dn)
            .map(|i| {
                let (a, b) = (i as f64 * scale, (i as f64 + 1.0) * scale);
                let first = a.floor() as usize;
                let last = (b.ceil() as usize).min(sn).max(first + 1);
                let w: Vec<f64> = (first..last)
                    .map(|s| (b.min(s as f64 + 1.0) - a.max(s as f64)).max(0.0) / scale)
                    .collect();
                (first, w)
            })
            .collect()
    };
    let (sw, sh, dw, dh) = (
        src.width as usize,
        src.height as usize,
        out_w as usize,
        out_h as usize,
    );
    let (wx, wy) = (axis(sw, dw), axis(sh, dh));
    let mut tmp = vec![0.0f64; dw * sh * 3];
    for y in 0..sh {
        for (x, (start, w)) in wx.iter().enumerate() {
            for c in 0..3 {
                tmp[(y * dw + x) * 3 + c] = w
                    .iter()
                    .enumerate()
                    .map(|(k, wk)| wk * f64::from(src.data[(y * sw + start + k) * 3 + c]))
                    .sum();
            }
        }
    }
    let mut out = Raster::new(out_w, out_h);
    for (y, (start, w)) in wy.iter().enumerate() {
        for x in 0..dw {
            for c in 0..3 {
                let v: f64 = w
                    .iter()
                    .enumerate()
                    .map(|(k, wk)| wk * tmp[((start + k) * dw + x) * 3 + c])
                    .sum();
                out.data[(y * dw + x) * 3 + c] = (v + 0.5).clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

/// The resizers under test, each returning an RGB8 raster of `out_w` x `out_h`.
pub mod resizers {
    use auto_crop_imgproc::Raster;
    use auto_crop_imgproc::scale::resize_area;
    use fast_image_resize::images::Image;
    use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};

    pub fn own_area(src: &Raster, w: u32, h: u32) -> Raster {
        resize_area(src, w, h)
    }

    pub fn fir(src: &Raster, w: u32, h: u32, filter: FilterType) -> Raster {
        let view = Image::from_vec_u8(src.width, src.height, src.data.clone(), PixelType::U8x3)
            .expect("rgb8 view");
        let mut dst = Image::new(w, h, PixelType::U8x3);
        let mut resizer = Resizer::new();
        resizer
            .resize(
                &view,
                &mut dst,
                &ResizeOptions::new().resize_alg(ResizeAlg::Convolution(filter)),
            )
            .expect("fast_image_resize");
        Raster::from_raw(w, h, dst.into_vec()).expect("size")
    }

    /// Variant that reuses the source buffer (no clone), for timing the resize alone.
    pub fn fir_view(
        src: &Image<'_>,
        dst: &mut Image<'_>,
        resizer: &mut Resizer,
        filter: FilterType,
    ) {
        resizer
            .resize(
                src,
                dst,
                &ResizeOptions::new().resize_alg(ResizeAlg::Convolution(filter)),
            )
            .expect("fast_image_resize");
    }

    pub fn image_crate(
        src: &Raster,
        w: u32,
        h: u32,
        filter: image::imageops::FilterType,
    ) -> Raster {
        let img = image::RgbImage::from_raw(src.width, src.height, src.data.clone()).expect("rgb8");
        let out = image::imageops::resize(&img, w, h, filter);
        Raster::from_raw(w, h, out.into_raw()).expect("size")
    }
}
