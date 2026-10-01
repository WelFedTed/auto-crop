// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Throwaway spike for ROADMAP M0.36-M0.38: perspective warp with Lanczos3.
//!
//! Compares, on a 12 MP RGB u8 output:
//!   * `ref`    - f64 Lanczos3 reference (slow, authoritative for PSNR)
//!   * `kornia` - kornia-imgproc 0.2 `warp_perspective` (f32 Lanczos), including u8<->f32 conversion
//!   * `own`    - strip-wise u8 kernel: 64-row rayon strips, Lanczos3 weight LUT, f32 accumulation,
//!                no full-frame f32 buffer
//!
//! Usage: spike-warp [samples8] [samples1]   (defaults 20 and 10)
//! Also writes src.raw / ref.raw / own.raw / kornia.raw / H.txt into ./out for the OpenCV oracle script.

use kornia_image::{Image, ImageSize};
use kornia_imgproc::interpolation::InterpolationMode;
use kornia_imgproc::warp::warp_perspective;
use peak_alloc::PeakAlloc;
use rayon::prelude::*;
use std::time::Instant;

#[global_allocator]
static PEAK: PeakAlloc = PeakAlloc;

/// 1 = 12 MP (4000x3000), 2 = 48 MP (8000x6000). Build with `--features scale2` for 48 MP.
const SCALE: usize = if cfg!(feature = "scale2") { 2 } else { 1 };
const SW: usize = 4000 * SCALE;
const SH: usize = 3000 * SCALE;
const DW: usize = 4000 * SCALE;
const DH: usize = 3000 * SCALE;
const STRIP: usize = 64;
const LUT_N: usize = 1024;

type H = [f64; 9];

// ---------------------------------------------------------------- source image
fn make_source() -> Vec<u8> {
    let mut img = vec![0u8; SW * SH * 3];
    let mut state: u64 = 0x1234_5678_9abc_def0;
    let mut next = || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (state >> 33) as u32
    };
    // smooth gradient + sinusoid
    for y in 0..SH {
        for x in 0..SW {
            let fx = x as f32 / SW as f32;
            let fy = y as f32 / SH as f32;
            let base = 140.0 + 60.0 * (fx * 6.0).sin() * (fy * 5.0).cos();
            for c in 0..3 {
                let v = base + 20.0 * c as f32 + 25.0 * fx - 15.0 * fy;
                img[(y * SW + x) * 3 + c] = v.clamp(0.0, 255.0) as u8;
            }
        }
    }
    // text-like dark rectangles (edges), as on a receipt
    for _ in 0..6000 * SCALE * SCALE {
        let w = 8 + (next() % 60) as usize;
        let h = 4 + (next() % 14) as usize;
        let x0 = (next() as usize) % (SW - w);
        let y0 = (next() as usize) % (SH - h);
        let v = (next() % 70) as u8;
        for y in y0..y0 + h {
            for x in x0..x0 + w {
                for c in 0..3 {
                    img[(y * SW + x) * 3 + c] = v;
                }
            }
        }
    }
    // mild noise
    for p in img.iter_mut() {
        let n = (next() % 7) as i32 - 3;
        *p = (*p as i32 + n).clamp(0, 255) as u8;
    }
    img
}

// ---------------------------------------------------------------- homography maths
fn solve8(mut a: [[f64; 9]; 8]) -> [f64; 8] {
    for col in 0..8 {
        let mut piv = col;
        for r in col + 1..8 {
            if a[r][col].abs() > a[piv][col].abs() {
                piv = r;
            }
        }
        a.swap(col, piv);
        let d = a[col][col];
        for c in col..9 {
            a[col][c] /= d;
        }
        for r in 0..8 {
            if r != col {
                let f = a[r][col];
                for c in col..9 {
                    a[r][c] -= f * a[col][c];
                }
            }
        }
    }
    let mut x = [0.0; 8];
    for i in 0..8 {
        x[i] = a[i][8];
    }
    x
}

/// Homography mapping `from[i]` -> `to[i]`.
fn homography(from: [(f64, f64); 4], to: [(f64, f64); 4]) -> H {
    let mut a = [[0.0; 9]; 8];
    for i in 0..4 {
        let (x, y) = from[i];
        let (u, v) = to[i];
        a[2 * i] = [x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y, u];
        a[2 * i + 1] = [0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y, v];
    }
    let s = solve8(a);
    [s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7], 1.0]
}

fn invert(m: &H) -> H {
    let [a, b, c, d, e, f, g, h, i] = *m;
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    [
        (e * i - f * h) / det,
        (c * h - b * i) / det,
        (b * f - c * e) / det,
        (f * g - d * i) / det,
        (a * i - c * g) / det,
        (c * d - a * f) / det,
        (d * h - e * g) / det,
        (b * g - a * h) / det,
        (a * e - b * d) / det,
    ]
}

// ---------------------------------------------------------------- Lanczos3
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

/// Six normalised weights for fractional offset `f` (taps at -2..=3 around floor).
fn weights(f: f64) -> [f64; 6] {
    let mut w = [0.0; 6];
    let mut sum = 0.0;
    for (i, wi) in w.iter_mut().enumerate() {
        *wi = lanczos3(f - (i as f64 - 2.0));
        sum += *wi;
    }
    for wi in &mut w {
        *wi /= sum;
    }
    w
}

// ---------------------------------------------------------------- reference (f64)
fn warp_ref(src: &[u8], h: &H) -> Vec<u8> {
    let mut out = vec![0u8; DW * DH * 3];
    out.par_chunks_mut(DW * 3).enumerate().for_each(|(v, row)| {
        for u in 0..DW {
            let (uf, vf) = (u as f64, v as f64);
            let d = h[6] * uf + h[7] * vf + h[8];
            let x = (h[0] * uf + h[1] * vf + h[2]) / d;
            let y = (h[3] * uf + h[4] * vf + h[5]) / d;
            if !(x >= 0.0 && x < SW as f64 && y >= 0.0 && y < SH as f64) {
                continue;
            }
            let (x0, y0) = (x.floor(), y.floor());
            let wx = weights(x - x0);
            let wy = weights(y - y0);
            for c in 0..3 {
                let mut acc = 0.0;
                for (dy, wyv) in wy.iter().enumerate() {
                    let yi = ((y0 as i64 + dy as i64 - 2).clamp(0, SH as i64 - 1)) as usize;
                    let mut rx = 0.0;
                    for (dx, wxv) in wx.iter().enumerate() {
                        let xi = ((x0 as i64 + dx as i64 - 2).clamp(0, SW as i64 - 1)) as usize;
                        rx += wxv * src[(yi * SW + xi) * 3 + c] as f64;
                    }
                    acc += wyv * rx;
                }
                row[u * 3 + c] = acc.round().clamp(0.0, 255.0) as u8;
            }
        }
    });
    out
}

// ---------------------------------------------------------------- own strip-wise u8 kernel
fn make_lut() -> Vec<[f32; 6]> {
    (0..LUT_N)
        .map(|i| {
            let w = weights(i as f64 / LUT_N as f64);
            [w[0] as f32, w[1] as f32, w[2] as f32, w[3] as f32, w[4] as f32, w[5] as f32]
        })
        .collect()
}

fn warp_own(src: &[u8], h: &H, lut: &[[f32; 6]], out: &mut [u8]) {
    let hf: [f32; 9] = std::array::from_fn(|i| h[i] as f32);
    out.par_chunks_mut(DW * 3 * STRIP).enumerate().for_each(|(strip, chunk)| {
        let v0 = strip * STRIP;
        for (r, row) in chunk.chunks_mut(DW * 3).enumerate() {
            let vf = (v0 + r) as f32;
            let (nx0, ny0, nd0) = (hf[1] * vf + hf[2], hf[4] * vf + hf[5], hf[7] * vf + hf[8]);
            for u in 0..DW {
                let uf = u as f32;
                let d = hf[6] * uf + nd0;
                let x = (hf[0] * uf + nx0) / d;
                let y = (hf[3] * uf + ny0) / d;
                if !(x >= 0.0 && x < SW as f32 && y >= 0.0 && y < SH as f32) {
                    row[u * 3] = 0;
                    row[u * 3 + 1] = 0;
                    row[u * 3 + 2] = 0;
                    continue;
                }
                let (xf, yf) = (x.floor(), y.floor());
                let wx = &lut[((x - xf) * LUT_N as f32) as usize];
                let wy = &lut[((y - yf) * LUT_N as f32) as usize];
                let (x0, y0) = (xf as i64, yf as i64);
                let mut xi = [0usize; 6];
                for (k, v) in xi.iter_mut().enumerate() {
                    *v = ((x0 + k as i64 - 2).clamp(0, SW as i64 - 1)) as usize * 3;
                }
                let mut acc = [0.0f32; 3];
                for (k, wyv) in wy.iter().enumerate() {
                    let yi = ((y0 + k as i64 - 2).clamp(0, SH as i64 - 1)) as usize;
                    let base = yi * SW * 3;
                    let (mut r0, mut r1, mut r2) = (0.0f32, 0.0f32, 0.0f32);
                    for (j, wxv) in wx.iter().enumerate() {
                        let p = base + xi[j];
                        r0 = wxv.mul_add(src[p] as f32, r0);
                        r1 = wxv.mul_add(src[p + 1] as f32, r1);
                        r2 = wxv.mul_add(src[p + 2] as f32, r2);
                    }
                    acc[0] = wyv.mul_add(r0, acc[0]);
                    acc[1] = wyv.mul_add(r1, acc[1]);
                    acc[2] = wyv.mul_add(r2, acc[2]);
                }
                row[u * 3] = (acc[0] + 0.5).clamp(0.0, 255.0) as u8;
                row[u * 3 + 1] = (acc[1] + 0.5).clamp(0.0, 255.0) as u8;
                row[u * 3 + 2] = (acc[2] + 0.5).clamp(0.0, 255.0) as u8;
            }
        }
    });
}

// ---------------------------------------------------------------- kornia (f32)
fn warp_kornia(src_f: &Image<f32, 3>, m_src_to_dst: &[f32; 9]) -> Vec<u8> {
    let mut dst = Image::<f32, 3>::from_size_val(ImageSize { width: DW, height: DH }, 0.0).expect("alloc");
    warp_perspective(src_f, &mut dst, m_src_to_dst, InterpolationMode::Lanczos).expect("warp");
    dst.as_slice().iter().map(|v| (v + 0.5).clamp(0.0, 255.0) as u8).collect()
}

fn to_f32_image(src: &[u8]) -> Image<f32, 3> {
    let data: Vec<f32> = src.par_iter().map(|&b| b as f32).collect();
    Image::<f32, 3>::new(ImageSize { width: SW, height: SH }, data).expect("image")
}

// ---------------------------------------------------------------- measurement helpers
fn psnr(a: &[u8], b: &[u8]) -> f64 {
    let se: u64 = a.par_iter().zip(b.par_iter()).map(|(x, y)| { let d = *x as i64 - *y as i64; (d * d) as u64 }).sum();
    if se == 0 {
        return f64::INFINITY;
    }
    let mse = se as f64 / a.len() as f64;
    10.0 * (255.0f64 * 255.0 / mse).log10()
}

fn stats(mut v: Vec<f64>) -> (f64, f64) {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = v[v.len() / 2];
    let p95 = v[((v.len() as f64 * 0.95).ceil() as usize).min(v.len()) - 1];
    (median, p95)
}

fn in_pool<T: Send>(threads: usize, f: impl FnOnce() -> T + Send) -> T {
    rayon::ThreadPoolBuilder::new().num_threads(threads).build().expect("pool").install(f)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let s8: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(20);
    let s1: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(10);

    println!("source {SW}x{SH}, output {DW}x{DH} ({} MP), RGB u8", DW * DH / 1_000_000);
    let src = make_source();
    // dst rect -> src quad (a tilted receipt)
    let dst_rect = [(0.0, 0.0), ((DW - 1) as f64, 0.0), ((DW - 1) as f64, (DH - 1) as f64), (0.0, (DH - 1) as f64)];
    let s = SCALE as f64;
    let src_quad = [(520.0 * s, 310.0 * s), (3480.0 * s, 240.0 * s), (3620.0 * s, 2680.0 * s), (380.0 * s, 2760.0 * s)];
    let h = homography(dst_rect, src_quad); // dst -> src
    let h_inv = invert(&h); // src -> dst (kornia convention)
    let m_kornia: [f32; 9] = std::array::from_fn(|i| h_inv[i] as f32);
    let lut = make_lut();

    let out_dir = if SCALE == 1 { "out" } else { "out48" };
    std::fs::create_dir_all(out_dir).ok();
    std::fs::write(format!("{out_dir}/H.txt"), h.iter().map(|v| format!("{v:.17e}")).collect::<Vec<_>>().join(" ")).ok();
    std::fs::write(format!("{out_dir}/src.raw"), &src).ok();

    // ---- accuracy
    let t = Instant::now();
    let reference = in_pool(8, || warp_ref(&src, &h));
    println!("reference f64 (8 threads): {:.1} s", t.elapsed().as_secs_f64());
    let mut own = vec![0u8; DW * DH * 3];
    in_pool(8, || warp_own(&src, &h, &lut, &mut own));
    let src_f = in_pool(8, || to_f32_image(&src));
    let kor = in_pool(8, || warp_kornia(&src_f, &m_kornia));
    println!("PSNR own    vs reference: {:.2} dB", psnr(&own, &reference));
    println!("PSNR kornia vs reference: {:.2} dB", psnr(&kor, &reference));
    println!("PSNR own    vs kornia   : {:.2} dB", psnr(&own, &kor));
    std::fs::write(format!("{out_dir}/ref.raw"), &reference).ok();
    std::fs::write(format!("{out_dir}/own.raw"), &own).ok();
    std::fs::write(format!("{out_dir}/kornia.raw"), &kor).ok();

    // ---- speed and memory
    for &(threads, samples) in &[(8usize, s8), (1usize, s1)] {
        println!("\n== {threads} thread(s), {samples} samples (median / p95 ms; first run dropped as warm-up)");
        // own
        let mut times = Vec::new();
        let mut out = vec![0u8; DW * DH * 3];
        in_pool(threads, || warp_own(&src, &h, &lut, &mut out)); // warm-up
        PEAK.reset_peak_usage();
        let base = PEAK.current_usage();
        for _ in 0..samples {
            let t = Instant::now();
            in_pool(threads, || warp_own(&src, &h, &lut, &mut out));
            times.push(t.elapsed().as_secs_f64() * 1e3);
        }
        let (m, p) = stats(times);
        println!("own    (u8 in, u8 out)            : {m:8.1} / {p:8.1} ms   extra peak heap {:.1} MB", (PEAK.peak_usage() - base) as f64 / 1e6);
        // kornia warp only (source already f32)
        let mut times = Vec::new();
        in_pool(threads, || warp_kornia(&src_f, &m_kornia));
        for _ in 0..samples {
            let t = Instant::now();
            let _ = in_pool(threads, || warp_kornia(&src_f, &m_kornia));
            times.push(t.elapsed().as_secs_f64() * 1e3);
        }
        let (m, p) = stats(times);
        println!("kornia (f32 warp + u8 readout)    : {m:8.1} / {p:8.1} ms");
        // kornia full u8 -> u8 including conversions
        let mut times = Vec::new();
        PEAK.reset_peak_usage();
        let base = PEAK.current_usage();
        for _ in 0..samples {
            let t = Instant::now();
            let _ = in_pool(threads, || {
                let sf = to_f32_image(&src);
                warp_kornia(&sf, &m_kornia)
            });
            times.push(t.elapsed().as_secs_f64() * 1e3);
        }
        let (m, p) = stats(times);
        println!("kornia (u8 -> f32 -> warp -> u8)  : {m:8.1} / {p:8.1} ms   extra peak heap {:.1} MB", (PEAK.peak_usage() - base) as f64 / 1e6);
    }
}
