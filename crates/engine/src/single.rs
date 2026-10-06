// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Saving one image as one file (ROADMAP M2.83, M2.84): the single-item path of
//! [`Engine::save_items`]. Both targets run the same protocol as a split scan, from
//! [`crate::group`] (PLAN 2.7): a journal, a verified backup, an fsynced temp beside the target,
//! re-read and re-decoded, then an atomic swap, with recovery at the next start.
//!
//! * **Replace** ([`commit_single`]): the file at the path is replaced after its original went to
//!   the backup store. A re-save keeps the one backup of the pristine original.
//! * **Copy** ([`crate::group::commit_group_verified`] with no backup): one new file in `<folder>/AutoCrop/`, named by the
//!   same planner as a split's outputs, never overwriting a file that is not our own unchanged
//!   earlier copy.
//!
//! Everything that can fail without touching the disk happens first, in this order: the file
//! state (read-only, cloud placeholder), the replaceability gate, reading and hashing the pixels'
//! source, the free-space preflight, decoding, rendering and encoding. The disk is touched only
//! once there is a verified-in-memory output to write.

use crate::api::*;
use crate::commit::Expect;
use crate::engine::{Engine, SavedRec, Snapshot, lock, stat_of};
use crate::error::{ErrKind, Result, codec_err};
use crate::fsplan::{OnCollision, PlanInput, ReservedKeys, path_key};
use crate::fsstate;
use crate::group::{
    BackupPlan, FaultHook, GroupError, GroupRequest, OutSpec, Output, Produced, SingleRequest,
    SourceFingerprint, commit_single,
};
use crate::lossless::{Backend, try_lossless};
use crate::output::{SourceMeta, encode_raster};
use crate::source::hash_file;
use crate::space::{Need, preflight};
use crate::store::NewBackup;
use crate::util::blake3_hex;
use auto_crop_codecs::{Format, decode_with};
use auto_crop_imgproc::render::{Limits, render_quad};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

/// A crop covering at least this share of the frame is a no-op (PLAN 2.7).
pub const NOOP_MIN_COVERAGE: f64 = 0.97;
/// ... if it is also skewed by less than this many degrees.
pub const NOOP_MAX_SKEW_DEG: f32 = 0.1;

/// Notice code of a save that took the lossless JPEG path (the UI may show a "lossless" badge).
pub const NOTICE_LOSSLESS: &str = "jpeg.lossless";

/// What a one-file save produced.
pub(crate) struct Done {
    pub info: SavedInfo,
    /// Things that did not stop the save (see `SaveOutcome::notes`).
    pub notes: Vec<ErrKind>,
    /// Notice codes: `jpeg.lossless`, `sync.root`.
    pub notices: Vec<String>,
}

fn hash_hex(p: &Path) -> Option<String> {
    hash_file(p).ok().map(|id| id.to_hex())
}

impl Engine {
    pub(crate) fn save_one(
        &self,
        id: u32,
        target: SaveTarget,
        run_id: &str,
        run_name: &str,
        hook: &dyn FaultHook,
    ) -> Result<Done> {
        let item = self.item(id).ok_or(ErrKind::Internal)?;
        let (path, original_path, state, src_format, snap, orig_mtime_ms, saved, icc) = {
            let it = lock(&item);
            if it.status != ItemStatus::Ready {
                return Err(ErrKind::Internal);
            }
            let state = it
                .history
                .as_ref()
                .ok_or(ErrKind::Internal)?
                .current()
                .clone();
            if state.quad().is_none() {
                return Err(ErrKind::NoCrop);
            }
            (
                it.path.clone(),
                it.original_path.clone(),
                state,
                it.format,
                it.snapshot.clone(),
                it.orig_mtime_ms,
                it.saved.clone(),
                it.icc.clone(),
            )
        };
        let geometry = state.quad().cloned().ok_or(ErrKind::NoCrop)?;
        let opts = self.options();
        let copy = target == SaveTarget::Copy;

        // Writers exist for JPEG and PNG only (PLAN 3.2.3). A source in any other format that this
        // build can open (WebP, TIFF, HEIC, AVIF) is never replaced in place: nothing can write it
        // back without dropping content, so the source stays byte-identical. A copy is written in a
        // format the build can write: JPEG for HEIC (the conversion target of PLAN 3.5), PNG for
        // the rest (lossless, carries the ICC profile).
        let frames = lock(&item).frames;
        let (fmt, copy_ext) = if !copy {
            // The one gate (`fsplan::replace_refusal`): the same code and notice as a split
            // scan's refusal; the dispatcher adds the notice.
            if crate::fsplan::replace_refusal(src_format, frames).is_some() {
                return Err(ErrKind::NotReplaceable);
            }
            (src_format, None)
        } else if src_format.is_encodable() {
            (src_format, None)
        } else {
            let out = if src_format == Format::Heic {
                Format::Jpeg
            } else {
                Format::Png
            };
            (out, Some(out.extension()))
        };

        // File state, before anything is read for the save or written.
        if copy {
            fsstate::check_readable(&original_path, opts.hydrate_cloud_files)?;
        } else {
            fsstate::check_replaceable(&path, opts.hydrate_cloud_files)?;
        }

        // Read the pixels' source: the file before the first save, the backup afterwards.
        let bytes = fs::read(&original_path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                if saved.is_some() {
                    ErrKind::OriginalExpired
                } else {
                    ErrKind::SourceChanged
                }
            } else {
                ErrKind::from_io(&e)
            }
        })?;
        // Reading the source file itself: it must still be what was opened.
        if original_path == path && blake3_hex(&bytes) != snap.blake3 {
            return Err(ErrKind::SourceChanged);
        }

        // The free-space preflight: twice the bytes written, never below the floor.
        let dir = if copy {
            path.parent().ok_or(ErrKind::Internal)?.join("AutoCrop")
        } else {
            path.parent().ok_or(ErrKind::Internal)?.to_path_buf()
        };
        let existing_backup = saved
            .as_ref()
            .and_then(|s| s.backup_id.clone())
            .filter(|_| !copy);
        let backup_cost =
            (!copy && existing_backup.is_none()).then(|| (self.inner.store.dir(), snap.size));
        preflight(&Need::new(&dir, bytes.len() as u64, backup_cost))?;

        // The pixels: the lossless path moves coefficients and decodes nothing; everything else
        // decodes, renders once and encodes through the codecs crate.
        let meta = SourceMeta::read(src_format, &bytes, icc.clone());
        let mut notices: Vec<String> = Vec::new();
        let lossless = (src_format == Format::Jpeg && fmt == Format::Jpeg)
            .then(|| try_lossless(&mut Backend::new(), &bytes, &geometry, &meta, &opts))
            .flatten();
        let (out_bytes, dims, expect) = match lossless {
            Some(l) => {
                notices.push(NOTICE_LOSSLESS.to_owned());
                let expect = Expect {
                    dims: l.dims,
                    format: Format::Jpeg,
                    icc: meta.icc.as_ref().map(|p| p.to_vec()),
                    // Coefficients were moved, not recomputed: there is no encoder input to
                    // fingerprint. The re-read hash, the full decode, the size, the orientation
                    // and the ICC profile are still checked.
                    content: crate::commit::Content::None,
                };
                (l.bytes, l.dims, expect)
            }
            None => {
                let decoded = decode_with(&bytes, &opts.limits()).map_err(codec_err)?;
                drop(bytes);
                let out = render_quad(&decoded.raster, &geometry, Limits::pixels(opts.max_pixels))
                    .map_err(|_| ErrKind::NoCrop)?;
                drop(decoded);
                let enc = encode_raster(&out, fmt, &meta, &opts)?;
                (enc.bytes, (out.width, out.height), enc.expect)
            }
        };
        let mtime = UNIX_EPOCH + Duration::from_millis(orig_mtime_ms.max(0) as u64);
        let produced = Produced {
            bytes: out_bytes,
            dims,
            format: fmt,
        };

        if !copy {
            if fsstate::in_sync_root(&path) {
                notices.push(fsstate::NOTICE_SYNC_ROOT.to_owned());
            }
            let original_hash = snap.blake3.clone();
            let backup = match &existing_backup {
                Some(bid) => BackupPlan::Existing(bid.clone()),
                None => BackupPlan::New(NewBackup {
                    source: &path,
                    source_blake3: &original_hash,
                    source_size: snap.size,
                    source_mtime_ms: snap.mtime_ms,
                    format_ext: fmt.extension(),
                    run_id,
                    run_name,
                    retention_days: lock(&self.inner.settings).retention_days,
                    edit: Some(state.clone()),
                }),
            };
            let req = SingleRequest {
                store: &self.inner.store,
                target: SourceFingerprint {
                    path: path.clone(),
                    size: snap.size,
                    mtime_ms: snap.mtime_ms,
                    blake3: snap.blake3.clone(),
                },
                backup,
                mtime,
                edit: Some(state.clone()),
                verify: opts.verify,
            };
            let mut once = Some(Output {
                produced,
                expect: Some(expect),
            });
            let mut produce = || once.take().ok_or(ErrKind::Internal);
            let done = commit_single(&req, &mut produce, hook).map_err(commit_err)?;

            let manifest = self.inner.store.read(&done.backup_id);
            let out_snap = stat_of(&path)
                .map(|(size, mtime_ms)| Snapshot {
                    size,
                    mtime_ms,
                    blake3: done.output.blake3.clone(),
                })
                .ok_or(ErrKind::Internal)?;
            let mut it = lock(&item);
            it.saved = Some(SavedRec {
                backup_id: Some(done.backup_id.clone()),
                output_path: path.clone(),
                copy: false,
                state,
                out: out_snap.clone(),
                group: Vec::new(),
            });
            if let Some(p) = manifest.and_then(|m| self.inner.store.original_path(&m)) {
                it.original_path = p;
            }
            it.snapshot = out_snap;
            return Ok(Done {
                info: SavedInfo {
                    backup_id: Some(done.backup_id),
                    output: file_name(&path),
                    copy: false,
                    outputs: Vec::new(),
                },
                notes: Vec::new(),
                notices,
            });
        }

        // ---- Save as a copy: one new file in `<folder>/AutoCrop/`.
        // Saving the same image as a copy again takes over its own earlier copy, but only while
        // that copy is still exactly what was written.
        let own: Vec<PathBuf> = match &saved {
            Some(s) if s.copy && s.group.is_empty() => {
                if hash_hex(&s.output_path).as_deref() == Some(s.out.blake3.as_str()) {
                    vec![s.output_path.clone()]
                } else {
                    Vec::new()
                }
            }
            _ => Vec::new(),
        };
        let ext: String = match copy_ext {
            Some(e) => e.to_owned(),
            // The same format keeps the source's own spelling of the extension (`.jpeg`, `.PNG`).
            None => path
                .extension()
                .map(|e| e.to_string_lossy().into_owned())
                .unwrap_or_else(|| fmt.extension().to_owned()),
        };
        let mut attempt = 0;
        let (plan, _reservation) = loop {
            attempt += 1;
            let reserved = self.reserved_names(id);
            let plan = crate::fsplan::plan_group(&PlanInput {
                source: &path,
                dir: &dir,
                count: 1,
                template: None,
                ext: &ext,
                on_collision: OnCollision::Rename,
                reserved: &reserved,
                own: &own,
            })
            .map_err(|e| e.kind())?;
            match crate::scan::Reservation::take(self, &plan.paths, &own) {
                Ok(r) => break (plan, r),
                Err(_) if attempt < 4 => continue,
                Err(e) => return Err(e),
            }
        };
        plan.recheck(&ReservedKeys::default(), &own)?;
        let dest = plan.paths[0].clone();
        let replaces = own
            .iter()
            .any(|o| path_key(o) == path_key(&dest))
            .then(|| saved.as_ref().map(|s| s.out.blake3.clone()))
            .flatten();
        let req = GroupRequest {
            store: &self.inner.store,
            source: SourceFingerprint {
                path: path.clone(),
                size: snap.size,
                mtime_ms: snap.mtime_ms,
                blake3: snap.blake3.clone(),
            },
            unlink_source: false,
            backup: BackupPlan::None,
            outputs: vec![OutSpec {
                item_id: 0,
                index: 1,
                final_path: dest.clone(),
                replaces_blake3: replaces,
            }],
            retire: Vec::new(),
            mtime,
            edit: Some(state.clone()),
        };
        let mut once = Some(Output {
            produced,
            expect: Some(expect),
        });
        let mut produce = |_: usize| once.take().ok_or(ErrKind::Internal);
        let done = crate::group::commit_group_verified(&req, &mut produce, opts.verify, hook)
            .map_err(commit_err)?;
        let rec = done.outputs.first().ok_or(ErrKind::Internal)?;
        let out_snap = stat_of(&dest)
            .map(|(size, mtime_ms)| Snapshot {
                size,
                mtime_ms,
                blake3: rec.blake3.clone(),
            })
            .ok_or(ErrKind::Internal)?;
        let mut it = lock(&item);
        // The first copy leaves the source where it is; later edits still start from it.
        let backup_id = saved.as_ref().and_then(|s| s.backup_id.clone());
        it.saved = Some(SavedRec {
            backup_id: backup_id.clone(),
            output_path: dest.clone(),
            copy: true,
            state,
            out: out_snap,
            group: Vec::new(),
        });
        Ok(Done {
            info: SavedInfo {
                backup_id,
                output: file_name(&dest),
                copy: true,
                outputs: Vec::new(),
            },
            notes: done.notes,
            notices,
        })
    }

    /// The no-op rule of the idempotency guard (ROADMAP M2.38): a crop that covers at least 97% of
    /// the frame, is not skewed by 0.1 degrees or more, is not turned or mirrored, and whose source
    /// carries no EXIF turn changes nothing worth a rewrite, so a save would only cost a backup and
    /// a generation. `false` for an item with no crop.
    pub fn is_noop(&self, id: u32) -> bool {
        let Some(item) = self.item(id) else {
            return false;
        };
        let it = lock(&item);
        let Some(q) = it
            .history
            .as_ref()
            .and_then(|h| h.current().quad().cloned())
        else {
            return false;
        };
        it.exif_orientation == 1
            && q.quarter_turns % 4 == 0
            && !q.mirror
            && q.fine_deg.abs() < NOOP_MAX_SKEW_DEG
            && q.signed_area() >= NOOP_MIN_COVERAGE
    }

    /// The options of every save and open (quality, metadata policy, lossless path, verify depth,
    /// cloud downloads, pixel cap).
    pub fn options(&self) -> crate::output::EngineOptions {
        *lock(&self.inner.options)
    }

    pub fn set_options(&self, o: crate::output::EngineOptions) {
        self.inner.store.set_method(o.backup_method);
        *lock(&self.inner.options) = o;
    }
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn commit_err(e: GroupError) -> ErrKind {
    match e {
        GroupError::Failed(k) => k,
        GroupError::Crashed => ErrKind::Internal,
    }
}
