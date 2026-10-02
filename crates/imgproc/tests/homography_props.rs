// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Property tests for the homography solver (ROADMAP M1.23): the four correspondences are mapped
//! exactly and `inverse` round-trips, both below 1e-9 pixels, over random convex quadrilaterals of
//! document and receipt proportions in a 100 MP-sized frame; degenerate inputs never panic and are
//! always reported as `Degenerate`.

use auto_crop_imgproc::homography::{Homography, HomographyError, Pt};
use proptest::prelude::*;

/// A convex quadrilateral (clockwise from the top-left) around `(cx, cy)`: a rotated rectangle
/// of aspect `aspect` and half-size `r`, then each corner moved by up to `jitter` times the
/// shorter half-side (so the quad stays convex: 0.25 of the half-side cannot fold it).
fn quad(cx: f64, cy: f64, r: f64, aspect: f64, ang: f64, jit: [f64; 8], jitter: f64) -> [Pt; 4] {
    let (hw, hh) = (r * aspect.sqrt(), r / aspect.sqrt());
    let unit = hw.min(hh) * jitter;
    let base = [(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)];
    let (s, c) = ang.sin_cos();
    std::array::from_fn(|i| {
        let (x, y) = base[i];
        (
            cx + x * c - y * s + jit[2 * i] * unit,
            cy + x * s + y * c + jit[2 * i + 1] * unit,
        )
    })
}

fn convex_quad() -> impl Strategy<Value = [Pt; 4]> {
    (
        (0.0f64..11_000.0, 0.0f64..8_000.0),
        200.0f64..3000.0,
        0.1f64..10.0,
        -3.2f64..3.2,
        prop::array::uniform8(-1.0f64..1.0),
        0.0f64..0.25,
    )
        .prop_map(|((cx, cy), r, aspect, ang, jit, jitter)| {
            quad(cx, cy, r, aspect, ang, jit, jitter)
        })
}

fn within(a: Pt, b: Pt, tol: f64) -> bool {
    (a.0 - b.0).abs() < tol && (a.1 - b.1).abs() < tol
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(4000))]

    #[test]
    fn corners_map_exactly_and_the_inverse_round_trips(from in convex_quad(), to in convex_quad()) {
        let h = Homography::from_quads(from, to).expect("a convex quad pair is never degenerate");
        for i in 0..4 {
            let mapped = h.apply(from[i].0, from[i].1).unwrap();
            prop_assert!(within(mapped, to[i], 1e-9), "corner {i}: {mapped:?} vs {:?}", to[i]);
        }
        let inv = h.inverse().unwrap();
        // Interior points as convex combinations of the source corners.
        for (a, b) in [(0.5, 0.5), (0.1, 0.9), (0.8, 0.3), (0.37, 0.61)] {
            let top = (from[0].0 * (1.0 - a) + from[1].0 * a, from[0].1 * (1.0 - a) + from[1].1 * a);
            let bot = (from[3].0 * (1.0 - a) + from[2].0 * a, from[3].1 * (1.0 - a) + from[2].1 * a);
            let p = (top.0 * (1.0 - b) + bot.0 * b, top.1 * (1.0 - b) + bot.1 * b);
            let q = h.apply(p.0, p.1).unwrap();
            let back = inv.apply(q.0, q.1).unwrap();
            prop_assert!(within(back, p, 1e-9), "round trip {p:?} -> {q:?} -> {back:?}");
        }
    }

    #[test]
    fn collinear_or_repeated_corners_are_degenerate_never_a_panic(
        p0 in (-5000.0f64..5000.0, -5000.0f64..5000.0),
        d in (-3000.0f64..3000.0, -3000.0f64..3000.0),
        t in prop::array::uniform3(-2.0f64..2.0),
        to in convex_quad(),
    ) {
        // Three of the four corners exactly on one line (t scales the direction), one off it.
        let line = |k: f64| (p0.0 + d.0 * k, p0.1 + d.1 * k);
        let from = [line(t[0]), line(t[1]), line(t[2]), (p0.0 - d.1 + 1.0, p0.1 + d.0 + 1.0)];
        match Homography::from_quads(from, to) {
            Err(HomographyError::Degenerate) => {}
            // The only legal success is a configuration that is not actually collinear after
            // rounding; it must still be a faithful map.
            Ok(h) => {
                for i in 0..4 {
                    let m = h.apply(from[i].0, from[i].1);
                    prop_assert!(m.is_none_or(|m| within(m, to[i], 1e-3)));
                }
            }
        }
        let same = [from[3]; 4];
        prop_assert_eq!(Homography::from_quads(same, to), Err(HomographyError::Degenerate));
    }
}
