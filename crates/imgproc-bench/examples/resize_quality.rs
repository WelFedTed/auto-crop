// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! M1.56 quality table: PSNR and a light SSIM of each resizer against an exact f64 area average
//! taken on the sRGB code values (gamma space), plus the PSNR against the same average taken in
//! linear light, and the gap between those two references (does the colour space matter?).
//! `cargo run --release -p auto-crop-imgproc-bench --example resize_quality`
//!
//! Two inputs: the synthetic document photo (sharp text-like rectangles plus noise) and a
//! blurred copy that stands in for a real optical image.

use auto_crop_imgproc::Raster;
use auto_crop_imgproc_bench::resizers::PicColour;
use auto_crop_imgproc_bench::{
    area_reference_f64, area_reference_linear_f64, psnr, resizers, ssim_luma_8x8, synthetic_photo,
};
use fast_image_resize::FilterType;
use pic_scale::ResamplingFunction;

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
            let linear = area_reference_linear_f64(src, ow, oh);
            let mean = |r: &Raster| {
                r.data.iter().map(|b| f64::from(*b)).sum::<f64>() / r.data.len() as f64
            };
            println!("\n{label} -> {ow}x{oh}");
            println!(
                "gamma-space vs linear-light exact average: {:.2} dB PSNR, mean level {:.2} vs {:.2}",
                psnr(&reference.data, &linear.data),
                mean(&reference),
                mean(&linear)
            );
            println!(
                "| resizer | PSNR dB vs gamma avg | SSIM (luma, 8x8) | PSNR dB vs linear avg |"
            );
            println!("|---|---:|---:|---:|");
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
                (
                    "pic-scale sRGB Bilinear",
                    resizers::pic_scale(src, ow, oh, PicColour::Srgb, ResamplingFunction::Bilinear),
                ),
                (
                    "pic-scale sRGB Lanczos3",
                    resizers::pic_scale(src, ow, oh, PicColour::Srgb, ResamplingFunction::Lanczos3),
                ),
                (
                    "pic-scale linear Bilinear",
                    resizers::pic_scale(
                        src,
                        ow,
                        oh,
                        PicColour::Linear,
                        ResamplingFunction::Bilinear,
                    ),
                ),
                (
                    "pic-scale linear Lanczos3",
                    resizers::pic_scale(
                        src,
                        ow,
                        oh,
                        PicColour::Linear,
                        ResamplingFunction::Lanczos3,
                    ),
                ),
                (
                    "pic-scale linear-approx Bilinear",
                    resizers::pic_scale(
                        src,
                        ow,
                        oh,
                        PicColour::LinearApprox,
                        ResamplingFunction::Bilinear,
                    ),
                ),
            ];
            let fmt = |p: f64| {
                if p.is_infinite() {
                    "inf".to_owned()
                } else {
                    format!("{p:.2}")
                }
            };
            for (name, out) in rows {
                println!(
                    "| {name} | {} | {:.4} | {} |",
                    fmt(psnr(&out.data, &reference.data)),
                    ssim_luma_8x8(&out, &reference),
                    fmt(psnr(&out.data, &linear.data))
                );
            }
        }
    }
}
