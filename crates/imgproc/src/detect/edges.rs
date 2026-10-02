// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Edge evidence for the page detector.
//!
//! * [`Field`]: a colour-fused gradient field (the strongest channel per pixel, so a white receipt on
//!   a coloured desk shows up even when the grey levels match), an automatic threshold that follows
//!   the image's own texture level instead of a fixed contrast, and thinned edge points with
//!   sub-pixel positions.
//! * [`find_lines`]: a Hough transform over those points (each point only votes for lines that are
//!   perpendicular to its own gradient, with saturated weight so a faint paper edge counts as much as
//!   a strong ink edge), refined by orientation-aware least squares.
//! * [`line_quads`]: four-line combinations scored by how much of each side is backed by edge points.
//!
//! None of this knows which side of an edge is paper, so the polarity of the edge may flip along a
//! side (a desk shading from darker to lighter than the paper). That is the low-contrast case the
//! plain brightness threshold cannot handle.

use super::{Gray, P, gaussian_blur};
use crate::Raster;

const BINS: usize = 180;

/// One thinned edge point.
#[derive(Clone, Copy)]
pub(super) struct EdgePoint {
    pub x: f64,
    pub y: f64,
    /// Direction of the gradient, folded into `[0, pi)`.
    pub phi: f64,
}

pub(super) struct Field {
    pub w: usize,
    pub h: usize,
    pub gx: Vec<f32>,
    pub gy: Vec<f32>,
    pub mag: Vec<f32>,
    /// Gradient magnitude (grey levels per pixel) above which a pixel counts as an edge.
    pub thr: f32,
    pub points: Vec<EdgePoint>,
    /// The blurred colour image, interleaved RGB.
    pub rgb: Vec<f32>,
}

fn channel(r: &Raster, c: usize) -> Gray {
    let (w, h) = (r.width as usize, r.height as usize);
    let v = r
        .data
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| f32::from(p[c]))
        .collect();
    Gray { w, h, v }
}

impl Field {
    pub fn new(proxy: &Raster, sigma: f32, floor: f32, ceil: f32, noise_gain: f32) -> Self {
        let chans = [
            gaussian_blur(&channel(proxy, 0), sigma),
            gaussian_blur(&channel(proxy, 1), sigma),
            gaussian_blur(&channel(proxy, 2), sigma),
        ];
        let (w, h) = (chans[0].w, chans[0].h);
        let mut gx = vec![0.0f32; w * h];
        let mut gy = vec![0.0f32; w * h];
        let mut mag = vec![0.0f32; w * h];
        for y in 1..h.saturating_sub(1) {
            for x in 1..w.saturating_sub(1) {
                let i = y * w + x;
                let mut best = (0.0f32, 0.0f32, 0.0f32);
                for ch in &chans {
                    let v = &ch.v;
                    let (a, b, c) = (v[i - w - 1], v[i - w], v[i - w + 1]);
                    let (d, f) = (v[i - 1], v[i + 1]);
                    let (g, hh, k) = (v[i + w - 1], v[i + w], v[i + w + 1]);
                    let sx = ((c + 2.0 * f + k) - (a + 2.0 * d + g)) / 8.0;
                    let sy = ((g + 2.0 * hh + k) - (a + 2.0 * b + c)) / 8.0;
                    let m2 = sx * sx + sy * sy;
                    if m2 > best.0 {
                        best = (m2, sx, sy);
                    }
                }
                gx[i] = best.1;
                gy[i] = best.2;
                mag[i] = best.0.sqrt();
            }
        }
        // Texture level: the median gradient. Edges are a small share of the pixels, so the median
        // measures noise, desk grain and JPEG blocking, and the threshold rides above it.
        let mut hist = vec![0u32; 1024];
        let mut n = 0u32;
        for y in 1..h.saturating_sub(1) {
            for x in 1..w.saturating_sub(1) {
                let b = ((mag[y * w + x] * 20.0) as usize).min(1023);
                hist[b] += 1;
                n += 1;
            }
        }
        let mut acc = 0u32;
        let mut median = 0.0f32;
        for (b, c) in hist.iter().enumerate() {
            acc += c;
            if acc * 2 >= n {
                median = b as f32 / 20.0;
                break;
            }
        }
        let thr = (median * noise_gain).clamp(floor, ceil);
        let mut rgb = vec![0.0f32; w * h * 3];
        for (c, ch) in chans.iter().enumerate() {
            for (i, v) in ch.v.iter().enumerate() {
                rgb[i * 3 + c] = *v;
            }
        }
        let mut f = Self {
            w,
            h,
            gx,
            gy,
            mag,
            thr,
            points: Vec::new(),
            rgb,
        };
        f.thin();
        f
    }

    /// Non-maximum suppression along the gradient with a parabolic sub-pixel position.
    fn thin(&mut self) {
        let (w, h) = (self.w, self.h);
        let mut pts = Vec::new();
        for y in 2..h.saturating_sub(2) {
            for x in 2..w.saturating_sub(2) {
                let i = y * w + x;
                let m = self.mag[i];
                if m < self.thr {
                    continue;
                }
                let (gx, gy) = (self.gx[i], self.gy[i]);
                let (ax, ay) = (gx.abs(), gy.abs());
                let (dx, dy): (i64, i64) = if ay <= 0.4142 * ax {
                    (1, 0)
                } else if ay >= 2.4142 * ax {
                    (0, 1)
                } else if (gx > 0.0) == (gy > 0.0) {
                    (1, 1)
                } else {
                    (1, -1)
                };
                let j1 = (y as i64 + dy) as usize * w + (x as i64 + dx) as usize;
                let j2 = (y as i64 - dy) as usize * w + (x as i64 - dx) as usize;
                let (m1, m2) = (self.mag[j1], self.mag[j2]);
                if m < m1 || m <= m2 {
                    continue;
                }
                // The ridge of a blurred edge is flat on top, so its local maximum wanders by a
                // pixel or two; the centre of mass of the gradient across the ridge does not.
                let reach: i64 = if dx != 0 && dy != 0 { 2 } else { 3 };
                let (mut sw, mut sk) = (0.0f32, 0.0f32);
                for k in -reach..=reach {
                    let (xx, yy) = (x as i64 + k * dx, y as i64 + k * dy);
                    if xx < 0 || yy < 0 || xx >= w as i64 || yy >= h as i64 {
                        continue;
                    }
                    let mk = self.mag[yy as usize * w + xx as usize];
                    if mk >= 0.5 * m {
                        sw += mk;
                        sk += mk * k as f32;
                    }
                }
                let off = if sw > 0.0 { sk / sw } else { 0.0 };
                let phi = f64::from(gy).atan2(f64::from(gx));
                let phi = if phi < 0.0 {
                    phi + std::f64::consts::PI
                } else {
                    phi
                };
                let phi = if phi >= std::f64::consts::PI {
                    phi - std::f64::consts::PI
                } else {
                    phi
                };
                pts.push(EdgePoint {
                    x: x as f64 + 0.5 + f64::from(off) * dx as f64,
                    y: y as f64 + 0.5 + f64::from(off) * dy as f64,
                    phi,
                });
            }
        }
        self.points = pts;
    }

    /// The blurred colour at `p`, `None` outside the frame.
    pub fn colour(&self, p: P) -> Option<[f32; 3]> {
        if p.0 < 0.0 || p.1 < 0.0 || p.0 >= self.w as f64 || p.1 >= self.h as f64 {
            return None;
        }
        let i = (p.1 as usize * self.w + p.0 as usize) * 3;
        Some([self.rgb[i], self.rgb[i + 1], self.rgb[i + 2]])
    }

    /// Projected gradient magnitude at the pixel under `p` along the unit vector `n`, with the signed
    /// value (positive when the brightness rises along `n`), and whether the gradient is aligned
    /// with `n` (within about 37 degrees). `None` outside the frame.
    pub fn along(&self, p: P, n: P) -> Option<(f32, bool)> {
        if p.0 < 1.0 || p.1 < 1.0 || p.0 >= (self.w - 1) as f64 || p.1 >= (self.h - 1) as f64 {
            return None;
        }
        let i = p.1 as usize * self.w + p.0 as usize;
        let g = self.gx[i] * n.0 as f32 + self.gy[i] * n.1 as f32;
        Some((g, g.abs() >= 0.8 * self.mag[i]))
    }
}

/// A refined line `x cos(theta) + y sin(theta) = rho`, with edge-point coverage along it.
pub(super) struct RLine {
    pub theta: f64,
    pub rho: f64,
    pub n: P,
    pub d: P,
    /// Prefix sums of covered 1 px bins along `d`, the first bin starting at `t0`.
    cov: Vec<u32>,
    t0: f64,
}

fn fold_pi(a: f64) -> f64 {
    let pi = std::f64::consts::PI;
    let a = a.rem_euclid(pi);
    if a >= pi { a - pi } else { a }
}

/// Smallest angle between two undirected line directions, in `0..=pi/2`.
fn line_angle(a: f64, b: f64) -> f64 {
    let d = fold_pi(a - b);
    d.min(std::f64::consts::PI - d)
}

fn make_line(theta: f64, rho: f64) -> (f64, f64) {
    // Normalise so theta is in [0, pi).
    let pi = std::f64::consts::PI;
    if theta < 0.0 {
        (theta + pi, -rho)
    } else if theta >= pi {
        (theta - pi, -rho)
    } else {
        (theta, rho)
    }
}

/// `b`'s offset expressed against `a`'s normal orientation (lines are stored with `theta` in
/// `[0, pi)`, so two near-parallel lines can have normals pointing opposite ways).
fn aligned_rho(a: f64, b: (f64, f64)) -> f64 {
    if (a - b.0).abs() > std::f64::consts::FRAC_PI_2 {
        -b.1
    } else {
        b.1
    }
}

/// Whether two lines have nearly the same direction and offset.
fn similar(a: (f64, f64), b: (f64, f64), max_deg: f64, max_off: f64) -> bool {
    line_angle(a.0, b.0) <= max_deg.to_radians() && (a.1 - aligned_rho(a.0, b)).abs() <= max_off
}

/// Least-squares line through the inliers of `line`: points within `tol` of it whose gradient
/// direction is within `ang` of its normal.
fn refine(
    f: &Field,
    by_bin: &[Vec<u32>],
    mut line: (f64, f64),
    tols: &[f64],
    ang_deg: f64,
) -> Option<((f64, f64), usize)> {
    let mut count = 0;
    for &tol in tols {
        let (c, s) = (line.0.cos(), line.0.sin());
        let centre = (line.0 / std::f64::consts::PI * BINS as f64) as i64;
        let span = (ang_deg / 180.0 * BINS as f64).ceil() as i64;
        let (mut n, mut sx, mut sy) = (0.0f64, 0.0f64, 0.0f64);
        let mut sel: Vec<&EdgePoint> = Vec::new();
        for db in -span..=span {
            let bin = (centre + db).rem_euclid(BINS as i64) as usize;
            for &pi in &by_bin[bin] {
                let p = &f.points[pi as usize];
                if (p.x * c + p.y * s - line.1).abs() <= tol
                    && line_angle(p.phi, line.0) <= ang_deg.to_radians()
                {
                    sel.push(p);
                    n += 1.0;
                    sx += p.x;
                    sy += p.y;
                }
            }
        }
        if sel.len() < 6 {
            return None;
        }
        count = sel.len();
        let (mx, my) = (sx / n, sy / n);
        let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
        for p in &sel {
            let (dx, dy) = (p.x - mx, p.y - my);
            sxx += dx * dx;
            syy += dy * dy;
            sxy += dx * dy;
        }
        // Direction of the line; the normal is perpendicular to it. Keep the side the previous
        // normal pointed to so the angle does not jump by pi.
        let dir = 0.5 * (2.0 * sxy).atan2(sxx - syy);
        let mut theta = dir + std::f64::consts::FRAC_PI_2;
        let mut rho = mx * theta.cos() + my * theta.sin();
        let (t2, r2) = make_line(theta, rho);
        theta = t2;
        rho = r2;
        // Align with the previous orientation (mod pi) for the next pass.
        line = (theta, rho);
    }
    Some((line, count))
}

/// Hough lines over the edge points, refined, deduplicated, strongest first.
pub(super) fn find_lines(f: &Field, max_lines: usize) -> Vec<RLine> {
    let (w, h) = (f.w as f64, f.h as f64);
    let rmax = (w.hypot(h)).ceil() as usize + 2;
    let rn = 2 * rmax + 1;
    // cos and sin of the bin centres, with two extra bins either side for votes that wrap.
    let trig: Vec<(f64, f64)> = (-2..BINS as i64 + 2)
        .map(|b| {
            let t = (b as f64 + 0.5) / BINS as f64 * std::f64::consts::PI;
            (t.cos(), t.sin())
        })
        .collect();
    let mut acc = vec![0u16; BINS * rn];
    let mut by_bin: Vec<Vec<u32>> = vec![Vec::new(); BINS];
    for (pi, p) in f.points.iter().enumerate() {
        let b0 = ((p.phi / std::f64::consts::PI * BINS as f64) as usize).min(BINS - 1);
        by_bin[b0].push(pi as u32);
        for db in -2i64..=2 {
            let ub = b0 as i64 + db;
            let (c, sn) = trig[(ub + 2) as usize];
            let rho = p.x * c + p.y * sn;
            // Past 0 or pi the line comes back with its normal reversed, so rho changes sign.
            let (b, rho) = if ub < 0 {
                ((ub + BINS as i64) as usize, -rho)
            } else if ub >= BINS as i64 {
                ((ub - BINS as i64) as usize, -rho)
            } else {
                (ub as usize, rho)
            };
            let ri = (rho + rmax as f64).round();
            if ri >= 0.0 && (ri as usize) < rn {
                acc[b * rn + ri as usize] += 1;
            }
        }
    }
    // Smoothed vote counts (sum over +-2 px of rho) and local maxima.
    let mut sm = vec![0u32; BINS * rn];
    for b in 0..BINS {
        let row = &acc[b * rn..(b + 1) * rn];
        for r in 2..rn - 2 {
            sm[b * rn + r] = u32::from(row[r - 2])
                + u32::from(row[r - 1])
                + u32::from(row[r])
                + u32::from(row[r + 1])
                + u32::from(row[r + 2]);
        }
    }
    let min_votes = (0.06 * w.max(h)) as u32;
    let mut peaks: Vec<(u32, usize, usize)> = Vec::new();
    for b in 0..BINS {
        for r in 3..rn - 3 {
            let v = sm[b * rn + r];
            if v < min_votes {
                continue;
            }
            let mut is_max = true;
            'n: for db in [-1i64, 0, 1] {
                let bb = (b as i64 + db).rem_euclid(BINS as i64) as usize;
                for dr in [-1i64, 0, 1] {
                    if db == 0 && dr == 0 {
                        continue;
                    }
                    // Neighbouring bin across the wrap has its rho mirrored; skip that comparison.
                    if (b as i64 + db) < 0 || (b as i64 + db) >= BINS as i64 {
                        continue;
                    }
                    let nv = sm[bb * rn + (r as i64 + dr) as usize];
                    if nv > v || (nv == v && (db, dr) < (0, 0)) {
                        is_max = false;
                        break 'n;
                    }
                }
            }
            if is_max {
                peaks.push((v, b, r));
            }
        }
    }
    peaks.sort_by_key(|a| std::cmp::Reverse(a.0));
    let mut chosen: Vec<(f64, f64)> = Vec::new();
    let mut refined: Vec<((f64, f64), usize)> = Vec::new();
    for (_, b, r) in peaks {
        if chosen.len() >= 4 * max_lines {
            break;
        }
        let theta = (b as f64 + 0.5) / BINS as f64 * std::f64::consts::PI;
        let rho = r as f64 - rmax as f64;
        if chosen.iter().any(|c| similar(*c, (theta, rho), 3.0, 6.0)) {
            continue;
        }
        chosen.push((theta, rho));
        let Some((line, inliers)) = refine(f, &by_bin, (theta, rho), &[3.0, 1.8, 1.2], 14.0) else {
            continue;
        };
        if (inliers as f64) < 0.07 * w.max(h) {
            continue;
        }
        if refined.iter().any(|o| similar(o.0, line, 1.5, 2.5)) {
            continue;
        }
        refined.push((line, inliers));
    }
    // A faint edge scatters its votes (the gradient direction is noisy), so rank by what the
    // refined line really collects, not by the raw vote count.
    refined.sort_by_key(|a| std::cmp::Reverse(a.1));
    refined.truncate(max_lines);
    refined
        .into_iter()
        .map(|(line, _)| build(f, &by_bin, line, rmax))
        .collect()
}

fn build(f: &Field, by_bin: &[Vec<u32>], line: (f64, f64), rmax: usize) -> RLine {
    let (theta, rho) = line;
    let (c, s) = (theta.cos(), theta.sin());
    let d = (-s, c);
    let t0 = -(rmax as f64) - 4.0;
    let len = 2 * rmax + 12;
    let mut mark = vec![0u8; len];
    let centre = (theta / std::f64::consts::PI * BINS as f64) as i64;
    for db in -4i64..=4 {
        let bin = (centre + db).rem_euclid(BINS as i64) as usize;
        for &pi in &by_bin[bin] {
            let p = &f.points[pi as usize];
            if (p.x * c + p.y * s - rho).abs() <= 2.0
                && line_angle(p.phi, theta) <= 20f64.to_radians()
            {
                let t = (p.x * d.0 + p.y * d.1 - t0).round();
                if t >= 0.0 && (t as usize) < len {
                    let t = t as usize;
                    for m in mark
                        .iter_mut()
                        .take((t + 3).min(len))
                        .skip(t.saturating_sub(2))
                    {
                        *m = 1;
                    }
                }
            }
        }
    }
    let mut cov = Vec::with_capacity(len + 1);
    cov.push(0u32);
    for m in &mark {
        let last = *cov.last().unwrap_or(&0);
        cov.push(last + u32::from(*m));
    }
    RLine {
        theta,
        rho,
        n: (c, s),
        d,
        cov,
        t0,
    }
}

impl RLine {
    /// Fraction of the segment between `a` and `b` (both on this line) that has edge points.
    fn coverage(&self, a: P, b: P) -> f64 {
        let (ta, tb) = (
            a.0 * self.d.0 + a.1 * self.d.1,
            b.0 * self.d.0 + b.1 * self.d.1,
        );
        let (lo, hi) = (ta.min(tb), ta.max(tb));
        let n = self.cov.len() - 1;
        let idx = |t: f64| ((t - self.t0).round().max(0.0) as usize).min(n);
        let (i, j) = (idx(lo), idx(hi));
        if j <= i {
            return 0.0;
        }
        f64::from(self.cov[j] - self.cov[i]) / (hi - lo).max(1.0)
    }

    fn intersect(&self, o: &RLine) -> Option<P> {
        let det = self.n.0 * o.n.1 - self.n.1 * o.n.0;
        if det.abs() < 1e-3 {
            return None;
        }
        Some((
            (self.rho * o.n.1 - o.rho * self.n.1) / det,
            (self.n.0 * o.rho - o.n.0 * self.rho) / det,
        ))
    }
}

pub(super) struct LineQuad {
    pub q: [P; 4],
}

fn cross(o: P, a: P, b: P) -> f64 {
    (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
}

/// Quadrilaterals made of two near-parallel pairs of lines that cross at roughly right angles,
/// ranked by edge coverage of the four sides.
pub(super) fn line_quads(lines: &[RLine], w: f64, h: f64, keep: usize) -> Vec<LineQuad> {
    let span = w.max(h);
    let n = lines.len();
    // Where two lines cross, if they do so at a plausible corner: not too shallow and inside the
    // frame (plus a margin).
    debug_assert!(n <= 128);
    let mut ix: Vec<Option<P>> = vec![None; n * n];
    let mut ixmask = vec![0u128; n];
    for i in 0..n {
        for j in i + 1..n {
            if line_angle(lines[i].theta, lines[j].theta) < 50f64.to_radians() {
                continue;
            }
            let Some(p) = lines[i].intersect(&lines[j]) else {
                continue;
            };
            if p.0 < -0.05 * w || p.1 < -0.05 * h || p.0 > 1.05 * w || p.1 > 1.05 * h {
                continue;
            }
            ix[i * n + j] = Some(p);
            ix[j * n + i] = Some(p);
            ixmask[i] |= 1 << j;
            ixmask[j] |= 1 << i;
        }
    }
    // Valid opposite sides: nearly parallel and far enough apart.
    let mut parmask = vec![0u128; n];
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    for i in 0..n {
        for j in i + 1..n {
            if line_angle(lines[i].theta, lines[j].theta) > 25f64.to_radians() {
                continue;
            }
            let (a, b) = (&lines[i], &lines[j]);
            if (a.rho - aligned_rho(a.theta, (b.theta, b.rho))).abs() < 0.07 * span {
                continue;
            }
            parmask[i] |= 1 << j;
            parmask[j] |= 1 << i;
            pairs.push((i, j));
        }
    }
    let mut found: Vec<(f64, LineQuad)> = Vec::new();
    for &(a1, a2) in &pairs {
        // Only lines that cross both of the pair can be the other two sides, and the two of them
        // must be opposite sides in turn. (Bit masks keep this a handful of operations.)
        let cross_mask = ixmask[a1] & ixmask[a2];
        let mut m1 = cross_mask;
        while m1 != 0 {
            let b1 = m1.trailing_zeros() as usize;
            m1 &= m1 - 1;
            // Each quad is found once, from the pair holding the lowest line index.
            if a1 > b1 {
                continue;
            }
            let mut m2 = parmask[b1] & cross_mask & !((2u128 << b1) - 1);
            while m2 != 0 {
                let b2 = m2.trailing_zeros() as usize;
                m2 &= m2 - 1;
                let (Some(p0), Some(p1), Some(p2), Some(p3)) = (
                    ix[a1 * n + b1],
                    ix[a1 * n + b2],
                    ix[a2 * n + b2],
                    ix[a2 * n + b1],
                ) else {
                    continue;
                };
                let q = [p0, p1, p2, p3];
                // Convex with a consistent turn.
                let turns: [f64; 4] =
                    std::array::from_fn(|k| cross(q[k], q[(k + 1) % 4], q[(k + 2) % 4]));
                let sign = turns[0].signum();
                if turns.iter().any(|t| t.signum() != sign || t.abs() < 1e-6) {
                    continue;
                }
                let area = crate::geometry::polygon_area(&q).abs();
                if area < 0.03 * w * h {
                    continue;
                }
                let sides = [
                    lines[a1].coverage(p0, p1),
                    lines[b2].coverage(p1, p2),
                    lines[a2].coverage(p2, p3),
                    lines[b1].coverage(p3, p0),
                ];
                let min = sides.iter().cloned().fold(1.0, f64::min);
                let mean = sides.iter().sum::<f64>() / 4.0;
                if min < 0.3 || mean < 0.55 {
                    continue;
                }
                let frac = (area / (w * h)).min(0.95);
                let rank = (0.5 * min + 0.5 * mean) * (0.5 + frac.sqrt());
                found.push((rank, LineQuad { q }));
            }
        }
    }
    found.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut out: Vec<LineQuad> = Vec::new();
    for (_, c) in found {
        if out.len() >= keep {
            break;
        }
        let dup = out.iter().any(|o| {
            (0..4).all(|k| crate::geometry::dist(o.q[k], c.q[k]) < 4.0)
                || (0..4).all(|k| (0..4).any(|m| crate::geometry::dist(o.q[k], c.q[m]) < 4.0))
        });
        if !dup {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A convex quad filled with `inner` on a desk whose brightness runs from `bg0` at the left to
    /// `bg1` at the right (so the edge can flip polarity), plus a little deterministic noise.
    fn picture(quad: [P; 4], inner: f32, bg0: f32, bg1: f32) -> Raster {
        let (w, h) = (240u32, 180u32);
        let mut r = Raster::new(w, h);
        let mut seed = 12345u32;
        for y in 0..h {
            for x in 0..w {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = ((seed >> 24) as f32 / 255.0 - 0.5) * 2.0;
                let p = (f64::from(x) + 0.5, f64::from(y) + 0.5);
                let inside = (0..4).all(|k| cross(quad[k], quad[(k + 1) % 4], p) >= 0.0);
                let bg = bg0 + (bg1 - bg0) * x as f32 / w as f32;
                let v = if inside { inner } else { bg } + noise;
                let v = v.clamp(0.0, 255.0) as u8;
                r.set_pixel(x, y, [v, v, v]);
            }
        }
        r
    }

    fn best_quad(r: &Raster) -> Option<[P; 4]> {
        let f = Field::new(r, 1.2, 1.0, 6.0, 1.5);
        let lines = find_lines(&f, 64);
        line_quads(&lines, f.w as f64, f.h as f64, 6)
            .into_iter()
            .next()
            .map(|q| q.q)
    }

    /// The largest distance from a true corner to the nearest found corner.
    fn worst_corner(found: &[P; 4], truth: &[P; 4]) -> f64 {
        truth
            .iter()
            .map(|t| {
                found
                    .iter()
                    .map(|f| crate::geometry::dist(*f, *t))
                    .fold(f64::MAX, f64::min)
            })
            .fold(0.0, f64::max)
    }

    #[test]
    fn the_four_sides_of_a_faint_rectangle_are_found_to_a_pixel() {
        let truth = [(40.0, 30.0), (190.0, 30.0), (190.0, 140.0), (40.0, 140.0)];
        let q = best_quad(&picture(truth, 140.0, 124.0, 124.0)).expect("a quad");
        assert!(worst_corner(&q, &truth) < 1.5, "{q:?}");
    }

    #[test]
    fn a_tilted_page_is_found_to_a_pixel_for_any_angle_including_the_wrap_at_zero() {
        // Sides whose normals sit either side of 0 and pi exercise the wrap of the accumulator.
        for deg in [-30.0f64, -2.0, -0.4, 0.0, 0.4, 2.0, 25.0, 44.0] {
            let (s, c) = deg.to_radians().sin_cos();
            let centre = (120.0, 90.0);
            let truth: [P; 4] = [(-60.0, -40.0), (60.0, -40.0), (60.0, 40.0), (-60.0, 40.0)]
                .map(|p| (centre.0 + p.0 * c - p.1 * s, centre.1 + p.0 * s + p.1 * c));
            let q = best_quad(&picture(truth, 150.0, 118.0, 118.0)).expect("a quad");
            assert!(worst_corner(&q, &truth) < 2.0, "{deg}: {q:?}");
        }
    }

    #[test]
    fn the_edge_polarity_may_flip_along_the_page() {
        // The desk runs from darker than the page (left) to lighter than it (right).
        let truth = [(50.0, 25.0), (190.0, 30.0), (185.0, 145.0), (45.0, 140.0)];
        let q = best_quad(&picture(truth, 150.0, 128.0, 172.0)).expect("a quad");
        assert!(worst_corner(&q, &truth) < 2.0, "{q:?}");
    }

    #[test]
    fn a_flat_picture_has_no_edges_and_no_lines() {
        let r = Raster::filled(120, 90, [100, 100, 100]);
        let f = Field::new(&r, 1.2, 1.0, 6.0, 1.5);
        assert!(f.points.is_empty());
        assert!(find_lines(&f, 64).is_empty());
    }

    #[test]
    fn the_edge_threshold_follows_the_texture_but_stays_inside_its_bounds() {
        let mut seed = 7u32;
        let mut noisy = Raster::new(100, 80);
        for p in noisy.data.iter_mut() {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *p = (seed >> 24) as u8;
        }
        let f = Field::new(&noisy, 1.2, 1.0, 6.0, 1.5);
        assert!((f.thr - 6.0).abs() < 1e-6, "ceiling: {}", f.thr);
        let flat = Field::new(&Raster::filled(50, 50, [9, 9, 9]), 1.2, 1.0, 6.0, 1.5);
        assert!((flat.thr - 1.0).abs() < 1e-6, "floor: {}", flat.thr);
    }

    #[test]
    fn line_helpers_handle_normals_that_point_opposite_ways() {
        let pi = std::f64::consts::PI;
        // The same vertical line described with theta just above 0 and just below pi.
        let a = (0.01, 100.0);
        let b = (pi - 0.01, -100.0);
        assert!(similar(a, b, 2.0, 1.0));
        assert!((aligned_rho(a.0, b) - 100.0).abs() < 1e-9);
        assert!(!similar(a, (pi / 2.0, 100.0), 5.0, 5.0));
        assert!(line_angle(0.01, pi - 0.01) < 0.03);
        assert_eq!(make_line(-0.1, 5.0), (pi - 0.1, -5.0));
    }
}
