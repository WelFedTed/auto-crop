// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Deterministic synthetic JPEGs of a given size for the memory profile and the batch benchmark
//! (ROADMAP M1.60). A page photographed on a desk, rendered by `auto_crop_imgproc::synth` with
//! per-image jitter derived from the index; never committed, regenerated on demand.
//!
//! STAND-IN: these are not real photographs. Their sensor-like grain gives the JPEG decoder a
//! realistic amount of entropy (about 0.3-0.5 bytes per pixel at q92), but the content is smooth
//! shapes, so decode and detect times are only indicative until the real perf corpus (M0.48,
//! M1.53) exists.

use auto_crop_codecs::{Format, encode};
use auto_crop_imgproc::Raster;
use auto_crop_imgproc::synth::{PaperKind, Rng, Scene, render_scene};

/// JPEG quality of the generated files.
pub const GEN_QUALITY: u8 = 92;

/// Width and height of a 4:3 image of about `megapixels` MP (12 gives 4000 x 3000).
pub fn dims_for_megapixels(megapixels: f64) -> (u32, u32) {
    let h = (megapixels * 1e6 * 3.0 / 4.0).sqrt();
    ((h * 4.0 / 3.0).round() as u32, h.round() as u32)
}

/// The scene of image number `index`: corners jittered by a few percent, colours and seed
/// derived from the index. The same `(width, height, index)` always gives the same scene.
pub fn scene(width: u32, height: u32, index: u64) -> Scene {
    let mut rng = Rng::new(index ^ 0xB3A7_C0DE);
    let mut j = |centre: f64, spread: f64| centre + (f64::from(rng.unit()) - 0.5) * spread;
    let corners = [
        (j(0.16, 0.06), j(0.10, 0.06)),
        (j(0.84, 0.06), j(0.10, 0.06)),
        (j(0.84, 0.06), j(0.90, 0.06)),
        (j(0.16, 0.06), j(0.90, 0.06)),
    ];
    let mut rng = Rng::new(index ^ 0x51DE);
    let mut tone = |base: u8, spread: u32| {
        base.saturating_add_signed((rng.below(spread) as i32 - spread as i32 / 2) as i8)
    };
    Scene {
        width,
        height,
        background: [tone(110, 60), tone(95, 60), tone(75, 60)],
        paper: [tone(240, 14), tone(238, 14), tone(230, 14)],
        ink: [60, 64, 76],
        kind: PaperKind::Document,
        corners,
        seed: index,
        noise: 8.0,
        blur_radius: 1,
        shadow: true,
    }
}

/// Like [`scene`] but the page nearly fills the frame (3% margin), so the rectified output is
/// about as large as the source: the worst case for memory.
pub fn scene_filling_frame(width: u32, height: u32, index: u64) -> Scene {
    let mut s = scene(width, height, index);
    s.corners = [(0.03, 0.03), (0.97, 0.04), (0.96, 0.97), (0.04, 0.96)];
    s
}

/// Image number `index` as an RGB raster.
pub fn raster(width: u32, height: u32, index: u64) -> Raster {
    render_scene(&scene(width, height, index))
}

/// Image number `index` as JPEG bytes.
pub fn jpeg(width: u32, height: u32, index: u64) -> Vec<u8> {
    jpeg_of(&scene(width, height, index))
}

/// Image number `index` with the page filling the frame, as JPEG bytes.
pub fn jpeg_filling_frame(width: u32, height: u32, index: u64) -> Vec<u8> {
    jpeg_of(&scene_filling_frame(width, height, index))
}

fn jpeg_of(s: &Scene) -> Vec<u8> {
    encode(&render_scene(s), Format::Jpeg, GEN_QUALITY, None)
        .expect("a synthetic raster always encodes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skeleton::{Input, Options, run};
    use auto_crop_core::CancelToken;

    #[test]
    fn megapixel_sizes_are_4_3_and_close() {
        assert_eq!(dims_for_megapixels(12.0), (4000, 3000));
        assert_eq!(dims_for_megapixels(48.0), (8000, 6000));
        let (w, h) = dims_for_megapixels(100.0);
        assert_eq!((w, h), (11547, 8660));
        assert!((u64::from(w) * u64::from(h)).abs_diff(100_000_000) < 10_000);
    }

    #[test]
    fn the_same_index_gives_the_same_bytes_and_another_index_does_not() {
        let a = jpeg(320, 240, 3);
        assert_eq!(a, jpeg(320, 240, 3));
        assert_ne!(a, jpeg(320, 240, 4));
    }

    #[test]
    fn a_filling_page_gives_an_output_close_to_the_source_size() {
        let bytes = jpeg_filling_frame(800, 600, 2);
        let out = run(
            Input::Bytes(&bytes),
            &Options::default(),
            &CancelToken::never(),
        )
        .expect("runs");
        let px = u64::from(out.report.output.0) * u64::from(out.report.output.1);
        assert!(out.report.quad_found);
        assert!(px > 800 * 600 * 8 / 10, "{px}");
    }

    #[test]
    fn the_pipeline_finds_the_page_in_every_generated_scene() {
        for index in 0..6 {
            let bytes = jpeg(800, 600, index);
            let out = run(
                Input::Bytes(&bytes),
                &Options::default(),
                &CancelToken::never(),
            )
            .expect("runs");
            assert!(out.report.quad_found, "scene {index}");
            assert_eq!(out.report.source, (800, 600));
        }
    }
}
