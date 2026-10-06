// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Item operations of a multi-item scan (ROADMAP M10.18, M10.19, M10.20): add, remove or restore
//! (`include`), merge, cut, reorder, per-item turn, flip and angle, re-detection that keeps what
//! the user placed, and revert of one item. Every operation is a pure function of an
//! [`EditState`] and the oriented source size; none touches pixels, files or history, so the
//! caller commits the result as one undo step.
//!
//! Curved pages (`Geometry::Curved`) are edited only through [`EditState::set_item_curves`],
//! [`EditState::curve_item_from_quad`] and [`EditState::clear_item_curves`]. Of the operations
//! below, turn and flip handle a curved item (they only change its turns and mirror); merge, cut,
//! the corner edit and the fine angle REFUSE it with [`ItemsError::Curved`] (code `ITEM_OP`), so a
//! curved item can never silently lose its curves to a quad operation.
//!
//! Invariants every operation keeps (and the property tests check): ids are unique, an id is never
//! reused ([`EditState::alloc_item_id`]), the output order is the order of `items`, an excluded
//! item stays in the list so it can be restored, and no operation yields more than [`MAX_ITEMS`]
//! items or a quad that cannot be warped.

use crate::curve::CurveWarp;
use crate::edit::{EditState, Item, ItemId, OrderMode, Origin};
use crate::geometry::{Geometry, Pt, QuadWarp};
use crate::{Confidence, ErrKind};
use serde::{Deserialize, Serialize};

/// Most items one scan may hold (M10.67). The detector keeps the largest and raises
/// `TOO_MANY_ITEMS` beyond it; manual operations refuse to go past it.
pub const MAX_ITEMS: usize = 32;

/// A cut piece must cover at least this fraction of the scan (M10.18).
pub const MIN_PIECE_FRACTION: f64 = 0.02;

/// Why an item operation was refused. The state is never changed by a refused operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ItemsError {
    #[error("no such item {0:?}")]
    UnknownItem(ItemId),
    #[error("the item {0:?} is excluded")]
    Excluded(ItemId),
    #[error("the item {0:?} has no quad")]
    NotAQuad(ItemId),
    #[error("merging needs at least two items")]
    NeedTwo,
    #[error("a piece would cover less than 2% of the scan")]
    PieceTooSmall,
    #[error("the cut does not cross the item")]
    BadCut,
    #[error("more than 32 items")]
    TooMany,
    #[error("the quad cannot be warped")]
    Degenerate,
    #[error("the item {0:?} is not in the baseline it is reverted to")]
    NotInBaseline(ItemId),
    #[error("the source has no size")]
    NoSize,
    #[error("the item {0:?} is a curved page: edit its curves instead")]
    Curved(ItemId),
    #[error("the curves are not a valid page outline")]
    BadCurves,
}

impl ItemsError {
    /// The engine-wide code for this refusal.
    pub fn kind(self) -> ErrKind {
        match self {
            ItemsError::Degenerate | ItemsError::BadCurves => ErrKind::Degenerate,
            _ => ErrKind::ItemOp,
        }
    }
}

type P = (f64, f64);
type Dims = (u32, u32);

fn check_dims(dims: Dims) -> Result<(), ItemsError> {
    if dims.0 == 0 || dims.1 == 0 {
        Err(ItemsError::NoSize)
    } else {
        Ok(())
    }
}

// ------------------------------------------------------------------ small geometry (pixel space)

fn polygon_area(p: &[P]) -> f64 {
    let n = p.len();
    let mut s = 0.0;
    for i in 0..n {
        let (a, b) = (p[i], p[(i + 1) % n]);
        s += a.0 * b.1 - b.0 * a.1;
    }
    (s / 2.0).abs()
}

fn cross(o: P, a: P, b: P) -> f64 {
    (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
}

/// Convex hull, counter-clockwise on a y-up plane (Andrew's monotone chain). Collinear points are
/// dropped.
pub fn convex_hull(points: &[P]) -> Vec<P> {
    let mut pts: Vec<P> = points
        .iter()
        .copied()
        .filter(|p| p.0.is_finite() && p.1.is_finite())
        .collect();
    pts.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let mut hull: Vec<P> = Vec::with_capacity(pts.len() * 2);
    for &p in &pts {
        while hull.len() >= 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0 {
            hull.pop();
        }
        hull.push(p);
    }
    let lower = hull.len() + 1;
    for &p in pts.iter().rev().skip(1) {
        while hull.len() >= lower && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0 {
            hull.pop();
        }
        hull.push(p);
    }
    hull.pop();
    hull
}

/// Minimum-area enclosing rectangle of `points` (rotating calipers over the hull): the four
/// corners TL, TR, BR, BL of the rectangle whose top edge is the one closest to horizontal.
/// `None` for fewer than three non-collinear points.
pub fn min_area_rect(points: &[P]) -> Option<[P; 4]> {
    let hull = convex_hull(points);
    if hull.len() < 3 {
        return None;
    }
    let n = hull.len();
    let mut best: Option<(f64, [P; 4])> = None;
    for i in 0..n {
        let (a, b) = (hull[i], hull[(i + 1) % n]);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = dx.hypot(dy);
        if len < 1e-12 {
            continue;
        }
        let (ux, uy) = (dx / len, dy / len);
        let (vx, vy) = (-uy, ux);
        let (mut min_u, mut max_u, mut min_v, mut max_v) = (
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
        );
        for p in &hull {
            let u = (p.0 - a.0) * ux + (p.1 - a.1) * uy;
            let v = (p.0 - a.0) * vx + (p.1 - a.1) * vy;
            min_u = min_u.min(u);
            max_u = max_u.max(u);
            min_v = min_v.min(v);
            max_v = max_v.max(v);
        }
        let area = (max_u - min_u) * (max_v - min_v);
        if best.as_ref().is_none_or(|(b, _)| area < *b) {
            let at = |u: f64, v: f64| (a.0 + u * ux + v * vx, a.1 + u * uy + v * vy);
            best = Some((
                area,
                [
                    at(min_u, min_v),
                    at(max_u, min_v),
                    at(max_u, max_v),
                    at(min_u, max_v),
                ],
            ));
        }
    }
    let (_, rect) = best?;
    Some(upright_order(rect))
}

/// Orders the corners of a rectangle (in either winding) as TL, TR, BR, BL on a y-down plane so
/// that the TL to TR edge is the one closest to horizontal.
fn upright_order(r: [P; 4]) -> [P; 4] {
    // Make the winding clockwise on screen (y down): TL, TR, BR, BL.
    let mut c = r;
    let s: f64 = (0..4)
        .map(|i| c[i].0 * c[(i + 1) % 4].1 - c[(i + 1) % 4].0 * c[i].1)
        .sum();
    if s < 0.0 {
        c.reverse();
    }
    // Of the two edge directions pick the closer to horizontal as the top edge.
    let angle = |a: P, b: P| (b.1 - a.1).atan2(b.0 - a.0).to_degrees();
    let fold = |a: f64| {
        // Distance of a direction from horizontal, 0..=90.
        let a = a.rem_euclid(180.0);
        a.min(180.0 - a)
    };
    let mut best = 0usize;
    let mut best_score = f64::INFINITY;
    for k in 0..4 {
        let (tl, tr) = (c[k], c[(k + 1) % 4]);
        let ang = angle(tl, tr);
        // The top edge points right-ish (|angle| < 90) and the TL is the upper end for ties.
        let score = fold(ang) + if ang.abs() > 90.0 { 1000.0 } else { 0.0 };
        if score < best_score - 1e-9 {
            best_score = score;
            best = k;
        }
    }
    [
        c[best],
        c[(best + 1) % 4],
        c[(best + 2) % 4],
        c[(best + 3) % 4],
    ]
}

/// Sutherland-Hodgman: `subject` clipped to the convex polygon `clip` (both counter-clockwise on
/// a y-up plane).
fn clip_convex(subject: &[P], clip: &[P]) -> Vec<P> {
    let mut out: Vec<P> = subject.to_vec();
    let m = clip.len();
    for i in 0..m {
        if out.is_empty() {
            break;
        }
        let (a, b) = (clip[i], clip[(i + 1) % m]);
        let inside = |p: P| cross(a, b, p) >= -1e-12;
        let input = std::mem::take(&mut out);
        for j in 0..input.len() {
            let (p, q) = (input[j], input[(j + 1) % input.len()]);
            let (pi, qi) = (inside(p), inside(q));
            if pi {
                out.push(p);
            }
            if pi != qi {
                let (d1, d2) = (cross(a, b, p), cross(a, b, q));
                let t = d1 / (d1 - d2);
                out.push((p.0 + t * (q.0 - p.0), p.1 + t * (q.1 - p.1)));
            }
        }
    }
    out
}

/// Intersection area of the convex hulls of two point sets, in the units of the points.
pub fn overlap_area(a: &[P], b: &[P]) -> f64 {
    let (ha, hb) = (convex_hull(a), convex_hull(b));
    if ha.len() < 3 || hb.len() < 3 {
        return 0.0;
    }
    polygon_area(&clip_convex(&ha, &hb))
}

/// Intersection over union of the convex hulls of two point sets.
pub fn hull_iou(a: &[P], b: &[P]) -> f64 {
    let inter = overlap_area(a, b);
    let union = polygon_area(&convex_hull(a)) + polygon_area(&convex_hull(b)) - inter;
    if union <= 0.0 { 0.0 } else { inter / union }
}

fn item_points(it: &Item, dims: Dims) -> Vec<P> {
    match &it.geometry {
        Geometry::Quad(q) => q.corners_px(dims.0, dims.1).to_vec(),
        Geometry::Curved(c) => c.outline().corners_px(dims.0, dims.1).to_vec(),
        Geometry::Grid(g) => g.outline.iter().map(|c| c.to_px(dims.0, dims.1)).collect(),
        Geometry::Identity => {
            let (w, h) = (f64::from(dims.0), f64::from(dims.1));
            vec![(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)]
        }
    }
}

/// Axis-aligned bounds `[left, top, right, bottom]` in pixels.
fn bounds(pts: &[P]) -> [f64; 4] {
    let mut b = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    for p in pts {
        b[0] = b[0].min(p.0);
        b[1] = b[1].min(p.1);
        b[2] = b[2].max(p.0);
        b[3] = b[3].max(p.1);
    }
    b
}

/// The area of a quad as a fraction of the scan.
pub fn area_fraction(q: &QuadWarp, dims: Dims) -> f64 {
    let px = q.corners_px(dims.0, dims.1);
    polygon_area(&px) / (f64::from(dims.0) * f64::from(dims.1)).max(1.0)
}

// ------------------------------------------------------------------ reading order (M10.20)

/// Reading order of boxes `[left, top, right, bottom]`: rows are built from the items sorted by
/// vertical centre, an item joining the first row whose first (topmost) item overlaps it
/// vertically by at least 50% of the shorter of the two heights; rows run top to bottom, items
/// within a row left to right by centre x, ties by id. The result depends only on the boxes and
/// ids, never on the order they are given in. Numbering stays left to right in right-to-left
/// locales (M10.20).
pub fn reading_order(boxes: &[(ItemId, [f64; 4])]) -> Vec<ItemId> {
    struct Row {
        top: f64,
        bottom: f64,
        members: Vec<(f64, ItemId)>,
    }
    let mut idx: Vec<&(ItemId, [f64; 4])> = boxes.iter().collect();
    let cy = |b: &[f64; 4]| (b[1] + b[3]) / 2.0;
    idx.sort_by(|a, b| cy(&a.1).total_cmp(&cy(&b.1)).then(a.0.cmp(&b.0)));
    let mut rows: Vec<Row> = Vec::new();
    for (id, b) in idx {
        let (top, bottom) = (b[1], b[3]);
        let cx = (b[0] + b[2]) / 2.0;
        let found = rows.iter_mut().find(|r| {
            let overlap = bottom.min(r.bottom) - top.max(r.top);
            let shorter = (bottom - top).min(r.bottom - r.top).max(1e-9);
            overlap / shorter >= 0.5
        });
        match found {
            Some(r) => r.members.push((cx, *id)),
            None => rows.push(Row {
                top,
                bottom,
                members: vec![(cx, *id)],
            }),
        }
    }
    let mut out = Vec::with_capacity(boxes.len());
    for mut r in rows {
        r.members
            .sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        out.extend(r.members.into_iter().map(|m| m.1));
    }
    out
}

/// Where a cut crosses a quad (M10.18): `axis` says which way the cut line runs and `t0`, `t1`
/// are the fractions (0..1) along the two edges it ends on. `Vertical` ends on the top edge (`t0`,
/// left to right) and the bottom edge (`t1`, left to right) and yields a left and a right piece;
/// `Horizontal` ends on the left edge (`t0`, top to bottom) and the right edge (`t1`) and yields a
/// top and a bottom piece. Equal `t0` and `t1` give a cut straight across in the item's own frame.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cut {
    pub axis: CutAxis,
    pub t0: f64,
    pub t1: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CutAxis {
    Vertical,
    Horizontal,
}

impl Cut {
    /// "Split in halves" (M10.39).
    pub fn halves(axis: CutAxis) -> Self {
        Self {
            axis,
            t0: 0.5,
            t1: 0.5,
        }
    }
}

fn lerp(a: Pt, b: Pt, t: f64) -> Pt {
    Pt::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t)
}

impl EditState {
    fn require_quad(&self, id: ItemId) -> Result<(usize, QuadWarp), ItemsError> {
        let i = self.item_index(id).ok_or(ItemsError::UnknownItem(id))?;
        match &self.items[i].geometry {
            Geometry::Quad(q) => Ok((i, q.clone())),
            Geometry::Curved(_) => Err(ItemsError::Curved(id)),
            _ => Err(ItemsError::NotAQuad(id)),
        }
    }

    fn require_included_quad(&self, id: ItemId) -> Result<(usize, QuadWarp), ItemsError> {
        let (i, q) = self.require_quad(id)?;
        if self.items[i].include {
            Ok((i, q))
        } else {
            Err(ItemsError::Excluded(id))
        }
    }

    /// Puts the items into reading order (rows, then left to right) and numbers them that way
    /// from now on: the order mode becomes `Reading`.
    pub fn sort_reading_order(&mut self, dims: Dims) {
        let boxes: Vec<(ItemId, [f64; 4])> = self
            .items
            .iter()
            .map(|it| (it.id, bounds(&item_points(it, dims))))
            .collect();
        let order = reading_order(&boxes);
        let mut rest = std::mem::take(&mut self.items);
        for id in order {
            if let Some(p) = rest.iter().position(|i| i.id == id) {
                self.items.push(rest.remove(p));
            }
        }
        self.items.extend(rest);
    }

    fn resort_if_reading(&mut self, dims: Dims) {
        if self.split.order_mode == OrderMode::Reading {
            self.sort_reading_order(dims);
        }
    }

    /// Adds a manually placed (or detected) quad as a new item with a fresh id; `Origin::Manual`
    /// for a hand-drawn one. It is numbered by reading order unless the user ordered the items.
    pub fn add_item(
        &mut self,
        quad: QuadWarp,
        origin: Origin,
        dims: Dims,
    ) -> Result<ItemId, ItemsError> {
        check_dims(dims)?;
        if self.items.len() >= MAX_ITEMS {
            return Err(ItemsError::TooMany);
        }
        let quad = quad.sanitised();
        quad.check().map_err(|_| ItemsError::Degenerate)?;
        let id = self.alloc_item_id();
        self.items.push(Item::quad(id, quad, origin));
        self.resort_if_reading(dims);
        Ok(id)
    }

    /// Removes (`false`) or restores (`true`) an item without deleting it (M10.37).
    pub fn set_include(&mut self, id: ItemId, include: bool) -> Result<(), ItemsError> {
        self.item_mut(id)
            .ok_or(ItemsError::UnknownItem(id))?
            .include = include;
        Ok(())
    }

    /// Replaces an item's quad (a corner drag, a nudge). An auto item becomes `AutoThenEdited`.
    pub fn edit_item_quad(&mut self, id: ItemId, quad: QuadWarp) -> Result<(), ItemsError> {
        let quad = quad.sanitised();
        quad.check().map_err(|_| ItemsError::Degenerate)?;
        let it = self.item_mut(id).ok_or(ItemsError::UnknownItem(id))?;
        if it.geometry.is_curved() {
            // Replacing a curved page by a quad would silently drop its curves.
            return Err(ItemsError::Curved(id));
        }
        it.geometry = Geometry::Quad(quad);
        if matches!(it.origin, Origin::Auto { .. }) {
            it.origin = Origin::AutoThenEdited;
        }
        Ok(())
    }

    /// Applies `f` to the turns and mirror of a quad or a curved page. `f` sees a quad; for a
    /// curved page only its turns and mirror are taken back (the curves stay as they are), so `f`
    /// must not rely on the fine angle (those callers refuse a curved item first).
    fn edit_item_warp(
        &mut self,
        id: ItemId,
        f: impl FnOnce(&mut QuadWarp),
    ) -> Result<(), ItemsError> {
        let it = self.item_mut(id).ok_or(ItemsError::UnknownItem(id))?;
        match &mut it.geometry {
            Geometry::Quad(q) => f(q),
            Geometry::Curved(c) => {
                let mut view = c.outline();
                f(&mut view);
                c.quarter_turns = view.quarter_turns % 4;
                c.mirror = view.mirror;
            }
            _ => return Err(ItemsError::NotAQuad(id)),
        }
        if matches!(it.origin, Origin::Auto { .. }) {
            it.origin = Origin::AutoThenEdited;
        }
        Ok(())
    }

    /// Turns one item by a quarter turn (M10.79); the other items are untouched.
    pub fn turn_item(&mut self, id: ItemId, clockwise: bool) -> Result<(), ItemsError> {
        self.edit_item_warp(id, |q| {
            q.quarter_turns = if clockwise {
                (q.quarter_turns + 1) % 4
            } else {
                (q.quarter_turns + 3) % 4
            };
        })
    }

    /// Sets one item's fine angle in degrees (clamped to -45..=45).
    pub fn set_item_angle(&mut self, id: ItemId, deg: f32) -> Result<(), ItemsError> {
        // A curved page has no fine angle: its curves already say how it lies.
        if self.item(id).is_some_and(|i| i.geometry.is_curved()) {
            return Err(ItemsError::Curved(id));
        }
        self.edit_item_warp(id, |q| {
            q.fine_deg = if deg.is_finite() {
                deg.clamp(-45.0, 45.0)
            } else {
                0.0
            };
        })
    }

    /// Mirrors one item left to right.
    pub fn flip_item(&mut self, id: ItemId) -> Result<(), ItemsError> {
        self.edit_item_warp(id, |q| q.mirror = !q.mirror)
    }

    /// Makes `id` a curved page with these boundary curves (the corners become the curves' end
    /// points). The item must be a quad or already curved; the curves are validated
    /// ([`CurveWarp::validate`]). An auto item becomes `AutoThenEdited`.
    pub fn set_item_curves(&mut self, id: ItemId, curves: CurveWarp) -> Result<(), ItemsError> {
        curves.validate().map_err(|_| ItemsError::BadCurves)?;
        let it = self.item_mut(id).ok_or(ItemsError::UnknownItem(id))?;
        if !matches!(it.geometry, Geometry::Quad(_) | Geometry::Curved(_)) {
            return Err(ItemsError::NotAQuad(id));
        }
        let mut curves = curves;
        curves.quarter_turns %= 4;
        it.geometry = Geometry::Curved(curves);
        if matches!(it.origin, Origin::Auto { .. }) {
            it.origin = Origin::AutoThenEdited;
        }
        Ok(())
    }

    /// Turns a quad item into a curved page whose four edges are straight (the fine angle is
    /// baked into the corners); the UI then adds points. An item that is already curved is left
    /// alone.
    pub fn curve_item_from_quad(&mut self, id: ItemId, dims: Dims) -> Result<(), ItemsError> {
        check_dims(dims)?;
        let it = self.item_mut(id).ok_or(ItemsError::UnknownItem(id))?;
        let q = match &it.geometry {
            Geometry::Curved(_) => return Ok(()),
            Geometry::Quad(q) => q.with_fine_baked(dims.0, dims.1),
            _ => return Err(ItemsError::NotAQuad(id)),
        };
        q.check().map_err(|_| ItemsError::Degenerate)?;
        let curves = CurveWarp::from_quad(&q);
        curves.validate().map_err(|_| ItemsError::BadCurves)?;
        it.geometry = Geometry::Curved(curves);
        if matches!(it.origin, Origin::Auto { .. }) {
            it.origin = Origin::AutoThenEdited;
        }
        Ok(())
    }

    /// Takes a curved page back to the straight quad through its corners (its turns and mirror
    /// are kept). A quad item is left alone.
    pub fn clear_item_curves(&mut self, id: ItemId) -> Result<(), ItemsError> {
        let it = self.item_mut(id).ok_or(ItemsError::UnknownItem(id))?;
        match &it.geometry {
            Geometry::Curved(c) => it.geometry = Geometry::Quad(c.outline()),
            Geometry::Quad(_) => {}
            _ => return Err(ItemsError::NotAQuad(id)),
        }
        Ok(())
    }

    /// Merges two or more included items into one whose quad is the minimum-area rectangle of
    /// their union (M10.38). The result is `Manual`, takes the place of the first merged item and
    /// the turn and mirror of that item; the merged items are deleted (undo restores them).
    pub fn merge_items(&mut self, ids: &[ItemId], dims: Dims) -> Result<ItemId, ItemsError> {
        check_dims(dims)?;
        let mut uniq: Vec<ItemId> = Vec::new();
        for id in ids {
            if !uniq.contains(id) {
                uniq.push(*id);
            }
        }
        if uniq.len() < 2 {
            return Err(ItemsError::NeedTwo);
        }
        let mut pts: Vec<P> = Vec::new();
        let mut first: Option<(usize, QuadWarp)> = None;
        let mut overrides = Vec::new();
        for id in &uniq {
            let (i, q) = self.require_included_quad(*id)?;
            pts.extend(q.corners_px(dims.0, dims.1));
            overrides.push(self.items[i].enhance_override);
            let earlier = first.as_ref().is_none_or(|(f, _)| i < *f);
            if earlier {
                first = Some((i, q));
            }
        }
        let (at, base) = first.expect("two items");
        let rect = min_area_rect(&pts).ok_or(ItemsError::Degenerate)?;
        let mut merged = QuadWarp::from_corners_px(rect, dims.0, dims.1);
        merged.quarter_turns = base.quarter_turns;
        merged.mirror = base.mirror;
        merged.check().map_err(|_| ItemsError::Degenerate)?;
        let enhance_override = if overrides.iter().all(|o| *o == overrides[0]) {
            overrides[0]
        } else {
            None
        };
        let id = self.alloc_item_id();
        let mut item = Item::quad(id, merged, Origin::Manual);
        item.enhance_override = enhance_override;
        self.items[at] = item;
        self.items
            .retain(|it| it.id == id || !uniq.contains(&it.id));
        Ok(id)
    }

    /// Cuts an included item in two along `cut` (M10.39). Both pieces must cover at least 2% of
    /// the scan. They are `Manual`, inherit the turn, mirror, angle and enhancement of the item,
    /// and replace it in order (first piece, then second).
    pub fn split_item(
        &mut self,
        id: ItemId,
        cut: Cut,
        dims: Dims,
    ) -> Result<[ItemId; 2], ItemsError> {
        check_dims(dims)?;
        let (at, q) = self.require_included_quad(id)?;
        if self.items.len() + 1 > MAX_ITEMS {
            return Err(ItemsError::TooMany);
        }
        if !(cut.t0.is_finite() && cut.t1.is_finite())
            || !(0.0..=1.0).contains(&cut.t0)
            || !(0.0..=1.0).contains(&cut.t1)
        {
            return Err(ItemsError::BadCut);
        }
        let c = q.corners;
        let (pa, pb) = match cut.axis {
            CutAxis::Vertical => {
                let top = lerp(c[0], c[1], cut.t0);
                let bottom = lerp(c[3], c[2], cut.t1);
                (
                    [c[0], top, bottom, c[3]], // left
                    [top, c[1], c[2], bottom], // right
                )
            }
            CutAxis::Horizontal => {
                let left = lerp(c[0], c[3], cut.t0);
                let right = lerp(c[1], c[2], cut.t1);
                (
                    [c[0], c[1], right, left], // top
                    [left, right, c[2], c[3]], // bottom
                )
            }
        };
        let make = |corners: [Pt; 4]| QuadWarp {
            corners,
            ..q.clone()
        };
        let (qa, qb) = (make(pa), make(pb));
        for piece in [&qa, &qb] {
            piece.check().map_err(|_| ItemsError::PieceTooSmall)?;
            if area_fraction(piece, dims) < MIN_PIECE_FRACTION {
                return Err(ItemsError::PieceTooSmall);
            }
        }
        let over = self.items[at].enhance_override;
        let (ida, idb) = (self.alloc_item_id(), self.alloc_item_id());
        let mut ia = Item::quad(ida, qa, Origin::Manual);
        let mut ib = Item::quad(idb, qb, Origin::Manual);
        ia.enhance_override = over;
        ib.enhance_override = over;
        self.items[at] = ia;
        self.items.insert(at + 1, ib);
        Ok([ida, idb])
    }

    /// Moves an item to `to` (an index into the output order) and switches to manual ordering,
    /// which re-detection then keeps (M10.20).
    pub fn move_item(&mut self, id: ItemId, to: usize) -> Result<(), ItemsError> {
        let from = self.item_index(id).ok_or(ItemsError::UnknownItem(id))?;
        let it = self.items.remove(from);
        let to = to.min(self.items.len());
        self.items.insert(to, it);
        self.split.order_mode = OrderMode::Manual;
        Ok(())
    }

    /// Back to numbering by reading order, re-sorting now.
    pub fn use_reading_order(&mut self, dims: Dims) {
        self.split.order_mode = OrderMode::Reading;
        self.sort_reading_order(dims);
    }

    /// Reverts one item to what it is in `from` (the auto baseline, or an earlier history step):
    /// geometry, inclusion, enhancement, provenance and confidence. Every other item, the order
    /// and the whole-image settings are left exactly as they are (M10.19).
    pub fn revert_item_from(&mut self, from: &EditState, id: ItemId) -> Result<(), ItemsError> {
        let src = from.item(id).ok_or(ItemsError::NotInBaseline(id))?;
        let it = self.item_mut(id).ok_or(ItemsError::UnknownItem(id))?;
        *it = src.clone();
        Ok(())
    }

    /// Re-detection (M10.18): `detected` are the new auto items (their ids are ignored). Items the
    /// user placed or edited, and items the user removed, are kept exactly as they are; an
    /// untouched auto item is replaced by the detection that matches it (IoU >= 0.7, keeping its
    /// id) or dropped if none does; a detection that overlaps something the user kept is
    /// dropped; every other detection becomes a new item. Returns what happened.
    pub fn redetect(&mut self, detected: Vec<Item>, dims: Dims) -> Result<Redetected, ItemsError> {
        check_dims(dims)?;
        let user_kept = |it: &Item| !matches!(it.origin, Origin::Auto { .. }) || !it.include;
        let mut report = Redetected::default();
        let mut pool: Vec<Option<Item>> = detected
            .into_iter()
            .filter(|d| matches!(&d.geometry, Geometry::Quad(q) if q.check().is_ok()))
            .map(Some)
            .collect();

        // 1. Pair every untouched auto item with its best new detection.
        let mut next: Vec<Item> = Vec::with_capacity(self.items.len());
        let old = std::mem::take(&mut self.items);
        for it in old {
            if user_kept(&it) {
                report.kept += 1;
                next.push(it);
                continue;
            }
            let mine = item_points(&it, dims);
            let best = pool
                .iter()
                .enumerate()
                .filter_map(|(k, d)| {
                    d.as_ref()
                        .map(|d| (k, hull_iou(&mine, &item_points(d, dims))))
                })
                .filter(|(_, iou)| *iou >= 0.7)
                .max_by(|a, b| a.1.total_cmp(&b.1));
            match best {
                Some((k, _)) => {
                    let d = pool[k].take().expect("present");
                    next.push(Item {
                        id: it.id,
                        include: true,
                        ..d
                    });
                    report.replaced += 1;
                }
                None => report.dropped += 1,
            }
        }
        self.items = next;

        // 2. Whatever is left is new, unless it overlaps something the user kept.
        let kept_pts: Vec<Vec<P>> = self
            .items
            .iter()
            .filter(|i| user_kept(i))
            .map(|i| item_points(i, dims))
            .collect();
        let mut added: Vec<Item> = Vec::new();
        for d in pool.into_iter().flatten() {
            let pts = item_points(&d, dims);
            let area = polygon_area(&convex_hull(&pts));
            let overlaps = kept_pts.iter().any(|k| {
                let inter = overlap_area(&pts, k);
                let smaller = area.min(polygon_area(&convex_hull(k))).max(1e-9);
                hull_iou(&pts, k) >= 0.3 || inter / smaller >= 0.5
            });
            if overlaps || self.items.len() + added.len() >= MAX_ITEMS {
                report.suppressed += 1;
            } else {
                added.push(d);
            }
        }
        // New ones are numbered in reading order among themselves, then appended; a reading-order
        // state is then re-sorted as a whole, a manual one keeps its order.
        let mut staged = EditState {
            items: added,
            ..EditState::default()
        };
        for (k, it) in staged.items.iter_mut().enumerate() {
            it.id = ItemId(k as u32 + 1);
        }
        staged.sort_reading_order(dims);
        for mut it in staged.items {
            it.id = self.alloc_item_id();
            it.include = true;
            self.items.push(it);
            report.added += 1;
        }
        self.resort_if_reading(dims);
        Ok(report)
    }
}

/// What [`EditState::redetect`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Redetected {
    /// Items left exactly as the user had them (placed, edited or removed).
    pub kept: usize,
    /// Untouched auto items replaced by a matching new detection.
    pub replaced: usize,
    /// Untouched auto items that no new detection matched.
    pub dropped: usize,
    /// New detections that became items.
    pub added: usize,
    /// New detections not used: they overlap an item the user kept, or the item cap was reached.
    pub suppressed: usize,
}

/// A detection to feed [`EditState::redetect`]: an auto item with its confidence.
pub fn auto_item(quad: QuadWarp, pipeline_ver: u32, confidence: Option<Confidence>) -> Item {
    Item {
        confidence,
        ..Item::quad(ItemId(0), quad, Origin::Auto { pipeline_ver })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const DIMS: Dims = (2000, 1500);

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> QuadWarp {
        QuadWarp::new([
            Pt::new(x0, y0),
            Pt::new(x1, y0),
            Pt::new(x1, y1),
            Pt::new(x0, y1),
        ])
    }

    fn two_by_two() -> EditState {
        let mut s = EditState::default();
        // Added out of reading order on purpose.
        for (x0, y0) in [(0.55, 0.55), (0.05, 0.05), (0.55, 0.05), (0.05, 0.55)] {
            s.add_item(
                rect(x0, y0, x0 + 0.4, y0 + 0.4),
                Origin::Auto { pipeline_ver: 1 },
                DIMS,
            )
            .unwrap();
        }
        s
    }

    fn assert_invariants(s: &EditState) {
        let mut ids: Vec<u32> = s.items.iter().map(|i| i.id.0).collect();
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n, "ids are unique");
        assert!(n <= MAX_ITEMS);
        assert!(s.items.iter().all(|i| i.id < s.next_item_id()));
        for it in &s.items {
            if let Geometry::Quad(q) = &it.geometry {
                assert_eq!(q.check(), Ok(()));
            }
        }
    }

    #[test]
    fn items_added_in_any_order_are_numbered_in_reading_order() {
        let s = two_by_two();
        let tl: Vec<_> = s
            .items
            .iter()
            .map(|i| {
                let c = i.geometry.quad().unwrap().corners[0];
                (c.x, c.y)
            })
            .collect();
        assert_eq!(tl, [(0.05, 0.05), (0.55, 0.05), (0.05, 0.55), (0.55, 0.55)]);
        assert_eq!(s.split.order_mode, OrderMode::Reading);
        assert_invariants(&s);
    }

    #[test]
    fn a_manual_reorder_survives_adding_and_sets_manual_mode() {
        let mut s = two_by_two();
        let last = s.items[3].id;
        s.move_item(last, 0).unwrap();
        assert_eq!(s.split.order_mode, OrderMode::Manual);
        assert_eq!(s.items[0].id, last);
        s.add_item(rect(0.4, 0.4, 0.5, 0.5), Origin::Manual, DIMS)
            .unwrap();
        assert_eq!(s.items[0].id, last, "manual order is kept");
        s.use_reading_order(DIMS);
        assert_eq!(s.split.order_mode, OrderMode::Reading);
        assert_ne!(s.items[0].id, last);
    }

    #[test]
    fn ids_are_never_reused_even_after_a_merge() {
        let mut s = two_by_two();
        let ids: Vec<ItemId> = s.items.iter().map(|i| i.id).collect();
        let merged = s.merge_items(&ids[..2], DIMS).unwrap();
        assert!(ids.iter().all(|i| *i != merged));
        let fresh = s.add_item(rect(0.45, 0.45, 0.5, 0.5), Origin::Manual, DIMS);
        let fresh = fresh.unwrap();
        assert!(fresh > merged && !ids.contains(&fresh));
        assert_invariants(&s);
    }

    #[test]
    fn remove_and_restore_keep_the_item_and_its_position() {
        let mut s = two_by_two();
        let id = s.items[1].id;
        s.set_include(id, false).unwrap();
        assert_eq!(s.items.len(), 4);
        assert_eq!(s.output_rank(id), None);
        assert_eq!(s.output_rank(s.items[2].id), Some(2));
        s.set_include(id, true).unwrap();
        assert_eq!(s.output_rank(id), Some(2));
        assert_eq!(
            s.set_include(ItemId(999), true),
            Err(ItemsError::UnknownItem(ItemId(999)))
        );
    }

    #[test]
    fn merge_refuses_an_excluded_or_unknown_item_and_a_single_one() {
        let mut s = two_by_two();
        let (a, b) = (s.items[0].id, s.items[1].id);
        s.set_include(b, false).unwrap();
        let before = s.clone();
        assert_eq!(s.merge_items(&[a, b], DIMS), Err(ItemsError::Excluded(b)));
        assert_eq!(s.merge_items(&[a], DIMS), Err(ItemsError::NeedTwo));
        assert_eq!(s.merge_items(&[a, a], DIMS), Err(ItemsError::NeedTwo));
        assert_eq!(
            s.merge_items(&[a, ItemId(77)], DIMS),
            Err(ItemsError::UnknownItem(ItemId(77)))
        );
        assert_eq!(s, before, "a refused operation changes nothing");
    }

    #[test]
    fn two_halves_merge_back_to_the_whole_within_two_pixels() {
        let mut s = EditState::default();
        let whole = rect(0.1, 0.2, 0.7, 0.8);
        let id = s.add_item(whole.clone(), Origin::Manual, DIMS).unwrap();
        let [a, b] = s
            .split_item(id, Cut::halves(CutAxis::Vertical), DIMS)
            .unwrap();
        assert_eq!(s.items.len(), 2);
        let m = s.merge_items(&[a, b], DIMS).unwrap();
        let got = s
            .item(m)
            .unwrap()
            .geometry
            .quad()
            .unwrap()
            .corners_px(DIMS.0, DIMS.1);
        let want = whole.corners_px(DIMS.0, DIMS.1);
        for (g, w) in got.iter().zip(want) {
            assert!(
                (g.0 - w.0).abs() < 2.0 && (g.1 - w.1).abs() < 2.0,
                "{g:?} {w:?}"
            );
        }
        assert_invariants(&s);
    }

    #[test]
    fn a_cut_with_a_tiny_piece_is_refused() {
        let mut s = EditState::default();
        let id = s
            .add_item(rect(0.1, 0.1, 0.9, 0.9), Origin::Manual, DIMS)
            .unwrap();
        let before = s.clone();
        for t in [0.0, 0.01, 0.99, 1.0] {
            assert_eq!(
                s.split_item(
                    id,
                    Cut {
                        axis: CutAxis::Horizontal,
                        t0: t,
                        t1: t
                    },
                    DIMS
                ),
                Err(ItemsError::PieceTooSmall),
                "t = {t}"
            );
        }
        assert_eq!(
            s.split_item(
                id,
                Cut {
                    axis: CutAxis::Vertical,
                    t0: 1.5,
                    t1: 0.5
                },
                DIMS
            ),
            Err(ItemsError::BadCut)
        );
        assert_eq!(s, before);
    }

    #[test]
    fn a_split_item_pieces_inherit_turns_and_replace_it_in_order() {
        let mut s = two_by_two();
        let id = s.items[1].id;
        s.turn_item(id, true).unwrap();
        s.set_item_angle(id, 2.5).unwrap();
        let [a, b] = s
            .split_item(id, Cut::halves(CutAxis::Horizontal), DIMS)
            .unwrap();
        assert_eq!(s.items.len(), 5);
        assert_eq!((s.items[1].id, s.items[2].id), (a, b));
        for p in [a, b] {
            let q = s.item(p).unwrap().geometry.quad().unwrap();
            assert_eq!((q.quarter_turns, q.fine_deg), (1, 2.5));
            assert_eq!(s.item(p).unwrap().origin, Origin::Manual);
        }
        assert!(s.item(id).is_none());
    }

    #[test]
    fn per_item_turn_flip_and_angle_touch_only_that_item() {
        let mut s = two_by_two();
        let other = s.clone();
        let id = s.items[2].id;
        s.turn_item(id, false).unwrap();
        s.flip_item(id).unwrap();
        s.set_item_angle(id, f32::NAN).unwrap();
        let q = s.item(id).unwrap().geometry.quad().unwrap();
        assert_eq!((q.quarter_turns, q.mirror, q.fine_deg), (3, true, 0.0));
        assert_eq!(s.items[2].origin, Origin::AutoThenEdited);
        for k in [0, 1, 3] {
            assert_eq!(s.items[k], other.items[k]);
        }
        assert_eq!(s.items[2].id, other.items[2].id);
        assert_ne!(s.render_hash(), other.render_hash());
        assert_eq!(
            s.item_render_hash(s.items[0].id),
            other.item_render_hash(other.items[0].id)
        );
    }

    #[test]
    fn reverting_one_item_leaves_the_others_byte_identical() {
        let auto = two_by_two();
        let mut s = auto.clone();
        for it in s.items.clone() {
            s.edit_item_quad(it.id, rect(0.0, 0.0, 0.3, 0.3)).unwrap();
        }
        let id = s.items[2].id;
        let before = serde_json::to_string(&s.items).unwrap();
        s.revert_item_from(&auto, id).unwrap();
        assert_eq!(s.items[2], auto.items[2]);
        for k in [0, 1, 3] {
            assert_eq!(
                serde_json::to_string(&s.items[k]).unwrap(),
                serde_json::to_string(&serde_json::from_str::<Vec<Item>>(&before).unwrap()[k])
                    .unwrap()
            );
        }
        // An item that was never in the baseline cannot be reverted to it.
        let manual = s
            .add_item(rect(0.4, 0.4, 0.5, 0.5), Origin::Manual, DIMS)
            .unwrap();
        assert_eq!(
            s.revert_item_from(&auto, manual),
            Err(ItemsError::NotInBaseline(manual))
        );
    }

    #[test]
    fn redetect_keeps_manual_edited_and_removed_items_and_drops_overlapping_auto_ones() {
        let mut s = two_by_two();
        let (a, b, c, d) = (s.items[0].id, s.items[1].id, s.items[2].id, s.items[3].id);
        // a: edited by hand; b: removed by the user; c, d: untouched auto.
        s.edit_item_quad(a, rect(0.06, 0.06, 0.44, 0.44)).unwrap();
        s.set_include(b, false).unwrap();
        let manual = s
            .add_item(rect(0.46, 0.46, 0.54, 0.54), Origin::Manual, DIMS)
            .unwrap();
        let kept_a = s.item(a).unwrap().clone();
        let kept_b = s.item(b).unwrap().clone();
        let kept_m = s.item(manual).unwrap().clone();

        let det = |x0: f64, y0: f64, x1: f64, y1: f64| auto_item(rect(x0, y0, x1, y1), 2, None);
        let new = vec![
            det(0.05, 0.05, 0.45, 0.45), // overlaps the edited a: dropped
            det(0.55, 0.05, 0.95, 0.45), // overlaps the removed b: dropped (stays removed)
            det(0.05, 0.55, 0.45, 0.95), // matches c: replaces it, keeps its id
            det(0.46, 0.47, 0.54, 0.53), // overlaps the manual item: dropped
            det(0.02, 0.47, 0.04, 0.53), // brand new
        ];
        let r = s.redetect(new, DIMS).unwrap();
        assert_eq!(r.kept, 3, "{r:?}");
        assert_eq!(r.replaced, 1);
        assert_eq!(r.dropped, 1, "d was not found again");
        assert_eq!(r.added, 1);
        assert_eq!(r.suppressed, 3);
        assert_eq!(s.item(a), Some(&kept_a));
        assert_eq!(s.item(b), Some(&kept_b));
        assert!(!s.item(b).unwrap().include);
        assert_eq!(s.item(manual), Some(&kept_m));
        assert!(s.item(c).is_some() && s.item(d).is_none());
        assert_eq!(s.item(c).unwrap().origin, Origin::Auto { pipeline_ver: 2 });
        assert_invariants(&s);
        assert!(
            s.items.iter().any(|i| i.id.0 > 5),
            "the new one has a fresh id"
        );
    }

    #[test]
    fn redetect_in_manual_order_keeps_the_user_order() {
        let mut s = two_by_two();
        let last = s.items[3].id;
        s.move_item(last, 0).unwrap();
        let order: Vec<_> = s.items.iter().map(|i| i.id).collect();
        let same: Vec<Item> = s
            .items
            .iter()
            .map(|i| auto_item(i.geometry.quad().unwrap().clone(), 1, None))
            .collect();
        s.redetect(same, DIMS).unwrap();
        assert_eq!(s.items.iter().map(|i| i.id).collect::<Vec<_>>(), order);
    }

    #[test]
    fn at_most_32_items() {
        let mut s = EditState::default();
        for k in 0..MAX_ITEMS {
            let x = 0.01 + (k % 8) as f64 * 0.12;
            let y = 0.01 + (k / 8) as f64 * 0.24;
            s.add_item(rect(x, y, x + 0.1, y + 0.2), Origin::Manual, DIMS)
                .unwrap();
        }
        assert_eq!(
            s.add_item(rect(0.1, 0.1, 0.2, 0.2), Origin::Manual, DIMS),
            Err(ItemsError::TooMany)
        );
        assert_invariants(&s);
    }

    #[test]
    fn min_area_rect_of_a_rotated_rectangle_is_that_rectangle() {
        let (cx, cy, w, h, deg) = (500.0f64, 400.0, 300.0, 120.0, 23.0f64);
        let (s, c) = deg.to_radians().sin_cos();
        let pt = |u: f64, v: f64| (cx + u * c - v * s, cy + u * s + v * c);
        let pts = [
            pt(-w / 2.0, -h / 2.0),
            pt(w / 2.0, -h / 2.0),
            pt(w / 2.0, h / 2.0),
            pt(-w / 2.0, h / 2.0),
        ];
        let r = min_area_rect(&pts).unwrap();
        assert!((polygon_area(&r) - w * h).abs() < 1e-6);
        // The top edge is the long one at 23 degrees (closest to horizontal).
        let ang = (r[1].1 - r[0].1).atan2(r[1].0 - r[0].0).to_degrees();
        assert!((ang - 23.0).abs() < 1e-6, "{ang}");
        assert!(min_area_rect(&[(0.0, 0.0), (1.0, 1.0), (2.0, 2.0)]).is_none());
    }

    #[test]
    fn hull_iou_and_overlap_are_exact_for_rectangles() {
        let a = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let b = [(5.0, 0.0), (15.0, 0.0), (15.0, 10.0), (5.0, 10.0)];
        assert!((overlap_area(&a, &b) - 50.0).abs() < 1e-9);
        assert!((hull_iou(&a, &b) - 50.0 / 150.0).abs() < 1e-9);
        assert_eq!(
            hull_iou(&a, &[(20.0, 20.0), (30.0, 20.0), (30.0, 30.0)]),
            0.0
        );
    }

    fn grid_boxes(jitter: &[f64]) -> Vec<(ItemId, [f64; 4])> {
        // 3 rows by 4 columns, ids deliberately not in reading order.
        let mut v = Vec::new();
        let mut k = 0usize;
        for r in 0..3 {
            for c in 0..4 {
                let j = jitter[k % jitter.len()];
                let (x, y) = (100.0 + c as f64 * 220.0 + j, 100.0 + r as f64 * 220.0 - j);
                v.push((ItemId(100 - k as u32), [x, y, x + 180.0, y + 180.0]));
                k += 1;
            }
        }
        v
    }

    #[test]
    fn reading_order_is_rows_then_columns_whatever_the_input_order() {
        let boxes = grid_boxes(&[0.0]);
        let want = reading_order(&boxes);
        let expect: Vec<ItemId> = (0..12).map(|k| ItemId(100 - k)).collect();
        assert_eq!(want, expect);
        let mut rev = boxes.clone();
        rev.reverse();
        assert_eq!(reading_order(&rev), want);
        // Rows are taken by vertical overlap, not by exact y: a slightly lower box in a row stays.
        let mut skew = boxes;
        skew[1].1[1] += 60.0;
        skew[1].1[3] += 60.0;
        assert_eq!(reading_order(&skew)[..4], expect[..4]);
    }

    proptest! {
        /// Order is invariant to the order the detector reports items in, and to +-2% jitter.
        #[test]
        fn reading_order_is_invariant_to_permutation_and_small_jitter(
            seed in 0u64..1000,
            jit in proptest::collection::vec(-4.0f64..4.0, 12),
        ) {
            let boxes = grid_boxes(&[0.0]);
            let base = reading_order(&boxes);
            // A deterministic shuffle.
            let mut shuffled = boxes.clone();
            let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
            for i in (1..shuffled.len()).rev() {
                x ^= x << 13; x ^= x >> 7; x ^= x << 17;
                shuffled.swap(i, (x % (i as u64 + 1)) as usize);
            }
            prop_assert_eq!(reading_order(&shuffled), base.clone());
            // Jitter of at most 4 px on 220 px spacing is under 2%.
            let jittered = grid_boxes(&jit);
            prop_assert_eq!(reading_order(&jittered), base);
        }
    }

    #[derive(Debug, Clone)]
    enum Op {
        Add(f64, f64, f64, f64),
        Remove(usize),
        Restore(usize),
        Merge(usize, usize),
        Split(usize, bool, f64),
        Move(usize, usize),
        Turn(usize, bool),
        Flip(usize),
        Redetect(Vec<(f64, f64, f64, f64)>),
    }

    fn op() -> impl Strategy<Value = Op> {
        let f = 0.0f64..1.0;
        prop_oneof![
            (f.clone(), f.clone(), 0.04f64..0.4, 0.04f64..0.4).prop_map(|(x, y, w, h)| Op::Add(
                x * 0.6,
                y * 0.6,
                w,
                h
            )),
            (0usize..40).prop_map(Op::Remove),
            (0usize..40).prop_map(Op::Restore),
            (0usize..40, 0usize..40).prop_map(|(a, b)| Op::Merge(a, b)),
            (0usize..40, any::<bool>(), 0.2f64..0.8).prop_map(|(a, v, t)| Op::Split(a, v, t)),
            (0usize..40, 0usize..40).prop_map(|(a, b)| Op::Move(a, b)),
            (0usize..40, any::<bool>()).prop_map(|(a, c)| Op::Turn(a, c)),
            (0usize..40).prop_map(Op::Flip),
            proptest::collection::vec((f.clone(), f.clone(), 0.04f64..0.4, 0.04f64..0.4), 0..8)
                .prop_map(|v| Op::Redetect(
                    v.into_iter()
                        .map(|(x, y, w, h)| (x * 0.6, y * 0.6, w, h))
                        .collect()
                )),
        ]
    }

    fn apply(s: &mut EditState, op: &Op) {
        let pick = |s: &EditState, k: usize| s.items.get(k % s.items.len().max(1)).map(|i| i.id);
        let _ = match op {
            Op::Add(x, y, w, h) => s
                .add_item(
                    rect(*x, *y, (x + w).min(1.0), (y + h).min(1.0)),
                    Origin::Manual,
                    DIMS,
                )
                .map(|_| ()),
            Op::Remove(k) => pick(s, *k).map_or(Ok(()), |id| s.set_include(id, false)),
            Op::Restore(k) => pick(s, *k).map_or(Ok(()), |id| s.set_include(id, true)),
            Op::Merge(a, b) => match (pick(s, *a), pick(s, *b)) {
                (Some(a), Some(b)) => s.merge_items(&[a, b], DIMS).map(|_| ()),
                _ => Ok(()),
            },
            Op::Split(k, vertical, t) => pick(s, *k).map_or(Ok(()), |id| {
                let axis = if *vertical {
                    CutAxis::Vertical
                } else {
                    CutAxis::Horizontal
                };
                s.split_item(
                    id,
                    Cut {
                        axis,
                        t0: *t,
                        t1: *t,
                    },
                    DIMS,
                )
                .map(|_| ())
            }),
            Op::Move(k, to) => pick(s, *k).map_or(Ok(()), |id| s.move_item(id, *to)),
            Op::Turn(k, cw) => pick(s, *k).map_or(Ok(()), |id| s.turn_item(id, *cw)),
            Op::Flip(k) => pick(s, *k).map_or(Ok(()), |id| s.flip_item(id)),
            Op::Redetect(v) => {
                let det = v
                    .iter()
                    .map(|(x, y, w, h)| {
                        auto_item(rect(*x, *y, (x + w).min(1.0), (y + h).min(1.0)), 1, None)
                    })
                    .collect();
                s.redetect(det, DIMS).map(|_| ())
            }
        };
    }

    proptest! {
        /// Any sequence of operations keeps ids unique and never reused, the count at most 32 and
        /// every quad warpable; a refused operation changes nothing.
        #[test]
        fn any_op_sequence_keeps_the_invariants(ops in proptest::collection::vec(op(), 1..40)) {
            let mut s = EditState::default();
            let mut seen: std::collections::HashSet<u32> = Default::default();
            for o in &ops {
                let before = s.clone();
                apply(&mut s, o);
                assert_invariants(&s);
                // An id that ever existed is never handed to a different item later.
                for it in &s.items {
                    if !before.items.iter().any(|b| b.id == it.id) {
                        prop_assert!(seen.insert(it.id.0), "id {} reused after {:?}", it.id.0, o);
                    }
                }
                for it in &before.items { seen.insert(it.id.0); }
            }
        }

        /// Cut then merge returns the rectangle within 2 px, for rotated rectangles too.
        #[test]
        fn cut_then_merge_round_trips_within_two_pixels(
            cx in 0.3f64..0.7, cy in 0.3f64..0.7, w in 0.3f64..0.5, h in 0.3f64..0.5,
            deg in -30.0f64..30.0, vertical in any::<bool>(), t in 0.3f64..0.7,
        ) {
            let (sn, cs) = deg.to_radians().sin_cos();
            let (pw, ph) = (w * f64::from(DIMS.0), h * f64::from(DIMS.1));
            let (mx, my) = (cx * f64::from(DIMS.0), cy * f64::from(DIMS.1));
            let pt = |u: f64, v: f64| (mx + u * cs - v * sn, my + u * sn + v * cs);
            let px = [pt(-pw/2.0, -ph/2.0), pt(pw/2.0, -ph/2.0), pt(pw/2.0, ph/2.0), pt(-pw/2.0, ph/2.0)];
            let q = QuadWarp::from_corners_px(px, DIMS.0, DIMS.1);
            let mut s = EditState::default();
            let id = s.add_item(q.clone(), Origin::Manual, DIMS).unwrap();
            let axis = if vertical { CutAxis::Vertical } else { CutAxis::Horizontal };
            let [a, b] = s.split_item(id, Cut { axis, t0: t, t1: t }, DIMS).unwrap();
            let m = s.merge_items(&[a, b], DIMS).unwrap();
            let got = s.item(m).unwrap().geometry.quad().unwrap().corners_px(DIMS.0, DIMS.1);
            // The merged rectangle covers the same area (corner order may differ by a turn).
            let want = q.corners_px(DIMS.0, DIMS.1);
            for w in want {
                let d = got.iter().map(|g| (g.0 - w.0).hypot(g.1 - w.1)).fold(f64::INFINITY, f64::min);
                prop_assert!(d < 2.0, "corner {w:?} off by {d}");
            }
        }
    }

    proptest! {
        /// M10.17: undo and redo over item operations restore every state exactly, labels
        /// included, and a refused or no-op operation adds no entry.
        #[test]
        fn undo_and_redo_over_item_ops_restore_exact_states(ops in proptest::collection::vec(op(), 1..30)) {
            use crate::history::History;
            let mut state = EditState::default();
            let mut h = History::for_edit(state.clone());
            let mut states = vec![state.clone()];
            for (k, o) in ops.iter().enumerate() {
                apply(&mut state, o);
                if h.commit(format!("op {k}"), state.clone()) {
                    states.push(state.clone());
                }
            }
            prop_assert_eq!(h.current(), states.last().unwrap());
            let n = states.len();
            for i in (0..n - 1).rev() {
                prop_assert!(h.undo().is_some());
                prop_assert_eq!(h.current(), &states[i]);
            }
            prop_assert!(!h.can_undo());
            for st in states.iter().skip(1) {
                prop_assert!(h.redo().is_some());
                prop_assert_eq!(h.current(), st);
            }
            prop_assert!(!h.can_redo());
        }
    }

    // ---------------------------------------------------------------- curved pages

    fn bulged(x0: f64, y0: f64, x1: f64, y1: f64) -> CurveWarp {
        let c = |p: &[(f64, f64)]| {
            crate::curve::Curve::new(p.iter().map(|&(x, y)| Pt::new(x, y)).collect()).unwrap()
        };
        let (mx, my) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        CurveWarp {
            top: c(&[(x0, y0), (mx, y0 - 0.02), (x1, y0)]),
            right: c(&[(x1, y0), (x1 + 0.02, my), (x1, y1)]),
            bottom: c(&[(x1, y1), (mx, y1 + 0.02), (x0, y1)]),
            left: c(&[(x0, y1), (x0 - 0.02, my), (x0, y0)]),
            quarter_turns: 0,
            mirror: false,
        }
    }

    fn curved_two() -> EditState {
        let mut s = EditState::default();
        let a = s
            .add_item(
                rect(0.1, 0.1, 0.4, 0.4),
                Origin::Auto { pipeline_ver: 1 },
                DIMS,
            )
            .unwrap();
        s.add_item(rect(0.55, 0.1, 0.9, 0.4), Origin::Manual, DIMS)
            .unwrap();
        s.set_item_curves(a, bulged(0.1, 0.1, 0.4, 0.4)).unwrap();
        s
    }

    #[test]
    fn curves_are_set_validated_cleared_and_never_lost_by_quad_operations() {
        let mut s = curved_two();
        let (a, b) = (s.items[0].id, s.items[1].id);
        let before = s.clone();
        assert!(s.items[0].geometry.is_curved());
        // An auto item that was curved counts as edited, so a re-detection keeps it.
        assert_eq!(s.items[0].origin, Origin::AutoThenEdited);

        // Operations that would drop the curves are refused, typed, and change nothing.
        let refused: Vec<(&str, Result<(), ItemsError>)> = vec![
            ("angle", s.clone().set_item_angle(a, 5.0)),
            (
                "corner edit",
                s.clone().edit_item_quad(a, QuadWarp::inset_frame(0.2)),
            ),
            ("merge", s.clone().merge_items(&[a, b], DIMS).map(|_| ())),
            (
                "cut",
                s.clone()
                    .split_item(a, Cut::halves(CutAxis::Vertical), DIMS)
                    .map(|_| ()),
            ),
        ];
        for (name, r) in refused {
            assert_eq!(r, Err(ItemsError::Curved(a)), "{name}");
            assert_eq!(r.unwrap_err().kind(), ErrKind::ItemOp, "{name}");
        }
        assert_eq!(s, before, "refused operations change nothing");

        // Turn and flip are handled: only the turns and mirror change, the curves stay.
        let curves = s.items[0].geometry.curves().unwrap().clone();
        s.turn_item(a, true).unwrap();
        s.flip_item(a).unwrap();
        let after = s.items[0].geometry.curves().unwrap();
        assert_eq!((after.quarter_turns, after.mirror), (1, true));
        assert_eq!(after.top, curves.top);
        assert_eq!(after.left, curves.left);
        s.turn_item(a, false).unwrap();
        assert_eq!(s.items[0].geometry.curves().unwrap().quarter_turns, 0);

        // Invalid curves are refused (corners that do not meet).
        let mut bad = bulged(0.1, 0.1, 0.4, 0.4);
        bad.right = crate::curve::Curve::new(vec![
            Pt::new(0.4, 0.1),
            Pt::new(0.45, 0.2),
            Pt::new(0.41, 0.4),
        ])
        .unwrap();
        assert_eq!(s.set_item_curves(a, bad), Err(ItemsError::BadCurves));
        assert_eq!(ItemsError::BadCurves.kind(), ErrKind::Degenerate);

        // Clearing gives the straight quad through the corners, turns kept.
        s.turn_item(a, true).unwrap();
        s.clear_item_curves(a).unwrap();
        let q = s.items[0].geometry.quad().unwrap();
        assert_eq!(q.corners[2], Pt::new(0.4, 0.4));
        assert_eq!(q.quarter_turns, 1);
        // And a quad can be made curved again, straight, with the fine angle baked in.
        let mut f = EditState::single(QuadWarp::inset_frame(0.1));
        f.set_item_angle(ItemId(1), 3.0).unwrap();
        f.curve_item_from_quad(ItemId(1), DIMS).unwrap();
        let c = f.items[0].geometry.curves().unwrap();
        assert!(c.is_straight() && c.validate().is_ok());
        assert_ne!(
            c.corners()[0],
            Pt::new(0.1, 0.1),
            "the angle moved the corners"
        );
        // Idempotent on a curved item.
        let again = f.clone();
        f.curve_item_from_quad(ItemId(1), DIMS).unwrap();
        assert_eq!(f, again);
    }

    #[test]
    fn a_redetection_keeps_a_curved_item_and_the_ids_are_stable() {
        let mut s = curved_two();
        let curved_id = s.items[0].id;
        let det = vec![auto_item(rect(0.1, 0.1, 0.4, 0.4), 1, None)];
        s.redetect(det, DIMS).unwrap();
        let it = s.item(curved_id).unwrap();
        assert!(
            it.geometry.is_curved(),
            "the user's curves survive re-detection"
        );
        assert_invariants(&s);
    }

    #[test]
    fn a_curved_item_is_never_approved_by_triage() {
        use crate::confidence::Confidence;
        use crate::triage::{STRICT_CUTOFF, ScanTriage, scan_triage};
        let mut s = EditState::default();
        let a = s
            .add_item(
                rect(0.1, 0.1, 0.9, 0.9),
                Origin::Auto { pipeline_ver: 1 },
                DIMS,
            )
            .unwrap();
        s.item_mut(a).unwrap().confidence = Some(Confidence {
            score: 0.99,
            forced: None,
            reasons: vec![],
        });
        assert_eq!(scan_triage(&s, STRICT_CUTOFF), ScanTriage::Approved);
        s.set_item_curves(a, bulged(0.1, 0.1, 0.9, 0.9)).unwrap();
        assert_eq!(
            scan_triage(&s, STRICT_CUTOFF),
            ScanTriage::HeldForReview {
                items_need_check: 1
            }
        );
        // Even a manual curved item, and in the only-item case.
        s.item_mut(a).unwrap().origin = Origin::Manual;
        assert_eq!(
            scan_triage(&s, STRICT_CUTOFF),
            ScanTriage::HeldForReview {
                items_need_check: 1
            }
        );
    }
}
