// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Multi-item detection: several photos or receipts on one scan or table (PLAN 4.7, ROADMAP
//! M10.01-M10.15). The engine calls [`detect_items`] and treats every returned item as an `Item`
//! with its own quad.
//!
//! The method is classical and conservative: it never merges items and never splits them without
//! evidence, and whatever it is unsure of stays one flagged item or holds the whole scan.
//!
//! 1. **Bed model** (`bed`): colours seen all around a ring inside the frame are the background;
//!    the flood fill from the frame removes the bed and its soft shadows, and the crisp outlines
//!    of items stop it. What is left is foreground.
//! 2. **Components** (`segment`): opening, hole filling, 8-connected labelling, area, dust and
//!    lid-edge filters, and the minimum-area rectangle with its fill ratio.
//! 3. **Splits** (`segment::split_blob`): a blob that is not rectangular is cut by a
//!    distance-transform watershed; the cut is kept only if both pieces are rectangles whose
//!    sides sit on crisp edges. Otherwise the blob stays one `Cluster`, flagged touching or
//!    overlapping.
//! 4. **Refinement** (`refine`): every side of every rectangle snaps to the crisp edge near it;
//!    the share of points on a crisp edge is the edge support and the colour step across the side
//!    its contrast.
//! 5. **Holds**: each item gets reason codes (partial frame, low contrast, items too close,
//!    touching, overlapping, odd aspect), the scan gets scan-level flags (unstable split, no bed,
//!    too many items, unexplained edges), and the confidence of the scan is the minimum over its
//!    items. A scan is auto-accepted only if every item is Good and nothing holds the scan.
//!
//! All thresholds are PROVISIONAL starting values (see `docs/perf/multi-item-baseline.md`).
//! The score is an UNCALIBRATED heuristic, like the single-item detector's.

mod bed;
mod color;
mod frame;
mod geom;
pub mod handoff;
mod label;
mod refine;
mod segment;

use crate::Raster;
use crate::scale::resize_to_fit;
use auto_crop_core::{Confidence, Forced, Pt, Reason, ReasonCode, Side};
use geom::{P, Rect, canonical_quad, centroid, convex_gap, convex_iou};
use segment::{Blob, Piece, Planes};

/// Per-batch split policy (PLAN 4.7; the engine stores it in `EditState.split`, M10.17).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SplitPolicy {
    #[default]
    Auto,
    /// Treat the image as a scan even if it does not look like one.
    Always,
    /// Do not look for several items: the result is the single-item route.
    Never,
}

/// What kind of things are expected (M10.12): sets the plausible aspect ratio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SplitProfile {
    #[default]
    Photos,
    Receipts,
}

#[derive(Debug, Clone)]
pub struct ItemsOptions {
    pub policy: SplitPolicy,
    pub profile: SplitProfile,
    /// Items kept (the largest) when more are found; `TooManyItems` is raised.
    pub max_items: usize,
    /// A clear gap under this share of the shorter side raises `ItemsTooClose`.
    pub min_gap_frac: f32,
    /// Components under this share of the image are dropped.
    pub min_area_frac: f32,
    /// Long edge of the analysis proxy.
    pub proxy_edge: u32,
    /// Re-run at 0.7x and 1.4x the edge threshold and flag a changing item count (M10.10).
    pub stability_check: bool,
    /// Score an item must reach to be Good (the strict preview cutoff, PLAN 4.9 and M10.29).
    pub good_cutoff: f32,
    /// Photographed tables are not scanner beds: when false (default), a scene whose border does
    /// not agree on one colour is held with `BED_UNCERTAIN`.
    pub trust_textured_beds: bool,
}

impl Default for ItemsOptions {
    fn default() -> Self {
        Self {
            policy: SplitPolicy::Auto,
            profile: SplitProfile::Photos,
            max_items: 32,
            min_gap_frac: 0.015,
            min_area_frac: 0.01,
            proxy_edge: 1024,
            stability_check: true,
            good_cutoff: 0.95,
            trust_textured_beds: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    /// A rectangle that passed the fill test.
    Rect,
    /// Several touching or overlapping items kept as one flagged outline (never accepted).
    Cluster,
}

#[derive(Debug, Clone)]
pub struct ItemCandidate {
    /// TL, TR, BR, BL (clockwise from the corner that starts the top edge), normalised to the
    /// image; may leave the frame slightly for a clipped item.
    pub quad: [Pt; 4],
    pub kind: ItemKind,
    /// Per-item confidence; `forced` is `Check` for every item with a hold reason.
    pub confidence: Confidence,
    /// Mask area over minimum-area-rectangle area of the component this item came from.
    pub fill: f32,
    /// The frame cuts the item off (also listed as `PARTIAL_FRAME`).
    pub partial_frame: bool,
    /// For a cluster: the plausible number of items inside it.
    pub count_range: Option<(u8, u8)>,
    pub signals: ItemSignals,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ItemSignals {
    pub support_min: f32,
    pub support_mean: f32,
    pub contrast_min: f32,
    /// Long side over short side.
    pub aspect: f32,
    /// Share of the image covered by the quad.
    pub area_frac: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing found on a bed-like scan: `NO_DOCUMENT`, held, nothing written.
    NoItems,
    /// One item that is (nearly) the whole picture, or `split: Never`: use the single-item
    /// pipeline (`detect::detect`).
    SingleItemRoute,
    /// One item: it is cropped by the normal single-item path from this quad.
    One,
    /// Two or more items (clusters count as one each).
    Many(usize),
}

#[derive(Debug, Clone)]
pub struct ScanFlags {
    pub bed_like: bool,
    pub outcome: Outcome,
    /// Scan-level hold reasons (no item carries them): any of them holds the whole scan.
    pub reasons: Vec<Reason>,
    /// Smallest clear gap between two accepted rectangles, as a share of the shorter side.
    pub min_gap_frac: Option<f32>,
    /// The item count and outlines survived the 0.7x and 1.4x re-runs.
    pub stable: bool,
    /// Crisp edge pixels outside every item, as a share of the shorter side's length.
    pub unexplained_edge: f32,
}

#[derive(Debug, Clone)]
pub struct Rejected {
    pub quad: [Pt; 4],
    pub why: &'static str,
}

#[derive(Debug, Clone, Default)]
pub struct Diagnostics {
    pub proxy_width: u32,
    pub proxy_height: u32,
    pub bed_sides_agreeing: u8,
    pub bed_cells: usize,
    pub bed_spread: f32,
    /// Median Lab colour of the bed (the agreeing border strips).
    pub bed_lab: [f32; 3],
    pub noise: f32,
    pub edge_threshold: f32,
    pub components: usize,
    pub blobs: usize,
    pub splits_accepted: usize,
    pub splits_rejected: usize,
    /// Components dropped by the filters; offered as "Add as item" in review (M10.05).
    pub rejected: Vec<Rejected>,
}

#[derive(Debug, Clone)]
pub struct ItemsDetection {
    /// In reading order (rows by vertical overlap, then left to right).
    pub items: Vec<ItemCandidate>,
    pub scan_flags: ScanFlags,
    pub diagnostics: Diagnostics,
}

impl ItemCandidate {
    /// The candidate as an auto `Item` (id 0: the engine assigns ids, `EditState::redetect`),
    /// ready to feed `redetect`. Its confidence carries the hold reasons.
    pub fn to_item(&self, pipeline_ver: u32) -> auto_crop_core::Item {
        auto_crop_core::items::auto_item(
            auto_crop_core::QuadWarp::new(self.quad),
            pipeline_ver,
            Some(self.confidence.clone()),
        )
    }
}

impl ItemsDetection {
    /// Every candidate as an auto item, in reading order. A `Cluster` is included as one item
    /// whose confidence is forced to Check, so the scan is held; it is never a Good item.
    pub fn to_items(&self, pipeline_ver: u32) -> Vec<auto_crop_core::Item> {
        self.items.iter().map(|i| i.to_item(pipeline_ver)).collect()
    }

    /// The confidence of the scan: the minimum score over the items, forced to `Check` when any
    /// item or the scan itself is held, `Failed` when nothing was found.
    pub fn scan_confidence(&self) -> Confidence {
        let score = self
            .items
            .iter()
            .map(|i| i.confidence.score)
            .fold(f32::INFINITY, f32::min);
        let mut reasons: Vec<Reason> = self.scan_flags.reasons.clone();
        for it in &self.items {
            for r in &it.confidence.reasons {
                if !reasons.iter().any(|x| x.code == r.code) {
                    reasons.push(*r);
                }
            }
        }
        let held = !reasons.is_empty() || self.items.iter().any(|i| i.confidence.forced.is_some());
        Confidence {
            score: if score.is_finite() { score } else { 0.0 },
            forced: (self.items.is_empty() || held).then_some(Forced::Check),
            reasons,
        }
    }

    /// The preview rule (M10.29): every item Good at `cutoff` and nothing holds the scan.
    pub fn auto_accept(&self, cutoff: f32) -> bool {
        !self.items.is_empty()
            && self.scan_flags.reasons.is_empty()
            && matches!(self.scan_flags.outcome, Outcome::Many(_) | Outcome::One)
            && self.items.iter().all(|i| {
                i.kind == ItemKind::Rect
                    && i.confidence.forced.is_none()
                    && i.confidence.score >= cutoff
            })
    }
}

fn reason(code: ReasonCode) -> Reason {
    Reason { code, side: None }
}

fn side_of(i: usize) -> Side {
    [Side::Top, Side::Right, Side::Bottom, Side::Left][i % 4]
}

struct Working {
    quad: [P; 4],
    kind: ItemKind,
    fill: f32,
    partial: bool,
    count_range: Option<(u8, u8)>,
    refined: Option<refine::Refined>,
    reasons: Vec<Reason>,
    area_px: f64,
}

fn rect_quad(r: &Rect) -> [P; 4] {
    r.corners()
}

fn pieces_overlap(pieces: &[Piece]) -> bool {
    for i in 0..pieces.len() {
        for j in i + 1..pieces.len() {
            let (a, b) = (pieces[i].rect.corners(), pieces[j].rect.corners());
            if convex_iou(&a, &b) > 0.08 {
                return true;
            }
        }
    }
    false
}

/// Detects the items on a scan or table photo. `src` is the decoded, EXIF-upright picture.
///
/// This is the one function the engine needs; its signature is stable.
pub fn detect_items(src: &Raster, opts: &ItemsOptions) -> ItemsDetection {
    detect_items_timed(src, opts, &mut Vec::new())
}

/// [`detect_items`] that also appends the cumulative milliseconds at the end of each stage to
/// `timings` (`proxy`, `frame`, `lab`, `model`, `prepare`, `segment`, `blobs`, `gaps`, `stability`,
/// `residual`, `finish`), for benchmarks (ROADMAP M10.61). The detection itself does not depend on
/// the clock.
pub fn detect_items_timed(
    src: &Raster,
    opts: &ItemsOptions,
    timings: &mut Vec<(&'static str, f64)>,
) -> ItemsDetection {
    let t0 = std::time::Instant::now();
    let mut mark = |name: &'static str| timings.push((name, t0.elapsed().as_secs_f64() * 1000.0));
    let proxy = if src.width.max(src.height) > opts.proxy_edge {
        resize_to_fit(src, opts.proxy_edge)
    } else {
        src.clone()
    };
    // A uniform frame around the picture (a white margin, a scanner border) is not the bed: cut
    // it off, look for items inside, and map the quads back (ROADMAP M10.02).
    mark("proxy");
    let trim = frame::find(&proxy);
    mark("frame");
    if trim == [0; 4] {
        return detect_in(&proxy, opts, [false; 4], &mut mark);
    }
    let (pw, ph) = (proxy.width as usize, proxy.height as usize);
    let (x0, y0) = (trim[3], trim[0]);
    let (cw, ch) = (pw - trim[1] - trim[3], ph - trim[0] - trim[2]);
    let cropped = frame::crop(&proxy, x0, y0, cw, ch);
    let mut det = detect_in(
        &cropped,
        opts,
        [trim[0] > 0, trim[1] > 0, trim[2] > 0, trim[3] > 0],
        &mut mark,
    );
    let map = |p: &mut Pt| {
        p.x = (x0 as f64 + p.x * cw as f64) / pw as f64;
        p.y = (y0 as f64 + p.y * ch as f64) / ph as f64;
    };
    for it in &mut det.items {
        it.quad.iter_mut().for_each(map);
    }
    for r in &mut det.diagnostics.rejected {
        r.quad.iter_mut().for_each(map);
    }
    det.diagnostics.proxy_width = proxy.width;
    det.diagnostics.proxy_height = proxy.height;
    det
}

fn detect_in(
    proxy: &Raster,
    opts: &ItemsOptions,
    frame_side: [bool; 4],
    mark: &mut dyn FnMut(&'static str),
) -> ItemsDetection {
    let (w, h) = (proxy.width as usize, proxy.height as usize);
    let mut diag = Diagnostics {
        proxy_width: proxy.width,
        proxy_height: proxy.height,
        ..Diagnostics::default()
    };
    let empty_flags = |bed_like: bool, outcome: Outcome, reasons: Vec<Reason>| ScanFlags {
        bed_like,
        outcome,
        reasons,
        min_gap_frac: None,
        stable: true,
        unexplained_edge: 0.0,
    };
    if w < 24 || h < 24 || opts.policy == SplitPolicy::Never {
        return ItemsDetection {
            items: Vec::new(),
            scan_flags: empty_flags(false, Outcome::SingleItemRoute, Vec::new()),
            diagnostics: diag,
        };
    }
    let lab_full = color::to_lab(proxy);
    mark("lab");
    let model = bed::BedModel::learn(&lab_full.blurred(1.0));
    diag.bed_sides_agreeing = model.triage.sides_agreeing;
    diag.bed_cells = model.cells;
    diag.bed_spread = model.triage.spread;
    diag.bed_lab = model.triage.bed_lab;
    mark("model");
    let mut planes = segment::prepare(&lab_full, &model);
    planes.frame_side = frame_side;
    diag.noise = planes.noise;
    mark("prepare");
    let seg = segment::segment(&planes, 1.0, opts.min_area_frac);
    diag.edge_threshold = seg.t_edge;
    diag.components = seg.n_components;
    diag.blobs = seg.blobs.len();
    diag.rejected = seg
        .rejected
        .iter()
        .map(|(r, why)| Rejected {
            quad: norm_quad(&rect_quad(r), w, h),
            why,
        })
        .collect();

    let short = planes.short() as f64;
    let min_piece_area = (opts.min_area_frac * (w * h) as f32) as usize;
    let fields = refine::Fields {
        w,
        h,
        g: &planes.g1,
        crisp: &seg.crisp,
        lab: &planes.lab,
    };

    mark("segment");
    let mut work: Vec<Working> = Vec::new();
    for blob in &seg.blobs {
        add_blob(
            blob,
            &seg,
            &planes,
            &fields,
            min_piece_area,
            &mut diag,
            &mut work,
        );
    }

    // Keep the largest `max_items`.
    let mut scan_reasons: Vec<Reason> = Vec::new();
    if work.len() > opts.max_items {
        work.sort_by(|a, b| b.area_px.total_cmp(&a.area_px));
        work.truncate(opts.max_items);
        scan_reasons.push(reason(ReasonCode::TooManyItems));
    }
    if seg.capped {
        scan_reasons.push(reason(ReasonCode::AnalysisLimit));
    }

    // Pairwise gaps between rectangles.
    mark("blobs");
    let mut min_gap: Option<f64> = None;
    let mut too_close = vec![false; work.len()];
    for i in 0..work.len() {
        for j in i + 1..work.len() {
            let g = convex_gap(&work[i].quad, &work[j].quad);
            min_gap = Some(min_gap.map_or(g, |m: f64| m.min(g)));
            if g < f64::from(opts.min_gap_frac) * short {
                too_close[i] = true;
                too_close[j] = true;
            }
        }
    }
    for (i, wk) in work.iter_mut().enumerate() {
        if too_close[i] && wk.kind == ItemKind::Rect {
            wk.reasons.push(reason(ReasonCode::ItemsTooClose));
        }
    }

    // Stability re-runs.
    mark("gaps");
    let mut stable = true;
    if opts.stability_check && !work.is_empty() {
        let base: Vec<[P; 4]> = seg.blobs.iter().map(|b| rect_quad(&b.rect)).collect();
        for gain in [0.7f32, 1.4] {
            let alt = segment::segment(&planes, gain, opts.min_area_frac);
            let alt_q: Vec<[P; 4]> = alt.blobs.iter().map(|b| rect_quad(&b.rect)).collect();
            if alt_q.len() != base.len() {
                stable = false;
                break;
            }
            let all_match = base
                .iter()
                .all(|b| alt_q.iter().any(|a| convex_iou(&b[..], &a[..]) >= 0.9));
            if !all_match {
                stable = false;
                break;
            }
        }
        if !stable {
            scan_reasons.push(reason(ReasonCode::SplitUnstable));
        }
    }

    // Crisp structure that no item explains (a missed low-contrast item, a hairline).
    mark("stability");
    let unexplained = unexplained_edge(&seg, &work, w, h);
    if unexplained > 0.35 && !work.is_empty() {
        scan_reasons.push(reason(ReasonCode::LowContrastEdge));
    }

    let bed_like = model.triage.bed_like;
    if !bed_like && !opts.trust_textured_beds {
        scan_reasons.push(reason(ReasonCode::BedUncertain));
    }

    // Per-item confidence.
    mark("residual");
    let mut items: Vec<ItemCandidate> = work
        .into_iter()
        .map(|wk| finish_item(wk, w, h, opts.profile))
        .collect();
    reading_order(&mut items);

    let outcome = match items.len() {
        0 => Outcome::NoItems,
        1 if items[0].signals.area_frac >= 0.95 => Outcome::SingleItemRoute,
        1 => Outcome::One,
        n => Outcome::Many(n),
    };
    if outcome == Outcome::NoItems {
        scan_reasons.push(reason(ReasonCode::NoDocument));
    }
    ItemsDetection {
        items,
        scan_flags: ScanFlags {
            bed_like,
            outcome,
            reasons: scan_reasons,
            min_gap_frac: min_gap.map(|g| (g / short) as f32),
            stable,
            unexplained_edge: unexplained as f32,
        },
        diagnostics: diag,
    }
}

fn norm_quad(q: &[P; 4], w: usize, h: usize) -> [Pt; 4] {
    std::array::from_fn(|i| Pt::new(q[i].0 / w as f64, q[i].1 / h as f64))
}

#[allow(clippy::too_many_arguments)]
fn add_blob(
    blob: &Blob,
    seg: &segment::Segmentation,
    planes: &Planes,
    fields: &refine::Fields,
    min_piece_area: usize,
    diag: &mut Diagnostics,
    out: &mut Vec<Working>,
) {
    let (w, h) = (planes.w, planes.h);
    let c = &blob.comp;
    // The mask already excludes soft shadows, so its outline is within a few pixels of the true
    // edge: a narrow window keeps the snap on the outer edge instead of an inner one (a card's
    // header band, a picture inside its border).
    let window = |r: &Rect| (0.03 * r.hw.min(r.hh) * 2.0).clamp(3.5, 8.0);
    let min_contact = ((0.02 * planes.short() as f32) as usize).max(3);
    // Contact with a side that is the edge of a trimmed frame is not a cut-off item.
    let partial = (0..4).any(|s| !planes.frame_side[s] && c.border[s] >= min_contact);
    if blob.fill >= 0.9 {
        let q = canonical_quad(rect_quad(&blob.rect));
        let r = refine::refine(&q, fields, window(&blob.rect));
        let mut reasons = Vec::new();
        let inside = refine::inspect_inside(&r.quad, fields);
        if let refine::Inside::Seam { axis, pos } = inside {
            // Two parallel crisp lines across the whole rectangle, opposite in polarity: the white
            // borders of two touching prints.
            let (a, b) = refine::cut_quad(&r.quad, axis, pos);
            let mut halves = Vec::new();
            for half in [a, b] {
                let hq = canonical_quad(half);
                let hr = refine::refine(&hq, fields, 4.0);
                halves.push(hr);
            }
            diag.splits_accepted += 1;
            for hr in halves {
                out.push(Working {
                    quad: hr.quad,
                    kind: ItemKind::Rect,
                    fill: blob.fill,
                    partial,
                    count_range: None,
                    area_px: geom::area(&hr.quad).abs(),
                    refined: Some(hr),
                    reasons: vec![reason(ReasonCode::TouchingItems)],
                });
            }
            return;
        }
        if inside == refine::Inside::Line {
            // A long internal edge: another item may lie on this one. Held, not cut.
            reasons.push(reason(ReasonCode::OverlappingItems));
        }
        out.push(Working {
            quad: r.quad,
            kind: ItemKind::Rect,
            fill: blob.fill,
            partial,
            count_range: None,
            area_px: geom::area(&r.quad).abs(),
            refined: Some(r),
            reasons,
        });
        return;
    }
    // Not a rectangle: try a cut, keep it only with crisp support on both pieces.
    if let Some(pieces) = segment::split_blob(&seg.labels, w, h, c, 0.88, min_piece_area, 1) {
        let mut refined: Vec<refine::Refined> = Vec::new();
        for p in &pieces {
            let q = canonical_quad(rect_quad(&p.rect));
            refined.push(refine::refine(&q, fields, window(&p.rect)));
        }
        let supported = refined.iter().all(|r| r.support_min() >= 0.5);
        let quads: Vec<[P; 4]> = refined.iter().map(|r| r.quad).collect();
        let mut overlap = false;
        for i in 0..quads.len() {
            for j in i + 1..quads.len() {
                if convex_iou(&quads[i], &quads[j]) > 0.1 {
                    overlap = true;
                }
            }
        }
        if supported && !overlap && !pieces_overlap(&pieces) {
            diag.splits_accepted += 1;
            for (p, r) in pieces.iter().zip(refined) {
                out.push(Working {
                    quad: r.quad,
                    kind: ItemKind::Rect,
                    fill: p.fill,
                    partial,
                    count_range: None,
                    area_px: geom::area(&r.quad).abs(),
                    refined: Some(r),
                    reasons: vec![reason(ReasonCode::TouchingItems)],
                });
            }
            return;
        }
        diag.splits_rejected += 1;
    }
    // A cluster: one flagged outline.
    let q = canonical_quad(rect_quad(&blob.rect));
    let overlapping = blob.fill < 0.8;
    let typical = typical_area(out).unwrap_or(blob.rect.area() / 2.0).max(1.0);
    let hi = ((blob.rect.area() * f64::from(blob.fill) / (0.6 * typical)).round() as i64)
        .clamp(2, 12) as u8;
    out.push(Working {
        quad: q,
        kind: ItemKind::Cluster,
        fill: blob.fill,
        partial,
        count_range: Some((2, hi)),
        area_px: blob.rect.area(),
        refined: None,
        reasons: vec![reason(if overlapping {
            ReasonCode::OverlappingItems
        } else {
            ReasonCode::TouchingItems
        })],
    });
}

fn typical_area(out: &[Working]) -> Option<f64> {
    let mut a: Vec<f64> = out
        .iter()
        .filter(|w| w.kind == ItemKind::Rect)
        .map(|w| w.area_px)
        .collect();
    if a.is_empty() {
        return None;
    }
    a.sort_by(f64::total_cmp);
    Some(a[a.len() / 2])
}

/// Crisp edge pixels that lie outside every item (grown by 8 px) and outside the outer 4% band,
/// in chains of at least 8% of the shorter side, as a multiple of that side.
fn unexplained_edge(seg: &segment::Segmentation, work: &[Working], w: usize, h: usize) -> f64 {
    let short = w.min(h);
    let band = (0.04 * short as f64) as usize;
    let mut explained = vec![false; w * h];
    for wk in work {
        let xs = wk.quad.iter().map(|p| p.0);
        let ys = wk.quad.iter().map(|p| p.1);
        let (x0, x1) = (
            xs.clone().fold(f64::MAX, f64::min) - 8.0,
            xs.fold(f64::MIN, f64::max) + 8.0,
        );
        let (y0, y1) = (
            ys.clone().fold(f64::MAX, f64::min) - 8.0,
            ys.fold(f64::MIN, f64::max) + 8.0,
        );
        for y in (y0.max(0.0) as usize)..=(y1.min((h - 1) as f64) as usize) {
            for x in (x0.max(0.0) as usize)..=(x1.min((w - 1) as f64) as usize) {
                explained[y * w + x] = true;
            }
        }
    }
    let loose: Vec<bool> = (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            seg.crisp[i] && !explained[i] && x >= band && y >= band && x + band < w && y + band < h
        })
        .collect();
    let (_, comps) = label::label8(&loose, w, h);
    let min_chain = 0.08 * short as f64;
    comps
        .iter()
        .filter(|c| ((c.x1 - c.x0).max(c.y1 - c.y0)) as f64 >= min_chain)
        .map(|c| c.area as f64)
        .sum::<f64>()
        / short as f64
}

fn finish_item(wk: Working, w: usize, h: usize, profile: SplitProfile) -> ItemCandidate {
    let q = canonical_quad(wk.quad);
    let (e0, e1) = (
        geom::len(geom::sub(q[1], q[0])),
        geom::len(geom::sub(q[3], q[0])),
    );
    let aspect = (e0.max(e1) / e0.min(e1).max(1.0)) as f32;
    let area_frac = (geom::area(&q).abs() / (w * h) as f64) as f32;
    let mut reasons = wk.reasons;
    let (mut support_min, mut support_mean, mut contrast_min) = (1.0f32, 1.0f32, 100.0f32);
    if let Some(r) = &wk.refined {
        support_min = r.support_min();
        support_mean = r.support_mean();
        contrast_min = r.contrast_min();
        if wk.kind == ItemKind::Rect {
            // A weak side that is not simply the frame edge.
            for (i, s) in r.sides.iter().enumerate() {
                let on_frame = {
                    let (a, b) = (r.quad[i], r.quad[(i + 1) % 4]);
                    let m = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
                    m.0 < 2.5 || m.1 < 2.5 || m.0 > w as f64 - 2.5 || m.1 > h as f64 - 2.5
                };
                if on_frame {
                    continue;
                }
                if s.support < 0.6 {
                    reasons.push(Reason {
                        code: ReasonCode::WeakEdge,
                        side: Some(side_of(i)),
                    });
                    break;
                }
            }
            if contrast_min < 8.0 {
                reasons.push(reason(ReasonCode::LowContrastEdge));
            }
        }
    }
    if wk.partial {
        reasons.push(reason(ReasonCode::PartialFrame));
    }
    let limit = match profile {
        SplitProfile::Photos => 4.0,
        SplitProfile::Receipts => 12.0,
    };
    if aspect > limit {
        reasons.push(reason(ReasonCode::OddAspect));
    }
    let sup = (support_min / 0.85).min(1.0);
    let con = (contrast_min / 30.0).clamp(0.0, 1.0);
    let fillf = (wk.fill / 0.95).min(1.0);
    let mut score = 0.35 + 0.65 * (0.55 * sup + 0.25 * con + 0.2 * fillf);
    if wk.kind == ItemKind::Cluster {
        score = score.min(0.5);
    }
    let forced = if reasons.is_empty() {
        None
    } else {
        Some(Forced::Check)
    };
    ItemCandidate {
        quad: norm_quad(&q, w, h),
        kind: wk.kind,
        confidence: Confidence {
            score,
            forced,
            reasons,
        },
        fill: wk.fill,
        partial_frame: wk.partial,
        count_range: wk.count_range,
        signals: ItemSignals {
            support_min,
            support_mean,
            contrast_min,
            aspect,
            area_frac,
        },
    }
}

/// Rows by at least 50% vertical overlap, then left to right (M10.20).
fn reading_order(items: &mut Vec<ItemCandidate>) {
    struct K {
        i: usize,
        y0: f64,
        y1: f64,
        cx: f64,
    }
    let mut ks: Vec<K> = items
        .iter()
        .enumerate()
        .map(|(i, it)| {
            let ys = it.quad.iter().map(|p| p.y);
            let pts: Vec<P> = it.quad.iter().map(|p| (p.x, p.y)).collect();
            K {
                i,
                y0: ys.clone().fold(f64::MAX, f64::min),
                y1: ys.fold(f64::MIN, f64::max),
                cx: centroid(&pts).0,
            }
        })
        .collect();
    ks.sort_by(|a, b| (a.y0 + a.y1).total_cmp(&(b.y0 + b.y1)));
    let mut rows: Vec<Vec<usize>> = Vec::new();
    let mut row_range: Vec<(f64, f64)> = Vec::new();
    for (n, k) in ks.iter().enumerate() {
        let mut placed = false;
        for (r, rng) in row_range.iter_mut().enumerate() {
            let overlap = (k.y1.min(rng.1) - k.y0.max(rng.0)).max(0.0);
            if overlap >= 0.5 * (k.y1 - k.y0).min(rng.1 - rng.0) {
                rows[r].push(n);
                rng.0 = rng.0.min(k.y0);
                rng.1 = rng.1.max(k.y1);
                placed = true;
                break;
            }
        }
        if !placed {
            rows.push(vec![n]);
            row_range.push((k.y0, k.y1));
        }
    }
    let mut order: Vec<usize> = Vec::new();
    for mut r in rows {
        r.sort_by(|a, b| {
            ks[*a]
                .cx
                .total_cmp(&ks[*b].cx)
                .then(ks[*a].i.cmp(&ks[*b].i))
        });
        order.extend(r.into_iter().map(|n| ks[n].i));
    }
    let mut taken: Vec<Option<ItemCandidate>> =
        std::mem::take(items).into_iter().map(Some).collect();
    for i in order {
        if let Some(it) = taken[i].take() {
            items.push(it);
        }
    }
}

#[cfg(test)]
mod tests;

/// A picture of what the segmentation saw, for local debugging (not a stable API): grey = pixels
/// the flood fill reached (bed and soft shadow), white = foreground, red = crisp edge pixels,
/// green marks pixels of bed-coloured class that the flood did not reach.
#[doc(hidden)]
pub fn debug_masks(src: &Raster, opts: &ItemsOptions) -> Raster {
    let proxy = if src.width.max(src.height) > opts.proxy_edge {
        resize_to_fit(src, opts.proxy_edge)
    } else {
        src.clone()
    };
    let (w, h) = (proxy.width as usize, proxy.height as usize);
    let lab_full = color::to_lab(&proxy);
    let model = bed::BedModel::learn(&lab_full.blurred(1.0));
    let planes = segment::prepare(&lab_full, &model);
    let seg = segment::segment(&planes, 1.0, opts.min_area_frac);
    let mut out = Raster::new(w as u32, h as u32);
    for i in 0..w * h {
        let fg = seg.labels[i] != 0;
        let mut c = if fg { [255, 255, 255] } else { [70, 70, 70] };
        if !fg && planes.class[i] == bed::CLASS_FG {
            c = [0, 120, 0];
        }
        if seg.crisp[i] {
            c = [230, 40, 40];
        }
        out.data[i * 3..i * 3 + 3].copy_from_slice(&c);
    }
    out
}
