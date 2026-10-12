// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Writes the synthetic enhancement test pages (`imgproc::enhance::testpages`) as PNG, lit by a few
//! illumination fields, so the enhancer can be looked at and benchmarked with `enhance_dir`.
//!
//! ```text
//! cargo run --release -p auto-crop-eval --example enhance_pages -- OUT_DIR [mp]
//! ```
//!
//! `mp` (default 1.4) scales the pages: 12 gives 12 MP pages for timing.

use auto_crop_codecs::{Format, encode};
use auto_crop_imgproc::enhance::testpages::{Light, PageSpec, fade, lit, page};

fn main() {
    let out = std::path::PathBuf::from(std::env::args().nth(1).expect("OUT_DIR"));
    let mp: f64 = std::env::args()
        .nth(2)
        .map_or(1.4, |s| s.parse().expect("mp"));
    std::fs::create_dir_all(&out).expect("mkdir");
    let k = (mp / 1.4).sqrt();
    let scale = (3.0 * k).round().max(1.0) as u32;
    let sized = |s: PageSpec| {
        let w = (f64::from(s.width) * k).round() as u32;
        let h = (f64::from(s.height) * k).round() as u32;
        s.sized(w, h, scale)
    };
    let cases: Vec<(&str, PageSpec, Light, [f32; 3], f32)> = vec![
        (
            "doc_flat",
            PageSpec::document(1),
            Light::Flat,
            [1.0; 3],
            1.0,
        ),
        (
            "doc_tilt",
            PageSpec::document(2).with_objects(),
            Light::Tilt { dx: 0.5, dy: 0.15 },
            [1.0; 3],
            1.0,
        ),
        (
            "doc_shadow",
            PageSpec::document(3),
            Light::Shadow {
                depth: 0.5,
                width: 150.0 * k as f32,
            },
            [1.0; 3],
            1.0,
        ),
        (
            "receipt_vignette",
            PageSpec::receipt(4).with_objects(),
            Light::Vignette { k: 0.55 },
            [1.0, 0.93, 0.82],
            1.0,
        ),
        (
            "receipt_faded",
            PageSpec::receipt(5),
            Light::Tilt { dx: 0.3, dy: 0.1 },
            [1.0; 3],
            0.30,
        ),
    ];
    for (name, spec, light, tint, keep) in cases {
        let spec = sized(spec);
        let p = page(&spec);
        let truth = if keep < 1.0 {
            fade(&p.truth, spec.paper, keep)
        } else {
            p.truth.clone()
        };
        let img = lit(&truth, light, tint, 3.0, 99);
        let path = out.join(format!("{name}.png"));
        std::fs::write(&path, encode(&img, Format::Png, 90, None).expect("encode")).expect("write");
        println!("{} {}x{}", path.display(), img.width, img.height);
    }
}
