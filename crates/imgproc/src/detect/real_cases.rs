// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Synthetic reproductions of failures that real photographs showed (the classical detector was
//! tuned on synthetic data first, then looked at on a private set of real pictures). Nothing here
//! is, or is derived from, a real image: the scenes are generated from fixed seeds by
//! `crate::synth` (`compose_on_desk`, `wood_desk`, `matted_print`), the same functions the
//! `real-cases` suite of the evaluation harness uses.
//!
//! * a white receipt photograph that fills the whole frame, with a barcode and rows of text: the
//!   barcode box used to be returned and accepted (a `Good` 0.97 result, IoU 0.03);
//! * a receipt on a strongly grained wood desk: a quad whose top side ran along the grain at the
//!   top of the frame, a little above the paper, used to be preferred and accepted;
//! * a black-and-white print with a wide white mat on a black background: the paper edge (the outer
//!   mat) and the picture inside it are both plausible outlines.

use super::*;
use crate::synth::{PaperKind, compose_on_desk, matted_print, wood_desk};

/// Largest corner distance as a share of the image diagonal.
fn worst_corner(found: &[Pt; 4], truth: &[(f64, f64); 4], w: f64, h: f64) -> f64 {
    (0..4)
        .map(|i| ((found[i].x - truth[i].0) * w).hypot((found[i].y - truth[i].1) * h))
        .fold(0.0, f64::max)
        / w.hypot(h)
}

fn reasons(d: &Detection) -> Vec<ReasonCode> {
    d.confidence.reasons.iter().map(|r| r.code).collect()
}

fn accepted(d: &Detection) -> bool {
    d.confidence.forced.is_none() && d.confidence.score >= 0.9
}

const FULL: [(f64, f64); 4] = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];

#[test]
fn a_white_receipt_filling_the_frame_is_the_whole_frame_held_with_two_outlines() {
    let flat = |c: [f32; 3]| move |_: u32, _: u32| c;
    for (seed, size, paper) in [
        (5u64, (360u32, 1100u32), [250u8, 250, 250]),
        (6, (300, 900), [244, 240, 228]),
        (7, (420, 1000), [255, 255, 255]),
    ] {
        let img = compose_on_desk(
            size,
            FULL,
            PaperKind::Receipt,
            paper,
            seed,
            2.0,
            &flat([0.0; 3]),
        );
        let d = detect(&img);
        let q = d.quad.expect("a quad");
        assert!(
            worst_corner(&q, &FULL, f64::from(size.0), f64::from(size.1)) < 0.02,
            "seed {seed}: {q:?}"
        );
        assert_eq!(d.confidence.forced, Some(Forced::Check), "seed {seed}");
        assert!(
            reasons(&d).contains(&ReasonCode::DetectorsDisagree),
            "seed {seed}: {:?}",
            d.confidence.reasons
        );
    }
}

#[test]
fn a_page_with_a_desk_in_view_is_never_taken_for_the_whole_frame() {
    // The same receipt with a plain light desk around it: the page is the receipt.
    let desk = |_: u32, _: u32| [238.0, 236.0, 230.0];
    let truth = [(0.30, 0.04), (0.70, 0.04), (0.70, 0.96), (0.30, 0.96)];
    let img = compose_on_desk(
        (420, 1000),
        truth,
        PaperKind::Receipt,
        [250, 250, 250],
        5,
        2.0,
        &desk,
    );
    let d = detect(&img);
    if let Some(q) = d.quad {
        // Whatever it answers, the whole frame is not it, and an accepted answer must be right.
        let whole = worst_corner(&q, &FULL, 420.0, 1000.0) < 0.02;
        assert!(!whole, "{q:?}");
        if accepted(&d) {
            assert!(worst_corner(&q, &truth, 420.0, 1000.0) < 0.02, "{q:?}");
        }
    }
}

#[test]
fn grain_in_a_wood_desk_does_not_pull_a_page_edge_out_of_place() {
    // Receipt 70% of the frame wide, its top edge 9% down; grain lines run parallel to the top
    // and bottom edges and are as long and as straight as they are.
    let (w, h) = (480u32, 720u32);
    let truth = [(0.15, 0.09), (0.86, 0.10), (0.85, 0.90), (0.14, 0.89)];
    let mut worst = 0.0f64;
    for grain in [20.0f32, 34.0, 50.0] {
        for seed in 1..=6u64 {
            let desk = wood_desk(h, seed, grain, [128.0, 80.0, 42.0]);
            let img = compose_on_desk(
                (w, h),
                truth,
                PaperKind::Receipt,
                [214, 212, 206],
                9,
                3.0,
                &desk,
            );
            let d = detect(&img);
            let q = d.quad.expect("a quad");
            let err = worst_corner(&q, &truth, f64::from(w), f64::from(h));
            worst = worst.max(err);
            assert!(
                err < 0.015,
                "grain {grain} seed {seed}: error {err:.4} {q:?}"
            );
        }
    }
    assert!(worst < 0.015);
}

#[test]
fn a_wood_edge_that_cannot_be_seen_is_held_never_accepted_in_the_wrong_place() {
    // The desk near the receipt's top edge is as bright as the paper (a glare band), so that edge
    // has nothing to see: the answer may be wrong, but then it must not be accepted.
    let (w, h) = (480u32, 720u32);
    let truth = [(0.15, 0.09), (0.86, 0.10), (0.85, 0.90), (0.14, 0.89)];
    for seed in 1..=6u64 {
        let wood = wood_desk(h, seed, 30.0, [128.0, 80.0, 42.0]);
        let desk = move |x: u32, y: u32| {
            let base = wood(x, y);
            // A glare band over the first 11% of the frame lifts the desk towards the paper.
            let k = (1.0 - f64::from(y) / (0.11 * f64::from(h))).clamp(0.0, 1.0) as f32;
            let paper = [214.0f32, 205.0, 190.0];
            std::array::from_fn(|i| base[i] * (1.0 - 0.8 * k) + paper[i] * 0.8 * k)
        };
        let img = compose_on_desk(
            (w, h),
            truth,
            PaperKind::Receipt,
            [214, 212, 206],
            9,
            3.0,
            &desk,
        );
        let d = detect(&img);
        if let Some(q) = d.quad
            && worst_corner(&q, &truth, f64::from(w), f64::from(h)) >= 0.015
        {
            assert!(!accepted(&d), "silent failure {q:?} seed {seed}");
        }
    }
}

#[test]
fn a_print_on_a_wide_white_mat_is_held_with_two_outlines_and_the_paper_edge_is_returned() {
    // Policy (docs/perf/detector-baseline.md): the detector answers with the paper edge and says
    // that a picture inside it is a second plausible outline.
    for (mat, pic, seed) in [(0.035, 0.107, 77u64), (0.05, 0.14, 3), (0.03, 0.09, 12)] {
        let (img, mat_corners, _) = matted_print((600, 600), mat, pic, seed);
        let d = detect(&img);
        let q = d.quad.expect("a quad");
        assert!(
            worst_corner(&q, &mat_corners, 600.0, 600.0) < 0.02,
            "mat {mat}: {q:?}"
        );
        assert_eq!(d.confidence.forced, Some(Forced::Check), "mat {mat}");
        assert!(
            reasons(&d).contains(&ReasonCode::DetectorsDisagree),
            "mat {mat}: {:?}",
            d.confidence.reasons
        );
    }
}

#[test]
fn a_picture_on_a_plain_light_desk_with_nothing_else_in_view_is_still_found() {
    // A grey block on an empty white desk: a surround of one colour, a dark block in it, but no
    // content around it, so it is not a patch of a bigger page.
    let (w, h) = (500u32, 400u32);
    let mut img = Raster::filled(w, h, [244, 244, 242]);
    let mut rng = crate::synth::Rng::new(4);
    for y in 0..h {
        for x in 0..w {
            let n = (rng.unit() - 0.5) * 3.0;
            let inside = (150..350).contains(&x) && (100..300).contains(&y);
            let v = if inside {
                let d = ((f64::from(x) - 250.0) / 80.0).hypot((f64::from(y) - 190.0) / 70.0);
                (150.0 - 80.0 * (-d * d).exp()) as f32 + n
            } else {
                244.0 + n
            };
            let c = v.round().clamp(0.0, 255.0) as u8;
            img.set_pixel(x, y, [c, c, c]);
        }
    }
    let truth = [(0.3, 0.25), (0.7, 0.25), (0.7, 0.75), (0.3, 0.75)];
    let d = detect(&img);
    let q = d.quad.expect("a quad");
    assert!(worst_corner(&q, &truth, 500.0, 400.0) < 0.02, "{q:?}");
    assert!(
        !reasons(&d).contains(&ReasonCode::DetectorsDisagree),
        "{:?}",
        d.confidence.reasons
    );
}

#[test]
fn a_document_page_filling_the_frame_is_the_whole_frame_held_too() {
    let flat = |c: [f32; 3]| move |_: u32, _: u32| c;
    for (seed, size) in [(5u64, (480u32, 640u32)), (6, (640, 480)), (7, (500, 700))] {
        let img = compose_on_desk(
            size,
            FULL,
            PaperKind::Document,
            [250, 250, 250],
            seed,
            2.0,
            &flat([0.0; 3]),
        );
        let d = detect(&img);
        // Held, and if it answers with the whole frame that is right; anything else must not be
        // accepted.
        assert_eq!(d.confidence.forced, Some(Forced::Check), "seed {seed}");
        let q = d.quad.expect("a quad");
        assert!(
            worst_corner(&q, &FULL, f64::from(size.0), f64::from(size.1)) < 0.02,
            "seed {seed}: {q:?}"
        );
    }
}

#[test]
fn chromaticity_ignores_brightness_and_tells_wood_from_paper() {
    let paper = chroma([214.0, 212.0, 206.0]).expect("bright enough");
    let shaded = chroma([107.0, 106.0, 103.0]).expect("bright enough");
    let wood = chroma([128.0, 80.0, 42.0]).expect("bright enough");
    assert!(chroma_gap(paper, shaded) < CHROMA_SAME);
    assert!(chroma_gap(paper, wood) > CHROMA_FAR);
    assert!(chroma([10.0, 12.0, 9.0]).is_none());
}
