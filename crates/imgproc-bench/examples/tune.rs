// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Quick min-of-N timer for kernel tuning (criterion is too slow for an edit-measure loop). Uses
//! small (1.4 MP) workloads and runs the cases round-robin, so that load drift on a shared
//! machine hits every case alike; the figure to trust is the ratio to `legacy`, not nanoseconds.
//! `cargo run --release -p auto-crop-imgproc-bench --example tune -- [threads]`

use auto_crop_imgproc::cancel::NeverCancel;
use auto_crop_imgproc::pixels::Image;
use auto_crop_imgproc::warp::{warp_perspective, warp_perspective_image};
use auto_crop_imgproc_bench::{legacy_warp, synthetic_photo, tilted_quad_homography};
use std::time::Instant;

fn main() {
    let threads: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap();
    let (sw, sh) = (1400u32, 1000u32);
    let src = synthetic_photo(sw, sh, 7);
    let px = f64::from(sw) * f64::from(sh);
    let id = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
    let shift = [1.0, 0.0, 0.5, 0.0, 1.0, 0.5, 0.0, 0.0, 1.0];
    let tilt = tilted_quad_homography(sw, sh, sw, sh);
    let mut gray = Image::<u8>::new(sw, sh, 1);
    let mut rgba = Image::<u8>::new(sw, sh, 4);
    let mut rgb16 = Image::<u16>::new(sw, sh, 3);
    for (i, v) in gray.data.iter_mut().enumerate() {
        *v = (i * 31 % 251) as u8;
    }
    for (i, v) in rgba.data.iter_mut().enumerate() {
        *v = (i * 31 % 251) as u8;
    }
    for (i, v) in rgb16.data.iter_mut().enumerate() {
        *v = (i * 7919 % 65521) as u16;
    }
    type Case<'a> = (&'a str, Box<dyn Fn() + Send + Sync + 'a>);
    let cases: Vec<Case> = vec![
        (
            "legacy tilt RGB8",
            Box::new(|| {
                std::hint::black_box(legacy_warp::warp_perspective(&src, &tilt, sw, sh));
            }),
        ),
        (
            "new copy path (identity)",
            Box::new(|| {
                std::hint::black_box(warp_perspective(&src, &id, sw, sh));
            }),
        ),
        (
            "new half-pixel shift RGB8",
            Box::new(|| {
                std::hint::black_box(warp_perspective(&src, &shift, sw, sh));
            }),
        ),
        (
            "new tilt RGB8",
            Box::new(|| {
                std::hint::black_box(warp_perspective(&src, &tilt, sw, sh));
            }),
        ),
        (
            "new tilt Gray8",
            Box::new(|| {
                std::hint::black_box(
                    warp_perspective_image(gray.as_ref(), &tilt, sw, sh, &NeverCancel).unwrap(),
                );
            }),
        ),
        (
            "new tilt RGBA8",
            Box::new(|| {
                std::hint::black_box(
                    warp_perspective_image(rgba.as_ref(), &tilt, sw, sh, &NeverCancel).unwrap(),
                );
            }),
        ),
        (
            "new tilt RGB16",
            Box::new(|| {
                std::hint::black_box(
                    warp_perspective_image(rgb16.as_ref(), &tilt, sw, sh, &NeverCancel).unwrap(),
                );
            }),
        ),
    ];
    let rounds = 24;
    let mut best = vec![f64::MAX; cases.len()];
    for _ in 0..rounds {
        for (i, (_, f)) in cases.iter().enumerate() {
            let t = Instant::now();
            pool.install(f);
            best[i] = best[i].min(t.elapsed().as_secs_f64() * 1e3);
        }
    }
    println!("{threads} thread(s), {sw}x{sh}, min of {rounds} round-robin rounds:");
    for (i, (name, _)) in cases.iter().enumerate() {
        println!(
            "  {name:<28} {:7.1} ns/px   x{:.2} of legacy",
            best[i] * 1e6 / px,
            best[i] / best[0]
        );
    }
}
