// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! M1.56 quality table: PSNR and a light SSIM of each resizer against an exact f64 area average.
//! `cargo run --release -p auto-crop-imgproc-bench --example resize_quality`
//!
//! Two inputs: the synthetic document photo (sharp text-like rectangles plus noise) and a
//! blurred copy that stands in for a real optical image.

use auto_crop_imgproc::Raster;
use auto_crop_imgproc_bench::{area_reference_f64, psnr, resizers, ssim_luma_8x8, synthetic_photo};
use fast_image_resize::FilterType;

/// 3x3 box blur twice: a cheap stand-in for lens blur.
fn blurred(src: &Raster) -> Raster {
    let (w, h) = (src.width as usize, src.height as usize);
    let mut cur = src.clone();
    for _ in 0..2 {
        let mut next = cur.clone();
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                for c in 0..3 {
                    let mut s = 0u32;
                    for dy in 0..3 {
                        for dx in 0..3 {
                            s += u32::from(cur.data[((y + dy - 1) * w + x + dx - 1) * 3 + c]);
                        }
                    }
                    next.data[(y * w + x) * 3 + c] = ((s + 4) / 9) as u8;
                }
            }
        }
        cur = next;
    }
    cur
}

fn main() {
    let (w, h) = (4000u32, 3000u32);
    let sharp = synthetic_photo(w, h, 5);
    let soft = blurred(&sharp);
    for (label, src) in [
        ("sharp synthetic page (12 MP)", &sharp),
        ("blurred synthetic page (12 MP)", &soft),
    ] {
        for (ow, oh) in [(1024u32, 768u32), (2000, 1500)] {
            let reference = area_reference_f64(src, ow, oh);
            println!("\n{label} -> {ow}x{oh}, vs exact f64 area average");
            println!("| resizer | PSNR dB | SSIM (luma, 8x8) |");
            println!("|---|---:|---:|");
            let rows: Vec<(&str, Raster)> = vec![
                ("own area (imgproc::scale)", resizers::own_area(src, ow, oh)),
                (
                    "fast_image_resize Box",
                    resizers::fir(src, ow, oh, FilterType::Box),
                ),
                (
                    "fast_image_resize Bilinear",
                    resizers::fir(src, ow, oh, FilterType::Bilinear),
                ),
                (
                    "fast_image_resize Lanczos3",
                    resizers::fir(src, ow, oh, FilterType::Lanczos3),
                ),
                (
                    "image Triangle",
                    resizers::image_crate(src, ow, oh, image::imageops::FilterType::Triangle),
                ),
                (
                    "image Lanczos3",
                    resizers::image_crate(src, ow, oh, image::imageops::FilterType::Lanczos3),
                ),
            ];
            for (name, out) in rows {
                let p = psnr(&out.data, &reference.data);
                let p = if p.is_infinite() {
                    "inf".to_owned()
                } else {
                    format!("{p:.2}")
                };
                println!("| {name} | {p} | {:.4} |", ssim_luma_8x8(&out, &reference));
            }
        }
    }
}
