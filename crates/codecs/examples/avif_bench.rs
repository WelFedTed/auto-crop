// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `avif_bench <width> <height> <speed> <quality> <420|444> [out.avif [ref.png]]`: encodes a
//! synthetic picture (multi-octave value noise over a gradient, about as hard to code as a detailed
//! photograph) with the AVIF writer and prints one line: size, wall time. The optional files are the
//! AVIF and the exact input as PNG, for an independent reader to decode and compare (PSNR).
//!
//! Run: `cargo run --release -p auto-crop-codecs --features avif,fixtures --example avif_bench -- 4000 3000 8 75 420`

use auto_crop_codecs::encoders::{AvifChroma, AvifEncoder};
use auto_crop_core::output::Quality;
use auto_crop_core::ports::{EncodeMeta, Encoder, PixelFormat, Raster, Samples};
use auto_crop_core::{CancelToken, OutputSpec};
use std::time::Instant;

/// Deterministic value noise in 0..1 on an integer lattice, bilinearly interpolated.
fn lattice(ix: i64, iy: i64, seed: u64) -> f64 {
    let mut h = (ix as u64)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add((iy as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F))
        .wrapping_add(seed.wrapping_mul(0x1656_67B1_9E37_79F9));
    h ^= h >> 29;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 32;
    (h & 0xFFFF) as f64 / 65535.0
}

fn noise(x: f64, y: f64, seed: u64) -> f64 {
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let (ix, iy) = (x0 as i64, y0 as i64);
    let a = lattice(ix, iy, seed);
    let b = lattice(ix + 1, iy, seed);
    let c = lattice(ix, iy + 1, seed);
    let d = lattice(ix + 1, iy + 1, seed);
    (a + (b - a) * sx) + ((c + (d - c) * sx) - (a + (b - a) * sx)) * sy
}

fn picture(w: u32, h: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity(w as usize * h as usize * 3);
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (f64::from(x), f64::from(y));
            let mut c = [0.0f64; 3];
            for (k, ch) in c.iter_mut().enumerate() {
                let mut amp = 0.5;
                let mut freq = 1.0 / 100.0;
                let mut s = 0.0;
                for o in 0..9 {
                    s += amp * noise(fx * freq, fy * freq, k as u64 * 17 + o);
                    amp *= 0.78;
                    freq *= 2.0;
                }
                *ch = 255.0 * (0.15 + 0.75 * s / 2.6)
                    + 40.0 * (lattice(x as i64, y as i64, k as u64 + 99) - 0.5);
            }
            v.extend(c.iter().map(|x| x.clamp(0.0, 255.0) as u8));
        }
    }
    v
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (w, h): (u32, u32) = (a[0].parse().unwrap(), a[1].parse().unwrap());
    let speed: u8 = a[2].parse().unwrap();
    let quality: u8 = a[3].parse().unwrap();
    let chroma = if a[4] == "444" {
        AvifChroma::Yuv444
    } else {
        AvifChroma::Yuv420
    };
    let rgb = picture(w, h);
    let raster = Raster::from_samples(w, h, PixelFormat::Rgb8, Samples::U8(rgb.clone())).unwrap();
    let enc = AvifEncoder {
        chroma,
        speed,
        ..AvifEncoder::default()
    };
    let spec = OutputSpec {
        quality: Quality::Fixed { value: quality },
        ..OutputSpec::default()
    };
    let t = Instant::now();
    let out = enc
        .encode(
            &raster.view(),
            &spec,
            &EncodeMeta::default(),
            &CancelToken::never(),
        )
        .expect("encode");
    let secs = t.elapsed().as_secs_f64();
    println!(
        "{w}x{h} speed {speed} q {quality} {} -> {} bytes ({:.2} bpp) in {secs:.2} s",
        a[4],
        out.len(),
        out.len() as f64 * 8.0 / (f64::from(w) * f64::from(h))
    );
    if let Some(p) = a.get(5) {
        std::fs::write(p, &out).unwrap();
    }
    if let Some(p) = a.get(6) {
        let png = auto_crop_codecs::encode(
            &auto_crop_imgproc::Raster::from_raw(w, h, rgb).unwrap(),
            auto_crop_codecs::Format::Png,
            0,
            None,
        )
        .unwrap();
        std::fs::write(p, png).unwrap();
    }
}
