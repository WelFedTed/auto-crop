// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Curved pages: the boundary-curve model of `docs/dev/curved-pages.md` (pure maths, no I/O).
//!
//! A page is four editable boundary curves. Each [`Curve`] is a list of 2 to 32 points joined by a
//! **centripetal Catmull-Rom spline** (alpha 0.5, the Barry-Goldman form, a phantom point reflected
//! through each end point). A curve parameter `t` in 0..=1 is chosen by **arc length** on a
//! 64-samples-per-segment polyline ([`ArcCurve`]). [`CurveWarp::to_grid`] and
//! [`CoonsSampler::point`] flatten the page with a **Coons patch**.
//!
//! Every number here is also produced by the labeller's JavaScript; the shared test vectors in
//! `docs/dev/curved-pages-vectors.json` are generated from this file (see the test
//! `vectors_match_the_checked_in_file`) so both implementations are held to the same values.
//!
//! Coordinates are normalised to the EXIF-oriented image (0..1, y down), like every quad. The
//! spline is evaluated in those normalised coordinates; arc length can be measured in a scaled
//! space (`scale = (width, height)` of the image in pixels) so that the parameter is uniform in
//! pixels, whatever the aspect ratio. The scale changes the parameterisation only, never the shape.

use crate::error::ErrKind;
use crate::geometry::{Pt, QuadWarp};
use serde::{Deserialize, Serialize};

/// Fewest points of a curve (two points = a straight edge).
pub const MIN_POINTS: usize = 2;
/// Most points of a curve (the cap checked when a document is loaded).
pub const MAX_POINTS: usize = 32;
/// Polyline samples per spline segment used for arc length.
pub const SAMPLES_PER_SEGMENT: usize = 64;
/// How closely the end points of the curves must agree with each other and with the quad.
pub const CORNER_TOLERANCE: f64 = 1e-6;
/// How far a curve point may lie outside the frame: normalised coordinates must be in
/// `-FRAME_MARGIN..=1 + FRAME_MARGIN` (a page cut by the frame, never a number that overflows a
/// pixel computation).
pub const FRAME_MARGIN: f64 = 1.0;
/// Samples per segment of the coarse self-intersection test.
const CHECK_SAMPLES: usize = 8;
/// Smallest knot interval (guards a division by zero for coincident points).
const MIN_KNOT: f64 = 1e-9;

/// Why a curve or a curved page was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CurveError {
    #[error("a curve has fewer than 2 points")]
    TooFewPoints,
    #[error("a curve has more than 32 points")]
    TooManyPoints,
    #[error("a curve point is not a finite number")]
    NonFinite,
    #[error("two neighbouring points of a curve coincide")]
    CoincidentPoints,
    #[error("a curve point is more than one frame width or height outside the image")]
    OutOfRange,
    #[error("the end points of the curves do not meet (or do not match the quad)")]
    CornerMismatch,
    #[error("the page outline crosses itself")]
    SelfIntersecting,
    #[error("the page outline has no area")]
    NoArea,
}

impl CurveError {
    /// The engine-wide code for this refusal.
    pub fn kind(self) -> ErrKind {
        ErrKind::Degenerate
    }
}

fn lerp(a: Pt, b: Pt, t: f64) -> Pt {
    Pt::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t)
}

fn dist(a: Pt, b: Pt) -> f64 {
    (a.x - b.x).hypot(a.y - b.y)
}

fn scaled_dist(a: Pt, b: Pt, scale: (f64, f64)) -> f64 {
    ((a.x - b.x) * scale.0).hypot((a.y - b.y) * scale.1)
}

/// One boundary curve: 2 to 32 finite points, in order along the curve.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Vec<Pt>", into = "Vec<Pt>")]
pub struct Curve(Vec<Pt>);

impl TryFrom<Vec<Pt>> for Curve {
    type Error = CurveError;
    fn try_from(points: Vec<Pt>) -> Result<Self, CurveError> {
        Curve::new(points)
    }
}

impl From<Curve> for Vec<Pt> {
    fn from(c: Curve) -> Vec<Pt> {
        c.0
    }
}

impl Curve {
    /// A curve through `points`; refused with fewer than 2, more than 32, or a non-finite point.
    pub fn new(points: Vec<Pt>) -> Result<Self, CurveError> {
        if points.len() < MIN_POINTS {
            return Err(CurveError::TooFewPoints);
        }
        if points.len() > MAX_POINTS {
            return Err(CurveError::TooManyPoints);
        }
        if points.iter().any(|p| !p.is_finite()) {
            return Err(CurveError::NonFinite);
        }
        Ok(Self(points))
    }

    /// The straight edge from `a` to `b`.
    pub fn straight(a: Pt, b: Pt) -> Self {
        Self(vec![a, b])
    }

    pub fn points(&self) -> &[Pt] {
        &self.0
    }

    pub fn first(&self) -> Pt {
        self.0[0]
    }

    pub fn last(&self) -> Pt {
        self.0[self.0.len() - 1]
    }

    /// Number of spline segments (points minus one).
    pub fn segments(&self) -> usize {
        self.0.len() - 1
    }

    /// True for a two-point curve.
    pub fn is_straight(&self) -> bool {
        self.0.len() == 2
    }

    /// The same curve run from its last point to its first.
    pub fn reversed(&self) -> Self {
        let mut p = self.0.clone();
        p.reverse();
        Self(p)
    }

    /// The point at local parameter `s` (0..=1, by the spline's own knots, NOT arc length) of
    /// segment `seg` (between points `seg` and `seg + 1`). Centripetal Catmull-Rom, Barry-Goldman
    /// form; the first and last segments use a phantom point `2 P0 - P1` / `2 Pn - Pn-1`.
    pub fn eval_segment(&self, seg: usize, s: f64) -> Pt {
        let p = &self.0;
        let n = p.len();
        let seg = seg.min(n - 2);
        let (p1, p2) = (p[seg], p[seg + 1]);
        if n == 2 {
            return lerp(p1, p2, s);
        }
        let p0 = if seg == 0 {
            Pt::new(2.0 * p[0].x - p[1].x, 2.0 * p[0].y - p[1].y)
        } else {
            p[seg - 1]
        };
        let p3 = if seg + 2 >= n {
            Pt::new(2.0 * p[n - 1].x - p[n - 2].x, 2.0 * p[n - 1].y - p[n - 2].y)
        } else {
            p[seg + 2]
        };
        // Centripetal knots: the interval is the square root of the distance.
        let knot = |a: Pt, b: Pt| dist(a, b).sqrt().max(MIN_KNOT);
        let t0 = 0.0;
        let t1 = t0 + knot(p0, p1);
        let t2 = t1 + knot(p1, p2);
        let t3 = t2 + knot(p2, p3);
        let t = t1 + s * (t2 - t1);
        let mix = |a: Pt, b: Pt, ta: f64, tb: f64| -> Pt {
            // (tb - t) / (tb - ta) a + (t - ta) / (tb - ta) b
            let d = tb - ta;
            let (wa, wb) = ((tb - t) / d, (t - ta) / d);
            Pt::new(wa * a.x + wb * b.x, wa * a.y + wb * b.y)
        };
        let a1 = mix(p0, p1, t0, t1);
        let a2 = mix(p1, p2, t1, t2);
        let a3 = mix(p2, p3, t2, t3);
        let b1 = mix(a1, a2, t0, t2);
        let b2 = mix(a2, a3, t1, t3);
        mix(b1, b2, t1, t2)
    }

    /// The polyline of [`SAMPLES_PER_SEGMENT`] steps per segment (`64 * segments + 1` points; the
    /// first and last are the end points exactly).
    pub fn polyline(&self) -> Vec<Pt> {
        self.polyline_with(SAMPLES_PER_SEGMENT)
    }

    fn polyline_with(&self, per_segment: usize) -> Vec<Pt> {
        let segs = self.segments();
        let mut out = Vec::with_capacity(segs * per_segment + 1);
        out.push(self.first());
        for seg in 0..segs {
            for k in 1..=per_segment {
                if k == per_segment {
                    // The knot itself, exactly (no round-off from the spline at s = 1).
                    out.push(self.0[seg + 1]);
                } else {
                    out.push(self.eval_segment(seg, k as f64 / per_segment as f64));
                }
            }
        }
        out
    }

    /// The arc-length parameterisation, with lengths measured in `scale` space (use `(1.0, 1.0)`
    /// for normalised units, `(width, height)` for pixels).
    pub fn arc(&self, scale: (f64, f64)) -> ArcCurve {
        ArcCurve::from_polyline(self.polyline(), scale)
    }

    /// Arc length in `scale` space.
    pub fn length(&self, scale: (f64, f64)) -> f64 {
        self.arc(scale).length()
    }

    /// The point at arc-length fraction `t` (clamped to 0..=1).
    pub fn at(&self, t: f64, scale: (f64, f64)) -> Pt {
        self.arc(scale).at(t)
    }
}

/// A curve prepared for arc-length lookups: the polyline and its cumulative length.
#[derive(Debug, Clone, PartialEq)]
pub struct ArcCurve {
    pts: Vec<Pt>,
    cum: Vec<f64>,
}

impl ArcCurve {
    /// The arc-length table of any polyline of at least 2 points (`None` for fewer): the curve's
    /// own polyline moved into another space (the renderer rectifies it with the page's corner
    /// homography), measured in `scale` space.
    pub fn from_points(pts: Vec<Pt>, scale: (f64, f64)) -> Option<Self> {
        (pts.len() >= 2).then(|| Self::from_polyline(pts, scale))
    }

    fn from_polyline(pts: Vec<Pt>, scale: (f64, f64)) -> Self {
        let mut cum = Vec::with_capacity(pts.len());
        let mut total = 0.0;
        cum.push(0.0);
        for w in pts.windows(2) {
            total += scaled_dist(w[0], w[1], scale);
            cum.push(total);
        }
        Self { pts, cum }
    }

    /// Total length.
    pub fn length(&self) -> f64 {
        self.cum[self.cum.len() - 1]
    }

    /// The polyline point at fraction `t` of the total length (`t` clamped to 0..=1): the last
    /// polyline segment that starts at or before the target length, linearly interpolated. A
    /// curve of zero length returns its first point.
    pub fn at(&self, t: f64) -> Pt {
        let total = self.length();
        let n = self.pts.len();
        if total.is_nan() || total <= 0.0 {
            return self.pts[0];
        }
        let t = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) };
        let target = t * total;
        let upper = self.cum.partition_point(|c| *c <= target);
        let k = upper.saturating_sub(1).min(n - 2);
        let span = self.cum[k + 1] - self.cum[k];
        let f = if span > 0.0 {
            ((target - self.cum[k]) / span).clamp(0.0, 1.0)
        } else {
            0.0
        };
        lerp(self.pts[k], self.pts[k + 1], f)
    }
}

/// A flattening grid: `cols * rows` source-space nodes, row-major, normalised like the curves.
#[derive(Debug, Clone, PartialEq)]
pub struct CurveGrid {
    pub cols: usize,
    pub rows: usize,
    pub nodes: Vec<Pt>,
}

/// The four boundary curves of a page, plus the turns and mirror of the result. The end points of
/// the curves ARE the corners of the page (TL, TR, BR, BL, clockwise): `top` runs TL to TR, `right`
/// TR to BR, `bottom` BR to BL, `left` BL to TL.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CurveWarp {
    pub top: Curve,
    pub right: Curve,
    pub bottom: Curve,
    pub left: Curve,
    /// Clockwise quarter turns applied to the flattened page, 0..=3.
    #[serde(default)]
    pub quarter_turns: u8,
    /// Mirror the flattened page left to right (applied before the turns).
    #[serde(default)]
    pub mirror: bool,
}

impl CurveWarp {
    /// A page with four straight edges: the quad's corners with no interior points. The quad's
    /// quarter turns and mirror are kept; its fine angle is NOT (the caller bakes it into the
    /// corners first, see `auto_crop_imgproc::render::effective_corners`).
    pub fn from_quad(q: &QuadWarp) -> Self {
        let [tl, tr, br, bl] = q.corners;
        Self {
            top: Curve::straight(tl, tr),
            right: Curve::straight(tr, br),
            bottom: Curve::straight(br, bl),
            left: Curve::straight(bl, tl),
            quarter_turns: q.quarter_turns % 4,
            mirror: q.mirror,
        }
    }

    /// The corners TL, TR, BR, BL (the start of `top`, of `right`, of `bottom`, of `left`).
    pub fn corners(&self) -> [Pt; 4] {
        [
            self.top.first(),
            self.right.first(),
            self.bottom.first(),
            self.left.first(),
        ]
    }

    /// The straight quad through the corners, with this page's turns and mirror.
    pub fn outline(&self) -> QuadWarp {
        QuadWarp {
            corners: self.corners(),
            quarter_turns: self.quarter_turns % 4,
            mirror: self.mirror,
            fine_deg: 0.0,
        }
    }

    /// The curves in the order top, right, bottom, left.
    pub fn curves(&self) -> [&Curve; 4] {
        [&self.top, &self.right, &self.bottom, &self.left]
    }

    /// True when all four edges are straight (no interior points).
    pub fn is_straight(&self) -> bool {
        self.curves().iter().all(|c| c.is_straight())
    }

    /// Checks everything that can be checked without a quad: point counts and finiteness (also
    /// enforced when a curve is built), that the end points meet, that no two neighbouring points
    /// coincide, that the outline has an area and does not cross itself (a coarse polyline test).
    pub fn validate(&self) -> Result<(), CurveError> {
        for c in self.curves() {
            if c.0.len() < MIN_POINTS {
                return Err(CurveError::TooFewPoints);
            }
            if c.0.len() > MAX_POINTS {
                return Err(CurveError::TooManyPoints);
            }
            if c.0.iter().any(|p| !p.is_finite()) {
                return Err(CurveError::NonFinite);
            }
            let range = -FRAME_MARGIN..=1.0 + FRAME_MARGIN;
            if c.0
                .iter()
                .any(|p| !range.contains(&p.x) || !range.contains(&p.y))
            {
                return Err(CurveError::OutOfRange);
            }
            if c.0.windows(2).any(|w| dist(w[0], w[1]) < MIN_KNOT) {
                return Err(CurveError::CoincidentPoints);
            }
        }
        let joins = [
            (self.top.last(), self.right.first()),
            (self.right.last(), self.bottom.first()),
            (self.bottom.last(), self.left.first()),
            (self.left.last(), self.top.first()),
        ];
        if joins.iter().any(|(a, b)| {
            (a.x - b.x).abs() > CORNER_TOLERANCE || (a.y - b.y).abs() > CORNER_TOLERANCE
        }) {
            return Err(CurveError::CornerMismatch);
        }
        let poly = self.outline_polyline(CHECK_SAMPLES);
        if polygon_area(&poly).abs() < 1e-9 {
            return Err(CurveError::NoArea);
        }
        if polygon_self_intersects(&poly) {
            return Err(CurveError::SelfIntersecting);
        }
        Ok(())
    }

    /// [`CurveWarp::validate`], and the corners must also equal `quad`'s within 1e-6.
    pub fn validate_with_quad(&self, quad: &QuadWarp) -> Result<(), CurveError> {
        self.validate()?;
        let ok = self.corners().iter().zip(&quad.corners).all(|(a, b)| {
            (a.x - b.x).abs() <= CORNER_TOLERANCE && (a.y - b.y).abs() <= CORNER_TOLERANCE
        });
        if ok {
            Ok(())
        } else {
            Err(CurveError::CornerMismatch)
        }
    }

    /// The closed outline as a polyline (top, right, bottom, left; each corner once).
    fn outline_polyline(&self, per_segment: usize) -> Vec<Pt> {
        let mut out = Vec::new();
        for c in self.curves() {
            let mut p = c.polyline_with(per_segment);
            p.pop(); // the next curve starts at this corner
            out.extend(p);
        }
        out
    }

    /// Arc lengths of top, right, bottom and left in `scale` space.
    pub fn arc_lengths(&self, scale: (f64, f64)) -> [f64; 4] {
        [
            self.top.length(scale),
            self.right.length(scale),
            self.bottom.length(scale),
            self.left.length(scale),
        ]
    }

    /// The natural size of the flattened page before the turns, in `scale` space: the longer of
    /// top and bottom by the longer of left and right.
    pub fn flat_size(&self, scale: (f64, f64)) -> (f64, f64) {
        let [t, r, b, l] = self.arc_lengths(scale);
        (t.max(b), l.max(r))
    }

    /// Arc-length tables of the four edges, for sampling the patch point by point.
    pub fn sampler(&self, scale: (f64, f64)) -> CoonsSampler {
        CoonsSampler {
            top: self.top.arc(scale),
            right: self.right.arc(scale),
            bottom: self.bottom.arc(scale),
            left: self.left.arc(scale),
            corners: self.corners(),
        }
    }

    /// The Coons grid in normalised units (see [`CurveWarp::to_grid_scaled`]).
    pub fn to_grid(&self, cols: usize, rows: usize) -> CurveGrid {
        self.to_grid_scaled(cols, rows, (1.0, 1.0))
    }

    /// The Coons patch sampled at `cols` by `rows` nodes (each at least 2): node `(i, j)` is
    /// `S(i / (cols - 1), j / (rows - 1))`, with `u` along the width and `v` down the height. The
    /// edges are walked by arc length measured in `scale` space.
    pub fn to_grid_scaled(&self, cols: usize, rows: usize, scale: (f64, f64)) -> CurveGrid {
        let (cols, rows) = (cols.max(2), rows.max(2));
        let s = self.sampler(scale);
        let mut nodes = Vec::with_capacity(cols * rows);
        for j in 0..rows {
            let v = j as f64 / (rows - 1) as f64;
            for i in 0..cols {
                let u = i as f64 / (cols - 1) as f64;
                nodes.push(s.point(u, v));
            }
        }
        CurveGrid { cols, rows, nodes }
    }
}

/// The four edges as arc-length tables, ready to evaluate the Coons patch.
#[derive(Debug, Clone)]
pub struct CoonsSampler {
    top: ArcCurve,
    right: ArcCurve,
    bottom: ArcCurve,
    left: ArcCurve,
    corners: [Pt; 4],
}

impl CoonsSampler {
    /// `T(u)`: the top edge, left to right.
    pub fn top_at(&self, u: f64) -> Pt {
        self.top.at(u)
    }

    /// `B(u)`: the bottom edge, left to right (the curve runs the other way).
    pub fn bottom_at(&self, u: f64) -> Pt {
        self.bottom.at(1.0 - u)
    }

    /// `L(v)`: the left edge, top to bottom (the curve runs the other way).
    pub fn left_at(&self, v: f64) -> Pt {
        self.left.at(1.0 - v)
    }

    /// `R(v)`: the right edge, top to bottom.
    pub fn right_at(&self, v: f64) -> Pt {
        self.right.at(v)
    }

    /// The corners TL, TR, BR, BL.
    pub fn corners(&self) -> [Pt; 4] {
        self.corners
    }

    /// The patch point: `(1-v) T(u) + v B(u) + (1-u) L(v) + u R(v)` minus the bilinear corner term.
    pub fn point(&self, u: f64, v: f64) -> Pt {
        Self::blend(
            self.corners,
            u,
            v,
            self.top_at(u),
            self.bottom_at(u),
            self.left_at(v),
            self.right_at(v),
        )
    }

    /// The Coons formula from already looked-up edge points (the resampler tabulates them).
    pub fn blend(corners: [Pt; 4], u: f64, v: f64, t: Pt, b: Pt, l: Pt, r: Pt) -> Pt {
        let [tl, tr, br, bl] = corners;
        let (w_tl, w_tr, w_bl, w_br) = ((1.0 - u) * (1.0 - v), u * (1.0 - v), (1.0 - u) * v, u * v);
        let x = (1.0 - v) * t.x + v * b.x + (1.0 - u) * l.x + u * r.x
            - (w_tl * tl.x + w_tr * tr.x + w_bl * bl.x + w_br * br.x);
        let y = (1.0 - v) * t.y + v * b.y + (1.0 - u) * l.y + u * r.y
            - (w_tl * tl.y + w_tr * tr.y + w_bl * bl.y + w_br * br.y);
        Pt::new(x, y)
    }
}

/// Shoelace area of a closed polyline (positive for clockwise on a y-down plane).
fn polygon_area(p: &[Pt]) -> f64 {
    let n = p.len();
    let mut s = 0.0;
    for i in 0..n {
        let (a, b) = (p[i], p[(i + 1) % n]);
        s += a.x * b.y - b.x * a.y;
    }
    s / 2.0
}

fn orient(a: Pt, b: Pt, c: Pt) -> f64 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

/// Do segments `ab` and `cd` cross properly (each strictly on both sides of the other)?
fn segments_cross(a: Pt, b: Pt, c: Pt, d: Pt) -> bool {
    let (o1, o2, o3, o4) = (
        orient(a, b, c),
        orient(a, b, d),
        orient(c, d, a),
        orient(c, d, b),
    );
    ((o1 > 0.0 && o2 < 0.0) || (o1 < 0.0 && o2 > 0.0))
        && ((o3 > 0.0 && o4 < 0.0) || (o3 < 0.0 && o4 > 0.0))
}

/// A coarse test: does any pair of non-adjacent polyline segments of the closed polygon cross?
fn polygon_self_intersects(p: &[Pt]) -> bool {
    let n = p.len();
    for i in 0..n {
        let (a, b) = (p[i], p[(i + 1) % n]);
        for j in i + 2..n {
            if i == 0 && j == n - 1 {
                continue; // adjacent through the closing segment
            }
            let (c, d) = (p[j], p[(j + 1) % n]);
            if segments_cross(a, b, c, d) {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: f64, y: f64) -> Pt {
        Pt::new(x, y)
    }

    fn curve(p: &[(f64, f64)]) -> Curve {
        Curve::new(p.iter().map(|&(x, y)| pt(x, y)).collect()).unwrap()
    }

    /// A page whose edges bulge a little, corners at 0.1 and 0.9.
    pub(crate) fn bulging() -> CurveWarp {
        CurveWarp {
            top: curve(&[(0.1, 0.1), (0.5, 0.06), (0.9, 0.1)]),
            right: curve(&[(0.9, 0.1), (0.94, 0.5), (0.9, 0.9)]),
            bottom: curve(&[(0.9, 0.9), (0.5, 0.95), (0.1, 0.9)]),
            left: curve(&[(0.1, 0.9), (0.07, 0.5), (0.1, 0.1)]),
            quarter_turns: 0,
            mirror: false,
        }
    }

    pub(crate) fn straight_rect() -> CurveWarp {
        CurveWarp::from_quad(&QuadWarp::new([
            pt(0.1, 0.1),
            pt(0.9, 0.1),
            pt(0.9, 0.9),
            pt(0.1, 0.9),
        ]))
    }

    #[test]
    fn point_count_and_finiteness_are_checked() {
        assert_eq!(
            Curve::new(vec![pt(0.0, 0.0)]),
            Err(CurveError::TooFewPoints)
        );
        let many: Vec<Pt> = (0..33).map(|i| pt(f64::from(i) / 40.0, 0.0)).collect();
        assert_eq!(Curve::new(many), Err(CurveError::TooManyPoints));
        let ok: Vec<Pt> = (0..32).map(|i| pt(f64::from(i) / 40.0, 0.0)).collect();
        assert!(Curve::new(ok).is_ok());
        assert_eq!(
            Curve::new(vec![pt(0.0, 0.0), pt(f64::NAN, 1.0)]),
            Err(CurveError::NonFinite)
        );
        // The cap holds when loading a document too.
        let json = format!("[{}]", vec![r#"{"x":0.1,"y":0.2}"#; 33].join(","));
        assert!(serde_json::from_str::<Curve>(&json).is_err());
        assert!(serde_json::from_str::<Curve>(r#"[{"x":0,"y":0}]"#).is_err());
    }

    #[test]
    fn the_spline_passes_through_its_points_and_two_points_are_straight() {
        let c = curve(&[(0.0, 0.0), (0.3, 0.2), (0.6, 0.1), (1.0, 0.5)]);
        for (seg, w) in c.points().windows(2).enumerate() {
            let a = c.eval_segment(seg, 0.0);
            let b = c.eval_segment(seg, 1.0);
            assert!(dist(a, w[0]) < 1e-12 && dist(b, w[1]) < 1e-12);
        }
        let s = Curve::straight(pt(0.2, 0.3), pt(0.8, 0.9));
        let at = s.at(0.25, (1.0, 1.0));
        assert!(dist(at, pt(0.35, 0.45)) < 1e-12);
        assert!((s.length((1.0, 1.0)) - 0.6f64.hypot(0.6)).abs() < 1e-12);
    }

    #[test]
    fn arc_length_parameter_is_uniform_whatever_the_point_spacing() {
        // Many points bunched at the start, few at the end, on the straight line y = 0.
        let c = curve(&[
            (0.0, 0.0),
            (0.01, 0.0),
            (0.02, 0.0),
            (0.03, 0.0),
            (1.0, 0.0),
        ]);
        for t in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let p = c.at(t, (1.0, 1.0));
            // Collinear points: the polyline is the line, so arc length t is x = t.
            assert!((p.x - t).abs() < 1e-9 && p.y.abs() < 1e-12, "{t} {p:?}");
        }
        // A real arc: the half-length point is the half-way point of the polyline.
        let arc = curve(&[(0.0, 0.0), (0.5, 0.3), (1.0, 0.0)]).arc((1.0, 1.0));
        let (a, m, b) = (arc.at(0.25), arc.at(0.5), arc.at(0.75));
        assert!(
            (m.x - 0.5).abs() < 1e-9,
            "symmetric curve: the middle is at x = 0.5"
        );
        assert!((a.x + b.x - 1.0).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9);
    }

    #[test]
    fn scale_changes_the_parameterisation_not_the_shape() {
        let c = curve(&[(0.0, 0.0), (0.4, 0.5), (1.0, 0.0)]);
        let flat = c.arc((1.0, 1.0));
        let wide = c.arc((4000.0, 1000.0));
        assert!(wide.length() > flat.length() * 900.0);
        // The same curve: every sampled point lies on the unscaled polyline.
        let poly = c.polyline();
        let p = wide.at(0.37);
        let near = poly
            .windows(2)
            .map(|w| {
                // Distance from p to the segment.
                let (dx, dy) = (w[1].x - w[0].x, w[1].y - w[0].y);
                let l2 = dx * dx + dy * dy;
                let t = (((p.x - w[0].x) * dx + (p.y - w[0].y) * dy) / l2).clamp(0.0, 1.0);
                dist(p, pt(w[0].x + t * dx, w[0].y + t * dy))
            })
            .fold(f64::INFINITY, f64::min);
        assert!(near < 1e-12, "{near}");
    }

    #[test]
    fn a_straight_quad_is_valid_and_a_coons_patch_of_it_is_bilinear() {
        let w = straight_rect();
        assert_eq!(w.validate(), Ok(()));
        let g = w.to_grid(5, 4);
        for j in 0..4 {
            for i in 0..5 {
                let (u, v) = (i as f64 / 4.0, j as f64 / 3.0);
                let n = g.nodes[j * 5 + i];
                assert!(
                    (n.x - (0.1 + 0.8 * u)).abs() < 1e-12 && (n.y - (0.1 + 0.8 * v)).abs() < 1e-12
                );
            }
        }
    }

    #[test]
    fn the_coons_patch_reproduces_all_four_edges_and_the_corners() {
        let w = bulging();
        let g = w.to_grid(9, 7);
        let s = w.sampler((1.0, 1.0));
        for i in 0..9 {
            let u = i as f64 / 8.0;
            assert!(dist(g.nodes[i], s.top_at(u)) < 1e-12);
            assert!(dist(g.nodes[6 * 9 + i], s.bottom_at(u)) < 1e-12);
        }
        for j in 0..7 {
            let v = j as f64 / 6.0;
            assert!(dist(g.nodes[j * 9], s.left_at(v)) < 1e-12);
            assert!(dist(g.nodes[j * 9 + 8], s.right_at(v)) < 1e-12);
        }
        let [tl, tr, br, bl] = w.corners();
        assert!(dist(g.nodes[0], tl) < 1e-12 && dist(g.nodes[8], tr) < 1e-12);
        assert!(dist(g.nodes[6 * 9 + 8], br) < 1e-12 && dist(g.nodes[6 * 9], bl) < 1e-12);
    }

    #[test]
    fn validation_rejects_broken_pages() {
        let ok = bulging();
        assert_eq!(ok.validate(), Ok(()));
        assert_eq!(ok.validate_with_quad(&ok.outline()), Ok(()));
        let mut off = ok.outline();
        off.corners[2].x += 1e-3;
        assert_eq!(ok.validate_with_quad(&off), Err(CurveError::CornerMismatch));

        let mut open = ok.clone();
        open.right = curve(&[(0.9, 0.1), (0.94, 0.5), (0.91, 0.9)]);
        assert_eq!(open.validate(), Err(CurveError::CornerMismatch));

        // Corners within 1e-6 are fine.
        let mut near = ok.clone();
        near.right = curve(&[(0.9, 0.1 + 5e-7), (0.94, 0.5), (0.9, 0.9)]);
        assert_eq!(near.validate(), Ok(()));

        let mut dup = ok.clone();
        dup.top = curve(&[(0.1, 0.1), (0.1, 0.1), (0.9, 0.1)]);
        assert_eq!(dup.validate(), Err(CurveError::CoincidentPoints));

        // A figure of eight: the top edge dips below the bottom edge in the middle.
        let mut cross = ok.clone();
        cross.top = curve(&[(0.1, 0.1), (0.3, 0.97), (0.7, 0.97), (0.9, 0.1)]);
        assert_eq!(cross.validate(), Err(CurveError::SelfIntersecting));

        // No area: all four corners on a line.
        let line = CurveWarp::from_quad(&QuadWarp::new([
            pt(0.1, 0.1),
            pt(0.4, 0.4),
            pt(0.7, 0.7),
            pt(0.9, 0.9),
        ]));
        assert_eq!(line.validate(), Err(CurveError::NoArea));

        // A page may be cut by the frame, but not run off to numbers that overflow a pixel
        // computation.
        let mut far = ok.clone();
        far.top = curve(&[(0.1, 0.1), (0.5, -0.9), (0.9, 0.1)]);
        assert_eq!(far.validate(), Ok(()));
        far.top = curve(&[(0.1, 0.1), (0.5, -1.5), (0.9, 0.1)]);
        assert_eq!(far.validate(), Err(CurveError::OutOfRange));
        far.top = curve(&[(0.1, 0.1), (1e300, 0.5), (0.9, 0.1)]);
        assert_eq!(far.validate(), Err(CurveError::OutOfRange));
    }

    #[test]
    fn serde_is_camel_case_and_round_trips() {
        let mut w = bulging();
        w.quarter_turns = 3;
        w.mirror = true;
        let json = serde_json::to_string(&w).unwrap();
        assert!(json.contains("\"quarterTurns\":3") && json.contains("\"mirror\":true"));
        assert_eq!(serde_json::from_str::<CurveWarp>(&json).unwrap(), w);
        // Turns and mirror may be left out.
        let v = serde_json::to_value(bulging()).unwrap();
        let mut v = v;
        let o = v.as_object_mut().unwrap();
        o.remove("quarterTurns");
        o.remove("mirror");
        assert_eq!(serde_json::from_value::<CurveWarp>(v).unwrap(), bulging());
        // Unknown fields are refused.
        let bad = json.replace("\"mirror\"", "\"mirrr\"");
        assert!(serde_json::from_str::<CurveWarp>(&bad).is_err());
    }

    #[test]
    fn outline_and_from_quad_agree() {
        let q = QuadWarp {
            quarter_turns: 2,
            mirror: true,
            ..QuadWarp::inset_frame(0.1)
        };
        let w = CurveWarp::from_quad(&q);
        assert!(w.is_straight());
        assert_eq!(w.outline(), q);
        assert_eq!(w.validate_with_quad(&q), Ok(()));
    }

    // ---------------------------------------------------------------- shared test vectors

    use serde_json::{Value, json};

    fn p2(p: Pt) -> Value {
        json!([p.x, p.y])
    }

    fn pts(c: &Curve) -> Value {
        Value::Array(c.points().iter().map(|p| p2(*p)).collect())
    }

    fn curve_case(name: &str, c: &Curve, scale: (f64, f64)) -> Value {
        let arc = c.arc(scale);
        let mut eval = Vec::new();
        for seg in 0..c.segments() {
            for s in [0.0, 0.25, 0.5, 0.75, 1.0] {
                eval.push(json!({"segment": seg, "s": s, "point": p2(c.eval_segment(seg, s))}));
            }
        }
        let at: Vec<Value> = [0.0, 0.1, 0.25, 0.5, 0.75, 0.9, 1.0]
            .iter()
            .map(|&t| json!({"t": t, "point": p2(arc.at(t))}))
            .collect();
        json!({
            "name": name,
            "points": pts(c),
            "scale": [scale.0, scale.1],
            "eval": eval,
            "polylineLength": c.polyline().len(),
            "length": arc.length(),
            "at": at,
        })
    }

    fn coons_case(name: &str, w: &CurveWarp, cols: usize, rows: usize, scale: (f64, f64)) -> Value {
        let g = w.to_grid_scaled(cols, rows, scale);
        json!({
            "name": name,
            "top": pts(&w.top),
            "right": pts(&w.right),
            "bottom": pts(&w.bottom),
            "left": pts(&w.left),
            "scale": [scale.0, scale.1],
            "cols": cols,
            "rows": rows,
            "arcLengths": w.arc_lengths(scale),
            "flatSize": [w.flat_size(scale).0, w.flat_size(scale).1],
            "valid": w.validate().is_ok(),
            "nodes": g.nodes.iter().map(|p| p2(*p)).collect::<Vec<_>>(),
        })
    }

    fn vectors() -> Value {
        let s_curve = curve(&[
            (0.05, 0.5),
            (0.2, 0.35),
            (0.4, 0.62),
            (0.6, 0.4),
            (0.8, 0.58),
            (0.95, 0.5),
        ]);
        let sine: Vec<(f64, f64)> = (0..32)
            .map(|i| {
                let x = f64::from(i) / 31.0;
                (x, 0.5 + 0.08 * (x * std::f64::consts::TAU * 1.5).sin())
            })
            .collect();
        let hairpin = curve(&[(0.1, 0.1), (0.12, 0.6), (0.5, 0.9), (0.88, 0.6), (0.9, 0.1)]);
        let uneven = curve(&[
            (0.0, 0.0),
            (0.01, 0.002),
            (0.02, 0.0),
            (0.03, 0.01),
            (1.0, 0.0),
        ]);
        let one = (1.0, 1.0);
        let px = (4000.0, 3000.0);
        let curves = vec![
            curve_case(
                "straight",
                &Curve::straight(pt(0.2, 0.3), pt(0.8, 0.9)),
                one,
            ),
            curve_case("arc3", &curve(&[(0.0, 0.0), (0.5, 0.3), (1.0, 0.0)]), one),
            curve_case("s_curve", &s_curve, one),
            curve_case("s_curve_pixels", &s_curve, px),
            curve_case("sine32", &curve(&sine), one),
            curve_case("hairpin", &hairpin, px),
            curve_case("uneven_spacing", &uneven, one),
        ];
        let cyl = CurveWarp {
            top: curve(&[
                (0.08, 0.12),
                (0.3, 0.08),
                (0.5, 0.06),
                (0.7, 0.08),
                (0.92, 0.12),
            ]),
            right: curve(&[(0.92, 0.12), (0.93, 0.5), (0.92, 0.88)]),
            bottom: curve(&[
                (0.92, 0.88),
                (0.7, 0.93),
                (0.5, 0.95),
                (0.3, 0.93),
                (0.08, 0.88),
            ]),
            left: curve(&[(0.08, 0.88), (0.07, 0.5), (0.08, 0.12)]),
            quarter_turns: 0,
            mirror: false,
        };
        let coons = vec![
            coons_case("straight_rect", &straight_rect(), 5, 4, one),
            coons_case("bulging", &bulging(), 5, 4, one),
            coons_case("bulging_pixels", &bulging(), 5, 4, px),
            coons_case("cylinder", &cyl, 7, 6, px),
        ];
        json!({
            "version": 1,
            "about": "Test vectors of docs/dev/curved-pages.md, generated by crates/core/src/curve.rs (cargo test -p auto-crop-core vectors; set AUTOCROP_REGEN_VECTORS=1 to rewrite). Points are [x, y]. 'eval' is the centripetal Catmull-Rom spline at local parameter s of a segment; 'at' is the arc-length parameter t on the 64-sample polyline measured in 'scale' space; coons 'nodes' are row-major S(i/(cols-1), j/(rows-1)).",
            "tolerance": 1e-9,
            "curves": curves,
            "coons": coons,
        })
    }

    fn close(a: &Value, b: &Value, path: &str, tol: f64) {
        match (a, b) {
            (Value::Number(x), Value::Number(y)) => {
                let (x, y) = (x.as_f64().unwrap(), y.as_f64().unwrap());
                assert!(
                    (x - y).abs() <= tol * (1.0 + x.abs().max(y.abs())),
                    "{path}: {x} vs {y}"
                );
            }
            (Value::Array(x), Value::Array(y)) => {
                assert_eq!(x.len(), y.len(), "{path}: length");
                for (i, (p, q)) in x.iter().zip(y).enumerate() {
                    close(p, q, &format!("{path}[{i}]"), tol);
                }
            }
            (Value::Object(x), Value::Object(y)) => {
                assert_eq!(x.len(), y.len(), "{path}: keys");
                for (k, p) in x {
                    close(
                        p,
                        y.get(k).unwrap_or(&Value::Null),
                        &format!("{path}.{k}"),
                        tol,
                    );
                }
            }
            _ => assert_eq!(a, b, "{path}"),
        }
    }

    #[test]
    fn vectors_match_the_checked_in_file() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/dev/curved-pages-vectors.json"
        );
        let fresh = vectors();
        if std::env::var_os("AUTOCROP_REGEN_VECTORS").is_some() {
            // One case per line: small diffs, readable in a review.
            let mut text = String::from(
                "{
",
            );
            let obj = fresh.as_object().unwrap();
            let n = obj.len();
            for (i, (k, v)) in obj.iter().enumerate() {
                text.push_str(&format!("  {}: ", serde_json::to_string(k).unwrap()));
                match v {
                    Value::Array(cases) => {
                        text.push_str(
                            "[
",
                        );
                        for (j, c) in cases.iter().enumerate() {
                            text.push_str("    ");
                            text.push_str(&serde_json::to_string(c).unwrap());
                            text.push_str(if j + 1 < cases.len() {
                                ",
"
                            } else {
                                "
"
                            });
                        }
                        text.push_str("  ]");
                    }
                    other => text.push_str(&serde_json::to_string(other).unwrap()),
                }
                text.push_str(if i + 1 < n {
                    ",
"
                } else {
                    "
"
                });
            }
            text.push_str(
                "}
",
            );
            std::fs::write(path, text).unwrap();
            return;
        }
        let text =
            std::fs::read_to_string(path).expect("docs/dev/curved-pages-vectors.json exists");
        let on_disk: Value = serde_json::from_str(&text).unwrap();
        close(&fresh, &on_disk, "$", 1e-9);
    }

    proptest::proptest! {
        #[test]
        fn arc_length_points_are_monotone_and_on_the_polyline(
            n in 2usize..8,
            seed in proptest::collection::vec((0.0f64..1.0, 0.0f64..1.0), 8),
            t in 0.0f64..=1.0,
        ) {
            let p: Vec<(f64, f64)> = seed.iter().take(n).enumerate()
                .map(|(i, (_, y))| (i as f64 / (n - 1) as f64, *y)).collect();
            let c = curve(&p);
            let arc = c.arc((1.0, 1.0));
            let total = arc.length();
            proptest::prop_assert!(total >= dist(c.first(), c.last()) - 1e-12);
            let a = arc.at(t);
            let b = arc.at((t + 0.01).min(1.0));
            // Moving 1% along the arc moves at most 1% of the length (plus round-off).
            proptest::prop_assert!(dist(a, b) <= 0.0101 * total + 1e-9);
            proptest::prop_assert!(dist(arc.at(0.0), c.first()) < 1e-12);
            proptest::prop_assert!(dist(arc.at(1.0), c.last()) < 1e-12);
        }
    }
}
