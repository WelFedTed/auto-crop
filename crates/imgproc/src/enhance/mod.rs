// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Document and receipt enhancement (PLAN 5; ROADMAP M7.02 to M7.19): a classical, CPU-only,
//! pure-Rust stage that runs on the geometry output and before the encoder.
//!
//! ```text
//!  RGB8 page (one resample, from geometry) + optional validity mask
//!  [P] proxy       area average to about 1.5 MP                      (never the full image twice)
//!  [T] text scale  median height of the dominant ink component
//!  [A] analysis    block percentile grid -> rejection -> fill -> smooth -> gain grid (2 rounds)
//!  [F] flatten     gain applied to the full image in 64-row bands, bilinear from the coarse grid
//!       Clean      flatten, gentle levels (black point, white point, gamma), colour kept
//!       Document   flatten, Sauvola on the flattened luma, soft ramp   -> 8-bit grey
//!       Receipt    flatten, contrast stretch, NICK (ink fade), ramp    -> 8-bit grey
//! ```
//!
//! * [`Mode::Original`] is the identity: the input comes back untouched (the same buffer).
//! * `strength` (0 to 1) blends the result with the input; 0 is `Original` and costs nothing.
//! * Estimates come from the item's own pixels only: a [`Validity`] mask (0 = outside the page)
//!   keeps the table, the neighbouring items and the fill outside the statistics, and a
//!   thresholded mode paints those pixels white. With several items on a scan each item is
//!   enhanced on its own raster, so estimates never mix items.
//! * The full-resolution pass is done in bands of 64 rows by rayon tasks, each of which polls the
//!   [`Cancel`] token; there is no full-image `f32` copy (peak extra heap: one `u8` plane for the
//!   flattened luma and one for the result, plus about 1.5 MP of proxy). The bytes do not depend
//!   on the thread count.
//!
//! Lengths are multiples of the text height, so the same page gives the same result at preview and
//! at full resolution up to resampling. Numbers marked PROVISIONAL in PLAN 5.12 are the defaults
//! of [`Tuning`]; the OCR oracle (`tools/ocr_oracle`, CI) is what measures them.

mod apply;
mod illum;
pub mod measures;
pub mod testpages;
mod textscale;

pub use apply::GainField;
pub use textscale::TextScale;

use crate::Raster;
use crate::cancel::{Cancel, Cancelled};
use crate::scale::resize_area;
use crate::threshold::{STRIP_ROWS, local_map, nick_threshold, sauvola_threshold};
use rayon::prelude::*;

/// Proxy area in megapixels (PLAN 5.3.1 step 1).
pub const PROXY_MEGAPIXELS: f64 = 1.5;
/// Longest proxy edge.
pub const PROXY_MAX_EDGE: u32 = 3072;
/// Ink must sit at least this many levels under the white point, or the page counts as blank.
const NO_INK_MARGIN: f32 = 28.0;

/// What the enhancer does to the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum Mode {
    /// The identity: the pixels are never touched.
    #[default]
    Original,
    /// Flatten the lighting and set the levels; colour is kept.
    Clean,
    /// Flatten and binarise with Sauvola; 8-bit grey, black on white.
    Document,
    /// Flatten, stretch the contrast and binarise with NICK (faded thermal print); 8-bit grey.
    Receipt,
}

impl Mode {
    /// The mode writes a single grey channel.
    pub fn is_gray_output(self) -> bool {
        matches!(self, Mode::Document | Mode::Receipt)
    }
}

/// The local threshold of the thresholded modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Binariser {
    Sauvola,
    Nick,
}

/// How the edge between ink and paper is written.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Edge {
    /// Anti-aliased: a smoothstep ramp of half-width `rho` levels around the threshold.
    Soft,
    /// Strictly 0 or 255.
    Hard,
}

/// The numbers behind the modes (PLAN 5.12; PROVISIONAL until measured by the OCR oracle). The
/// default is what ships; sweeps change single fields.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tuning {
    /// Luma percentile of a block that measures the paper (0.90).
    pub percentile: f32,
    /// Block edge as a multiple of the text height, per mode.
    pub block_clean: f32,
    pub block_document: f32,
    pub block_receipt: f32,
    /// Estimate-and-apply rounds (2: the second measures the flattened proxy).
    pub passes: u8,
    /// Largest gain (PLAN: 3.0).
    pub gain_cap: f32,
    /// The level the paper is brought to by the flatten (before the white point).
    pub white_target: f32,
    /// Keep the paper tint: one luma gain for all channels (Clean only).
    pub keep_tint: bool,
    /// Threshold window as a multiple of the text height, Document and Receipt.
    pub window_document: f32,
    pub window_receipt: f32,
    pub document_method: Binariser,
    pub receipt_method: Binariser,
    pub sauvola_k: f32,
    pub nick_k: f32,
    /// The threshold is clamped to this range (PLAN 5.4: 64 to 208 for Sauvola) ...
    pub clamp_lo: f32,
    pub clamp_hi: f32,
    /// ... and NICK's upper limit (232).
    pub nick_hi: f32,
    /// Ramp half-width in levels.
    pub rho: f32,
    pub edge: Edge,
    /// The white point of the levels (242): flattened paper at or above it becomes 255.
    pub tone_white: f32,
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            // Sweep on the synthetic pages (`sweep_flatten`): the residual falls steadily as the
            // block shrinks towards the glyph height (hard-edged shadow: 10% at 1.75, 4% at 1.25,
            // 1.5% at 1.0), the percentile matters little. 1.25 keeps a margin against bold print
            // and large type filling a block.
            percentile: 0.88,
            block_clean: 1.25,
            block_document: 1.25,
            block_receipt: 1.25,
            passes: 2,
            gain_cap: 3.0,
            white_target: 250.0,
            keep_tint: false,
            window_document: 2.5,
            window_receipt: 2.5,
            document_method: Binariser::Sauvola,
            receipt_method: Binariser::Nick,
            sauvola_k: 0.25,
            // PLAN 5.4: -0.10. The OCR sweep (docs/perf/enhance.md): -0.2 read best on every
            // slice of the synthetic suite (mean CER 0.06% against 0.21%).
            nick_k: -0.20,
            clamp_lo: 64.0,
            clamp_hi: 208.0,
            nick_hi: 232.0,
            rho: 8.0,
            edge: Edge::Soft,
            tone_white: 242.0,
        }
    }
}

/// What to do and how much of it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    pub mode: Mode,
    /// 0 to 1: the result is blended with the input. 0 is the same as `Original`.
    pub strength: f32,
    pub tuning: Tuning,
}

impl Params {
    pub fn new(mode: Mode, strength: f32) -> Self {
        Self {
            mode,
            strength,
            tuning: Tuning::default(),
        }
    }

    /// Strength as the blend actually used: NaN and negatives are 0, above 1 is 1.
    pub fn strength01(&self) -> f32 {
        if self.strength.is_nan() {
            0.0
        } else {
            self.strength.clamp(0.0, 1.0)
        }
    }

    /// The call changes nothing (Original, or strength 0).
    pub fn is_identity(&self) -> bool {
        self.mode == Mode::Original || self.strength01() <= 0.0
    }
}

/// The pixels outside the page are 0. Any size: it is sampled by position, so a coarse mask of a
/// large image is fine.
#[derive(Debug, Clone, Copy)]
pub struct Validity<'a> {
    pub width: u32,
    pub height: u32,
    pub data: &'a [u8],
}

impl<'a> Validity<'a> {
    pub fn new(width: u32, height: u32, data: &'a [u8]) -> Option<Self> {
        (width > 0 && height > 0 && data.len() == width as usize * height as usize).then_some(
            Self {
                width,
                height,
                data,
            },
        )
    }
}

/// An 8-bit grey image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrayImage {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl GrayImage {
    /// Expands to RGB (R = G = B), for previews and verification.
    pub fn to_rgb(&self) -> Raster {
        let mut data = Vec::with_capacity(self.data.len() * 3);
        for v in &self.data {
            data.extend_from_slice(&[*v, *v, *v]);
        }
        Raster {
            width: self.width,
            height: self.height,
            data,
        }
    }
}

/// The result: colour kept, or one grey channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Enhanced {
    Rgb(Raster),
    Gray(GrayImage),
}

impl Enhanced {
    pub fn dims(&self) -> (u32, u32) {
        match self {
            Enhanced::Rgb(r) => (r.width, r.height),
            Enhanced::Gray(g) => (g.width, g.height),
        }
    }

    pub fn is_gray(&self) -> bool {
        matches!(self, Enhanced::Gray(_))
    }

    /// The result as RGB (a grey image is expanded).
    pub fn into_rgb(self) -> Raster {
        match self {
            Enhanced::Rgb(r) => r,
            Enhanced::Gray(g) => g.to_rgb(),
        }
    }
}

/// What the analysis measured. Pure data: the engine turns it into notices, the suggestion into
/// reasons.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Report {
    /// The enhancer ran (false for Original and strength 0).
    pub applied: bool,
    /// Proxy size.
    pub proxy: (u32, u32),
    /// Text height in full-resolution pixels and whether it was measured (else short side / 40).
    pub text_h: f32,
    pub text_confident: bool,
    /// Components that fed the text height.
    pub components: u32,
    /// How regular the glyph-sized components are, and their share of the ink (text-likeness).
    pub concentration: f32,
    pub glyph_ink_share: f32,
    /// Share of the page with saturated colour after the flatten (logos, photos, stamps).
    pub colour_fraction: f32,
    /// Spread of the paper colour: `(max - min) / max` over the channels of `paper`.
    pub cast: f32,
    /// Measuring block (proxy pixels) and threshold window (full-resolution pixels).
    pub block: u32,
    pub window: u32,
    /// Grid cells inside the page, and the share of them rejected as not paper.
    pub cells: u32,
    pub rejected: f32,
    /// Largest gain applied (1 when nothing was flattened).
    pub max_gain: f32,
    /// Paper colour before the flatten: the 90th percentile over cells, per channel.
    pub paper: [f32; 3],
    /// Illumination variation `(p95 - p5) / P` of the paper luma, before the flatten.
    pub shading: f32,
    /// Share of the page that is paper (flattened luma at or above 0.9 of the target).
    pub paper_fraction: f32,
    /// Share of the page that is ink (flattened luma more than 36 levels under the target).
    pub ink_fraction: f32,
    /// Paper level minus the 0.75th percentile of the flattened luma, in levels.
    pub ink_contrast: f32,
    /// Almost no ink: the stretch was skipped.
    pub low_contrast: bool,
    /// More than 40% of the cells were rejected, or none could be measured.
    pub unreliable: bool,
    /// Ink contrast is low: thermal paper that has faded (PROVISIONAL threshold).
    pub faded: bool,
    /// The gain field is not the identity.
    pub flattened: bool,
    /// Wall time of the analysis and of the application, in milliseconds (informational; not
    /// part of any determinism claim).
    pub analysis_ms: f32,
    pub apply_ms: f32,
    /// Where those milliseconds went.
    pub stage_ms: StageMs,
}

/// Wall time per stage, in milliseconds (informational).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct StageMs {
    /// Proxy and mask sampling.
    pub proxy: f32,
    /// Text scale (ink mask, components).
    pub text_scale: f32,
    /// First illumination estimate, its statistics and the first flatten of the proxy.
    pub illumination: f32,
    /// Second round (re-estimate on the flattened proxy) and the final proxy flatten.
    pub second_round: f32,
    /// Levels (histogram, LUT).
    pub tone: f32,
    /// Application: gain, levels and blend over the full image (for Document and Receipt: the
    /// flattened luma plane).
    pub flatten: f32,
    /// Application: the local threshold and the final pass (Document and Receipt).
    pub threshold: f32,
}

fn lap_ms(lap: &mut std::time::Instant) -> f32 {
    let ms = lap.elapsed().as_secs_f32() * 1e3;
    *lap = std::time::Instant::now();
    ms
}

/// The gain, the tone and the numbers the application needs. Cloneable and cheap to keep: a
/// slider that only changes the strength or the threshold reuses it.
#[derive(Debug, Clone)]
pub struct Analysis {
    pub mode: Mode,
    pub width: u32,
    pub height: u32,
    pub gain: GainField,
    pub(crate) lut: [u8; 256],
    pub(crate) text_h: f32,
    pub(crate) report: Report,
}

impl Analysis {
    pub fn report(&self) -> &Report {
        &self.report
    }
}

/// Proxy planes.
pub(crate) struct Proxy {
    pub w: usize,
    pub h: usize,
    pub rgb: Vec<u8>,
    pub luma: Vec<u8>,
}

/// Proxy size for an image: about 1.5 MP by area, at most 3072 px on the long edge, never larger
/// than the image.
pub fn proxy_size(width: u32, height: u32) -> (u32, u32) {
    let px = f64::from(width) * f64::from(height);
    let mut f = (px / (PROXY_MEGAPIXELS * 1e6)).sqrt().round().max(1.0);
    f = f.max((f64::from(width.max(height)) / f64::from(PROXY_MAX_EDGE)).ceil());
    (
        (f64::from(width) / f).ceil().max(1.0) as u32,
        (f64::from(height) / f).ceil().max(1.0) as u32,
    )
}

fn build_proxy(src: &Raster) -> Proxy {
    let (pw, ph) = proxy_size(src.width, src.height);
    let small = resize_area(src, pw, ph);
    let luma: Vec<u8> = small
        .data
        .par_chunks_exact(3)
        .map(|p| apply::luma709([p[0], p[1], p[2]]))
        .collect();
    Proxy {
        w: pw as usize,
        h: ph as usize,
        rgb: small.data,
        luma,
    }
}

/// The mask sampled at proxy positions.
fn proxy_mask(v: &Validity<'_>, pw: usize, ph: usize) -> Vec<u8> {
    let (mw, mh) = (v.width as usize, v.height as usize);
    let mut out = vec![0u8; pw * ph];
    out.par_chunks_mut(pw).enumerate().for_each(|(y, row)| {
        let my = (2 * y + 1) * mh / (2 * ph);
        for (x, o) in row.iter_mut().enumerate() {
            *o = v.data[my * mw + (2 * x + 1) * mw / (2 * pw)];
        }
    });
    out
}

fn percentile_of(hist: &[u64; 256], total: u64, q: f64) -> u8 {
    if total == 0 {
        return 0;
    }
    let target = ((total as f64 * q).ceil() as u64).clamp(1, total);
    let mut acc = 0;
    for (v, c) in hist.iter().enumerate() {
        acc += c;
        if acc >= target {
            return v as u8;
        }
    }
    255
}

fn histogram_of(luma: &[u8], valid: Option<&[u8]>) -> ([u64; 256], u64) {
    let mut h = [0u64; 256];
    let mut n = 0;
    match valid {
        None => {
            for v in luma {
                h[usize::from(*v)] += 1;
            }
            n = luma.len() as u64;
        }
        Some(m) => {
            for (v, mv) in luma.iter().zip(m) {
                if *mv != 0 {
                    h[usize::from(*v)] += 1;
                    n += 1;
                }
            }
        }
    }
    (h, n)
}

/// The levels LUT of the flattened luma: black point at the 0.75th percentile (capped at 0.35
/// of the white point), white point `tone_white`, gamma that puts the median ink-candidate near
/// 0.32 of the range (clamped 0.8 to 1.0). Almost no ink (the darkest 0.75% are within
/// [`NO_INK_MARGIN`] of the white point, which is what noise alone gives): no stretch, only the
/// white point. PLAN 5.3.1 says `white - 60`; faded thermal print sits 40 to 60 levels under the
/// paper and must still be stretched, so the margin is the noise floor instead (PROVISIONAL).
fn tone_lut(hist: &[u64; 256], total: u64, white: f32) -> ([u8; 256], bool, u8) {
    let b_raw = percentile_of(hist, total, 0.0075);
    let low = f32::from(b_raw) > white - NO_INK_MARGIN;
    let b = if low {
        0.0
    } else {
        f32::from(b_raw).min(0.35 * white)
    };
    let mut gamma = 1.0f32;
    if !low {
        let cut = (0.75 * white) as usize;
        let n: u64 = hist[..cut].iter().sum();
        if n > 0 {
            let mut acc = 0;
            let mut med = 0usize;
            for (v, c) in hist[..cut].iter().enumerate() {
                acc += c;
                if acc * 2 >= n {
                    med = v;
                    break;
                }
            }
            let x = ((med as f32 - b) / (white - b)).clamp(0.02, 0.98);
            gamma = (x.ln() / 0.32f32.ln()).clamp(0.8, 1.0);
        }
    }
    let lut = std::array::from_fn(|i| {
        let x = ((i as f32 - b) / (white - b)).clamp(0.0, 1.0);
        (255.0 * x.powf(1.0 / gamma) + 0.5) as u8
    });
    (lut, low, b_raw)
}

/// Analyses `src` for `mode`: text scale, illumination estimate and gain, tone. `Original` has
/// nothing to analyse and gets the identity.
pub fn analyse(
    src: &Raster,
    mode: Mode,
    tuning: &Tuning,
    validity: Option<&Validity<'_>>,
    cancel: &dyn Cancel,
) -> Result<Analysis, Cancelled> {
    let identity_lut: [u8; 256] = std::array::from_fn(|i| i as u8);
    let an = Analysis {
        mode,
        width: src.width,
        height: src.height,
        gain: GainField::identity(),
        lut: identity_lut,
        text_h: 0.0,
        report: Report::default(),
    };
    if mode == Mode::Original || src.width == 0 || src.height == 0 {
        return Ok(an);
    }
    let started = std::time::Instant::now();
    let mut an = analyse_inner(an, src, mode, tuning, validity, cancel)?;
    an.report.analysis_ms = started.elapsed().as_secs_f32() * 1e3;
    Ok(an)
}

fn analyse_inner(
    mut an: Analysis,
    src: &Raster,
    mode: Mode,
    tuning: &Tuning,
    validity: Option<&Validity<'_>>,
    cancel: &dyn Cancel,
) -> Result<Analysis, Cancelled> {
    an.report.applied = true;
    let mut lap = std::time::Instant::now();
    let proxy = build_proxy(src);
    let (pw, ph) = (proxy.w, proxy.h);
    let vmask = validity.map(|v| proxy_mask(v, pw, ph));
    let vm = vmask.as_deref();
    let scale = src.width as f32 / pw as f32;
    an.report.proxy = (pw as u32, ph as u32);
    an.report.stage_ms.proxy = lap_ms(&mut lap);
    if cancel.is_cancelled() {
        return Err(Cancelled);
    }

    let ts = textscale::text_scale(&proxy.luma, pw, ph, vm);
    an.report.stage_ms.text_scale = lap_ms(&mut lap);
    an.text_h = ts.height * scale;
    an.report.text_h = an.text_h;
    an.report.text_confident = ts.confident;
    an.report.components = ts.components;
    an.report.concentration = ts.concentration;
    an.report.glyph_ink_share = ts.glyph_ink_share;
    let bf = match mode {
        Mode::Clean => tuning.block_clean,
        Mode::Document => tuning.block_document,
        _ => tuning.block_receipt,
    };
    let block = ((bf * ts.height).round() as usize).clamp(16, 96);
    an.report.block = block as u32;
    let window = ((match mode {
        Mode::Document => tuning.window_document,
        _ => tuning.window_receipt,
    } * an.text_h)
        .round() as usize)
        .clamp(15, 151);
    an.report.window = (window | 1) as u32;

    let ip = illum::IllumParams::with_block(block, tuning.percentile);
    let luma_only = tuning.keep_tint && mode == Mode::Clean;
    let Some(est1) = illum::estimate(&proxy, vm, &ip, cancel)? else {
        an.report.unreliable = true;
        return Ok(an);
    };
    an.report.cells = est1.cells as u32;
    an.report.rejected = est1.rejected;
    an.report.unreliable = est1.rejected > 0.40;
    // The paper before the flatten: P per channel (90th percentile of the cells) and V.
    let inside_v: Vec<[f32; 3]> = est1
        .grid
        .v
        .iter()
        .zip(&est1.inside)
        .filter(|(_, ok)| **ok)
        .map(|(v, _)| *v)
        .collect();
    let mut chan: [Vec<f32>; 3] = std::array::from_fn(|c| inside_v.iter().map(|v| v[c]).collect());
    for ch in &mut chan {
        ch.sort_unstable_by(f32::total_cmp);
    }
    let at = |ch: &[f32], q: f32| ch[((ch.len() - 1) as f32 * q).round() as usize];
    an.report.paper = [at(&chan[0], 0.9), at(&chan[1], 0.9), at(&chan[2], 0.9)];
    let (pmax, pmin) = an
        .report
        .paper
        .iter()
        .fold((0f32, f32::MAX), |(a, b), v| (a.max(*v), b.min(*v)));
    an.report.cast = if pmax > 0.0 {
        (pmax - pmin) / pmax
    } else {
        0.0
    };
    let mut lumas: Vec<f32> = inside_v
        .iter()
        .map(|v| 0.299 * v[0] + 0.587 * v[1] + 0.114 * v[2])
        .collect();
    lumas.sort_unstable_by(f32::total_cmp);
    let p_luma = at(&lumas, 0.9).max(1.0);
    an.report.shading = ((at(&lumas, 0.95) - at(&lumas, 0.05)) / p_luma).max(0.0);

    let g1 = GainField::from_paper(&est1.grid, tuning.white_target, tuning.gain_cap, luma_only);
    let mut luma1 = Vec::new();
    let rgb1 = apply::flatten_copy(&proxy.rgb, &mut luma1, pw, ph, &g1);
    // Round-one statistics: how much of the page sits at the paper level, how much is ink.
    let (h1, n1) = histogram_of(&luma1, vm);
    let paper_cut = (0.9 * tuning.white_target) as usize;
    // Ink: clearly under the paper level. 36 levels is 3.5 sigma of the noise a gain of 3 leaves,
    // and still catches faded thermal print (40 to 60 levels under the paper).
    let ink_cut = (tuning.white_target - 36.0).max(1.0) as usize;
    let n1f = n1.max(1) as f32;
    an.report.paper_fraction = h1[paper_cut..].iter().sum::<u64>() as f32 / n1f;
    an.report.ink_fraction = h1[..ink_cut].iter().sum::<u64>() as f32 / n1f;
    // Saturated colour after the white balance: logos, photographs and stamps on the page.
    let coloured = rgb1
        .par_chunks_exact(3)
        .enumerate()
        .filter(|(i, p)| {
            vm.is_none_or(|m| m[*i] != 0)
                && p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]) > 48
        })
        .count();
    an.report.colour_fraction = coloured as f32 / n1f;
    an.report.stage_ms.illumination = lap_ms(&mut lap);

    let (gain, final_luma) = if tuning.passes >= 2 {
        let p1 = Proxy {
            w: pw,
            h: ph,
            rgb: rgb1,
            luma: luma1,
        };
        match illum::estimate(&p1, vm, &ip, cancel)? {
            Some(est2) => {
                let g2 = GainField::from_paper(
                    &est2.grid,
                    tuning.white_target,
                    tuning.gain_cap,
                    luma_only,
                );
                let mut luma2 = Vec::new();
                let _ = apply::flatten_copy(&p1.rgb, &mut luma2, pw, ph, &g2);
                (g1.times(&g2), luma2)
            }
            None => (g1, p1.luma),
        }
    } else {
        (g1, luma1)
    };
    if cancel.is_cancelled() {
        return Err(Cancelled);
    }
    an.report.stage_ms.second_round = lap_ms(&mut lap);
    an.report.max_gain = gain.max_gain();
    an.report.flattened = !gain.is_identity();
    an.gain = gain;

    let (hist, n) = histogram_of(&final_luma, vm);
    let (lut, low, b_raw) = tone_lut(&hist, n, tuning.tone_white);
    let paper_level = f32::from(percentile_of(&hist, n, 0.5));
    an.report.low_contrast = low;
    an.report.ink_contrast = (paper_level - f32::from(b_raw)).max(0.0);
    an.report.faded = !low && an.report.ink_contrast < 100.0;
    // Clean and Receipt always set the levels. Document does only for faded print: Sauvola alone
    // loses faint ink (the OCR matrix: Document on thermal fade read nothing at all), and with
    // the stretch it reads as well as Receipt.
    if matches!(mode, Mode::Clean | Mode::Receipt) || (mode == Mode::Document && an.report.faded) {
        an.lut = lut;
    }
    an.report.stage_ms.tone = lap_ms(&mut lap);
    Ok(an)
}

/// Applies `an` to `src` (the same image the analysis saw) and returns the result and the report.
pub fn apply(
    src: Raster,
    an: &Analysis,
    params: &Params,
    validity: Option<&Validity<'_>>,
    cancel: &dyn Cancel,
) -> Result<(Enhanced, Report), Cancelled> {
    let mut report = an.report;
    if params.is_identity() || an.mode == Mode::Original {
        return Ok((Enhanced::Rgb(src), Report::default()));
    }
    let started = std::time::Instant::now();
    let r = apply_inner(src, an, params, validity, cancel);
    report.apply_ms = started.elapsed().as_secs_f32() * 1e3;
    r.map(|(e, flatten, threshold)| {
        report.stage_ms.flatten = flatten;
        report.stage_ms.threshold = threshold;
        (e, report)
    })
}

fn apply_inner(
    src: Raster,
    an: &Analysis,
    params: &Params,
    validity: Option<&Validity<'_>>,
    cancel: &dyn Cancel,
) -> Result<(Enhanced, f32, f32), Cancelled> {
    let (w, h) = (src.width as usize, src.height as usize);
    if w == 0 || h == 0 {
        return Ok((Enhanced::Rgb(src), 0.0, 0.0));
    }
    let weight = apply::weight256(params.strength01());
    let t = &params.tuning;
    let mut lap = std::time::Instant::now();
    match an.mode {
        Mode::Original => Ok((Enhanced::Rgb(src), 0.0, 0.0)),
        Mode::Clean => {
            let mut src = src;
            apply::clean_in_place(&mut src.data, w, h, &an.gain, &an.lut, weight, cancel)?;
            Ok((Enhanced::Rgb(src), lap_ms(&mut lap), 0.0))
        }
        Mode::Document | Mode::Receipt => {
            let vm = validity.map(|v| (v.data, v.width as usize, v.height as usize));
            let luma = apply::flat_luma(&src.data, w, h, &an.gain, &an.lut, vm, cancel)?;
            let flatten_ms = lap_ms(&mut lap);
            // The original is needed again only to blend at a strength below 1.
            let orig = (weight < 256).then_some(src.data);
            let radius = (an.report.window as usize | 1) / 2;
            let (receipt, lo, hi) = (an.mode == Mode::Receipt, t.clamp_lo, t.clamp_hi);
            let method = if receipt {
                t.receipt_method
            } else {
                t.document_method
            };
            let (k_s, k_n, nick_hi, rho) = (
                f64::from(t.sauvola_k),
                f64::from(t.nick_k),
                f64::from(t.nick_hi),
                f64::from(t.rho.max(0.5)),
            );
            let (lo, hi) = (f64::from(lo), f64::from(hi));
            let thr = move |s: u64, q: u64, n: u64| -> f64 {
                match method {
                    Binariser::Sauvola => sauvola_threshold(s, q, n, k_s, 128.0).clamp(lo, hi),
                    Binariser::Nick => nick_threshold(s, q, n, k_n).clamp(lo, nick_hi),
                }
            };
            let soft = t.edge == Edge::Soft;
            let emit = move |v: u8, th: f64| -> u8 {
                if soft {
                    let x = ((f64::from(v) - (th - rho)) / (2.0 * rho)).clamp(0.0, 1.0);
                    (255.0 * x * x * (3.0 - 2.0 * x) + 0.5) as u8
                } else if f64::from(v) > th {
                    255
                } else {
                    0
                }
            };
            let mut out = local_map(&luma, w, h, radius, STRIP_ROWS, &thr, &emit, cancel)?;
            drop(luma);
            apply::finish_gray(&mut out, orig.as_deref(), w, h, weight, vm, cancel)?;
            Ok((
                Enhanced::Gray(GrayImage {
                    width: src.width,
                    height: src.height,
                    data: out,
                }),
                flatten_ms,
                lap_ms(&mut lap),
            ))
        }
    }
}

/// Analyses and applies in one call. [`Mode::Original`] and strength 0 return the input untouched.
pub fn enhance(
    src: Raster,
    params: &Params,
    validity: Option<&Validity<'_>>,
    cancel: &dyn Cancel,
) -> Result<(Enhanced, Report), Cancelled> {
    if params.is_identity() {
        return Ok((Enhanced::Rgb(src), Report::default()));
    }
    let an = analyse(&src, params.mode, &params.tuning, validity, cancel)?;
    apply(src, &an, params, validity, cancel)
}

/// Extra heap, in bytes, an enhancement of a `width` x `height` page needs beyond the input
/// raster (for memory admission): the flattened luma plane, the result plane, the original
/// luma when blending, plus the proxy and its working copies.
pub fn extra_bytes(width: u32, height: u32, mode: Mode) -> u64 {
    let px = u64::from(width) * u64::from(height);
    let (pw, ph) = proxy_size(width, height);
    let proxy = u64::from(pw) * u64::from(ph) * (3 + 1) * 3;
    match mode {
        Mode::Original => 0,
        Mode::Clean => proxy,
        Mode::Document | Mode::Receipt => 2 * px + proxy,
    }
}

#[cfg(test)]
mod tests;
