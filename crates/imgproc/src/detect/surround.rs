// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! What lies around the chosen quad, judged by colour, after the candidates are ranked.
//!
//! Two situations in which an edge-backed quad is still not what the user wants, both seen on real
//! photographs and neither visible to the edge evidence:
//!
//! * **A patch of a bigger page** ([`surround`]): the quad (a barcode box, a picture) is a darker
//!   block surrounded on all sides by one colour that fills a region several times larger than the
//!   quad. The page continues around it: the quad is never accepted. When the surround is light, the
//!   region reaches every side of the frame and more content (text lines, logos) lies outside the
//!   quad, nothing in the picture is desk at all: the page fills the frame.
//! * **A picture inside a border** ([`inner_picture`]): a second quad with a different content
//!   colour lies inside the chosen one, with the chosen quad's paper colour all around it (a print
//!   on a wide white mat). Paper edge and picture are both plausible outlines; which one the user
//!   wants is a policy question the detector does not decide, so it holds the answer.
//!
//! The thresholds are derived on the synthetic reproductions in `real_cases` and checked on the
//! synthetic suites (`docs/perf/detector-baseline.md`); none was tuned on the private real photographs.

use super::{
    CHROMA_FAR, Candidate, Field, Forced, P, PROBE_OFFSET, chroma, chroma_gap, inside, median3,
    overlap_share, paper_colour,
};

/// A colour counts as the surround colour within this many grey levels in every channel.
const SAME_COLOUR: f32 = 14.0;
/// Share of the ring around the quad that must have the surround colour for the surround to be one
/// uniform colour.
const RING_UNIFORM: f64 = 0.75;
/// The quad's mean luminance must be this much below the surround's: a dark block on a lighter page
/// (a barcode, which the 640 px proxy renders as a grey block, or a print). A quad that is made of
/// its surround's colour (a text block, a page of the desk's own colour) was tried too: it held
/// about 70 correct results of 5,200 for 3 fewer silent failures, so it is not used.
const DARKER_BY: f32 = 40.0;
/// The quad covers less than this share of (itself + the connected region of the surround colour
/// outside it) when it is a patch of a bigger page.
const COVER_MAX: f64 = 0.5;
/// The page fills the frame only when the quad covers even less than this, ...
const FRAME_COVER_MAX: f64 = 0.25;
/// ... and is at most this share of the frame, ...
const FRAME_QUAD_MAX: f64 = 0.3;
/// ... the surround is light (luminance), ...
const LIGHT_SURROUND: f32 = 140.0;
/// ... the region reaches at least this share of every side of the frame, ...
const FRAME_SIDE_SHARE: f64 = 0.9;
/// ... and outside the quad there are at least this many separate small non-surround blobs (text
/// lines, logos), together covering at least `ISLANDS_AREA` of the frame, each under `ISLAND_MAX`.
const ISLANDS_MIN: usize = 8;
const ISLANDS_AREA: f64 = 0.02;
const ISLAND_MAX: f64 = 0.02;
/// A rival strip's colour must differ from the desk's by this many grey levels of luminance ...
const STANDS_OUT_LUMA: f32 = 35.0;
/// An inner picture needs the paper colour of its own band to differ from the chosen quad's by this
/// many grey levels of luminance, or by a chromaticity gap, ...
const PICTURE_LUMA_GAP: f32 = 25.0;
/// ... a score and an area share relative to the chosen quad ...
const PICTURE_MIN_SCORE: f32 = 0.85;
const PICTURE_MIN_AREA: f64 = 0.35;
const PICTURE_MAX_AREA: f64 = 0.97;
/// ... and the ring just outside it to have the chosen quad's paper colour (the mat).
const PICTURE_RING_SHARE: f64 = 0.75;

fn near(a: [f32; 3], b: [f32; 3], tol: f32) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() <= tol)
}

fn luma(c: [f32; 3]) -> f32 {
    0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]
}

/// The outward unit normal of side `k` of the clockwise quad `q`.
fn normal(q: &[P; 4], k: usize) -> P {
    let (a, b) = (q[k], q[(k + 1) % 4]);
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = dx.hypot(dy).max(1e-6);
    (dy / len, -dx / len)
}

/// Points along each side of `q`, `per_side` of them, pushed outward by each distance in `dists`.
fn ring_points(q: &[P; 4], dists: &[f64], per_side: usize) -> Vec<P> {
    let mut out = Vec::new();
    for k in 0..4 {
        let (a, b) = (q[k], q[(k + 1) % 4]);
        let n = normal(q, k);
        for s in 0..per_side {
            let t = 0.05 + 0.9 * s as f64 / (per_side - 1) as f64;
            for d in dists {
                out.push((
                    a.0 + (b.0 - a.0) * t + n.0 * d,
                    a.1 + (b.1 - a.1) * t + n.1 * d,
                ));
            }
        }
    }
    out
}

/// The colours at the ring points that fall inside the frame, and how many were tried.
fn ring_colours(f: &Field, q: &[P; 4], dists: &[f64], per_side: usize) -> (Vec<[f32; 3]>, usize) {
    let pts = ring_points(q, dists, per_side);
    let cols: Vec<[f32; 3]> = pts.iter().filter_map(|p| f.colour(*p)).collect();
    (cols, pts.len())
}

/// A quad that is a patch of a bigger page (see the module docs).
pub(super) struct Surround {
    /// Nothing in the picture is desk: the page fills the frame.
    pub fills_frame: bool,
}

/// `Some` when `q` is a darker block inside a uniform surround region that is several times its size
/// and holds more content (text lines, logos) outside it. `None` for everything else: a surround that
/// is not one uniform colour (a page on a textured desk), a quad that is not darker than its
/// surround, too little of the ring inside the frame to judge, or no content around it.
pub(super) fn surround(f: &Field, q: &[P; 4]) -> Option<Surround> {
    let (ring_cols, tried) = ring_colours(f, q, &[PROBE_OFFSET, 2.0 * PROBE_OFFSET], 24);
    if ring_cols.len() * 2 < tried {
        return None;
    }
    let c_out = median3(std::array::from_fn(|c| {
        ring_cols.iter().map(|p| p[c]).collect()
    }));
    let same = ring_cols
        .iter()
        .filter(|p| near(**p, c_out, SAME_COLOUR))
        .count();
    if (same as f64) < RING_UNIFORM * ring_cols.len() as f64 {
        return None;
    }
    // The quad's own content: a grid of points through its interior.
    let (mut total, mut luma_sum) = (0usize, 0.0f32);
    for i in 0..24 {
        for j in 0..24 {
            let (u, v) = ((i as f64 + 0.5) / 24.0, (j as f64 + 0.5) / 24.0);
            let top = (
                q[0].0 + (q[1].0 - q[0].0) * u,
                q[0].1 + (q[1].1 - q[0].1) * u,
            );
            let bot = (
                q[3].0 + (q[2].0 - q[3].0) * u,
                q[3].1 + (q[2].1 - q[3].1) * u,
            );
            let p = (top.0 + (bot.0 - top.0) * v, top.1 + (bot.1 - top.1) * v);
            if let Some(c) = f.colour(p) {
                total += 1;
                luma_sum += luma(c);
            }
        }
    }
    if total == 0 {
        return None;
    }
    let darker = luma(c_out) - luma_sum / total as f32 >= DARKER_BY;
    if !darker {
        return None;
    }
    // Flood the surround colour from the ring points that have it, over the whole proxy.
    let (w, h) = (f.w, f.h);
    let matches = |i: usize| {
        near(
            [f.rgb[i * 3], f.rgb[i * 3 + 1], f.rgb[i * 3 + 2]],
            c_out,
            SAME_COLOUR,
        )
    };
    let mut seen = vec![false; w * h];
    let mut stack: Vec<usize> = Vec::new();
    for p in ring_points(q, &[PROBE_OFFSET], 24) {
        if p.0 >= 0.0 && p.1 >= 0.0 && p.0 < w as f64 && p.1 < h as f64 {
            let i = p.1 as usize * w + p.0 as usize;
            if matches(i) && !seen[i] {
                seen[i] = true;
                stack.push(i);
            }
        }
    }
    let mut outside_region = 0usize;
    while let Some(i) = stack.pop() {
        let (x, y) = (i % w, i / w);
        if !inside(q, (x as f64 + 0.5, y as f64 + 0.5)) {
            outside_region += 1;
        }
        let mut push = |j: usize| {
            if !seen[j] && matches(j) {
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
    let frame = (w * h) as f64;
    let quad_area = crate::geometry::polygon_area(q).abs();
    let coverage = quad_area / (quad_area + outside_region as f64);
    if coverage >= COVER_MAX {
        return None;
    }
    // The page's own content (text lines, logos) lies outside the quad, in the surround region.
    let (n_islands, island_area) = islands(&seen, w, h, q);
    if n_islands < ISLANDS_MIN || (island_area as f64) < ISLANDS_AREA * frame {
        return None;
    }
    let mut fills_frame = coverage < FRAME_COVER_MAX
        && quad_area <= FRAME_QUAD_MAX * frame
        && luma(c_out) >= LIGHT_SURROUND;
    if fills_frame {
        let share = |ix: &dyn Fn(usize) -> usize, n: usize| {
            (0..n).filter(|k| seen[ix(*k)]).count() as f64 / n as f64
        };
        let (m_y, m_x) = (2.min(h - 1), 2.min(w - 1));
        let sides = [
            share(&|x| m_y * w + x, w),
            share(&|y| y * w + (w - 1 - m_x), h),
            share(&|x| (h - 1 - m_y) * w + x, w),
            share(&|y| y * w + m_x, h),
        ];
        fills_frame = sides.iter().all(|s| *s >= FRAME_SIDE_SHARE);
    }
    Some(Surround { fills_frame })
}

/// Number and total area of the small blobs of non-surround pixels outside the quad.
fn islands(seen: &[bool], w: usize, h: usize, q: &[P; 4]) -> (usize, usize) {
    let max_area = (ISLAND_MAX * (w * h) as f64) as usize;
    let mut done = vec![false; w * h];
    let (mut count, mut total) = (0usize, 0usize);
    let mut stack = Vec::new();
    for start in 0..w * h {
        if seen[start] || done[start] {
            continue;
        }
        if inside(q, ((start % w) as f64 + 0.5, (start / w) as f64 + 0.5)) {
            continue;
        }
        let mut area = 0usize;
        done[start] = true;
        stack.push(start);
        while let Some(i) = stack.pop() {
            area += 1;
            let (x, y) = (i % w, i / w);
            let mut push = |j: usize| {
                if !seen[j] && !done[j] {
                    done[j] = true;
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
        if area <= max_area {
            count += 1;
            total += area;
        }
    }
    (count, total)
}

/// Whether the page colour of `other` stands clear of the colour around `best` (the desk): by
/// `STANDS_OUT_LUMA` grey levels of luminance or by a chromaticity gap. A long thin "page" that is
/// the desk's own colour (two grain lines of a wood desk, a seam) is no rival of a quad.
pub(super) fn stands_out_of_surround(f: &Field, best: &Candidate, other: &Candidate) -> bool {
    let (ring_cols, _) = ring_colours(f, &best.quad, &[PROBE_OFFSET, 2.0 * PROBE_OFFSET], 24);
    if ring_cols.is_empty() {
        return true;
    }
    let desk = median3(std::array::from_fn(|c| {
        ring_cols.iter().map(|p| p[c]).collect()
    }));
    let own = paper_colour(f, &other.quad);
    (luma(own) - luma(desk)).abs() >= STANDS_OUT_LUMA
        || match (chroma(own), chroma(desk)) {
            (Some(a), Some(b)) => chroma_gap(a, b) > CHROMA_FAR,
            _ => false,
        }
}

/// Another quad inside `best`, with different content, surrounded by `best`'s paper colour: a
/// picture inside a border (the mat of a print). Returns the picture's quad.
pub(super) fn inner_picture(f: &Field, best: &Candidate, others: &[Candidate]) -> Option<[P; 4]> {
    let best_area = crate::geometry::polygon_area(&best.quad).abs();
    let outer_paper = paper_colour(f, &best.quad);
    others
        .iter()
        .filter(|o| {
            let a = crate::geometry::polygon_area(&o.quad).abs() / best_area.max(1e-9);
            !o.from_strip
                && o.score >= PICTURE_MIN_SCORE
                && o.forced != Some(Forced::Failed)
                && (PICTURE_MIN_AREA..=PICTURE_MAX_AREA).contains(&a)
                && overlap_share(&o.quad, &best.quad) >= 0.95
        })
        .find(|o| {
            let own = paper_colour(f, &o.quad);
            let differs = (luma(own) - luma(outer_paper)).abs() > PICTURE_LUMA_GAP
                || match (chroma(own), chroma(outer_paper)) {
                    (Some(a), Some(b)) => chroma_gap(a, b) > CHROMA_FAR,
                    _ => false,
                };
            if !differs {
                return false;
            }
            let (ring_cols, _) = ring_colours(f, &o.quad, &[PROBE_OFFSET], 24);
            let mat = ring_cols
                .iter()
                .filter(|c| near(**c, outer_paper, SAME_COLOUR * 1.5))
                .count();
            !ring_cols.is_empty() && mat as f64 >= PICTURE_RING_SHARE * ring_cols.len() as f64
        })
        .map(|o| o.quad)
}
