// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! "Trim to content" in the engine: a picture sitting inside a border of the page's own colour (a
//! print on a wide white mat) has two plausible outlines, the paper edge and the picture. The
//! detector answers with the paper edge (decision B14) and holds it with `DETECTORS_DISAGREE`;
//! this module proposes the picture's outline for the crop the person is looking at.
//!
//! * [`Engine::trim_to_content`] is a PROPOSAL. It records nothing, caches nothing and never touches
//!   the crop, exactly like `suggest_curves`; the answer is `None` when there is no picture inside
//!   to trim to (no clear border, a text block, a full-bleed picture).
//! * [`Engine::apply_trim`] applies it: ONE undo step labelled "Trim to content", through the same
//!   quad edit a corner drag uses (the crop becomes an Edited crop; turns and mirror are kept; the
//!   fine angle is baked into the corners, which renders the same). It recomputes the proposal on the
//!   crop as it is NOW, so no number from the webview is trusted, and refuses with `ITEM_OP` when the
//!   crop changed meanwhile, is curved, or has nothing to trim to. The crop's confidence becomes the
//!   proposal's, and the usual held/accept rules stay as they are: a trimmed crop is an edited one,
//!   which the triage counts as reviewed, never an automatic Good.
//! * A curved crop is refused with `ITEM_OP` (edit its curves, or straighten it first).

use crate::api::ItemView;
use crate::engine::{Engine, lock};
use crate::error::{ErrKind, Result};
use crate::scan::labelled;
use auto_crop_core::{Confidence, Geometry, ItemId, ItemsError, Pt, QuadWarp};
use auto_crop_imgproc::trim::{TrimReason, trim_to_content};
use serde::{Deserialize, Serialize};

/// The label of the undo step an applied trim makes.
pub const TRIM_LABEL: &str = "Trim to content";

/// Why the proposal exists, for the wording (`mat`: a wide border; `thinBorder`: a print's edge).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrimWhy {
    Mat,
    ThinBorder,
}

impl From<TrimReason> for TrimWhy {
    fn from(r: TrimReason) -> Self {
        match r {
            TrimReason::Mat => TrimWhy::Mat,
            TrimReason::ThinBorder => TrimWhy::ThinBorder,
        }
    }
}

/// A proposed trim of one crop (`Engine::trim_to_content`): the picture inside the page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrimProposal {
    /// The picture's corners TL, TR, BR, BL, normalised, in the same space as the crop's quad. What the
    /// UI draws as the dashed outline.
    pub quad: [Pt; 4],
    /// 0.5 to 0.95, UNCALIBRATED (edge agreement, fill and contrast).
    pub confidence: f32,
    pub reason: TrimWhy,
}

/// The crop's own straight quad with the fine rotation baked in, or why it has none.
fn straight_outline(g: &Geometry, dims: (u32, u32)) -> Result<QuadWarp> {
    match g {
        Geometry::Quad(q) => Ok(q.with_fine_baked(dims.0, dims.1)),
        Geometry::Curved(_) => Err(ErrKind::ItemOp),
        _ => Err(ErrKind::NoCrop),
    }
}

impl Engine {
    /// The picture inside the page of one crop, if there is one (see the module docs). `Ok(None)`
    /// while the image is not ready or when there is nothing to trim to; `Err(ItemOp)` for a curved
    /// crop, `Err(NoCrop)` for an unknown crop or one without an outline. Takes a few milliseconds
    /// to a few tens on the display proxy: call it from a worker, on demand.
    pub fn trim_to_content(&self, id: u32, crop: u32) -> Result<Option<TrimProposal>> {
        Ok(self.propose_trim(id, crop)?.map(|(_, p)| p))
    }

    /// The proposal together with the baked outline it was made for.
    fn propose_trim(&self, id: u32, crop: u32) -> Result<Option<(QuadWarp, TrimProposal)>> {
        let item = self.item(id).ok_or(ErrKind::Internal)?;
        let outline = {
            let it = lock(&item);
            if it.status != crate::api::ItemStatus::Ready {
                return Ok(None);
            }
            let st = it.history.as_ref().ok_or(ErrKind::Internal)?.current();
            let c = st.item(ItemId(crop)).ok_or(ErrKind::NoCrop)?;
            straight_outline(&c.geometry, it.dims)?
        };
        let proxy = self.proxy(id)?;
        Ok(trim_to_content(&proxy, outline.corners).map(|r| {
            (
                outline,
                TrimProposal {
                    quad: r.quad,
                    confidence: r.confidence,
                    reason: r.reason.into(),
                },
            )
        }))
    }

    /// Applies the trim of `crop` as ONE undo step (see the module docs).
    pub fn apply_trim(&self, id: u32, crop: u32) -> Result<ItemView> {
        let (outline, proposal) = self.propose_trim(id, crop)?.ok_or(ErrKind::ItemOp)?;
        let c = ItemId(crop);
        self.crop_op(
            id,
            |st| labelled(TRIM_LABEL, st, c),
            None,
            move |st, dims| {
                let now = {
                    let g = &st.item(c).ok_or(ItemsError::UnknownItem(c))?.geometry;
                    match g {
                        Geometry::Quad(q) => q.with_fine_baked(dims.0, dims.1),
                        Geometry::Curved(_) => return Err(ItemsError::Curved(c)),
                        _ => return Err(ItemsError::NotAQuad(c)),
                    }
                };
                // The crop moved since the proposal was made: nothing is applied.
                if now != outline {
                    return Err(ItemsError::UnknownItem(c));
                }
                st.edit_item_quad(
                    c,
                    QuadWarp {
                        corners: proposal.quad,
                        quarter_turns: now.quarter_turns,
                        mirror: now.mirror,
                        fine_deg: 0.0,
                    },
                )?;
                if let Some(item) = st.item_mut(c) {
                    item.confidence = Some(Confidence {
                        score: proposal.confidence,
                        forced: None,
                        reasons: Vec::new(),
                    });
                }
                Ok(())
            },
        )
    }
}
