// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! "Trim to content": the picture inside a border of the page's own colour.
//!
//! The classical detector crops the *object* (decision B14: paper edge plus a small margin), so a
//! print on a wide white mat is cropped at the outer edge of the mat and held with
//! `DETECTORS_DISAGREE` (`docs/perf/detector-baseline.md`, "The matted-print question"). This module
//! answers the follow-up the person can ask for: given the page quadrilateral, where is the picture
//! inside it? It PROPOSES a quad; it never changes what the detector returns.
//!
//! The page is rectified to a small working raster (long edge 400 px) and looked at there:
//!
//! 1. **The border colour.** Each of the four sides contributes the median colour of its outermost
//!    rows (the central 60% of each row, so the corners cannot interfere); the colour that has a
//!    uniform run of rows on all four sides is the border (the mat). A page with no such colour (a
//!    full-bleed photograph, a page on a patterned desk) is refused. A little desk beyond the paper
//!    edge (a crop with a margin) is skipped, not mistaken for the border.
//! 2. **The inner edge of each side.** Column by column the border run is followed inwards to the
//!    first place where three rows in a row differ from the border colour; the boundary is placed
//!    where the difference reaches half the contrast (sub-pixel). A robust line (Theil-Sen, then
//!    least squares on the inliers) is fitted through these points per side: the picture may be
//!    rotated a few degrees relative to the page. At least 70% of the columns of every side (85% on
//!    average) must agree with the line within 2 working pixels: the inner outline must be a
//!    straight edge along its length on all four sides.
//! 3. **A picture, not a text block.** Inside the four lines at least 70% of the pixels must differ
//!    from the border colour (a picture fills its rectangle; the text block of a letter does not,
//!    nor does a receipt), the rows must not show the line-by-line rhythm of text, the inner outline
//!    must be at least 20% of the page and have a border of at least 1.2% of the page on every side.
//!
//! Anything else is `None`: refusing is the safe answer, because the proposal only ever helps. The
//! confidence is an UNCALIBRATED heuristic (edge agreement, fill and contrast), like the
//! detector's.

use crate::Raster;
use crate::geometry::{H, apply, homography};
use crate::scale::resize_area;
use auto_crop_core::Pt;

/// Long edge of the rectified working page, in pixels.
const WORK_EDGE: f64 = 400.0;
/// Shortest working side that still says anything.
const MIN_WORK_SIDE: usize = 40;
/// A source whose page is bigger than this many working edges is reduced first (area average).
const REDUCE_ABOVE: f64 = 3.0;
/// Share of the page that may lie outside the picture frame.
const MAX_INVALID: f64 = 0.35;
/// How deep (share of the page) the border colour is looked for on each side.
const PROFILE_DEPTH: f64 = 0.14;
/// A row is "uniform" when its mean deviation from its median colour is at most this.
const UNIFORM_MAD: f32 = 8.0;
/// Border colours tried, best supported first.
const MAX_BORDERS: usize = 3;
/// Rows of one colour that two uniform rows share (largest channel difference).
const SAME_COLOUR: f32 = 16.0;
/// Least run of border rows, as a share of the page and in working pixels.
const MIN_BORDER_SHARE: f64 = 0.012;
const MIN_BORDER_PX: usize = 3;
/// Desk beyond the paper edge that is skipped before the border begins, as a share of the page.
const SPILL_SHARE: f64 = 0.05;
/// The picture's edge must lie within this share of the page from its side.
const MAX_DEPTH_SHARE: f64 = 0.42;
/// Points the robust slope of an edge is taken from.
const SLOPE_POINTS: usize = 64;
/// A column agrees with the fitted edge within this many working pixels.
const EDGE_TOL: f64 = 2.0;
/// Share of the columns that must agree with the edge, in the final pass: on every side, and on
/// average (a picture can be as bright as its mat along a stretch of one edge). The rough first
/// pass only needs half.
const MIN_COVERAGE: f64 = 0.70;
const MEAN_COVERAGE: f64 = 0.85;
const ROUGH_COVERAGE: f64 = 0.50;
/// The steepest edge, as a slope (4 degrees).
const MAX_SLOPE: f64 = 0.0699;
/// Corners of the inner outline are 90 degrees within this many degrees.
const MAX_SKEW_DEG: f64 = 5.0;
/// The picture covers at least this share of the page, and each side at least `MIN_SIDE_SHARE`.
const MIN_AREA_SHARE: f64 = 0.20;
const MIN_SIDE_SHARE: f64 = 0.15;
/// A picture fills its outline: this share of the pixels inside differs from the border colour.
const MIN_FILL: f64 = 0.70;
/// Text-line rhythm: the row profile of the interior correlates with itself above this.
const TEXT_RHYTHM: f32 = 0.45;

/// Why a page was not trimmed. Public so tests and tools can say what happened; the front ends only
/// say "no picture inside to trim to".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrimRefusal {
    /// The quad is degenerate, too small or mostly outside the picture.
    BadQuad,
    /// No colour runs uniformly along all four sides: there is no border.
    NoBorder,
    /// The border has no straight inner edge on all four sides.
    NoEdge,
    /// The inner outline is too small, or the border too thin.
    TooSmall,
    /// The inside is mostly page colour, or has the rhythm of text lines: a block of text.
    TextBlock,
}

/// What kind of border the picture sits in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrimReason {
    /// A wide mat: the border is at least 5% of the page on average.
    Mat,
    /// A thin border (a photographic print's white edge).
    ThinBorder,
}

/// A proposal: the quad of the picture inside the page, in the image's normalised coordinates
/// (TL, TR, BR, BL, like the page quad it came from).
#[derive(Debug, Clone, PartialEq)]
pub struct TrimResult {
    pub quad: [Pt; 4],
    /// 0..1, UNCALIBRATED: edge agreement, fill and contrast. At least 0.5 for any proposal.
    pub confidence: f32,
    pub reason: TrimReason,
}

/// The picture inside `quad` (the page, TL, TR, BR, BL in normalised image coordinates), or `None`
/// when there is no clear border, no picture-like content inside it, or the content is a block of
/// text. Costs a few milliseconds on a 2000 px proxy.
pub fn trim_to_content(src: &Raster, quad: [Pt; 4]) -> Option<TrimResult> {
    trim_analysis(src, quad).ok()
}

/// [`trim_to_content`] with the reason for a refusal.
pub fn trim_analysis(src: &Raster, quad: [Pt; 4]) -> Result<TrimResult, TrimRefusal> {
    if src.width < 16 || src.height < 16 || quad.iter().any(|p| !p.is_finite()) {
        return Err(TrimRefusal::BadQuad);
    }
    let page = Page::rectify(src, quad)?;
    analyse(&page)
}

// ------------------------------------------------------------------------------------ the page

type Rgb = [f32; 3];

fn max_diff(a: Rgb, b: Rgb) -> f32 {
    (a[0] - b[0])
        .abs()
        .max((a[1] - b[1]).abs())
        .max((a[2] - b[2]).abs())
}

/// A grid of colours with a validity mask (a pixel outside the source picture is invalid).
#[derive(Clone)]
struct Grid {
    w: usize,
    h: usize,
    rgb: Vec<Rgb>,
    valid: Vec<bool>,
}

impl Grid {
    /// Quarter turn clockwise: the left side becomes the top.
    fn rot_cw(&self) -> Grid {
        let (w, h) = (self.w, self.h);
        let mut rgb = vec![[0.0; 3]; w * h];
        let mut valid = vec![false; w * h];
        for y in 0..h {
            for x in 0..w {
                let d = x * h + (h - 1 - y);
                rgb[d] = self.rgb[y * w + x];
                valid[d] = self.valid[y * w + x];
            }
        }
        Grid {
            w: h,
            h: w,
            rgb,
            valid,
        }
    }

    /// 3x3 mean over the valid pixels, in place of the colours; invalid pixels stay invalid.
    fn smoothed(&self) -> Grid {
        let (w, h) = (self.w, self.h);
        let mut rgb = self.rgb.clone();
        for y in 0..h {
            for x in 0..w {
                if !self.valid[y * w + x] {
                    continue;
                }
                let (mut acc, mut n) = ([0.0f32; 3], 0.0f32);
                for yy in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                    for xx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                        if self.valid[yy * w + xx] {
                            let c = self.rgb[yy * w + xx];
                            acc[0] += c[0];
                            acc[1] += c[1];
                            acc[2] += c[2];
                            n += 1.0;
                        }
                    }
                }
                rgb[y * w + x] = [acc[0] / n, acc[1] / n, acc[2] / n];
            }
        }
        Grid {
            w,
            h,
            rgb,
            valid: self.valid.clone(),
        }
    }
}

/// The rectified page and the map from its pixels to the normalised image.
struct Page {
    grid: Grid,
    /// Working page (continuous, 0..w x 0..h) to normalised image coordinates.
    to_image: H,
}

impl Page {
    fn rectify(src: &Raster, quad: [Pt; 4]) -> Result<Page, TrimRefusal> {
        let sides = |a: Pt, b: Pt| {
            ((a.x - b.x) * f64::from(src.width)).hypot((a.y - b.y) * f64::from(src.height))
        };
        let pw = 0.5 * (sides(quad[0], quad[1]) + sides(quad[3], quad[2]));
        let ph = 0.5 * (sides(quad[0], quad[3]) + sides(quad[1], quad[2]));
        if !(pw.is_finite() && ph.is_finite()) || pw < 40.0 || ph < 40.0 {
            return Err(TrimRefusal::BadQuad);
        }
        // Convex and clockwise, or the quad is not a page.
        for i in 0..4 {
            let (a, b, c) = (quad[i], quad[(i + 1) % 4], quad[(i + 2) % 4]);
            if (b.x - a.x) * (c.y - b.y) - (b.y - a.y) * (c.x - b.x) <= 0.0 {
                return Err(TrimRefusal::BadQuad);
            }
        }
        let long = pw.max(ph);
        // A big source is reduced first so that the sampling below stays a small area average.
        let reduced;
        let src = if long > REDUCE_ABOVE * WORK_EDGE {
            let s = 2.0 * WORK_EDGE / long;
            reduced = resize_area(
                src,
                ((f64::from(src.width) * s).round() as u32).max(16),
                ((f64::from(src.height) * s).round() as u32).max(16),
            );
            &reduced
        } else {
            src
        };
        let (w, h) = if pw >= ph {
            (WORK_EDGE as usize, (WORK_EDGE * ph / pw).round() as usize)
        } else {
            ((WORK_EDGE * pw / ph).round() as usize, WORK_EDGE as usize)
        };
        if w.min(h) < MIN_WORK_SIDE {
            return Err(TrimRefusal::BadQuad);
        }
        let rect = [
            (0.0, 0.0),
            (w as f64, 0.0),
            (w as f64, h as f64),
            (0.0, h as f64),
        ];
        let norm: [(f64, f64); 4] = std::array::from_fn(|i| (quad[i].x, quad[i].y));
        let to_image = homography(rect, norm).ok_or(TrimRefusal::BadQuad)?;
        let (sw, sh) = (f64::from(src.width), f64::from(src.height));
        let mut rgb = vec![[0.0f32; 3]; w * h];
        let mut valid = vec![false; w * h];
        let mut invalid = 0usize;
        for y in 0..h {
            for x in 0..w {
                let (mut acc, mut n) = ([0.0f32; 3], 0u32);
                for (dx, dy) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                    let Some((u, v)) = apply(&to_image, x as f64 + dx, y as f64 + dy) else {
                        continue;
                    };
                    let (u, v) = (u * sw, v * sh);
                    if u < 0.0 || v < 0.0 || u > sw || v > sh {
                        continue;
                    }
                    let c = bilinear(src, u - 0.5, v - 0.5);
                    acc[0] += c[0];
                    acc[1] += c[1];
                    acc[2] += c[2];
                    n += 1;
                }
                if n >= 3 {
                    let k = n as f32;
                    rgb[y * w + x] = [acc[0] / k, acc[1] / k, acc[2] / k];
                    valid[y * w + x] = true;
                } else {
                    invalid += 1;
                }
            }
        }
        if invalid as f64 > MAX_INVALID * (w * h) as f64 {
            return Err(TrimRefusal::BadQuad);
        }
        Ok(Page {
            grid: Grid { w, h, rgb, valid },
            to_image,
        })
    }
}

fn bilinear(r: &Raster, x: f64, y: f64) -> Rgb {
    let (w, h) = (r.width as i64, r.height as i64);
    let (x0, y0) = (x.floor() as i64, y.floor() as i64);
    let (fx, fy) = ((x - x0 as f64) as f32, (y - y0 as f64) as f32);
    let at = |xx: i64, yy: i64| {
        let p = r.pixel(xx.clamp(0, w - 1) as u32, yy.clamp(0, h - 1) as u32);
        [f32::from(p[0]), f32::from(p[1]), f32::from(p[2])]
    };
    let (a, b, c, d) = (
        at(x0, y0),
        at(x0 + 1, y0),
        at(x0, y0 + 1),
        at(x0 + 1, y0 + 1),
    );
    std::array::from_fn(|i| {
        (a[i] * (1.0 - fx) + b[i] * fx) * (1.0 - fy) + (c[i] * (1.0 - fx) + d[i] * fx) * fy
    })
}

// ------------------------------------------------------------------------------------ frames

/// The four frames in which a side of the page is the top: 0 top, 1 left, 2 bottom, 3 right.
/// A quarter turn clockwise brings the left side to the top.
struct Frames {
    g: [Grid; 4],
    /// Page size.
    w: f64,
    h: f64,
}

impl Frames {
    fn new(page: &Grid) -> Frames {
        let g0 = page.clone();
        let g1 = g0.rot_cw();
        let g2 = g1.rot_cw();
        let g3 = g2.rot_cw();
        Frames {
            g: [g0, g1, g2, g3],
            w: page.w as f64,
            h: page.h as f64,
        }
    }

    /// A point of the page in the frame `k`.
    fn forward(&self, k: usize, p: (f64, f64)) -> (f64, f64) {
        match k {
            0 => p,
            1 => (self.h - p.1, p.0),
            2 => (self.w - p.0, self.h - p.1),
            _ => (p.1, self.w - p.0),
        }
    }

    /// A point of the frame `k` in the page.
    fn back(&self, k: usize, p: (f64, f64)) -> (f64, f64) {
        match k {
            0 => p,
            1 => (p.1, self.h - p.0),
            2 => (self.w - p.0, self.h - p.1),
            _ => (self.w - p.1, p.0),
        }
    }
}

// ------------------------------------------------------------------------------------ the border

struct Border {
    /// The border colour as each frame sees it, and one for the page.
    per_side: [Rgb; 4],
    page: Rgb,
    /// Distance from the border colour that counts as content.
    threshold: f32,
}

fn median(v: &mut [f32]) -> f32 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v[v.len() / 2]
}

fn median_rgb(cols: &[Rgb]) -> Rgb {
    std::array::from_fn(|c| median(&mut cols.iter().map(|p| p[c]).collect::<Vec<_>>()))
}

struct Row {
    side: usize,
    depth: usize,
    colour: Rgb,
    mad: f32,
}

/// The colours that run uniformly along all four sides, best supported first (at most
/// `MAX_BORDERS`). The best supported one is usually the mat, but a plain background of the picture
/// next to a thin mat can be a longer run; the caller tries them in turn.
fn find_borders(fr: &Frames) -> Vec<Border> {
    let mut rows: Vec<Row> = Vec::new();
    for (k, g) in fr.g.iter().enumerate() {
        let depth = ((PROFILE_DEPTH * g.h as f64) as usize).max(MIN_BORDER_PX + 3);
        // The middle 60% of each row: the corners are not part of the test.
        let (xa, xb) = (g.w / 5, g.w - g.w / 5);
        for y in 0..depth.min(g.h) {
            let cols: Vec<Rgb> = (xa..xb)
                .filter(|x| g.valid[y * g.w + x])
                .map(|x| g.rgb[y * g.w + x])
                .collect();
            if cols.len() * 10 < (xb - xa) * 7 {
                continue;
            }
            let colour = median_rgb(&cols);
            let mad = cols.iter().map(|c| max_diff(*c, colour)).sum::<f32>() / cols.len() as f32;
            rows.push(Row {
                side: k,
                depth: y,
                colour,
                mad,
            });
        }
    }
    let min_run = |k: usize| -> usize {
        let dim = fr.g[k].h as f64;
        ((MIN_BORDER_SHARE * dim).ceil() as usize).max(MIN_BORDER_PX)
    };
    let uniform: Vec<&Row> = rows.iter().filter(|r| r.mad <= UNIFORM_MAD).collect();
    let mut clusters: Vec<(Rgb, Vec<&Row>)> = Vec::new();
    for s in &uniform {
        let support: Vec<&Row> = uniform
            .iter()
            .copied()
            .filter(|t| max_diff(t.colour, s.colour) <= SAME_COLOUR)
            .collect();
        // The border begins at the outer edge (after a little desk): a uniform patch of the
        // picture further in is no border.
        let on_all = (0..4).all(|k| {
            let own: Vec<&&Row> = support.iter().filter(|t| t.side == k).collect();
            let spill = (SPILL_SHARE * fr.g[k].h as f64) as usize;
            own.len() >= min_run(k) && own.iter().any(|t| t.depth <= spill)
        });
        if on_all {
            clusters.push((s.colour, support));
        }
    }
    clusters.sort_by_key(|c| std::cmp::Reverse(c.1.len()));
    let mut out: Vec<Border> = Vec::new();
    let mut seen: Vec<Rgb> = Vec::new();
    for (colour, support) in clusters {
        if out.len() >= MAX_BORDERS {
            break;
        }
        if seen.iter().any(|c| max_diff(*c, colour) <= SAME_COLOUR) {
            continue;
        }
        seen.push(colour);
        let page = median_rgb(&support.iter().map(|r| r.colour).collect::<Vec<_>>());
        let per_side = std::array::from_fn(|k| {
            let side: Vec<Rgb> = support
                .iter()
                .filter(|r| r.side == k)
                .map(|r| r.colour)
                .collect();
            median_rgb(&side)
        });
        let mut mads: Vec<f32> = support.iter().map(|r| r.mad).collect();
        let noise = median(&mut mads);
        out.push(Border {
            per_side,
            page,
            threshold: (4.0 * noise).clamp(18.0, 36.0),
        });
    }
    out
}

// ------------------------------------------------------------------------------------ an edge

/// The inner edge of one side, in its frame: `y = a + b x`.
#[derive(Clone, Copy)]
struct Edge {
    a: f64,
    b: f64,
    /// Share of the span's columns that agree with the line.
    coverage: f64,
    /// Mean border width, working pixels.
    width: f64,
    /// Median contrast across the edge.
    contrast: f32,
}

/// Where the border run of column `x` of `g` ends: the sub-pixel boundary and the border's width.
fn column_edge(g: &Grid, border: Rgb, thr: f32, x: usize) -> Option<(f64, f64, f32)> {
    let h = g.h;
    let d = |y: usize| -> f32 {
        let i = y * g.w + x;
        if g.valid[i] {
            max_diff(g.rgb[i], border)
        } else {
            f32::INFINITY
        }
    };
    let spill = (SPILL_SHARE * h as f64) as usize;
    let max_depth = (MAX_DEPTH_SHARE * h as f64) as usize;
    let min_b = ((MIN_BORDER_SHARE * h as f64).ceil() as usize).max(MIN_BORDER_PX);
    // Skip the desk beyond the paper edge: the border starts where three rows in a row have it.
    let start = (0..=spill).find(|&p| p + 2 < h && (p..p + 3).all(|y| d(y) <= thr))?;
    let mut y = start;
    let edge = loop {
        if y + 3 > max_depth.min(h) {
            return None;
        }
        if d(y) > thr && d(y + 1) > thr && d(y + 2) > thr {
            break y;
        }
        y += 1;
    };
    if edge - start < min_b {
        return None;
    }
    // The half-contrast crossing, with the contrast read just inside the edge.
    let mut inside: Vec<f32> = (edge + 1..(edge + 5).min(h)).map(d).collect();
    let c = median(&mut inside);
    if !c.is_finite() {
        return None;
    }
    let half = 0.5 * c;
    let mut p = edge.saturating_sub(2).max(start);
    while p < edge + 2 && d(p) < half {
        p += 1;
    }
    let (d0, d1) = (if p > 0 { d(p - 1) } else { 0.0 }, d(p));
    let frac = if d1 > d0 && d0.is_finite() {
        f64::from((half - d0) / (d1 - d0))
    } else {
        0.5
    };
    let pos = p as f64 - 0.5 + frac.clamp(0.0, 1.0);
    Some((pos, pos - start as f64, c))
}

/// Theil-Sen line through `pts`, then least squares on the points within `EDGE_TOL` of it.
fn robust_line(pts: &[(f64, f64)]) -> Option<(f64, f64, usize)> {
    if pts.len() < 8 {
        return None;
    }
    // The Theil-Sen slope from an evenly thinned set of at most `SLOPE_POINTS` points (the least
    // squares step below uses them all).
    let step = pts.len().div_ceil(SLOPE_POINTS);
    let thin: Vec<&(f64, f64)> = pts.iter().step_by(step).collect();
    let mut slopes = Vec::with_capacity(thin.len() * thin.len() / 2);
    for i in 0..thin.len() {
        for j in i + 1..thin.len() {
            let dx = thin[j].0 - thin[i].0;
            if dx.abs() > 1e-9 {
                slopes.push(((thin[j].1 - thin[i].1) / dx) as f32);
            }
        }
    }
    if slopes.is_empty() {
        return None;
    }
    let mut b = f64::from(median(&mut slopes));
    let mut a = {
        let mut r: Vec<f32> = pts.iter().map(|p| (p.1 - b * p.0) as f32).collect();
        f64::from(median(&mut r))
    };
    for _ in 0..3 {
        let inl: Vec<&(f64, f64)> = pts
            .iter()
            .filter(|p| (p.1 - (a + b * p.0)).abs() <= EDGE_TOL)
            .collect();
        if inl.len() < 8 {
            break;
        }
        let n = inl.len() as f64;
        let (mx, my) = (
            inl.iter().map(|p| p.0).sum::<f64>() / n,
            inl.iter().map(|p| p.1).sum::<f64>() / n,
        );
        let sxx: f64 = inl.iter().map(|p| (p.0 - mx).powi(2)).sum();
        if sxx < 1e-9 {
            break;
        }
        b = inl.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum::<f64>() / sxx;
        a = my - b * mx;
    }
    let inliers = pts
        .iter()
        .filter(|p| (p.1 - (a + b * p.0)).abs() <= EDGE_TOL)
        .count();
    Some((a, b, inliers))
}

/// Fits the inner edge of the side shown at the top of `g`, over the columns `xa..xb`.
fn fit_edge(g: &Grid, border: Rgb, thr: f32, xa: usize, xb: usize) -> Option<Edge> {
    if xb <= xa + 8 {
        return None;
    }
    let cols: Vec<(usize, (f64, f64, f32))> = (xa..xb)
        .filter_map(|x| column_edge(g, border, thr, x).map(|e| (x, e)))
        .collect();
    let pts: Vec<(f64, f64)> = cols.iter().map(|(x, e)| (*x as f64 + 0.5, e.0)).collect();
    let (a, b, inliers) = robust_line(&pts)?;
    let agree: Vec<&(usize, (f64, f64, f32))> = cols
        .iter()
        .filter(|(x, e)| (e.0 - (a + b * (*x as f64 + 0.5))).abs() <= EDGE_TOL)
        .collect();
    let width = agree.iter().map(|(_, e)| e.1).sum::<f64>() / agree.len().max(1) as f64;
    let mut contrast: Vec<f32> = agree.iter().map(|(_, e)| e.2).collect();
    Some(Edge {
        a,
        b,
        coverage: inliers as f64 / (xb - xa) as f64,
        width,
        contrast: if contrast.is_empty() {
            0.0
        } else {
            median(&mut contrast)
        },
    })
}

// ------------------------------------------------------------------------------------ the picture

type P = (f64, f64);

/// A line of the page as a point and a direction.
#[derive(Clone, Copy)]
struct PageLine {
    p: P,
    d: P,
}

fn intersect(a: &PageLine, b: &PageLine) -> Option<P> {
    let det = a.d.0 * b.d.1 - a.d.1 * b.d.0;
    if det.abs() < 1e-9 {
        return None;
    }
    let t = ((b.p.0 - a.p.0) * b.d.1 - (b.p.1 - a.p.1) * b.d.0) / det;
    Some((a.p.0 + a.d.0 * t, a.p.1 + a.d.1 * t))
}

/// The edge `e` of frame `k` as a line of the page.
fn page_line(fr: &Frames, k: usize, e: &Edge) -> PageLine {
    let w = fr.g[k].w as f64;
    let (p0, p1) = (fr.back(k, (0.0, e.a)), fr.back(k, (w, e.a + e.b * w)));
    PageLine {
        p: p0,
        d: (p1.0 - p0.0, p1.1 - p0.1),
    }
}

/// TL, TR, BR, BL of the picture from the edges of the frames top (0), left (1), bottom (2), right (3).
fn corners(fr: &Frames, e: &[Edge; 4]) -> Option<[P; 4]> {
    let l: Vec<PageLine> = (0..4).map(|k| page_line(fr, k, &e[k])).collect();
    let q = [
        intersect(&l[0], &l[1])?,
        intersect(&l[0], &l[3])?,
        intersect(&l[2], &l[3])?,
        intersect(&l[2], &l[1])?,
    ];
    // Right angles, within the skew limit.
    for i in 0..4 {
        let (a, b, c) = (q[(i + 3) % 4], q[i], q[(i + 1) % 4]);
        let (u, v) = ((a.0 - b.0, a.1 - b.1), (c.0 - b.0, c.1 - b.1));
        let cos = (u.0 * v.0 + u.1 * v.1) / (u.0.hypot(u.1) * v.0.hypot(v.1)).max(1e-9);
        if cos.abs() > MAX_SKEW_DEG.to_radians().sin() {
            return None;
        }
    }
    Some(q)
}

fn area(q: &[P; 4]) -> f64 {
    crate::geometry::polygon_area(q).abs()
}

/// The picture inside the page, or why not.
fn analyse(page: &Page) -> Result<TrimResult, TrimRefusal> {
    let fr = Frames::new(&page.grid.smoothed());
    let borders = find_borders(&fr);
    let mut first_error = TrimRefusal::NoBorder;
    for (i, border) in borders.iter().enumerate() {
        match analyse_with(page, &fr, border) {
            Ok(r) => return Ok(r),
            Err(e) if i == 0 => first_error = e,
            Err(_) => {}
        }
    }
    Err(first_error)
}

/// The picture inside the page for one candidate border colour.
fn analyse_with(page: &Page, fr: &Frames, border: &Border) -> Result<TrimResult, TrimRefusal> {
    let thr = border.threshold;
    // Pass 1: the middle half of each side, to learn roughly where the picture is.
    let mut rough = Vec::with_capacity(4);
    for k in 0..4 {
        let w = fr.g[k].w;
        let e = fit_edge(&fr.g[k], border.per_side[k], thr, w / 4, w - w / 4)
            .ok_or(TrimRefusal::NoEdge)?;
        if e.coverage < ROUGH_COVERAGE || e.b.abs() > MAX_SLOPE {
            return Err(TrimRefusal::NoEdge);
        }
        rough.push(e);
    }
    let rough: [Edge; 4] = rough.try_into().map_err(|_| TrimRefusal::NoEdge)?;
    let guess = corners(fr, &rough).ok_or(TrimRefusal::NoEdge)?;
    // Pass 2: the whole length of each edge, up to the corners of the guess.
    let mut fin = Vec::with_capacity(4);
    for k in 0..4 {
        let (g, w) = (&fr.g[k], fr.g[k].w);
        // The two corners that bound side k, in its frame.
        let (c0, c1) = match k {
            0 => (guess[0], guess[1]),
            1 => (guess[3], guess[0]),
            2 => (guess[2], guess[3]),
            _ => (guess[1], guess[2]),
        };
        let (f0, f1) = (fr.forward(k, c0).0, fr.forward(k, c1).0);
        let (lo, hi) = (f0.min(f1), f0.max(f1));
        let trim = (0.02 * w as f64).max(3.0);
        let xa = (lo + trim).max(0.0) as usize;
        let xb = ((hi - trim).max(0.0) as usize).min(w);
        let e = fit_edge(g, border.per_side[k], thr, xa, xb).ok_or(TrimRefusal::NoEdge)?;
        if e.coverage < MIN_COVERAGE || e.b.abs() > MAX_SLOPE {
            return Err(TrimRefusal::NoEdge);
        }
        fin.push(e);
    }
    let fin: [Edge; 4] = fin.try_into().map_err(|_| TrimRefusal::NoEdge)?;
    let q = corners(fr, &fin).ok_or(TrimRefusal::NoEdge)?;
    let (pw, ph) = (fr.w, fr.h);
    // Inside the page, big enough, with a border on every side.
    let inside_page = q
        .iter()
        .all(|p| p.0 > 0.0 && p.1 > 0.0 && p.0 < pw && p.1 < ph);
    let side = |a: P, b: P| (a.0 - b.0).hypot(a.1 - b.1);
    let (iw, ih) = (
        0.5 * (side(q[0], q[1]) + side(q[3], q[2])),
        0.5 * (side(q[0], q[3]) + side(q[1], q[2])),
    );
    if !inside_page
        || area(&q) < MIN_AREA_SHARE * pw * ph
        || iw < MIN_SIDE_SHARE * pw
        || ih < MIN_SIDE_SHARE * ph
        || fin.iter().any(|e| e.width < 2.0)
    {
        return Err(TrimRefusal::TooSmall);
    }
    // A picture, not a block of text.
    let (fill, rhythm) = interior(&fr.g[0], &q, border.page, thr);
    if fill < MIN_FILL || rhythm >= TEXT_RHYTHM {
        return Err(TrimRefusal::TextBlock);
    }
    let cov = fin.iter().map(|e| e.coverage).sum::<f64>() / 4.0;
    if cov < MEAN_COVERAGE {
        return Err(TrimRefusal::NoEdge);
    }
    let contrast = fin.iter().map(|e| e.contrast).fold(f32::INFINITY, f32::min);
    let confidence = (0.5
        + 0.25 * ((cov - MEAN_COVERAGE) / (1.0 - MEAN_COVERAGE)).clamp(0.0, 1.0)
        + 0.15 * ((fill - MIN_FILL) / (1.0 - MIN_FILL)).clamp(0.0, 1.0)
        + 0.10 * f64::from(((contrast - 24.0) / 60.0).clamp(0.0, 1.0)))
    .min(0.95) as f32;
    let mean_border = fin
        .iter()
        .enumerate()
        .map(|(k, e)| e.width / fr.g[k].h as f64)
        .sum::<f64>()
        / 4.0;
    let out: [Pt; 4] = std::array::from_fn(|i| {
        let (x, y) = apply(&page.to_image, q[i].0, q[i].1).unwrap_or((0.0, 0.0));
        Pt::new(x, y)
    });
    Ok(TrimResult {
        quad: out,
        confidence,
        reason: if mean_border >= 0.05 {
            TrimReason::Mat
        } else {
            TrimReason::ThinBorder
        },
    })
}

/// The share of the pixels inside `q` (a little shrunk) that differ from the border colour, and the
/// strongest self-correlation of the row profile of that difference (text-line rhythm).
fn interior(g: &Grid, q: &[P; 4], border: Rgb, thr: f32) -> (f64, f32) {
    const N: usize = 48;
    let lerp = |a: P, b: P, t: f64| (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
    let (mut total, mut differ) = (0usize, 0usize);
    // Row profile: for each of N rows of the quad the share of differing samples.
    let mut profile = [0.0f32; N];
    for (j, pr) in profile.iter_mut().enumerate() {
        let v = 0.03 + 0.94 * (j as f64 + 0.5) / N as f64;
        let (l, r) = (lerp(q[0], q[3], v), lerp(q[1], q[2], v));
        let (mut n, mut m) = (0usize, 0usize);
        for i in 0..N * 2 {
            let u = 0.03 + 0.94 * (i as f64 + 0.5) / (N * 2) as f64;
            let (x, y) = lerp(l, r, u);
            let (xi, yi) = (x as usize, y as usize);
            if x < 0.0 || y < 0.0 || xi >= g.w || yi >= g.h {
                continue;
            }
            let idx = yi * g.w + xi;
            n += 1;
            if !g.valid[idx] || max_diff(g.rgb[idx], border) > thr {
                m += 1;
            }
        }
        total += n;
        differ += m;
        *pr = if n > 0 { m as f32 / n as f32 } else { 0.0 };
    }
    let fill = if total > 0 {
        differ as f64 / total as f64
    } else {
        0.0
    };
    // Line rhythm: a normalised autocorrelation of the mean-removed profile.
    let mean = profile.iter().sum::<f32>() / N as f32;
    let dev: Vec<f32> = profile.iter().map(|p| p - mean).collect();
    let var: f32 = dev.iter().map(|d| d * d).sum();
    let mut peak = 0.0f32;
    if var / N as f32 > 0.01 {
        for lag in 3..N / 3 {
            let c: f32 = (0..N - lag).map(|i| dev[i] * dev[i + lag]).sum::<f32>() / var;
            peak = peak.max(c);
        }
    }
    (fill, peak)
}

#[cfg(test)]
mod tests;
