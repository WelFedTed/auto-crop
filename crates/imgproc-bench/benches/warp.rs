// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! M1.57: Lanczos3 warp from 12 / 48 / 100 MP sources to the A4-at-300-dpi page (2480 x 3508) and
//! to a heavy-minification output (1024 x 768), 1 and 8 threads.
//!
//! * `legacy`  the frozen ADR-0002 spike kernel (before the M1.24 rewrite)
//! * `plain`   the current kernel, no minification guard
//! * `guarded` the current kernel with the M1.26 pyramid guard (what the engine should use)
//!
//! Environment: `WARP_BENCH_MP=12,48` restricts the source sizes; `WARP_BENCH_LEGACY=0` skips the
//! slow legacy runs. The machine must be otherwise idle (the label belongs in docs/perf/kernels.md).

use auto_crop_imgproc::Raster;
use auto_crop_imgproc::cancel::NeverCancel;
use auto_crop_imgproc::minify::warp_perspective_guarded;
use auto_crop_imgproc::pixels::ImageRef;
use auto_crop_imgproc::warp::warp_perspective;
use auto_crop_imgproc_bench::{legacy_warp, synthetic_photo, tilted_quad_homography};
use criterion::{Criterion, criterion_group, criterion_main};
use std::time::Duration;

fn pool(threads: usize) -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .expect("thread pool")
}

fn dims(mp: u32) -> (u32, u32) {
    // 4:3 sources: 12 MP = 4000 x 3000, 48 MP = 8000 x 6000, 100 MP = 11547 x 8660.
    let h = ((f64::from(mp) * 1e6) / (4.0 / 3.0)).sqrt().round() as u32;
    (h * 4 / 3, h)
}

fn selected() -> Vec<u32> {
    std::env::var("WARP_BENCH_MP")
        .ok()
        .map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![12, 48, 100])
}

fn legacy_enabled() -> bool {
    std::env::var("WARP_BENCH_LEGACY").map_or(true, |v| v != "0")
}

fn run_group(c: &mut Criterion, name: &str, out: (u32, u32)) {
    let mut group = c.benchmark_group(name);
    group.sample_size(10);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(8));
    for mp in selected() {
        let (sw, sh) = dims(mp);
        let src: Raster = synthetic_photo(sw, sh, 7);
        let h = tilted_quad_homography(sw, sh, out.0, out.1);
        let img = ImageRef::new(sw, sh, 3, &src.data[..]).expect("rgb8 image");
        for threads in [8usize, 1] {
            // One-thread runs of the big sources add minutes and little insight.
            if threads == 1 && mp > 12 {
                continue;
            }
            let p = pool(threads);
            if legacy_enabled() && (mp <= 48 || threads == 8) {
                group.bench_function(format!("legacy/{mp}MP/{threads}t"), |b| {
                    b.iter(|| p.install(|| legacy_warp::warp_perspective(&src, &h, out.0, out.1)));
                });
            }
            group.bench_function(format!("plain/{mp}MP/{threads}t"), |b| {
                b.iter(|| p.install(|| warp_perspective(&src, &h, out.0, out.1)));
            });
            group.bench_function(format!("guarded/{mp}MP/{threads}t"), |b| {
                b.iter(|| {
                    p.install(|| warp_perspective_guarded(img, &h, out.0, out.1, &NeverCancel))
                        .expect("never cancelled")
                });
            });
        }
    }
    group.finish();
}

fn bench_a4(c: &mut Criterion) {
    run_group(c, "warp_to_a4_2480x3508", (2480, 3508));
}

fn bench_heavy_minification(c: &mut Criterion) {
    run_group(c, "warp_to_1024x768_heavy_minification", (1024, 768));
}

fn bench_same_size(c: &mut Criterion) {
    // ADR-0002 protocol: output equals source size (12 MP to 12 MP), the number the ADR recorded.
    let mut group = c.benchmark_group("warp_same_size_12MP");
    group.sample_size(10);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(8));
    let (sw, sh) = dims(12);
    let src = synthetic_photo(sw, sh, 7);
    let h = tilted_quad_homography(sw, sh, sw, sh);
    for threads in [8usize, 1] {
        let p = pool(threads);
        if legacy_enabled() {
            group.bench_function(format!("legacy/{threads}t"), |b| {
                b.iter(|| p.install(|| legacy_warp::warp_perspective(&src, &h, sw, sh)));
            });
        }
        group.bench_function(format!("plain/{threads}t"), |b| {
            b.iter(|| p.install(|| warp_perspective(&src, &h, sw, sh)));
        });
    }
    group.finish();
}

criterion_group!(benches, bench_a4, bench_heavy_minification, bench_same_size);
criterion_main!(benches);
