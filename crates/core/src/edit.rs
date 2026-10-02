// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `EditState` v1 (PLAN 2.3): the parametric description of what to do to one source image.
//! History holds these, never pixels, so undo and redo are cheap and rendering stays a pure
//! function of (source, `EditState`, engine version).

use crate::confidence::Confidence;
use crate::error::CoreError;
use crate::geometry::{Geometry, QuadWarp};
use serde::{Deserialize, Serialize};

/// Current schema version of [`EditState`]. Migrations are pure functions `v(n) -> v(n+1)` that
/// live next to the JSON handling in the engine (`auto_crop_engine::migrate`).
pub const EDIT_STATE_VERSION: u32 = 1;

/// Stable id of an item across edits and undo (not render-relevant).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ItemId(pub u32);

/// Quarter turns and mirror of the WHOLE image, on top of EXIF (which is applied once, at
/// decode). Used for identity geometry and convert-only jobs; per-item turns live in the item's
/// geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Orient {
    pub quarter_turns: u8,
    pub mirror: bool,
}

impl Orient {
    pub const IDENTITY: Orient = Orient {
        quarter_turns: 0,
        mirror: false,
    };

    pub fn is_identity(self) -> bool {
        self.quarter_turns.is_multiple_of(4) && !self.mirror
    }
}

/// Margin policy around the paper (B14). v1 has only the paper-edge default; `ContentTight` and
/// `None` arrive additively with the content bbox (M7+).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MarginPolicy {
    /// Quad at 0 plus the engine's per-route guard; `margin` is the extra fraction of the shorter
    /// side (0 = the paper edge itself).
    PaperEdge { margin: f32 },
}

impl Default for MarginPolicy {
    fn default() -> Self {
        MarginPolicy::PaperEdge { margin: 0.0 }
    }
}

/// Enhancement (B13): suggested, never forced. v1 has only `Original`; the modes and their
/// parameters (PLAN 5.2) are additive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "camelCase")]
pub enum Enhance {
    #[default]
    Original,
}

/// Where an item came from. Provenance: render ignores it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Origin {
    Auto {
        #[serde(rename = "pipelineVer")]
        pipeline_ver: u32,
    },
    #[default]
    Manual,
    AutoThenEdited,
}

/// One cut-out of the source. A multi-item scan has several (B1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Item {
    pub id: ItemId,
    /// Drop a false detection without deleting it.
    pub include: bool,
    pub geometry: Geometry,
    pub enhance_override: Option<Enhance>,
    /// Provenance: render ignores it.
    pub origin: Origin,
    /// Provenance: render ignores it.
    pub confidence: Option<Confidence>,
}

impl Default for Item {
    fn default() -> Self {
        Self {
            id: ItemId(1),
            include: true,
            geometry: Geometry::Identity,
            enhance_override: None,
            origin: Origin::Manual,
            confidence: None,
        }
    }
}

impl Item {
    pub fn quad(id: ItemId, quad: QuadWarp, origin: Origin) -> Self {
        Self {
            id,
            geometry: Geometry::Quad(quad),
            origin,
            ..Self::default()
        }
    }
}

/// Versioned, serialisable edit parameters (PLAN 2.3). `items` is empty for an image with no crop
/// (the original is left untouched, or a convert-only job).
///
/// Unknown JSON fields are rejected on purpose: data from another schema goes through the
/// engine's `migrate`, which either upgrades it or refuses it as too new; it must never be
/// half-read and written back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct EditState {
    pub version: u32,
    pub orientation: Orient,
    pub items: Vec<Item>,
    pub margin: MarginPolicy,
    /// Default enhancement for all items.
    pub enhance: Enhance,
}

impl Default for EditState {
    fn default() -> Self {
        Self {
            version: EDIT_STATE_VERSION,
            orientation: Orient::IDENTITY,
            items: Vec::new(),
            margin: MarginPolicy::default(),
            enhance: Enhance::Original,
        }
    }
}

impl EditState {
    /// A one-item state cropping to `quad`, authored by hand.
    pub fn single(quad: QuadWarp) -> Self {
        Self::single_with(quad, Origin::Manual, None)
    }

    /// A one-item state with explicit provenance.
    pub fn single_with(quad: QuadWarp, origin: Origin, confidence: Option<Confidence>) -> Self {
        Self {
            items: vec![Item {
                confidence,
                ..Item::quad(ItemId(1), quad, origin)
            }],
            ..Self::default()
        }
    }

    /// The first included item that has a quad: what the single-image UI edits.
    pub fn quad(&self) -> Option<&QuadWarp> {
        self.items
            .iter()
            .filter(|i| i.include)
            .find_map(|i| i.geometry.quad())
    }

    /// Mutable access to the same quad as [`EditState::quad`].
    pub fn quad_mut(&mut self) -> Option<&mut QuadWarp> {
        self.items
            .iter_mut()
            .filter(|i| i.include)
            .find_map(|i| match &mut i.geometry {
                Geometry::Quad(q) => Some(q),
                _ => None,
            })
    }

    /// Items that render an output.
    pub fn included(&self) -> impl Iterator<Item = &Item> {
        self.items.iter().filter(|i| i.include)
    }

    /// Rejects states written by a newer schema.
    pub fn validate(&self) -> Result<(), CoreError> {
        if self.version > EDIT_STATE_VERSION {
            Err(CoreError::UnsupportedVersion(self.version))
        } else {
            Ok(())
        }
    }

    /// A 64-bit hash of the render-relevant fields only (PLAN 2.3): geometry, orientation, margin,
    /// enhancement, item order and inclusion. Provenance (`origin`, `confidence`), item ids and the
    /// schema version do not contribute, so re-scoring an item does not re-render it. The value is
    /// stable across runs, platforms and releases of the same schema (FNV-1a over a fixed byte
    /// layout), so it may be persisted and used to dedupe results.
    pub fn render_hash(&self) -> u64 {
        let mut h = Fnv::new();
        h.u8(self.orientation.quarter_turns % 4);
        h.bool(self.orientation.mirror);
        h.margin(&self.margin);
        h.enhance(&self.enhance);
        h.u64(self.items.len() as u64);
        for it in &self.items {
            h.bool(it.include);
            match &it.geometry {
                Geometry::Identity => h.u8(0),
                Geometry::Quad(q) => {
                    h.u8(1);
                    for c in &q.corners {
                        h.f64(c.x);
                        h.f64(c.y);
                    }
                    h.u8(q.quarter_turns % 4);
                    h.bool(q.mirror);
                    h.f32(q.fine_deg);
                }
                Geometry::Grid(g) => {
                    h.u8(2);
                    for c in &g.outline {
                        h.f64(c.x);
                        h.f64(c.y);
                    }
                    h.u32(u32::from(g.cols));
                    h.u32(u32::from(g.rows));
                    h.u64(g.nodes.len() as u64);
                    for c in &g.nodes {
                        h.f64(c.x);
                        h.f64(c.y);
                    }
                    h.u8(g.quarter_turns % 4);
                    h.bool(g.mirror);
                }
            }
            match &it.enhance_override {
                None => h.u8(0),
                Some(e) => {
                    h.u8(1);
                    h.enhance(e);
                }
            }
        }
        h.finish()
    }
}

/// FNV-1a, 64 bit. Stable by construction (no `std::hash` randomness, no layout dependence).
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 ^= u64::from(x);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    fn u8(&mut self, v: u8) {
        self.bytes(&[v]);
    }
    fn bool(&mut self, v: bool) {
        self.u8(u8::from(v));
    }
    fn u32(&mut self, v: u32) {
        self.bytes(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }
    /// Negative zero and zero hash alike; every NaN hashes alike.
    fn f64(&mut self, v: f64) {
        let v = if v == 0.0 {
            0.0
        } else if v.is_nan() {
            f64::NAN
        } else {
            v
        };
        self.u64(v.to_bits());
    }
    fn f32(&mut self, v: f32) {
        let v = if v == 0.0 {
            0.0
        } else if v.is_nan() {
            f32::NAN
        } else {
            v
        };
        self.u32(v.to_bits());
    }
    fn margin(&mut self, m: &MarginPolicy) {
        match m {
            MarginPolicy::PaperEdge { margin } => {
                self.u8(0);
                self.f32(*margin);
            }
        }
    }
    fn enhance(&mut self, e: &Enhance) {
        match e {
            Enhance::Original => self.u8(0),
        }
    }
    fn finish(&self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::confidence::{Forced, Reason, ReasonCode};
    use crate::geometry::Pt;
    use std::sync::Arc;

    fn sample() -> EditState {
        EditState::single_with(
            QuadWarp::inset_frame(0.05),
            Origin::Auto { pipeline_ver: 1 },
            Some(Confidence {
                score: 0.91,
                forced: None,
                reasons: vec![],
            }),
        )
    }

    #[test]
    fn default_state_is_current_version_and_valid() {
        let s = EditState::default();
        assert_eq!(s.version, EDIT_STATE_VERSION);
        assert_eq!(s.validate(), Ok(()));
        assert!(s.items.is_empty() && s.quad().is_none());
        assert!(s.orientation.is_identity());
        assert_eq!(s.margin, MarginPolicy::PaperEdge { margin: 0.0 });
        assert_eq!(s.enhance, Enhance::Original);
    }

    #[test]
    fn newer_version_is_rejected() {
        let s = EditState {
            version: EDIT_STATE_VERSION + 1,
            ..EditState::default()
        };
        assert_eq!(
            s.validate(),
            Err(CoreError::UnsupportedVersion(EDIT_STATE_VERSION + 1))
        );
    }

    #[test]
    fn edit_state_survives_a_json_round_trip_and_missing_fields() {
        let s = sample();
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<EditState>(&json).unwrap(), s);
        // serde(default): an old or partial record still loads.
        assert_eq!(
            serde_json::from_str::<EditState>("{}").unwrap(),
            EditState::default()
        );
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["items"][0]["geometry"]["type"], "quad");
        assert_eq!(v["margin"]["kind"], "paperEdge");
        assert_eq!(v["enhance"]["mode"], "original");
        assert_eq!(v["items"][0]["origin"]["kind"], "auto");
    }

    #[test]
    fn unknown_fields_are_refused_not_dropped() {
        // The pre-items shape of the early slice must go through `migrate`, never be half-read.
        let legacy = r#"{"version":1,"geometry":{"corners":[],"quarterTurns":0,"fineDeg":0.0}}"#;
        assert!(serde_json::from_str::<EditState>(legacy).is_err());
    }

    #[test]
    fn corner_change_alters_the_render_hash_provenance_does_not() {
        let base = sample();
        let h0 = base.render_hash();
        assert_eq!(h0, sample().render_hash(), "deterministic");

        // A moved corner changes it, however small.
        let mut moved = base.clone();
        moved.quad_mut().unwrap().corners[2].x += 1e-9;
        assert_ne!(moved.render_hash(), h0);

        // Every render-relevant field changes it.
        let mut e = base.clone();
        e.quad_mut().unwrap().fine_deg = 0.5;
        assert_ne!(e.render_hash(), h0);
        let mut e = base.clone();
        e.quad_mut().unwrap().quarter_turns = 1;
        assert_ne!(e.render_hash(), h0);
        let mut e = base.clone();
        e.quad_mut().unwrap().mirror = true;
        assert_ne!(e.render_hash(), h0);
        let mut e = base.clone();
        e.items[0].include = false;
        assert_ne!(e.render_hash(), h0);
        let mut e = base.clone();
        e.orientation.quarter_turns = 2;
        assert_ne!(e.render_hash(), h0);
        let mut e = base.clone();
        e.margin = MarginPolicy::PaperEdge { margin: 0.01 };
        assert_ne!(e.render_hash(), h0);
        let mut e = base.clone();
        e.items[0].geometry = Geometry::Identity;
        assert_ne!(e.render_hash(), h0);

        // Provenance, ids and version do not.
        let mut p = base.clone();
        p.items[0].origin = Origin::AutoThenEdited;
        p.items[0].confidence = Some(Confidence {
            score: 0.1,
            forced: Some(Forced::Failed),
            reasons: vec![Reason {
                code: ReasonCode::WeakEdge,
                side: None,
            }],
        });
        p.items[0].id = ItemId(77);
        p.version = 99;
        assert_eq!(p.render_hash(), h0);
        p.items[0].confidence = None;
        p.items[0].origin = Origin::Manual;
        assert_eq!(p.render_hash(), h0);
    }

    #[test]
    fn render_hash_normalises_turns_and_negative_zero() {
        let mut a = sample();
        let mut b = sample();
        a.quad_mut().unwrap().quarter_turns = 0;
        b.quad_mut().unwrap().quarter_turns = 4;
        assert_eq!(a.render_hash(), b.render_hash());
        a.quad_mut().unwrap().corners[0] = Pt::new(0.0, 0.0);
        b.quad_mut().unwrap().corners[0] = Pt::new(-0.0, -0.0);
        assert_eq!(a.render_hash(), b.render_hash());
    }

    #[test]
    fn render_hash_has_a_pinned_value() {
        // Guards the byte layout: a change here invalidates persisted dedupe keys.
        assert_eq!(
            EditState::default().render_hash(),
            9_808_874_869_469_701_221
        );
    }

    #[test]
    fn item_order_and_grid_payload_matter() {
        let mut two = EditState::single(QuadWarp::inset_frame(0.1));
        two.items.push(Item::quad(
            ItemId(2),
            QuadWarp::inset_frame(0.3),
            Origin::Manual,
        ));
        let mut swapped = two.clone();
        swapped.items.swap(0, 1);
        assert_ne!(two.render_hash(), swapped.render_hash());

        let g = crate::geometry::GridWarp {
            cols: 1,
            rows: 1,
            nodes: vec![Pt::new(0.5, 0.5)],
            ..Default::default()
        };
        let mut a = EditState::default();
        a.items.push(Item {
            geometry: Geometry::Grid(Arc::new(g.clone())),
            ..Item::default()
        });
        let mut b = a.clone();
        if let Geometry::Grid(arc) = &mut b.items[0].geometry {
            Arc::make_mut(arc).nodes[0] = Pt::new(0.5, 0.6);
        }
        assert_ne!(a.render_hash(), b.render_hash());
    }

    #[test]
    fn quad_accessors_skip_excluded_items() {
        let mut s = EditState::single(QuadWarp::inset_frame(0.2));
        s.items[0].include = false;
        assert!(s.quad().is_none() && s.quad_mut().is_none());
        assert_eq!(s.included().count(), 0);
    }
}
