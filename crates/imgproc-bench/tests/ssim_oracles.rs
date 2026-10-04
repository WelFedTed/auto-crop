// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! SSIM oracle checks for the warp (ROADMAP M1.25): the `image-compare` MSSIM (never `dssim-core`,
//! which is AGPL) of the kernel's output against the NumPy Lanczos3 reference and the OpenCV
//! `warpPerspective` fixtures that `crates/imgproc/tests/oracles.rs` checks by PSNR. It lives in
//! this crate because `image-compare` pulls the `image` crate, which `check-deps` keeps out of
//! `auto-crop-imgproc`. The bar is >= 0.99 on one OS; every fixture clears it here, including the
//! sharp-edged `blur0` case (0.9965). The >= 0.98 bar across operating systems is measured by
//! `examples/warp_oracle_report.rs` in the `Warp oracles` workflow (all outputs were byte-identical
//! on windows-2025, macos-latest and ubuntu-22.04; see docs/perf/kernels.md).

use auto_crop_imgproc::cancel::NeverCancel;
use auto_crop_imgproc::pixels::ImageRef;
use auto_crop_imgproc::warp::warp_perspective_image;
use image::RgbImage;
use image_compare::{Algorithm, rgb_similarity_structure};
use serde_json::Value;

fn fixture(name: &str) -> Value {
    let path = format!(
        "{}/../imgproc/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}")))
        .unwrap()
}

fn hex_bytes(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn run(file: &str, min_for: impl Fn(&str) -> f64) {
    let doc = fixture(file);
    let mut checked = 0;
    for c in doc["cases"].as_array().unwrap() {
        // 8-bit RGB cases only (image-compare takes `RgbImage`).
        if c["bits"].as_u64() != Some(8) || c["channels"].as_u64() != Some(3) {
            continue;
        }
        let name = c["name"].as_str().unwrap();
        let (w, h) = (
            c["w"].as_u64().unwrap() as u32,
            c["h"].as_u64().unwrap() as u32,
        );
        let (ow, oh) = (
            c["out_w"].as_u64().unwrap() as u32,
            c["out_h"].as_u64().unwrap() as u32,
        );
        let m: Vec<f64> = c["matrix"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect();
        let m: [f64; 9] = m.try_into().unwrap();
        let src = hex_bytes(c["src"].as_str().unwrap());
        let want = hex_bytes(c["expected"].as_str().unwrap());
        let ours = warp_perspective_image(
            ImageRef::new(w, h, 3, &src[..]).unwrap(),
            &m,
            ow,
            oh,
            &NeverCancel,
        )
        .unwrap();
        let a = RgbImage::from_raw(ow, oh, ours.data).unwrap();
        let b = RgbImage::from_raw(ow, oh, want).unwrap();
        let score = rgb_similarity_structure(&Algorithm::MSSIMSimple, &a, &b)
            .unwrap()
            .score;
        println!("{file} {name}: MSSIM {score:.5}");
        assert!(score >= min_for(name), "{name}: MSSIM {score:.5}");
        checked += 1;
    }
    assert!(checked >= 2, "{file}: no RGB8 cases checked");
}

#[test]
fn warp_ssim_against_the_numpy_lanczos3_reference_is_at_least_0_99() {
    run("warp_numpy_lanczos3.json", |_| 0.99);
}

#[test]
fn warp_ssim_against_cv2_warp_perspective_meets_the_bar() {
    run("warp_cv2_lanczos4.json", |_| 0.99);
}
