// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Curved pages in the engine (`docs/dev/curved-pages.md`): the shape of a crop that is rendered
//! (a plain quad through the homography, or a curved page through the Coons resampler), the edit
//! API that makes a crop curved (`set_curves`, `curve_from_quad`, `clear_curves`), the preview of
//! a candidate curve set, and the admission weight of rendering one.
//!
//! * The detector never proposes a curved page: an image becomes curved only through this API.
//! * A curved crop is **held**: scan triage never approves it (`core::triage`), the crop's band is
//!   Check until the user accepts the scan (`Engine::accept_scan`, the machinery of M10.29), and
//!   an in-place save without that acceptance fails with `HELD_FOR_REVIEW` and the notice
//!   `curved.held`. A save as a copy destroys nothing and needs no acceptance. Any later edit
//!   (the acceptance is bound to the render hash) needs a new one.
//! * A curved crop is never saved by the lossless JPEG path and is resampled exactly once,
//!   straight from the full-resolution source, by the same resampler as the preview.

use crate::api::ItemView;
use crate::engine::{Engine, lock};
use crate::error::{ErrKind, Result};
use crate::memory::job_weight;
use crate::scan::{CropImage, labelled};
use auto_crop_core::{CurveWarp, EditState, Geometry, GestureId, ItemId, ItemsError, QuadWarp};
use auto_crop_imgproc::Raster;
use auto_crop_imgproc::cancel::{Cancel, NeverCancel};
use auto_crop_imgproc::curved::{output_size, render_curved_cancellable};
use auto_crop_imgproc::render::{Limits, RenderError, render_quad_cancellable};
use std::sync::Arc;

/// What a rendered crop is cut with: a perspective quad or a curved page.
#[derive(Debug, Clone, PartialEq)]
pub enum PageShape {
    Quad(QuadWarp),
    Curved(CurveWarp),
}

impl PageShape {
    /// The shape of a geometry that renders an output; `None` for identity and dense grids.
    pub fn of(g: &Geometry) -> Option<Self> {
        match g {
            Geometry::Quad(q) => Some(Self::Quad(q.clone())),
            Geometry::Curved(c) => Some(Self::Curved(c.clone())),
            _ => None,
        }
    }

    pub fn is_curved(&self) -> bool {
        matches!(self, Self::Curved(_))
    }

    pub fn as_quad(&self) -> Option<&QuadWarp> {
        match self {
            Self::Quad(q) => Some(q),
            Self::Curved(_) => None,
        }
    }

    /// Renders the crop out of `src`, capped by `limits`. A quad is `render_quad`, a curved page
    /// is `render_curved`: one resample either way, straight from `src`.
    pub fn render(&self, src: &Raster, limits: Limits) -> Result<Raster> {
        self.render_cancellable(src, limits, &NeverCancel)
    }

    /// [`PageShape::render`] that stops at the next 64-row band with `Cancelled` once `cancel`
    /// fires. A degenerate quad or an invalid curve set is `NoCrop` (the code every render of an
    /// unusable crop has always had).
    pub fn render_cancellable(
        &self,
        src: &Raster,
        limits: Limits,
        cancel: &dyn Cancel,
    ) -> Result<Raster> {
        let r = match self {
            Self::Quad(q) => render_quad_cancellable(src, q, limits, cancel),
            Self::Curved(c) => render_curved_cancellable(src, c, limits, cancel),
        };
        r.map_err(|e| match e {
            RenderError::DegenerateQuad => ErrKind::NoCrop,
            RenderError::Cancelled => ErrKind::Cancelled,
        })
    }
}

/// The weight a save of a curved page asks of a [`crate::memory::MemoryBudget`]: the ordinary job
/// weight of the decoded source (`pixels * 9 + 64 MiB`) plus six bytes per OUTPUT pixel (the
/// flattened raster and the encoder's working copy), because a curved page's size comes from its
/// arc lengths and not from the source's pixel count. PROVISIONAL, like the base weight.
pub fn curved_job_weight(source_pixels: u64, output_pixels: u64) -> u64 {
    job_weight(source_pixels).saturating_add(output_pixels.saturating_mul(6))
}

impl Engine {
    /// Edits the curves of one crop: a curve point drag, add or delete. The crop must be a quad or
    /// already curved; the curve set is validated (corners meet, point caps, no self-crossing).
    /// `live` returns the view the UI would show for a drag in progress without recording
    /// anything; `gesture` makes a drag one undo step per crop. A refused edit changes nothing.
    pub fn set_curves(
        &self,
        id: u32,
        crop: u32,
        curves: &CurveWarp,
        live: bool,
        label: &str,
        gesture: Option<u64>,
    ) -> Result<ItemView> {
        let crop = ItemId(crop);
        if live {
            return self.with_history(id, |it| {
                let mut st = it.history.as_ref().expect("checked").current().clone();
                st.set_item_curves(crop, curves.clone())
                    .map_err(ItemsError::kind)?;
                Ok(it.view_of(Some(&st)))
            })?;
        }
        let g = gesture.map(|g| GestureId((g << 32) ^ u64::from(crop.0)));
        let curves = curves.clone();
        self.crop_op(
            id,
            |st| labelled(label, st, crop),
            g,
            move |st, _| st.set_item_curves(crop, curves),
        )
    }

    /// Makes a quad crop a curved page with four straight edges (the fine angle is baked into the
    /// corners); the UI then adds points. A crop that is already curved is left as it is.
    pub fn curve_from_quad(&self, id: u32, crop: u32) -> Result<ItemView> {
        let c = ItemId(crop);
        self.crop_op(
            id,
            |st| labelled("Curve edges", st, c),
            None,
            |st, dims| st.curve_item_from_quad(c, dims),
        )
    }

    /// Takes a curved crop back to the straight quad through its corners (turns and mirror kept).
    pub fn clear_curves(&self, id: u32, crop: u32) -> Result<ItemView> {
        let c = ItemId(crop);
        self.crop_op(
            id,
            |st| labelled("Straighten edges", st, c),
            None,
            |st, _| st.clear_item_curves(c),
        )
    }

    /// Encoded bytes of `curves` rendered for one crop at preview resolution, without committing
    /// anything and without caching: what the UI shows while a curve point is being dragged. The
    /// source is the display proxy and the long edge the preview edge of `kind`, so the cost is
    /// that of a preview, not of a save.
    pub fn preview_curves(
        &self,
        id: u32,
        crop: u32,
        curves: &CurveWarp,
        kind: CropImage,
        cancel: &dyn Cancel,
    ) -> Result<(Arc<Vec<u8>>, &'static str)> {
        let item = self.item(id).ok_or(ErrKind::Internal)?;
        let ready = {
            let it = lock(&item);
            it.status == crate::api::ItemStatus::Ready
                && it
                    .history
                    .as_ref()
                    .is_some_and(|h| h.current().item(ItemId(crop)).is_some())
        };
        if !ready {
            return Err(ErrKind::Internal);
        }
        let proxy = self.proxy(id)?;
        let (edge, quality) = crate::scan::preview_edge(kind);
        let out = PageShape::Curved(curves.clone()).render_cancellable(
            &proxy,
            Limits {
                max_pixels: u64::MAX,
                max_edge: edge,
            },
            cancel,
        )?;
        let out = if kind == CropImage::Thumb {
            auto_crop_imgproc::scale::resize_to_fit(&out, crate::engine::THUMB_EDGE)
        } else {
            out
        };
        let bytes = crate::engine::jpeg(&out, quality)?;
        Ok((Arc::new(bytes), "image/jpeg"))
    }

    /// The weight a save of image `id` asks of a memory budget: the plain job weight for a quad,
    /// [`curved_job_weight`] when a crop is curved (by the largest output the crops will render,
    /// capped by the pixel cap).
    pub fn save_weight(&self, id: u32) -> Option<u64> {
        let item = self.item(id)?;
        let (dims, state) = {
            let it = lock(&item);
            (it.dims, it.history.as_ref()?.current().clone())
        };
        let src_px = u64::from(dims.0) * u64::from(dims.1);
        let cap = self.options().max_pixels;
        let out_px = curved_output_pixels(&state, dims, cap);
        Some(if out_px > 0 {
            curved_job_weight(src_px, out_px)
        } else {
            job_weight(src_px)
        })
    }
}

/// The largest output, in pixels, among the included curved crops of `state` rendered from a
/// source of `dims` under the pixel cap `cap`; 0 when no crop is curved.
pub fn curved_output_pixels(state: &EditState, dims: (u32, u32), cap: u64) -> u64 {
    state
        .included()
        .filter_map(|i| i.geometry.curves())
        .filter_map(|c| output_size(dims.0, dims.1, c, Limits::pixels(cap)))
        .map(|(w, h)| u64::from(w) * u64::from(h))
        .max()
        .unwrap_or(0)
}
