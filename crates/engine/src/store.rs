// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The backup store (PLAN 2.7): `backups/<id>/original.<ext>` plus a self-describing
//! `manifest.json` per backed-up file. The store survives loss of anything else: each directory
//! carries everything needed to restore. Runs are only a grouping in the manifest.

use crate::error::{ErrKind, Result};
use crate::util::{blake3_hex, new_id, now_secs};
use auto_crop_core::EditState;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const MANIFEST_SCHEMA: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackupState {
    /// The original is safely stored; the output may not have replaced it yet.
    BackedUp,
    Saved,
    Restored,
}

/// What a backup was made for (PLAN 2.7 `backups.kind`). Additive: a manifest without the field
/// is `OneToOne`, which is every manifest written before M10.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BackupKind {
    #[default]
    OneToOne,
    /// A multi-item scan: the scan went to the store and `outputs` holds its N derived files.
    OneToN,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputRec {
    pub path: String,
    pub blake3: String,
    pub size: u64,
    pub mtime_ms: i64,
    /// The item this output was cut from (1-to-N only; M10.25).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<u32>,
    /// 1-based rank among the outputs, the `{n}` of its name (1-to-N only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<u32>,
}

impl OutputRec {
    /// A one-to-one output.
    pub fn plain(path: String, blake3: String, size: u64, mtime_ms: i64) -> Self {
        Self {
            path,
            blake3,
            size,
            mtime_ms,
            item_id: None,
            index: None,
        }
    }
}

/// A derived file that Restore moved into the store (M10.26 "Remove"): reversible, never a delete.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MovedRec {
    /// Where the file was.
    pub path: String,
    /// Its name inside `<backup>/derived-by-restore/`.
    pub stored: String,
    pub blake3: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Manifest {
    pub schema: u32,
    pub id: String,
    pub run_id: String,
    pub run_name: String,
    pub created_at: i64,
    pub original_path: String,
    pub original_name: String,
    pub original_file: String,
    pub original_blake3: String,
    pub original_size: u64,
    pub original_mtime_ms: i64,
    pub format: String,
    pub outputs: Vec<OutputRec>,
    pub state: BackupState,
    pub pinned: bool,
    pub purge_after: Option<i64>,
    pub engine_version: String,
    #[serde(deserialize_with = "crate::migrate::deserialize_optional")]
    pub edit: Option<EditState>,
    pub restored_at: Option<i64>,
    /// `OneToN` for a split scan (M10.25); older manifests read as `OneToOne`.
    pub kind: BackupKind,
    /// Derived files Restore moved into the store (M10.26).
    pub derived_moved: Vec<MovedRec>,
}

impl Default for Manifest {
    fn default() -> Self {
        Self {
            schema: MANIFEST_SCHEMA,
            id: String::new(),
            run_id: String::new(),
            run_name: String::new(),
            created_at: 0,
            original_path: String::new(),
            original_name: String::new(),
            original_file: String::new(),
            original_blake3: String::new(),
            original_size: 0,
            original_mtime_ms: 0,
            format: String::new(),
            outputs: Vec::new(),
            state: BackupState::BackedUp,
            pinned: false,
            purge_after: None,
            engine_version: env!("CARGO_PKG_VERSION").to_owned(),
            edit: None,
            restored_at: None,
            kind: BackupKind::OneToOne,
            derived_moved: Vec::new(),
        }
    }
}

/// Everything needed to back an original up.
#[derive(Clone)]
pub struct NewBackup<'a> {
    pub source: &'a Path,
    pub source_blake3: &'a str,
    pub source_size: u64,
    pub source_mtime_ms: i64,
    pub format_ext: &'a str,
    pub run_id: &'a str,
    pub run_name: &'a str,
    pub retention_days: Option<u32>,
    pub edit: Option<EditState>,
}

#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
}

fn sync_file(path: &Path) -> std::io::Result<()> {
    fs::OpenOptions::new().write(true).open(path)?.sync_all()
}

impl Store {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn entry_dir(&self, id: &str) -> Option<PathBuf> {
        // Ids are our own 28 hex digits; anything else is refused so no path can escape the store.
        (id.len() == 28 && id.bytes().all(|b| b.is_ascii_hexdigit())).then(|| self.dir.join(id))
    }

    pub fn original_path(&self, m: &Manifest) -> Option<PathBuf> {
        let name = Path::new(&m.original_file).file_name()?;
        Some(self.entry_dir(&m.id)?.join(name))
    }

    /// The folder of a backup entry (validated id), for the files a group commit or a restore
    /// keeps next to the original.
    pub fn entry_path(&self, id: &str) -> Option<PathBuf> {
        self.entry_dir(id)
    }

    /// Where group-commit journals live (M10.24). Not a backup id, so `list` skips it.
    pub fn groups_dir(&self) -> PathBuf {
        self.dir.join(".groups")
    }

    /// Removes a backup that never became part of a save (a group that rolled back): an entry
    /// still in `BackedUp`, or a half-made one with no manifest at all, inside the store. A
    /// saved, restored or unreadable entry is never touched. Returns whether it was removed.
    pub fn remove_unused(&self, id: &str) -> bool {
        let Some(dir) = self.entry_dir(id) else {
            return false;
        };
        match self.read(id) {
            Some(m) if m.state == BackupState::BackedUp => fs::remove_dir_all(dir).is_ok(),
            None if dir.is_dir() && !dir.join("manifest.json").exists() => {
                fs::remove_dir_all(dir).is_ok()
            }
            _ => false,
        }
    }

    pub fn pre_restore_path(&self, m: &Manifest) -> Option<PathBuf> {
        Some(
            self.entry_dir(&m.id)?
                .join(format!("replaced-by-restore.{}", m.format)),
        )
    }

    /// Copies the original into the store, verifies the copy against the hash taken when the file
    /// was opened, makes it durable, and only then writes the manifest. No backup, no overwrite.
    pub fn create(&self, req: &NewBackup<'_>) -> Result<Manifest> {
        self.create_with_id(new_id(), req)
    }

    /// [`Store::create`] under an id chosen by the caller, so a journal can name the backup
    /// before it exists (M10.24).
    pub fn create_with_id(&self, id: String, req: &NewBackup<'_>) -> Result<Manifest> {
        let dir = self.entry_dir(&id).ok_or(ErrKind::Internal)?;
        fs::create_dir_all(&dir)
            .map_err(|e| ErrKind::from_io(&e))
            .map_err(|_| ErrKind::BackupFailed)?;
        let result = (|| -> Result<Manifest> {
            let file_name = format!("original.{}", req.format_ext);
            let dest = dir.join(&file_name);
            fs::copy(req.source, &dest).map_err(|_| ErrKind::BackupFailed)?;
            let bytes = fs::read(&dest).map_err(|_| ErrKind::BackupFailed)?;
            if blake3_hex(&bytes) != req.source_blake3 {
                return Err(ErrKind::BackupFailed);
            }
            sync_file(&dest).map_err(|_| ErrKind::BackupFailed)?;
            let now = now_secs();
            let m = Manifest {
                id: id.clone(),
                run_id: req.run_id.to_owned(),
                run_name: req.run_name.to_owned(),
                created_at: now,
                original_path: req.source.to_string_lossy().into_owned(),
                original_name: req
                    .source
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                original_file: file_name,
                original_blake3: req.source_blake3.to_owned(),
                original_size: req.source_size,
                original_mtime_ms: req.source_mtime_ms,
                format: req.format_ext.to_owned(),
                state: BackupState::BackedUp,
                purge_after: req.retention_days.map(|d| now + i64::from(d) * 86_400),
                edit: req.edit.clone(),
                ..Manifest::default()
            };
            self.write(&m)?;
            Ok(m)
        })();
        if result.is_err() {
            // The directory is ours and holds nothing the user needs yet.
            let _ = fs::remove_dir_all(&dir);
        }
        result
    }

    /// Writes the manifest atomically: temp file, fsync, rename.
    pub fn write(&self, m: &Manifest) -> Result<()> {
        let dir = self.entry_dir(&m.id).ok_or(ErrKind::Internal)?;
        let tmp = dir.join(".manifest.json.tmp");
        let json = serde_json::to_vec_pretty(m).map_err(|_| ErrKind::Internal)?;
        let mut f = fs::File::create(&tmp).map_err(|e| ErrKind::from_io(&e))?;
        f.write_all(&json).map_err(|e| ErrKind::from_io(&e))?;
        f.sync_all().map_err(|e| ErrKind::from_io(&e))?;
        drop(f);
        fs::rename(&tmp, dir.join("manifest.json")).map_err(|e| ErrKind::from_io(&e))
    }

    pub fn read(&self, id: &str) -> Option<Manifest> {
        let dir = self.entry_dir(id)?;
        let m: Manifest =
            serde_json::from_slice(&fs::read(dir.join("manifest.json")).ok()?).ok()?;
        // A manifest from a newer schema is refused rather than half-understood.
        (m.schema <= MANIFEST_SCHEMA && m.id == id).then_some(m)
    }

    /// All readable manifests, newest first. Unreadable directories are ignored, never deleted.
    pub fn list(&self) -> Vec<Manifest> {
        let mut out: Vec<Manifest> = fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| self.read(&e.file_name().to_string_lossy()))
            .collect();
        out.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(b.id.cmp(&a.id)));
        out
    }

    /// The idempotency guard (M10.25): the saved backup, if any, one of whose recorded outputs has
    /// this hash, and which output it is (0-based). A split's N outputs are all searched.
    pub fn find_output(&self, blake3: &str) -> Option<(Manifest, usize)> {
        self.list().into_iter().find_map(|m| {
            let i = m.outputs.iter().position(|o| o.blake3 == blake3)?;
            Some((m, i))
        })
    }

    pub fn used_bytes(&self) -> u64 {
        fn walk(p: &Path) -> u64 {
            let Ok(rd) = fs::read_dir(p) else { return 0 };
            rd.flatten()
                .map(|e| match e.metadata() {
                    Ok(m) if m.is_dir() => walk(&e.path()),
                    Ok(m) => m.len(),
                    Err(_) => 0,
                })
                .sum()
        }
        walk(&self.dir)
    }

    /// Deletes one backup entry (the original, its manifest and everything stored beside it).
    /// Only a directory inside the store with a valid id is ever removed. This is the one place a
    /// stored original is destroyed on request (`auto-crop backups purge`); callers decide which
    /// entries may go. Returns whether the entry is gone.
    pub fn remove(&self, id: &str) -> bool {
        match self.entry_dir(id) {
            Some(dir) if dir.is_dir() => fs::remove_dir_all(dir).is_ok(),
            _ => false,
        }
    }

    /// Deletes backups past their retention that are not pinned. Only directories inside the store
    /// with a valid id are ever removed, and a backup whose output was never committed is kept.
    pub fn purge(&self, now: i64) -> usize {
        let mut n = 0;
        for m in self.list() {
            let expired = m.purge_after.is_some_and(|t| t <= now);
            if expired
                && !m.pinned
                && m.state != BackupState::BackedUp
                && let Some(dir) = self.entry_dir(&m.id)
                && fs::remove_dir_all(dir).is_ok()
            {
                n += 1;
            }
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(store: &Store, dir: &Path, bytes: &[u8], retention: Option<u32>) -> Manifest {
        let src = dir.join("a.jpg");
        fs::write(&src, bytes).unwrap();
        store
            .create(&NewBackup {
                source: &src,
                source_blake3: &blake3_hex(bytes),
                source_size: bytes.len() as u64,
                source_mtime_ms: 0,
                format_ext: "jpg",
                run_id: "run",
                run_name: "Run",
                retention_days: retention,
                edit: None,
            })
            .unwrap()
    }

    #[test]
    fn a_backup_holds_the_exact_bytes_and_a_readable_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join("backups"));
        let m = make(&store, tmp.path(), b"original bytes", Some(30));
        assert_eq!(
            fs::read(store.original_path(&m).unwrap()).unwrap(),
            b"original bytes"
        );
        let back = store.read(&m.id).unwrap();
        assert_eq!(back.original_blake3, blake3_hex(b"original bytes"));
        assert_eq!(back.state, BackupState::BackedUp);
        assert!(back.purge_after.is_some());
        assert_eq!(store.list().len(), 1);
        assert!(store.used_bytes() > 0);
    }

    #[test]
    fn a_hash_mismatch_refuses_the_backup_and_leaves_nothing_behind() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join("backups"));
        let src = tmp.path().join("a.jpg");
        fs::write(&src, b"abc").unwrap();
        let err = store
            .create(&NewBackup {
                source: &src,
                source_blake3: "wrong",
                source_size: 3,
                source_mtime_ms: 0,
                format_ext: "jpg",
                run_id: "r",
                run_name: "r",
                retention_days: None,
                edit: None,
            })
            .unwrap_err();
        assert_eq!(err, ErrKind::BackupFailed);
        assert!(store.list().is_empty());
        assert_eq!(fs::read_dir(store.dir()).unwrap().count(), 0);
    }

    #[test]
    fn purge_removes_only_expired_unpinned_saved_backups() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join("backups"));
        let mut expired = make(&store, tmp.path(), b"1", Some(1));
        let mut pinned = make(&store, tmp.path(), b"2", Some(1));
        let mut unsaved = make(&store, tmp.path(), b"3", Some(1));
        let mut fresh = make(&store, tmp.path(), b"4", Some(30));
        for m in [&mut expired, &mut pinned, &mut fresh] {
            m.state = BackupState::Saved;
        }
        pinned.pinned = true;
        unsaved.state = BackupState::BackedUp;
        for m in [&expired, &pinned, &unsaved, &fresh] {
            store.write(m).unwrap();
        }
        let in_two_days = now_secs() + 2 * 86_400;
        assert_eq!(store.purge(in_two_days), 1);
        assert!(store.read(&expired.id).is_none());
        assert!(store.read(&pinned.id).is_some());
        assert!(
            store.read(&unsaved.id).is_some(),
            "an uncommitted backup is never purged"
        );
        assert!(store.read(&fresh.id).is_some());
    }

    #[test]
    fn ids_that_could_escape_the_store_are_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join("backups"));
        assert!(store.read("../../etc").is_none());
        assert!(store.read("").is_none());
        assert!(store.entry_dir("zzzzzzzzzzzzzzzzzzzzzzzzzzzz").is_none());
    }

    /// M10.25: a manifest written before the 1-to-N fields (kind, per-output item id and index,
    /// moved derived files) still loads, as a one-to-one backup, and restores as before.
    #[test]
    fn a_manifest_written_before_m10_loads_as_one_to_one() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join("backups"));
        let id = "0123456789abcdef0123456789ab";
        let dir = tmp.path().join("backups").join(id);
        fs::create_dir_all(&dir).unwrap();
        let old = serde_json::json!({
            "schema": 1, "id": id, "run_id": "r1", "run_name": "Old run", "created_at": 1_790_000_000,
            "original_path": "C:\\photos\\a.jpg", "original_name": "a.jpg",
            "original_file": "original.jpg", "original_blake3": "00", "original_size": 5,
            "original_mtime_ms": 1000, "format": "jpg",
            "outputs": [{"path": "C:\\photos\\a.jpg", "blake3": "ff", "size": 3, "mtime_ms": 1000}],
            "state": "Saved", "pinned": false, "purge_after": null, "engine_version": "0.0.1",
            "edit": {"version": 1, "items": [{"id": 1, "include": true,
                "geometry": {"type": "quad", "corners": [{"x":0.1,"y":0.1},{"x":0.9,"y":0.1},{"x":0.9,"y":0.9},{"x":0.1,"y":0.9}], "quarterTurns": 0, "fineDeg": 0.0},
                "origin": {"kind": "manual"}}]},
            "restored_at": null
        });
        fs::write(dir.join("manifest.json"), serde_json::to_vec(&old).unwrap()).unwrap();
        let m = store.read(id).expect("the old manifest loads");
        assert_eq!(m.kind, BackupKind::OneToOne);
        assert_eq!(m.outputs.len(), 1);
        assert_eq!((m.outputs[0].item_id, m.outputs[0].index), (None, None));
        assert!(m.derived_moved.is_empty());
        assert_eq!(
            m.edit.as_ref().unwrap().version,
            auto_crop_core::EDIT_STATE_VERSION
        );
        assert_eq!(m.edit.as_ref().unwrap().items.len(), 1);
        // A one-to-one manifest written now has no trace of the new fields in its outputs, so an
        // older build reads it unchanged.
        let json = serde_json::to_value(&m).unwrap();
        assert!(json["outputs"][0].get("item_id").is_none());
        // The guard finds an output by hash, in any backup.
        assert_eq!(
            store.find_output("ff").map(|(m, i)| (m.id, i)),
            Some((id.to_owned(), 0))
        );
        assert!(store.find_output("nope").is_none());
    }

    #[test]
    fn a_newer_manifest_schema_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join("backups"));
        let mut m = make(&store, tmp.path(), b"x", None);
        m.schema = MANIFEST_SCHEMA + 1;
        store.write(&m).unwrap();
        assert!(store.read(&m.id).is_none());
    }
}
