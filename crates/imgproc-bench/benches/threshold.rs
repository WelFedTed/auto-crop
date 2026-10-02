// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! M1.57: Otsu, Sauvola and NICK on a 12 MP grey page at windows 31, 51 and 101, 1 and 8 threads
//! (`THRESHOLD_BENCH_MP=48` selects another size).

use auto_crop_imgproc::threshold::{binarize, nick, otsu_threshold, sauvola};
use auto_crop_imgproc_bench::synthetic_photo;
use criterion::{Criterion, criterion_group, criterion_main};
use std::time::Duration;

fn pool(threads: usize) -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .expect("thread pool")
}

fn gray_page(mp: u32) -> (Vec<u8>, u32, u32) {
    let h = ((f64::from(mp) * 1e6) / (4.0 / 3.0)).sqrt().round() as u32;
    let w = h * 4 / 3;
    let rgb = synthetic_photo(w, h, 3);
    let gray = rgb
        .data
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| ((u32::from(p[0]) * 77 + u32::from(p[1]) * 150 + u32::from(p[2]) * 29) >> 8) as u8)
        .collect();
    (gray, w, h)
}

fn bench_threshold(c: &mut Criterion) {
    let mp = std::env::var("THRESHOLD_BENCH_MP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(12);
    let (gray, w, h) = gray_page(mp);
    let mut group = c.benchmark_group(format!("threshold_{mp}MP"));
    group.sample_size(20);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(5));
    for threads in [8usize, 1] {
        let p = pool(threads);
        group.bench_function(format!("otsu+binarize/{threads}t"), |b| {
            b.iter(|| p.install(|| binarize(&gray, otsu_threshold(&gray))));
        });
        for window in [31u32, 51, 101] {
            group.bench_function(format!("sauvola/w{window}/{threads}t"), |b| {
                b.iter(|| p.install(|| sauvola(&gray, w, h, window, 0.25, 128.0)));
            });
            group.bench_function(format!("nick/w{window}/{threads}t"), |b| {
                b.iter(|| p.install(|| nick(&gray, w, h, window, -0.1)));
            });
        }
    }
    group.finish();
}

criterion_group!(benches, bench_threshold);
criterion_main!(benches);
