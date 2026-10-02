// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! M1.56: the in-tree area resize against `fast_image_resize` and `image::imageops::resize`
//! (the decoders are compared in `crates/codecs`; `pic-scale` is not in the tree). Timing only;
//! PSNR and SSIM against an exact f64 area average come from
//! `cargo run --release -p auto-crop-imgproc-bench --example resize_quality`.
//!
//! The `clone` cost of building the library input is excluded: `fir` runs on a prebuilt image.

use auto_crop_imgproc::scale::resize_area;
use auto_crop_imgproc_bench::{resizers, synthetic_photo};
use criterion::{Criterion, criterion_group, criterion_main};
use fast_image_resize::images::Image;
use fast_image_resize::{FilterType, PixelType, Resizer};
use std::time::Duration;

fn pool(threads: usize) -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .expect("thread pool")
}

fn bench_resize(c: &mut Criterion) {
    // (source megapixels, output): the 1024 px detection proxy and the 3 MP display proxy.
    for (mp, ow, oh) in [(12u32, 1024u32, 768u32), (12, 2000, 1500), (48, 1024, 768)] {
        let h = ((f64::from(mp) * 1e6) / (4.0 / 3.0)).sqrt().round() as u32;
        let w = h * 4 / 3;
        let src = synthetic_photo(w, h, 5);
        let fir_src = Image::from_vec_u8(w, h, src.data.clone(), PixelType::U8x3).unwrap();
        let mut group = c.benchmark_group(format!("resize_{mp}MP_to_{ow}x{oh}"));
        group.sample_size(15);
        group.warm_up_time(Duration::from_secs(1));
        group.measurement_time(Duration::from_secs(4));
        for threads in [1usize, 8] {
            let p = pool(threads);
            group.bench_function(format!("own_area/{threads}t"), |b| {
                b.iter(|| p.install(|| resize_area(&src, ow, oh)));
            });
            for (name, filter) in [
                ("fir_box", FilterType::Box),
                ("fir_bilinear", FilterType::Bilinear),
                ("fir_lanczos3", FilterType::Lanczos3),
            ] {
                let mut dst = Image::new(ow, oh, PixelType::U8x3);
                let mut resizer = Resizer::new();
                group.bench_function(format!("{name}/{threads}t"), |b| {
                    b.iter(|| {
                        p.install(|| resizers::fir_view(&fir_src, &mut dst, &mut resizer, filter))
                    });
                });
            }
        }
        // The `image` crate is single-threaded; one run each.
        for (name, filter) in [
            ("image_triangle", image::imageops::FilterType::Triangle),
            ("image_lanczos3", image::imageops::FilterType::Lanczos3),
        ] {
            let img = image::RgbImage::from_raw(w, h, src.data.clone()).unwrap();
            group.bench_function(format!("{name}/1t"), |b| {
                b.iter(|| image::imageops::resize(&img, ow, oh, filter));
            });
        }
        group.finish();
    }
}

criterion_group!(benches, bench_resize);
criterion_main!(benches);
