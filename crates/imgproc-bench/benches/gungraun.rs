// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! M1.58: instruction-count benchmarks (gungraun / Valgrind Callgrind). Compiles everywhere (so
//! `clippy --all-targets` covers it) but can only be run on Linux, where Valgrind exists.
//!
//! Small fixed inputs so a run under Valgrind takes seconds; the metric is `Ir` (instructions),
//! which is deterministic, so the PR gate (`tools/perf_gate`, `.github/workflows/perf-gate.yml`)
//! can compare two builds exactly. Wall-clock benchmarks stay in the criterion benches.
//!
//! * Every input is generated in the (unmeasured) setup function from fixed seeds.
//! * Kernels run on the calling thread: rayon's global pool is built with one thread that *is* the
//!   current thread, so no work-stealing or spinning adds noise to the count.
//! * SIMD levels: the imgproc kernels have no hand-written SIMD and no runtime dispatch (they rely
//!   on autovectorisation, see `docs/perf/kernels.md`). The levels are therefore compile-time
//!   `target-cpu` levels, selected by the workflow matrix through `RUSTFLAGS`; each level is its
//!   own job and its own result file, so benchmark ids do not carry the level.
//! * `canary` exists only to prove the gate: `AUTO_CROP_GATE_CANARY_PCT=10` makes it do 10% more
//!   work (the workflow's dispatch input `canary_pct`), which must fail the comparison.

use auto_crop_codecs::fixtures::{jpeg_baseline, png_rgb};
use auto_crop_imgproc::Raster;
use auto_crop_imgproc::cancel::NeverCancel;
use auto_crop_imgproc::minify::warp_perspective_guarded;
use auto_crop_imgproc::pixels::ImageRef;
use auto_crop_imgproc::scale::resize_area;
use auto_crop_imgproc::threshold::{binarize, histogram, nick, otsu_threshold, sauvola};
use auto_crop_imgproc::warp::warp_perspective;
use auto_crop_imgproc_bench::{synthetic_photo, tilted_quad_homography};
use gungraun::{LibraryBenchmarkConfig, library_benchmark, library_benchmark_group, main};
use std::hint::black_box;

/// Makes every kernel run on the calling thread (idempotent; ignored if a pool exists already).
fn one_thread() {
    let _ = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .use_current_thread()
        .build_global();
}

fn gray_page(w: u32, h: u32) -> Vec<u8> {
    synthetic_photo(w, h, 3)
        .data
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| ((u32::from(p[0]) * 77 + u32::from(p[1]) * 150 + u32::from(p[2]) * 29) >> 8) as u8)
        .collect()
}

// ---- decode -------------------------------------------------------------------------------

fn make_jpeg() -> Vec<u8> {
    one_thread();
    jpeg_baseline(256, 192)
}

fn make_png() -> Vec<u8> {
    one_thread();
    png_rgb(256, 192)
}

#[library_benchmark]
#[bench::jpeg_256x192(setup = make_jpeg)]
#[bench::png_256x192(setup = make_png)]
fn decode(bytes: Vec<u8>) -> usize {
    black_box(auto_crop_codecs::decode(black_box(&bytes)))
        .map(|d| d.raster.data.len())
        .unwrap_or(0)
}

// ---- resize -------------------------------------------------------------------------------

fn make_src_512x384() -> Raster {
    one_thread();
    synthetic_photo(512, 384, 7)
}

#[library_benchmark]
#[bench::area_512x384_to_128x96(setup = make_src_512x384)]
fn resize(src: Raster) -> Raster {
    black_box(resize_area(black_box(&src), 128, 96))
}

// ---- warp ---------------------------------------------------------------------------------

#[library_benchmark]
#[bench::plain_512x384_to_256x352(setup = make_src_512x384)]
fn warp(src: Raster) -> Raster {
    let h = tilted_quad_homography(src.width, src.height, 256, 352);
    black_box(warp_perspective(black_box(&src), &h, 256, 352))
}

fn make_src_1024x768() -> Raster {
    one_thread();
    synthetic_photo(1024, 768, 7)
}

#[library_benchmark]
#[bench::guarded_1024x768_to_256x192(setup = make_src_1024x768)]
fn warp_guarded(src: Raster) -> usize {
    let h = tilted_quad_homography(src.width, src.height, 256, 192);
    let img = ImageRef::new(src.width, src.height, 3, &src.data[..]).expect("rgb8 image");
    black_box(
        warp_perspective_guarded(black_box(img), &h, 256, 192, &NeverCancel)
            .expect("never cancelled")
            .data
            .len(),
    )
}

// ---- threshold ----------------------------------------------------------------------------

fn make_gray() -> Vec<u8> {
    one_thread();
    gray_page(256, 192)
}

#[library_benchmark]
#[bench::otsu_binarize_256x192(setup = make_gray)]
fn otsu(gray: Vec<u8>) -> Vec<u8> {
    black_box(binarize(&gray, otsu_threshold(black_box(&gray))))
}

#[library_benchmark]
#[bench::sauvola_w31_256x192(setup = make_gray)]
fn sauvola_w31(gray: Vec<u8>) -> Vec<u8> {
    black_box(sauvola(black_box(&gray), 256, 192, 31, 0.25, 128.0))
}

#[library_benchmark]
#[bench::nick_w31_256x192(setup = make_gray)]
fn nick_w31(gray: Vec<u8>) -> Vec<u8> {
    black_box(nick(black_box(&gray), 256, 192, 31, -0.1))
}

// ---- canary -------------------------------------------------------------------------------

/// Repetitions of the histogram kernel for a canary percentage (`1000 + pct` for `pct` in 0..=100,
/// so +10 is exactly 10% more kernel work).
fn canary_reps() -> u32 {
    let pct: u32 = std::env::var("AUTO_CROP_GATE_CANARY_PCT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    1000 + pct.min(100)
}

#[library_benchmark]
#[bench::histogram_x1000(setup = make_gray)]
fn canary(gray: Vec<u8>) -> u64 {
    let mut acc = 0u64;
    for _ in 0..canary_reps() {
        acc = acc.wrapping_add(black_box(histogram(black_box(&gray)))[128]);
    }
    black_box(acc)
}

library_benchmark_group!(
    name = kernels,
    benchmarks = [
        decode,
        resize,
        warp,
        warp_guarded,
        otsu,
        sauvola_w31,
        nick_w31,
        canary
    ]
);

main!(
    config = LibraryBenchmarkConfig::default().pass_through_env("AUTO_CROP_GATE_CANARY_PCT"),
    library_benchmark_groups = kernels
);
