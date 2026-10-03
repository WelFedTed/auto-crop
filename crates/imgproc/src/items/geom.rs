// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Small planar geometry for the multi-item detector: convex hull, minimum-area rectangle,
//! convex polygon clipping, IoU and distances. Points are `(x, y)` in a y-down frame.

pub type P = (f64, f64);

pub fn sub(a: P, b: P) -> P {
    (a.0 - b.0, a.1 - b.1)
}

pub fn dot(a: P, b: P) -> f64 {
    a.0 * b.0 + a.1 * b.1
}

pub fn cross(a: P, b: P) -> f64 {
    a.0 * b.1 - a.1 * b.0
}

pub fn len(a: P) -> f64 {
    a.0.hypot(a.1)
}

/// Shoelace area, positive when the corners run clockwise on screen (y down).
pub fn area(p: &[P]) -> f64 {
    let n = p.len();
    let mut s = 0.0;
    for i in 0..n {
        let (a, b) = (p[i], p[(i + 1) % n]);
        s += a.0 * b.1 - b.0 * a.1;
    }
    s / 2.0
}

/// Convex hull (Andrew's monotone chain), clockwise on screen (y down). Sorts `pts`.
pub fn convex_hull(pts: &mut Vec<P>) -> Vec<P> {
    pts.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    pts.dedup();
    if pts.len() < 3 {
        return pts.clone();
    }
    let mut h: Vec<P> = Vec::with_capacity(pts.len() * 2);
    for &p in pts.iter() {
        while h.len() >= 2
            && cross(sub(h[h.len() - 1], h[h.len() - 2]), sub(p, h[h.len() - 2])) <= 0.0
        {
            h.pop();
        }
        h.push(p);
    }
    let lower = h.len() + 1;
    for &p in pts.iter().rev().skip(1) {
        while h.len() >= lower
            && cross(sub(h[h.len() - 1], h[h.len() - 2]), sub(p, h[h.len() - 2])) <= 0.0
        {
            h.pop();
        }
        h.push(p);
    }
    h.pop();
    // The chain above is counter-clockwise in a y-up frame, i.e. clockwise on screen (y down).
    if area(&h) < 0.0 {
        h.reverse();
    }
    h
}

/// A rectangle by centre, unit axis `u` (along the width) and half sizes.
#[derive(Debug, Clone, Copy)]
pub struct Rect {
    pub c: P,
    pub u: P,
    pub hw: f64,
    pub hh: f64,
}

impl Rect {
    pub fn v(&self) -> P {
        (-self.u.1, self.u.0)
    }

    pub fn area(&self) -> f64 {
        4.0 * self.hw * self.hh
    }

    /// Corners clockwise on screen: the TL corner has the smallest (x + y) rotation order start.
    pub fn corners(&self) -> [P; 4] {
        let v = self.v();
        let at = |sw: f64, sh: f64| {
            (
                self.c.0 + self.u.0 * self.hw * sw + v.0 * self.hh * sh,
                self.c.1 + self.u.1 * self.hw * sw + v.1 * self.hh * sh,
            )
        };
        [at(-1.0, -1.0), at(1.0, -1.0), at(1.0, 1.0), at(-1.0, 1.0)]
    }
}

/// Minimum-area enclosing rectangle of a convex polygon (one orientation per hull edge).
pub fn min_area_rect(hull: &[P]) -> Option<Rect> {
    if hull.len() < 3 {
        return None;
    }
    let n = hull.len();
    let mut best: Option<(f64, Rect)> = None;
    for i in 0..n {
        let e = sub(hull[(i + 1) % n], hull[i]);
        let l = len(e);
        if l < 1e-9 {
            continue;
        }
        let u = (e.0 / l, e.1 / l);
        let v = (-u.1, u.0);
        let (mut u0, mut u1, mut v0, mut v1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
        for &p in hull {
            let (a, b) = (dot(p, u), dot(p, v));
            u0 = u0.min(a);
            u1 = u1.max(a);
            v0 = v0.min(b);
            v1 = v1.max(b);
        }
        let a = (u1 - u0) * (v1 - v0);
        if best.as_ref().is_none_or(|(b, _)| a < *b) {
            let (mu, mv) = ((u0 + u1) / 2.0, (v0 + v1) / 2.0);
            let c = (u.0 * mu + v.0 * mv, u.1 * mu + v.1 * mv);
            best = Some((
                a,
                Rect {
                    c,
                    u,
                    hw: (u1 - u0) / 2.0,
                    hh: (v1 - v0) / 2.0,
                },
            ));
        }
    }
    best.map(|(_, r)| r)
}

/// Sutherland-Hodgman clip of `subject` against a convex `clip`, both clockwise on screen.
pub fn clip_convex(subject: &[P], clip: &[P]) -> Vec<P> {
    let mut out: Vec<P> = subject.to_vec();
    let n = clip.len();
    for i in 0..n {
        let (a, b) = (clip[i], clip[(i + 1) % n]);
        let inside = |p: P| cross(sub(b, a), sub(p, a)) >= 0.0;
        let input = std::mem::take(&mut out);
        if input.is_empty() {
            break;
        }
        for j in 0..input.len() {
            let (p, q) = (input[j], input[(j + 1) % input.len()]);
            let (ip, iq) = (inside(p), inside(q));
            if ip != iq {
                let (d1, d2) = (cross(sub(b, a), sub(p, a)), cross(sub(b, a), sub(q, a)));
                let t = d1 / (d1 - d2);
                out.push((p.0 + (q.0 - p.0) * t, p.1 + (q.1 - p.1) * t));
            }
            if iq {
                out.push(q);
            }
        }
    }
    out
}

/// Jaccard index of two convex polygons (any start corner, either winding).
pub fn convex_iou(a: &[P], b: &[P]) -> f64 {
    let (a, b) = (oriented(a), oriented(b));
    let inter = clip_convex(&a, &b);
    let ai = if inter.len() >= 3 {
        area(&inter).abs()
    } else {
        0.0
    };
    let union = area(&a).abs() + area(&b).abs() - ai;
    if union <= 0.0 { 0.0 } else { ai / union }
}

fn point_seg(p: P, a: P, b: P) -> f64 {
    let ab = sub(b, a);
    let d = dot(ab, ab);
    let t = if d < 1e-18 {
        0.0
    } else {
        (dot(sub(p, a), ab) / d).clamp(0.0, 1.0)
    };
    len(sub(p, (a.0 + ab.0 * t, a.1 + ab.1 * t)))
}

fn oriented(p: &[P]) -> Vec<P> {
    let mut v = p.to_vec();
    if area(&v) < 0.0 {
        v.reverse();
    }
    v
}

/// Distance between two convex polygons: 0 when they overlap.
pub fn convex_gap(a: &[P], b: &[P]) -> f64 {
    if clip_convex(&oriented(a), &oriented(b)).len() >= 3 {
        return 0.0;
    }
    let mut best = f64::MAX;
    for (p, q) in [(a, b), (b, a)] {
        for &pt in p {
            for i in 0..q.len() {
                best = best.min(point_seg(pt, q[i], q[(i + 1) % q.len()]));
            }
        }
    }
    best
}

pub fn centroid(p: &[P]) -> P {
    let n = p.len() as f64;
    (
        p.iter().map(|q| q.0).sum::<f64>() / n,
        p.iter().map(|q| q.1).sum::<f64>() / n,
    )
}

/// Orders the corners of a rectangle-like quad clockwise starting at the corner that begins the
/// edge closest to horizontal (the "top" edge), left end first.
pub fn canonical_quad(q: [P; 4]) -> [P; 4] {
    let mut pts = q.to_vec();
    if area(&pts) < 0.0 {
        pts.reverse();
    }
    let mut best = (f64::MAX, 0usize);
    for i in 0..4 {
        let e = sub(pts[(i + 1) % 4], pts[i]);
        // Horizontal edges pointing right; an edge pointing left is the bottom one.
        let ang = e.1.atan2(e.0).abs();
        let score = if e.0 >= 0.0 {
            ang
        } else {
            std::f64::consts::PI - ang + 10.0
        };
        if score < best.0 {
            best = (score, i);
        }
    }
    let i = best.1;
    [pts[i], pts[(i + 1) % 4], pts[(i + 2) % 4], pts[(i + 3) % 4]]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(c: P, w: f64, h: f64, ang: f64) -> Vec<P> {
        let r = Rect {
            c,
            u: (ang.cos(), ang.sin()),
            hw: w / 2.0,
            hh: h / 2.0,
        };
        r.corners().to_vec()
    }

    #[test]
    fn min_area_rect_recovers_a_rotated_rectangle() {
        let q = rect((50.0, 40.0), 60.0, 20.0, 0.4);
        let mut pts = q.clone();
        // interior points must not matter
        pts.push((50.0, 40.0));
        let hull = convex_hull(&mut pts);
        let r = min_area_rect(&hull).expect("rect");
        assert!((r.area() - 1200.0).abs() < 1e-6, "{}", r.area());
        assert!(convex_iou(&r.corners(), &q) > 0.999999);
    }

    #[test]
    fn iou_and_gap_of_axis_aligned_boxes() {
        let a = rect((10.0, 10.0), 10.0, 10.0, 0.0);
        let b = rect((15.0, 10.0), 10.0, 10.0, 0.0);
        assert!((convex_iou(&a, &b) - 1.0 / 3.0).abs() < 1e-9);
        assert_eq!(convex_gap(&a, &b), 0.0);
        let c = rect((30.0, 10.0), 10.0, 10.0, 0.0);
        assert!((convex_gap(&a, &c) - 10.0).abs() < 1e-9);
        assert_eq!(convex_iou(&a, &c), 0.0);
    }

    #[test]
    fn canonical_quad_starts_at_the_top_left_of_the_top_edge() {
        let q = rect((0.0, 0.0), 40.0, 20.0, std::f64::consts::PI); // listed rotated by 180 degrees
        let c = canonical_quad([q[0], q[1], q[2], q[3]]);
        assert!(c[0].0 < c[1].0 && (c[0].1 - c[1].1).abs() < 1e-9);
        assert!(c[1].1 < c[2].1);
    }
}
