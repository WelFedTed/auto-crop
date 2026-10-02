// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! M1.57: Lanczos3 warp from 12 / 48 / 100 MP sources to the A4-at-300-dpi page (2480 x 3508),
//! 1 and 8 threads. `legacy` is the frozen ADR-0002 spike kernel, `new` the current kernel.
//!
//! Set `WARP_BENCH_MP=12` (comma list) to restrict the source sizes, e.g. for a quick run.

use auto_crop_imgproc::Raster;
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

fn bench_warp(c: &mut Criterion) {
    let mut group = c.benchmark_group("warp_to_a4_2480x3508");
    group.sample_size(10);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(6));
    for mp in selected() {
        let (sw, sh) = dims(mp);
        let src: Raster = synthetic_photo(sw, sh, 7);
        let h = tilted_quad_homography(sw, sh, 2480, 3508);
        for threads in [8usize, 1] {
            if threads == 1 && mp > 12 {
                continue; // one-thread runs of the big sources add minutes and little insight
            }
            let p = pool(threads);
            group.bench_function(format!("legacy/{mp}MP/{threads}t"), |b| {
                b.iter(|| p.install(|| legacy_warp::warp_perspective(&src, &h, 2480, 3508)));
            });
            group.bench_function(format!("new/{mp}MP/{threads}t"), |b| {
                b.iter(|| p.install(|| warp_perspective(&src, &h, 2480, 3508)));
            });
        }
    }
    group.finish();
}

fn bench_same_size(c: &mut Criterion) {
    // ADR-0002 protocol: output equals source size.
    let mut group = c.benchmark_group("warp_same_size");
    group.sample_size(10);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(6));
    let (sw, sh) = dims(12);
    let src = synthetic_photo(sw, sh, 7);
    let h = tilted_quad_homography(sw, sh, sw, sh);
    for threads in [8usize, 1] {
        let p = pool(threads);
        group.bench_function(format!("legacy/12MP/{threads}t"), |b| {
            b.iter(|| p.install(|| legacy_warp::warp_perspective(&src, &h, sw, sh)));
        });
        group.bench_function(format!("new/12MP/{threads}t"), |b| {
            b.iter(|| p.install(|| warp_perspective(&src, &h, sw, sh)));
        });
    }
    group.finish();
}

criterion_group!(benches, bench_warp, bench_same_size);
criterion_main!(benches);
