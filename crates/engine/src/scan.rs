// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Multi-item scans in the engine (ROADMAP M10): the crops of an image as views, the operations
//! on them (each one undo step, labelled with the item), per-crop rendering and cache keys, the
//! scan-level hold rule, and the group save (N files from one scan, all or nothing).
//!
//! Vocabulary: the engine's registry entry is an *image* (what the UI calls an item in the grid);
//! the things cut out of it are *crops* (`EditState::items`, `Item` in `core`). IDs of crops are
//! the stable `ItemId`s of the edit state.

use crate::api::*;
use crate::engine::{
    Engine, Item as Image, JPEG_PREVIEW_QUALITY, JPEG_SAVE_QUALITY, RESULT_EDGE, SavedRec,
    Snapshot, THUMB_EDGE, jpeg, lock, stat_of,
};
use crate::error::{ErrKind, Result, codec_err};
use crate::fsplan::{
    DEFAULT_TEMPLATE_1, DEFAULT_TEMPLATE_N, OnCollision, PlanError, PlanInput, ReservedKeys,
    expand_name, path_key,
};
use crate::group::{
    BackupPlan, FaultHook, GroupError, GroupRequest, NoFaults, OutSpec, Produced, Retire,
    SourceFingerprint, commit_group,
};
use crate::source::hash_file;
use crate::store::NewBackup;
use crate::util::blake3_hex;
use auto_crop_codecs::{Format, MAX_PIXELS, decode, encode};
use auto_crop_core::{
    Band, Cut, EditState, GestureId, ItemId, ItemsError, Origin, Pt, QuadWarp, STRICT_CUTOFF,
    ScanTriage, SplitPolicy, scan_triage,
};
use auto_crop_imgproc::render::{Limits, render_quad};
use auto_crop_imgproc::scale::resize_to_fit;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

/// One output of a split save, as the image remembers it.
#[derive(Debug, Clone)]
pub(crate) struct GroupOut {
    #[allow(dead_code)] // kept for the Backups panel and diagnostics
    pub(crate) item_id: u32,
    #[allow(dead_code)]
    pub(crate) index: u32,
    pub(crate) path: PathBuf,
    pub(crate) snap: Snapshot,
}

/// Which crop image the UI wants (M10.28).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CropImage {
    Thumb,
    Result,
}

fn hash_hex(p: &Path) -> Option<String> {
    hash_file(p).ok().map(|id| id.to_hex())
}

fn crop_origin(o: &Origin) -> CropOrigin {
    match o {
        Origin::Auto { .. } => CropOrigin::Auto,
        Origin::Manual => CropOrigin::Manual,
        Origin::AutoThenEdited => CropOrigin::AutoThenEdited,
    }
}

/// The views of every crop of `st` and the scan-level split view (called from `Image::view`).
pub(crate) fn crop_views(img: &Image, st: &EditState) -> (Vec<CropView>, Option<SplitView>) {
    let total = st
        .included()
        .filter(|i| i.geometry.quad().is_some())
        .count();
    let stem = img
        .path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "image".to_owned());
    let ext = img.format.extension();
    let crops = st
        .items
        .iter()
        .map(|it| {
            let quad = it.geometry.quad();
            let order = st.output_rank(it.id).unwrap_or(0) as u32;
            let baseline = img.auto.as_ref().and_then(|a| a.item(it.id));
            let reviewed = !matches!(it.origin, Origin::Auto { .. });
            let band = if reviewed {
                Some(Band::Good)
            } else {
                it.confidence.as_ref().map(|c| c.band(STRICT_CUTOFF))
            };
            let output_name = (order > 0 && quad.is_some())
                .then(|| {
                    let (tpl, rank) = if total > 1 {
                        (DEFAULT_TEMPLATE_N, Some((order as usize, total)))
                    } else {
                        (DEFAULT_TEMPLATE_1, None)
                    };
                    expand_name(tpl, &stem, rank, ext).ok()
                })
                .flatten();
            CropView {
                id: it.id.0,
                order,
                include: it.include,
                edit: quad.map(Edit::from),
                auto_edit: baseline.and_then(|b| b.geometry.quad()).map(Edit::from),
                mirror: quad.is_some_and(|q| q.mirror),
                origin: crop_origin(&it.origin),
                confidence: it.confidence.clone(),
                band,
                edited: baseline
                    .is_none_or(|b| b.geometry != it.geometry || b.include != it.include),
                output_name,
                render_key: format!("{:016x}", crop_key(st, it.id).unwrap_or(0)),
            }
        })
        .collect();
    let split = SplitView {
        policy: st.split.policy,
        profile: st.split.profile,
        order_mode: st.split.order_mode,
        triage: scan_triage(st, STRICT_CUTOFF),
        accepted: img.accepted == Some(st.render_hash()),
        is_split: total >= 2 || img.saved.as_ref().is_some_and(|s| !s.group.is_empty()),
        included: total,
    };
    (crops, Some(split))
}

/// The cache key of one crop's pixels (M10.28): what decides them (`item_render_hash`) mixed with
/// the crop's id, so the key changes when this crop is edited and only then.
fn crop_key(st: &EditState, id: ItemId) -> Option<u64> {
    let h = st.item_render_hash(id)?;
    Some(h.rotate_left(17) ^ u64::from(id.0).wrapping_mul(0x9E37_79B9_7F4A_7C15))
}

/// "Move corner (item 2)": the history label names the item (M10.19). The number is the item's
/// output rank, or its place in the list while it is excluded; a single-crop image keeps the
/// plain label.
fn labelled(label: &str, st: &EditState, crop: ItemId) -> String {
    let multi = st.items.len() > 1;
    // The output rank while included; the place in the list while excluded.
    let who = match (st.output_rank(crop), st.item_index(crop)) {
        (Some(n), _) => n.to_string(),
        (None, Some(i)) => (i + 1).to_string(),
        (None, None) => format!("#{}", crop.0),
    };
    let s = if multi {
        format!("{label} (item {who})")
    } else {
        label.to_owned()
    };
    s.chars().take(60).collect()
}

impl Engine {
    /// Applies `f` to a copy of the current state and commits the result as one undo step
    /// (`gesture` coalesces a drag). A refused operation changes nothing and says why.
    fn crop_op(
        &self,
        id: u32,
        label: impl FnOnce(&EditState) -> String,
        gesture: Option<GestureId>,
        f: impl FnOnce(&mut EditState, (u32, u32)) -> std::result::Result<(), ItemsError>,
    ) -> Result<ItemView> {
        self.with_history(id, |it| {
            let h = it.history.as_mut().expect("checked");
            let mut st = h.current().clone();
            let before = st.clone();
            f(&mut st, it.dims).map_err(ItemsError::kind)?;
            // The label may name an item that the operation removed, so it reads the old state.
            let label = label(&before);
            if h.commit_gesture(label, st, gesture) {
                it.generation += 1;
            }
            Ok(it.view())
        })?
    }

    /// Edits one crop's quad and angle (a corner drag, a nudge, the angle ruler). `live` returns
    /// the view without recording anything; `gesture` makes a drag one undo step per crop
    /// (the gesture id includes the crop id, M10.19).
    pub fn set_crop_edit(
        &self,
        id: u32,
        crop: u32,
        edit: &Edit,
        live: bool,
        label: &str,
        gesture: Option<u64>,
    ) -> Result<ItemView> {
        let crop = ItemId(crop);
        if live {
            return self.with_history(id, |it| {
                let st = it.history.as_ref().expect("checked").current();
                let q = st
                    .item(crop)
                    .and_then(|c| c.geometry.quad())
                    .ok_or(ErrKind::ItemOp)?;
                let mut v = it.view();
                v.edit = Some(Edit::from(&edit.apply_to(q)));
                Ok(v)
            })?;
        }
        let g = gesture.map(|g| GestureId((g << 32) ^ u64::from(crop.0)));
        self.crop_op(
            id,
            |st| labelled(label, st, crop),
            g,
            |st, _| {
                let q = st
                    .item(crop)
                    .ok_or(ItemsError::UnknownItem(crop))?
                    .geometry
                    .quad()
                    .ok_or(ItemsError::NotAQuad(crop))?
                    .clone();
                st.edit_item_quad(crop, edit.apply_to(&q))
            },
        )
    }

    /// Adds a crop the detector missed (M10.37): `quad` if the UI drew one; else the item at
    /// `at` as the detector finds it (snapped to the edges), else a box a fifth of the frame
    /// wide and high around `at`; else the frame inset 20%.
    pub fn add_crop(&self, id: u32, quad: Option<[Pt; 4]>, at: Option<Pt>) -> Result<ItemView> {
        let found = match (quad, at) {
            (Some(q), _) => Some(q),
            (None, Some(p)) => {
                let detector = lock(&self.inner.detector).clone();
                let raster = self.proxy(id)?;
                detector.detect_at(&raster, p)
            }
            _ => None,
        };
        let q = match (found, at) {
            (Some(q), _) => QuadWarp::new(q),
            (None, Some(p)) => {
                let (hw, hh) = (0.1, 0.1);
                let (x0, x1) = ((p.x - hw).clamp(0.0, 0.8), (p.x + hw).clamp(0.2, 1.0));
                let (y0, y1) = ((p.y - hh).clamp(0.0, 0.8), (p.y + hh).clamp(0.2, 1.0));
                QuadWarp::new([
                    Pt::new(x0, y0),
                    Pt::new(x1, y0),
                    Pt::new(x1, y1),
                    Pt::new(x0, y1),
                ])
            }
            (None, None) => QuadWarp::inset_frame(0.2),
        };
        self.crop_op(
            id,
            |_| "Add item".to_owned(),
            None,
            |st, dims| st.add_item(q, Origin::Manual, dims).map(|_| ()),
        )
    }

    /// Removes a crop without deleting it (it can be restored, M10.37).
    pub fn remove_crop(&self, id: u32, crop: u32) -> Result<ItemView> {
        let c = ItemId(crop);
        self.crop_op(
            id,
            |st| labelled("Remove", st, c),
            None,
            |st, _| st.set_include(c, false),
        )
    }

    pub fn restore_crop(&self, id: u32, crop: u32) -> Result<ItemView> {
        let c = ItemId(crop);
        self.crop_op(
            id,
            |st| labelled("Restore", st, c),
            None,
            |st, _| st.set_include(c, true),
        )
    }

    /// Merges crops into the minimum-area rectangle of their union (M10.38).
    pub fn merge_crops(&self, id: u32, crops: &[u32]) -> Result<ItemView> {
        let ids: Vec<ItemId> = crops.iter().copied().map(ItemId).collect();
        self.crop_op(
            id,
            |_| format!("Merge {} items", ids.len()),
            None,
            |st, dims| st.merge_items(&ids, dims).map(|_| ()),
        )
    }

    /// Cuts a crop in two (M10.39).
    pub fn cut_crop(&self, id: u32, crop: u32, cut: Cut) -> Result<ItemView> {
        let c = ItemId(crop);
        self.crop_op(
            id,
            |st| labelled("Cut", st, c),
            None,
            |st, dims| st.split_item(c, cut, dims).map(|_| ()),
        )
    }

    /// Moves a crop in the output order; the order becomes manual and survives re-detection.
    pub fn move_crop(&self, id: u32, crop: u32, to_index: usize) -> Result<ItemView> {
        let c = ItemId(crop);
        self.crop_op(
            id,
            |st| labelled("Move", st, c),
            None,
            |st, _| st.move_item(c, to_index),
        )
    }

    /// Back to reading-order numbering.
    pub fn use_reading_order(&self, id: u32) -> Result<ItemView> {
        self.crop_op(
            id,
            |_| "Reading order".to_owned(),
            None,
            |st, dims| {
                st.use_reading_order(dims);
                Ok(())
            },
        )
    }

    /// Turns one crop by a quarter turn (M10.79).
    pub fn turn_crop(&self, id: u32, crop: u32, clockwise: bool) -> Result<ItemView> {
        let c = ItemId(crop);
        self.crop_op(
            id,
            |st| labelled(if clockwise { "Turn right" } else { "Turn left" }, st, c),
            None,
            |st, _| st.turn_item(c, clockwise),
        )
    }

    pub fn set_crop_angle(
        &self,
        id: u32,
        crop: u32,
        deg: f32,
        gesture: Option<u64>,
    ) -> Result<ItemView> {
        let c = ItemId(crop);
        let g = gesture.map(|g| GestureId((g << 32) ^ u64::from(c.0)));
        self.crop_op(
            id,
            |st| labelled("Straighten", st, c),
            g,
            |st, _| st.set_item_angle(c, deg),
        )
    }

    pub fn flip_crop(&self, id: u32, crop: u32) -> Result<ItemView> {
        let c = ItemId(crop);
        self.crop_op(
            id,
            |st| labelled("Flip", st, c),
            None,
            |st, _| st.flip_item(c),
        )
    }

    /// Reverts one crop to the detector's proposal or to an earlier step; no other crop changes
    /// (M10.19).
    pub fn revert_crop(&self, id: u32, crop: u32, to: RevertTo) -> Result<ItemView> {
        let c = ItemId(crop);
        let baseline: EditState = self.with_history(id, |it| match to {
            RevertTo::Auto => it.auto.clone().ok_or(ErrKind::ItemOp),
            RevertTo::Step { position } => it
                .history
                .as_ref()
                .expect("checked")
                .state_at(position)
                .cloned()
                .ok_or(ErrKind::ItemOp),
        })??;
        self.crop_op(
            id,
            |st| labelled("Revert", st, c),
            None,
            |st, _| st.revert_item_from(&baseline, c),
        )
    }

    /// Changes the split policy or profile of an image and runs detection again (M10.40):
    /// `Never` is "Treat as one item" (the single-item result replaces the crops), anything else
    /// keeps what the user placed or edited and refreshes the rest. One undo step.
    pub fn redetect(&self, id: u32, patch: SplitPatch) -> Result<ItemView> {
        let raster = self.proxy(id)?;
        let (policy, profile) = self.with_history(id, |it| {
            let s = it.history.as_ref().expect("checked").current().split;
            (
                patch.policy.unwrap_or(s.policy),
                patch.profile.unwrap_or(s.profile),
            )
        })?;
        let detector = lock(&self.inner.detector).clone();
        let split = (policy != SplitPolicy::Never)
            .then(|| detector.detect(&raster, policy, profile))
            .flatten()
            .filter(|d| d.items.len() >= 2);
        let single = if split.is_none() {
            let d = auto_crop_imgproc::detect::detect(&raster);
            d.quad.map(|q| {
                let mut quad = QuadWarp::new(q);
                quad.quarter_turns = 0;
                auto_crop_core::items::auto_item(quad, 1, None)
            })
        } else {
            None
        };
        let label = match (policy, patch.policy) {
            (SplitPolicy::Never, Some(_)) => "Treat as one item",
            (_, Some(_)) => "Split into items",
            _ => "Re-detect items",
        };
        self.crop_op(
            id,
            |_| label.to_owned(),
            None,
            move |st, dims| {
                st.split.policy = policy;
                st.split.profile = profile;
                match split {
                    Some(d) => {
                        let items = d
                            .items
                            .into_iter()
                            .map(|i| {
                                let mut q = QuadWarp::new(i.quad);
                                q.quarter_turns = i.quarter_turns % 4;
                                auto_crop_core::items::auto_item(q, 1, Some(i.confidence))
                            })
                            .collect();
                        st.redetect(items, dims).map(|_| ())
                    }
                    None => {
                        // One item, as the single-item route sees it: it replaces the auto
                        // crops, and keeps what the user placed or edited.
                        st.redetect(single.into_iter().collect(), dims).map(|_| ())
                    }
                }
            },
        )
    }

    /// The same split change for several images (the grid menu and batch actions, M10.40), as ONE
    /// undo step of the session: [`Engine::session_undo`] reverts all of them (M10.19).
    pub fn redetect_many(&self, ids: &[u32], patch: SplitPatch) -> Vec<(u32, Result<ItemView>)> {
        let label = match patch.policy {
            Some(SplitPolicy::Never) => "Treat as one item",
            Some(_) => "Split into items",
            None => "Re-detect items",
        };
        let mut cmd = auto_crop_core::SessionCmd::new(format!("{label} ({} images)", ids.len()));
        let results: Vec<(u32, Result<ItemView>)> = ids
            .iter()
            .map(|id| {
                let before = self.item_view(*id).map(|v| v.history_position);
                let r = self.redetect(*id, patch);
                if let (Some(b), Ok(v)) = (before, &r) {
                    cmd.mark(*id, b, v.history_position);
                }
                (*id, r)
            })
            .collect();
        lock(&self.inner.session).push(cmd);
        results
    }

    /// Undoes the last multi-image command: every affected image goes back to the history
    /// position it had before it. Returns the command's label and the new views.
    pub fn session_undo(&self) -> Option<(String, Vec<ItemView>)> {
        self.session_step(true)
    }

    pub fn session_redo(&self) -> Option<(String, Vec<ItemView>)> {
        self.session_step(false)
    }

    fn session_step(&self, undo: bool) -> Option<(String, Vec<ItemView>)> {
        let mut moves: Vec<(u32, usize)> = Vec::new();
        let label = {
            let mut s = lock(&self.inner.session);
            let l = if undo {
                s.undo(|id, pos| moves.push((*id, pos)))
            } else {
                s.redo(|id, pos| moves.push((*id, pos)))
            };
            l.map(str::to_owned)?
        };
        let views = moves
            .into_iter()
            .filter_map(|(id, pos)| {
                self.with_history(id, |it| {
                    if it.history.as_mut().expect("checked").seek(pos) {
                        it.generation += 1;
                    }
                    it.view()
                })
                .ok()
            })
            .collect();
        Some((label, views))
    }

    /// The user looked at this scan and accepts it as it is now (M10.29): a split scan that
    /// triage holds may be saved while its state stays exactly this one. Any later edit needs a
    /// new acceptance.
    pub fn accept_scan(&self, id: u32) -> Result<ItemView> {
        self.with_history(id, |it| {
            it.accepted = Some(
                it.history
                    .as_ref()
                    .expect("checked")
                    .current()
                    .render_hash(),
            );
            it.view()
        })
    }

    /// Withdraws an acceptance.
    pub fn unaccept_scan(&self, id: u32) -> Result<ItemView> {
        self.with_history(id, |it| {
            it.accepted = None;
            it.view()
        })
    }

    // ------------------------------------------------------------------ per-crop pixels

    /// Encoded bytes of one crop for the UI (M10.28). The cache key is the image id, the kind and
    /// the crop's own render hash, so editing item 2 leaves item 1's entry alone, and the same
    /// crop is never rendered twice. A panic while rendering fails this call only.
    pub fn crop_image_bytes(
        &self,
        id: u32,
        crop: u32,
        kind: CropImage,
    ) -> Result<(Arc<Vec<u8>>, &'static str)> {
        let item = self.item(id).ok_or(ErrKind::Internal)?;
        let (state, ready) = {
            let it = lock(&item);
            (
                it.history.as_ref().map(|h| h.current().clone()),
                it.status == crate::api::ItemStatus::Ready,
            )
        };
        let state = state.filter(|_| ready).ok_or(ErrKind::Internal)?;
        let c = ItemId(crop);
        let quad = state
            .item(c)
            .and_then(|i| i.geometry.quad())
            .cloned()
            .ok_or(ErrKind::NoCrop)?;
        let hash = crop_key(&state, c).ok_or(ErrKind::NoCrop)?;
        let key = (id, 0x40 | kind as u8, hash);
        if let Some(b) = lock(&self.inner.renders).get(&key) {
            return Ok((b, "image/jpeg"));
        }
        let proxy = self.proxy(id)?;
        let (edge, quality) = match kind {
            CropImage::Result => (RESULT_EDGE, JPEG_PREVIEW_QUALITY),
            CropImage::Thumb => (THUMB_EDGE * 2, 80),
        };
        let bytes = crate::run_isolated(std::panic::AssertUnwindSafe(|| -> Result<Vec<u8>> {
            let out = render_quad(
                &proxy,
                &quad,
                Limits {
                    max_pixels: u64::MAX,
                    max_edge: edge,
                },
            )
            .map_err(|_| ErrKind::NoCrop)?;
            let out = if kind == CropImage::Thumb {
                resize_to_fit(&out, THUMB_EDGE)
            } else {
                out
            };
            jpeg(&out, quality)
        }))
        .unwrap_or(Err(ErrKind::InternalPanic))?;
        let bytes = Arc::new(bytes);
        lock(&self.inner.renders).put(key, bytes.clone());
        Ok((bytes, "image/jpeg"))
    }

    // ------------------------------------------------------------------ saving

    /// The ids of other open images' paths and of split saves in flight: names nobody may plan.
    fn reserved_names(&self, except: u32) -> ReservedKeys {
        let mut r = lock(&self.inner.reserved).clone();
        let items: Vec<_> = lock(&self.inner.items)
            .iter()
            .filter(|(k, _)| **k != except)
            .map(|(_, v)| v.clone())
            .collect();
        for i in items {
            let it = lock(&i);
            r.insert(&it.path);
            if let Some(s) = &it.saved {
                r.insert(&s.output_path);
                for g in &s.group {
                    r.insert(&g.path);
                }
            }
        }
        r
    }

    /// Saves one image: one file the way it always was, or, when it has two or more crops (or was
    /// split before), several as one group.
    pub(crate) fn save_dispatch(
        &self,
        id: u32,
        target: SaveTarget,
        run_id: &str,
        run_name: &str,
    ) -> SaveOutcome {
        self.save_dispatch_with(id, target, run_id, run_name, &NoFaults)
    }

    /// A panic anywhere in a save fails that scan only (M10.67). What it interrupted is finished or
    /// undone from the journal at the next start; the folder is never left half-written.
    pub(crate) fn save_dispatch_with(
        &self,
        id: u32,
        target: SaveTarget,
        run_id: &str,
        run_name: &str,
        hook: &dyn FaultHook,
    ) -> SaveOutcome {
        crate::run_isolated(std::panic::AssertUnwindSafe(|| {
            self.save_dispatch_inner(id, target, run_id, run_name, hook)
        }))
        .unwrap_or_else(|_| SaveOutcome::failed(id, ErrKind::InternalPanic))
    }

    fn save_dispatch_inner(
        &self,
        id: u32,
        target: SaveTarget,
        run_id: &str,
        run_name: &str,
        hook: &dyn FaultHook,
    ) -> SaveOutcome {
        // Two or more crops are a group. So is a single crop that follows a group saved the same
        // way (a split scan edited down to one item: the scan is gone, its pixels are in the
        // backup, and the old set has to be retired). A single crop after a group saved the other
        // way is an ordinary single save (replace in place, or one copy).
        let copy = target == SaveTarget::Copy;
        let group = self.item(id).is_some_and(|i| {
            let it = lock(&i);
            it.status == ItemStatus::Ready
                && (it.is_split()
                    || it
                        .saved
                        .as_ref()
                        .is_some_and(|s| !s.group.is_empty() && s.copy == copy))
        });
        if group {
            return self.save_group(id, target, run_id, run_name, hook);
        }
        match self.save_one(id, target, run_id, run_name) {
            Ok(saved) => SaveOutcome {
                id,
                ok: true,
                error: None,
                saved: Some(saved),
                notes: Vec::new(),
                notices: Vec::new(),
            },
            Err(e) => SaveOutcome::failed(id, e),
        }
    }

    /// Test hook: [`Engine::save_items`] with a fault hook in the group commit (it can fail or
    /// "crash" steps; it cannot skip the backup or the verification).
    #[doc(hidden)]
    pub fn save_items_with_faults(
        &self,
        ids: &[u32],
        target: SaveTarget,
        run_name: &str,
        hook: &dyn FaultHook,
    ) -> Vec<SaveOutcome> {
        let run_id = crate::util::new_id();
        ids.iter()
            .map(|id| self.save_dispatch_with(*id, target, &run_id, run_name, hook))
            .collect()
    }

    fn save_group(
        &self,
        id: u32,
        target: SaveTarget,
        run_id: &str,
        run_name: &str,
        hook: &dyn FaultHook,
    ) -> SaveOutcome {
        match self.save_group_inner(id, target, run_id, run_name, hook) {
            Ok(o) => o,
            Err((e, notices)) => SaveOutcome {
                notices,
                ..SaveOutcome::failed(id, e)
            },
        }
    }

    fn save_group_inner(
        &self,
        id: u32,
        target: SaveTarget,
        run_id: &str,
        run_name: &str,
        hook: &dyn FaultHook,
    ) -> std::result::Result<SaveOutcome, (ErrKind, Vec<String>)> {
        let plain = |e: ErrKind| (e, Vec::new());
        let item = self.item(id).ok_or(plain(ErrKind::Internal))?;
        let (path, original_path, state, fmt, snap, orig_mtime_ms, saved, icc, frames, accepted) = {
            let it = lock(&item);
            if it.status != crate::api::ItemStatus::Ready {
                return Err(plain(ErrKind::Internal));
            }
            let state = it
                .history
                .as_ref()
                .ok_or(plain(ErrKind::Internal))?
                .current()
                .clone();
            (
                it.path.clone(),
                it.original_path.clone(),
                state,
                it.format,
                it.snapshot.clone(),
                it.orig_mtime_ms,
                it.saved.clone(),
                it.icc.clone(),
                it.frames,
                it.accepted,
            )
        };
        let crops: Vec<(ItemId, QuadWarp)> = state
            .included()
            .filter_map(|i| i.geometry.quad().map(|q| (i.id, q.clone())))
            .collect();
        if crops.is_empty() {
            return Err(plain(ErrKind::NoCrop));
        }
        let mut notices: Vec<String> = Vec::new();

        // The 0.x preview rule (M10.29): a scan that would become several files is written only
        // when every crop is Good at the Strict cutoff and the owner has switched automatic
        // splitting on (Experimental), or when the user looked and accepted this exact state.
        // Otherwise nothing is written.
        if crops.len() >= 2 {
            let settings = self.settings();
            let approved = settings.auto_save_splits
                && scan_triage(&state, STRICT_CUTOFF) == ScanTriage::Approved;
            let user_accepted = accepted == Some(state.render_hash());
            if !(approved || user_accepted) {
                return Err((ErrKind::HeldForReview, vec!["split.held".to_owned()]));
            }
        }

        // What may be replaced (PLAN 2.7): single-frame sources this build can write back.
        let out_fmt = match target {
            SaveTarget::Replace => {
                if frames > 1 {
                    return Err((ErrKind::NotReplaceable, vec!["tiff.multi_page".to_owned()]));
                }
                if !fmt.is_encodable() {
                    return Err((
                        ErrKind::NotReplaceable,
                        vec!["format.write_unavailable".to_owned()],
                    ));
                }
                fmt
            }
            // A copy is written in a format this build can write: JPEG for HEIC (the conversion
            // target of PLAN 3.5), PNG for everything else.
            SaveTarget::Copy => {
                if fmt.is_encodable() {
                    fmt
                } else if fmt == Format::Heic {
                    Format::Jpeg
                } else {
                    Format::Png
                }
            }
        };

        // The pixels: the file before the first save, the backup afterwards.
        let bytes = fs::read(&original_path).map_err(|e| {
            plain(if e.kind() == std::io::ErrorKind::NotFound {
                if saved.is_some() {
                    ErrKind::OriginalExpired
                } else {
                    ErrKind::SourceChanged
                }
            } else {
                ErrKind::from_io(&e)
            })
        })?;
        if original_path == path && blake3_hex(&bytes) != snap.blake3 {
            return Err(plain(ErrKind::SourceChanged));
        }
        let decoded = decode(&bytes).map_err(|e| plain(codec_err(e)))?;
        drop(bytes);

        // Names: planned together, then re-checked inside the commit.
        let parent = path.parent().ok_or(plain(ErrKind::Internal))?.to_path_buf();
        let (dir, copy) = match target {
            SaveTarget::Replace => (parent, false),
            SaveTarget::Copy => (parent.join("AutoCrop"), true),
        };
        // Previous outputs of this image (one file or a group): unchanged ones may be replaced,
        // an edited one never is.
        let previous: Vec<(PathBuf, String)> = match &saved {
            Some(s) if !s.group.is_empty() => s
                .group
                .iter()
                .map(|g| (g.path.clone(), g.snap.blake3.clone()))
                .collect(),
            Some(s) => vec![(s.output_path.clone(), s.out.blake3.clone())],
            None => Vec::new(),
        };
        // Only outputs in the folder this save writes to can be taken over.
        let previous: Vec<(PathBuf, String)> = previous
            .into_iter()
            .filter(|(p, _)| p.parent() == Some(dir.as_path()))
            .collect();
        let mut unchanged: Vec<(PathBuf, String)> = Vec::new();
        let mut user_edited = false;
        for (p, h) in &previous {
            match hash_hex(p) {
                Some(cur) if cur == *h => unchanged.push((p.clone(), h.clone())),
                Some(_) => user_edited = true,
                None => {}
            }
        }
        if user_edited {
            notices.push("derived.user_edited".to_owned());
        }
        let own: Vec<PathBuf> = if user_edited {
            Vec::new()
        } else {
            unchanged.iter().map(|(p, _)| p.clone()).collect()
        };

        let mut attempt = 0;
        let (plan, _reservation) = loop {
            attempt += 1;
            let reserved = self.reserved_names(id);
            let plan = crate::fsplan::plan_group(&PlanInput {
                source: &path,
                dir: &dir,
                count: crops.len(),
                template: None,
                ext: out_fmt.extension(),
                on_collision: OnCollision::Rename,
                reserved: &reserved,
                own: &own,
            })
            .map_err(|e: PlanError| plain(e.kind()))?;
            // Phase 2 and the reservation in one step, so two scans never plan the same name.
            match Reservation::take(self, &plan.paths, &own) {
                Ok(r) => break (plan, r),
                Err(_) if attempt < 4 => continue,
                Err(e) => return Err(plain(e)),
            }
        };
        plan.recheck(&ReservedKeys::default(), &own)
            .map_err(plain)?;

        let replaces: std::collections::HashMap<String, String> = unchanged
            .iter()
            .filter(|(p, _)| own.iter().any(|o| path_key(o) == path_key(p)))
            .map(|(p, h)| (path_key(p), h.clone()))
            .collect();
        let outputs: Vec<OutSpec> = crops
            .iter()
            .enumerate()
            .map(|(i, (cid, _))| OutSpec {
                item_id: cid.0,
                index: i as u32 + 1,
                final_path: plan.paths[i].clone(),
                replaces_blake3: replaces.get(&path_key(&plan.paths[i])).cloned(),
            })
            .collect();
        // Old outputs with no successor move to the store (Replace saves only: a copy has no
        // store to keep them in, and its old copies are simply left alone).
        let retire: Vec<Retire> = if copy || user_edited {
            // With an edited file around the whole old set stays where it is.
            Vec::new()
        } else {
            unchanged
                .iter()
                .filter(|(p, _)| !plan.paths.iter().any(|n| path_key(n) == path_key(p)))
                .map(|(p, h)| Retire {
                    path: p.clone(),
                    expected_blake3: h.clone(),
                })
                .collect()
        };

        let hash = snap.blake3.clone();
        let new_backup = NewBackup {
            source: &path,
            source_blake3: &hash,
            source_size: snap.size,
            source_mtime_ms: snap.mtime_ms,
            format_ext: fmt.extension(),
            run_id,
            run_name,
            retention_days: lock(&self.inner.settings).retention_days,
            edit: Some(state.clone()),
        };
        let backup = match (&saved, target) {
            (_, SaveTarget::Copy) => BackupPlan::None,
            (Some(s), SaveTarget::Replace) if !s.copy && s.backup_id.is_some() => {
                BackupPlan::Existing(s.backup_id.clone().expect("checked"))
            }
            _ => BackupPlan::New(new_backup),
        };
        let first_replace = matches!(backup, BackupPlan::New(_));
        let req = GroupRequest {
            store: &self.inner.store,
            source: SourceFingerprint {
                path: path.clone(),
                size: snap.size,
                mtime_ms: snap.mtime_ms,
                blake3: snap.blake3.clone(),
            },
            unlink_source: first_replace,
            backup,
            outputs,
            retire,
            mtime: UNIX_EPOCH + Duration::from_millis(orig_mtime_ms.max(0) as u64),
            edit: Some(state.clone()),
        };
        let raster = &decoded.raster;
        let mut produce = |i: usize| -> Result<Produced> {
            let quad = crops[i].1.clone();
            crate::run_isolated(std::panic::AssertUnwindSafe(|| -> Result<Produced> {
                let out = render_quad(raster, &quad, Limits::pixels(MAX_PIXELS))
                    .map_err(|_| ErrKind::NoCrop)?;
                let dims = (out.width, out.height);
                let bytes = encode(
                    &out,
                    out_fmt,
                    JPEG_SAVE_QUALITY,
                    icc.as_deref().map(|v| v.as_slice()),
                )
                .map_err(codec_err)?;
                Ok(Produced {
                    bytes,
                    dims,
                    format: out_fmt,
                })
            }))
            .unwrap_or(Err(ErrKind::InternalPanic))
        };
        let result = commit_group(&req, &mut produce, hook);
        drop(decoded);
        let done = match result {
            Ok(d) => d,
            Err(GroupError::Failed(k)) => return Err((k, notices)),
            Err(GroupError::Crashed) => return Err((ErrKind::Internal, notices)),
        };

        // Remember the set (and, after a first Replace, that the pixels now live in the backup).
        let group: Vec<GroupOut> = done
            .outputs
            .iter()
            .map(|o| GroupOut {
                item_id: o.item_id.unwrap_or(0),
                index: o.index.unwrap_or(0),
                path: PathBuf::from(&o.path),
                snap: Snapshot {
                    size: o.size,
                    mtime_ms: stat_of(Path::new(&o.path)).map_or(o.mtime_ms, |s| s.1),
                    blake3: o.blake3.clone(),
                },
            })
            .collect();
        let first = group.first().cloned().ok_or(plain(ErrKind::Internal))?;
        let backup_original = done
            .backup_id
            .as_ref()
            .and_then(|b| self.inner.store.read(b))
            .and_then(|m| self.inner.store.original_path(&m));
        {
            let mut it = lock(&item);
            it.saved = Some(SavedRec {
                backup_id: if copy {
                    saved.as_ref().and_then(|s| s.backup_id.clone())
                } else {
                    done.backup_id.clone()
                },
                output_path: first.path.clone(),
                copy,
                state,
                out: first.snap.clone(),
                group: group.clone(),
            });
            if let (false, Some(p)) = (copy, backup_original) {
                it.original_path = p;
            }
        }
        let names: Vec<String> = group
            .iter()
            .filter_map(|g| g.path.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .collect();
        Ok(SaveOutcome {
            id,
            ok: true,
            error: None,
            saved: Some(SavedInfo {
                backup_id: done.backup_id,
                output: names.first().cloned().unwrap_or_default(),
                copy,
                outputs: names,
            }),
            notes: done.notes,
            notices,
        })
    }

    /// The idempotency guard (M10.25): is this file one of the outputs of an earlier save, 1-to-1
    /// or any of the N of a split? Looks the file's hash up in every manifest's output list.
    pub fn processed_by(&self, path: &Path) -> Option<ProcessedInfo> {
        let h = hash_hex(path)?;
        self.inner
            .store
            .find_output(&h)
            .map(|(m, i)| ProcessedInfo {
                backup_id: m.id.clone(),
                run_name: m.run_name.clone(),
                original_name: m.original_name.clone(),
                output_index: i,
                output_count: m.outputs.len(),
                restored: m.state == crate::store::BackupState::Restored,
            })
    }
}

/// Where an already-processed file came from (the "Already processed" chip, PLAN 2.7).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessedInfo {
    pub backup_id: String,
    pub run_name: String,
    pub original_name: String,
    /// Which of the outputs this file is (0-based).
    pub output_index: usize,
    /// How many outputs the save made (N for a split).
    pub output_count: usize,
    pub restored: bool,
}

/// Names reserved for a split save in flight; released on drop.
struct Reservation<'a> {
    engine: &'a Engine,
    paths: Vec<PathBuf>,
}

impl<'a> Reservation<'a> {
    fn take(engine: &'a Engine, paths: &[PathBuf], own: &[PathBuf]) -> Result<Self> {
        let mut r = lock(&engine.inner.reserved);
        let own: Vec<String> = own.iter().map(|p| path_key(p)).collect();
        if paths
            .iter()
            .any(|p| r.contains(p) && !own.contains(&path_key(p)))
        {
            return Err(ErrKind::PlanStale);
        }
        for p in paths {
            r.insert(p);
        }
        Ok(Self {
            engine,
            paths: paths.to_vec(),
        })
    }
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        let mut r = lock(&self.engine.inner.reserved);
        for p in &self.paths {
            r.remove(p);
        }
    }
}
