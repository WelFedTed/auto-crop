// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Homography maths. A homography is a row-major 3x3 with `h[8] == 1`.

pub type H = [f64; 9];

/// The homography taking `from[i]` to `to[i]`; `None` for a degenerate configuration.
pub fn homography(from: [(f64, f64); 4], to: [(f64, f64); 4]) -> Option<H> {
    let mut a = [[0.0f64; 9]; 8];
    for i in 0..4 {
        let (x, y) = from[i];
        let (u, v) = to[i];
        a[2 * i] = [x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y, u];
        a[2 * i + 1] = [0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y, v];
    }
    for col in 0..8 {
        let mut piv = col;
        for r in col + 1..8 {
            if a[r][col].abs() > a[piv][col].abs() {
                piv = r;
            }
        }
        if a[piv][col].abs() < 1e-12 {
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

/// Maps a point through `h`; `None` if it lands at infinity.
pub fn apply(h: &H, x: f64, y: f64) -> Option<(f64, f64)> {
    let w = h[6] * x + h[7] * y + h[8];
    if w.abs() < 1e-12 {
        return None;
    }
    Some((
        (h[0] * x + h[1] * y + h[2]) / w,
        (h[3] * x + h[4] * y + h[5]) / w,
    ))
}

pub fn invert(m: &H) -> Option<H> {
    let [a, b, c, d, e, f, g, h, i] = *m;
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    if det.abs() < 1e-18 {
        return None;
    }
    let inv = [
        (e * i - f * h) / det,
        (c * h - b * i) / det,
        (b * f - c * e) / det,
        (f * g - d * i) / det,
        (a * i - c * g) / det,
        (c * d - a * f) / det,
        (d * h - e * g) / det,
        (b * g - a * h) / det,
        (a * e - b * d) / det,
    ];
    let s = inv[8];
    if s.abs() < 1e-18 {
        return None;
    }
    Some(inv.map(|v| v / s))
}

pub fn dist(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

/// Shoelace area; positive when the corners run clockwise on screen (y down).
pub fn polygon_area(p: &[(f64, f64)]) -> f64 {
    let n = p.len();
    let mut s = 0.0;
    for i in 0..n {
        let (x0, y0) = p[i];
        let (x1, y1) = p[(i + 1) % n];
        s += x0 * y1 - x1 * y0;
    }
    s / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    const SQ: [(f64, f64); 4] = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];

    #[test]
    fn maps_corners_exactly_and_inverts() {
        let quad = [(12.0, 8.0), (95.0, 20.0), (88.0, 110.0), (5.0, 92.0)];
        let h = homography(SQ, quad).unwrap();
        for i in 0..4 {
            let (x, y) = apply(&h, SQ[i].0, SQ[i].1).unwrap();
            assert!((x - quad[i].0).abs() < 1e-9 && (y - quad[i].1).abs() < 1e-9);
        }
        let inv = invert(&h).unwrap();
        let (x, y) = apply(&inv, 50.0, 50.0).unwrap();
        let (bx, by) = apply(&h, x, y).unwrap();
        assert!((bx - 50.0).abs() < 1e-8 && (by - 50.0).abs() < 1e-8);
    }

    #[test]
    fn degenerate_quads_are_rejected() {
        let flat = [(0.0, 0.0), (1.0, 0.0), (2.0, 0.0), (3.0, 0.0)];
        assert!(homography(SQ, flat).is_none());
    }

    #[test]
    fn clockwise_area_is_positive() {
        assert!((polygon_area(&SQ) - 100.0).abs() < 1e-9);
        let mut rev = SQ;
        rev.reverse();
        assert!(polygon_area(&rev) < 0.0);
    }
}
