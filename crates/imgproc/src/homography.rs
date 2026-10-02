// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Homography solver (ROADMAP M1.23): normalised (Hartley) DLT from four point correspondences,
//! in f64, with a typed degeneracy error.
//!
//! The 8 x 9 DLT system is built in Hartley-normalised coordinates (centroid at the origin, mean
//! distance sqrt(2)) and its one-dimensional null space is found by Gaussian elimination with
//! complete pivoting. Complete pivoting is backward stable and exposes rank deficiency directly:
//! a pivot below `1e-10` of the largest one means duplicate points, three collinear points, or a
//! configuration with no unique homography, and the solver returns
//! [`HomographyError::Degenerate`] instead of a garbage matrix. (`auto-crop-core` has no
//! `ErrKind` yet; the engine maps this error onto `ErrKind::Degenerate` when it lands.)
//!
//! Convention: a [`Homography`] maps `(x, y)` to `((h0 x + h1 y + h2) / w, (h3 x + h4 y + h5) / w)`
//! with `w = h6 x + h7 y + h8`, row-major, normalised so that `h8 == 1` when `|h8|` is not tiny.

/// A point `(x, y)`.
pub type Pt = (f64, f64);

/// Why a homography could not be computed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HomographyError {
    /// Duplicate or collinear points, non-finite input, or a singular matrix: no unique
    /// homography exists.
    Degenerate,
}

impl std::fmt::Display for HomographyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("degenerate point configuration: no unique homography")
    }
}

impl std::error::Error for HomographyError {}

/// A 3 x 3 projective transform, row-major.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Homography(pub [f64; 9]);

type M3 = [f64; 9];

fn mul3(a: &M3, b: &M3) -> M3 {
    let mut o = [0.0; 9];
    for i in 0..3 {
        for j in 0..3 {
            o[i * 3 + j] = (0..3).map(|k| a[i * 3 + k] * b[k * 3 + j]).sum();
        }
    }
    o
}

/// Hartley normalisation: the similarity taking the centroid to the origin and the mean distance
/// from it to sqrt(2). `None` if the points coincide.
fn normalisation(p: &[Pt; 4]) -> Option<M3> {
    let cx = p.iter().map(|q| q.0).sum::<f64>() / 4.0;
    let cy = p.iter().map(|q| q.1).sum::<f64>() / 4.0;
    let mean = p.iter().map(|q| (q.0 - cx).hypot(q.1 - cy)).sum::<f64>() / 4.0;
    if !(mean.is_finite() && mean > 1e-300) {
        return None;
    }
    let s = std::f64::consts::SQRT_2 / mean;
    Some([s, 0.0, -s * cx, 0.0, s, -s * cy, 0.0, 0.0, 1.0])
}

fn apply3(t: &M3, p: Pt) -> Pt {
    (
        t[0] * p.0 + t[1] * p.1 + t[2],
        t[3] * p.0 + t[4] * p.1 + t[5],
    )
}

fn inverse3(m: &M3) -> Option<M3> {
    let [a, b, c, d, e, f, g, h, i] = *m;
    let (c0, c1, c2) = (e * i - f * h, f * g - d * i, d * h - e * g);
    let det = a * c0 + b * c1 + c * c2;
    // |det| over the product of the row norms (Hadamard's bound) is scale-free: it is the sine of
    // the angle between the rows, and tiny exactly when the matrix is singular.
    let rows: f64 = (0..3)
        .map(|r| {
            m[r * 3..r * 3 + 3]
                .iter()
                .map(|v| v * v)
                .sum::<f64>()
                .sqrt()
        })
        .product();
    if !det.is_finite()
        || det.abs().partial_cmp(&(1e-12 * rows)) != Some(std::cmp::Ordering::Greater)
    {
        return None;
    }
    let inv = [
        c0,
        c * h - b * i,
        b * f - c * e,
        c1,
        a * i - c * g,
        c * d - a * f,
        c2,
        b * g - a * h,
        a * e - b * d,
    ];
    Some(inv.map(|v| v / det))
}

/// Null space of an 8 x 9 matrix by Gaussian elimination with complete pivoting. `None` if the
/// rank is below 8 (relative pivot tolerance `1e-10`).
fn null_vector(mut a: [[f64; 9]; 8]) -> Option<[f64; 9]> {
    let mut col_of: [usize; 9] = std::array::from_fn(|i| i); // logical column -> physical column
    let mut first_pivot = 0.0f64;
    for step in 0..8 {
        // Complete pivot: the largest remaining entry.
        let (mut pr, mut pc, mut best) = (step, step, 0.0f64);
        for (r, row) in a.iter().enumerate().skip(step) {
            for (c, v) in row.iter().enumerate().skip(step) {
                if v.abs() > best {
                    (pr, pc, best) = (r, c, v.abs());
                }
            }
        }
        if step == 0 {
            first_pivot = best;
        }
        if best.is_nan() || best <= 1e-10 * first_pivot || !best.is_finite() {
            return None;
        }
        a.swap(step, pr);
        for row in a.iter_mut() {
            row.swap(step, pc);
        }
        col_of.swap(step, pc);
        let piv = a[step][step];
        for r in step + 1..8 {
            let f = a[r][step] / piv;
            if f != 0.0 {
                let top = a[step];
                for c in step..9 {
                    a[r][c] -= f * top[c];
                }
            }
        }
    }
    // Free variable is logical column 8 (set to 1); back-substitute.
    let mut x = [0.0f64; 9];
    x[8] = 1.0;
    for r in (0..8).rev() {
        let mut s = a[r][8];
        for c in r + 1..8 {
            s += a[r][c] * x[c];
        }
        x[r] = -s / a[r][r];
    }
    let mut out = [0.0f64; 9];
    for (logical, v) in x.iter().enumerate() {
        out[col_of[logical]] = *v;
    }
    out.iter().all(|v| v.is_finite()).then_some(out)
}

impl Homography {
    pub const IDENTITY: Homography = Homography([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);

    /// The homography taking `from[i]` to `to[i]` (normalised DLT).
    pub fn from_quads(from: [Pt; 4], to: [Pt; 4]) -> Result<Self, HomographyError> {
        let finite = from
            .iter()
            .chain(&to)
            .all(|p| p.0.is_finite() && p.1.is_finite());
        if !finite {
            return Err(HomographyError::Degenerate);
        }
        let ts = normalisation(&from).ok_or(HomographyError::Degenerate)?;
        let td = normalisation(&to).ok_or(HomographyError::Degenerate)?;
        let mut a = [[0.0f64; 9]; 8];
        for i in 0..4 {
            let (x, y) = apply3(&ts, from[i]);
            let (u, v) = apply3(&td, to[i]);
            a[2 * i] = [x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y, -u];
            a[2 * i + 1] = [0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y, -v];
        }
        let hn = null_vector(a).ok_or(HomographyError::Degenerate)?;
        // A rank-8 system can still have a singular null vector (it then maps the plane onto a
        // line, e.g. three collinear sources against three non-collinear targets).
        inverse3(&hn).ok_or(HomographyError::Degenerate)?;
        let td_inv = inverse3(&td).ok_or(HomographyError::Degenerate)?;
        let h = mul3(&td_inv, &mul3(&hn, &ts));
        Self::normalised(h).ok_or(HomographyError::Degenerate)
    }

    /// Scales to `h8 == 1` (or unit max-norm if `h8` is tiny); `None` for non-finite or zero.
    fn normalised(h: M3) -> Option<Self> {
        let max = h.iter().fold(0.0f64, |s, v| s.max(v.abs()));
        if !(max.is_finite() && max > 0.0) {
            return None;
        }
        let d = if h[8].abs() > 1e-12 * max { h[8] } else { max };
        let out = h.map(|v| v / d);
        out.iter().all(|v| v.is_finite()).then_some(Self(out))
    }

    /// The inverse map; `Err` if the matrix is singular.
    pub fn inverse(&self) -> Result<Self, HomographyError> {
        let inv = inverse3(&self.0).ok_or(HomographyError::Degenerate)?;
        Self::normalised(inv).ok_or(HomographyError::Degenerate)
    }

    /// `self` applied after `first`: `compose(a, b).apply(p) == a.apply(b.apply(p))`.
    pub fn after(&self, first: &Homography) -> Homography {
        let m = mul3(&self.0, &first.0);
        Self::normalised(m).unwrap_or(Homography(m))
    }

    /// Maps a point; `None` if it lands at infinity.
    pub fn apply(&self, x: f64, y: f64) -> Option<Pt> {
        let h = &self.0;
        let w = h[6] * x + h[7] * y + h[8];
        if w.is_nan() || w.abs() <= 1e-300 {
            return None;
        }
        let (u, v) = (
            (h[0] * x + h[1] * y + h[2]) / w,
            (h[3] * x + h[4] * y + h[5]) / w,
        );
        (u.is_finite() && v.is_finite()).then_some((u, v))
    }

    pub fn as_array(&self) -> &[f64; 9] {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SQ: [Pt; 4] = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];

    fn close(a: Pt, b: Pt, tol: f64) -> bool {
        (a.0 - b.0).abs() < tol && (a.1 - b.1).abs() < tol
    }

    #[test]
    fn maps_the_four_points_and_inverts() {
        let quad = [(12.0, 8.0), (95.0, 20.0), (88.0, 110.0), (5.0, 92.0)];
        let h = Homography::from_quads(SQ, quad).unwrap();
        for i in 0..4 {
            assert!(close(h.apply(SQ[i].0, SQ[i].1).unwrap(), quad[i], 1e-10));
        }
        let inv = h.inverse().unwrap();
        for p in [(5.0, 5.0), (1.0, 9.0), (7.5, 2.5)] {
            let q = h.apply(p.0, p.1).unwrap();
            assert!(close(inv.apply(q.0, q.1).unwrap(), p, 1e-10));
        }
    }

    #[test]
    fn identity_and_translation_are_recovered_exactly_enough() {
        let h = Homography::from_quads(SQ, SQ).unwrap();
        for (a, b) in h.0.iter().zip(Homography::IDENTITY.0) {
            assert!((a - b).abs() < 1e-12, "{:?}", h.0);
        }
        let moved = SQ.map(|p| (p.0 + 3.5, p.1 - 2.0));
        let h = Homography::from_quads(SQ, moved).unwrap();
        assert!(close(h.apply(4.0, 4.0).unwrap(), (7.5, 2.0), 1e-12));
    }

    #[test]
    fn duplicate_and_collinear_points_are_degenerate() {
        let flat = [(0.0, 0.0), (1.0, 0.0), (2.0, 0.0), (3.0, 0.0)];
        assert_eq!(
            Homography::from_quads(SQ, flat),
            Err(HomographyError::Degenerate)
        );
        assert_eq!(
            Homography::from_quads(flat, SQ),
            Err(HomographyError::Degenerate)
        );
        let dup = [(0.0, 0.0), (10.0, 0.0), (10.0, 0.0), (0.0, 10.0)];
        assert_eq!(
            Homography::from_quads(dup, SQ),
            Err(HomographyError::Degenerate)
        );
        // Three collinear corners.
        let three = [(0.0, 0.0), (5.0, 0.0), (10.0, 0.0), (0.0, 10.0)];
        assert_eq!(
            Homography::from_quads(three, SQ),
            Err(HomographyError::Degenerate)
        );
        // All four the same point.
        let p = [(2.0, 2.0); 4];
        assert_eq!(
            Homography::from_quads(p, SQ),
            Err(HomographyError::Degenerate)
        );
        let nan = [(f64::NAN, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
        assert_eq!(
            Homography::from_quads(nan, SQ),
            Err(HomographyError::Degenerate)
        );
    }

    #[test]
    fn a_thin_receipt_quad_is_fine_and_a_hair_from_collinear_is_not_flagged() {
        // 20:1 strip, strongly tilted.
        let strip = [
            (100.0, 300.0),
            (3900.0, 340.0),
            (3890.0, 540.0),
            (95.0, 505.0),
        ];
        let target = [(0.0, 0.0), (3800.0, 0.0), (3800.0, 190.0), (0.0, 190.0)];
        let h = Homography::from_quads(strip, target).unwrap();
        for i in 0..4 {
            assert!(close(
                h.apply(strip[i].0, strip[i].1).unwrap(),
                target[i],
                1e-8
            ));
        }
        // Third point 1e-6 relative off the line: legal, barely.
        let near = [(0.0, 0.0), (5.0, 1e-5), (10.0, 0.0), (0.0, 10.0)];
        assert!(Homography::from_quads(near, SQ).is_ok());
    }

    #[test]
    fn compose_matches_sequential_application() {
        let a = Homography::from_quads(SQ, [(1.0, 2.0), (30.0, 1.0), (28.0, 33.0), (2.0, 29.0)])
            .unwrap();
        let b = Homography::from_quads(SQ, [(0.0, 0.0), (20.0, 0.0), (22.0, 15.0), (-3.0, 18.0)])
            .unwrap();
        let ab = a.after(&b);
        let p = (3.0, 4.0);
        let seq = a
            .apply(b.apply(p.0, p.1).unwrap().0, b.apply(p.0, p.1).unwrap().1)
            .unwrap();
        assert!(close(ab.apply(p.0, p.1).unwrap(), seq, 1e-9));
    }
}
