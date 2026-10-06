// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Conversion replaces the source (ROADMAP M2.36; PLAN 2.7 "Conversion replaces the source (B3,
//! B12)"): `IMG_0001.HEIC` becomes `IMG_0001.jpg`, `scan.bmp` becomes `scan.png`. No geometry is
//! applied (the `convert-only` preset): the decoded pixels, with the EXIF turn applied once, are
//! encoded in the target format.
//!
//! The order is the one of the group commit it runs on ([`crate::group`]): encode and verify the
//! temp, back up the source (a verified copy, link or clone, so it stays put), commit the output by
//! **no-clobber** move, then unlink the source path last and only after re-checking it is still the
//! file that was backed up. Each step is journalled; a crash between the last two leaves both
//! files and recovery finishes the unlink. If another file already holds the target name, the plan
//! renames to `IMG_0001 (2).jpg` (never overwrites). Restore returns the source to its path and, with
//! [`DerivedAction::Remove`], moves the converted file into the backup entry.

use crate::api::*;
use crate::engine::{Engine, SavedRec, Snapshot, lock, stat_of};
use crate::error::{ErrKind, codec_err};
use crate::fsplan::{OnCollision, PlanInput, ReservedKeys};
use crate::fsstate;
use crate::group::{
    BackupPlan, FaultHook, GroupError, GroupRequest, NoFaults, OutSpec, Output, Produced,
    SourceFingerprint, commit_group_verified,
};
use crate::output::{SourceMeta, encode_raster};
use crate::space::{Need, preflight};
use crate::store::NewBackup;
use crate::util::blake3_hex;
use auto_crop_codecs::{Format, decode_with};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, UNIX_EPOCH};

/// Notice code of a source that is already in its target format (nothing to convert).
pub const NOTICE_SAME_FORMAT: &str = "convert.same_format";

/// The format a source is converted to, or `None` if it is not converted: HEIC and HEIF become
/// JPEG, formats with no writer that PNG can hold without loss (BMP; GIF, ICO, TGA, PNM, QOI and
/// HDR when they are decodable) become PNG. A JPEG or PNG is already in its target; WebP, TIFF and
/// AVIF are never replaced (PLAN 2.7 "Never replaced", "Not replaced until the writer ships").
pub fn conversion_target(format: Format, frames: u32) -> Option<Format> {
    if frames > 1 {
        return None;
    }
    match format {
        Format::Heic => Some(Format::Jpeg),
        Format::Bmp | Format::Gif => Some(Format::Png),
        _ => None,
    }
}

impl Engine {
    /// Converts each image to its target format, replacing the source after a verified backup.
    /// `notify` hears the new view of every image that was handled.
    pub fn convert_items(
        &self,
        ids: &[u32],
        run_name: &str,
        notify: &dyn Fn(ItemView),
    ) -> Vec<SaveOutcome> {
        self.convert_items_with_faults(ids, run_name, &NoFaults, notify)
    }

    #[doc(hidden)]
    pub fn convert_items_with_faults(
        &self,
        ids: &[u32],
        run_name: &str,
        hook: &dyn FaultHook,
        notify: &dyn Fn(ItemView),
    ) -> Vec<SaveOutcome> {
        let run_id = crate::util::new_id();
        let run_name: String = run_name
            .chars()
            .filter(|c| !c.is_control())
            .take(80)
            .collect();
        ids.iter()
            .map(|id| {
                let outcome = crate::run_isolated(std::panic::AssertUnwindSafe(|| {
                    self.convert_one(*id, &run_id, &run_name, hook)
                }))
                .unwrap_or(Err((ErrKind::InternalPanic, Vec::new())));
                let outcome = match outcome {
                    Ok(o) => o,
                    Err((e, notices)) => SaveOutcome {
                        notices,
                        ..SaveOutcome::failed(*id, e)
                    },
                };
                if let Some(v) = self.item_view(*id) {
                    notify(v);
                }
                outcome
            })
            .collect()
    }

    fn convert_one(
        &self,
        id: u32,
        run_id: &str,
        run_name: &str,
        hook: &dyn FaultHook,
    ) -> std::result::Result<SaveOutcome, (ErrKind, Vec<String>)> {
        let plain = |e: ErrKind| (e, Vec::new());
        let item = self.item(id).ok_or(plain(ErrKind::Internal))?;
        let (path, original_path, state, src_format, snap, orig_mtime_ms, saved, icc, frames) = {
            let it = lock(&item);
            if it.status != ItemStatus::Ready {
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
            )
        };
        // One rule for what may be replaced (D3): a multi-frame source is never converted either.
        let Some(target) = conversion_target(src_format, frames) else {
            let notice = if frames > 1 {
                crate::fsplan::replace_refusal(src_format, frames)
            } else if src_format.is_encodable() {
                Some(NOTICE_SAME_FORMAT)
            } else {
                crate::fsplan::replace_refusal(src_format, frames)
            };
            let kind = if src_format.is_encodable() && frames <= 1 {
                ErrKind::UnsupportedOutput
            } else {
                ErrKind::NotReplaceable
            };
            return Err((kind, notice.map(|n| vec![n.to_owned()]).unwrap_or_default()));
        };
        if saved.is_some() {
            // Already converted in this session: its source is in the store, not at `path`.
            return Err(plain(ErrKind::NotReplaceable));
        }
        let opts = self.options();
        fsstate::check_replaceable(&path, opts.hydrate_cloud_files).map_err(plain)?;
        let bytes = fs::read(&original_path).map_err(|e| {
            plain(if e.kind() == std::io::ErrorKind::NotFound {
                ErrKind::SourceChanged
            } else {
                ErrKind::from_io(&e)
            })
        })?;
        if original_path == path && blake3_hex(&bytes) != snap.blake3 {
            return Err(plain(ErrKind::SourceChanged));
        }
        let dir = path.parent().ok_or(plain(ErrKind::Internal))?.to_path_buf();
        preflight(&Need::new(
            &dir,
            bytes.len() as u64,
            Some((self.inner.store.dir(), snap.size)),
        ))
        .map_err(plain)?;

        // The pixels: decoded once (the EXIF turn applied once), encoded in the target format.
        let meta = SourceMeta::read(src_format, &bytes, icc);
        let decoded = decode_with(&bytes, &opts.limits()).map_err(|e| plain(codec_err(e)))?;
        drop(bytes);
        let enc = encode_raster(&decoded.raster, target, &meta, &opts).map_err(plain)?;
        let dims = (decoded.raster.width, decoded.raster.height);
        drop(decoded);

        // The name: `stem.<ext>` beside the source, never a taken name.
        let mut attempt = 0;
        let (plan, _reservation) = loop {
            attempt += 1;
            let reserved = self.reserved_names(id);
            let plan = crate::fsplan::plan_group(&PlanInput {
                source: &path,
                dir: &dir,
                count: 1,
                template: None,
                ext: target.extension(),
                on_collision: OnCollision::Rename,
                reserved: &reserved,
                own: &[],
            })
            .map_err(|e| plain(e.kind()))?;
            match crate::scan::Reservation::take(self, &plan.paths, &[]) {
                Ok(r) => break (plan, r),
                Err(_) if attempt < 4 => continue,
                Err(e) => return Err(plain(e)),
            }
        };
        plan.recheck(&ReservedKeys::default(), &[]).map_err(plain)?;
        let dest = plan.paths[0].clone();

        let hash = snap.blake3.clone();
        let req = GroupRequest {
            store: &self.inner.store,
            source: SourceFingerprint {
                path: path.clone(),
                size: snap.size,
                mtime_ms: snap.mtime_ms,
                blake3: snap.blake3.clone(),
            },
            unlink_source: true,
            backup: BackupPlan::New(NewBackup {
                source: &path,
                source_blake3: &hash,
                source_size: snap.size,
                source_mtime_ms: snap.mtime_ms,
                format_ext: src_format.extension(),
                run_id,
                run_name,
                retention_days: lock(&self.inner.settings).retention_days,
                edit: Some(state.clone()),
            }),
            outputs: vec![OutSpec {
                item_id: 0,
                index: 1,
                final_path: dest.clone(),
                replaces_blake3: None,
            }],
            retire: Vec::new(),
            mtime: UNIX_EPOCH + Duration::from_millis(orig_mtime_ms.max(0) as u64),
            edit: Some(state.clone()),
        };
        let mut once = Some(Output {
            produced: Produced {
                bytes: enc.bytes,
                dims,
                format: target,
            },
            expect: Some(enc.expect),
        });
        let mut produce = |_: usize| once.take().ok_or(ErrKind::Internal);
        let done = commit_group_verified(&req, &mut produce, opts.verify, hook).map_err(|e| {
            plain(match e {
                GroupError::Failed(k) => k,
                GroupError::Crashed => ErrKind::Internal,
            })
        })?;

        let rec = done.outputs.first().ok_or(plain(ErrKind::Internal))?;
        let out_snap = stat_of(&dest)
            .map(|(size, mtime_ms)| Snapshot {
                size,
                mtime_ms,
                blake3: rec.blake3.clone(),
            })
            .ok_or(plain(ErrKind::Internal))?;
        let backup_original = done
            .backup_id
            .as_ref()
            .and_then(|b| self.inner.store.read(b))
            .and_then(|m| self.inner.store.original_path(&m));
        {
            let mut it = lock(&item);
            it.saved = Some(SavedRec {
                backup_id: done.backup_id.clone(),
                output_path: dest.clone(),
                copy: false,
                state,
                out: out_snap,
                group: Vec::new(),
            });
            if let Some(p) = backup_original {
                it.original_path = p;
            }
        }
        let name: PathBuf = dest.file_name().map(PathBuf::from).unwrap_or_default();
        let name = name.to_string_lossy().into_owned();
        Ok(SaveOutcome {
            id,
            ok: true,
            error: None,
            saved: Some(SavedInfo {
                backup_id: done.backup_id,
                output: name.clone(),
                copy: false,
                outputs: vec![name],
            }),
            notes: done.notes,
            notices: Vec::new(),
        })
    }
}
