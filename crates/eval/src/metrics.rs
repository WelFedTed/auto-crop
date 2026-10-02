// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Per-image geometry metrics (ROADMAP M1.46, PLAN 7.4).
//!
//! * **IoU after canonical warp.** The ground-truth quad is mapped to the unit square by the unique
//!   homography through its four corners; the predicted quad goes through the same map; the score is
//!   the Jaccard index of the mapped prediction and the unit square. The map is unique, so the score
//!   is the same whether corners are in pixels or in 0..1 coordinates. It does not depend on the
//!   order in which the predictor lists its corners (the polygon is the same).
//! * **Corner error** is the mean of the four corner distances in pixels, as a percentage of the
//!   image diagonal, in the corner order the predictor gave. A page found with its corners rotated
//!   therefore scores a large corner error but a perfect IoU; the orientation class tells the two
//!   cases apart.
//! * **Skew** is the absolute difference between the rotation of the predicted and the true page,
//!   where a page's rotation is the mean direction of its four edges (each mapped to "rightward").
//!   It is measured after undoing a corner relabelling, so it isolates rotation from orientation
//!   confusion.

use crate::geom::{self, H, P, Quad};
use serde::{Deserialize, Serialize};

/// A result below this IoU is a failure (the silent-failure line, PLAN 7.4).
pub const FAILURE_IOU: f64 = 0.90;
pub const SUCCESS_IOU_95: f64 = 0.95;
pub const SUCCESS_IOU_98: f64 = 0.98;

/// How the predicted corner order relates to the ground truth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Orientation {
    /// Same cyclic corner order and start.
    Upright,
    /// Starts at the true top-right (rotated 90 degrees clockwise).
    Rot90,
    Rot180,
    Rot270,
    /// Counter-clockwise listing (a mirrored page): the polygon is right, the order is not.
    Mirrored,
}

/// Why a quad could not be scored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Invalid {
    NonFinite,
    /// Opposite edges cross, or the area is zero.
    NotAPage,
    /// A corner lies on or beyond the horizon of the true page plane.
    BeyondHorizon,
}

/// Metrics for one scorable prediction.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GeometryScore {
    pub iou: f64,
    /// Mean corner distance as % of the image diagonal, in the given corner order.
    pub corner_err_pct: f64,
    pub skew_deg: f64,
    pub orientation: Orientation,
}

/// The canonical-warp IoU of `pred` against `truth` (both 0..1 or both pixels), or why not.
pub fn canonical_iou(truth: &Quad, pred: &Quad) -> Result<f64, Invalid> {
    if !geom::quad_is_finite(pred) {
        return Err(Invalid::NonFinite);
    }
    let unit: Quad = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    // A degenerate ground truth is a data error, not a prediction error; the manifest validator
    // rejects it, so reaching here with one scores zero rather than panicking.
    let Some(h) = geom::homography(truth, &unit) else {
        return Ok(0.0);
    };
    let mapped = map_quad(&h, pred).ok_or(Invalid::BeyondHorizon)?;
    if !geom::quad_is_simple(&mapped) || geom::area(&mapped) < 1e-12 {
        return Err(Invalid::NotAPage);
    }
    let inter = geom::area(&geom::clip_convex(&mapped, &unit));
    let union = geom::area(&mapped) + 1.0 - inter;
    Ok((inter / union).clamp(0.0, 1.0))
}

fn map_quad(h: &H, q: &Quad) -> Option<Quad> {
    let mut out = [[0.0; 2]; 4];
    for i in 0..4 {
        out[i] = geom::apply(h, q[i])?;
    }
    Some(out)
}

fn dist(a: P, b: P) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

fn to_pixels(q: &Quad, w: f64, h: f64) -> Quad {
    std::array::from_fn(|i| [q[i][0] * w, q[i][1] * h])
}

/// Mean corner distance in pixels.
fn mean_corner_dist(a: &Quad, b: &Quad) -> f64 {
    (0..4).map(|i| dist(a[i], b[i])).sum::<f64>() / 4.0
}

/// Rotation of a page outline in degrees (y down, clockwise positive): the mean direction of its
/// four edges after turning each to point "rightward".
pub fn page_angle_deg(q: &Quad) -> f64 {
    let unit = |a: P, b: P| {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let n = dx.hypot(dy);
        if n == 0.0 {
            [0.0, 0.0]
        } else {
            [dx / n, dy / n]
        }
    };
    let top = unit(q[0], q[1]);
    let bottom = unit(q[3], q[2]);
    // A downward edge (dx, dy) turned a quarter turn anticlockwise points rightward: (dy, -dx).
    let l = unit(q[0], q[3]);
    let r = unit(q[1], q[2]);
    let (left, right) = ([l[1], -l[0]], [r[1], -r[0]]);
    let sx = top[0] + bottom[0] + left[0] + right[0];
    let sy = top[1] + bottom[1] + left[1] + right[1];
    sy.atan2(sx).to_degrees()
}

/// Smallest absolute angle between two directions, in 0..=180.
pub fn angle_diff_deg(a: f64, b: f64) -> f64 {
    let d = (a - b).rem_euclid(360.0);
    if d > 180.0 { 360.0 - d } else { d }
}

/// The corner order of `pred` that best matches `truth` (pixel coordinates), and the relabelled
/// non-mirrored quad used for the skew measurement.
fn classify_orientation(truth_px: &Quad, pred_px: &Quad) -> (Orientation, Quad) {
    let mut best: Option<(f64, Orientation, Quad)> = None;
    let classes = [
        Orientation::Upright,
        Orientation::Rot90,
        Orientation::Rot180,
        Orientation::Rot270,
    ];
    for k in 0..4 {
        // The prediction lists the true corner `s` places along when `relabelled[i] = pred[i + k]`
        // matches the truth with `s = (4 - k) % 4`.
        let class = classes[(4 - k) % 4];
        let relabelled: Quad = std::array::from_fn(|i| pred_px[(i + k) % 4]);
        let e = mean_corner_dist(truth_px, &relabelled);
        if best.as_ref().is_none_or(|b| e < b.0 - 1e-9) {
            best = Some((e, class, relabelled));
        }
    }
    let (rot_err, rot_class, rot_quad) = best.expect("four candidates");
    // Counter-clockwise listings: reversed order, any start.
    let mut mirrored_err = f64::INFINITY;
    for k in 0..4 {
        let relabelled: Quad = std::array::from_fn(|i| pred_px[(k + 4 - i) % 4]);
        mirrored_err = mirrored_err.min(mean_corner_dist(truth_px, &relabelled));
    }
    if mirrored_err < rot_err - 1e-9 {
        (Orientation::Mirrored, rot_quad)
    } else {
        (rot_class, rot_quad)
    }
}

/// Scores one prediction. `truth` and `pred` are in 0..1 coordinates of an image of
/// `width` x `height` pixels.
pub fn score(truth: &Quad, pred: &Quad, width: u32, height: u32) -> Result<GeometryScore, Invalid> {
    let iou = canonical_iou(truth, pred)?;
    let (w, h) = (f64::from(width), f64::from(height));
    let (t_px, p_px) = (to_pixels(truth, w, h), to_pixels(pred, w, h));
    let diag = w.hypot(h);
    let corner_err_pct = 100.0 * mean_corner_dist(&t_px, &p_px) / diag;
    let (orientation, aligned) = classify_orientation(&t_px, &p_px);
    let skew_deg = angle_diff_deg(page_angle_deg(&t_px), page_angle_deg(&aligned));
    Ok(GeometryScore {
        iou,
        corner_err_pct,
        skew_deg,
        orientation,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SQ: Quad = [[0.2, 0.2], [0.6, 0.2], [0.6, 0.6], [0.2, 0.6]];

    fn shift(q: &Quad, dx: f64, dy: f64) -> Quad {
        std::array::from_fn(|i| [q[i][0] + dx, q[i][1] + dy])
    }

    #[test]
    fn identical_quads_score_one_with_zero_errors() {
        let s = score(&SQ, &SQ, 1000, 500).expect("valid");
        assert!((s.iou - 1.0).abs() < 1e-12);
        assert!(s.corner_err_pct.abs() < 1e-12 && s.skew_deg.abs() < 1e-9);
        assert_eq!(s.orientation, Orientation::Upright);
    }

    #[test]
    fn a_shifted_rectangle_has_the_analytic_iou() {
        // Shift by (a, b) fractions of the edge lengths: IoU = i / (2 - i), i = (1-a)(1-b).
        for (a, b) in [(0.1, 0.0), (0.05, 0.2), (0.3, 0.3), (0.0, 0.0)] {
            let p = shift(&SQ, a * 0.4, b * 0.4);
            let i = (1.0 - a) * (1.0 - b);
            let want = i / (2.0 - i);
            let got = canonical_iou(&SQ, &p).expect("valid");
            assert!((got - want).abs() < 1e-12, "{a} {b}: {got} vs {want}");
        }
    }

    #[test]
    fn a_scaled_rectangle_has_iou_equal_to_the_area_ratio() {
        let c = [0.4, 0.4];
        for s in [0.5, 0.9, 1.1, 2.0] {
            let p: Quad = std::array::from_fn(|i| {
                [c[0] + (SQ[i][0] - c[0]) * s, c[1] + (SQ[i][1] - c[1]) * s]
            });
            let want = if s <= 1.0 { s * s } else { 1.0 / (s * s) };
            assert!((canonical_iou(&SQ, &p).expect("valid") - want).abs() < 1e-12);
        }
    }

    #[test]
    fn iou_is_independent_of_the_coordinate_scaling_and_the_corner_start() {
        let truth: Quad = [[0.3, 0.1], [0.8, 0.2], [0.75, 0.9], [0.25, 0.8]];
        let pred = shift(&truth, 0.02, -0.01);
        let a = canonical_iou(&truth, &pred).expect("valid");
        // Pixel coordinates of a 4000 x 3000 image give the same score (projective uniqueness).
        let b = canonical_iou(
            &to_pixels(&truth, 4000.0, 3000.0),
            &to_pixels(&pred, 4000.0, 3000.0),
        )
        .expect("valid");
        assert!((a - b).abs() < 1e-9, "{a} {b}");
        // Starting the list at another corner leaves the polygon, so the IoU, unchanged.
        let rot: Quad = std::array::from_fn(|i| pred[(i + 1) % 4]);
        assert!((canonical_iou(&truth, &rot).expect("valid") - a).abs() < 1e-12);
    }

    #[test]
    fn disjoint_and_bad_predictions() {
        assert_eq!(canonical_iou(&SQ, &shift(&SQ, 5.0, 5.0)), Ok(0.0));
        let nan: Quad = [[f64::NAN, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        assert_eq!(canonical_iou(&SQ, &nan), Err(Invalid::NonFinite));
        let bow: Quad = [[0.2, 0.2], [0.6, 0.6], [0.6, 0.2], [0.2, 0.6]];
        assert_eq!(canonical_iou(&SQ, &bow), Err(Invalid::NotAPage));
        let flat: Quad = [[0.2, 0.2], [0.4, 0.2], [0.6, 0.2], [0.8, 0.2]];
        assert_eq!(canonical_iou(&SQ, &flat), Err(Invalid::NotAPage));
    }

    #[test]
    fn corner_error_is_a_percentage_of_the_diagonal() {
        // 3-4-5: every corner moves 30 px right and 40 px down on a 300 x 400 image (diag 500).
        let p = shift(&SQ, 0.1, 0.1);
        let s = score(&SQ, &p, 300, 400).expect("valid");
        assert!((s.corner_err_pct - 100.0 * 50.0 / 500.0).abs() < 1e-9);
    }

    #[test]
    fn skew_measures_rotation_and_ignores_translation() {
        let (w, h) = (1000u32, 1000u32);
        let rot = |deg: f64| -> Quad {
            let c = [0.4, 0.4];
            let (s, co) = deg.to_radians().sin_cos();
            std::array::from_fn(|i| {
                let (x, y) = (SQ[i][0] - c[0], SQ[i][1] - c[1]);
                [c[0] + x * co - y * s, c[1] + x * s + y * co]
            })
        };
        for deg in [0.1, 0.5, 2.0, 15.0, 44.0] {
            let s = score(&SQ, &rot(deg), w, h).expect("valid");
            assert!((s.skew_deg - deg).abs() < 1e-9, "{deg}: {}", s.skew_deg);
        }
        let s = score(&SQ, &shift(&SQ, 0.05, 0.02), w, h).expect("valid");
        assert!(s.skew_deg < 1e-9);
        // Rotation is measured in pixels: the same normalised quad has a different angle on a
        // non-square image, but truth and prediction are measured the same way, so it still cancels.
        let t: Quad = [[0.2, 0.2], [0.7, 0.3], [0.7, 0.8], [0.2, 0.7]];
        assert!(score(&t, &t, 4000, 1000).expect("valid").skew_deg < 1e-9);
    }

    #[test]
    fn orientation_classes() {
        let rot = |k: usize| -> Quad { std::array::from_fn(|i| SQ[(i + k) % 4]) };
        let want = [
            Orientation::Upright,
            Orientation::Rot90,
            Orientation::Rot180,
            Orientation::Rot270,
        ];
        let truth: Quad = [[0.1, 0.1], [0.7, 0.15], [0.65, 0.9], [0.12, 0.8]];
        for (k, w) in want.iter().enumerate() {
            let p: Quad = std::array::from_fn(|i| truth[(i + k) % 4]);
            let s = score(&truth, &p, 800, 600).expect("valid");
            assert_eq!(s.orientation, *w);
            assert!((s.iou - 1.0).abs() < 1e-12, "same polygon, same IoU");
            assert!(s.skew_deg < 1e-9, "skew is measured after relabelling");
            if k > 0 {
                assert!(s.corner_err_pct > 1.0, "corner error exposes the rotation");
            }
        }
        let mirrored: Quad = [truth[0], truth[3], truth[2], truth[1]];
        let s = score(&truth, &mirrored, 800, 600).expect("valid");
        assert_eq!(s.orientation, Orientation::Mirrored);
        assert!((s.iou - 1.0).abs() < 1e-12);
        let _ = rot(0);
    }
}
