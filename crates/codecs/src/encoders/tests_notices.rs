// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The decoder's flags for what a lossless writer of this build could not give back, which the
//! engine's replaceability rule reads (`webp.lossy`, `tiff.low_depth`).

use super::*;
use crate::fixtures::{self, pattern};
use auto_crop_core::output::Bilevel;
use auto_crop_core::ports::{Raster, Samples};

fn notices(bytes: &[u8]) -> Vec<&'static str> {
    crate::decode(bytes).unwrap().notices
}

#[test]
fn a_lossy_webp_is_flagged_and_a_lossless_one_is_not() {
    assert!(notices(&fixtures::webp_lossy_1x1()).contains(&"webp.lossy"));
    assert!(!notices(&fixtures::webp_lossless(16, 16)).contains(&"webp.lossy"));
    let r = Raster::from_samples(20, 20, PixelFormat::Rgb8, Samples::U8(pattern(20, 20))).unwrap();
    let ours = WebpEncoder::default()
        .encode(
            &r.view(),
            &OutputSpec::default(),
            &EncodeMeta::default(),
            &CancelToken::never(),
        )
        .unwrap();
    assert!(!notices(&ours).contains(&"webp.lossy"));
}

#[test]
fn a_one_bit_tiff_is_flagged_and_an_eight_bit_one_is_not() {
    assert!(notices(&fixtures::tiff_g4(40, 30)).contains(&"tiff.low_depth"));
    assert!(notices(&fixtures::tiff_bilevel_raw(40, 30, true)).contains(&"tiff.low_depth"));
    let r = Raster::from_samples(20, 20, PixelFormat::Rgb8, Samples::U8(pattern(20, 20))).unwrap();
    let ours = TiffEncoder::default()
        .encode(
            &r.view(),
            &OutputSpec::default(),
            &EncodeMeta::default(),
            &CancelToken::never(),
        )
        .unwrap();
    assert!(notices(&ours).is_empty());
    // A bilevel file of ours is flagged too: replacing it with a 24-bit one would not be right.
    let g =
        Raster::from_samples(40, 30, PixelFormat::Gray8, Samples::U8(vec![200; 40 * 30])).unwrap();
    let g4 = OutputSpec {
        bits: Bits::One {
            bilevel: Bilevel::TiffG4,
        },
        ..OutputSpec::default()
    };
    let bytes = TiffEncoder::default()
        .encode(
            &g.view(),
            &g4,
            &EncodeMeta::default(),
            &CancelToken::never(),
        )
        .unwrap();
    assert!(notices(&bytes).contains(&"tiff.low_depth"));
}
