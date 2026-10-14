// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Synthetic pages: a photograph on a white mat, with a thin border, with a spill of desk around
//! it, in perspective and rotated; and the pages that must NOT be trimmed (a letter with margins, a
//! receipt, a full-bleed photograph, a blank page, a picture with a caption).

use super::*;
use crate::geometry::{apply, homography, invert};
use crate::synth::{PaperKind, Rng, matted_print, paper_texture};

type Corners = [(f64, f64); 4];

fn pts(c: Corners) -> [Pt; 4] {
    c.map(|(x, y)| Pt::new(x, y))
}

/// Largest corner distance as a share of the image diagonal.
fn worst(found: &[Pt; 4], truth: &Corners, w: f64, h: f64) -> f64 {
    (0..4)
        .map(|i| ((found[i].x - truth[i].0) * w).hypot((found[i].y - truth[i].1) * h))
        .fold(0.0, f64::max)
        / w.hypot(h)
}

fn grow(c: Corners, by: f64) -> Corners {
    let (cx, cy) = (
        c.iter().map(|p| p.0).sum::<f64>() / 4.0,
        c.iter().map(|p| p.1).sum::<f64>() / 4.0,
    );
    c.map(|(x, y)| (cx + (x - cx) * (1.0 + by), cy + (y - cy) * (1.0 + by)))
}

/// `print` (a matted print on a black desk, as `matted_print` makes it) turned into a page on a
/// mid-grey desk: the mat is mapped onto `dst` (normalised corners TL, TR, BR, BL) of a `w` x `h`
/// canvas. Returns the canvas and the picture's corners in it.
fn on_desk(
    print: &Raster,
    mat: Corners,
    pic: Corners,
    dst: Corners,
    (w, h): (u32, u32),
    seed: u64,
) -> (Raster, Corners) {
    let (pw, ph) = (f64::from(print.width), f64::from(print.height));
    let mat_px: Corners = mat.map(|(x, y)| (x * pw, y * ph));
    let dst_px: Corners = dst.map(|(x, y)| (x * f64::from(w), y * f64::from(h)));
    let to_print = homography(dst_px, mat_px).expect("a plain quad");
    let to_canvas = invert(&to_print).expect("invertible");
    let mut rng = Rng::new(seed);
    let mut out = Raster::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let (u, v) =
                apply(&to_print, f64::from(x) + 0.5, f64::from(y) + 0.5).unwrap_or((-1.0, -1.0));
            let inside =
                u >= mat_px[0].0 && u <= mat_px[2].0 && v >= mat_px[0].1 && v <= mat_px[2].1;
            let mut c = if inside {
                bilinear(print, u - 0.5, v - 0.5)
            } else {
                [96.0, 90.0, 84.0]
            };
            for ch in &mut c {
                *ch += (rng.unit() - 0.5) * 3.0;
            }
            out.set_pixel(x, y, c.map(|v| v.round().clamp(0.0, 255.0) as u8));
        }
    }
    let pic_out = pic.map(|(x, y)| {
        let (u, v) = apply(&to_canvas, x * pw, y * ph).expect("finite");
        (u / f64::from(w), v / f64::from(h))
    });
    (out, pic_out)
}

fn square(m: f64) -> Corners {
    [(m, m), (1.0 - m, m), (1.0 - m, 1.0 - m), (m, 1.0 - m)]
}

#[test]
fn a_print_on_a_wide_white_mat_is_trimmed_to_the_picture() {
    for (mat, pic, seed) in [
        (0.035, 0.107, 77u64),
        (0.05, 0.14, 3),
        (0.03, 0.09, 12),
        (0.02, 0.2, 8),
    ] {
        let (img, mat_c, pic_c) = matted_print((600, 600), mat, pic, seed);
        let r = trim_analysis(&img, pts(mat_c))
            .unwrap_or_else(|e| panic!("mat {mat} pic {pic}: {e:?}"));
        let err = worst(&r.quad, &pic_c, 600.0, 600.0);
        assert!(
            err < 0.006,
            "mat {mat} pic {pic}: corner error {err:.4}: {:?}",
            r.quad
        );
        assert!(r.confidence >= 0.5 && r.confidence <= 0.95);
        assert_eq!(r.reason, TrimReason::Mat, "mat {mat} pic {pic}");
    }
}

#[test]
fn a_thin_photographic_border_is_trimmed_too() {
    // The white edge of a print: about 3% of the page on every side.
    for (mat, pic, seed) in [(0.04, 0.062, 5u64), (0.04, 0.065, 6)] {
        let (img, mat_c, pic_c) = matted_print((600, 600), mat, pic, seed);
        let r = trim_analysis(&img, pts(mat_c)).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(worst(&r.quad, &pic_c, 600.0, 600.0) < 0.006, "{:?}", r.quad);
        assert_eq!(r.reason, TrimReason::ThinBorder);
    }
}

#[test]
fn a_crop_with_a_margin_of_desk_around_the_paper_is_trimmed_all_the_same() {
    let (img, mat_c, pic_c) = matted_print((600, 600), 0.05, 0.15, 21);
    // 2% and 4% bigger than the paper: the quad holds a strip of black desk all round.
    for by in [0.02, 0.04] {
        let r = trim_analysis(&img, pts(grow(mat_c, by)))
            .unwrap_or_else(|e| panic!("grow {by}: {e:?}"));
        assert!(
            worst(&r.quad, &pic_c, 600.0, 600.0) < 0.008,
            "grow {by}: {:?}",
            r.quad
        );
    }
}

#[test]
fn a_page_seen_in_perspective_and_slightly_turned_is_trimmed() {
    let (print, mat_c, pic_c) = matted_print((900, 900), 0.04, 0.13, 31);
    let dsts: [Corners; 2] = [
        // Perspective: the top is narrower than the bottom.
        [(0.27, 0.12), (0.71, 0.14), (0.80, 0.88), (0.18, 0.86)],
        // Turned by about 4 degrees.
        [(0.22, 0.16), (0.78, 0.09), (0.84, 0.82), (0.28, 0.89)],
    ];
    for (i, dst) in dsts.iter().enumerate() {
        let (img, truth) = on_desk(&print, mat_c, pic_c, *dst, (700, 640), 7 + i as u64);
        let r = trim_analysis(&img, pts(*dst)).unwrap_or_else(|e| panic!("case {i}: {e:?}"));
        let err = worst(&r.quad, &truth, 700.0, 640.0);
        assert!(err < 0.01, "case {i}: corner error {err:.4}: {:?}", r.quad);
    }
}

#[test]
fn a_letter_with_margins_is_never_trimmed() {
    for (kind, seed) in [
        (PaperKind::Document, 1u64),
        (PaperKind::Document, 2),
        (PaperKind::Document, 9),
    ] {
        let page = paper_texture(kind, [246, 244, 238], [58, 62, 74], seed);
        let r = trim_analysis(&page, pts(square(0.0)));
        assert!(r.is_err(), "document seed {seed}: {r:?}");
    }
}

#[test]
fn a_receipt_is_never_trimmed() {
    for seed in [1u64, 2, 3, 4] {
        let page = paper_texture(PaperKind::Receipt, [250, 250, 250], [58, 62, 74], seed);
        let r = trim_analysis(&page, pts(square(0.0)));
        assert!(r.is_err(), "receipt seed {seed}: {r:?}");
    }
}

#[test]
fn a_full_bleed_photograph_has_no_border_to_trim() {
    for seed in [5u64, 6] {
        let (img, ..) = matted_print((600, 600), 0.0, 0.0, seed);
        assert!(
            trim_analysis(&img, pts(square(0.0))).is_err(),
            "seed {seed}"
        );
    }
}

#[test]
fn a_blank_page_and_a_page_of_noise_are_refused() {
    let blank = Raster::filled(500, 400, [244, 244, 240]);
    assert!(trim_analysis(&blank, pts(square(0.0))).is_err());
    let mut rng = Rng::new(3);
    let mut noise = Raster::new(500, 400);
    for p in noise.data.as_chunks_mut::<3>().0 {
        let v = (rng.unit() * 255.0) as u8;
        *p = [v, v ^ 0x35, v.wrapping_add(50)];
    }
    assert!(trim_analysis(&noise, pts(square(0.0))).is_err());
}

#[test]
fn a_caption_in_the_border_never_gets_cut_off() {
    let (mut img, mat_c, pic_c) = matted_print((600, 600), 0.03, 0.09, 12);
    // Caption lines under the picture, within the mat.
    for line in 0..2u32 {
        let y0 = 550 + line * 7;
        for y in y0..y0 + 3 {
            for x in 80..420 {
                if (x / 3) % 4 != 3 {
                    img.set_pixel(x, y, [30, 30, 36]);
                }
            }
        }
    }
    // Refused, or the picture plus its caption: never a cut through the picture or the text.
    if let Ok(r) = trim_analysis(&img, pts(mat_c)) {
        assert!(r.quad[0].x <= pic_c[0].0 + 0.005 && r.quad[0].y <= pic_c[0].1 + 0.005);
        assert!(
            r.quad[2].x >= pic_c[2].0 - 0.005 && r.quad[2].y >= 0.9333 - 0.005,
            "{:?}",
            r.quad
        );
    }
}

#[test]
fn degenerate_input_never_panics() {
    let tiny = Raster::filled(10, 10, [200, 200, 200]);
    assert_eq!(
        trim_analysis(&tiny, pts(square(0.0))),
        Err(TrimRefusal::BadQuad)
    );
    let img = Raster::filled(300, 300, [200, 200, 200]);
    // Collapsed, reversed, non-finite and far outside the picture.
    assert!(trim_to_content(&img, pts([(0.5, 0.5); 4])).is_none());
    assert!(trim_to_content(&img, pts([(0.0, 0.0), (0.0, 1.0), (1.0, 1.0), (1.0, 0.0)])).is_none());
    assert!(
        trim_to_content(
            &img,
            [
                Pt::new(f64::NAN, 0.0),
                Pt::new(1.0, 0.0),
                Pt::new(1.0, 1.0),
                Pt::new(0.0, 1.0)
            ]
        )
        .is_none()
    );
    assert!(trim_to_content(&img, pts([(2.0, 2.0), (3.0, 2.0), (3.0, 3.0), (2.0, 3.0)])).is_none());
    let sliver = pts([(0.0, 0.0), (1.0, 0.0), (1.0, 0.05), (0.0, 0.05)]);
    assert!(trim_to_content(&img, sliver).is_none());
}

#[test]
fn the_proposal_is_deterministic_and_inside_the_page() {
    let (img, mat_c, _) = matted_print((600, 600), 0.035, 0.107, 77);
    let a = trim_analysis(&img, pts(mat_c)).expect("a proposal");
    let b = trim_analysis(&img, pts(mat_c)).expect("a proposal");
    assert_eq!(a, b);
    for p in a.quad {
        assert!(p.x > mat_c[0].0 && p.x < mat_c[2].0 && p.y > mat_c[0].1 && p.y < mat_c[2].1);
    }
}
