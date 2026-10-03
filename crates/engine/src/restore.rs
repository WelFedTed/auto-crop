// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Restore original for a split scan (ROADMAP M10.26; PLAN 2.7 "Restore original"): the scan comes
//! back byte for byte, and the user chooses what happens to the N derived files. **Keep** leaves
//! them alone. **Remove** moves the ones that are still exactly as they were saved into the
//! backup store (`<backup>/derived-by-restore/`, recorded in the manifest), which is reversible:
//! nothing is ever deleted. A derived file that changed since it was saved, or is already gone, is
//! kept (the user's edit is never taken away).
//!
//! The order is the safe one: the scan is put back first (a verified temp and a no-clobber move,
//! or `name (restored).ext` if something else now sits on its path), then each derived file is
//! moved and recorded in the manifest before the next. A crash anywhere leaves a state a second
//! restore completes.

use crate::api::*;
use crate::commit::{free_name, swap, write_temp};
use crate::engine::{Engine, Snapshot, lock, stat_of};
use crate::error::{ErrKind, Result};
use crate::group::move_no_clobber;
use crate::source::hash_file;
use crate::store::{BackupKind, BackupState, Manifest, MovedRec, Store};
use crate::util::{blake3_hex, now_secs};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

fn hash_hex(p: &Path) -> Option<String> {
    hash_file(p).ok().map(|id| id.to_hex())
}

/// The derived files of a split backup and what became of each.
pub(crate) fn derived_files(m: &Manifest) -> Vec<DerivedFile> {
    m.outputs
        .iter()
        .map(|o| {
            let p = Path::new(&o.path);
            let moved = m.derived_moved.iter().any(|r| r.path == o.path);
            let state = if moved {
                DerivedState::Removed
            } else {
                match fs::metadata(p) {
                    Err(_) => DerivedState::Missing,
                    Ok(meta) if meta.len() != o.size => DerivedState::Changed,
                    Ok(_) => match hash_hex(p) {
                        Some(h) if h == o.blake3 => DerivedState::Unchanged,
                        Some(_) => DerivedState::Changed,
                        None => DerivedState::Missing,
                    },
                }
            };
            DerivedFile {
                name: p
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                bytes: o.size,
                state,
            }
        })
        .collect()
}

/// Moves `src` to `dst` inside the store: a rename, or (across volumes) a copy that is verified
/// against `hash` before the original is removed.
fn move_into_store(src: &Path, dst: &Path, hash: &str) -> Result<()> {
    if let Some(d) = dst.parent() {
        fs::create_dir_all(d).map_err(|e| ErrKind::from_io(&e))?;
    }
    if dst.exists() {
        // A previous, interrupted restore already put it there.
        return if hash_hex(dst).as_deref() == Some(hash) {
            let _ = fs::remove_file(src);
            Ok(())
        } else {
            Err(ErrKind::Internal)
        };
    }
    if fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    fs::copy(src, dst).map_err(|e| ErrKind::from_io(&e))?;
    if hash_hex(dst).as_deref() != Some(hash) {
        let _ = fs::remove_file(dst);
        return Err(ErrKind::VerifyFailed);
    }
    fs::remove_file(src).map_err(|e| ErrKind::from_io(&e))
}

fn stored_name(i: usize, o: &crate::store::OutputRec) -> String {
    let name = Path::new(&o.path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    format!("{:02}_{name}", o.index.map_or(i + 1, |n| n as usize))
}

impl Engine {
    /// Restores one backup with the user's choice for the derived files of a split (M10.26).
    /// For a one-to-one backup `derived` is ignored.
    pub fn restore_file_derived(
        &self,
        file_id: &str,
        mode: RestoreMode,
        derived: DerivedAction,
        notify: &dyn Fn(ItemView),
    ) -> RestoreOutcome {
        let backup_id = file_id.split('/').next().unwrap_or("");
        match self.inner.store.read(backup_id) {
            Some(m) if m.kind == BackupKind::OneToN => self.restore_group(m, mode, derived, notify),
            Some(m) => self.restore_manifest(m, mode, notify),
            None => RestoreOutcome::failed(ErrKind::OriginalExpired),
        }
    }

    /// "Restore all from this run" with one choice for every split in it.
    pub fn restore_run_derived(
        &self,
        run_id: &str,
        derived: DerivedAction,
        notify: &dyn Fn(ItemView),
    ) -> Vec<RestoreOutcome> {
        let mut ms: Vec<Manifest> = self
            .inner
            .store
            .list()
            .into_iter()
            .filter(|m| m.run_id == run_id && m.state == BackupState::Saved)
            .collect();
        ms.reverse();
        ms.into_iter()
            .map(|m| {
                if m.kind == BackupKind::OneToN {
                    self.restore_group(m, RestoreMode::Auto, derived, notify)
                } else {
                    self.restore_manifest(m, RestoreMode::Auto, notify)
                }
            })
            .collect()
    }

    pub(crate) fn restore_group(
        &self,
        mut m: Manifest,
        mode: RestoreMode,
        derived: DerivedAction,
        notify: &dyn Fn(ItemView),
    ) -> RestoreOutcome {
        let store: &Store = &self.inner.store;
        let result = (|| -> Result<RestoreOutcome> {
            let orig_file = store.original_path(&m).ok_or(ErrKind::OriginalExpired)?;
            let orig_bytes = fs::read(&orig_file).map_err(|_| ErrKind::OriginalExpired)?;
            if blake3_hex(&orig_bytes) != m.original_blake3 {
                return Err(ErrKind::VerifyFailed);
            }
            let target = PathBuf::from(&m.original_path);
            let dir = target.parent().ok_or(ErrKind::Internal)?.to_path_buf();
            let mtime = UNIX_EPOCH + Duration::from_millis(m.original_mtime_ms.max(0) as u64);
            let current = hash_hex(&target);
            let already_original = current.as_deref() == Some(m.original_blake3.as_str());
            let name = target
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("image")
                .to_owned();
            let restored_name = |dir: &Path| {
                let stem = Path::new(&name)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("image")
                    .to_owned();
                free_name(&dir.join(format!("{stem} (restored).{}", m.format)))
            };

            // 1. The scan.
            let dest = if already_original {
                target.clone()
            } else if mode == RestoreMode::AsCopy
                || (current.is_some() && mode == RestoreMode::Auto)
            {
                // Something else sits on the scan's path (or a copy was asked for): the scan
                // comes back beside it, as `name (restored).ext`, and nothing is replaced.
                restored_name(&dir)
            } else {
                target.clone()
            };
            if !already_original {
                let tmp = write_temp(&dir, &orig_bytes, Some(mtime))?;
                let placed = if dest == target && current.is_some() {
                    // ReplaceAnyway: what is there is kept in the backup first.
                    if let Some(pre) = store.pre_restore_path(&m) {
                        fs::copy(&target, &pre).map_err(|e| ErrKind::from_io(&e))?;
                    }
                    swap(&tmp.path, &dest)
                } else {
                    move_no_clobber(&tmp.path, &dest)
                };
                placed.inspect_err(|_| tmp.discard())?;
            }
            if mode == RestoreMode::AsCopy {
                // A copy leaves the backup and the derived files as they are.
                return Ok(RestoreOutcome {
                    ok: true,
                    needs_choice: false,
                    error: None,
                    restored: dest.file_name().map(|n| n.to_string_lossy().into_owned()),
                    derived: derived_files(&m),
                });
            }

            // 2. The derived files: Keep leaves them; Remove moves the unchanged ones into the
            // store, recording each before the next.
            if derived == DerivedAction::Remove {
                let states = derived_files(&m);
                let outputs = m.outputs.clone();
                for (i, (o, s)) in outputs.iter().zip(&states).enumerate() {
                    if s.state != DerivedState::Unchanged {
                        continue; // changed, missing or already moved: kept as it is
                    }
                    let stored = stored_name(i, o);
                    let dst = store
                        .entry_path(&m.id)
                        .ok_or(ErrKind::Internal)?
                        .join("derived-by-restore")
                        .join(&stored);
                    move_into_store(Path::new(&o.path), &dst, &o.blake3)?;
                    m.derived_moved.push(MovedRec {
                        path: o.path.clone(),
                        stored,
                        blake3: o.blake3.clone(),
                    });
                    store.write(&m)?;
                }
            }
            m.state = BackupState::Restored;
            m.restored_at = Some(now_secs());
            store.write(&m)?;

            // 3. Any open image saved from this backup is an unsaved original again.
            let snapshot = Snapshot {
                size: orig_bytes.len() as u64,
                mtime_ms: stat_of(&dest).map(|s| s.1).unwrap_or(0),
                blake3: m.original_blake3.clone(),
            };
            let items: Vec<_> = lock(&self.inner.items).values().cloned().collect();
            for item in items {
                let mut it = lock(&item);
                if it.saved.as_ref().and_then(|s| s.backup_id.as_deref()) == Some(m.id.as_str()) {
                    it.saved = None;
                    it.original_path = dest.clone();
                    it.snapshot = snapshot.clone();
                    it.generation += 1;
                    notify(it.view());
                }
            }
            Ok(RestoreOutcome {
                ok: true,
                needs_choice: false,
                error: None,
                restored: dest.file_name().map(|n| n.to_string_lossy().into_owned()),
                derived: derived_files(&m),
            })
        })();
        result.unwrap_or_else(RestoreOutcome::failed)
    }
}
