// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Classical page detector. Two independent sources of candidate quadrilaterals, scored by one
//! rule:
//!
//! * straight edge lines (a colour-fused gradient, thinned, voted into a Hough accumulator,
//!   refined by least squares) combined into four-sided shapes: this finds a page whatever its
//!   brightness relative to the desk, because only the presence of an edge counts and its polarity
//!   may flip along a side (module `edges`);
//! * the best bright or dark region, fitted with four lines: this still yields a page that the
//!   frame cuts off.
//!
//! A candidate is scored by how much of each of its four sides is backed by an aligned edge, with a
//! veto for edges that have paper on the far side (the text block inside a page). The result is held
//! for review, not accepted, when any side is weakly supported, when a bigger well supported quad
//! exists, when the contrast is low or when the page is cut by the frame.
//!
//! Its confidence is an UNCALIBRATED heuristic: it orders results sensibly but its numbers mean
//! nothing as probabilities. Accuracy on synthetic scenes is in `docs/perf/detector-baseline.md`
//! and says nothing about real photographs.

use crate::Raster;
use crate::scale::resize_to_fit;
use auto_crop_core::{Confidence, Forced, Pt, Reason, ReasonCode, Side};

mod edges;
use edges::Field;

/// Below this score the detector reports no quad at all (a Failed item).
const MIN_QUAD_SCORE: f32 = 0.6;

/// Proxy size for detection (long edge).
const PROXY_EDGE: u32 = 640;

/// Blur applied to the colour channels before the gradient is taken, in proxy pixels.
const FIELD_SIGMA: f32 = 1.2;
/// Bounds on the edge threshold, in grey levels per pixel of gradient.
const EDGE_FLOOR: f32 = 1.0;
const EDGE_CEIL: f32 = 6.0;
/// The edge threshold is this many times the median gradient (the texture level) of the picture.
const EDGE_NOISE_GAIN: f32 = 1.5;
/// Straight lines kept from the Hough search.
const MAX_LINES: usize = 64;
/// Line quadrilaterals kept for scoring.
const KEEP_LINE_QUADS: usize = 6;

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
    let radius = (sigma * 3.0).ceil() as usize;
    let mut k: Vec<f32> = (0..=2 * radius)
        .map(|i| {
            let d = i as f32 - radius as f32;
            (-(d * d) / (2.0 * sigma * sigma)).exp()
        })
        .collect();
    let s: f32 = k.iter().sum();
    for v in &mut k {
        *v /= s;
    }
    let (w, h) = (g.w, g.h);
    // Horizontal pass on an edge-replicated copy of each row. Looping over the taps on the outside
    // and the pixels on the inside keeps the inner loop a plain multiply-add over contiguous
    // memory, which the compiler vectorises.
    let mut tmp = vec![0.0f32; w * h];
    let mut pad = vec![0.0f32; w + 2 * radius];
    for y in 0..h {
        let row = &g.v[y * w..(y + 1) * w];
        pad[..radius].fill(row[0]);
        pad[radius..radius + w].copy_from_slice(row);
        pad[radius + w..].fill(row[w - 1]);
        let out = &mut tmp[y * w..(y + 1) * w];
        for (j, kv) in k.iter().enumerate() {
            for (o, v) in out.iter_mut().zip(&pad[j..j + w]) {
                *o += kv * v;
            }
        }
    }
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        let dst = &mut out[y * w..(y + 1) * w];
        for (j, kv) in k.iter().enumerate() {
            let yy = (y + j).saturating_sub(radius).min(h - 1);
            for (o, v) in dst.iter_mut().zip(&tmp[yy * w..(yy + 1) * w]) {
                *o += kv * v;
            }
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
/// The square window is applied as a row pass and a column pass.
fn morph(m: &[bool], w: usize, h: usize, erode: bool) -> Vec<bool> {
    let combine = |a: bool, b: bool| if erode { a & b } else { a | b };
    let mut rows = m.to_vec();
    for y in 0..h {
        let src = &m[y * w..(y + 1) * w];
        let dst = &mut rows[y * w..(y + 1) * w];
        for x in 1..w {
            dst[x] = combine(dst[x], src[x - 1]);
        }
        for x in 0..w.saturating_sub(1) {
            dst[x] = combine(dst[x], src[x + 1]);
        }
    }
    let mut out = rows.clone();
    for y in 0..h {
        for ny in [y.wrapping_sub(1), y + 1] {
            if ny >= h {
                continue;
            }
            let (dst, src) = (&mut out[y * w..(y + 1) * w], &rows[ny * w..(ny + 1) * w]);
            for (d, s) in dst.iter_mut().zip(src) {
                *d = combine(*d, *s);
            }
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
///
/// A page rolled by roughly 45 degrees has two edges that could pass for the top. When the closest
/// one is more than 30 degrees off horizontal and the other is within 60, the page is taken
/// portrait-up (its short edge on top, as receipts and most documents are held). A page that is
/// close to upright keeps the plain closest-to-horizontal rule, so a landscape page stays landscape.
fn orient(mut q: [P; 4]) -> [P; 4] {
    // Clockwise on screen (y down), so the list never mirrors the page.
    if crate::geometry::polygon_area(&q) < 0.0 {
        q.reverse();
    }
    let ang: [f64; 4] = std::array::from_fn(|k| {
        let (a, b) = (q[k], q[(k + 1) % 4]);
        (b.1 - a.1).atan2(b.0 - a.0).abs()
    });
    let mut order = [0usize, 1, 2, 3];
    order.sort_by(|a, b| {
        ang[*a]
            .partial_cmp(&ang[*b])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut start = order[0];
    let (near, other) = (ang[order[0]], ang[order[1]]);
    if near > 30f64.to_radians() && other <= 60f64.to_radians() {
        let k2 = order[1];
        let top = |k: usize| crate::geometry::dist(q[k], q[(k + 1) % 4]);
        let left = |k: usize| crate::geometry::dist(q[k], q[(k + 3) % 4]);
        if top(start) > 1.15 * left(start) && left(k2) > 1.15 * top(k2) {
            start = k2;
        }
    }
    std::array::from_fn(|i| q[(i + start) % 4])
}

struct Candidate {
    quad: [P; 4],
    score: f32,
    forced: Option<Forced>,
    reasons: Vec<Reason>,
    /// Index of the side with the least edge evidence.
    weakest_side: usize,
    /// Mean edge support over the four sides, 0..1.
    mean_support: f64,
}

fn side_of(i: usize) -> Side {
    Side::ALL[i % 4]
}

/// Edge evidence along one side of a candidate quad.
struct SideStat {
    /// Fraction of samples with an aligned edge of consistent polarity within a few pixels, or 0
    /// when the edge demonstrably runs through the paper (see `side_stats`).
    support: f64,
    /// Mean edge strength of the supported samples in units of the edge threshold (capped).
    strength: f64,
    /// Samples that fall on or beyond the frame border and so cannot be judged.
    blind: usize,
}

/// Samples per side when scoring a candidate.
const SAMPLES: usize = 48;
/// How far outside (and inside) a side the page colour is probed, in proxy pixels.
const PROBE_OFFSET: f64 = 6.0;
/// Outside colour within this many grey levels (every channel) of the page colour counts as paper.
const PAPER_MATCH: f32 = 4.0;
/// A side is thrown out when more than this share of its samples is an edge with paper beyond it.
const PAPER_EDGE_SHARE: f64 = 0.3;
/// Mean edge strength (in edge thresholds) below which the contrast is called low.
const LOW_STRENGTH: f64 = 1.8;
/// Every side needs at least this edge support for the result to be eligible for auto-accept.
const GOOD_SIDE_SUPPORT: f64 = 0.9;
/// A different, bigger quad scoring at least this much makes the choice ambiguous.
const RIVAL_SCORE: f32 = 0.75;
/// ... or when at least three of its four sides are solid (a page whose far edge is invisible).
const RIVAL_MEAN_SUPPORT: f64 = 0.7;
/// ... when it is this much bigger than the chosen one.
const RIVAL_AREA_RATIO: f64 = 1.1;

/// The colour of the page: the per-channel median of a thin band just inside the four sides (page
/// margins are blank paper; ink and pictures sit further in).
fn paper_colour(f: &Field, q: &[P; 4]) -> [f32; 3] {
    let mut vals: [Vec<f32>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for k in 0..4 {
        let (a, b) = (q[k], q[(k + 1) % 4]);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = dx.hypot(dy).max(1e-6);
        let inward = (-dy / len, dx / len);
        for s in 0..12 {
            let t = 0.1 + 0.8 * s as f64 / 11.0;
            let p = (
                a.0 + dx * t + inward.0 * PROBE_OFFSET,
                a.1 + dy * t + inward.1 * PROBE_OFFSET,
            );
            if let Some(c) = f.colour(p) {
                for (v, c) in vals.iter_mut().zip(c) {
                    v.push(c);
                }
            }
        }
    }
    vals.map(|mut v| {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        v.get(v.len() / 2).copied().unwrap_or(0.0)
    })
}

/// Edge evidence along the side `a`-`b` (clockwise on screen, so its outward normal is on the
/// left of the direction of travel in y-up terms, `(dy, -dx)` here).
///
/// A sample is supported when a gradient aligned with the side, at least as strong as the edge
/// threshold, lies within two pixels of it; the polarity may change from sample to sample but only
/// the majority polarity counts. The sign of an edge says nothing about which side is paper, so a
/// desk that shades from darker to lighter than the page along one side is not a problem.
///
/// What would be a problem is an edge with paper behind it: a text block, a fold, a picture
/// frame inside the page. The far side of a true page edge is desk, so when the colour a few pixels
/// beyond the side matches the page colour while an edge is present, the sample is contradicting
/// and a side with many of them gets no support at all.
fn side_stats(f: &Field, a: P, b: P, paper: [f32; 3]) -> SideStat {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = dx.hypot(dy).max(1e-6);
    let n = (dy / len, -dx / len);
    let (w, h) = (f.w as f64, f.h as f64);
    let (mut counted, mut blind) = (0usize, 0usize);
    let (mut pos, mut neg, mut contra) = (0usize, 0usize, 0usize);
    let mut strength = 0.0f64;
    for s in 0..SAMPLES {
        let t = 0.05 + 0.9 * s as f64 / (SAMPLES - 1) as f64;
        let p = (a.0 + dx * t, a.1 + dy * t);
        if p.0 < 3.0 || p.1 < 3.0 || p.0 > w - 3.0 || p.1 > h - 3.0 {
            blind += 1;
            continue;
        }
        counted += 1;
        let mut best = 0.0f32;
        let mut signed = 0.0f32;
        for o in -2i32..=2 {
            let q = (p.0 + n.0 * f64::from(o), p.1 + n.1 * f64::from(o));
            if let Some((g, aligned)) = f.along(q, n)
                && aligned
                && g.abs() > best
            {
                best = g.abs();
                signed = g;
            }
        }
        if best < f.thr {
            continue;
        }
        strength += f64::from((best / f.thr).min(4.0));
        if signed > 0.0 {
            pos += 1;
        } else {
            neg += 1;
        }
        let beyond = (p.0 + n.0 * PROBE_OFFSET, p.1 + n.1 * PROBE_OFFSET);
        if f.colour(beyond).is_some_and(|c| {
            c.iter()
                .zip(paper)
                .all(|(c, p)| (c - p).abs() < PAPER_MATCH)
        }) {
            contra += 1;
        }
    }
    let all = pos + neg;
    let contradicted = counted > 0 && contra as f64 > PAPER_EDGE_SHARE * counted as f64;
    SideStat {
        support: if counted == 0 || contradicted {
            0.0
        } else {
            pos.max(neg) as f64 / counted as f64
        },
        strength: if all == 0 { 0.0 } else { strength / all as f64 },
        blind,
    }
}

fn evaluate(quad_raw: [P; 4], f: &Field) -> Candidate {
    let (w, h) = (f.w as f64, f.h as f64);
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

    let paper = paper_colour(f, &q);
    let sides: Vec<SideStat> = (0..4)
        .map(|k| side_stats(f, q[k], q[(k + 1) % 4], paper))
        .collect();
    let mut supports: Vec<f64> = sides.iter().map(|s| s.support).collect();
    // One side lying outside the frame is a page cut by the picture: it cannot be judged, and the
    // page-cut flag covers it. Two or more means the quad merely hugs the frame, which proves
    // nothing about a page.
    let blind: Vec<usize> = (0..4).filter(|k| sides[*k].blind >= SAMPLES / 2).collect();
    if blind.len() == 1 {
        supports[blind[0]] = 1.0;
    }
    let partial = (0..4).any(|k| sides[k].blind >= SAMPLES / 6)
        || q.iter()
            .any(|p| p.0 < 1.5 || p.1 < 1.5 || p.0 > w - 2.5 || p.1 > h - 2.5);
    let weakest_side = (0..4)
        .min_by(|a, b| {
            supports[*a]
                .partial_cmp(&supports[*b])
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(0);
    let weakest = supports[weakest_side];
    let mean_support = supports.iter().sum::<f64>() / 4.0;
    let strength = (0..4)
        .filter(|k| !blind.contains(k))
        .map(|k| sides[k].strength)
        .fold(f64::MAX, f64::min);
    if strength < LOW_STRENGTH {
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

    let mut score = (0.30 * mean_support + 0.15 * angle_score + 0.55 * weakest) as f32;
    if partial {
        score = score.min(0.85);
    }
    // Auto-accept needs every side well backed, not merely a good average.
    if weakest < GOOD_SIDE_SUPPORT {
        forced.get_or_insert(Forced::Check);
    }
    if reasons.is_empty() && (forced.is_some() || score < 0.95) {
        // Never leave a held item without a reason: name the weakest side.
        reasons.push(Reason {
            code: ReasonCode::WeakEdge,
            side: Some(side_of(weakest_side)),
        });
    }
    Candidate {
        quad: q,
        score: score.clamp(0.0, 1.0),
        forced,
        reasons,
        weakest_side,
        mean_support,
    }
}

/// Whether `p` lies inside the quad (clockwise on screen).
fn inside(q: &[P; 4], p: P) -> bool {
    (0..4).all(|k| cross(q[k], q[(k + 1) % 4], p) >= 0.0)
}

/// Distance from `p` to the segment `a`-`b`.
fn dist_to_segment(a: P, b: P, p: P) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = (dx * dx + dy * dy).max(1e-12);
    let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0);
    (p.0 - (a.0 + t * dx)).hypot(p.1 - (a.1 + t * dy))
}

/// When another, bigger and well supported quad exists, the chosen one may be a part of the page
/// (the text block, say) or a distractor beside it: the side of `best` that the rival extends
/// furthest beyond, or `None` when there is no such rival.
fn rival_side(best: &Candidate, others: &[Candidate]) -> Option<usize> {
    let best_area = crate::geometry::polygon_area(&best.quad).abs();
    let rival = others.iter().find(|o| {
        (o.score >= RIVAL_SCORE || o.mean_support >= RIVAL_MEAN_SUPPORT)
            && o.forced != Some(Forced::Failed)
            && crate::geometry::polygon_area(&o.quad).abs() > RIVAL_AREA_RATIO * best_area
    })?;
    // The side whose midpoint lies deepest inside the rival is the one the rival grows past; for a
    // rival elsewhere in the picture fall back on the side with the least evidence.
    let depth = |k: usize| {
        let (a, b) = (best.quad[k], best.quad[(k + 1) % 4]);
        let mid = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
        if inside(&rival.quad, mid) {
            (0..4)
                .map(|j| dist_to_segment(rival.quad[j], rival.quad[(j + 1) % 4], mid))
                .fold(f64::MAX, f64::min)
        } else {
            0.0
        }
    };
    let deepest = (0..4).max_by(|a, b| {
        depth(*a)
            .partial_cmp(&depth(*b))
            .unwrap_or(std::cmp::Ordering::Equal)
    })?;
    Some(if depth(deepest) > 0.0 {
        deepest
    } else {
        best.weakest_side
    })
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
    let field = Field::new(&proxy, FIELD_SIGMA, EDGE_FLOOR, EDGE_CEIL, EDGE_NOISE_GAIN);
    let (w, h) = (field.w, field.h);
    let rank = |c: &Candidate| {
        let area = crate::geometry::polygon_area(&c.quad).abs() / (w * h) as f64;
        f64::from(c.score) * (0.4 + area.min(0.9))
    };

    let mut cands: Vec<Candidate> = Vec::new();
    // Source 1: quadrilaterals made of straight edge lines. Finds a page whatever the brightness
    // relation to the desk, since only the presence of an edge matters.
    let lines = edges::find_lines(&field, MAX_LINES);
    for lq in edges::line_quads(&lines, w as f64, h as f64, KEEP_LINE_QUADS) {
        cands.push(evaluate(lq.q, &field));
    }
    // Source 2: the best bright or dark region, as a convex hull fitted with four lines. Still the
    // way to get a page that is cut by the frame, whose edge along the frame is no edge at all.
    let blurred = gaussian_blur(&to_gray(&proxy), 2.0);
    let thr = otsu(&blurred);
    let min_area = (w * h) / 40;
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
            // A region that is (almost) the whole frame has nothing to be told apart from.
            if comp.area * 100 > w * h * 97 {
                continue;
            }
            let mut mask = vec![false; w * h];
            for &i in &comp.pixels {
                mask[i] = true;
            }
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
            cands.push(evaluate(quad, &field));
        }
    }
    let Some(best_i) = (0..cands.len()).max_by(|a, b| {
        rank(&cands[*a])
            .partial_cmp(&rank(&cands[*b]))
            .unwrap_or(std::cmp::Ordering::Equal)
    }) else {
        return failed(0.2);
    };
    let mut c = cands.swap_remove(best_i);
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
    // Not sure it is the whole page: hold it for review rather than accept it.
    if let Some(k) = rival_side(&c, &cands) {
        c.forced.get_or_insert(Forced::Check);
        c.reasons.push(Reason {
            code: ReasonCode::WeakEdge,
            side: Some(side_of(k)),
        });
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

    /// A desk that is lighter than the paper in one corner and darker in the other, so the sign of
    /// the page edge flips along the page (the case a brightness threshold cannot handle).
    fn low_contrast_scene(kind: PaperKind, corners: [(f64, f64); 4], seed: u64) -> Scene {
        Scene {
            background: [222, 219, 212],
            paper: [240, 238, 232],
            shadow: false,
            seed,
            noise: 3.0,
            ..scene(kind, corners)
        }
    }

    #[test]
    fn finds_a_low_contrast_page_that_a_brightness_threshold_cannot() {
        let truth = [(0.25, 0.14), (0.74, 0.10), (0.78, 0.88), (0.22, 0.91)];
        let img = render_scene(&low_contrast_scene(PaperKind::Document, truth, 5));
        let d = detect(&img);
        let q = d.quad.expect("a quad");
        assert!(corner_error(&q, &truth, 960.0, 720.0) < 0.012, "{q:?}");
    }

    /// Whatever it finds, a result that would be auto-accepted (no forced band, score at the
    /// Balanced cutoff) must be right. Unsure results may be wrong; they are held for review.
    #[test]
    fn low_contrast_results_are_either_right_or_held() {
        let layouts: [[(f64, f64); 4]; 4] = [
            [(0.25, 0.14), (0.74, 0.10), (0.78, 0.88), (0.22, 0.91)],
            [(0.10, 0.20), (0.62, 0.12), (0.66, 0.80), (0.14, 0.90)],
            [(0.36, 0.07), (0.63, 0.11), (0.60, 0.94), (0.39, 0.90)],
            [(0.40, 0.10), (0.92, 0.18), (0.80, 0.86), (0.30, 0.80)],
        ];
        let mut found = 0;
        for (i, corners) in layouts.iter().enumerate() {
            for kind in [PaperKind::Document, PaperKind::Receipt] {
                let d = detect(&render_scene(&low_contrast_scene(
                    kind,
                    *corners,
                    20 + i as u64,
                )));
                let accepted = d.confidence.forced.is_none() && d.confidence.score >= 0.9;
                match d.quad {
                    Some(q) if corner_error(&q, corners, 960.0, 720.0) < 0.015 => found += 1,
                    Some(q) => assert!(!accepted, "silent failure {q:?} for {corners:?}"),
                    None => {}
                }
            }
        }
        assert!(found >= 6, "only {found} of 8 low-contrast pages found");
    }

    /// The top edge of the page is invisible (the desk above it is the colour of the paper), but the
    /// first rule of the text is not: the detector must not hand back the text block as the page.
    #[test]
    fn a_text_block_is_not_accepted_as_the_page() {
        let truth = [(0.20, 0.20), (0.80, 0.20), (0.80, 0.90), (0.20, 0.90)];
        let mut s = scene(PaperKind::Document, truth);
        s.background = [200, 196, 190];
        s.paper = [240, 238, 232];
        s.shadow = false;
        let mut img = render_scene(&s);
        let top = (0.20 * 720.0) as u32 - 1;
        for y in 0..top {
            for x in 0..img.width {
                img.set_pixel(x, y, [240, 238, 232]);
            }
        }
        let d = detect(&img);
        let accepted = d.confidence.forced.is_none() && d.confidence.score >= 0.9;
        if let Some(q) = d.quad
            && accepted
        {
            assert!(corner_error(&q, &truth, 960.0, 720.0) < 0.02, "{q:?}");
        }
    }

    #[test]
    fn a_paper_white_distractor_next_to_a_page_does_not_change_the_answer() {
        let truth = [(0.12, 0.10), (0.55, 0.12), (0.57, 0.90), (0.10, 0.88)];
        let mut img = render_scene(&scene(PaperKind::Document, truth));
        // A small paper-white rectangle on the desk, away from the page.
        for y in 120..200 {
            for x in 760..900 {
                img.set_pixel(x, y, [246, 244, 238]);
            }
        }
        let d = detect(&img);
        let q = d.quad.expect("a quad");
        assert!(corner_error(&q, &truth, 960.0, 720.0) < 0.015, "{q:?}");
    }

    fn cand(quad: [P; 4], score: f32, weakest_side: usize) -> Candidate {
        Candidate {
            quad,
            score,
            forced: None,
            reasons: Vec::new(),
            weakest_side,
            mean_support: f64::from(score),
        }
    }

    #[test]
    fn a_bigger_well_supported_quad_makes_the_choice_ambiguous_on_the_side_it_grows_past() {
        // Clockwise on screen. The chosen quad is the lower part of the rival: it is the top side
        // that the rival extends past.
        let rival = [(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)];
        let best = [(0.0, 40.0), (100.0, 40.0), (100.0, 100.0), (0.0, 100.0)];
        let r = cand(rival, 0.9, 0);
        assert_eq!(rival_side(&cand(best, 0.95, 2), &[r]), Some(0));
        // A weak rival, or one that is not bigger, is no rival.
        assert_eq!(
            rival_side(&cand(best, 0.95, 2), &[cand(rival, 0.5, 0)]),
            None
        );
        assert_eq!(
            rival_side(&cand(rival, 0.95, 2), &[cand(best, 0.9, 0)]),
            None
        );
        // A disjoint bigger rival falls back on the weakest side of the chosen quad.
        let far = [(200.0, 0.0), (400.0, 0.0), (400.0, 200.0), (200.0, 200.0)];
        assert_eq!(
            rival_side(&cand(best, 0.95, 3), &[cand(far, 0.9, 0)]),
            Some(3)
        );
    }

    #[test]
    fn a_page_rolled_far_is_taken_portrait_up_and_a_near_upright_one_keeps_its_orientation() {
        let rot = |w: f64, h: f64, deg: f64, start: usize| -> [P; 4] {
            let (s, c) = deg.to_radians().sin_cos();
            let base = [
                (-w / 2.0, -h / 2.0),
                (w / 2.0, -h / 2.0),
                (w / 2.0, h / 2.0),
                (-w / 2.0, h / 2.0),
            ];
            let moved: Vec<P> = base
                .iter()
                .map(|p| (500.0 + p.0 * c - p.1 * s, 400.0 + p.0 * s + p.1 * c))
                .collect();
            std::array::from_fn(|i| moved[(i + start) % 4])
        };
        let top_len = |q: &[P; 4]| crate::geometry::dist(q[0], q[1]);
        let left_len = |q: &[P; 4]| crate::geometry::dist(q[0], q[3]);
        for start in 0..4 {
            // Portrait page rolled 40 degrees either way: the short edge is on top.
            for deg in [40.0, -40.0, 44.0] {
                let q = orient(rot(100.0, 300.0, deg, start));
                assert!(top_len(&q) < left_len(&q), "start {start} roll {deg}");
            }
            // Landscape page, nearly upright: the long edge stays on top.
            let q = orient(rot(300.0, 100.0, 5.0, start));
            assert!(top_len(&q) > left_len(&q), "start {start}");
            // Square-ish pages: closest to horizontal wins.
            let q = orient(rot(100.0, 105.0, 20.0, start));
            let ang = (q[1].1 - q[0].1).atan2(q[1].0 - q[0].0).to_degrees();
            assert!((ang - 20.0).abs() < 1.0, "angle {ang}");
        }
    }

    #[test]
    fn the_quad_is_always_listed_clockwise() {
        let truth = [(0.30, 0.10), (0.70, 0.12), (0.68, 0.90), (0.32, 0.88)];
        let q = detect(&render_scene(&scene(PaperKind::Receipt, truth)))
            .quad
            .expect("a quad");
        let px: Vec<P> = q.iter().map(|p| (p.x, p.y)).collect();
        assert!(crate::geometry::polygon_area(&px) > 0.0);
    }

    #[test]
    fn degenerate_pictures_never_panic() {
        // Noise, a checkerboard, a flat black square, hairline strips and a single pixel.
        let mut rng = crate::synth::Rng::new(3);
        let mut noise = Raster::new(97, 61);
        for p in noise.data.iter_mut() {
            *p = rng.below(256) as u8;
        }
        let mut checker = Raster::new(200, 150);
        for y in 0..150 {
            for x in 0..200 {
                let v = if (x / 3 + y / 3) % 2 == 0 { 20 } else { 235 };
                checker.set_pixel(x, y, [v, v, v]);
            }
        }
        for img in [
            noise,
            checker,
            Raster::filled(16, 16, [0, 0, 0]),
            Raster::filled(4000, 20, [200, 200, 200]),
            Raster::filled(20, 4000, [255, 255, 255]),
            Raster::new(1, 1),
        ] {
            let d = detect(&img);
            if d.quad.is_none() {
                assert_eq!(d.confidence.forced, Some(Forced::Failed));
            }
        }
    }

    /// The straightforward implementation the vectorised blur replaced.
    fn blur_reference(g: &Gray, sigma: f32) -> Gray {
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

    #[test]
    fn the_blur_matches_the_plain_implementation_including_borders() {
        let mut rng = crate::synth::Rng::new(9);
        for (w, h, sigma) in [
            (37usize, 23usize, 1.2f32),
            (5, 40, 2.0),
            (3, 3, 2.0),
            (64, 1, 1.2),
        ] {
            let g = Gray {
                w,
                h,
                v: (0..w * h).map(|_| rng.unit() * 255.0).collect(),
            };
            let (a, b) = (gaussian_blur(&g, sigma), blur_reference(&g, sigma));
            for (x, y) in a.v.iter().zip(&b.v) {
                assert!((x - y).abs() < 1e-3, "{x} vs {y} at {w}x{h}");
            }
        }
    }

    #[test]
    fn the_morphology_matches_the_plain_3x3_window() {
        let mut rng = crate::synth::Rng::new(4);
        let (w, h) = (23usize, 17usize);
        let m: Vec<bool> = (0..w * h).map(|_| rng.unit() < 0.6).collect();
        for erode in [true, false] {
            let fast = morph(&m, w, h, erode);
            for y in 0..h {
                for x in 0..w {
                    let (mut all, mut any) = (true, false);
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
                    assert_eq!(fast[y * w + x], if erode { all } else { any }, "{x},{y}");
                }
            }
        }
    }
}
