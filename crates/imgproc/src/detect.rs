// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Classical page detector: threshold the paper against the background, take the best connected
//! region, fit a quadrilateral to its hull, then score the result.
//!
//! This is a deliberately simple baseline, not the M4 detector. Its confidence is an UNCALIBRATED
//! heuristic: it orders results sensibly (a clean quad scores high, a weak edge or a page cut by the
//! frame scores lower) but its numbers mean nothing as probabilities.

use crate::Raster;
use crate::scale::resize_to_fit;
use auto_crop_core::{Confidence, Forced, Pt, Reason, ReasonCode, Side};

/// Below this score the detector reports no quad at all (a Failed item).
const MIN_QUAD_SCORE: f32 = 0.6;

/// Proxy size for detection (long edge).
const PROXY_EDGE: u32 = 640;

#[derive(Debug, Clone)]
pub struct Detection {
    /// TL, TR, BR, BL in normalised coordinates, `None` when nothing plausible was found.
    pub quad: Option<[Pt; 4]>,
    pub confidence: Confidence,
}

type P = (f64, f64);

struct Gray {
    w: usize,
    h: usize,
    v: Vec<f32>,
}

impl Gray {
    fn at(&self, x: usize, y: usize) -> f32 {
        self.v[y * self.w + x]
    }

    fn bilinear(&self, x: f64, y: f64) -> Option<f32> {
        if x < 0.0 || y < 0.0 || x > (self.w - 1) as f64 || y > (self.h - 1) as f64 {
            return None;
        }
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (fx, fy) = ((x - x0 as f64) as f32, (y - y0 as f64) as f32);
        let top = self.at(x0, y0) * (1.0 - fx) + self.at(x1, y0) * fx;
        let bot = self.at(x0, y1) * (1.0 - fx) + self.at(x1, y1) * fx;
        Some(top * (1.0 - fy) + bot * fy)
    }
}

fn to_gray(r: &Raster) -> Gray {
    let (w, h) = (r.width as usize, r.height as usize);
    let v = r
        .data
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| 0.299 * f32::from(p[0]) + 0.587 * f32::from(p[1]) + 0.114 * f32::from(p[2]))
        .collect();
    Gray { w, h, v }
}

fn gaussian_blur(g: &Gray, sigma: f32) -> Gray {
    let radius = (sigma * 3.0).ceil() as i64;
    let mut k: Vec<f32> = (-radius..=radius)
        .map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp())
        .collect();
    let s: f32 = k.iter().sum();
    for v in &mut k {
        *v /= s;
    }
    let (w, h) = (g.w, g.h);
    let mut tmp = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut acc = 0.0;
            for (j, kv) in k.iter().enumerate() {
                let xx = (x as i64 + j as i64 - radius).clamp(0, w as i64 - 1) as usize;
                acc += kv * g.v[y * w + xx];
            }
            tmp[y * w + x] = acc;
        }
    }
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut acc = 0.0;
            for (j, kv) in k.iter().enumerate() {
                let yy = (y as i64 + j as i64 - radius).clamp(0, h as i64 - 1) as usize;
                acc += kv * tmp[yy * w + x];
            }
            out[y * w + x] = acc;
        }
    }
    Gray { w, h, v: out }
}

fn otsu(g: &Gray) -> f32 {
    let mut hist = [0u64; 256];
    for v in &g.v {
        hist[(*v).round().clamp(0.0, 255.0) as usize] += 1;
    }
    let total = g.v.len() as f64;
    let sum_all: f64 = hist
        .iter()
        .enumerate()
        .map(|(i, c)| i as f64 * *c as f64)
        .sum();
    let (mut w0, mut sum0, mut best, mut thr) = (0.0f64, 0.0f64, -1.0f64, 128usize);
    for (t, c) in hist.iter().enumerate() {
        w0 += *c as f64;
        if w0 == 0.0 {
            continue;
        }
        let w1 = total - w0;
        if w1 == 0.0 {
            break;
        }
        sum0 += t as f64 * *c as f64;
        let (m0, m1) = (sum0 / w0, (sum_all - sum0) / w1);
        let between = w0 * w1 * (m0 - m1) * (m0 - m1);
        if between > best {
            best = between;
            thr = t;
        }
    }
    thr as f32 + 0.5
}

/// 3x3 erosion (`erode = true`) or dilation of a binary mask; outside the frame counts as the
/// value that does not change the result (so a page touching the frame stays attached to it).
fn morph(m: &[bool], w: usize, h: usize, erode: bool) -> Vec<bool> {
    let mut out = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut all = true;
            let mut any = false;
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let (xx, yy) = (x as i64 + dx, y as i64 + dy);
                    let v = if xx < 0 || yy < 0 || xx >= w as i64 || yy >= h as i64 {
                        m[y * w + x]
                    } else {
                        m[yy as usize * w + xx as usize]
                    };
                    all &= v;
                    any |= v;
                }
            }
            out[y * w + x] = if erode { all } else { any };
        }
    }
    out
}

struct Component {
    pixels: Vec<usize>,
    area: usize,
}

fn components(mask: &[bool], w: usize, h: usize, min_area: usize, keep: usize) -> Vec<Component> {
    let mut seen = vec![false; w * h];
    let mut found = Vec::new();
    let mut stack = Vec::new();
    for start in 0..w * h {
        if !mask[start] || seen[start] {
            continue;
        }
        let mut pixels = Vec::new();
        seen[start] = true;
        stack.push(start);
        while let Some(i) = stack.pop() {
            pixels.push(i);
            let (x, y) = (i % w, i / w);
            let mut push = |j: usize| {
                if mask[j] && !seen[j] {
                    seen[j] = true;
                    stack.push(j);
                }
            };
            if x > 0 {
                push(i - 1);
            }
            if x + 1 < w {
                push(i + 1);
            }
            if y > 0 {
                push(i - w);
            }
            if y + 1 < h {
                push(i + w);
            }
        }
        if pixels.len() >= min_area {
            found.push(Component {
                area: pixels.len(),
                pixels,
            });
        }
    }
    found.sort_by_key(|c| std::cmp::Reverse(c.area));
    found.truncate(keep);
    found
}

fn cross(o: P, a: P, b: P) -> f64 {
    (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
}

/// Convex hull (Andrew's monotone chain). Returned clockwise on screen (y down).
fn convex_hull(mut pts: Vec<P>) -> Vec<P> {
    pts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let mut lower: Vec<P> = Vec::new();
    for &p in &pts {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 0.0 {
            lower.pop();
        }
        lower.push(p);
    }
    let mut upper: Vec<P> = Vec::new();
    for &p in pts.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 0.0 {
            upper.pop();
        }
        upper.push(p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    // With y down this orientation is visually clockwise already; make sure.
    if crate::geometry::polygon_area(&lower) < 0.0 {
        lower.reverse();
    }
    lower
}

fn tri_area(a: P, b: P, c: P) -> f64 {
    (cross(a, b, c) / 2.0).abs()
}

/// Reduces a convex polygon to four vertices by repeatedly dropping the least significant one.
fn reduce_to_four(hull: &[P]) -> Vec<usize> {
    let mut alive: Vec<usize> = (0..hull.len()).collect();
    while alive.len() > 4 {
        let n = alive.len();
        let mut best = (f64::MAX, 0usize);
        for k in 0..n {
            let a = tri_area(
                hull[alive[(k + n - 1) % n]],
                hull[alive[k]],
                hull[alive[(k + 1) % n]],
            );
            if a < best.0 {
                best = (a, k);
            }
        }
        alive.remove(best.1);
    }
    alive
}

struct Line {
    p: P,
    d: P,
}

fn fit_line(pts: &[P]) -> Option<Line> {
    if pts.len() < 2 {
        return None;
    }
    let n = pts.len() as f64;
    let (mx, my) = (
        pts.iter().map(|p| p.0).sum::<f64>() / n,
        pts.iter().map(|p| p.1).sum::<f64>() / n,
    );
    let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
    for p in pts {
        let (dx, dy) = (p.0 - mx, p.1 - my);
        sxx += dx * dx;
        syy += dy * dy;
        sxy += dx * dy;
    }
    let angle = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    Some(Line {
        p: (mx, my),
        d: (angle.cos(), angle.sin()),
    })
}

fn intersect(a: &Line, b: &Line) -> Option<P> {
    let det = a.d.0 * (-b.d.1) - a.d.1 * (-b.d.0);
    if det.abs() < 1e-6 {
        return None;
    }
    let (rx, ry) = (b.p.0 - a.p.0, b.p.1 - a.p.1);
    let t = (rx * (-b.d.1) - ry * (-b.d.0)) / det;
    Some((a.p.0 + t * a.d.0, a.p.1 + t * a.d.1))
}

/// Four corners from a hull: the reduced vertices, then each side refit by least squares so the
/// corners sit on the straight edges instead of the rounded mask corners.
fn quad_from_hull(hull: &[P]) -> Option<[P; 4]> {
    if hull.len() < 4 {
        return None;
    }
    let idx = reduce_to_four(hull);
    let raw: [P; 4] = std::array::from_fn(|i| hull[idx[i]]);
    let mut lines: Vec<Line> = Vec::new();
    for k in 0..4 {
        let (a, b) = (idx[k], idx[(k + 1) % 4]);
        let mut seg = Vec::new();
        let mut i = a;
        loop {
            seg.push(hull[i]);
            if i == b {
                break;
            }
            i = (i + 1) % hull.len();
        }
        let trim = seg.len() / 7;
        let core = if seg.len() > 2 * trim + 3 {
            &seg[trim..seg.len() - trim]
        } else {
            &seg[..]
        };
        match fit_line(core) {
            Some(l) => lines.push(l),
            None => return Some(raw),
        }
    }
    let mut refined = [(0.0, 0.0); 4];
    let diag = crate::geometry::dist(raw[0], raw[2]).max(crate::geometry::dist(raw[1], raw[3]));
    for k in 0..4 {
        match intersect(&lines[(k + 3) % 4], &lines[k]) {
            Some(p) if crate::geometry::dist(p, raw[k]) < diag * 0.06 => refined[k] = p,
            _ => return Some(raw),
        }
    }
    Some(refined)
}

/// Rotates the corner list so corner 0 starts the edge that is closest to horizontal and points
/// right (TL, TR, BR, BL).
fn orient(q: [P; 4]) -> [P; 4] {
    let mut best = (f64::MAX, 0usize);
    for k in 0..4 {
        let (a, b) = (q[k], q[(k + 1) % 4]);
        let ang = (b.1 - a.1).atan2(b.0 - a.0).abs();
        if ang < best.0 {
            best = (ang, k);
        }
    }
    std::array::from_fn(|i| q[(i + best.1) % 4])
}

struct Candidate {
    quad: [P; 4],
    score: f32,
    forced: Option<Forced>,
    reasons: Vec<Reason>,
}

fn side_of(i: usize) -> Side {
    Side::ALL[i % 4]
}

fn evaluate(
    quad_raw: [P; 4],
    comp_area: usize,
    inside_mean: f32,
    outside_mean: f32,
    blurred: &Gray,
) -> Candidate {
    let (w, h) = (blurred.w as f64, blurred.h as f64);
    let q = orient(quad_raw);
    let area = crate::geometry::polygon_area(&q).abs();
    let frame = w * h;
    let area_frac = area / frame;
    let mut reasons = Vec::new();
    let mut forced = None;

    // Interior angles.
    let mut worst_dev: f64 = 0.0;
    for k in 0..4 {
        let (p, c, n) = (q[(k + 3) % 4], q[k], q[(k + 1) % 4]);
        let (a, b) = ((p.0 - c.0, p.1 - c.1), (n.0 - c.0, n.1 - c.1));
        let ang = (a.0 * b.1 - a.1 * b.0)
            .atan2(a.0 * b.0 + a.1 * b.1)
            .abs()
            .to_degrees();
        worst_dev = worst_dev.max((ang - 90.0).abs());
    }
    let angle_score = (1.0 - worst_dev / 40.0).clamp(0.0, 1.0);
    if worst_dev > 45.0 || area_frac < 0.03 {
        forced = Some(Forced::Failed);
        reasons.push(Reason {
            code: ReasonCode::ImplausibleQuad,
            side: None,
        });
    }

    // How completely the region fills its quad.
    let rect = (comp_area as f64 / area.max(1.0)).min(1.0);
    let rect_score = ((rect - 0.6) / 0.35).clamp(0.0, 1.0);

    // Edge support: contrast across each side compared with the page/background contrast.
    let global = f64::from((inside_mean - outside_mean).abs());
    let thr = (global * 0.35).max(8.0);
    const SAMPLES: usize = 40;
    let mut supports = [0.0f64; 4];
    let mut out_of_frame = [0usize; 4];
    for k in 0..4 {
        let (a, b) = (q[k], q[(k + 1) % 4]);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = dx.hypot(dy).max(1e-6);
        let n = (dy / len, -dx / len);
        let mut good = 0;
        let mut counted = 0;
        for s in 0..SAMPLES {
            let t = 0.1 + 0.8 * s as f64 / (SAMPLES - 1) as f64;
            let p = (a.0 + dx * t, a.1 + dy * t);
            let d = 4.0;
            let inside = blurred.bilinear(p.0 - n.0 * d, p.1 - n.1 * d);
            let outside = blurred.bilinear(p.0 + n.0 * d, p.1 + n.1 * d);
            match (inside, outside) {
                (Some(i), Some(o)) => {
                    counted += 1;
                    if f64::from((i - o).abs()) >= thr {
                        good += 1;
                    }
                }
                _ => out_of_frame[k] += 1,
            }
        }
        supports[k] = if counted == 0 || out_of_frame[k] >= SAMPLES / 2 {
            0.0
        } else {
            f64::from(good) / f64::from(counted)
        };
    }
    // One side lying outside the frame is a page cut by the picture: it cannot be judged, and the
    // page-cut flag covers it. Two or more means the quad merely hugs the frame, which proves
    // nothing about a page.
    let blind: Vec<usize> = (0..4).filter(|k| out_of_frame[*k] >= SAMPLES / 2).collect();
    if blind.len() == 1 {
        supports[blind[0]] = 1.0;
    }
    let partial = (0..4).any(|k| out_of_frame[k] >= 8)
        || q.iter()
            .any(|p| p.0 < 1.5 || p.1 < 1.5 || p.0 > w - 2.5 || p.1 > h - 2.5);
    let weakest = supports.iter().cloned().fold(1.0, f64::min);
    let support_score = weakest.clamp(0.0, 1.0);
    if global < 14.0 {
        reasons.push(Reason {
            code: ReasonCode::LowContrastEdge,
            side: None,
        });
        forced.get_or_insert(Forced::Check);
    }
    let mut weak: Vec<usize> = (0..4).filter(|k| supports[*k] < 0.6).collect();
    weak.sort_by(|a, b| {
        supports[*a]
            .partial_cmp(&supports[*b])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for k in weak.into_iter().take(2) {
        reasons.push(Reason {
            code: ReasonCode::WeakEdge,
            side: Some(side_of(k)),
        });
    }
    if partial {
        reasons.push(Reason {
            code: ReasonCode::PartialFrame,
            side: None,
        });
        forced.get_or_insert(Forced::Check);
    }
    if area_frac > 0.97 {
        forced.get_or_insert(Forced::Check);
    }
    let (l1, l2) = (
        (crate::geometry::dist(q[0], q[1]) + crate::geometry::dist(q[3], q[2])) / 2.0,
        (crate::geometry::dist(q[0], q[3]) + crate::geometry::dist(q[1], q[2])) / 2.0,
    );
    if l1.max(l2) / l1.min(l2).max(1.0) > 12.0 {
        reasons.push(Reason {
            code: ReasonCode::OddAspect,
            side: None,
        });
        forced.get_or_insert(Forced::Check);
    }

    let mut score = (0.30 * rect_score + 0.15 * angle_score + 0.55 * support_score) as f32;
    if partial {
        score = score.min(0.85);
    }
    if reasons.is_empty() && score < 0.95 {
        // Never leave a held item without a reason: name the weakest side.
        let k = (0..4)
            .min_by(|a, b| {
                supports[*a]
                    .partial_cmp(&supports[*b])
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or(0);
        reasons.push(Reason {
            code: ReasonCode::WeakEdge,
            side: Some(side_of(k)),
        });
    }
    Candidate {
        quad: q,
        score: score.clamp(0.0, 1.0),
        forced,
        reasons,
    }
}

/// Finds the page quad in `src`.
pub fn detect(src: &Raster) -> Detection {
    let failed = |score: f32| Detection {
        quad: None,
        confidence: Confidence {
            score,
            forced: Some(Forced::Failed),
            reasons: vec![Reason {
                code: ReasonCode::NoQuad,
                side: None,
            }],
        },
    };
    if src.width < 16 || src.height < 16 {
        return failed(0.0);
    }
    let proxy = resize_to_fit(src, PROXY_EDGE);
    let gray = to_gray(&proxy);
    let blurred = gaussian_blur(&gray, 2.0);
    let (w, h) = (blurred.w, blurred.h);
    let thr = otsu(&blurred);
    let min_area = (w * h) / 40;

    let mut best: Option<Candidate> = None;
    for bright in [true, false] {
        let raw: Vec<bool> = blurred.v.iter().map(|v| (*v > thr) == bright).collect();
        // Open (drop specks), then close (fill text and holes).
        let opened = morph(&morph(&raw, w, h, true), w, h, false);
        let mut closed = opened;
        for _ in 0..2 {
            closed = morph(&closed, w, h, false);
        }
        for _ in 0..2 {
            closed = morph(&closed, w, h, true);
        }
        for comp in components(&closed, w, h, min_area, 3) {
            let mut inside_sum = 0.0f64;
            let mut mask = vec![false; w * h];
            for &i in &comp.pixels {
                mask[i] = true;
                inside_sum += f64::from(blurred.v[i]);
            }
            // A region that is (almost) the whole frame has nothing to be told apart from.
            if comp.area * 100 > w * h * 97 {
                continue;
            }
            let outside_n = (w * h - comp.area).max(1);
            let outside_sum: f64 = blurred.v.iter().sum::<f32>() as f64 - inside_sum;
            let inside_mean = (inside_sum / comp.area as f64) as f32;
            let outside_mean = (outside_sum / outside_n as f64) as f32;
            // Boundary pixels: in the component with a 4-neighbour outside it (or on the frame).
            let mut boundary: Vec<P> = Vec::new();
            for &i in &comp.pixels {
                let (x, y) = (i % w, i / w);
                let edge = x == 0
                    || y == 0
                    || x + 1 == w
                    || y + 1 == h
                    || !mask[i - 1]
                    || !mask[i + 1]
                    || !mask[i - w]
                    || !mask[i + w];
                if edge {
                    boundary.push((x as f64 + 0.5, y as f64 + 0.5));
                }
            }
            let hull = convex_hull(boundary);
            let Some(quad) = quad_from_hull(&hull) else {
                continue;
            };
            let cand = evaluate(quad, comp.area, inside_mean, outside_mean, &blurred);
            // Prefer well-supported quads; among equals, the larger one.
            let rank = |c: &Candidate| {
                let area = crate::geometry::polygon_area(&c.quad).abs() / (w * h) as f64;
                f64::from(c.score) * (0.4 + area.min(0.9))
            };
            if best.as_ref().is_none_or(|b| rank(&cand) > rank(b)) {
                best = Some(cand);
            }
        }
    }
    let Some(c) = best else { return failed(0.2) };
    // PLAN 6.2.4: below 0.60 is Failed in every mode, and a Failed item has no crop to apply.
    if c.score < MIN_QUAD_SCORE || c.forced == Some(Forced::Failed) {
        let mut d = failed(c.score);
        if let Some(r) = c
            .reasons
            .iter()
            .find(|r| r.code == ReasonCode::ImplausibleQuad)
        {
            d.confidence.reasons = vec![*r];
        }
        return d;
    }
    let (wf, hf) = (w as f64, h as f64);
    Detection {
        quad: Some(std::array::from_fn(|i| {
            Pt::new(c.quad[i].0 / wf, c.quad[i].1 / hf)
        })),
        confidence: Confidence {
            score: c.score,
            forced: c.forced,
            reasons: c.reasons,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::{PaperKind, Scene, render_scene};

    fn scene(kind: PaperKind, corners: [(f64, f64); 4]) -> Scene {
        Scene {
            width: 960,
            height: 720,
            background: [118, 98, 80],
            paper: [244, 242, 236],
            ink: [60, 64, 76],
            kind,
            corners,
            seed: 11,
            noise: 4.0,
            blur_radius: 1,
            shadow: true,
        }
    }

    /// Mean corner error as a fraction of the image diagonal.
    fn corner_error(found: &[Pt; 4], truth: &[(f64, f64); 4], w: f64, h: f64) -> f64 {
        let diag = w.hypot(h);
        (0..4)
            .map(|i| ((found[i].x - truth[i].0) * w).hypot((found[i].y - truth[i].1) * h))
            .sum::<f64>()
            / 4.0
            / diag
    }

    #[test]
    fn finds_a_tilted_receipt_accurately() {
        let truth = [(0.36, 0.07), (0.63, 0.11), (0.60, 0.94), (0.39, 0.90)];
        let img = render_scene(&scene(PaperKind::Receipt, truth));
        let d = detect(&img);
        let q = d.quad.expect("a quad");
        assert!(corner_error(&q, &truth, 960.0, 720.0) < 0.012, "{q:?}");
        assert!(d.confidence.score > 0.9, "score {}", d.confidence.score);
        assert_eq!(d.confidence.forced, None);
    }

    #[test]
    fn finds_a_document_with_perspective() {
        let truth = [(0.17, 0.12), (0.84, 0.08), (0.88, 0.90), (0.12, 0.93)];
        let img = render_scene(&scene(PaperKind::Document, truth));
        let d = detect(&img);
        let q = d.quad.expect("a quad");
        assert!(corner_error(&q, &truth, 960.0, 720.0) < 0.015, "{q:?}");
        assert!(d.confidence.score > 0.85, "score {}", d.confidence.score);
    }

    #[test]
    fn finds_a_dark_page_on_a_light_desk() {
        let mut s = scene(
            PaperKind::Document,
            [(0.2, 0.15), (0.8, 0.12), (0.82, 0.88), (0.18, 0.9)],
        );
        s.background = [215, 210, 200];
        s.paper = [70, 74, 82];
        s.ink = [200, 205, 210];
        let d = detect(&render_scene(&s));
        let q = d.quad.expect("a quad");
        assert!(corner_error(&q, &s.corners, 960.0, 720.0) < 0.02, "{q:?}");
    }

    #[test]
    fn a_page_cut_by_the_frame_is_flagged() {
        let truth = [(0.30, -0.10), (0.70, -0.08), (0.72, 0.95), (0.28, 0.93)];
        let d = detect(&render_scene(&scene(PaperKind::Receipt, truth)));
        assert!(
            d.confidence
                .reasons
                .iter()
                .any(|r| r.code == ReasonCode::PartialFrame)
        );
        assert_ne!(d.confidence.forced, Some(Forced::Failed));
        assert!(d.confidence.score < 0.9);
    }

    #[test]
    fn a_blank_frame_is_not_a_document() {
        let d = detect(&Raster::filled(320, 240, [120, 118, 116]));
        assert!(d.quad.is_none());
        assert_eq!(d.confidence.forced, Some(Forced::Failed));
        assert_eq!(d.confidence.reasons[0].code, ReasonCode::NoQuad);
    }

    #[test]
    fn low_contrast_scores_lower_than_clean() {
        let truth = [(0.30, 0.10), (0.70, 0.12), (0.68, 0.90), (0.32, 0.88)];
        let clean = detect(&render_scene(&scene(PaperKind::Receipt, truth)));
        let mut s = scene(PaperKind::Receipt, truth);
        s.background = [214, 210, 202];
        s.paper = [232, 229, 222];
        let faint = detect(&render_scene(&s));
        assert!(faint.confidence.score < clean.confidence.score);
    }

    #[test]
    fn tiny_images_do_not_panic() {
        assert!(detect(&Raster::new(4, 4)).quad.is_none());
    }
}
