// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Edge snapping of a candidate rectangle (ROADMAP M10.03, M10.06, M10.11).
//!
//! The mask outline of an item can be a little too big (a soft shadow reaches into the mask) or
//! too small. Each side of the rectangle is therefore moved to the crisp edge nearest to it: the
//! gradient profile is read along the normal at many points, the peak position of each point is
//! collected, and a robust line is fitted through them. The share of points with a crisp peak on
//! the fitted line is the side's edge support; the colour difference across the side is its
//! contrast.

use super::color::{Lab, de76};
use super::geom::{P, area, cross, len, sub};

#[derive(Debug, Clone, Copy, Default)]
pub struct SideStat {
    /// Share of sample points that have a crisp edge on the fitted line (0..1).
    pub support: f32,
    /// Median Lab difference between 3 px outside and 3 px inside the side.
    pub contrast: f32,
}

#[derive(Debug, Clone)]
pub struct Refined {
    pub quad: [P; 4],
    pub sides: [SideStat; 4],
}

impl Refined {
    pub fn support_min(&self) -> f32 {
        self.sides.iter().map(|s| s.support).fold(1.0, f32::min)
    }

    pub fn support_mean(&self) -> f32 {
        self.sides.iter().map(|s| s.support).sum::<f32>() / 4.0
    }

    pub fn contrast_min(&self) -> f32 {
        self.sides
            .iter()
            .map(|s| s.contrast)
            .fold(f32::MAX, f32::min)
    }
}

pub struct Fields<'a> {
    pub w: usize,
    pub h: usize,
    /// Gradient magnitude (sigma 1) of the Lab planes.
    pub g: &'a [f32],
    /// Crisp edge pixels (strong and narrow).
    pub crisp: &'a [bool],
    /// Lab planes blurred at sigma 1.
    pub lab: &'a Lab,
}

fn bilinear(f: &Fields, p: &[f32], x: f64, y: f64) -> f32 {
    // Points are in pixel-corner coordinates (pixel i covers i..i+1); samples live at centres.
    Lab::sample(p, f.w, f.h, x - 0.5, y - 0.5)
}

fn crisp_near(f: &Fields, x: f64, y: f64) -> bool {
    let (cx, cy) = ((x - 0.5).round() as isize, (y - 0.5).round() as isize);
    for dy in -1..=1 {
        for dx in -1..=1 {
            let (nx, ny) = (cx + dx, cy + dy);
            if nx >= 0
                && ny >= 0
                && (nx as usize) < f.w
                && (ny as usize) < f.h
                && f.crisp[ny as usize * f.w + nx as usize]
            {
                return true;
            }
        }
    }
    false
}

/// The brightness step at `p` runs along `line_dir` (across `normal`): the change along the normal
/// is at least twice the change along the line and big enough to matter. Text strokes, which have
/// edges in every direction, fail this over a run; a straight edge passes.
fn aligned(f: &Fields, p: (f64, f64), normal: (f64, f64), line_dir: (f64, f64)) -> bool {
    let at = |d: (f64, f64), k: f64| {
        f64::from(Lab::sample(
            &f.lab.l,
            f.w,
            f.h,
            p.0 + d.0 * k - 0.5,
            p.1 + d.1 * k - 0.5,
        ))
    };
    let dn = (at(normal, 1.5) - at(normal, -1.5)).abs();
    let dt = (at(line_dir, 1.5) - at(line_dir, -1.5)).abs();
    dn >= 2.0 && dn >= 2.0 * dt
}

fn median_f(v: &mut [f64]) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

struct Line {
    p: P,
    d: P,
}

fn intersect(a: &Line, b: &Line) -> Option<P> {
    let den = cross(a.d, b.d);
    if den.abs() < 1e-9 {
        return None;
    }
    let t = cross(sub(b.p, a.p), b.d) / den;
    Some((a.p.0 + a.d.0 * t, a.p.1 + a.d.1 * t))
}

/// One snapping pass with the given search `window` (pixels). `None` when the result is not a
/// plausible quad (the caller then keeps the input).
pub fn snap(quad: &[P; 4], f: &Fields, window: f64) -> Option<Refined> {
    let mut lines: Vec<Line> = Vec::with_capacity(4);
    let mut sides = [SideStat::default(); 4];
    for i in 0..4 {
        let (p0, p1) = (quad[i], quad[(i + 1) % 4]);
        let l = len(sub(p1, p0));
        if l < 6.0 {
            return None;
        }
        let t = ((p1.0 - p0.0) / l, (p1.1 - p0.1) / l);
        let n = (t.1, -t.0);
        let n_samples = ((l * 0.8 / 1.5) as usize).clamp(10, 120);
        let mut pts: Vec<(f64, f64, bool)> = Vec::with_capacity(n_samples); // (s, offset, supported)
        for k in 0..n_samples {
            let s = l * (0.1 + 0.8 * (k as f64 + 0.5) / n_samples as f64);
            let b = (p0.0 + t.0 * s, p0.1 + t.1 * s);
            let (mut best, mut best_d, mut best_v) = (f64::MIN, 0.0f64, 0.0f32);
            let steps = (window * 2.0) as i32;
            for j in 0..=steps {
                let d = -window + j as f64 * 0.5;
                let (x, y) = (b.0 + n.0 * d, b.1 + n.1 * d);
                if x < 0.0 || y < 0.0 || x > (f.w - 1) as f64 || y > (f.h - 1) as f64 {
                    continue;
                }
                let v = bilinear(f, f.g, x, y);
                let score = f64::from(v) * (1.0 - 0.35 * d.abs() / window);
                if score > best {
                    best = score;
                    best_d = d;
                    best_v = v;
                }
            }
            let sup = best_v > 0.0 && crisp_near(f, b.0 + n.0 * best_d, b.1 + n.1 * best_d);
            pts.push((s, best_d, sup));
        }
        let mut sup_off: Vec<f64> = pts.iter().filter(|p| p.2).map(|p| p.1).collect();
        let (a, slope, inl) = if sup_off.len() >= 4 {
            let med = median_f(&mut sup_off);
            let inl: Vec<&(f64, f64, bool)> = pts
                .iter()
                .filter(|p| p.2 && (p.1 - med).abs() <= 1.6)
                .collect();
            if inl.len() >= 4 {
                // least squares d = a + b (s - l/2)
                let n = inl.len() as f64;
                let (mut sx, mut sy, mut sxx, mut sxy) = (0.0, 0.0, 0.0, 0.0);
                for p in &inl {
                    let x = p.0 - l / 2.0;
                    sx += x;
                    sy += p.1;
                    sxx += x * x;
                    sxy += x * p.1;
                }
                let den = n * sxx - sx * sx;
                let b = if den.abs() > 1e-9 {
                    (n * sxy - sx * sy) / den
                } else {
                    0.0
                };
                let b = b.clamp(-0.07, 0.07);
                let a = (sy - b * sx) / n;
                (a, b, inl.len())
            } else {
                (0.0, 0.0, inl.len())
            }
        } else {
            (0.0, 0.0, 0)
        };
        let support = inl as f32 / n_samples as f32;
        let (a, slope) = if support >= 0.3 {
            (a, slope)
        } else {
            (0.0, 0.0)
        };
        let q0 = (
            p0.0 + t.0 * l / 2.0 + n.0 * a,
            p0.1 + t.1 * l / 2.0 + n.1 * a,
        );
        let dir = (t.0 + n.0 * slope, t.1 + n.1 * slope);
        let dl = len(dir);
        lines.push(Line {
            p: q0,
            d: (dir.0 / dl, dir.1 / dl),
        });
        sides[i] = SideStat {
            support,
            contrast: 0.0,
        };
    }
    let mut out = [(0.0, 0.0); 4];
    for i in 0..4 {
        // corner i is where side i-1 meets side i
        out[i] = intersect(&lines[(i + 3) % 4], &lines[i])?;
    }
    // Plausibility: still a convex clockwise quad of similar size.
    let (a0, a1) = (area(quad).abs(), area(&out));
    if a1 <= 0.0 || a1 < 0.4 * a0 || a1 > 2.5 * a0 {
        return None;
    }
    for i in 0..4 {
        let c = cross(
            sub(out[(i + 1) % 4], out[i]),
            sub(out[(i + 2) % 4], out[(i + 1) % 4]),
        );
        if c <= 0.0 {
            return None;
        }
    }
    let mut r = Refined { quad: out, sides };
    measure_contrast(&mut r, f);
    Some(r)
}

/// Fills in the contrast of each side of a quad (median dE between 3 px outside and inside).
pub fn measure_contrast(r: &mut Refined, f: &Fields) {
    for i in 0..4 {
        let (p0, p1) = (r.quad[i], r.quad[(i + 1) % 4]);
        let l = len(sub(p1, p0));
        let t = ((p1.0 - p0.0) / l, (p1.1 - p0.1) / l);
        let n = (t.1, -t.0);
        let n_samples = ((l * 0.8 / 2.0) as usize).clamp(8, 80);
        let mut v = Vec::with_capacity(n_samples);
        for k in 0..n_samples {
            let s = l * (0.1 + 0.8 * (k as f64 + 0.5) / n_samples as f64);
            let b = (p0.0 + t.0 * s, p0.1 + t.1 * s);
            let at = |d: f64| {
                let (x, y) = (b.0 + n.0 * d, b.1 + n.1 * d);
                [
                    Lab::sample(&f.lab.l, f.w, f.h, x - 0.5, y - 0.5),
                    Lab::sample(&f.lab.a, f.w, f.h, x - 0.5, y - 0.5),
                    Lab::sample(&f.lab.b, f.w, f.h, x - 0.5, y - 0.5),
                ]
            };
            v.push(f64::from(de76(at(3.0), at(-3.0))));
        }
        r.sides[i].contrast = median_f(&mut v) as f32;
    }
}

/// Two snapping passes (a wide one, then a tight one) with a final contrast measurement.
pub fn refine(quad: &[P; 4], f: &Fields, window: f64) -> Refined {
    let mut cur = quad.to_vec();
    let mut best: Option<Refined> = None;
    for w in [window, (window * 0.3).max(3.0)] {
        let q = [cur[0], cur[1], cur[2], cur[3]];
        match snap(&q, f, w) {
            Some(r) => {
                cur = r.quad.to_vec();
                best = Some(r);
            }
            None => break,
        }
    }
    best.unwrap_or_else(|| {
        let mut r = Refined {
            quad: *quad,
            sides: [SideStat::default(); 4],
        };
        measure_contrast(&mut r, f);
        r
    })
}

/// What the internal edges of a rectangle say (ROADMAP M10.07, M10.08, M10.09).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Inside {
    /// Nothing that looks like another item.
    Clean,
    /// A long crisp line that starts or ends on the outline of the rectangle (another item lies
    /// on this one, or an abutting item whose edge is the only trace), or a single line across
    /// the whole rectangle (which a card's header band also is): a warning, never a cut.
    Line,
    /// Two parallel crisp lines across the whole rectangle with opposite polarity and a narrow
    /// band between them: the white borders of two prints that touch. Cut in the middle.
    Seam { axis: usize, pos: f64 },
}

fn line_polarity(q: &[P; 4], f: &Fields, axis: usize, s: f64) -> f64 {
    let (along, across) = if axis == 0 {
        (sub(q[1], q[0]), sub(q[3], q[0]))
    } else {
        (sub(q[3], q[0]), sub(q[1], q[0]))
    };
    let la = len(along).max(1.0);
    let step = 2.0 / la;
    let n = (len(across) as usize).max(16);
    let mut acc = 0.0f64;
    for j in 0..n {
        let t = 0.1 + 0.8 * (j as f64 + 0.5) / n as f64;
        let at = |ss: f64| {
            let (x, y) = (
                q[0].0 + along.0 * ss + across.0 * t,
                q[0].1 + along.1 * ss + across.1 * t,
            );
            f64::from(Lab::sample(&f.lab.l, f.w, f.h, x - 0.5, y - 0.5))
        };
        acc += at(s + step) - at(s - step);
    }
    acc / n as f64
}

pub fn inspect_inside(q: &[P; 4], f: &Fields) -> Inside {
    let u = sub(q[1], q[0]);
    let v = sub(q[3], q[0]);
    let (lu, lv) = (len(u), len(v));
    if lu < 40.0 || lv < 40.0 {
        return Inside::Clean;
    }
    let mut verdict = Inside::Clean;
    for axis in 0..2usize {
        let (along, across, n_along, n_across) = if axis == 0 {
            (u, v, lu, lv)
        } else {
            (v, u, lv, lu)
        };
        let steps = (n_along as usize).min(240);
        let samples = (n_across as usize).clamp(16, 256);
        let mut bits = vec![false; samples];
        let an = (along.0 / n_along, along.1 / n_along);
        let cn = (across.0 / n_across, across.1 / n_across);
        let mut full_lines: Vec<f64> = Vec::new();
        let mut any_line = false;
        for k in 0..=steps {
            let s = 0.10 + 0.80 * k as f64 / steps as f64;
            for (j, bit) in bits.iter_mut().enumerate() {
                let t = 0.003 + 0.994 * (j as f64 + 0.5) / samples as f64;
                let x = q[0].0 + along.0 * s + across.0 * t;
                let y = q[0].1 + along.1 * s + across.1 * t;
                *bit = x >= 0.0
                    && y >= 0.0
                    && x <= (f.w - 1) as f64
                    && y <= (f.h - 1) as f64
                    && crisp_near(f, x, y)
                    && aligned(f, (x, y), (an.0, an.1), (cn.0, cn.1));
            }
            // Longest run of aligned crisp samples, bridging a gap of one sample.
            let (mut last_hit, mut cur_start) = (0usize, usize::MAX);
            let mut best_run = (0usize, 0usize);
            for (j, bit) in bits.iter().enumerate() {
                if *bit {
                    if cur_start == usize::MAX || j > last_hit + 2 {
                        cur_start = j;
                    }
                    last_hit = j;
                    if last_hit + 1 - cur_start > best_run.1 + 1 - best_run.0 {
                        best_run = (cur_start, last_hit);
                    }
                }
            }
            let (r0, r1) = best_run;
            let span = (r1 + 1 - r0) as f64 / samples as f64;
            let touches = r0 as f64 <= 0.012 * samples as f64 + 1.0
                || r1 as f64 >= 0.988 * samples as f64 - 2.0;
            if span >= 0.97 {
                full_lines.push(s);
            }
            if span >= 0.55 && touches {
                any_line = true;
            }
        }
        // Group the full-span positions into lines (adjacent positions are one blurred line).
        let mut lines: Vec<f64> = Vec::new();
        let mut group: Vec<f64> = Vec::new();
        for s in full_lines {
            if group.last().is_some_and(|l| (s - l) * n_along > 2.5) {
                lines.push(group.iter().sum::<f64>() / group.len() as f64);
                group.clear();
            }
            group.push(s);
        }
        if !group.is_empty() {
            lines.push(group.iter().sum::<f64>() / group.len() as f64);
        }
        for i in 0..lines.len() {
            for j in i + 1..lines.len() {
                let (s1, s2) = (lines[i], lines[j]);
                let width = (s2 - s1) * n_along;
                if (4.0..=0.16 * n_along).contains(&width)
                    && line_polarity(q, f, axis, s1) * line_polarity(q, f, axis, s2) < 0.0
                {
                    return Inside::Seam {
                        axis,
                        pos: (s1 + s2) / 2.0,
                    };
                }
            }
        }
        if !lines.is_empty() || any_line {
            verdict = Inside::Line;
        }
    }
    verdict
}

/// Cuts a quad along a seam found by [`find_seam`] into two quads.
pub fn cut_quad(q: &[P; 4], axis: usize, s: f64) -> ([P; 4], [P; 4]) {
    let lerp = |a: P, b: P, t: f64| (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
    if axis == 0 {
        let (top, bot) = (lerp(q[0], q[1], s), lerp(q[3], q[2], s));
        ([q[0], top, bot, q[3]], [top, q[1], q[2], bot])
    } else {
        let (left, right) = (lerp(q[0], q[3], s), lerp(q[1], q[2], s));
        ([q[0], q[1], right, left], [left, right, q[2], q[3]])
    }
}

/// The white border of a print on a white bed is the colour of the bed, so the mask stops at the
/// picture. This looks outward from each side for a faint straight step (the print's own edge or
/// the start of its shadow), averaged along the whole side so noise cancels, and accepts it only
/// when the band between the picture and the step is as bright and flat as the bed itself
/// (paper), not darker (a shadow). Needs three sides with similar widths; the fourth takes their
/// median. Returns the grown quad and the mean width in pixels.
pub fn extend_border(q: &[P; 4], f: &Fields, class: &[u8], bed_l: f32) -> Option<([P; 4], f32)> {
    // Only a bed as light as paper can swallow a print's white border; elsewhere the faint steps
    // next to a strong edge are JPEG ringing, not a border.
    if bed_l < 80.0 {
        return None;
    }
    let mut found: [Option<f64>; 4] = [None; 4];
    for i in 0..4 {
        let (p0, p1) = (q[i], q[(i + 1) % 4]);
        let l = len(sub(p1, p0));
        let t = ((p1.0 - p0.0) / l, (p1.1 - p0.1) / l);
        let n = (t.1, -t.0);
        let dmax = (0.14 * l.min(len(sub(q[(i + 2) % 4], q[(i + 1) % 4])))).clamp(4.0, 30.0);
        let ns = ((l * 0.76 / 1.5) as usize).clamp(12, 120);
        let nd = dmax as usize + 3;
        let mut prof = vec![0.0f64; nd + 2];
        let mut bed_frac = vec![0.0f64; nd + 2];
        let mut valid = true;
        for (di, (pv, bf)) in prof.iter_mut().zip(bed_frac.iter_mut()).enumerate() {
            let d = di as f64 - 2.0;
            let (mut sum, mut cnt, mut bed) = (0.0f64, 0usize, 0usize);
            for k in 0..ns {
                let s = l * (0.12 + 0.76 * (k as f64 + 0.5) / ns as f64);
                let (x, y) = (p0.0 + t.0 * s + n.0 * d, p0.1 + t.1 * s + n.1 * d);
                if x < 1.0 || y < 1.0 || x > (f.w - 2) as f64 || y > (f.h - 2) as f64 {
                    continue;
                }
                sum += f64::from(Lab::sample(&f.lab.l, f.w, f.h, x - 0.5, y - 0.5));
                bed += usize::from(class[y as usize * f.w + x as usize] == 2);
                cnt += 1;
            }
            if cnt < ns / 2 {
                valid = false;
                break;
            }
            *pv = sum / cnt as f64;
            *bf = bed as f64 / cnt as f64;
        }
        if !valid {
            continue;
        }
        // |D(d)| = |P(d+1) - P(d-1)| for d from 3 up.
        let mut ds: Vec<(usize, f64)> = Vec::new();
        for di in 5..nd {
            ds.push((di, (prof[di + 1] - prof[di - 1]).abs()));
        }
        if ds.is_empty() {
            continue;
        }
        let mut mags: Vec<f64> = ds.iter().map(|x| x.1).collect();
        mags.sort_by(f64::total_cmp);
        let med = mags[mags.len() / 2];
        let &(pk, val) = ds.iter().max_by(|a, b| a.1.total_cmp(&b.1))?;
        if val < 2.2 || val < 2.5 * med.max(0.2) {
            continue;
        }
        // The band between the picture and the step (offsets 2..pk-1) must be bed-bright, flat.
        let band = &prof[4..pk.saturating_sub(1).max(5)];
        if band.is_empty() {
            continue;
        }
        let mean = band.iter().sum::<f64>() / band.len() as f64;
        let spread = band.iter().fold(f64::MIN, |a, b| a.max(*b))
            - band.iter().fold(f64::MAX, |a, b| a.min(*b));
        let bedness =
            bed_frac[4..pk.saturating_sub(1).max(5)].iter().sum::<f64>() / band.len() as f64;
        if (mean - f64::from(bed_l)).abs() > 7.0 || spread > 6.0 || bedness < 0.7 {
            continue;
        }
        found[i] = Some(pk as f64 - 2.0 + 0.5);
    }
    let have: Vec<f64> = found.iter().flatten().copied().collect();
    if have.len() < 3 {
        return None;
    }
    let mut sorted = have.clone();
    sorted.sort_by(f64::total_cmp);
    let med = sorted[sorted.len() / 2];
    if have.iter().any(|w| *w < 0.4 * med || *w > 2.5 * med) {
        return None;
    }
    let widths: Vec<f64> = found.iter().map(|w| w.unwrap_or(med)).collect();
    let mut lines: Vec<Line> = Vec::with_capacity(4);
    for i in 0..4 {
        let (p0, p1) = (q[i], q[(i + 1) % 4]);
        let l = len(sub(p1, p0));
        let t = ((p1.0 - p0.0) / l, (p1.1 - p0.1) / l);
        let n = (t.1, -t.0);
        lines.push(Line {
            p: (p0.0 + n.0 * widths[i], p0.1 + n.1 * widths[i]),
            d: t,
        });
    }
    let mut out = [(0.0, 0.0); 4];
    for i in 0..4 {
        out[i] = intersect(&lines[(i + 3) % 4], &lines[i])?;
    }
    Some((out, (widths.iter().sum::<f64>() / 4.0) as f32))
}
