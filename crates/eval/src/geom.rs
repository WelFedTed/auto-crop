// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Geometry for the metrics: a 4-point homography solver, a polygon clipper and polygon areas.
//!
//! Everything here is written independently of `auto-crop-imgproc` (ROADMAP M1.46): the code that
//! scores the detector must not share a homography or a clipper with the detector's own warp. A
//! test (`independence`) fails if this module family ever imports a project crate.

/// A point `[x, y]`.
pub type P = [f64; 2];
/// Four corners, clockwise from the top-left of the upright item (y down).
pub type Quad = [P; 4];

/// A row-major 3x3 homography with `h[8] == 1`.
pub type H = [f64; 9];

pub fn quad_is_finite(q: &Quad) -> bool {
    q.iter().all(|p| p[0].is_finite() && p[1].is_finite())
}

/// Shoelace area, signed: positive for clockwise order in a y-down frame.
pub fn signed_area(poly: &[P]) -> f64 {
    let n = poly.len();
    let mut s = 0.0;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        s += a[0] * b[1] - b[0] * a[1];
    }
    s / 2.0
}

pub fn area(poly: &[P]) -> f64 {
    signed_area(poly).abs()
}

/// The homography taking `from[i]` to `to[i]`, or `None` if the four correspondences are
/// degenerate (three collinear points, duplicates). Gaussian elimination with partial pivoting
/// on the 8x8 system, `h[8]` fixed to 1.
pub fn homography(from: &Quad, to: &Quad) -> Option<H> {
    let mut a = [[0.0f64; 9]; 8];
    for i in 0..4 {
        let (x, y) = (from[i][0], from[i][1]);
        let (u, v) = (to[i][0], to[i][1]);
        a[2 * i] = [x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y, u];
        a[2 * i + 1] = [0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y, v];
    }
    for col in 0..8 {
        let piv = (col..8).max_by(|&r, &s| {
            a[r][col]
                .abs()
                .partial_cmp(&a[s][col].abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })?;
        if a[piv][col].abs().is_nan() || a[piv][col].abs() <= 1e-12 {
            return None;
        }
        a.swap(col, piv);
        let d = a[col][col];
        for v in &mut a[col][col..] {
            *v /= d;
        }
        for r in 0..8 {
            if r != col {
                let f = a[r][col];
                if f != 0.0 {
                    let pivot_row = a[col];
                    for (v, p) in a[r][col..].iter_mut().zip(&pivot_row[col..]) {
                        *v -= f * p;
                    }
                }
            }
        }
    }
    let mut h = [0.0; 9];
    for (i, row) in a.iter().enumerate() {
        h[i] = row[8];
    }
    h[8] = 1.0;
    h.iter().all(|v| v.is_finite()).then_some(h)
}

/// Maps a point through `h`. `None` when the point lies on or beyond the plane's horizon
/// (non-positive homogeneous weight), where "inside the page" has no meaning.
pub fn apply(h: &H, p: P) -> Option<P> {
    let w = h[6] * p[0] + h[7] * p[1] + h[8];
    if w.is_nan() || w <= 1e-9 {
        return None;
    }
    Some([
        (h[0] * p[0] + h[1] * p[1] + h[2]) / w,
        (h[3] * p[0] + h[4] * p[1] + h[5]) / w,
    ])
}

/// Sutherland-Hodgman clipping of `subject` (any simple polygon, convex or not) against the convex
/// polygon `clip`. For a concave subject the output may contain coincident edges, which add no
/// area, so [`area`] of the result is still exact.
pub fn clip_convex(subject: &[P], clip: &[P]) -> Vec<P> {
    // Orient the clip polygon counter-clockwise (positive signed area) so "left of the edge" is inside.
    let mut clip: Vec<P> = clip.to_vec();
    if signed_area(&clip) < 0.0 {
        clip.reverse();
    }
    let mut out: Vec<P> = subject.to_vec();
    for i in 0..clip.len() {
        if out.is_empty() {
            break;
        }
        let (a, b) = (clip[i], clip[(i + 1) % clip.len()]);
        let side = |p: P| (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
        let input = std::mem::take(&mut out);
        for j in 0..input.len() {
            let (cur, prev) = (input[j], input[(j + input.len() - 1) % input.len()]);
            let (sc, sp) = (side(cur), side(prev));
            if sc >= 0.0 {
                if sp < 0.0 {
                    out.push(intersect(prev, cur, sp, sc));
                }
                out.push(cur);
            } else if sp >= 0.0 {
                out.push(intersect(prev, cur, sp, sc));
            }
        }
    }
    out
}

/// The point where the segment `p -> q` crosses the clip line, given the signed side values.
fn intersect(p: P, q: P, sp: f64, sq: f64) -> P {
    let t = sp / (sp - sq);
    [p[0] + t * (q[0] - p[0]), p[1] + t * (q[1] - p[1])]
}

fn cross(o: P, a: P, b: P) -> f64 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

/// True when the open segments `p1p2` and `p3p4` properly cross.
fn segments_cross(p1: P, p2: P, p3: P, p4: P) -> bool {
    let (d1, d2) = (cross(p3, p4, p1), cross(p3, p4, p2));
    let (d3, d4) = (cross(p1, p2, p3), cross(p1, p2, p4));
    ((d1 > 0.0) != (d2 > 0.0))
        && ((d3 > 0.0) != (d4 > 0.0))
        && d1 != 0.0
        && d2 != 0.0
        && d3 != 0.0
        && d4 != 0.0
}

/// A quad is simple when its opposite edges do not cross (a bow-tie is not a page outline).
pub fn quad_is_simple(q: &Quad) -> bool {
    !segments_cross(q[0], q[1], q[2], q[3]) && !segments_cross(q[1], q[2], q[3], q[0])
}

/// Point in a convex polygon of either winding (boundary counts as inside).
pub fn point_in_convex(poly: &[P], p: P) -> bool {
    let mut pos = false;
    let mut neg = false;
    for i in 0..poly.len() {
        let c = cross(poly[i], poly[(i + 1) % poly.len()], p);
        pos |= c > 0.0;
        neg |= c < 0.0;
    }
    !(pos && neg)
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNIT: Quad = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];

    #[test]
    fn areas_of_known_shapes() {
        assert_eq!(signed_area(&UNIT), 1.0);
        let mut rev = UNIT;
        rev.reverse();
        assert_eq!(signed_area(&rev), -1.0);
        assert_eq!(area(&rev), 1.0);
        let tri = [[0.0, 0.0], [4.0, 0.0], [0.0, 3.0]];
        assert_eq!(area(&tri), 6.0);
    }

    #[test]
    fn homography_maps_the_four_points_and_inverts() {
        let q: Quad = [[0.2, 0.1], [0.8, 0.15], [0.9, 0.9], [0.1, 0.8]];
        let h = homography(&q, &UNIT).expect("solvable");
        for i in 0..4 {
            let m = apply(&h, q[i]).expect("finite");
            assert!((m[0] - UNIT[i][0]).abs() < 1e-12 && (m[1] - UNIT[i][1]).abs() < 1e-12);
        }
        // The centre of the quad's diagonals' intersection maps to the square's centre only for a
        // parallelogram; for a general quad the map is projective, not affine.
        let par: Quad = [[0.1, 0.1], [0.7, 0.2], [0.8, 0.6], [0.2, 0.5]];
        let hp = homography(&par, &UNIT).expect("solvable");
        let mid = [(par[0][0] + par[2][0]) / 2.0, (par[0][1] + par[2][1]) / 2.0];
        let m = apply(&hp, mid).expect("finite");
        assert!((m[0] - 0.5).abs() < 1e-12 && (m[1] - 0.5).abs() < 1e-12);
        // Round trip through the inverse correspondence.
        let hi = homography(&UNIT, &par).expect("solvable");
        let back = apply(&hi, apply(&hp, [0.4, 0.35]).expect("finite")).expect("finite");
        assert!((back[0] - 0.4).abs() < 1e-9 && (back[1] - 0.35).abs() < 1e-9);
    }

    #[test]
    fn degenerate_correspondences_are_rejected() {
        let collinear: Quad = [[0.0, 0.0], [1.0, 1.0], [2.0, 2.0], [3.0, 3.0]];
        assert!(homography(&collinear, &UNIT).is_none());
        let dup: Quad = [[0.0, 0.0], [0.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        assert!(homography(&dup, &UNIT).is_none());
    }

    #[test]
    fn horizon_points_are_not_mapped() {
        let h: H = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0];
        assert!(apply(&h, [-1.0, 0.0]).is_none());
        assert!(apply(&h, [-2.0, 0.0]).is_none());
        assert!(apply(&h, [1.0, 0.0]).is_some());
    }

    #[test]
    fn clipping_matches_hand_computed_overlaps() {
        // Unit square shifted by (0.25, 0.5): overlap 0.75 * 0.5.
        let shifted: Quad = [[0.25, 0.5], [1.25, 0.5], [1.25, 1.5], [0.25, 1.5]];
        let c = clip_convex(&shifted, &UNIT);
        assert!((area(&c) - 0.375).abs() < 1e-15);
        // Disjoint.
        let far: Quad = [[3.0, 3.0], [4.0, 3.0], [4.0, 4.0], [3.0, 4.0]];
        assert!(area(&clip_convex(&far, &UNIT)) == 0.0);
        // Containment either way.
        let big: Quad = [[-1.0, -1.0], [2.0, -1.0], [2.0, 2.0], [-1.0, 2.0]];
        assert!((area(&clip_convex(&big, &UNIT)) - 1.0).abs() < 1e-15);
        let small: Quad = [[0.25, 0.25], [0.75, 0.25], [0.75, 0.75], [0.25, 0.75]];
        assert!((area(&clip_convex(&small, &UNIT)) - 0.25).abs() < 1e-15);
        // A diamond of half-diagonal 1 centred in the square clips to the square minus 4 corners.
        let diamond = [[0.5, -0.5], [1.5, 0.5], [0.5, 1.5], [-0.5, 0.5]];
        // Diamond area 2; each of the four corner triangles outside has legs 0.5 -> 0.125.
        assert!((area(&clip_convex(&diamond, &UNIT)) - 1.0).abs() < 1e-15);
        // Winding of either polygon does not matter.
        let mut rev = UNIT;
        rev.reverse();
        assert!((area(&clip_convex(&shifted, &rev)) - 0.375).abs() < 1e-15);
    }

    /// Reference by brute force: fraction of a fine grid inside both polygons.
    fn grid_overlap(a: &[P], b: &[P], n: usize) -> f64 {
        let (lo, hi) = (-1.0, 2.0);
        let step = (hi - lo) / n as f64;
        let mut hits = 0usize;
        for i in 0..n {
            for j in 0..n {
                let p = [lo + (i as f64 + 0.5) * step, lo + (j as f64 + 0.5) * step];
                if inside_any(a, p) && inside_any(b, p) {
                    hits += 1;
                }
            }
        }
        hits as f64 * step * step
    }

    /// Even-odd point in polygon (works for concave polygons).
    fn inside_any(poly: &[P], p: P) -> bool {
        let mut inside = false;
        for i in 0..poly.len() {
            let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
            if (a[1] > p[1]) != (b[1] > p[1])
                && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
            {
                inside = !inside;
            }
        }
        inside
    }

    #[test]
    fn clipping_agrees_with_a_brute_force_raster_on_random_and_concave_polygons() {
        let mut s = 0x1234_5678_9abc_def0u64;
        let mut rnd = || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 11) as f64 / (1u64 << 53) as f64
        };
        for case in 0..30 {
            // Random convex-ish quads around the unit square, and every few cases an arrow-head
            // (concave) polygon.
            let jitter = |base: P, r: &mut dyn FnMut() -> f64| {
                [base[0] + (r() - 0.5) * 1.2, base[1] + (r() - 0.5) * 1.2]
            };
            let subj: Vec<P> = if case % 5 == 4 {
                vec![
                    jitter([0.0, 0.0], &mut rnd),
                    jitter([1.0, 0.0], &mut rnd),
                    [0.5 + (rnd() - 0.5) * 0.2, 0.4],
                    jitter([1.0, 1.0], &mut rnd),
                    jitter([0.0, 1.0], &mut rnd),
                ]
            } else {
                UNIT.iter().map(|p| jitter(*p, &mut rnd)).collect()
            };
            // Only compare simple polygons (even-odd and signed area agree only on those).
            let n = subj.len();
            let simple = (0..n).all(|i| {
                (i + 2..n).all(|j| {
                    (i == 0 && j == n - 1)
                        || !segments_cross(subj[i], subj[(i + 1) % n], subj[j], subj[(j + 1) % n])
                })
            });
            if !simple {
                continue;
            }
            let exact = area(&clip_convex(&subj, &UNIT));
            let approx = grid_overlap(&subj, &UNIT, 900);
            assert!(
                (exact - approx).abs() < 0.01,
                "case {case}: exact {exact} vs grid {approx}"
            );
        }
    }

    #[test]
    fn a_concave_subject_clips_to_the_exact_hand_computed_area() {
        // The unit square with a triangular notch cut from its right side (area 0.75).
        let notched = [[0.0, 0.0], [1.0, 0.0], [0.5, 0.4], [1.0, 1.0], [0.0, 1.0]];
        assert!((area(&notched) - 0.75).abs() < 1e-15);
        // Keep x >= 0.4: the strip has area 0.6, and the whole notch (x >= 0.5) lies inside it.
        let half_plane = [[0.4, -1.0], [2.0, -1.0], [2.0, 2.0], [0.4, 2.0]];
        assert!((area(&clip_convex(&notched, &half_plane)) - 0.35).abs() < 1e-15);
        // A clip through the notch splits the subject in two pieces joined along the clip edge:
        // x >= 0.75 leaves the two triangles above and below the notch.
        let right = [[0.75, -1.0], [2.0, -1.0], [2.0, 2.0], [0.75, 2.0]];
        let kept = area(&clip_convex(&notched, &right));
        // At x = 0.75 the notch spans y in [0.2, 0.7], so the notch part with x >= 0.75 is a
        // trapezoid of width 0.25 and parallel sides 0.5 and 1 (area 0.1875) inside a 0.25 strip;
        // the two corner triangles left are 0.5*0.25*0.2 + 0.5*0.25*0.3 = 0.0625.
        assert!((kept - 0.0625).abs() < 1e-12, "{kept}");
    }

    #[test]
    fn bow_ties_are_not_simple_quads() {
        let bow: Quad = [[0.0, 0.0], [1.0, 1.0], [1.0, 0.0], [0.0, 1.0]];
        assert!(!quad_is_simple(&bow));
        assert!(quad_is_simple(&UNIT));
        let dart: Quad = [[0.0, 0.0], [1.0, 0.0], [0.5, 0.3], [0.0, 1.0]];
        assert!(quad_is_simple(&dart));
    }

    #[test]
    fn point_in_convex_handles_both_windings() {
        let mut rev = UNIT;
        rev.reverse();
        for poly in [UNIT, rev] {
            assert!(point_in_convex(&poly, [0.5, 0.5]));
            assert!(!point_in_convex(&poly, [1.5, 0.5]));
        }
    }
}
