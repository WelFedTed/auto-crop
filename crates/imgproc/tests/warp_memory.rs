// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Memory bound of the warp (ROADMAP M1.24): extra peak heap while warping stays within the output
//! image + 16 MB + the per-band scratch, i.e. there is no full-frame f32 copy of source or output.
//! A counting global allocator measures the peak above the heap in use when the call starts.
//!
//! This binary holds only these tests so that nothing else allocates concurrently; the 100 MP case
//! is `#[ignore]` (it allocates about 330 MB) and runs with `cargo test --release -- --ignored`.

use auto_crop_imgproc::cancel::NeverCancel;
use auto_crop_imgproc::homography::Homography;
use auto_crop_imgproc::pixels::Image;
use auto_crop_imgproc::warp::{BAND_ROWS, warp_perspective_image};
use peak_alloc::PeakAlloc;
use std::sync::Mutex;

// A counting allocator from a maintained crate (it carries the `unsafe`; this crate forbids it).
#[global_allocator]
static ALLOC: PeakAlloc = PeakAlloc;

/// The tests share the global counters, so they run one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

fn extra_peak_for(
    src_w: u32,
    src_h: u32,
    out_w: u32,
    out_h: u32,
    threads: usize,
) -> (usize, usize) {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap();
    let mut src = Image::<u8>::new(src_w, src_h, 3);
    for (i, v) in src.data.iter_mut().enumerate() {
        *v = (i.wrapping_mul(31) >> 3) as u8;
    }
    let (sw, sh) = (f64::from(src_w), f64::from(src_h));
    let (ow, oh) = (f64::from(out_w), f64::from(out_h));
    let quad = [
        (0.06 * sw, 0.10 * sh),
        (0.93 * sw, 0.05 * sh),
        (0.97 * sw, 0.92 * sh),
        (0.03 * sw, 0.95 * sh),
    ];
    let dst = [
        (-0.5, -0.5),
        (ow - 0.5, -0.5),
        (ow - 0.5, oh - 0.5),
        (-0.5, oh - 0.5),
    ];
    let m = Homography::from_quads(dst, quad).unwrap();
    // Warm the rayon pool and the lazily built weight table so they are not billed to the warp.
    pool.install(|| warp_perspective_image(src.as_ref(), &m.0, 8, 8, &NeverCancel).unwrap());
    let baseline = ALLOC.current_usage();
    ALLOC.reset_peak_usage();
    let out = pool
        .install(|| warp_perspective_image(src.as_ref(), &m.0, out_w, out_h, &NeverCancel))
        .unwrap();
    let extra = ALLOC.peak_usage().saturating_sub(baseline);
    assert_eq!((out.width, out.height), (out_w, out_h));
    (extra, out.data.len())
}

/// Per-band scratch: two position rows (i64) per worker thread plus slack for the thread pool.
fn band_budget(out_w: u32, threads: usize) -> usize {
    threads * (out_w as usize * 8 * 2 + BAND_ROWS * 64) + 256 * 1024
}

#[test]
fn warp_12mp_extra_heap_is_the_output_plus_16_mb_plus_bands() {
    for threads in [1usize, 8] {
        let (extra, out_bytes) = extra_peak_for(4000, 3000, 2480, 3508, threads);
        let budget = out_bytes + 16 * 1024 * 1024 + band_budget(2480, threads);
        println!(
            "12 MP, {threads} thread(s): extra {extra} B, output {out_bytes} B, budget {budget} B"
        );
        assert!(extra <= budget, "{extra} > {budget}");
        // And in fact barely more than the output.
        assert!(
            extra <= out_bytes + band_budget(2480, threads) + 1024 * 1024,
            "{extra}"
        );
    }
}

#[test]
#[ignore = "allocates about 330 MB; run with --release -- --ignored"]
fn warp_100mp_extra_heap_is_the_output_plus_16_mb_plus_bands() {
    let (extra, out_bytes) = extra_peak_for(11_547, 8_660, 2480, 3508, 8);
    let budget = out_bytes + 16 * 1024 * 1024 + band_budget(2480, 8);
    println!("100 MP, 8 threads: extra {extra} B, output {out_bytes} B, budget {budget} B");
    assert!(extra <= budget, "{extra} > {budget}");
}
