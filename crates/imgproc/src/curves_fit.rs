// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Automatic curve suggestion (`docs/dev/curved-pages.md`, "Suggestion API"): given the picture and
//! a straight quad, fit the four boundary curves of a curved page. Classical, weight-free, offline.
//!
//! For each edge of the quad, 48 to 64 stations are laid along the chord. At each station a profile
//! of the colour along the chord's normal is taken (three points one pixel apart along the edge
//! averaged, to tame noise) and every candidate offset `d` inside the search window (about 6% of
//! the shorter side each way, more for a long edge, never beyond the frame) gets an **edge response**
//!
//! ```text
//! E(d) = colour_distance(mean(d-3..d-1), mean(d+1..d+3)) * near_side_is_paper(d) * far_side_is_not_paper(d)
//! ```
//!
//! The colour distance is Euclidean over the three channels (a white receipt on a coloured desk is
//! an edge even when the grey levels match). "Paper" is a model taken from INSIDE the quad: the
//! bright end (pale paper) or the dark end (dark paper) of a band deeper than the window reaches,
//! pooled over 17 neighbouring stations, so dense text, a title bar or a shadow across the page do
//! not become the paper; a local colour too far from the page's dominant one to be shading is
//! refused. The polarity rule keeps the text block of a page from passing as its border: the near
//! side of a boundary must look like paper, and the colours 6 and 11 pixels beyond it must not (the
//! far side of a true page edge is desk; the far side of a printed line is paper again).
//!
//! A Viterbi pass over the stations then picks ONE offset per station with a smoothness penalty
//! (slope squared) and a weak pull towards the chord. If many stations hit the window's edge the
//! window is re-centred on the result and the search repeated (4 passes at most), which widens the
//! reach to several windows for a strongly bowed edge. Each station gets a confidence from its
//! contrast above the picture's own texture level; runs of low confidence (a finger, a hard shadow,
//! a place where the desk matches the paper) are bridged by interpolation, and a robust polynomial
//! trend pulls isolated detours back. How much better the fitted path is backed by edge evidence
//! than the best straight line is the **support gain**.
//!
//! The corners are then refined: the tangent lines at the two fitted ends meeting at a corner are
//! intersected, and the corner moves there if that is within 4% of the shorter side. The fitted
//! polylines are simplified (Ramer-Douglas-Peucker, tolerance 0.4% of the shorter side, then points
//! are added where the Catmull-Rom spline through them strays) to 2 to 9 control points per edge;
//! an edge that is not bent by more than the fit's noise, or no better supported than a straight
//! line, is left straight. The result is validated with [`CurveWarp::validate`], and anything
//! non-finite, self-crossing or far from the quad falls back to the straight quad.
//!
//! **Curvature** of an edge is the largest distance of its fitted curve from its chord, in percent
//! of the chord length. `looks_curved` says some edge passes every gate `LOOKS_CURVED_*` (confidence,
//! length, deviation in pixels, support gain, curvature); the constants come from the dev split of
//! synthetic data (see `docs/perf/curve-fit-baseline.md`, which also says how little that proves).
//! The suggestion is never applied by itself.

use crate::Raster;
use crate::scale::resize_to_fit;
use auto_crop_core::{Curve, CurveWarp, Pt};

/// Long edge of the picture the fit works on (the detector's proxy size). A larger picture is
/// reduced first.
pub const PROXY_EDGE: u32 = 640;
/// Fewest and most stations laid along an edge.
const MIN_STATIONS: usize = 48;
const MAX_STATIONS: usize = 64;
/// Half width of the search window, as a share of the shorter side of the quad.
const WINDOW_FRAC: f64 = 0.06;
/// ... or this share of the edge's own length if that is more (long edges of a bowed receipt),
const LONG_EDGE_FRAC: f64 = 0.035;
/// ... but at most this share of the shorter side.
const MAX_WINDOW_FRAC: f64 = 0.25;
/// Samples averaged on each side of a candidate boundary.
const SIDE: i32 = 3;
/// Offsets (beyond a candidate) probed for the "outside is not paper" rule.
const FAR_A: i32 = 6;
const FAR_B: i32 = 11;
/// Depth range of the paper reference, measured from the chord, beyond the window.
const REF_FROM: i32 = 5;
const REF_TO: i32 = 14;
/// Unary cost: contrast is saturated at `E_CAP` grey levels and scaled by `E_SCALE`.
const E_CAP: f32 = 40.0;
const E_SCALE: f32 = 20.0;
/// Weak pull of the boundary towards the chord (per squared window width).
const PRIOR: f32 = 0.15;
/// How much a paper-like colour 6 or 11 px beyond a candidate boundary discounts it.
const VETO: f32 = 0.97;
/// Smoothness: cost per squared slope between neighbouring stations.
const SMOOTH: f32 = 6.0;
/// A candidate without a usable profile (outside the frame) costs this much.
const BLIND_COST: f32 = 0.6;
/// Stations below this confidence are bridged; an edge with fewer than `MIN_RELIABLE` of them
/// reliable is left straight.
const RELIABLE: f32 = 0.3;
const MIN_RELIABLE: f64 = 0.25;
/// Contrast (grey levels above the texture floor) that counts as fully confident.
const CONF_SPAN: f32 = 13.0;
/// Corners may move by this share of the shorter side.
const CORNER_MOVE: f64 = 0.04;
/// Simplification tolerance, share of the shorter side.
const SIMPLIFY_TOL: f64 = 0.004;
/// Most control points on an edge (ends included).
const MAX_CONTROL: usize = 9;
/// An edge is left straight when its fitted bend is under this many pixels of the chord ...
const KEEP_MIN_DEV_PX: f64 = 2.0;
/// ... or its evidence gain over the best straight line is under this.
const KEEP_MIN_GAIN: f32 = 0.15;
/// An edge whose curve strays more than this share of the shorter side from its chord is refused.
const MAX_BULGE: f64 = 0.35;

/// `looks_curved` thresholds. An edge counts when ALL of these hold; the page looks curved when
/// the largest curvature (percent of the chord) of the counting edges reaches
/// [`LOOKS_CURVED_MIN_PCT`]. Chosen on the dev split of the synthetic suites with a margin: the
/// flat dev pages had no false positive from a gain of 0.3 up (1 in 174 at 0.25, 3 in 174 at 0.2), and a flat sample of the engine's own set sat at 0.307, so 0.35 and confidence 0.7 leave a margin; see
/// `docs/perf/curve-fit-baseline.md`.
pub const LOOKS_CURVED_MIN_PCT: f32 = 1.0;
/// Edge confidence (mean station confidence).
pub const LOOKS_CURVED_MIN_CONF: f32 = 0.7;
/// Edge length in picture pixels (of the 640 px working picture): a short end of a long strip is
/// resolution-limited and noisy in percent of its own length.
pub const LOOKS_CURVED_MIN_LEN_PX: f64 = 80.0;
/// Largest distance from the chord, in picture pixels: below it the bend is within the fit's noise.
pub const LOOKS_CURVED_MIN_DEV_PX: f64 = 3.0;
/// How much better the curve must be backed by edge evidence than the best straight line
/// (`CurveFit::support_gain`). This is the gate that keeps flat pages flat.
pub const LOOKS_CURVED_MIN_GAIN: f32 = 0.35;

/// Stations pooled either side for the paper reference, and the brightness quantile taken from
/// the pooled band (the bright end for pale paper, the dark end for dark paper).
const POOL: usize = 8;
const PAPER_QUANTILE: f32 = 0.9;

/// Range of the ratio of a local paper reference's brightness to the page's own.
const SHADOW_RATIO: (f32, f32) = (0.38, 1.8);

/// Passes of the search (the window is re-centred between them).
const PASSES: usize = 4;

/// Edge names in the order of every array here: top, right, bottom, left.
pub const EDGE_NAMES: [&str; 4] = ["top", "right", "bottom", "left"];

/// The fitted page: refined corners, the four curves and how far to trust them.
#[derive(Debug, Clone, PartialEq)]
pub struct CurveFit {
    /// TL, TR, BR, BL in normalised coordinates (the input quad's corners, possibly moved).
    pub quad: [Pt; 4],
    /// The fitted boundary curves; their end points are `quad`. Straight edges when `fell_back`.
    pub curves: CurveWarp,
    /// Per edge (top, right, bottom, left) confidence 0..1: the mean station confidence.
    pub confidence: [f32; 4],
    /// Per edge the largest deviation of the fitted curve from its chord, percent of the chord.
    pub curvature: [f32; 4],
    /// Per edge the same deviation as a percentage of the quad's SHORTER side (a short edge's
    /// curvature is noisy in percent of its own length; this is the size of the bend on the page).
    pub bulge: [f32; 4],
    /// Per edge: how much better the fitted curve is backed by edge evidence than the best
    /// straight line (0 = no better; about 1 = a full-contrast edge's worth at every station).
    pub support_gain: [f32; 4],
    /// The largest curvature among edges with at least [`LOOKS_CURVED_MIN_CONF`] confidence (0 if none).
    pub max_curvature: f32,
    /// Some confident edge reaches [`LOOKS_CURVED_MIN_PCT`]: the page looks curved.
    pub looks_curved: bool,
    /// The fit was refused (bad input or an invalid result): `curves` are the input quad's straight
    /// edges, the confidences and curvatures are zero.
    pub fell_back: bool,
}

impl CurveFit {
    fn straight(quad: [Pt; 4]) -> Self {
        let curves = CurveWarp {
            top: Curve::straight(quad[0], quad[1]),
            right: Curve::straight(quad[1], quad[2]),
            bottom: Curve::straight(quad[2], quad[3]),
            left: Curve::straight(quad[3], quad[0]),
            quarter_turns: 0,
            mirror: false,
        };
        Self {
            quad,
            curves,
            confidence: [0.0; 4],
            curvature: [0.0; 4],
            bulge: [0.0; 4],
            support_gain: [0.0; 4],
            max_curvature: 0.0,
            looks_curved: false,
            fell_back: true,
        }
    }
}

type V = (f64, f64);
type Rgb = [f32; 3];

fn dist(a: V, b: V) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

/// Fits the four boundary curves of the page whose straight outline is `quad` (TL, TR, BR, BL,
/// normalised to `image`). Never panics and never fails: a result that cannot be trusted is the
/// straight quad with `fell_back` set.
pub fn fit_curves(image: &Raster, quad: [Pt; 4]) -> CurveFit {
    if image.width < 16
        || image.height < 16
        || quad.iter().any(|p| !p.is_finite())
        || image.data.len() != image.width as usize * image.height as usize * 3
    {
        return CurveFit::straight(quad);
    }
    let shrunk;
    let img = if image.width.max(image.height) > PROXY_EDGE {
        shrunk = resize_to_fit(image, PROXY_EDGE);
        &shrunk
    } else {
        image
    };
    fit_on_proxy(img, quad).unwrap_or_else(|| CurveFit::straight(quad))
}

struct Frame<'a> {
    w: usize,
    h: usize,
    data: &'a [u8],
}

impl Frame<'_> {
    /// Bilinear colour at `p` (pixel edge coordinates), `None` when the neighbourhood leaves the
    /// frame.
    fn at(&self, p: V) -> Option<Rgb> {
        let (x, y) = (p.0 - 0.5, p.1 - 0.5);
        if !(x >= 0.0 && y >= 0.0 && x <= (self.w - 1) as f64 && y <= (self.h - 1) as f64) {
            return None;
        }
        let (x0, y0) = (x as usize, y as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (fx, fy) = ((x - x0 as f64) as f32, (y - y0 as f64) as f32);
        let px = |xx: usize, yy: usize, c: usize| f32::from(self.data[(yy * self.w + xx) * 3 + c]);
        let mut out = [0.0f32; 3];
        for (c, o) in out.iter_mut().enumerate() {
            let top = px(x0, y0, c) * (1.0 - fx) + px(x1, y0, c) * fx;
            let bot = px(x0, y1, c) * (1.0 - fx) + px(x1, y1, c) * fx;
            *o = top * (1.0 - fy) + bot * fy;
        }
        Some(out)
    }
}

/// Distance between two colours in grey levels (Euclidean over the channels, divided by sqrt 3 so
/// a uniform change of `g` grey levels reads `g`).
fn cdist(a: Rgb, b: Rgb) -> f32 {
    let s: f32 = a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum();
    (s / 3.0).sqrt()
}

fn lum(c: Rgb) -> f32 {
    (c[0] + c[1] + c[2]) / 3.0
}

fn mean_rgb(v: &[Rgb]) -> Option<Rgb> {
    if v.is_empty() {
        return None;
    }
    let n = v.len() as f32;
    let mut s = [0.0f32; 3];
    for c in v {
        for i in 0..3 {
            s[i] += c[i];
        }
    }
    Some([s[0] / n, s[1] / n, s[2] / n])
}

fn median(v: &mut [f32]) -> f32 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v[v.len() / 2]
}

/// The dominant colour of `v`: the median of the samples in the fullest brightness band (twelve
/// bands of 21 grey levels, a band counted with its two neighbours). Unlike the median it names the
/// paper even when ink covers 40% of the page, light on dark or dark on light.
fn mode_rgb(v: &[Rgb]) -> Option<Rgb> {
    if v.is_empty() {
        return None;
    }
    let band = |c: &Rgb| ((lum(*c) / 21.34) as usize).min(11);
    let mut count = [0usize; 12];
    for c in v {
        count[band(c)] += 1;
    }
    let best = (0..12)
        .max_by_key(|b| count[*b] + count[b.saturating_sub(1)] + count[(*b + 1).min(11)])
        .unwrap_or(0);
    let chosen: Vec<Rgb> = v.iter().filter(|c| band(c) == best).copied().collect();
    median_rgb(&chosen)
}

/// The sample at the q quantile of brightness (a real colour of , not a per-channel mix).
fn percentile_rgb(v: &[Rgb], q: f32) -> Option<Rgb> {
    if v.is_empty() {
        return None;
    }
    let mut s: Vec<Rgb> = v.to_vec();
    s.sort_by(|a, b| {
        lum(*a)
            .partial_cmp(&lum(*b))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Some(s[((s.len() - 1) as f32 * q).round() as usize])
}

fn median_rgb(v: &[Rgb]) -> Option<Rgb> {
    if v.is_empty() {
        return None;
    }
    let mut out = [0.0f32; 3];
    for (c, o) in out.iter_mut().enumerate() {
        let mut ch: Vec<f32> = v.iter().map(|p| p[c]).collect();
        *o = median(&mut ch);
    }
    Some(out)
}

/// One side of the quad as a chord with its stations.
struct Chord {
    a: V,
    dir: V,
    /// Unit normal pointing out of the page.
    out: V,
    len: f64,
    /// Station spacing in pixels.
    step: f64,
    n: usize,
    /// Half width of the search window in pixels.
    win: i32,
    /// Depth range (pixels inside the chord) of the paper reference band.
    reference: (i32, i32),
}

impl Chord {
    fn base(&self, k: usize) -> V {
        let t = k as f64 / (self.n - 1) as f64 * self.len;
        (self.a.0 + self.dir.0 * t, self.a.1 + self.dir.1 * t)
    }

    fn point(&self, k: usize, d: f64) -> V {
        let b = self.base(k);
        (b.0 + self.out.0 * d, b.1 + self.out.1 * d)
    }
}

/// Colour profile along the normal at station `k` for offsets `lo..=hi` (integers, pixels from the
/// chord), each averaged over three points one pixel apart along the edge.
fn profile(frame: &Frame, c: &Chord, k: usize, lo: i32, hi: i32) -> Vec<Option<Rgb>> {
    let b = c.base(k);
    (lo..=hi)
        .map(|o| {
            let mut acc = [0.0f32; 3];
            for lat in [-1.0f64, 0.0, 1.0] {
                let p = (
                    b.0 + c.out.0 * f64::from(o) + c.dir.0 * lat,
                    b.1 + c.out.1 * f64::from(o) + c.dir.1 * lat,
                );
                let v = frame.at(p)?;
                for i in 0..3 {
                    acc[i] += v[i] / 3.0;
                }
            }
            Some(acc)
        })
        .collect()
}

/// What one pass of the search found along one edge.
struct Pass {
    /// Chosen offset per station (pixels from the chord, sub-pixel).
    d: Vec<f64>,
    /// Edge response at the chosen offset.
    e: Vec<f32>,
    /// Best response at least 4 px away from the chosen one.
    alt: Vec<f32>,
    /// All responses of the pass, for the picture's texture level.
    all: Vec<f32>,
    /// The responses per station and the centre of each station's window.
    resp: Vec<Vec<f32>>,
    centres: Vec<i32>,
}

/// The local paper colour at every station: per-channel median of a thin band deeper inside than
/// the window reaches, then a median over seven neighbouring stations.
fn paper_reference(frame: &Frame, c: &Chord, global: Rgb, light: bool) -> Vec<Rgb> {
    let bands: Vec<Vec<Rgb>> = (0..c.n)
        .map(|k| {
            profile(frame, c, k, -c.reference.1, -c.reference.0)
                .into_iter()
                .flatten()
                .collect()
        })
        .collect();
    (0..c.n)
        .map(|k| {
            let pooled: Vec<Rgb> = bands[k.saturating_sub(POOL)..=(k + POOL).min(c.n - 1)]
                .iter()
                .flatten()
                .copied()
                .collect();
            let q = if light {
                PAPER_QUANTILE
            } else {
                1.0 - PAPER_QUANTILE
            };
            // Shading dims the paper by up to about 2.6 and rarely lights it by more than 1.8
            // against the page's dominant colour; ink (a dark title bar on pale paper, a pale one
            // on dark paper) goes beyond that.
            percentile_rgb(&pooled, q)
                .filter(|m| {
                    let ratio = lum(*m).max(1.0) / lum(global).max(1.0);
                    (SHADOW_RATIO.0..=SHADOW_RATIO.1).contains(&ratio)
                })
                .unwrap_or(global)
        })
        .collect()
}

/// The edge responses of station `k` for the window `centre - win ..= centre + win`.
fn responses(
    frame: &Frame,
    c: &Chord,
    k: usize,
    centre: i32,
    win: i32,
    paper: Rgb,
) -> (Vec<f32>, Vec<bool>) {
    let lo = centre - win - SIDE;
    let hi = centre + win + FAR_B + 1;
    let prof = profile(frame, c, k, lo, hi);
    let at = |o: i32| prof[(o - lo) as usize];
    let l = lum(paper);
    let tau_in = 10.0 + 0.10 * l;
    let tau_far = 5.0 + 0.03 * l;
    let mut e = Vec::with_capacity((2 * win + 1) as usize);
    let mut ok = Vec::with_capacity((2 * win + 1) as usize);
    for d in centre - win..=centre + win {
        let ins: Vec<Rgb> = (d - SIDE..d).filter_map(at).collect();
        let outs: Vec<Rgb> = (d + 1..=d + SIDE).filter_map(at).collect();
        if ins.len() < SIDE as usize || outs.len() < SIDE as usize {
            e.push(0.0);
            ok.push(false);
            continue;
        }
        let (mi, mo) = (
            mean_rgb(&ins).unwrap_or(paper),
            mean_rgb(&outs).unwrap_or(paper),
        );
        let delta = cdist(mi, mo);
        let w_in = (-(cdist(mi, paper) / tau_in).powi(2)).exp();
        let far = |from: i32| {
            let v: Vec<Rgb> = (from - 1..=from + 1).filter_map(at).collect();
            mean_rgb(&v).map_or(0.0, |m| (-(cdist(m, paper) / tau_far).powi(2)).exp())
        };
        let veto = 1.0 - VETO * far(d + FAR_A).max(far(d + FAR_B));
        e.push(delta * w_in * veto);
        ok.push(true);
    }
    (e, ok)
}

/// Viterbi over the stations. `centres[k]` is the middle of the window of station `k`.
fn search(frame: &Frame, c: &Chord, centres: &[i32], win: i32, refs: &[Rgb]) -> Pass {
    let s = (2 * win + 1) as usize;
    let mut resp: Vec<Vec<f32>> = Vec::with_capacity(c.n);
    let mut unary: Vec<Vec<f32>> = Vec::with_capacity(c.n);
    let wf = f64::from(win).max(1.0) as f32;
    for k in 0..c.n {
        let (e, ok) = responses(frame, c, k, centres[k], win, refs[k]);
        let u = (0..s)
            .map(|i| {
                let d = (centres[k] - win + i as i32) as f32;
                let base = PRIOR * (d / wf) * (d / wf);
                if ok[i] {
                    base - e[i].min(E_CAP) / E_SCALE
                } else {
                    base + BLIND_COST
                }
            })
            .collect();
        unary.push(u);
        resp.push(e);
    }
    // Slope limit between neighbouring stations: 1.0 (45 degrees) at least 3 px.
    let step = c.step.max(1.0);
    let dmax = step.max(3.0).ceil() as i32;
    let lam = SMOOTH / (step * step) as f32;
    let mut cost: Vec<Vec<f32>> = vec![unary[0].clone()];
    let mut back: Vec<Vec<u16>> = vec![vec![0; s]];
    for k in 1..c.n {
        let mut row = vec![f32::MAX; s];
        let mut bp = vec![0u16; s];
        let shift = centres[k] - centres[k - 1];
        for i in 0..s {
            // Neighbours of state `i` one station back: absolute offsets within `dmax` of it.
            for delta in -dmax..=dmax {
                let j = i as i32 + shift - delta;
                if j < 0 || j >= s as i32 {
                    continue;
                }
                let j = j as usize;
                let v = cost[k - 1][j] + lam * (delta * delta) as f32;
                if v < row[i] {
                    row[i] = v;
                    bp[i] = j as u16;
                }
            }
            if row[i] == f32::MAX {
                row[i] = 1e6;
            }
            row[i] += unary[k][i];
        }
        cost.push(row);
        back.push(bp);
    }
    let last = &cost[c.n - 1];
    let mut idx = (0..s)
        .min_by(|a, b| {
            last[*a]
                .partial_cmp(&last[*b])
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(0);
    let mut chosen = vec![0usize; c.n];
    for k in (0..c.n).rev() {
        chosen[k] = idx;
        idx = back[k][idx] as usize;
    }
    let mut d = Vec::with_capacity(c.n);
    let mut e = Vec::with_capacity(c.n);
    let mut alt = Vec::with_capacity(c.n);
    let mut all = Vec::new();
    for k in 0..c.n {
        let i = chosen[k];
        let r = &resp[k];
        // Parabolic sub-pixel refinement on the response (only inside the window).
        let mut off = 0.0f64;
        if i > 0 && i + 1 < s {
            let (a, b, cc) = (r[i - 1], r[i], r[i + 1]);
            let den = a - 2.0 * b + cc;
            if den < -1e-6 {
                off = f64::from((0.5 * (a - cc) / den).clamp(-0.5, 0.5));
            }
        }
        d.push(f64::from(centres[k] - win + i as i32) + off);
        e.push(r[i]);
        alt.push(
            (0..s)
                .filter(|j| j.abs_diff(i) > 4)
                .map(|j| r[j])
                .fold(0.0f32, f32::max),
        );
        all.extend_from_slice(r);
    }
    Pass {
        d,
        e,
        alt,
        all,
        resp,
        centres: centres.to_vec(),
    }
}

/// The result for one edge.
struct EdgeFit {
    /// Boundary points in picture pixels, one per station (bridged and lightly smoothed).
    pts: Vec<V>,
    /// Station confidence before bridging.
    conf: Vec<f32>,
    reliable: bool,
    /// How much better the fitted path is backed by edge evidence than the best straight line
    /// (0 = no better, 1 = a full-contrast edge's worth at every station).
    gain: f32,
}

fn moving_median(v: &[f64], half: usize) -> Vec<f64> {
    (0..v.len())
        .map(|k| {
            let lo = k.saturating_sub(half);
            let hi = (k + half).min(v.len() - 1);
            let mut w: Vec<f32> = v[lo..=hi].iter().map(|x| *x as f32).collect();
            f64::from(median(&mut w))
        })
        .collect()
}

fn fit_edge(
    frame: &Frame,
    c: &Chord,
    global: Rgb,
    light: bool,
    floor_hint: &mut Vec<f32>,
) -> EdgeFit {
    let win = c.win;
    let refs = paper_reference(frame, c, global, light);
    let mut centres = vec![0i32; c.n];
    let mut best: Option<Pass> = None;
    for pass in 0..PASSES {
        let p = search(frame, c, &centres, win, &refs);
        let wf = f64::from(win);
        let pushed =
            p.d.iter()
                .zip(&centres)
                .filter(|(d, ce)| (**d - f64::from(**ce)).abs() > 0.7 * wf)
                .count();
        let again = pass + 1 < PASSES && pushed as f64 > 0.10 * c.n as f64;
        if again {
            let sm = moving_median(&p.d, 3);
            centres = sm.iter().map(|d| d.round() as i32).collect();
        }
        best = Some(p);
        if !again {
            break;
        }
    }
    let p = best.expect("one pass at least");
    floor_hint.extend(p.all.iter().step_by(3).copied());
    // Confidence needs the picture's texture floor, which is known only after all edges ran, so
    // the raw contrast goes out and the caller finishes the job.
    let conf: Vec<f32> =
        p.e.iter()
            .zip(&p.alt)
            .map(|(e, a)| if *a > 0.9 * *e { *e * 0.7 } else { *e })
            .collect();
    let pts: Vec<V> = (0..c.n).map(|k| c.point(k, p.d[k])).collect();
    let gain = support_gain(&p, win);
    EdgeFit {
        pts,
        conf,
        reliable: false,
        gain,
    }
}

/// Mean (over stations) saturated edge response, in units of a full-contrast edge, of the fitted
/// path minus that of the best straight line through the same windows. The line is searched over
/// every pair of end offsets (integer pixels) and interpolated in between.
fn support_gain(p: &Pass, win: i32) -> f32 {
    let n = p.resp.len();
    let s = (2 * win + 1) as usize;
    let sat = |e: f32| e.min(E_CAP) / E_SCALE;
    let curve: f32 = p.e.iter().step_by(2).map(|e| sat(*e)).sum::<f32>() / n.div_ceil(2) as f32;
    // The line is searched in the same coordinates as the windows: absolute offset d.
    let lo = p.centres.iter().copied().min().unwrap_or(0) - win;
    let hi = p.centres.iter().copied().max().unwrap_or(0) + win;
    let mut best = 0.0f32;
    // A coarser grid when the range is wide keeps this cheap.
    let grid = (1 + (hi - lo) / 48) as usize;
    for a in (lo..=hi).step_by(grid) {
        for b in (lo..=hi).step_by(grid) {
            let mut sum = 0.0f32;
            for k in (0..n).step_by(2) {
                let t = k as f32 / (n - 1) as f32;
                let d = (a as f32 + (b - a) as f32 * t).round() as i32;
                let i = d - (p.centres[k] - win);
                if i >= 0 && (i as usize) < s {
                    sum += sat(p.resp[k][i as usize]);
                }
            }
            best = best.max(sum / n.div_ceil(2) as f32);
        }
    }
    (curve - best).max(0.0)
}

/// Degree of the robust trend of an edge's offsets.
const TREND_DEGREE: usize = 5;
/// Offsets that stray from the trend by more than this many robust scales are replaced by it.
const TREND_KEEP: f64 = 2.5;

/// Robust polynomial trend of `y` over `t` in -1..1 (Chebyshev basis, Tukey IRLS, observation
/// weights `w`), and the series with its outliers replaced by the trend.
#[allow(clippy::needless_range_loop)] // small dense linear algebra: index maths reads best as loops
fn robust_smooth(y: &[f64], w: &[f64]) -> Vec<f64> {
    let n = y.len();
    let m = TREND_DEGREE + 1;
    let basis: Vec<[f64; TREND_DEGREE + 1]> = (0..n)
        .map(|k| {
            let t = 2.0 * k as f64 / (n - 1) as f64 - 1.0;
            let mut b = [0.0; TREND_DEGREE + 1];
            b[0] = 1.0;
            b[1] = t;
            for j in 2..m {
                b[j] = 2.0 * t * b[j - 1] - b[j - 2];
            }
            b
        })
        .collect();
    let mut rw = vec![1.0f64; n];
    let mut coef = [0.0f64; TREND_DEGREE + 1];
    let mut scale = 1.0f64;
    for _ in 0..8 {
        // Weighted normal equations.
        let mut a = [[0.0f64; TREND_DEGREE + 2]; TREND_DEGREE + 1];
        for k in 0..n {
            let wk = w[k] * rw[k];
            for i in 0..m {
                for j in 0..m {
                    a[i][j] += wk * basis[k][i] * basis[k][j];
                }
                a[i][m] += wk * basis[k][i] * y[k];
            }
        }
        for i in 0..m {
            a[i][i] += 1e-9;
        }
        for col in 0..m {
            let piv = (col..m)
                .max_by(|p, q| {
                    a[*p][col]
                        .abs()
                        .partial_cmp(&a[*q][col].abs())
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .unwrap_or(col);
            a.swap(col, piv);
            let d = a[col][col];
            if d.abs() < 1e-12 {
                return y.to_vec();
            }
            for j in col..=m {
                a[col][j] /= d;
            }
            for r in 0..m {
                if r != col {
                    let f = a[r][col];
                    let row = a[col];
                    for j in col..=m {
                        a[r][j] -= f * row[j];
                    }
                }
            }
        }
        for i in 0..m {
            coef[i] = a[i][m];
        }
        let res: Vec<f64> = (0..n)
            .map(|k| y[k] - (0..m).map(|i| coef[i] * basis[k][i]).sum::<f64>())
            .collect();
        let mut abs: Vec<f32> = res.iter().map(|r| r.abs() as f32).collect();
        scale = (1.4826 * f64::from(median(&mut abs))).max(0.8);
        let c = 4.685 * scale;
        for k in 0..n {
            let u = res[k] / c;
            rw[k] = if u.abs() < 1.0 {
                (1.0 - u * u).powi(2)
            } else {
                0.0
            };
        }
    }
    (0..n)
        .map(|k| {
            let trend: f64 = (0..m).map(|i| coef[i] * basis[k][i]).sum();
            if (y[k] - trend).abs() <= TREND_KEEP * scale {
                y[k]
            } else {
                trend
            }
        })
        .collect()
}

/// Line through `pts` (total least squares): a point and a unit direction.
fn fit_line(pts: &[V]) -> Option<(V, V)> {
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
    let a = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    Some(((mx, my), (a.cos(), a.sin())))
}

fn intersect(a: (V, V), b: (V, V)) -> Option<V> {
    let det = a.1.0 * (-b.1.1) - a.1.1 * (-b.1.0);
    if det.abs() < 0.3 {
        // Less than about 17 degrees between the lines: no reliable crossing.
        return None;
    }
    let (rx, ry) = (b.0.0 - a.0.0, b.0.1 - a.0.1);
    let t = (rx * (-b.1.1) - ry * (-b.1.0)) / det;
    Some((a.0.0 + t * a.1.0, a.0.1 + t * a.1.1))
}

/// Ramer-Douglas-Peucker: indices of the kept points of `pts` (ends always kept).
fn rdp(pts: &[V], tol: f64) -> Vec<usize> {
    let n = pts.len();
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    let mut stack = vec![(0usize, n - 1)];
    while let Some((a, b)) = stack.pop() {
        let (mut far, mut at) = (0.0f64, a);
        for i in a + 1..b {
            let d = seg_dist(pts[i], pts[a], pts[b]);
            if d > far {
                far = d;
                at = i;
            }
        }
        if far > tol {
            keep[at] = true;
            stack.push((a, at));
            stack.push((at, b));
        }
    }
    (0..n).filter(|i| keep[*i]).collect()
}

fn seg_dist(p: V, a: V, b: V) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    dist(p, (a.0 + t * dx, a.1 + t * dy))
}

fn poly_dist(p: V, poly: &[V]) -> f64 {
    poly.windows(2)
        .map(|w| seg_dist(p, w[0], w[1]))
        .fold(f64::MAX, f64::min)
}

/// Control points for a polyline: RDP, then points are added where the Catmull-Rom spline through
/// the chosen ones strays from the polyline by more than 1.5 tolerances (at most [`MAX_CONTROL`]).
fn control_points(poly: &[V], tol: f64, size: (f64, f64)) -> Vec<V> {
    let mut idx = rdp(poly, tol);
    while idx.len() > MAX_CONTROL {
        // Drop the least significant point (smallest triangle) until it fits.
        let mut best = (f64::MAX, 1usize);
        for k in 1..idx.len() - 1 {
            let d = seg_dist(poly[idx[k]], poly[idx[k - 1]], poly[idx[k + 1]]);
            if d < best.0 {
                best = (d, k);
            }
        }
        idx.remove(best.1);
    }
    loop {
        let pts: Vec<V> = idx.iter().map(|i| poly[*i]).collect();
        if idx.len() >= MAX_CONTROL || idx.len() < 3 {
            return pts;
        }
        let Some(curve) = to_curve(&pts, size) else {
            return pts;
        };
        let line: Vec<V> = curve
            .polyline()
            .iter()
            .map(|p| (p.x * size.0, p.y * size.1))
            .collect();
        let (mut far, mut at) = (0.0f64, usize::MAX);
        for (i, p) in poly.iter().enumerate() {
            if idx.binary_search(&i).is_ok() {
                continue;
            }
            let d = poly_dist(*p, &line);
            if d > far {
                far = d;
                at = i;
            }
        }
        if far <= 1.5 * tol || at == usize::MAX {
            return pts;
        }
        let pos = idx.partition_point(|i| *i < at);
        idx.insert(pos, at);
    }
}

fn to_curve(pts: &[V], size: (f64, f64)) -> Option<Curve> {
    Curve::new(
        pts.iter()
            .map(|p| Pt::new(p.0 / size.0, p.1 / size.1))
            .collect(),
    )
    .ok()
}

fn fit_on_proxy(img: &Raster, quad: [Pt; 4]) -> Option<CurveFit> {
    let (w, h) = (img.width as usize, img.height as usize);
    let size = (w as f64, h as f64);
    let frame = Frame {
        w,
        h,
        data: &img.data,
    };
    let q: [V; 4] = std::array::from_fn(|i| (quad[i].x * size.0, quad[i].y * size.1));
    let area = crate::geometry::polygon_area(&q);
    if area.abs() < 0.0004 * size.0 * size.1 {
        return None;
    }
    let sign = if area >= 0.0 { 1.0 } else { -1.0 };
    let lens: [f64; 4] = std::array::from_fn(|i| dist(q[i], q[(i + 1) % 4]));
    if lens.iter().any(|l| *l < 12.0) {
        return None;
    }
    let short = ((lens[0] + lens[2]) / 2.0).min((lens[1] + lens[3]) / 2.0);
    let chords: Vec<Chord> = (0..4)
        .map(|i| {
            let (a, b) = (q[i], q[(i + 1) % 4]);
            let len = lens[i];
            let dir = ((b.0 - a.0) / len, (b.1 - a.1) / len);
            let n = ((len / 5.0).round() as usize).clamp(MIN_STATIONS, MAX_STATIONS);
            // A long edge may bow further than 6% of the page's width, but never so far that the
            // window reaches the opposite edge.
            let win = ((WINDOW_FRAC * short)
                .max(LONG_EDGE_FRAC * len)
                .min(MAX_WINDOW_FRAC * short)
                .round() as i32)
                .clamp(3, 80);
            let ref_lo = (win + REF_FROM).min((0.35 * short) as i32).max(win.min(4));
            let ref_hi = (ref_lo + (REF_TO - REF_FROM))
                .min((0.48 * short) as i32)
                .max(ref_lo + 3);
            Chord {
                a,
                dir,
                out: (sign * dir.1, -sign * dir.0),
                len,
                step: len / (n - 1) as f64,
                n,
                win,
                reference: (ref_lo, ref_hi),
            }
        })
        .collect();

    // The page's own colour: the per-channel median of a grid over the middle of the quad (ink
    // covers a minority of the page, so the median is the paper). A local reference that differs
    // from it by more than a shadow could is refused (see `paper_reference`).
    let mut inner: Vec<Rgb> = Vec::new();
    for iy in 0..13 {
        for ix in 0..13 {
            let (u, v) = (0.15 + 0.7 * ix as f64 / 12.0, 0.15 + 0.7 * iy as f64 / 12.0);
            let top = (
                q[0].0 + (q[1].0 - q[0].0) * u,
                q[0].1 + (q[1].1 - q[0].1) * u,
            );
            let bottom = (
                q[3].0 + (q[2].0 - q[3].0) * u,
                q[3].1 + (q[2].1 - q[3].1) * u,
            );
            let p = (
                top.0 + (bottom.0 - top.0) * v,
                top.1 + (bottom.1 - top.1) * v,
            );
            if let Some(c) = frame.at(p) {
                inner.push(c);
            }
        }
    }
    let global = mode_rgb(&inner)?;
    // Which way the paper differs from the desk decides which end of a band's brightness is paper:
    // ink (even blurred, dense text) is darker than pale paper and paler than dark paper.
    let mut outside: Vec<Rgb> = Vec::new();
    for c in &chords {
        for k in (0..c.n).step_by(3) {
            outside.extend(
                profile(&frame, c, k, c.win + 4, c.win + 10)
                    .into_iter()
                    .flatten(),
            );
        }
    }
    let light = median_rgb(&outside).is_none_or(|o| lum(global) >= lum(o));

    let mut floor: Vec<f32> = Vec::new();
    let mut edges: Vec<EdgeFit> = chords
        .iter()
        .map(|c| fit_edge(&frame, c, global, light, &mut floor))
        .collect();
    // The picture's own texture level: the median response over every candidate.
    let tex = if floor.is_empty() {
        3.0
    } else {
        median(&mut floor)
    };
    let e_floor = (1.5 * tex).clamp(3.0, 12.0);
    let span = (CONF_SPAN + 5.0).max(e_floor + CONF_SPAN);
    for ed in &mut edges {
        for c in &mut ed.conf {
            *c = ((*c - e_floor) / (span - e_floor)).clamp(0.0, 1.0);
        }
    }

    // Bridge unreliable runs, per edge.
    let mut edge_conf = [0.0f32; 4];
    for (i, (ed, c)) in edges.iter_mut().zip(&chords).enumerate() {
        edge_conf[i] = ed.conf.iter().sum::<f32>() / ed.conf.len() as f32;
        let good: Vec<usize> = (0..c.n).filter(|k| ed.conf[*k] >= RELIABLE).collect();
        ed.reliable = good.len() as f64 >= MIN_RELIABLE * c.n as f64 && good.len() >= 4;
        // Offsets from the chord, for bridging.
        let off: Vec<f64> = (0..c.n)
            .map(|k| {
                let b = c.base(k);
                (ed.pts[k].0 - b.0) * c.out.0 + (ed.pts[k].1 - b.1) * c.out.1
            })
            .collect();
        let bridged: Vec<f64> = if ed.reliable {
            (0..c.n)
                .map(|k| {
                    if ed.conf[k] >= RELIABLE {
                        return off[k];
                    }
                    let prev = good.iter().rev().find(|g| **g < k);
                    let next = good.iter().find(|g| **g > k);
                    match (prev, next) {
                        (Some(p), Some(n)) => {
                            let f = (k - p) as f64 / (n - p) as f64;
                            off[*p] + (off[*n] - off[*p]) * f
                        }
                        (Some(p), None) => off[*p],
                        (None, Some(n)) => off[*n],
                        (None, None) => off[k],
                    }
                })
                .collect()
        } else {
            // Too little evidence: a straight edge (the line through whatever was reliable, or the
            // chord).
            let pts: Vec<V> = good.iter().map(|k| ed.pts[*k]).collect();
            if pts.len() >= 3 {
                let (m, d) = fit_line(&pts).unwrap_or(((0.0, 0.0), (1.0, 0.0)));
                (0..c.n)
                    .map(|k| {
                        let b = c.base(k);
                        // Where the normal at the station meets the line.
                        let den = d.0 * c.out.1 - d.1 * c.out.0;
                        if den.abs() < 1e-3 {
                            return 0.0;
                        }
                        let t = ((m.0 - b.0) * d.1 - (m.1 - b.1) * d.0) / den;
                        t.clamp(-f64::from(c.win), f64::from(c.win))
                    })
                    .collect()
            } else {
                vec![0.0; c.n]
            }
        };
        // A detour (an object touching the page, a shadow edge) is pulled back to the robust trend.
        let weights: Vec<f64> = (0..c.n)
            .map(|k| {
                if ed.conf[k] >= RELIABLE {
                    f64::from(ed.conf[k])
                } else {
                    0.05
                }
            })
            .collect();
        let bridged = robust_smooth(&bridged, &weights);
        // One light smoothing pass ([1 2 1] / 4) keeps single-pixel jitter out of the polyline.
        let sm: Vec<f64> = (0..c.n)
            .map(|k| {
                let a = bridged[k.saturating_sub(1)];
                let b = bridged[k];
                let cc = bridged[(k + 1).min(c.n - 1)];
                if k == 0 || k == c.n - 1 {
                    b
                } else {
                    0.25 * a + 0.5 * b + 0.25 * cc
                }
            })
            .collect();
        ed.pts = (0..c.n).map(|k| c.point(k, sm[k])).collect();
    }

    // Corners: intersect the tangent lines of the two fitted ends that meet there.
    let m_end = |c: &Chord| ((0.12 * c.n as f64).ceil() as usize).max(4);
    let mut corners = q;
    for i in 0..4 {
        let prev = (i + 3) % 4;
        let (pe, ne) = (&edges[prev], &edges[i]);
        if !pe.reliable || !ne.reliable {
            continue;
        }
        let mp = m_end(&chords[prev]);
        let mn = m_end(&chords[i]);
        let pl = fit_line(&pe.pts[pe.pts.len() - mp..]);
        let nl = fit_line(&ne.pts[..mn]);
        if let (Some(pl), Some(nl)) = (pl, nl)
            && let Some(x) = intersect(pl, nl)
            && dist(x, q[i]) <= CORNER_MOVE * short
            && x.0 >= -0.2 * size.0
            && x.0 <= 1.2 * size.0
            && x.1 >= -0.2 * size.1
            && x.1 <= 1.2 * size.1
        {
            corners[i] = x;
        }
    }

    // Polylines between the refined corners, then control points.
    let tol = SIMPLIFY_TOL * short;
    let to_pt = |p: V| Pt::new(p.0 / size.0, p.1 / size.1);
    let mut curves: Vec<Curve> = Vec::with_capacity(4);
    let mut curvature = [0.0f32; 4];
    let mut bulge = [0.0f32; 4];
    let mut lens_new = [0.0f64; 4];
    for i in 0..4 {
        let (a, b) = (corners[i], corners[(i + 1) % 4]);
        let len = dist(a, b);
        if len < 8.0 {
            return None;
        }
        let dir = ((b.0 - a.0) / len, (b.1 - a.1) / len);
        let mut poly: Vec<V> = vec![a];
        let mut last_s = 0.0;
        for p in &edges[i].pts {
            let s = ((p.0 - a.0) * dir.0 + (p.1 - a.1) * dir.1) / len;
            if s > 0.02 && s < 0.98 && s > last_s + 0.004 {
                poly.push(*p);
                last_s = s;
            }
        }
        poly.push(b);
        let ctl = control_points(&poly, tol, size);
        let curve = to_curve(&ctl, size)?;
        // Deviation from the chord, of the spline itself.
        let dev = curve
            .polyline()
            .iter()
            .map(|p| seg_dist((p.x * size.0, p.y * size.1), a, b))
            .fold(0.0f64, f64::max);
        if dev > MAX_BULGE * short {
            return None;
        }
        lens_new[i] = len;
        // An edge whose bend is within the fit's own noise, or that no more evidence backs than a
        // straight line, is left straight: fitted wobble would only make a flat edge worse.
        if dev < KEEP_MIN_DEV_PX || edges[i].gain < KEEP_MIN_GAIN {
            curves.push(Curve::straight(to_pt(a), to_pt(b)));
            continue;
        }
        curvature[i] = (100.0 * dev / len) as f32;
        bulge[i] = (100.0 * dev / short) as f32;
        curves.push(curve);
    }
    let norm: [Pt; 4] =
        std::array::from_fn(|i| Pt::new(corners[i].0 / size.0, corners[i].1 / size.1));
    let [top, right, bottom, left]: [Curve; 4] = curves.try_into().ok()?;
    let warp = CurveWarp {
        top,
        right,
        bottom,
        left,
        quarter_turns: 0,
        mirror: false,
    };
    // Make the shared corners exactly equal (they are the same numbers by construction).
    warp.validate().ok()?;
    let new_area = crate::geometry::polygon_area(&corners).abs();
    if !(0.6..=1.6).contains(&(new_area / area.abs())) {
        return None;
    }
    // An edge counts towards `looks_curved` only when it is confident, long enough and bent by
    // more than the fit's own noise (in pixels), and the bend is backed by edge evidence that a
    // straight line does not have.
    let mut max_curvature = 0.0f32;
    for i in 0..4 {
        let dev_px = f64::from(curvature[i]) / 100.0 * lens_new[i];
        if edge_conf[i] >= LOOKS_CURVED_MIN_CONF
            && lens_new[i] >= LOOKS_CURVED_MIN_LEN_PX
            && dev_px >= LOOKS_CURVED_MIN_DEV_PX
            && edges[i].gain >= LOOKS_CURVED_MIN_GAIN
        {
            max_curvature = max_curvature.max(curvature[i]);
        }
    }
    Some(CurveFit {
        quad: norm,
        curves: warp,
        confidence: edge_conf,
        curvature,
        bulge,
        support_gain: std::array::from_fn(|i| edges[i].gain),
        max_curvature,
        looks_curved: max_curvature >= LOOKS_CURVED_MIN_PCT,
        fell_back: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How a test page looks.
    #[derive(Clone, Copy)]
    struct Look {
        paper: f64,
        desk: f64,
        ink: f64,
        grain: f64,
        /// A bar of `ink` colour across the top of the page (a title) 4% to 14% down.
        title_bar: bool,
    }

    const LIGHT_PAGE: Look = Look {
        paper: 235.0,
        desk: 95.0,
        ink: 85.0,
        grain: 6.0,
        title_bar: false,
    };

    /// A page on a desk: the unit square mapped by `x = X0 + s W`, `y = Y0 + t H + sag(s, t)`, so
    /// its top and bottom edges follow `sag` (`sag(s, 0)` and `sag(s, 1)`) and the page is the set
    /// of points whose inverse image lies in the square. Printed rules and an optional title bar
    /// inside; supersampled 3 x 3; deterministic grain.
    fn scene(w: u32, h: u32, sag: impl Fn(f64, f64) -> f64, look: Look) -> (Raster, [Pt; 4]) {
        let (x0, y0) = (0.15 * f64::from(w), 0.2 * f64::from(h));
        let (pw, ph) = (0.7 * f64::from(w), 0.6 * f64::from(h));
        let mut r = Raster::new(w, h);
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut noise = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f64 / (1u64 << 53) as f64 - 0.5
        };
        for y in 0..h {
            for x in 0..w {
                let mut acc = 0.0f64;
                for sy in 0..3 {
                    for sx in 0..3 {
                        let (px, py) = (
                            f64::from(x) + (f64::from(sx) + 0.5) / 3.0,
                            f64::from(y) + (f64::from(sy) + 0.5) / 3.0,
                        );
                        let s = (px - x0) / pw;
                        // Invert y = Y0 + t H + sag(s, t) by fixed point (the sag is gentle).
                        let mut t = (py - y0) / ph;
                        for _ in 0..6 {
                            t = (py - y0 - sag(s, t)) / ph;
                        }
                        let inside = (0.0..=1.0).contains(&s) && (0.0..=1.0).contains(&t);
                        let bar = look.title_bar && (0.04..0.14).contains(&t) && s > 0.1 && s < 0.7;
                        let rule = (t * 12.0).fract() < 0.08 && t > 0.25 && s > 0.15 && s < 0.85;
                        let c = if !inside {
                            look.desk
                        } else if bar || rule {
                            look.ink
                        } else {
                            look.paper
                        };
                        acc += c / 9.0;
                    }
                }
                let v = (acc + noise() * look.grain).clamp(0.0, 255.0) as u8;
                r.set_pixel(x, y, [v, v, v.saturating_sub(4)]);
            }
        }
        let q = [
            Pt::new(x0 / f64::from(w), y0 / f64::from(h)),
            Pt::new((x0 + pw) / f64::from(w), y0 / f64::from(h)),
            Pt::new((x0 + pw) / f64::from(w), (y0 + ph) / f64::from(h)),
            Pt::new(x0 / f64::from(w), (y0 + ph) / f64::from(h)),
        ];
        (r, q)
    }

    #[test]
    fn a_flat_page_stays_straight() {
        let (img, quad) = scene(480, 360, |_, _| 0.0, LIGHT_PAGE);
        let f = fit_curves(&img, quad);
        assert!(!f.fell_back);
        assert!(!f.looks_curved, "{:?}", f.curvature);
        assert!(
            f.max_curvature == 0.0 && f.curvature.iter().all(|c| *c < 0.8),
            "{:?}",
            f.curvature
        );
        assert!(f.confidence.iter().all(|c| *c > 0.5), "{:?}", f.confidence);
        // A flat page needs no interior control points to speak of.
        assert!(f.curves.curves().iter().all(|c| c.points().len() <= 3));
    }

    #[test]
    fn a_bowed_page_is_found_curved_and_the_curves_follow_it() {
        // The top and bottom edges sag by 18 px over 336 px: the sides do not move.
        let amp = 18.0;
        let (img, quad) = scene(480, 360, |s, _| amp * 4.0 * s * (1.0 - s), LIGHT_PAGE);
        let f = fit_curves(&img, quad);
        assert!(!f.fell_back);
        assert!(f.looks_curved, "{:?} {:?}", f.curvature, f.support_gain);
        let expect = 100.0 * amp / 336.0;
        for e in [0, 2] {
            assert!(
                (f64::from(f.curvature[e]) - expect).abs() < 1.2,
                "edge {e}: {:?} vs {expect}",
                f.curvature
            );
            assert!(f.support_gain[e] > LOOKS_CURVED_MIN_GAIN);
        }
        assert!(
            f.curvature[1] < 1.0 && f.curvature[3] < 1.0,
            "{:?}",
            f.curvature
        );
        f.curves.validate().expect("valid");
        assert!((3..=MAX_CONTROL).contains(&f.curves.top.points().len()));
        // The curve really sits on the edge: its midpoint is `amp` below the chord's.
        let mid = f.curves.top.at(0.5, (480.0, 360.0));
        let chord_mid_y = (f.quad[0].y + f.quad[1].y) / 2.0;
        assert!(
            ((mid.y - chord_mid_y) * 360.0 - amp).abs() < 3.0,
            "{} px",
            (mid.y - chord_mid_y) * 360.0
        );
    }

    #[test]
    fn a_page_darker_than_the_desk_and_a_title_bar_do_not_fool_it() {
        // Pale title bar on a dark page on a light desk: the bar's edge is the strongest edge
        // near the top, and the paper colour near it is the bar's.
        let look = Look {
            paper: 70.0,
            desk: 205.0,
            ink: 215.0,
            grain: 6.0,
            title_bar: true,
        };
        let (img, quad) = scene(480, 360, |_, _| 0.0, look);
        let f = fit_curves(&img, quad);
        assert!(
            !f.fell_back && !f.looks_curved,
            "{:?} {:?}",
            f.curvature,
            f.support_gain
        );
        assert!(f.curvature.iter().all(|c| *c < 0.8), "{:?}", f.curvature);
        // ... and the same page bowed is still found.
        let (img, quad) = scene(480, 360, |s, _| 16.0 * 4.0 * s * (1.0 - s), look);
        let f = fit_curves(&img, quad);
        assert!(f.looks_curved, "{:?} {:?}", f.curvature, f.support_gain);
    }

    #[test]
    fn corners_a_few_pixels_off_are_pulled_back_and_the_result_is_valid() {
        let (img, quad) = scene(480, 360, |s, _| 12.0 * 4.0 * s * (1.0 - s), LIGHT_PAGE);
        let off = [(5.0, 4.0), (-4.0, 5.0), (4.0, -5.0), (-5.0, -3.0)];
        let nudged: [Pt; 4] = std::array::from_fn(|i| {
            Pt::new(quad[i].x + off[i].0 / 480.0, quad[i].y + off[i].1 / 360.0)
        });
        let f = fit_curves(&img, nudged);
        assert!(!f.fell_back);
        for i in 0..4 {
            let d = ((f.quad[i].x - quad[i].x) * 480.0).hypot((f.quad[i].y - quad[i].y) * 360.0);
            let before = off[i].0.hypot(off[i].1);
            assert!(d < before, "corner {i}: {d:.1} px off, {before:.1} before");
        }
        f.curves
            .validate_with_quad(&auto_crop_core::QuadWarp {
                corners: f.quad,
                quarter_turns: 0,
                mirror: false,
                fine_deg: 0.0,
            })
            .expect("corners agree");
    }

    #[test]
    fn the_fit_is_deterministic_and_a_larger_picture_is_reduced_first() {
        let (img, quad) = scene(480, 360, |s, _| 10.0 * 4.0 * s * (1.0 - s), LIGHT_PAGE);
        assert_eq!(fit_curves(&img, quad), fit_curves(&img, quad));
        let (big, quad) = scene(1280, 960, |s, _| 26.0 * 4.0 * s * (1.0 - s), LIGHT_PAGE);
        let f = fit_curves(&big, quad);
        assert!(!f.fell_back && f.looks_curved, "{:?}", f.curvature);
    }

    #[test]
    fn nonsense_input_falls_back_to_the_straight_quad() {
        let img = Raster::filled(64, 64, [200, 200, 200]);
        let nan = [Pt::new(f64::NAN, 0.0); 4];
        let f = fit_curves(&img, nan);
        assert!(f.fell_back && !f.looks_curved && f.curves.is_straight());
        let tiny = [
            Pt::new(0.5, 0.5),
            Pt::new(0.5001, 0.5),
            Pt::new(0.5001, 0.5001),
            Pt::new(0.5, 0.5001),
        ];
        assert!(fit_curves(&img, tiny).fell_back);
        // A bow-tie is not a page.
        let bow_tie = [
            Pt::new(0.2, 0.2),
            Pt::new(0.8, 0.8),
            Pt::new(0.8, 0.2),
            Pt::new(0.2, 0.8),
        ];
        let f = fit_curves(&img, bow_tie);
        assert!(f.curves.is_straight() && !f.looks_curved);
        // A blank picture has no edge anywhere: the quad stays straight and nothing is confident.
        let quad = [
            Pt::new(0.2, 0.2),
            Pt::new(0.8, 0.2),
            Pt::new(0.8, 0.8),
            Pt::new(0.2, 0.8),
        ];
        let f = fit_curves(&img, quad);
        assert!(!f.looks_curved);
        assert!(f.confidence.iter().all(|c| *c < 0.2), "{:?}", f.confidence);
        // A mismatched buffer is refused, not read.
        let bad = Raster {
            width: 64,
            height: 64,
            data: vec![0; 10],
        };
        assert!(fit_curves(&bad, quad).fell_back);
    }
}
