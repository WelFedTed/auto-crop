// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Enhancement in the engine (PLAN 5; `docs/dev/enhance-engine.md` is the API for the UI):
//! the edit calls (`set_enhance`, `clear_enhance`, `set_default_enhance`), the live preview
//! (`preview_enhance`), the SUGGESTION (`suggest_enhance`), and the pixels of an enhanced crop for
//! the preview and the save path.
//!
//! * **Never forced (B13).** The state holds `Original` until a person (or an opt-in CLI run,
//!   `--enhance`) chose a mode. `suggest_enhance` only recommends; nothing here applies a
//!   suggestion.
//! * **Order of operations on a save:** geometry (quad, curved page or grid: ONE resample from the
//!   full-resolution source) -> flatten -> threshold -> encode. The enhancer sees the geometry
//!   output, never the source, so a crop's estimates come from that crop's pixels only; with a
//!   validity mask (pixels the geometry takes from outside the source frame are not paper) a quad
//!   that leaves the frame does not contaminate them either. With several items on a scan each is
//!   rendered and enhanced on its own, so estimates never mix items.
//! * **The lossless JPEG path** (`jpeg.lossless`) is for the identity only: a non-`Original`
//!   enhancement decodes, renders, enhances and re-encodes (one generation, like any other edit).
//! * **Colour management.** Clean keeps the source's colour space and ICC profile (the gain is
//!   applied in it; the wide-gamut conversion of ROADMAP M7.26 is not done). Document and Receipt
//!   write ONE grey channel, drop the colour profile (`enhance.icc_dropped`) and recommend PNG
//!   (`enhance.png_recommended`) on a JPEG save.

use crate::api::ItemView;
use crate::curved::PageShape;
use crate::engine::{Engine, lock};
use crate::error::{ErrKind, Result};
use crate::scan::{CropImage, labelled};
use auto_crop_core::{
    EditState, Enhance, EnhanceMode, Geometry, GestureId, ItemId, ItemsError, QuadWarp,
};
use auto_crop_imgproc::Raster;
use auto_crop_imgproc::cancel::{Cancel, NeverCancel};
use auto_crop_imgproc::enhance::measures::{Measures, Reason, measure, suggest};
use auto_crop_imgproc::enhance::{self as ie, Enhanced, Mode, Params, Report, Validity};
use auto_crop_imgproc::render::Limits;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Arc;

/// The engine's mode for a core mode.
pub fn mode_of(m: EnhanceMode) -> Mode {
    match m {
        EnhanceMode::Original => Mode::Original,
        EnhanceMode::Clean => Mode::Clean,
        EnhanceMode::Document => Mode::Document,
        EnhanceMode::Receipt => Mode::Receipt,
    }
}

/// The enhancer's parameters for a state value (the shipped tuning).
pub fn params_of(e: Enhance) -> Params {
    Params::new(mode_of(e.mode()), e.strength01())
}

/// A rendered, enhanced page: what is encoded.
pub struct Page {
    pub pixels: Enhanced,
    pub report: Report,
}

/// Does the shape take pixels from outside the source frame? Then the validity mask matters.
fn leaves_frame(shape: &PageShape, w: u32, h: u32) -> bool {
    const EPS: f64 = 1e-4;
    let outside =
        |p: &auto_crop_core::Pt| p.x < -EPS || p.x > 1.0 + EPS || p.y < -EPS || p.y > 1.0 + EPS;
    match shape {
        PageShape::Quad(q) => q.with_fine_baked(w, h).corners.iter().any(outside),
        PageShape::Curved(c) => c
            .curves()
            .iter()
            .any(|curve| curve.points().iter().any(outside)),
    }
}

/// The page mask of a shape: 255 where the output takes its pixels from inside the source frame,
/// 0 where it does not. Computed by rendering a small white image through the same geometry, so
/// it follows the quad, its fine angle and the curves exactly. `None` when the shape stays
/// inside the frame (every pixel is the page).
fn validity_mask(shape: &PageShape, w: u32, h: u32) -> Option<(u32, u32, Vec<u8>)> {
    if !leaves_frame(shape, w, h) {
        return None;
    }
    let k = w.max(h).div_ceil(256).max(1);
    let white = Raster::filled(w.div_ceil(k).max(2), h.div_ceil(k).max(2), [255; 3]);
    let out = shape
        .render_cancellable(
            &white,
            Limits {
                max_pixels: u64::MAX,
                max_edge: 256,
            },
            &NeverCancel,
        )
        .ok()?;
    let mask = out
        .data
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| if p[0] > 127 { 255 } else { 0 })
        .collect();
    Some((out.width, out.height, mask))
}

impl PageShape {
    /// Renders the crop (one resample, from `src`) and enhances it. [`Enhance::Original`] and
    /// strength 0 return the render untouched, so an unenhanced save is bit for bit what it was.
    /// `cancel` is polled once per 64-row band by the resampler and by the enhancer.
    pub fn render_enhanced(
        &self,
        src: &Raster,
        limits: Limits,
        enhance: Enhance,
        cancel: &dyn Cancel,
    ) -> Result<Page> {
        let raster = self.render_cancellable(src, limits, cancel)?;
        if enhance.is_identity() {
            return Ok(Page {
                pixels: Enhanced::Rgb(raster),
                report: Report::default(),
            });
        }
        let mask = validity_mask(self, src.width, src.height);
        let validity = mask
            .as_ref()
            .and_then(|(w, h, data)| Validity::new(*w, *h, data));
        let (pixels, report) = ie::enhance(raster, &params_of(enhance), validity.as_ref(), cancel)
            .map_err(|_| ErrKind::Cancelled)?;
        Ok(Page { pixels, report })
    }
}

/// Extra heap, in bytes, a save of the crops of `state` needs for enhancement beyond the render:
/// the largest over the included crops that are enhanced (flattened luma plus result plane, the
/// proxy). `dims` is the source size, `cap` the pixel cap of the output.
pub fn enhance_extra_bytes(state: &EditState, dims: (u32, u32), cap: u64) -> u64 {
    state
        .included()
        .filter_map(|i| {
            let e = i.enhance_override.unwrap_or(state.enhance);
            if e.is_identity() {
                return None;
            }
            let (w, h) = match &i.geometry {
                Geometry::Quad(q) => {
                    auto_crop_imgproc::render::output_size(dims.0, dims.1, q, Limits::pixels(cap))
                }
                Geometry::Curved(c) => {
                    auto_crop_imgproc::curved::output_size(dims.0, dims.1, c, Limits::pixels(cap))?
                }
                _ => return None,
            };
            Some(ie::extra_bytes(w, h, mode_of(e.mode())))
        })
        .max()
        .unwrap_or(0)
}

// ------------------------------------------------------------------------------ the suggestion

/// The numbers behind a suggestion, measured on the crop as it would be saved with enhancement
/// off (a 2 MP rendering of it). All are fractions of the page unless noted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnhanceMeasures {
    /// Long side over short side.
    pub aspect: f32,
    /// Share of the page at the local paper level, and share that is ink.
    pub paper_fraction: f32,
    pub ink_fraction: f32,
    /// Variation of the lighting `(p95 - p5) / P` before any flatten.
    pub shading: f32,
    /// Luma (0 to 255) of the brightest paper.
    pub paper_luma: f32,
    /// Spread of the paper colour: `(max - min) / max` over the channels.
    pub cast: f32,
    /// Share of saturated pixels after the white balance (logos, photographs, stamps).
    pub colour_fraction: f32,
    /// Glyph-sized components of the dominant size, how regular they are, and their share of the ink.
    pub text_components: u32,
    pub concentration: f32,
    pub glyph_ink_share: f32,
    /// Ink contrast in levels and whether it is low enough to read as faded thermal print.
    pub ink_contrast: f32,
    pub faded: bool,
}

impl From<&Measures> for EnhanceMeasures {
    fn from(m: &Measures) -> Self {
        Self {
            aspect: m.aspect,
            paper_fraction: m.paper_fraction,
            ink_fraction: m.ink_fraction,
            shading: m.shading,
            paper_luma: m.paper_luma,
            cast: m.cast,
            colour_fraction: m.colour_fraction,
            text_components: m.text_components,
            concentration: m.concentration,
            glyph_ink_share: m.glyph_ink_share,
            ink_contrast: m.ink_contrast,
            faded: m.faded,
        }
    }
}

/// What the output of a mode looks like, so a front end can offer the right format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputHint {
    /// The mode writes one grey channel (no colour profile, no colour).
    pub gray: bool,
    /// PNG is the recommended format: it is lossless and keeps faint marks. A JPEG save is allowed
    /// (grey JPEG at quality 90 or more) and carries the notice `enhance.png_recommended`.
    pub recommend_png: bool,
}

impl OutputHint {
    pub fn for_mode(mode: EnhanceMode) -> Self {
        Self {
            gray: mode.is_gray_output(),
            recommend_png: mode.is_gray_output(),
        }
    }
}

/// A recommendation to enhance one crop. It is a PROPOSAL: nothing is applied until the caller
/// passes it to [`Engine::set_enhance`] (`mode`, `strength / 100`), and the engine never applies it
/// itself; the one automatic application is the CLI's opt-in `process --enhance auto`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnhanceSuggestion {
    /// The suggested mode (never `original`).
    pub mode: EnhanceMode,
    /// Strength in percent (100).
    pub strength: u8,
    /// The main reason: `uneven_light`, `dull_paper`, `colour_cast` or `faded_print`; the UI looks
    /// its text up by it.
    pub reason: String,
    /// Every reason that holds, the main one first.
    pub reasons: Vec<String>,
    pub measures: EnhanceMeasures,
    pub hint: OutputHint,
}

impl EnhanceSuggestion {
    /// The value to put in the state to take the suggestion.
    pub fn enhance(&self) -> Enhance {
        Enhance::new(self.mode, self.strength)
    }
}

/// A small cache of suggestions by (image, crop page hash): the measure costs a render and an
/// analysis, the answer depends only on the page.
#[derive(Default)]
pub(crate) struct SuggestionCache {
    order: VecDeque<(u32, u64)>,
    map: std::collections::HashMap<(u32, u64), Option<EnhanceSuggestion>>,
}

const SUGGESTION_CACHE: usize = 128;

impl SuggestionCache {
    fn get(&self, k: &(u32, u64)) -> Option<Option<EnhanceSuggestion>> {
        self.map.get(k).cloned()
    }

    fn put(&mut self, k: (u32, u64), v: Option<EnhanceSuggestion>) {
        if self.map.insert(k, v).is_none() {
            self.order.push_back(k);
        }
        while self.order.len() > SUGGESTION_CACHE {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            }
        }
    }

    pub(crate) fn drop_item(&mut self, id: u32) {
        self.order.retain(|k| k.0 != id);
        self.map.retain(|k, _| k.0 != id);
    }
}

/// Long edge of the crop the suggestion is measured on (the display proxy's own edge).
const MEASURE_EDGE: u32 = crate::engine::DISPLAY_EDGE;

fn to_suggestion(
    s: auto_crop_imgproc::enhance::measures::Suggestion,
    m: &Measures,
) -> EnhanceSuggestion {
    let mode = match s.mode {
        Mode::Clean => EnhanceMode::Clean,
        Mode::Document => EnhanceMode::Document,
        Mode::Receipt => EnhanceMode::Receipt,
        Mode::Original => EnhanceMode::Original,
    };
    let reasons: Vec<String> = s
        .reasons
        .iter()
        .map(|r: &Reason| r.code().to_owned())
        .collect();
    EnhanceSuggestion {
        mode,
        strength: 100,
        reason: reasons.first().cloned().unwrap_or_default(),
        reasons,
        measures: EnhanceMeasures::from(m),
        hint: OutputHint::for_mode(mode),
    }
}

impl Engine {
    /// Does this crop look like a receipt or a document that enhancement would visibly improve?
    /// `Ok(Some(..))` says which mode and why, `Ok(None)` says leave it alone (a photograph, a page
    /// that is already clean, or an estimate that could not be trusted).
    ///
    /// The page is measured as it would be saved with enhancement off: the crop is rendered from
    /// the display proxy (a 2 MP preview of it) and analysed once; the answer is cached by the
    /// crop's page (orientation, margin, geometry), so asking again for an unchanged crop is free
    /// and an enhancement edit does not invalidate it. Nothing is recorded or applied. Takes a few
    /// tens of milliseconds the first time: call it from a worker, on demand (it is not run during
    /// analysis). `Ok(None)` while the image is not ready; `Err(NoCrop)` for an unknown crop or one
    /// that is not a quad or a curved page.
    pub fn suggest_enhance(&self, id: u32, crop: u32) -> Result<Option<EnhanceSuggestion>> {
        let item = self.item(id).ok_or(ErrKind::Internal)?;
        let (shape, key) = {
            let it = lock(&item);
            if it.status != crate::api::ItemStatus::Ready {
                return Ok(None);
            }
            let st = it.history.as_ref().ok_or(ErrKind::Internal)?.current();
            let c = ItemId(crop);
            let g = st.item(c).ok_or(ErrKind::NoCrop)?;
            let shape = PageShape::of(&g.geometry).ok_or(ErrKind::NoCrop)?;
            (shape, (id, st.item_page_hash(c).ok_or(ErrKind::NoCrop)?))
        };
        if let Some(hit) = lock(&self.inner.suggestions).get(&key) {
            return Ok(hit);
        }
        let proxy = self.proxy(id)?;
        let out = crate::run_isolated(std::panic::AssertUnwindSafe(
            || -> Result<Option<EnhanceSuggestion>> {
                let page = shape.render_cancellable(
                    &proxy,
                    Limits {
                        max_pixels: u64::MAX,
                        max_edge: MEASURE_EDGE,
                    },
                    &NeverCancel,
                )?;
                let m = measure(&page, &NeverCancel).map_err(|_| ErrKind::Cancelled)?;
                Ok(suggest(&m).map(|s| to_suggestion(s, &m)))
            },
        ))
        .unwrap_or(Err(ErrKind::InternalPanic))?;
        lock(&self.inner.suggestions).put(key, out.clone());
        Ok(out)
    }

    /// Sets the enhancement of one crop: `mode` at `strength` (0 to 1, clamped; `Original` ignores
    /// it). It is ONE undo step ("Enhance: Document"), the item's own override: the other crops
    /// and the state's default are untouched, and the crop's geometry, band and provenance do not
    /// change. `live` returns the view the UI would show for a slider drag in progress without
    /// recording anything (the picture for it comes from [`Engine::preview_enhance`]); `gesture`
    /// makes a drag one undo step per crop. A refused edit changes nothing.
    // The argument list is the command's own (`set_enhance(id, crop, mode, strength, live, label,
    // gesture)`, the shape of `set_curves` and `set_crop_edit`).
    #[allow(clippy::too_many_arguments)]
    pub fn set_enhance(
        &self,
        id: u32,
        crop: u32,
        mode: EnhanceMode,
        strength: f32,
        live: bool,
        label: &str,
        gesture: Option<u64>,
    ) -> Result<ItemView> {
        let crop = ItemId(crop);
        let e = Enhance::from_fraction(mode, strength);
        if live {
            return self.with_history(id, |it| {
                let mut st = it.history.as_ref().expect("checked").current().clone();
                st.set_item_enhance(crop, Some(e))
                    .map_err(ItemsError::kind)?;
                Ok(it.view_of(Some(&st)))
            })?;
        }
        let g = gesture.map(|g| GestureId((g << 32) ^ u64::from(crop.0)));
        let name = if label.is_empty() {
            format!("Enhance: {}", mode.name())
        } else {
            label.to_owned()
        };
        self.crop_op(
            id,
            |st| labelled(&name, st, crop),
            g,
            move |st, _| st.set_item_enhance(crop, Some(e)),
        )
    }

    /// The crop follows the state's default enhancement again (its override is removed). One undo
    /// step ("Reset enhancement").
    pub fn clear_enhance(&self, id: u32, crop: u32) -> Result<ItemView> {
        let c = ItemId(crop);
        self.crop_op(
            id,
            |st| labelled("Reset enhancement", st, c),
            None,
            move |st, _| st.set_item_enhance(c, None),
        )
    }

    /// Sets the DEFAULT enhancement of the image (what every crop without its own override gets).
    /// One undo step; `live` and `gesture` as in [`Engine::set_enhance`]. A front end that works
    /// per crop never needs it; the CLI uses it for `--enhance <mode>`.
    pub fn set_default_enhance(
        &self,
        id: u32,
        mode: EnhanceMode,
        strength: f32,
        live: bool,
        label: &str,
        gesture: Option<u64>,
    ) -> Result<ItemView> {
        let e = Enhance::from_fraction(mode, strength);
        if live {
            return self.with_history(id, |it| {
                let mut st = it.history.as_ref().expect("checked").current().clone();
                st.enhance = e;
                Ok::<_, ErrKind>(it.view_of(Some(&st)))
            })?;
        }
        let g = gesture.map(|g| GestureId((g << 32) ^ 0xE1A5));
        let name: String = if label.is_empty() {
            format!("Enhance: {}", mode.name())
        } else {
            label.chars().take(60).collect()
        };
        self.crop_op(
            id,
            move |_| name,
            g,
            move |st, _| {
                st.enhance = e;
                Ok(())
            },
        )
    }

    /// JPEG bytes of one crop with a CANDIDATE enhancement, at preview resolution, without
    /// committing or caching anything: what the UI shows while a strength slider is dragged.
    /// Cancellable per 64-row band (`Cancelled`); the preview edge is the display proxy's.
    pub fn preview_enhance(
        &self,
        id: u32,
        crop: u32,
        enhance: Enhance,
        kind: CropImage,
        cancel: &dyn Cancel,
    ) -> Result<(Arc<Vec<u8>>, &'static str)> {
        self.crop_jpeg(id, crop, kind, Some(enhance), cancel, false)
    }
}

/// A quad is the same quad whichever way it was reached; kept for the CLI's copy writer, which
/// holds quads rather than edit states.
pub fn quad_enhances(state: &EditState) -> Vec<(QuadWarp, Enhance)> {
    state
        .included()
        .filter_map(|i| {
            i.geometry
                .quad()
                .map(|q| (q.clone(), i.enhance_override.unwrap_or(state.enhance)))
        })
        .collect()
}
