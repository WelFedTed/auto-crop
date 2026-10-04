// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Oracle tests (ROADMAP M1.23, M1.25): the homography solver against `cv2.getPerspectiveTransform`,
//! the warp against a NumPy float64 Lanczos3 reference and against `cv2.warpPerspective`, plus the
//! exactness cases (identity, integer shifts, 90-degree turns). The fixtures come from
//! `tools/imgproc-oracles/gen_oracles.py`; nothing here needs Python.

use auto_crop_imgproc::cancel::NeverCancel;
use auto_crop_imgproc::homography::Homography;
use auto_crop_imgproc::pixels::{Image, ImageRef, Sample};
use auto_crop_imgproc::warp::warp_perspective_image;
use serde_json::Value;

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&text).unwrap()
}

fn hex_bytes(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn floats(v: &Value) -> Vec<f64> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap())
        .collect()
}

/// One warp fixture case, decoded.
struct Case {
    name: String,
    w: u32,
    h: u32,
    channels: u8,
    bits: u64,
    src: Vec<u8>,
    matrix: [f64; 9],
    out_w: u32,
    out_h: u32,
    expected: Vec<u8>,
    /// cv2 fixtures only (0.0 elsewhere): see `warp_psnr_against_cv2_warp_perspective_meets_the_bar`.
    kernel_floor_db: f64,
    numpy_lanczos3_vs_cv2_db: f64,
    cv2_vs_lanczos4_model_db: f64,
}

fn cases(file: &str) -> Vec<Case> {
    let doc = fixture(file);
    doc["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| Case {
            name: c["name"].as_str().unwrap().to_owned(),
            w: c["w"].as_u64().unwrap() as u32,
            h: c["h"].as_u64().unwrap() as u32,
            channels: c["channels"].as_u64().unwrap() as u8,
            bits: c["bits"].as_u64().unwrap(),
            src: hex_bytes(c["src"].as_str().unwrap()),
            matrix: floats(&c["matrix"]).try_into().unwrap(),
            out_w: c["out_w"].as_u64().unwrap() as u32,
            out_h: c["out_h"].as_u64().unwrap() as u32,
            expected: hex_bytes(c["expected"].as_str().unwrap()),
            kernel_floor_db: c["lanczos3_vs_lanczos4_db"].as_f64().unwrap_or(0.0),
            numpy_lanczos3_vs_cv2_db: c["numpy_lanczos3_vs_cv2_db"].as_f64().unwrap_or(0.0),
            cv2_vs_lanczos4_model_db: c["cv2_vs_lanczos4_model_db"].as_f64().unwrap_or(0.0),
        })
        .collect()
}

fn u16s(bytes: &[u8]) -> Vec<u16> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes(*b))
        .collect()
}

/// Runs the case through the kernel; returns (ours, expected) as widened integers.
fn run(c: &Case) -> (Vec<i64>, Vec<i64>) {
    if c.bits == 8 {
        let src = ImageRef::new(c.w, c.h, c.channels, &c.src[..]).unwrap();
        let out = warp_perspective_image(src, &c.matrix, c.out_w, c.out_h, &NeverCancel).unwrap();
        (
            out.data.iter().map(|v| i64::from(*v)).collect(),
            c.expected.iter().map(|v| i64::from(*v)).collect(),
        )
    } else {
        let data = u16s(&c.src);
        let src = ImageRef::new(c.w, c.h, c.channels, &data[..]).unwrap();
        let out = warp_perspective_image(src, &c.matrix, c.out_w, c.out_h, &NeverCancel).unwrap();
        (
            out.data.iter().map(|v| i64::from(*v)).collect(),
            u16s(&c.expected).iter().map(|v| i64::from(*v)).collect(),
        )
    }
}

#[test]
fn homography_matches_cv2_get_perspective_transform() {
    let doc = fixture("homography_cv2.json");
    let cases = doc["cases"].as_array().unwrap();
    assert!(cases.len() >= 40);
    let mut worst = 0.0f64;
    for (i, c) in cases.iter().enumerate() {
        let (s, d, h) = (floats(&c["src"]), floats(&c["dst"]), floats(&c["h"]));
        let pts =
            |v: &[f64]| -> [(f64, f64); 4] { std::array::from_fn(|k| (v[2 * k], v[2 * k + 1])) };
        let ours =
            Homography::from_quads(pts(&s), pts(&d)).unwrap_or_else(|e| panic!("case {i}: {e}"));
        for (k, (a, b)) in ours.0.iter().zip(&h).enumerate() {
            // Relative to the entry's own scale (h6, h7 are ~1e-4 while h2 is ~1e3).
            let err = (a - b).abs() / b.abs().max(1.0);
            worst = worst.max(err);
            assert!(err < 1e-6, "case {i} entry {k}: ours {a} cv2 {b}");
        }
        // And the geometric statement: the corners land where the oracle says.
        for k in 0..4 {
            let (u, v) = ours.apply(pts(&s)[k].0, pts(&s)[k].1).unwrap();
            assert!((u - pts(&d)[k].0).abs() < 1e-6 && (v - pts(&d)[k].1).abs() < 1e-6);
        }
    }
    println!("worst relative entry error vs cv2: {worst:.3e}");
}

#[test]
fn warp_is_within_one_lsb_of_the_numpy_lanczos3_reference() {
    let all = cases("warp_numpy_lanczos3.json");
    assert!(all.len() >= 24);
    let mut worst8 = 0i64;
    let mut worst16 = 0i64;
    for c in &all {
        let (ours, want) = run(c);
        assert_eq!(ours.len(), want.len(), "{}", c.name);
        let max = ours
            .iter()
            .zip(&want)
            .map(|(a, b)| (a - b).abs())
            .max()
            .unwrap();
        // 8-bit: within 1 LSB (M1.25). 16-bit: the same relative accuracy means a few LSB at most
        // for the table interpolation plus f32 accumulation; the roadmap's M6.39 bar is 2 LSB.
        let limit = if c.bits == 8 { 1 } else { 2 };
        assert!(max <= limit, "{}: max diff {max} > {limit}", c.name);
        if c.bits == 8 {
            worst8 = worst8.max(max);
        } else {
            worst16 = worst16.max(max);
        }
    }
    println!(
        "worst diff vs numpy: 8-bit {worst8} LSB, 16-bit {worst16} LSB over {} cases",
        all.len()
    );
}

#[test]
fn warp_psnr_against_cv2_warp_perspective_meets_the_bar() {
    for c in cases("warp_cv2_lanczos4.json") {
        let (ours, want) = run(&c);
        // Compare only pixels whose 8x8 OpenCV footprint is inside the source: OpenCV blends in
        // its constant border there, this kernel clamps to the edge pixels.
        let m = Homography(c.matrix);
        let (mut se, mut n) = (0.0f64, 0u64);
        for v in 0..c.out_h {
            for u in 0..c.out_w {
                let (x, y) = m.apply(f64::from(u), f64::from(v)).unwrap();
                if x < 5.0 || y < 5.0 || x > f64::from(c.w) - 6.0 || y > f64::from(c.h) - 6.0 {
                    continue;
                }
                for ch in 0..usize::from(c.channels) {
                    let i =
                        (v as usize * c.out_w as usize + u as usize) * usize::from(c.channels) + ch;
                    let d = (ours[i] - want[i]) as f64;
                    se += d * d;
                    n += 1;
                }
            }
        }
        assert!(n > 1000, "{}: mask too small", c.name);
        let psnr = 10.0 * (255.0f64 * 255.0 / (se / n as f64)).log10();
        println!(
            "{}: PSNR vs cv2 Lanczos4 {psnr:.2} dB over {n} samples",
            c.name
        );
        // What the 45 dB bar (PROVISIONAL) applies to, and why (docs/perf/kernels.md, M1.25).
        // OpenCV has no Lanczos3: INTER_LANCZOS4 is an 8 x 8 kernel with 1/32 px coordinates, ours
        // is a 6 x 6 kernel, so two correct resamplers differ by design, most on one-pixel edges.
        // The generator measures that unavoidable difference (an exact Lanczos3 against an exact
        // Lanczos4 on the same content, `lanczos3_vs_lanczos4_db`) and checks that cv2 itself
        // matches its Lanczos4 model to >= 70 dB (so the geometry, pixel-centre convention and
        // border handling are the ones modelled). Three assertions follow:
        //  1. 45 dB on content whose kernel floor is at least 46 dB (photo-like, band-limited);
        //  2. on harder content the bar is the floor minus 1 dB (we must not lose more than 1 dB
        //     to the kernel difference that no Lanczos3 can avoid);
        //  3. in every case we are as close to cv2 as the exact Lanczos3 reference is (0.25 dB).
        let floor = c.kernel_floor_db;
        let bar = if floor >= 46.0 { 45.0 } else { floor - 1.0 };
        assert!(psnr >= bar, "{}: PSNR {psnr:.2} dB < {bar:.2}", c.name);
        assert!(
            psnr >= c.numpy_lanczos3_vs_cv2_db - 0.25,
            "{}: {psnr:.2} dB is further from cv2 than the NumPy Lanczos3 reference ({:.2} dB)",
            c.name,
            c.numpy_lanczos3_vs_cv2_db
        );
        assert!(c.cv2_vs_lanczos4_model_db >= 70.0, "{}: fixture", c.name);
    }
}

fn noise<T: Sample + TryFrom<u32>>(w: u32, h: u32, ch: u8, modulus: u32) -> Image<T> {
    let mut img = Image::<T>::new(w, h, ch);
    let mut s = 0x2545_F491u32;
    for v in &mut img.data {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        *v = T::try_from(s % modulus).ok().unwrap();
    }
    img
}

/// Rotates by `turns` clockwise quarter turns by direct index mapping (the oracle).
fn rotate<T: Sample>(src: &Image<T>, turns: u32) -> Image<T> {
    let (w, h, c) = (src.width, src.height, usize::from(src.channels));
    let (ow, oh) = if turns % 2 == 1 { (h, w) } else { (w, h) };
    let mut out = Image::<T>::new(ow, oh, src.channels);
    for y in 0..oh {
        for x in 0..ow {
            let (sx, sy) = match turns % 4 {
                0 => (x, y),
                1 => (y, h - 1 - x),
                2 => (w - 1 - x, h - 1 - y),
                _ => (w - 1 - y, x),
            };
            out.pixel_mut(x, y)
                .copy_from_slice(&src.data[(sy as usize * w as usize + sx as usize) * c..][..c]);
        }
    }
    out
}

fn turn_matrix(w: u32, h: u32, turns: u32) -> [f64; 9] {
    let (w, h) = (f64::from(w) - 1.0, f64::from(h) - 1.0);
    match turns % 4 {
        0 => [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        1 => [0.0, 1.0, 0.0, -1.0, 0.0, h, 0.0, 0.0, 1.0],
        2 => [-1.0, 0.0, w, 0.0, -1.0, h, 0.0, 0.0, 1.0],
        _ => [0.0, -1.0, w, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0],
    }
}

fn exact_turns<T: Sample + TryFrom<u32> + PartialEq + std::fmt::Debug>(ch: u8, modulus: u32) {
    let src = noise::<T>(37, 23, ch, modulus);
    for turns in 0..4 {
        let want = rotate(&src, turns);
        let got = warp_perspective_image(
            src.as_ref(),
            &turn_matrix(src.width, src.height, turns),
            want.width,
            want.height,
            &NeverCancel,
        )
        .unwrap();
        assert!(
            got == want,
            "{ch} channel(s), {turns} turn(s) are not bit-exact"
        );
    }
}

#[test]
fn identity_and_quarter_turns_are_bit_exact_for_every_format() {
    exact_turns::<u8>(1, 256);
    exact_turns::<u8>(3, 256);
    exact_turns::<u8>(4, 256);
    exact_turns::<u16>(1, 65536);
    exact_turns::<u16>(3, 65536);
}

#[test]
fn integer_shifts_are_bit_exact_including_the_clamped_border_band() {
    let src = noise::<u16>(50, 40, 3, 65536);
    // Output (u, v) samples source (u + 7, v - 4): the top rows fall in the half-pixel slack only
    // when shifted by a fraction, so integer shifts are pure copies or outside (zero).
    let m = [1.0, 0.0, 7.0, 0.0, 1.0, -4.0, 0.0, 0.0, 1.0];
    let out = warp_perspective_image(src.as_ref(), &m, 50, 40, &NeverCancel).unwrap();
    for v in 0..40i64 {
        for u in 0..50i64 {
            let (sx, sy) = (u + 7, v - 4);
            let inside = (0..50).contains(&sx) && (0..40).contains(&sy);
            for c in 0..3 {
                let got = out.pixel(u as u32, v as u32)[c];
                let want = if inside {
                    src.pixel(sx as u32, sy as u32)[c]
                } else {
                    0
                };
                assert_eq!(got, want, "({u},{v}) channel {c}");
            }
        }
    }
}
