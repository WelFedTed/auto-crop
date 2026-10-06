// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Curved page edges in the golden-set labels (`docs/dev/curved-pages.md`, the shared spec).
//!
//! A curved page has the straight quad plus up to four boundary curves: `top` (TL to TR), `right`
//! (TR to BR), `bottom` (BR to BL) and `left` (BL to TL). A curve is a list of 2 to 32 points whose
//! first and last point ARE the quad's corners (within 1e-6); between points it is a centripetal
//! Catmull-Rom spline (alpha 0.5, Barry-Goldman form, phantom end points reflected through the end
//! points). An edge that is absent from [`Curves`] is straight.
//!
//! Like [`crate::geom`], this module imports no project crate: the ground-truth maths must not
//! share code with the thing it measures (`crates/core` has its own `curve.rs` for the engine, and
//! the labeller page has a third copy in JavaScript; the shared test vectors keep all three equal).
//!
//! The harness metrics stay quad-based. [`mean_boundary_distance`] is a small, separate helper for
//! the day a predictor emits curves; nothing in the evaluation calls it yet.

use crate::geom::{P, Quad};
use serde::{Deserialize, Serialize};

/// Fewest points on a curve (the two corners: a straight edge).
pub const MIN_POINTS: usize = 2;
/// Most points on a curve, corners included.
pub const MAX_POINTS: usize = 32;
/// How far a curve end point may be from the quad corner it stands for.
pub const ENDPOINT_TOLERANCE: f64 = 1e-6;
/// Names of the four edges, in quad order (edge `i` runs from corner `i` to corner `i + 1`).
pub const EDGE_NAMES: [&str; 4] = ["top", "right", "bottom", "left"];
const CORNER_NAMES: [&str; 4] = ["top-left", "top-right", "bottom-right", "bottom-left"];
/// Samples per spline segment for the arc-length parameter (fixed by the spec).
pub const ARC_SAMPLES_PER_SEGMENT: usize = 64;
/// Samples per spline segment for the self-intersection test (coarse on purpose).
const CROSSING_SAMPLES_PER_SEGMENT: usize = 8;
/// Two consecutive points closer than this are the same point.
const COINCIDENT: f64 = 1e-9;
/// Corners and curve points may lie this many image sizes outside the frame (as the quad may).
const MAX_OUTSIDE: f64 = crate::golden::MAX_OUTSIDE;

/// The curves of one item. Every edge is optional: a missing edge is straight.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Curves {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top: Option<Vec<P>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right: Option<Vec<P>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bottom: Option<Vec<P>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<Vec<P>>,
}

impl Curves {
    /// The stored points of edge `i` (0 top, 1 right, 2 bottom, 3 left), if the edge is given.
    pub fn edge(&self, i: usize) -> Option<&[P]> {
        match i {
            0 => self.top.as_deref(),
            1 => self.right.as_deref(),
            2 => self.bottom.as_deref(),
            3 => self.left.as_deref(),
            _ => None,
        }
    }

    /// The control points of edge `i` for `quad`: the stored ones, or the two corners when the
    /// edge is straight (absent).
    pub fn full_edge(&self, quad: &Quad, i: usize) -> Vec<P> {
        self.edge(i)
            .map_or_else(|| vec![quad[i], quad[(i + 1) % 4]], <[P]>::to_vec)
    }

    /// True when at least one edge has more than its two end points.
    pub fn any_bent(&self) -> bool {
        (0..4).any(|i| self.edge(i).is_some_and(|e| e.len() > MIN_POINTS))
    }
}

fn dist(a: P, b: P) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// Knot spacing of the centripetal parameterisation (distance to the power 0.5), kept above zero
/// so a repeated point cannot divide by zero.
fn knot(a: P, b: P) -> f64 {
    dist(a, b).sqrt().max(1e-9)
}

/// The point at `u` (0..1) on the spline segment between `p1` and `p2`, with `p0` and `p3` as the
/// neighbours: centripetal Catmull-Rom (alpha 0.5) in the Barry-Goldman form.
pub fn segment_point(p0: P, p1: P, p2: P, p3: P, u: f64) -> P {
    let t0 = 0.0;
    let t1 = t0 + knot(p0, p1);
    let t2 = t1 + knot(p1, p2);
    let t3 = t2 + knot(p2, p3);
    let t = t1 + u * (t2 - t1);
    let mix = |a: P, b: P, ta: f64, tb: f64| -> P {
        let w = (t - ta) / (tb - ta);
        [a[0] + w * (b[0] - a[0]), a[1] + w * (b[1] - a[1])]
    };
    let a1 = mix(p0, p1, t0, t1);
    let a2 = mix(p1, p2, t1, t2);
    let a3 = mix(p2, p3, t2, t3);
    let b1 = mix(a1, a2, t0, t2);
    let b2 = mix(a2, a3, t1, t3);
    mix(b1, b2, t1, t2)
}

/// The four control points that shape segment `j` (between `pts[j]` and `pts[j + 1]`), with the
/// phantom end points `2 P0 - P1` and `2 Pn - Pn-1` at the ends.
fn segment_controls(pts: &[P], j: usize) -> [P; 4] {
    let n = pts.len();
    let p0 = if j == 0 {
        [2.0 * pts[0][0] - pts[1][0], 2.0 * pts[0][1] - pts[1][1]]
    } else {
        pts[j - 1]
    };
    let p3 = if j + 2 >= n {
        [
            2.0 * pts[n - 1][0] - pts[n - 2][0],
            2.0 * pts[n - 1][1] - pts[n - 2][1],
        ]
    } else {
        pts[j + 2]
    };
    [p0, pts[j], pts[j + 1], p3]
}

/// A polyline of the whole curve, `per_segment` samples per spline segment, each tagged with the
/// segment it belongs to. Two points give the straight segment. Needs at least two points.
pub fn polyline(pts: &[P], per_segment: usize) -> Vec<(P, usize)> {
    let per = per_segment.max(1);
    let segs = pts.len().saturating_sub(1);
    let mut out = Vec::with_capacity(segs * per + 1);
    for j in 0..segs {
        let [p0, p1, p2, p3] = segment_controls(pts, j);
        for k in 0..per {
            let u = k as f64 / per as f64;
            let p = if pts.len() == 2 {
                [p1[0] + u * (p2[0] - p1[0]), p1[1] + u * (p2[1] - p1[1])]
            } else {
                segment_point(p0, p1, p2, p3, u)
            };
            out.push((p, j));
        }
    }
    if let Some(last) = pts.last() {
        out.push((*last, segs.saturating_sub(1)));
    }
    out
}

/// The point at arc-length fraction `t` (0..1) of the curve, measured on the 64-samples-per-segment
/// polyline.
pub fn point_at(pts: &[P], t: f64) -> P {
    let poly = polyline(pts, ARC_SAMPLES_PER_SEGMENT);
    let mut cum = Vec::with_capacity(poly.len());
    let mut total = 0.0;
    cum.push(0.0);
    for w in poly.windows(2) {
        total += dist(w[0].0, w[1].0);
        cum.push(total);
    }
    if total <= 0.0 {
        return poly[0].0;
    }
    let target = t.clamp(0.0, 1.0) * total;
    let i = cum
        .partition_point(|c| *c < target)
        .clamp(1, poly.len() - 1);
    let (c0, c1) = (cum[i - 1], cum[i]);
    let w = if c1 > c0 {
        (target - c0) / (c1 - c0)
    } else {
        0.0
    };
    let (a, b) = (poly[i - 1].0, poly[i].0);
    [a[0] + w * (b[0] - a[0]), a[1] + w * (b[1] - a[1])]
}

fn cross(o: P, a: P, b: P) -> f64 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

fn proper_cross(p1: P, p2: P, p3: P, p4: P) -> bool {
    let (d1, d2) = (cross(p3, p4, p1), cross(p3, p4, p2));
    let (d3, d4) = (cross(p1, p2, p3), cross(p1, p2, p4));
    d1 != 0.0
        && d2 != 0.0
        && d3 != 0.0
        && d4 != 0.0
        && ((d1 > 0.0) != (d2 > 0.0))
        && ((d3 > 0.0) != (d4 > 0.0))
}

fn point_text(p: P) -> String {
    format!("[{}, {}]", p[0], p[1])
}

/// Everything wrong with the curves of an item whose (valid) quad is `quad`, as readable strings
/// (empty = valid): point counts, non-finite or far-outside points, end points that differ from the
/// quad corners, coincident points, and curves that cross themselves or each other.
pub fn problems(quad: &Quad, curves: &Curves) -> Vec<String> {
    let mut out = Vec::new();
    for (e, name) in EDGE_NAMES.iter().enumerate() {
        let Some(pts) = curves.edge(e) else { continue };
        let n = pts.len();
        if !(MIN_POINTS..=MAX_POINTS).contains(&n) {
            out.push(format!(
                "curves.{name}: {n} point(s), a curve needs {MIN_POINTS} to {MAX_POINTS}"
            ));
            continue;
        }
        if pts.iter().any(|p| !p[0].is_finite() || !p[1].is_finite()) {
            out.push(format!("curves.{name}: non-finite coordinate"));
            continue;
        }
        if pts
            .iter()
            .flatten()
            .any(|v| *v < -MAX_OUTSIDE || *v > 1.0 + MAX_OUTSIDE)
        {
            out.push(format!(
                "curves.{name}: a point lies more than {MAX_OUTSIDE} image sizes outside the frame (coordinates are normalised to 0..1)"
            ));
            continue;
        }
        for (end, corner) in [(pts[0], e), (pts[n - 1], (e + 1) % 4)] {
            let want = quad[corner];
            if (end[0] - want[0]).abs() > ENDPOINT_TOLERANCE
                || (end[1] - want[1]).abs() > ENDPOINT_TOLERANCE
            {
                let which = if corner == e { "first" } else { "last" };
                out.push(format!(
                    "curves.{name}: the {which} point {} must equal the quad's {} corner {} (to 1e-6)",
                    point_text(end),
                    CORNER_NAMES[corner],
                    point_text(want)
                ));
            }
        }
        if let Some(k) = pts.windows(2).position(|w| dist(w[0], w[1]) < COINCIDENT) {
            out.push(format!(
                "curves.{name}: points {} and {} are the same point",
                k + 1,
                k + 2
            ));
        }
    }
    if out.is_empty() {
        out.extend(crossing_problems(quad, curves));
    }
    out
}

/// Curves that cross themselves or each other, found on a coarse polyline of the closed boundary.
fn crossing_problems(quad: &Quad, curves: &Curves) -> Vec<String> {
    // (point, edge) of the closed boundary, the last point of an edge being the first of the next.
    let mut pts: Vec<(P, usize)> = Vec::new();
    for e in 0..4 {
        let full = curves.full_edge(quad, e);
        let poly = polyline(&full, CROSSING_SAMPLES_PER_SEGMENT);
        let keep = poly.len() - 1; // the end point is the next edge's start
        pts.extend(poly.into_iter().take(keep).map(|(p, _)| (p, e)));
    }
    let n = pts.len();
    let mut found: Vec<(usize, usize)> = Vec::new();
    for i in 0..n {
        let (a, b) = (pts[i].0, pts[(i + 1) % n].0);
        for j in i + 2..n {
            if i == 0 && j == n - 1 {
                continue; // adjacent through the wrap-around
            }
            let (c, d) = (pts[j].0, pts[(j + 1) % n].0);
            if proper_cross(a, b, c, d) {
                let pair = (pts[i].1.min(pts[j].1), pts[i].1.max(pts[j].1));
                if !found.contains(&pair) {
                    found.push(pair);
                }
            }
        }
    }
    found
        .into_iter()
        .map(|(a, b)| {
            if a == b {
                format!("curves.{} crosses itself", EDGE_NAMES[a])
            } else {
                format!("curves.{} crosses curves.{}", EDGE_NAMES[a], EDGE_NAMES[b])
            }
        })
        .collect()
}

/// The closed boundary of an item as `per_edge` points per edge, evenly spaced by arc length along
/// each edge (corner `i` is the first sample of edge `i`).
pub fn boundary_samples(quad: &Quad, curves: Option<&Curves>, per_edge: usize) -> Vec<P> {
    let per = per_edge.max(1);
    let mut out = Vec::with_capacity(per * 4);
    for e in 0..4 {
        let full = match curves {
            Some(c) => c.full_edge(quad, e),
            None => vec![quad[e], quad[(e + 1) % 4]],
        };
        for k in 0..per {
            out.push(point_at(&full, k as f64 / per as f64));
        }
    }
    out
}

/// Mean distance between two item boundaries (predicted against labelled), in the units of the
/// coordinates (normalised, so multiply by the image diagonal for pixels). Samples are matched edge
/// by edge at equal arc-length fractions, so both boundaries must start at the same top-left
/// corner. A cheap curve-aware complement to the quad IoU; not used by the harness yet because no
/// predictor emits curves.
pub fn mean_boundary_distance(
    a: (&Quad, Option<&Curves>),
    b: (&Quad, Option<&Curves>),
    per_edge: usize,
) -> f64 {
    let (sa, sb) = (
        boundary_samples(a.0, a.1, per_edge),
        boundary_samples(b.0, b.1, per_edge),
    );
    sa.iter().zip(&sb).map(|(p, q)| dist(*p, *q)).sum::<f64>() / sa.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    const Q: Quad = [[0.1, 0.1], [0.9, 0.1], [0.9, 0.9], [0.1, 0.9]];

    fn bowed() -> Curves {
        Curves {
            top: Some(vec![Q[0], [0.5, 0.06], Q[1]]),
            right: Some(vec![Q[1], [0.94, 0.5], Q[2]]),
            bottom: Some(vec![Q[2], [0.5, 0.95], Q[3]]),
            left: Some(vec![Q[3], [0.06, 0.5], Q[0]]),
        }
    }

    #[test]
    fn a_valid_curved_page_has_no_problems_and_round_trips_as_json() {
        let c = bowed();
        assert!(problems(&Q, &c).is_empty(), "{:?}", problems(&Q, &c));
        assert!(c.any_bent());
        let text = serde_json::to_string(&c).expect("json");
        assert_eq!(serde_json::from_str::<Curves>(&text).expect("parses"), c);
        // An absent edge is straight and is simply not written.
        let only_top = Curves {
            top: c.top.clone(),
            ..Curves::default()
        };
        assert!(
            !serde_json::to_string(&only_top)
                .expect("json")
                .contains("right")
        );
        assert!(problems(&Q, &only_top).is_empty());
        assert_eq!(only_top.full_edge(&Q, 1), vec![Q[1], Q[2]]);
        assert!(!Curves::default().any_bent());
        // Two points per edge is a straight page and valid.
        let straight = Curves {
            top: Some(vec![Q[0], Q[1]]),
            ..Curves::default()
        };
        assert!(problems(&Q, &straight).is_empty());
        assert!(!straight.any_bent());
    }

    #[test]
    fn point_counts_endpoints_and_numbers_are_checked_with_clear_messages() {
        let mut c = bowed();
        c.top = Some(vec![Q[0]]);
        assert!(problems(&Q, &c)[0].contains("curves.top: 1 point(s)"));
        let mut c = bowed();
        c.top = Some(Vec::new());
        assert!(problems(&Q, &c)[0].contains("0 point(s)"));
        let mut c = bowed();
        let mut many = vec![Q[0]];
        for k in 1..=MAX_POINTS - 1 {
            many.push([
                0.1 + 0.8 * k as f64 / MAX_POINTS as f64,
                0.1 - 0.0001 * k as f64,
            ]);
        }
        many.push(Q[1]);
        assert_eq!(many.len(), MAX_POINTS + 1);
        c.top = Some(many.clone());
        assert!(problems(&Q, &c)[0].contains("33 point(s)"));
        many.remove(5);
        c.top = Some(many);
        assert!(problems(&Q, &c).is_empty(), "{:?}", problems(&Q, &c));
        let mut c = bowed();
        c.right = Some(vec![[0.9, 0.1001], [0.94, 0.5], Q[2]]);
        let p = problems(&Q, &c);
        assert!(
            p.len() == 1 && p[0].contains("first point") && p[0].contains("top-right"),
            "{p:?}"
        );
        let mut c = bowed();
        c.left = Some(vec![Q[3], [0.06, 0.5], [0.1, 0.1 + 2e-6]]);
        let p = problems(&Q, &c);
        assert!(
            p[0].contains("last point") && p[0].contains("top-left"),
            "{p:?}"
        );
        // Within 1e-6 is the same corner.
        let mut c = bowed();
        c.left = Some(vec![Q[3], [0.06, 0.5], [0.1, 0.1 + 5e-7]]);
        assert!(problems(&Q, &c).is_empty());
        let mut c = bowed();
        c.bottom = Some(vec![Q[2], [f64::NAN, 0.95], Q[3]]);
        assert!(problems(&Q, &c)[0].contains("non-finite"));
        let mut c = bowed();
        c.bottom = Some(vec![Q[2], [0.5, 40.0], Q[3]]);
        assert!(problems(&Q, &c)[0].contains("outside"));
        let mut c = bowed();
        c.top = Some(vec![Q[0], [0.5, 0.06], [0.5, 0.06], Q[1]]);
        assert!(problems(&Q, &c)[0].contains("same point"));
        // JSON cannot carry NaN, and an unknown edge name does not parse.
        assert!(serde_json::from_str::<Curves>("{\"top\":[[0,0],[1,0]],\"middle\":[]}").is_err());
        assert!(serde_json::from_str::<Curves>("{\"top\":[[0,0,1],[1,0]]}").is_err());
    }

    #[test]
    fn self_intersecting_and_mutually_crossing_curves_are_rejected() {
        // A loop in the top edge.
        let mut c = bowed();
        c.top = Some(vec![
            Q[0],
            [0.6, 0.1],
            [0.6, 0.3],
            [0.4, 0.3],
            [0.4, -0.05],
            Q[1],
        ]);
        let p = problems(&Q, &c);
        assert!(
            p.iter().any(|m| m.contains("curves.top crosses itself")),
            "{p:?}"
        );
        // The top edge dives through the bottom edge.
        let mut c = bowed();
        c.top = Some(vec![Q[0], [0.5, 1.2], Q[1]]);
        let p = problems(&Q, &c);
        assert!(
            p.iter()
                .any(|m| m.contains("curves.top crosses curves.bottom")),
            "{p:?}"
        );
        // Bows that merely stay inside or outside the quad never cross.
        let mut c = bowed();
        c.top = Some(vec![Q[0], [0.5, 0.4], Q[1]]);
        assert!(problems(&Q, &c).is_empty(), "{:?}", problems(&Q, &c));
    }

    #[test]
    fn the_spline_passes_through_its_points_and_is_straight_for_two() {
        let pts = [[0.1, 0.1], [0.3, 0.05], [0.6, 0.15], [0.9, 0.1]];
        let poly = polyline(&pts, 16);
        for (j, p) in pts.iter().enumerate() {
            let at = poly[j * 16].0;
            assert!(dist(at, *p) < 1e-12, "point {j}: {at:?} vs {p:?}");
        }
        assert_eq!(poly.last().expect("end").0, pts[3]);
        let line = [[0.0, 0.0], [1.0, 0.5]];
        for (p, _) in polyline(&line, 8) {
            assert!((p[1] - p[0] * 0.5).abs() < 1e-12);
        }
        let mid = point_at(&line, 0.5);
        assert!(dist(mid, [0.5, 0.25]) < 1e-9);
        assert_eq!(point_at(&line, 0.0), line[0]);
        assert!(dist(point_at(&line, 1.0), line[1]) < 1e-9);
    }

    #[test]
    fn arc_length_parameter_is_independent_of_point_spacing() {
        // Three collinear points, the middle one very close to the start: t = 0.5 is still halfway.
        let pts = [[0.0, 0.0], [0.05, 0.0], [1.0, 0.0]];
        let mid = point_at(&pts, 0.5);
        assert!((mid[0] - 0.5).abs() < 1e-3, "{mid:?}");
        assert!(mid[1].abs() < 1e-9);
    }

    #[test]
    fn mean_boundary_distance_is_zero_for_equal_boundaries_and_grows_with_the_bend() {
        let c = bowed();
        assert!(mean_boundary_distance((&Q, Some(&c)), (&Q, Some(&c)), 16) < 1e-12);
        assert!(mean_boundary_distance((&Q, None), (&Q, Some(&Curves::default())), 16) < 1e-12);
        let d = mean_boundary_distance((&Q, None), (&Q, Some(&c)), 32);
        assert!(d > 0.005 && d < 0.05, "{d}");
        let mut deeper = bowed();
        deeper.top = Some(vec![Q[0], [0.5, 0.0], Q[1]]);
        deeper.bottom = Some(vec![Q[2], [0.5, 1.0], Q[3]]);
        assert!(mean_boundary_distance((&Q, None), (&Q, Some(&deeper)), 32) > d);
    }
}
