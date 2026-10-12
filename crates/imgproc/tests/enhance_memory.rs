// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Memory bound of the enhancer at 12 MP (PLAN 5.8, ROADMAP M7.61, M7.71): Clean works in place
//! and needs only the proxy and the gain grid; Document and Receipt add one flattened luma plane
//! and the result plane (2 bytes per pixel), never a full-frame `f32` copy. Like
//! `curved_memory.rs`, this binary holds only these tests so nothing else allocates concurrently.
//! The timing test is `#[ignore]`: `cargo test --release -p auto-crop-imgproc --test
//! enhance_memory -- --ignored --nocapture`.

use auto_crop_imgproc::Raster;
use auto_crop_imgproc::cancel::NeverCancel;
use auto_crop_imgproc::enhance::testpages::{Light, PageSpec, lit, page};
use auto_crop_imgproc::enhance::{Mode, Params, enhance, extra_bytes};
use peak_alloc::PeakAlloc;
use std::sync::Mutex;
use std::time::Instant;

#[global_allocator]
static ALLOC: PeakAlloc = PeakAlloc;

static SERIAL: Mutex<()> = Mutex::new(());

/// A shaded page of about 12 MP.
fn page_12mp() -> Raster {
    let spec = PageSpec::document(5).with_objects().sized(2928, 4099, 9);
    let p = page(&spec);
    lit(
        &p.truth,
        Light::Tilt { dx: 0.4, dy: 0.15 },
        [1.0, 0.95, 0.9],
        2.0,
        3,
    )
}

fn extra_peak(img: Raster, mode: Mode, strength: f32, threads: usize) -> usize {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap();
    // Warm the pool so its own allocations are not billed.
    pool.install(|| {
        enhance(
            Raster::filled(64, 64, [200, 200, 200]),
            &Params::new(mode, 1.0),
            None,
            &NeverCancel,
        )
    })
    .unwrap();
    let baseline = ALLOC.current_usage();
    ALLOC.reset_peak_usage();
    let (out, _) = pool
        .install(|| enhance(img, &Params::new(mode, strength), None, &NeverCancel))
        .unwrap();
    let extra = ALLOC.peak_usage().saturating_sub(baseline);
    println!(
        "  [{mode:?} x{threads}] baseline {baseline}, peak {}, after {}",
        ALLOC.peak_usage(),
        ALLOC.current_usage()
    );
    drop(out);
    extra
}

#[test]
fn clean_runs_in_place_and_document_adds_two_planes_at_12_mp() {
    let img = page_12mp();
    let px = u64::from(img.width) * u64::from(img.height);
    assert!(px > 11_000_000, "{px} pixels");
    let in_bytes = img.data.len();

    // Clean: in place. The extra heap is the proxy, its working copies and the grids: far below
    // one byte per pixel (the proxy is about 1.5 MP).
    let clean = extra_peak(img.clone(), Mode::Clean, 1.0, 8);
    println!(
        "clean: extra peak {:.1} MB ({:.3} B/px)",
        clean as f64 / 1e6,
        clean as f64 / px as f64
    );
    assert!(
        (clean as u64) <= extra_bytes(img.width, img.height, Mode::Clean) * 2,
        "clean {clean}"
    );
    // A full-frame f32 copy of the RGB image would be 12 bytes per pixel.
    assert!((clean as f64) < 2.0 * px as f64, "clean {clean}");

    // Document: the flattened luma plane and the result plane, plus the proxy work. The input
    // buffer is freed before the result is built, so the peak is below input + 2 planes.
    let doc = extra_peak(img.clone(), Mode::Document, 1.0, 8);
    println!(
        "document: extra peak {:.1} MB ({:.3} B/px); input is {:.1} MB",
        doc as f64 / 1e6,
        doc as f64 / px as f64,
        in_bytes as f64 / 1e6
    );
    assert!(
        (doc as u64) <= extra_bytes(img.width, img.height, Mode::Document) + 8 * 1024 * 1024,
        "document {doc} vs {}",
        extra_bytes(img.width, img.height, Mode::Document)
    );
    // A blend below full strength keeps the input alive until the end: one more plane set, still
    // bounded (the input is the caller's, already allocated).
    let half = extra_peak(img, Mode::Receipt, 0.5, 8);
    println!(
        "receipt at strength 0.5: extra peak {:.1} MB ({:.3} B/px)",
        half as f64 / 1e6,
        half as f64 / px as f64
    );
    assert!((half as f64) < 2.5 * px as f64, "receipt {half}");
}

#[test]
fn the_peak_does_not_grow_with_the_thread_count_beyond_the_band_scratch() {
    let img = page_12mp();
    let one = extra_peak(img.clone(), Mode::Document, 1.0, 1);
    let eight = extra_peak(img, Mode::Document, 1.0, 8);
    println!("1 thread {one}, 8 threads {eight}");
    // Per-strip scratch of the threshold: four u64 columns of the image width per thread.
    assert!(eight <= one + 8 * 4 * 8 * 4000 * 4, "{one} vs {eight}");
}

/// A large striped "page": shaded paper with dark text-like bars (cheap to build at any size).
fn big_page(w: u32, h: u32) -> Raster {
    let mut r = Raster::new(w, h);
    for y in 0..h {
        let light = 1.0 - 0.45 * (y as f32 / h as f32);
        for x in 0..w {
            let bar = (y / 40) % 3 == 0 && (x / 14) % 5 != 0 && y % 40 > 8 && y % 40 < 30;
            let v = if bar { 40.0 } else { 235.0 } * light;
            let i = ((y as usize * w as usize) + x as usize) * 3;
            r.data[i..i + 3].copy_from_slice(&[v as u8, v as u8, (v * 0.92) as u8]);
        }
    }
    r
}

#[test]
#[ignore = "48 and 100 MP: slow and memory hungry, run by hand or nightly (ROADMAP M7.61)"]
fn the_peak_at_48_and_100_mp_is_two_bytes_per_pixel_not_a_float_copy() {
    for (w, h) in [(6928u32, 6928u32), (10000, 10000)] {
        let img = big_page(w, h);
        let px = u64::from(w) * u64::from(h);
        for mode in [Mode::Clean, Mode::Document] {
            let t = Instant::now();
            let extra = extra_peak(img.clone(), mode, 1.0, 8);
            println!(
                "{:.0} MP {mode:?}: extra peak {:.1} MB ({:.2} B/px), {:.0} ms",
                px as f64 / 1e6,
                extra as f64 / 1e6,
                extra as f64 / px as f64,
                t.elapsed().as_secs_f64() * 1e3
            );
            // The result plane and the flattened luma: 2 bytes per pixel at most, plus the proxy.
            assert!(
                (extra as u64) <= 2 * px + extra_bytes(w, h, mode) + 16 * 1024 * 1024,
                "{w} x {h} {mode:?}: {extra}"
            );
        }
    }
}

#[test]
#[ignore = "measures, does not assert a budget"]
fn time_the_modes_at_12_mp() {
    let img = page_12mp();
    for mode in [Mode::Clean, Mode::Document, Mode::Receipt] {
        let mut best = f64::MAX;
        let mut report = None;
        for _ in 0..5 {
            let src = img.clone();
            let t = Instant::now();
            let (_, rep) = enhance(src, &Params::new(mode, 1.0), None, &NeverCancel).unwrap();
            best = best.min(t.elapsed().as_secs_f64() * 1e3);
            report = Some(rep);
        }
        let r = report.unwrap();
        println!(
            "{mode:?}: best of 5 {best:.1} ms (last run: analysis {:.1} ms, apply {:.1} ms), text_h {:.0}, window {}\n    stages {:?}",
            r.analysis_ms, r.apply_ms, r.text_h, r.window, r.stage_ms
        );
    }
}
