// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! ROADMAP M1.19 without the `turbojpeg` feature: the `Want::Scaled` contract (sizes, orientation
//! after scaling, never upscaling) holds for the safe-Rust fallback, and ImageMagick's libjpeg-turbo
//! DCT-scaled decode (when installed) validates the 35 dB PSNR bound that the feature's own tests
//! check against the pinned library in CI.

// Two of the tests below describe the safe-Rust fallback and are compiled out with the feature.
#![cfg_attr(feature = "turbojpeg", allow(unused_imports, dead_code))]

use crate::fixtures::{JpegSpec, orient_reference, photo, png_rgb, psnr};
use crate::scaled::block_mean;
use crate::{DecodeLimits, decode, decode_scaled};
use auto_crop_core::ports::Want;

fn limits() -> DecodeLimits {
    DecodeLimits::default()
}

fn jpeg(w: u32, h: u32, orientation: Option<u16>) -> Vec<u8> {
    let mut s = JpegSpec::new(w, h);
    s.sampling = (2, 2);
    s.exif_orientation = orientation;
    s.encode(&photo(w, h, 7), jpeg_encoder::ColorType::Rgb)
}

#[test]
fn the_reduction_and_the_oriented_size_follow_the_request() {
    // 97 x 61, orientation 6 (turn 90 degrees): min_edge 20 -> 1/4 (ceil(97/4) = 25 >= 20), stored
    // 25 x 16, shown 16 x 25.
    let bytes = jpeg(97, 61, Some(6));
    let r = decode_scaled(&bytes, Want::Scaled { min_edge: 20 }, &limits()).unwrap();
    assert_eq!(r.denom, 4);
    assert_eq!((r.source_width, r.source_height), (97, 61));
    assert_eq!((r.decoded.raster.width, r.decoded.raster.height), (16, 25));
    assert_eq!(r.decoded.exif_orientation, 6);
    // min_edge 26 needs 1/2 (49 x 31), shown 31 x 49.
    let r = decode_scaled(&bytes, Want::Scaled { min_edge: 26 }, &limits()).unwrap();
    assert_eq!(r.denom, 2);
    assert_eq!((r.decoded.raster.width, r.decoded.raster.height), (31, 49));
}

// Exact equality with `decode` holds for the safe-Rust path; libjpeg-turbo differs by a few LSB
// (checked by the feature's own tests).
#[cfg(not(feature = "turbojpeg"))]
#[test]
fn full_and_oversized_requests_return_the_full_decode_and_never_upscale() {
    let bytes = jpeg(80, 48, Some(3));
    let full = decode(&bytes).unwrap();
    for want in [
        Want::Full,
        Want::Scaled { min_edge: 80 },
        Want::Scaled { min_edge: 10_000 },
    ] {
        let r = decode_scaled(&bytes, want, &limits()).unwrap();
        assert_eq!(r.denom, 1, "{want:?}");
        assert_eq!(r.decoded.raster, full.raster, "{want:?}");
    }
}

#[test]
fn formats_without_a_native_reduction_come_back_at_full_size() {
    let png = png_rgb(40, 30);
    let r = decode_scaled(&png, Want::Scaled { min_edge: 1 }, &limits()).unwrap();
    assert_eq!(r.denom, 1);
    assert_eq!(r.decoded.raster, decode(&png).unwrap().raster);
}

#[cfg(not(feature = "turbojpeg"))]
#[test]
fn the_fallback_is_the_block_average_of_the_stored_pixels_then_turned() {
    let (w, h) = (97u32, 61u32);
    let plain = jpeg(w, h, None);
    let base = decode(&plain).unwrap().raster;
    let small = block_mean(&base, 8);
    for o in 1u8..=8 {
        let tagged = jpeg(w, h, Some(u16::from(o)));
        let r = decode_scaled(&tagged, Want::Scaled { min_edge: 10 }, &limits()).unwrap();
        assert_eq!(r.denom, 8);
        let (want, ww, wh) = orient_reference(&small.data, small.width, small.height, o);
        let got = &r.decoded.raster;
        assert_eq!((got.width, got.height), (ww, wh), "orientation {o}");
        assert_eq!(got.data, want, "orientation {o}: reduce first, then turn");
    }
}

#[test]
fn the_block_average_has_the_dct_geometry_and_rounds_partial_edge_blocks() {
    // 5 x 3, reduced by 2: 3 x 2 outputs; the last column and row cover fewer pixels.
    let mut r = auto_crop_imgproc::Raster::new(5, 3);
    for (i, px) in r.data.as_chunks_mut::<3>().0.iter_mut().enumerate() {
        px.fill(i as u8 * 10);
    }
    let b = block_mean(&r, 2);
    assert_eq!((b.width, b.height), (3, 2));
    // Block (0, 0): pixels 0, 1, 5, 6 -> (0 + 10 + 50 + 60) / 4 = 30. Block (2, 1): pixel 14 only.
    assert_eq!(&b.data[0..3], &[30, 30, 30]);
    assert_eq!(&b.data[(3 + 2) * 3..(3 + 2) * 3 + 3], &[140, 140, 140]);
    assert_eq!(block_mean(&r, 1), r);
}

// ------------------------------------------------- ImageMagick (libjpeg-turbo) as the DCT oracle

/// Decodes with ImageMagick at libjpeg's scale factor `denom` (the `jpeg:size` hint), returning
/// the RGB bytes and size; `None` when `magick` is missing or the hint did not give `denom`.
fn magick_scaled(bytes: &[u8], w: u32, h: u32, denom: u32) -> Option<(Vec<u8>, u32, u32)> {
    let dir = std::env::temp_dir().join(format!(
        "auto-crop-scaled-{}-{w}x{h}-{denom}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).ok()?;
    let (src, dst) = (dir.join("in.jpg"), dir.join("out.ppm"));
    std::fs::write(&src, bytes).ok()?;
    // A hint of floor(size / denom) makes the size ratio at least `denom` and below `2 * denom`.
    let hint = format!("{}x{}", (w / denom).max(1), (h / denom).max(1));
    let ok = std::process::Command::new("magick")
        .args(["-define", &format!("jpeg:size={hint}")])
        .arg(&src)
        .arg(format!("ppm:{}", dst.display()))
        .output()
        .ok()?
        .status
        .success();
    let raw = ok.then(|| std::fs::read(&dst).ok()).flatten();
    let _ = std::fs::remove_dir_all(&dir);
    let raw = raw?;
    // "P6\nW H\n255\n" then the pixels.
    let mut parts = raw.splitn(4, |b| b.is_ascii_whitespace());
    (parts.next()? == b"P6").then_some(())?;
    let ow: u32 = std::str::from_utf8(parts.next()?).ok()?.parse().ok()?;
    let oh: u32 = std::str::from_utf8(parts.next()?).ok()?.parse().ok()?;
    let rest = parts.next()?; // "255\n<pixels>"
    let pixels = rest.get(rest.iter().position(|b| *b == b'\n')? + 1..)?;
    (pixels.len() == ow as usize * oh as usize * 3).then(|| (pixels.to_vec(), ow, oh))
}

#[test]
fn libjpeg_turbo_dct_scaling_stays_within_35_db_of_the_block_average() {
    let mut worst = [f64::INFINITY; 3];
    let mut compared = 0;
    for (i, (w, h)) in [(200u32, 150u32), (333, 257), (640, 480), (1025, 769)]
        .into_iter()
        .enumerate()
    {
        let bytes = {
            let mut s = JpegSpec::new(w, h);
            s.sampling = (2, 2);
            s.quality = 92;
            s.encode(&photo(w, h, i as u64 + 1), jpeg_encoder::ColorType::Rgb)
        };
        let full = decode(&bytes).unwrap().raster;
        for (k, denom) in [2u32, 4, 8].into_iter().enumerate() {
            let Some((dct, sw, sh)) = magick_scaled(&bytes, w, h, denom) else {
                eprintln!("magick not available (or no {denom}x hint): skipping");
                return;
            };
            if (sw, sh) != (w.div_ceil(denom), h.div_ceil(denom)) {
                eprintln!("{w}x{h} 1/{denom}: magick gave {sw}x{sh}, skipping this case");
                continue;
            }
            let area = block_mean(&full, denom);
            let p = psnr(&dct, &area.data);
            worst[k] = worst[k].min(p);
            compared += 1;
            assert!(p >= 35.0, "{w}x{h} 1/{denom}: PSNR {p:.2} dB");
        }
    }
    println!(
        "libjpeg-turbo DCT scaling vs block average ({compared} cases): worst PSNR 1/2 {:.1} dB, 1/4 {:.1} dB, 1/8 {:.1} dB",
        worst[0], worst[1], worst[2]
    );
}
