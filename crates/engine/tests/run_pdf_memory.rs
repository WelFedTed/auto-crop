// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Scan to PDF holds one page at a time (ROADMAP M11.31, M11.65 in miniature): the peak heap of a
//! 40-page run is not larger than that of a 4-page run by more than a page's worth of slack, so the
//! pages are neither kept decoded nor kept as encoded bytes until the end. A negative control (the
//! naive plan, every page's pixels held until the end) is computed here from the same pages and
//! must be far above the bar, so the bar can fail.

use auto_crop_codecs::Format;
use auto_crop_core::{CancelToken, Pt};
use auto_crop_engine::run_pdf::{PdfRunOptions, PdfSelect};
use auto_crop_engine::{AppPaths, Edit, Engine, NoSplit};
use auto_crop_imgproc::Raster;
use peak_alloc::PeakAlloc;
use std::fs;
use std::sync::Arc;

// A counting allocator from a maintained crate (it carries the `unsafe`; this crate forbids it).
#[global_allocator]
static ALLOC: PeakAlloc = PeakAlloc;

/// Noise that does not compress: a page's JPEG is a sizeable part of what a buffering run would hold.
fn noisy(w: u32, h: u32) -> Vec<u8> {
    let mut s = 0x9E37_79B9_7F4A_7C15u64;
    (0..w as usize * h as usize * 3)
        .map(|i| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            ((s >> 40) as u8 / 2).wrapping_add((i % 251) as u8 / 2)
        })
        .collect()
}

#[test]
fn the_peak_heap_does_not_grow_with_the_number_of_pages() {
    let root = tempfile::tempdir().unwrap();
    let engine = Engine::new(AppPaths::under(&root.path().join("data")));
    engine.set_item_detector(Arc::new(NoSplit));
    let (w, h) = (1200u32, 900u32);
    let path = root.path().join("big.png");
    let rgb = noisy(w, h);
    fs::write(
        &path,
        auto_crop_codecs::encode(&Raster::from_raw(w, h, rgb).unwrap(), Format::Png, 0, None)
            .unwrap(),
    )
    .unwrap();
    let id = engine.open_paths(&[path], false).ids[0];
    engine.analyse(id).unwrap();
    let edit = Edit {
        quad: [
            Pt::new(0.02, 0.02),
            Pt::new(0.98, 0.02),
            Pt::new(0.98, 0.98),
            Pt::new(0.02, 0.98),
        ],
        quarter_turns: 0,
        fine_deg: 0.0,
    };
    engine.set_edit(id, &edit, false, "crop").unwrap();
    let peak_of = |pages: usize, name: &str| {
        let select = vec![
            PdfSelect {
                item: id,
                crop: None
            };
            pages
        ];
        let baseline = ALLOC.current_usage();
        ALLOC.reset_peak_usage();
        let saved = engine
            .save_run_pdf(
                &select,
                &root.path().join(name),
                &PdfRunOptions::default(),
                &CancelToken::never(),
                &|_| {},
            )
            .unwrap();
        assert_eq!(saved.pages as usize, pages);
        (
            ALLOC.peak_usage().saturating_sub(baseline),
            saved.bytes as usize,
        )
    };
    let (small, small_bytes) = peak_of(4, "four.pdf");
    let (large, large_bytes) = peak_of(40, "forty.pdf");
    eprintln!(
        "peak 4 pages {} KiB ({} KiB file), 40 pages {} KiB ({} KiB file)",
        small / 1024,
        small_bytes / 1024,
        large / 1024,
        large_bytes / 1024
    );
    // The files grow ten-fold; the peak does not (a few MiB of slack for the allocator and caches).
    assert!(large_bytes > small_bytes * 8);
    assert!(
        large <= small + 6 * 1024 * 1024,
        "the peak grew with the page count: {small} -> {large}"
    );
    // The negative control: holding every page's decoded pixels (what a buffering writer would) is
    // 40 pages of 1152 x 864 x 3 bytes, far above the slack the bar allows.
    let buffering = 40 * (1152usize * 864 * 3);
    assert!(buffering > small + 6 * 1024 * 1024 * 4);
}
