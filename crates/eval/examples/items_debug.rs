// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Prints the multi-item detector's diagnostics for one image (local debugging aid).
//!
//! ```text
//! cargo run --release -p auto-crop-eval --example items_debug -- <image>
//! ```

use auto_crop_codecs::decode;
use auto_crop_codecs::{Format, encode};
use auto_crop_imgproc::items::{ItemsOptions, debug_masks, detect_items};

fn main() {
    let path = std::env::args().nth(1).expect("usage: items_debug <image>");
    let bytes = std::fs::read(&path).expect("read");
    let img = decode(&bytes).expect("decode").raster;
    let det = detect_items(&img, &ItemsOptions::default());
    if let Some(out) = std::env::args().nth(2) {
        let m = debug_masks(&img, &ItemsOptions::default());
        std::fs::write(out, encode(&m, Format::Png, 90, None).expect("encode")).expect("write");
    }
    println!("{:#?}", det.diagnostics);
    println!("{:#?}", det.scan_flags);
    for (i, it) in det.items.iter().enumerate() {
        println!(
            "item {i}: {:?} fill {:.2} partial {} signals {:?} reasons {:?}",
            it.kind,
            it.fill,
            it.partial_frame,
            it.signals,
            it.confidence
                .reasons
                .iter()
                .map(|r| r.code)
                .collect::<Vec<_>>()
        );
    }
}
