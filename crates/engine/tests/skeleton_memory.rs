// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Memory profile of the pipeline skeleton (ROADMAP M1.60): the peak heap of one image from bytes
//! to JPEG stays within the per-job weight the memory budget admits it with, `pixels * 9 + 64 MiB`
//! (about three decoded RGB8 copies plus overhead: 175 MB at 12 MP, 500 MB at 48 MP, 1.0 GB at
//! 100 MP; PROVISIONAL). A counting global allocator measures the peak above the heap in use when
//! the run starts (the input bytes are already live then).
//!
//! This binary holds only these tests so that nothing else allocates concurrently. The 48 and
//! 100 MP cases allocate up to 1 GB and are `#[ignore]`; `cargo xtask perf memory` runs all three
//! with per-stage numbers and the budget table of `docs/perf/budgets.md`.

use auto_crop_core::CancelToken;
use auto_crop_engine::memory::job_weight;
use auto_crop_engine::skeleton::bench_images::{dims_for_megapixels, jpeg, jpeg_filling_frame};
use auto_crop_engine::skeleton::{Input, Options, run, thread_pool};
use peak_alloc::PeakAlloc;
use std::sync::Mutex;

// A counting allocator from a maintained crate (it carries the `unsafe`; this crate forbids it).
#[global_allocator]
static ALLOC: PeakAlloc = PeakAlloc;

/// The tests share the global counters, so they run one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

/// Extra peak heap in bytes for one run, and the pixel count.
fn extra_peak(megapixels: f64, threads: usize, fill: bool) -> (usize, u64) {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (w, h) = dims_for_megapixels(megapixels);
    let bytes = if fill {
        jpeg_filling_frame(w, h, 1)
    } else {
        jpeg(w, h, 1)
    };
    let opts = Options {
        pool: Some(thread_pool(threads).unwrap()),
        ..Options::default()
    };
    let token = CancelToken::never();
    // Warm the pool, the lazily built weight tables and the thread-locals on a tiny image so
    // they are not billed to the run.
    let tiny = jpeg(64, 48, 1);
    let _ = run(Input::Bytes(&tiny), &opts, &token);
    let baseline = ALLOC.current_usage();
    ALLOC.reset_peak_usage();
    let out = run(Input::Bytes(&bytes), &opts, &token).expect("pipeline runs");
    let extra = ALLOC.peak_usage().saturating_sub(baseline);
    assert_eq!(out.report.source, (w, h));
    (extra, u64::from(w) * u64::from(h))
}

fn check(megapixels: f64, threads: usize, fill: bool) {
    let (extra, px) = extra_peak(megapixels, threads, fill);
    let budget = job_weight(px) as usize;
    println!(
        "{megapixels} MP ({px} px), {threads} thread(s), page fills frame: {fill}: peak heap {:.1} MB, budget {:.1} MB, ratio {:.2}",
        extra as f64 / 1e6,
        budget as f64 / 1e6,
        extra as f64 / budget as f64
    );
    assert!(
        extra <= budget,
        "{megapixels} MP, {threads} thread(s): peak heap {extra} B exceeds the job weight {budget} B"
    );
}

#[test]
fn peak_heap_at_12_mp_is_within_three_rgb8_copies_plus_64_mib() {
    for threads in [1, 8] {
        check(12.0, threads, false);
        // Worst case: the page fills the frame, so the rectified output is about source-sized.
        check(12.0, threads, true);
    }
}

#[test]
#[ignore = "allocates up to 0.5 GB; run with --ignored (or cargo xtask perf memory)"]
fn peak_heap_at_48_mp_is_within_three_rgb8_copies_plus_64_mib() {
    check(48.0, 8, true);
}

#[test]
#[ignore = "allocates up to 1 GB; run with --ignored (or cargo xtask perf memory)"]
fn peak_heap_at_100_mp_is_within_three_rgb8_copies_plus_64_mib() {
    check(100.0, 8, true);
}
