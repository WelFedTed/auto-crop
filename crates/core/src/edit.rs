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
///
/// Version 2 (M10.17) adds [`SplitState`]: the split policy, profile, order mode and the next free
/// [`ItemId`]. A version 1 document migrates by adding the default `split` block, so nothing in it
/// is lost or reinterpreted.
///
/// Version 3 (curved pages, `docs/dev/curved-pages.md`) adds the geometry variant `curved`. Nothing
/// in an older document changes meaning, so v2 -> v3 only raises the version; the bump exists so an
/// older build refuses a document that holds curves (`SchemaTooNew`) instead of half-reading it.
pub const EDIT_STATE_VERSION: u32 = 3;

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

/// Whether a scan may be split into several items (PLAN 4.7, `split` of `AnalyzeOptions`). `Never`
/// is the single-item behaviour of M2.16 and is what a state written before M10 means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SplitPolicy {
    /// Split when the scan looks like several items on a bed; the result is held for review
    /// unless the user has opted in to auto-saving splits (0.x preview rule, M10.29).
    Auto,
    /// Look for several items even where the scan does not look bed-like (such a scan is held).
    Always,
    /// One item per file: no splitting.
    #[default]
    Never,
}

/// What the items on a scan are (M10.12): Photos keep the orientation they were placed in,
/// Receipts are made upright by the orientation net.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SplitProfile {
    #[default]
    Photos,
    Receipts,
}

/// How the items are numbered (M10.20).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OrderMode {
    /// Reading order: rows by vertical overlap, then left to right.
    #[default]
    Reading,
    /// The user reordered the items; re-detection keeps their order.
    Manual,
}

/// Multi-item bookkeeping that render ignores (M10.17). The order of `EditState::items` IS the
/// output order (`{n}` numbers the included items in that order), so there is no per-item order
/// field that could disagree with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct SplitState {
    pub policy: SplitPolicy,
    pub profile: SplitProfile,
    pub order_mode: OrderMode,
    /// The next id to hand out. An id is never reused for the life of the state; 0 means "derive
    /// it from the items" ([`EditState::next_item_id`] never goes below that).
    pub next_id: u32,
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
    /// Split policy, profile, order mode and id allocator (M10.17). Not render-relevant.
    pub split: SplitState,
}

impl Default for EditState {
    fn default() -> Self {
        Self {
            version: EDIT_STATE_VERSION,
            orientation: Orient::IDENTITY,
            items: Vec::new(),
            margin: MarginPolicy::default(),
            enhance: Enhance::Original,
            split: SplitState::default(),
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

    /// The straight outline of the first included item that is a quad or a curved page.
    pub fn outline_quad(&self) -> Option<QuadWarp> {
        self.included().find_map(|i| i.geometry.outline_quad())
    }

    /// An included item is a curved page: it is held for review until the user accepts it.
    pub fn has_curved(&self) -> bool {
        self.included().any(|i| i.geometry.is_curved())
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

    /// The item with this id, included or not.
    pub fn item(&self, id: ItemId) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    pub fn item_mut(&mut self, id: ItemId) -> Option<&mut Item> {
        self.items.iter_mut().find(|i| i.id == id)
    }

    /// Position of the item with this id in the output order.
    pub fn item_index(&self, id: ItemId) -> Option<usize> {
        self.items.iter().position(|i| i.id == id)
    }

    /// The id the next new item gets: above every id in use and above every id handed out so far,
    /// so an id is never reused (M10.17).
    pub fn next_item_id(&self) -> ItemId {
        let used = self.items.iter().map(|i| i.id.0).max().unwrap_or(0);
        ItemId(self.split.next_id.max(used.saturating_add(1)).max(1))
    }

    /// Hands out a fresh id and records it.
    pub fn alloc_item_id(&mut self) -> ItemId {
        let id = self.next_item_id();
        self.split.next_id = id.0.saturating_add(1);
        id
    }

    /// The 1-based output rank (`{n}`) of an included item, or `None` if it is excluded or absent.
    pub fn output_rank(&self, id: ItemId) -> Option<usize> {
        self.included().position(|i| i.id == id).map(|p| p + 1)
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
            h.geometry(&it.geometry);
            h.enhance_override(&it.enhance_override);
        }
        h.finish()
    }

    /// A 64-bit hash of what decides the pixels of ONE item's output (M10.28): the whole-image
    /// orientation and margin, the effective enhancement (the item's override, else the default)
    /// and the item's geometry. Inclusion, position, id and provenance do not contribute, so
    /// editing item 2, excluding item 3 or reordering leaves item 1's key (and its cache) alone.
    /// `None` if there is no such item.
    pub fn item_render_hash(&self, id: ItemId) -> Option<u64> {
        let it = self.item(id)?;
        let mut h = Fnv::new();
        h.u8(0xA5); // domain separator: never equal to a whole-state hash by construction
        h.u8(self.orientation.quarter_turns % 4);
        h.bool(self.orientation.mirror);
        h.margin(&self.margin);
        h.enhance(it.enhance_override.as_ref().unwrap_or(&self.enhance));
        h.geometry(&it.geometry);
        Some(h.finish())
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
    fn geometry(&mut self, g: &Geometry) {
        match g {
            Geometry::Identity => self.u8(0),
            Geometry::Quad(q) => {
                self.u8(1);
                for c in &q.corners {
                    self.f64(c.x);
                    self.f64(c.y);
                }
                self.u8(q.quarter_turns % 4);
                self.bool(q.mirror);
                self.f32(q.fine_deg);
            }
            Geometry::Curved(c) => {
                self.u8(3);
                for curve in c.curves() {
                    self.u64(curve.points().len() as u64);
                    for p in curve.points() {
                        self.f64(p.x);
                        self.f64(p.y);
                    }
                }
                self.u8(c.quarter_turns % 4);
                self.bool(c.mirror);
            }
            Geometry::Grid(g) => {
                self.u8(2);
                for c in &g.outline {
                    self.f64(c.x);
                    self.f64(c.y);
                }
                self.u32(u32::from(g.cols));
                self.u32(u32::from(g.rows));
                self.u64(g.nodes.len() as u64);
                for c in &g.nodes {
                    self.f64(c.x);
                    self.f64(c.y);
                }
                self.u8(g.quarter_turns % 4);
                self.bool(g.mirror);
            }
        }
    }
    fn enhance_override(&mut self, e: &Option<Enhance>) {
        match e {
            None => self.u8(0),
            Some(e) => {
                self.u8(1);
                self.enhance(e);
            }
        }
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

    #[test]
    fn split_state_defaults_serialise_and_ids_are_never_reused() {
        let s = EditState::default();
        assert_eq!(
            s.split.policy,
            SplitPolicy::Never,
            "no splitting unless asked"
        );
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["version"], EDIT_STATE_VERSION);
        assert_eq!(v["split"]["policy"], "never");
        assert_eq!(v["split"]["orderMode"], "reading");
        // A document without the block still loads (serde(default)).
        let no_split: EditState = serde_json::from_str(r#"{"version":2}"#).unwrap();
        assert_eq!(no_split.split, SplitState::default());

        let mut s = EditState::single(QuadWarp::inset_frame(0.1));
        assert_eq!(s.next_item_id(), ItemId(2));
        let a = s.alloc_item_id();
        let b = s.alloc_item_id();
        assert_eq!((a, b), (ItemId(2), ItemId(3)));
        // Nothing was inserted, yet the ids are not handed out again.
        assert_eq!(s.next_item_id(), ItemId(4));
        // Items with high ids push the counter up.
        s.items.push(Item {
            id: ItemId(50),
            ..Item::default()
        });
        assert_eq!(s.next_item_id(), ItemId(51));
    }

    #[test]
    fn item_render_hash_ignores_other_items_inclusion_order_and_provenance() {
        let mut s = EditState::single(QuadWarp::inset_frame(0.1));
        s.items.push(Item::quad(
            ItemId(2),
            QuadWarp::inset_frame(0.3),
            Origin::Manual,
        ));
        let h1 = s.item_render_hash(ItemId(1)).unwrap();
        let h2 = s.item_render_hash(ItemId(2)).unwrap();
        assert_ne!(h1, h2);
        assert_eq!(s.item_render_hash(ItemId(9)), None);

        let mut t = s.clone();
        t.items[1].include = false; // another item excluded
        t.items.swap(0, 1); // reordered
        t.items[1].origin = Origin::AutoThenEdited;
        t.items[1].confidence = None;
        assert_eq!(t.item_render_hash(ItemId(1)), Some(h1));
        // Editing item 2 changes item 2's key only.
        let mut e = s.clone();
        e.quad_mut().unwrap(); // item 1
        e.items[1].geometry = Geometry::Quad(QuadWarp::inset_frame(0.35));
        assert_eq!(e.item_render_hash(ItemId(1)), Some(h1));
        assert_ne!(e.item_render_hash(ItemId(2)), Some(h2));
        // The whole-image settings and the effective enhancement are part of every key.
        let mut o = s.clone();
        o.orientation.quarter_turns = 1;
        assert_ne!(o.item_render_hash(ItemId(1)), Some(h1));
        let mut m = s.clone();
        m.margin = MarginPolicy::PaperEdge { margin: 0.02 };
        assert_ne!(m.item_render_hash(ItemId(2)), Some(h2));
    }

    // ---------------------------------------------------------------- curved pages (schema v3)

    fn curve_pts(p: &[(f64, f64)]) -> crate::curve::Curve {
        crate::curve::Curve::new(p.iter().map(|&(x, y)| Pt::new(x, y)).collect()).unwrap()
    }

    fn curved_state() -> EditState {
        let mut s = EditState::single(QuadWarp::new([
            Pt::new(0.1, 0.1),
            Pt::new(0.9, 0.1),
            Pt::new(0.9, 0.9),
            Pt::new(0.1, 0.9),
        ]));
        let c = crate::curve::CurveWarp {
            top: curve_pts(&[(0.1, 0.1), (0.5, 0.06), (0.9, 0.1)]),
            right: curve_pts(&[(0.9, 0.1), (0.94, 0.5), (0.9, 0.9)]),
            bottom: curve_pts(&[(0.9, 0.9), (0.5, 0.95), (0.1, 0.9)]),
            left: curve_pts(&[(0.1, 0.9), (0.07, 0.5), (0.1, 0.1)]),
            quarter_turns: 1,
            mirror: false,
        };
        s.set_item_curves(ItemId(1), c).unwrap();
        s
    }

    #[test]
    fn the_schema_is_v3_and_a_curved_state_round_trips_through_json() {
        assert_eq!(EDIT_STATE_VERSION, 3);
        let s = curved_state();
        assert_eq!(s.version, 3);
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<EditState>(&json).unwrap(), s);
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["items"][0]["geometry"]["type"], "curved");
        assert_eq!(
            v["items"][0]["geometry"]["top"].as_array().unwrap().len(),
            3
        );
        assert_eq!(v["items"][0]["geometry"]["quarterTurns"], 1);
        assert!(s.has_curved());
        assert!(s.quad().is_none(), "a curved page is not a plain quad");
        let o = s.outline_quad().unwrap();
        assert_eq!((o.corners[0], o.quarter_turns), (Pt::new(0.1, 0.1), 1));
    }

    #[test]
    fn curves_are_part_of_the_render_hash_and_of_the_item_key() {
        let base = curved_state();
        let h0 = base.render_hash();
        let k0 = base.item_render_hash(ItemId(1)).unwrap();
        // Moving one interior control point by a hair changes both hashes.
        let mut moved = base.clone();
        if let Geometry::Curved(c) = &mut moved.items[0].geometry {
            let mut p = c.top.points().to_vec();
            p[1].y += 1e-9;
            c.top = crate::curve::Curve::new(p).unwrap();
        }
        assert_ne!(moved.render_hash(), h0);
        assert_ne!(moved.item_render_hash(ItemId(1)).unwrap(), k0);
        // Turns and mirror too; a straight page differs from a curved one.
        let mut turned = base.clone();
        if let Geometry::Curved(c) = &mut turned.items[0].geometry {
            c.quarter_turns = 2;
        }
        assert_ne!(turned.render_hash(), h0);
        let mut straight = base.clone();
        straight.clear_item_curves(ItemId(1)).unwrap();
        assert_ne!(straight.render_hash(), h0);
        // A curved page and its quad never collide: the tags differ.
        let quad_only = EditState::single(QuadWarp::inset_frame(0.1));
        assert_ne!(quad_only.render_hash(), h0);
        // Provenance still does not count.
        let mut p = base.clone();
        p.items[0].origin = Origin::AutoThenEdited;
        assert_eq!(p.render_hash(), h0);
    }

    #[test]
    fn undo_and_redo_restore_the_curves_exactly() {
        use crate::history::History;
        let quad_state = EditState::single(QuadWarp::inset_frame(0.1));
        let mut h = History::for_edit(quad_state.clone());
        let mut next = quad_state.clone();
        next.curve_item_from_quad(ItemId(1), (4000, 3000)).unwrap();
        assert!(h.commit("Curve", next.clone()), "adding curves is a change");
        let mut bent = next.clone();
        let c = curved_state().items[0].geometry.curves().unwrap().clone();
        bent.set_item_curves(ItemId(1), c).unwrap();
        assert!(h.commit("Bend", bent.clone()));
        assert_eq!(h.undo().unwrap(), &next);
        assert!(h.current().has_curved());
        assert_eq!(h.undo().unwrap(), &quad_state);
        assert!(!h.current().has_curved());
        assert_eq!(h.redo().unwrap(), &next);
        assert_eq!(h.redo().unwrap(), &bent);
    }

    proptest::proptest! {
        /// Any valid curved state survives save and load, byte for byte on the second pass.
        #[test]
        fn a_curved_state_round_trips(
            ys in proptest::collection::vec(0.0f64..0.04, 4 * 5),
            n in 2usize..6,
            turns in 0u8..4,
            mirror in proptest::bool::ANY,
        ) {
            use crate::curve::{Curve, CurveWarp};
            let edge = |a: (f64, f64), b: (f64, f64), k: usize| -> Curve {
                let mut p = vec![Pt::new(a.0, a.1)];
                for i in 1..n - 1 {
                    let t = i as f64 / (n - 1) as f64;
                    // Bow the interior points sideways a little.
                    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
                    let (nx, ny) = (-dy, dx);
                    let off = ys[(k * 5 + i) % ys.len()];
                    p.push(Pt::new(a.0 + dx * t + nx * off, a.1 + dy * t + ny * off));
                }
                p.push(Pt::new(b.0, b.1));
                Curve::new(p).unwrap()
            };
            let (tl, tr, br, bl) = ((0.1, 0.1), (0.9, 0.12), (0.88, 0.9), (0.12, 0.88));
            let warp = CurveWarp {
                top: edge(tl, tr, 0),
                right: edge(tr, br, 1),
                bottom: edge(br, bl, 2),
                left: edge(bl, tl, 3),
                quarter_turns: turns,
                mirror,
            };
            proptest::prop_assert_eq!(warp.validate(), Ok(()));
            let mut s = EditState::single(QuadWarp::new([
                Pt::new(tl.0, tl.1), Pt::new(tr.0, tr.1), Pt::new(br.0, br.1), Pt::new(bl.0, bl.1),
            ]));
            s.set_item_curves(ItemId(1), warp).unwrap();
            let json = serde_json::to_string(&s).unwrap();
            let back: EditState = serde_json::from_str(&json).unwrap();
            proptest::prop_assert_eq!(&back, &s);
            proptest::prop_assert_eq!(serde_json::to_string(&back).unwrap(), json);
            proptest::prop_assert_eq!(back.render_hash(), s.render_hash());
        }
    }
}
