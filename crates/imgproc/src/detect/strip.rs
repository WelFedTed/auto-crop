// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Candidates for long thin pages (till receipts, strips): two long, nearly parallel edge lines
//! whose short ends are too faint, too short or cut by the frame to be found as lines of their own.
//!
//! The four-line combinations in `edges::line_quads` need the two short sides as lines, and a short
//! side of a thin receipt has only a few dozen edge points on a textured desk. Here the long sides
//! are enough: between them, the colour of the strip is compared with the colour on the far side of
//! each long edge, all the way along. The page is where that contrast holds; its ends are where the
//! contrast gives out (or where the frame cuts the page off). The ends are confirmed by the same
//! inside/outside colour contrast instead of by an edge line, so a candidate built here is scored
//! on its ends by that contrast and never by edges it does not have.

use super::P;
use super::edges::{Field, RLine};

/// Offset of the flank samples beyond a long edge, in proxy pixels.
const FLANK: f64 = 5.0;
/// Offset of the margin samples inside a long edge, in proxy pixels.
const MARGIN: f64 = 4.0;
/// Offset of the samples across a short end, in proxy pixels.
const END_PROBE: f64 = 3.0;
/// Narrowest strip taken into account, in proxy pixels (the interior needs room for its samples).
const MIN_WIDTH: f64 = 9.0;
/// Contrast (grey levels, strongest channel) below which a strip is not trusted as a page.
const MIN_CONTRAST: f64 = 6.0;
/// Shortest page, as a multiple of its width.
const MIN_ASPECT: f64 = 1.8;
/// Pairs of lines that get a colour profile, best covered first.
const MAX_PAIRS: usize = 16;
/// Longest dip of the contrast bridged inside a page (a dark picture or a stain), in pixels.
const GAP: usize = 4;

/// One short end of a candidate: where it lies and how well the colour contrast confirms it.
#[derive(Clone, Copy)]
pub(super) struct End {
    pub a: P,
    pub b: P,
    /// Share of the samples across the end with inside/outside contrast, or 1 for a cut end.
    pub support: f64,
}

pub(super) struct StripQuad {
    /// Line `a` start, line `a` end, line `b` end, line `b` start (start = low position).
    pub q: [P; 4],
    pub ends: [End; 2],
    /// Median strip contrast (grey levels), the rank key together with the length.
    pub contrast: f64,
}

fn dot(a: P, b: P) -> f64 {
    a.0 * b.0 + a.1 * b.1
}

fn max_abs_diff(a: [f32; 3], b: [f32; 3]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(x, y)| f64::from((x - y).abs()))
        .fold(0.0, f64::max)
}

/// The frame-relative geometry of a pair: positions are `tau` along the mean direction and `u`
/// across it, `P = tau * d + u * n` with `d` and `n` orthonormal.
struct Pair<'a> {
    a: &'a RLine,
    b: &'a RLine,
    d: P,
    n: P,
}

impl Pair<'_> {
    /// Across position of `line` at the along position `tau`.
    fn u_at(&self, line: &RLine, tau: f64) -> f64 {
        let p0 = (line.n.0 * line.rho, line.n.1 * line.rho);
        let ld = if dot(line.d, self.d) < 0.0 {
            (-line.d.0, -line.d.1)
        } else {
            line.d
        };
        let t = (tau - dot(p0, self.d)) / dot(ld, self.d).max(1e-6);
        let p = (p0.0 + t * ld.0, p0.1 + t * ld.1);
        dot(p, self.n)
    }

    fn point(&self, tau: f64, u: f64) -> P {
        (tau * self.d.0 + u * self.n.0, tau * self.d.1 + u * self.n.1)
    }
}

/// Stretches of `line` carrying edge points, as positions along the pair's mean direction.
fn runs_in(pair: &Pair, line: &RLine) -> Vec<(f64, f64)> {
    let p0 = (line.n.0 * line.rho, line.n.1 * line.rho);
    let ld = if dot(line.d, pair.d) < 0.0 {
        (-line.d.0, -line.d.1)
    } else {
        line.d
    };
    line.runs(40.0)
        .into_iter()
        .map(|(t0, t1)| {
            // `t` runs along `line.d`; flip when the pair's direction is the opposite one.
            let (t0, t1) = if dot(line.d, pair.d) < 0.0 {
                (-t1, -t0)
            } else {
                (t0, t1)
            };
            let tau = |t: f64| dot((p0.0 + t * ld.0, p0.1 + t * ld.1), pair.d);
            (tau(t0), tau(t1))
        })
        .collect()
}

/// Strip candidates from pairs of long near-parallel lines, best first.
pub(super) fn strip_quads(lines: &[RLine], f: &Field, keep: usize) -> Vec<StripQuad> {
    let span = (f.w as f64).max(f.h as f64);
    let mut found: Vec<Overlapping> = Vec::new();
    for i in 0..lines.len() {
        for j in i + 1..lines.len() {
            let (a, b) = (&lines[i], &lines[j]);
            let ang = {
                let d = (a.theta - b.theta).abs();
                d.min(std::f64::consts::PI - d)
            };
            if ang > 12f64.to_radians() {
                continue;
            }
            let mut dsum = (a.d.0, a.d.1);
            let sign = if dot(a.d, b.d) < 0.0 { -1.0 } else { 1.0 };
            dsum.0 += sign * b.d.0;
            dsum.1 += sign * b.d.1;
            let len = dsum.0.hypot(dsum.1);
            if len < 1e-6 {
                continue;
            }
            let d = (dsum.0 / len, dsum.1 / len);
            let pair = Pair {
                a,
                b,
                d,
                n: (-d.1, d.0),
            };
            if let Some(ov) = overlap(&pair, span) {
                found.push((ov, i, j, d));
            }
        }
    }
    // The colour profile costs a few thousand samples per pair: only the pairs with the longest
    // shared edge coverage get one.
    found.sort_by(|x, y| {
        (y.0.1 - y.0.0)
            .partial_cmp(&(x.0.1 - x.0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut out: Vec<StripQuad> = Vec::new();
    for (ov, i, j, d) in found.into_iter().take(MAX_PAIRS) {
        let pair = Pair {
            a: &lines[i],
            b: &lines[j],
            d,
            n: (-d.1, d.0),
        };
        if let Some(sq) = one_pair(&pair, f, ov) {
            out.push(sq);
        }
    }
    out.sort_by(|x, y| {
        let key = |s: &StripQuad| crate::geometry::dist(s.q[0], s.q[1]) * s.contrast.min(40.0);
        key(y)
            .partial_cmp(&key(x))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    // Drop near duplicates (the same page found from lines that merely differ a little).
    let mut kept: Vec<StripQuad> = Vec::new();
    for c in out {
        if kept.len() >= keep {
            break;
        }
        let dup = kept.iter().any(|o| {
            (0..4).all(|k| crate::geometry::dist(o.q[k], c.q[k]) < 6.0)
                || (0..4).all(|k| crate::geometry::dist(o.q[k], c.q[(k + 2) % 4]) < 6.0)
        });
        if !dup {
            kept.push(c);
        }
    }
    kept
}

/// A pair of lines with its shared stretch `(lo, hi, width)`, the line indices and mean direction.
type Overlapping = ((f64, f64, f64), usize, usize, P);

/// The stretch where both lines carry edge points, as positions along the pair's direction, and the
/// width of the strip there; `None` when the pair cannot be a long thin page.
fn overlap(pair: &Pair, span: f64) -> Option<(f64, f64, f64)> {
    // Longest covered stretch of each line, and their overlap.
    let (ra, rb) = (runs_in(pair, pair.a), runs_in(pair, pair.b));
    let mut best: Option<(f64, f64)> = None;
    for x in ra.iter().take(3) {
        for y in rb.iter().take(3) {
            let (lo, hi) = (x.0.max(y.0), x.1.min(y.1));
            if hi - lo > best.map_or(0.0, |b| b.1 - b.0) {
                best = Some((lo, hi));
            }
        }
    }
    let (ov_lo, ov_hi) = best?;
    let mid = 0.5 * (ov_lo + ov_hi);
    let width = (pair.u_at(pair.b, mid) - pair.u_at(pair.a, mid)).abs();
    if !(MIN_WIDTH..=0.5 * span).contains(&width) || ov_hi - ov_lo < (1.8 * width).max(30.0) {
        return None;
    }
    Some((ov_lo, ov_hi, width))
}

fn one_pair(pair: &Pair, f: &Field, (ov_lo, ov_hi, width): (f64, f64, f64)) -> Option<StripQuad> {
    let (w, h) = (f.w as f64, f.h as f64);
    let mid = 0.5 * (ov_lo + ov_hi);
    // Positions along the strip's axis that lie inside the frame (with a small margin).
    let uc = |tau: f64| 0.5 * (pair.u_at(pair.a, tau) + pair.u_at(pair.b, tau));
    let inside = |tau: f64| {
        let p = pair.point(tau, uc(tau));
        p.0 >= 2.0 && p.1 >= 2.0 && p.0 <= w - 2.0 && p.1 <= h - 2.0
    };
    // The axis crosses the frame in one stretch; find it by marching from the middle of the overlap.
    if !inside(mid) {
        return None;
    }
    let (mut t_min, mut t_max) = (mid, mid);
    while inside(t_min - 1.0) {
        t_min -= 1.0;
    }
    while inside(t_max + 1.0) {
        t_max += 1.0;
    }
    let n = ((t_max - t_min).floor() as usize) + 1;
    if n < 20 {
        return None;
    }
    // Contrast profile: strip interior against the flanks beyond both long edges.
    let mut prof = vec![f64::NAN; n];
    for (k, pk) in prof.iter_mut().enumerate() {
        let tau = t_min + k as f64;
        let (ua, ub) = (pair.u_at(pair.a, tau), pair.u_at(pair.b, tau));
        let sgn = if ub > ua { 1.0 } else { -1.0 };
        let hw = 0.5 * (ub - ua);
        let c = 0.5 * (ua + ub);
        let sample = |u: f64| f.colour_bilinear(pair.point(tau, u));
        let mut cols = Vec::with_capacity(5);
        for frac in [-0.4, -0.2, 0.0, 0.2, 0.4] {
            if let Some(v) = sample(c + hw * frac) {
                cols.push(v);
            }
        }
        if cols.len() < 3 {
            continue;
        }
        let mut inner = [0.0f32; 3];
        for ch in 0..3 {
            let mut v: Vec<f32> = cols.iter().map(|c| c[ch]).collect();
            v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            inner[ch] = v[v.len() / 2];
        }
        let fa = sample(ua - sgn * FLANK);
        let fb = sample(ub + sgn * FLANK);
        // The inside of each long edge is judged by the centre of the strip or by the margin just
        // inside that edge, whichever differs more from the flank: a dark logo or a dense row of
        // print can match the desk across the middle while the margin is still blank paper.
        let ma = sample(ua + sgn * MARGIN);
        let mb = sample(ub - sgn * MARGIN);
        let side = |flank: Option<[f32; 3]>, margin: Option<[f32; 3]>| {
            flank.map(|f| {
                let m = margin.map_or(0.0, |m| max_abs_diff(m, f));
                max_abs_diff(inner, f).max(m)
            })
        };
        let contrast = match (side(fa, ma), side(fb, mb)) {
            (Some(x), Some(y)) => x.min(y),
            (Some(x), None) | (None, Some(x)) => x,
            (None, None) => continue,
        };
        *pk = contrast;
    }
    // Light smoothing along the strip.
    let sm: Vec<f64> = (0..n)
        .map(|k| {
            let (mut s, mut c) = (0.0, 0.0);
            for v in &prof[k.saturating_sub(1)..=(k + 1).min(n - 1)] {
                if v.is_finite() {
                    s += v;
                    c += 1.0;
                }
            }
            if c > 0.0 { s / c } else { f64::NAN }
        })
        .collect();
    // Typical contrast over the middle of the covered stretch.
    let (lo_i, hi_i) = (
        (((ov_lo + 0.25 * (ov_hi - ov_lo)) - t_min).max(0.0)) as usize,
        (((ov_hi - 0.25 * (ov_hi - ov_lo)) - t_min).max(0.0) as usize).min(n - 1),
    );
    let mut mids: Vec<f64> = sm[lo_i.min(n - 1)..=hi_i.max(lo_i).min(n - 1)]
        .iter()
        .copied()
        .filter(|v| v.is_finite())
        .collect();
    if mids.len() < 8 {
        return None;
    }
    mids.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p_mid = mids[mids.len() / 2];
    if p_mid < MIN_CONTRAST {
        return None;
    }
    let thr = (0.5 * p_mid).max(4.0);
    let good: Vec<bool> = sm.iter().map(|v| v.is_finite() && *v >= thr).collect();
    // Bridge short dips, then take the run holding the middle of the covered stretch.
    let mut filled = good.clone();
    let mut k = 0;
    while k < n {
        if !good[k] {
            let s = k;
            while k < n && !good[k] {
                k += 1;
            }
            if s > 0 && k < n && k - s <= GAP {
                for f in &mut filled[s..k] {
                    *f = true;
                }
            }
        } else {
            k += 1;
        }
    }
    let centre = (((mid - t_min).max(0.0)) as usize).min(n - 1);
    let mut start = centre;
    // Nearest good position to the centre.
    if !filled[start] {
        let mut found = None;
        for r in 1..n {
            if centre + r < n && filled[centre + r] {
                found = Some(centre + r);
                break;
            }
            if centre >= r && filled[centre - r] {
                found = Some(centre - r);
                break;
            }
        }
        start = found?;
    }
    let (mut lo, mut hi) = (start, start);
    while lo > 0 && filled[lo - 1] {
        lo -= 1;
    }
    while hi + 1 < n && filled[hi + 1] {
        hi += 1;
    }
    // Beyond the stretch where both long edges carry edge points the strip has no business going
    // on: past a page's end the colour contrast usually continues into a neighbouring sheet. The
    // frame may still cut a page whose edges run all the way out of it.
    let reach = (0.15 * width).max(5.0);
    let lo_cap = ((ov_lo - reach - t_min).max(0.0)) as usize;
    let hi_cap = (((ov_hi + reach - t_min).max(0.0)) as usize).min(n - 1);
    // A run that reaches the frame is a page cut by it, whatever its edges do near the frame.
    if lo > 0 && lo < lo_cap {
        lo = lo_cap.min(hi);
    }
    if hi < n - 1 && hi > hi_cap {
        hi = hi_cap.max(lo);
    }
    let length = (hi - lo) as f64;
    if length < MIN_ASPECT * width || length < 30.0 {
        return None;
    }
    let cut_lo = lo == 0;
    let cut_hi = hi == n - 1;
    let end_tau = |idx: usize, low: bool| {
        // The contrast falls off between the last good sample and the next one.
        let base = t_min + idx as f64;
        if low { base - 0.5 } else { base + 0.5 }
    };
    let (mut tau_lo, mut tau_hi) = (end_tau(lo, true), end_tau(hi, false));
    // Sharpen each uncut end to where the colour changes most across the strip.
    let across = |tau: f64, fracs: &[f64]| -> Vec<f64> {
        let (ua, ub) = (pair.u_at(pair.a, tau), pair.u_at(pair.b, tau));
        fracs.iter().map(|fr| ua + (ub - ua) * fr).collect()
    };
    let fracs: Vec<f64> = (0..9).map(|k| 0.1 + 0.8 * k as f64 / 8.0).collect();
    let end_contrast = |tau: f64, inward: f64| -> f64 {
        // Mean contrast between the sample `END_PROBE` inside and outside across the strip.
        let (mut s, mut c) = (0.0, 0.0);
        let us = across(tau, &fracs);
        for u in us {
            let i = f.colour_bilinear(pair.point(tau - inward * END_PROBE, u));
            let o = f.colour_bilinear(pair.point(tau + inward * END_PROBE, u));
            if let (Some(i), Some(o)) = (i, o) {
                s += max_abs_diff(i, o);
                c += 1.0;
            }
        }
        if c > 0.0 { s / c } else { 0.0 }
    };
    let refine = |tau: f64, inward: f64| -> f64 {
        let mut best = (end_contrast(tau, inward), tau);
        for dk in -4i32..=4 {
            let t = tau + f64::from(dk);
            let v = end_contrast(t, inward);
            if v > best.0 {
                best = (v, t);
            }
        }
        if best.0 >= 0.8 * thr { best.1 } else { tau }
    };
    if !cut_lo {
        tau_lo = refine(tau_lo, -1.0);
        // `inward` is +1 for the high end: inside lies at smaller tau there.
    }
    if !cut_hi {
        tau_hi = refine(tau_hi, 1.0);
    }
    if tau_hi - tau_lo < MIN_ASPECT * width {
        return None;
    }
    // Corners: on each line at the end positions, or where the line leaves the frame.
    let corner = |line: &RLine, tau: f64, cut: bool, high: bool| -> P {
        let u = pair.u_at(line, tau);
        let p = pair.point(tau, u);
        if !cut {
            return p;
        }
        // Walk along the line towards the cut until it leaves the frame.
        let mut t = tau;
        let step = if high { 0.5 } else { -0.5 };
        let mut last = p;
        for _ in 0..4000 {
            t += step;
            let q = pair.point(t, pair.u_at(line, t));
            if q.0 < 0.0 || q.1 < 0.0 || q.0 > w || q.1 > h {
                break;
            }
            last = q;
        }
        last
    };
    let la = corner(pair.a, tau_lo, cut_lo, false);
    let lb = corner(pair.b, tau_lo, cut_lo, false);
    let ha = corner(pair.a, tau_hi, cut_hi, true);
    let hb = corner(pair.b, tau_hi, cut_hi, true);
    let end_thr = (0.4 * p_mid).max(3.0);
    let support = |tau: f64, inward: f64, cut: bool| -> f64 {
        if cut {
            return 1.0;
        }
        let us = across(tau, &fracs);
        let mut ok = 0usize;
        for u in &us {
            let i = f.colour_bilinear(pair.point(tau - inward * END_PROBE, *u));
            let o = f.colour_bilinear(pair.point(tau + inward * END_PROBE, *u));
            if let (Some(i), Some(o)) = (i, o)
                && max_abs_diff(i, o) >= end_thr
            {
                ok += 1;
            }
        }
        ok as f64 / us.len() as f64
    };
    let ends = [
        End {
            a: la,
            b: lb,
            support: support(tau_lo, -1.0, cut_lo),
        },
        End {
            a: ha,
            b: hb,
            support: support(tau_hi, 1.0, cut_hi),
        },
    ];
    Some(StripQuad {
        q: [la, ha, hb, lb],
        ends,
        contrast: p_mid,
    })
}
