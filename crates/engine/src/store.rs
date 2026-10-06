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
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

pub const MANIFEST_SCHEMA: u32 = 1;

/// How a backup of an original was made (PLAN 2.7 "Methods"): reflink beats hardlink beats copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackupMethod {
    /// Copy-on-write clone: the data blocks are shared until one side changes, so the backup costs
    /// nothing and stays independent of later writes.
    Reflink,
    /// A second name for the same file (same volume only). Safe because the engine never writes
    /// through an existing file: a save puts a new file in its place and the old one lives on
    /// under the backup's name.
    Hardlink,
    /// A copy, re-read and checked against the original's hash, then fsynced.
    Copy,
}

/// Which methods a store may try. `Auto` is reflink, then hardlink, then copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MethodPref {
    #[default]
    Auto,
    /// Hardlink, else copy (no reflink).
    Hardlink,
    /// Always copy (cross-volume stores behave like this by necessity).
    Copy,
}

impl MethodPref {
    fn to_u8(self) -> u8 {
        match self {
            MethodPref::Auto => 0,
            MethodPref::Hardlink => 1,
            MethodPref::Copy => 2,
        }
    }

    fn from_u8(v: u8) -> Self {
        match v {
            1 => MethodPref::Hardlink,
            2 => MethodPref::Copy,
            _ => MethodPref::Auto,
        }
    }
}

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
    /// How the original was backed up; absent in a manifest written before M2.34.
    pub method: Option<BackupMethod>,
    /// The original's permission bits (Unix mode) or file attributes (Windows), restored with it.
    pub original_attrs: Option<u32>,
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
            method: None,
            original_attrs: None,
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
    /// The method preference, shared by every clone (the engine changes it at run time).
    pref: Arc<AtomicU8>,
}

/// A symlink, or on Windows any reparse point (a junction, a mount point, a placeholder).
fn is_link_like(meta: &fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    false
}

/// The permission bits (Unix) or attributes (Windows) a restore puts back.
fn attrs_of(meta: &fs::Metadata) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(meta.mode() & 0o7777)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        Some(meta.file_attributes())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = meta;
        None
    }
}

/// Puts the saved `attrs` on a freshly written file: the mode on Unix, the read-only bit on
/// Windows (the other attribute bits are not settable through `std`). Best effort.
pub(crate) fn apply_attrs(path: &Path, attrs: Option<u32>) {
    let Some(a) = attrs else { return };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(a));
    }
    #[cfg(windows)]
    if a & 0x1 != 0
        && let Ok(m) = fs::metadata(path)
    {
        let mut p = m.permissions();
        p.set_readonly(true);
        let _ = fs::set_permissions(path, p);
    }
    #[cfg(not(any(unix, windows)))]
    let _ = (path, a);
}

fn sync_file(path: &Path) -> std::io::Result<()> {
    fs::OpenOptions::new().write(true).open(path)?.sync_all()
}

/// fsyncs a directory so a new or renamed entry survives power loss (Unix; NTFS journals its
/// metadata and Windows has no directory fsync, PLAN 2.7).
pub(crate) fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(f) = fs::File::open(dir) {
        let _ = f.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

/// Puts `src` at `dest` by the best method `pref` allows, falling through reflink, hardlink and
/// copy. Whatever a failed attempt left at `dest` is removed before the next.
fn place_backup(src: &Path, dest: &Path, pref: MethodPref) -> std::io::Result<BackupMethod> {
    let clear = || {
        let _ = fs::remove_file(dest);
    };
    if pref == MethodPref::Auto {
        match reflink_copy::reflink(src, dest) {
            Ok(()) => return Ok(BackupMethod::Reflink),
            Err(_) => clear(),
        }
    }
    if pref != MethodPref::Copy {
        match fs::hard_link(src, dest) {
            Ok(()) => return Ok(BackupMethod::Hardlink),
            Err(_) => clear(),
        }
    }
    fs::copy(src, dest)?;
    Ok(BackupMethod::Copy)
}

impl Store {
    /// A store at `dir` that makes **copies**. The engine switches to [`MethodPref::Auto`]
    /// (reflink, then hardlink, then copy; the default of [`crate::EngineOptions`]) through
    /// [`Store::set_method`]. A bare store stays conservative: a hardlink shares the original's
    /// data, so a program that writes the original in place between the backup and the swap would
    /// change the backup too (the engine's re-hash right before the swap refuses to go on, but
    /// the backup then holds that program's version, not the one that was read).
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            pref: Arc::new(AtomicU8::new(MethodPref::Copy.to_u8())),
        }
    }

    /// A store that tries only the methods `pref` allows.
    pub fn with_method(self, pref: MethodPref) -> Self {
        self.set_method(pref);
        self
    }

    pub fn set_method(&self, pref: MethodPref) {
        self.pref.store(pref.to_u8(), Ordering::Relaxed);
    }

    pub fn method_pref(&self) -> MethodPref {
        MethodPref::from_u8(self.pref.load(Ordering::Relaxed))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn entry_dir(&self, id: &str) -> Option<PathBuf> {
        // Ids are our own 28 hex digits; anything else is refused so no path can escape the store.
        (id.len() == 28 && id.bytes().all(|b| b.is_ascii_hexdigit())).then(|| self.dir.join(id))
    }

    /// [`Store::entry_dir`] for deleting: the folder must be a real directory (not a symlink or a
    /// junction) that sits directly inside the store, after resolving both, so no entry can make a
    /// purge or a clean-up reach outside the store (ROADMAP M2.35).
    fn deletable_entry_dir(&self, id: &str) -> Option<PathBuf> {
        let dir = self.entry_dir(id)?;
        let meta = fs::symlink_metadata(&dir).ok()?;
        if !meta.is_dir() || is_link_like(&meta) {
            return None;
        }
        let real = fs::canonicalize(&dir).ok()?;
        let store = fs::canonicalize(&self.dir).ok()?;
        (real.parent() == Some(store.as_path())).then_some(dir)
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
        let Some(dir) = self.deletable_entry_dir(id) else {
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
            let method = place_backup(req.source, &dest, self.method_pref())
                .map_err(|_| ErrKind::BackupFailed)?;
            // Every method is re-read and checked against the hash taken when the file was
            // opened: a clone or a link of a file that changed since is refused too.
            let bytes = fs::read(&dest).map_err(|_| ErrKind::BackupFailed)?;
            if blake3_hex(&bytes) != req.source_blake3 {
                return Err(ErrKind::BackupFailed);
            }
            // A copy has data blocks of its own to flush; a clone or a link rests on the
            // directory entry and the file system's journal, which the directory fsync covers.
            if method == BackupMethod::Copy {
                sync_file(&dest).map_err(|_| ErrKind::BackupFailed)?;
            }
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
                method: Some(method),
                original_attrs: fs::metadata(req.source).ok().and_then(|m| attrs_of(&m)),
                state: BackupState::BackedUp,
                purge_after: req.retention_days.map(|d| now + i64::from(d) * 86_400),
                edit: req.edit.clone(),
                ..Manifest::default()
            };
            self.write(&m)?;
            // The entry's own directory and the store's: the backup is durable before anything
            // is replaced (the data was synced above, the names are synced here).
            sync_dir(&dir);
            sync_dir(&self.dir);
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
        fs::rename(&tmp, dir.join("manifest.json")).map_err(|e| ErrKind::from_io(&e))?;
        sync_dir(&dir);
        Ok(())
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
    /// Only a real directory inside the store with a valid id is ever removed (never a link or a junction). This is the one place a
    /// stored original is destroyed on request (`auto-crop backups purge`); callers decide which
    /// entries may go. Returns whether the entry is gone.
    pub fn remove(&self, id: &str) -> bool {
        match self.deletable_entry_dir(id) {
            Some(dir) if dir.is_dir() => fs::remove_dir_all(dir).is_ok(),
            _ => false,
        }
    }

    /// [`Store::purge`] at most once per `every_secs` (PLAN 2.7: a purge at start, then every 24
    /// hours): the time of the last run is kept in `backups.last_purge` next to the store. A marker that cannot be read or
    /// written only makes the purge run, never skip.
    pub fn purge_if_due(&self, now: i64, every_secs: i64) -> usize {
        let marker = self.purge_marker();
        let last = fs::read_to_string(&marker)
            .ok()
            .and_then(|t| t.trim().parse::<i64>().ok());
        if last.is_some_and(|l| now >= l && now - l < every_secs) {
            return 0;
        }
        let n = self.purge(now);
        if let Some(parent) = marker.parent()
            && fs::create_dir_all(parent).is_ok()
        {
            let _ = fs::write(&marker, now.to_string());
        }
        n
    }

    /// Where the time of the last purge is kept: a sibling of the store (`backups.last_purge`), so
    /// the store folder holds backup entries and the journals folder only.
    fn purge_marker(&self) -> PathBuf {
        let mut name = self
            .dir
            .file_name()
            .map(|n| n.to_os_string())
            .unwrap_or_default();
        name.push(".last_purge");
        self.dir.with_file_name(name)
    }

    /// The warnings a Backups panel shows (PLAN 2.7, PROVISIONAL numbers): `store.large` at 10 GB
    /// used, `store.low_space` under 5 GB free on the store's volume.
    pub fn warnings(&self) -> Vec<&'static str> {
        const GB: u64 = 1 << 30;
        let mut w = Vec::new();
        if self.used_bytes() >= 10 * GB {
            w.push("store.large");
        }
        if crate::space::available_space(&self.dir).is_ok_and(|f| f < 5 * GB) {
            w.push("store.low_space");
        }
        w
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
                && let Some(dir) = self.deletable_entry_dir(&m.id)
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
    fn a_purge_runs_at_most_once_a_day() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join("backups"));
        let mut m = make(&store, tmp.path(), b"1", Some(1));
        m.state = BackupState::Saved;
        store.write(&m).unwrap();
        let day = 86_400;
        let t0 = now_secs() + 2 * day; // the entry is expired by then
        assert_eq!(store.purge_if_due(t0, day), 1);
        // A second entry expires an hour later; the purge is not due yet.
        let mut m2 = make(&store, tmp.path(), b"2", Some(1));
        m2.state = BackupState::Saved;
        store.write(&m2).unwrap();
        assert_eq!(store.purge_if_due(t0 + 3600, day), 0);
        assert!(store.read(&m2.id).is_some());
        // A day later it is.
        assert_eq!(store.purge_if_due(t0 + day, day), 1);
        assert!(store.read(&m2.id).is_none());
        // A damaged marker never blocks a purge.
        fs::write(store.purge_marker(), "not a number").unwrap();
        assert_eq!(store.purge_if_due(t0 + 2 * day, day), 0); // nothing left, but it ran
    }

    #[test]
    fn day_short_entries_survive_and_expired_ones_do_not() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join("backups"));
        let mut m = make(&store, tmp.path(), b"1", Some(30));
        m.state = BackupState::Saved;
        let expiry = m.purge_after.unwrap();
        store.write(&m).unwrap();
        assert_eq!(store.purge(expiry - 86_400), 0, "a day short: kept");
        assert_eq!(store.purge(expiry - 1), 0, "a second short: kept");
        assert_eq!(store.purge(expiry), 1, "at expiry: purged");
    }

    /// A hostile entry (a link that points outside the store, a manifest whose names climb out)
    /// is never followed or deleted through.
    #[test]
    fn hostile_entries_never_make_a_purge_leave_the_store() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join("backups"));
        let outside = tmp.path().join("precious");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("keep.txt"), b"mine").unwrap();
        // An expired, Saved manifest in a real entry whose names try to climb out.
        let mut m = make(&store, tmp.path(), b"1", Some(1));
        m.state = BackupState::Saved;
        m.original_file = "../../precious/keep.txt".to_owned();
        m.original_name = "../../precious/keep.txt".to_owned();
        store.write(&m).unwrap();
        assert!(
            store
                .original_path(&m)
                .unwrap()
                .starts_with(store.entry_path(&m.id).unwrap()),
            "the backed-up file name is reduced to a file name inside the entry"
        );
        // An entry that is a link to the outside folder.
        let id = "0123456789abcdef0123456789ab";
        let link = store.dir().join(id);
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&outside, &link).is_ok();
        #[cfg(windows)]
        let linked = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&outside)
            .output()
            .is_ok_and(|o| o.status.success());
        #[cfg(not(any(unix, windows)))]
        let linked = false;
        if linked {
            let hostile = Manifest {
                id: id.to_owned(),
                state: BackupState::Saved,
                purge_after: Some(1),
                ..Manifest::default()
            };
            fs::write(
                outside.join("manifest.json"),
                serde_json::to_vec(&hostile).unwrap(),
            )
            .unwrap();
            assert_eq!(store.purge(i64::MAX), 1, "only the real entry goes");
            assert!(!store.remove_unused(id), "a linked entry is never removed");
        } else {
            assert_eq!(store.purge(i64::MAX), 1);
        }
        assert_eq!(fs::read(outside.join("keep.txt")).unwrap(), b"mine");
        assert!(outside.join("manifest.json").exists() || !linked);
    }

    #[test]
    fn a_backup_records_how_it_was_made_and_each_method_restores_the_exact_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        for (pref, allowed) in [
            (MethodPref::Copy, vec![BackupMethod::Copy]),
            (
                MethodPref::Hardlink,
                vec![BackupMethod::Hardlink, BackupMethod::Copy],
            ),
            (
                MethodPref::Auto,
                vec![
                    BackupMethod::Reflink,
                    BackupMethod::Hardlink,
                    BackupMethod::Copy,
                ],
            ),
        ] {
            let store = Store::new(tmp.path().join(format!("backups-{pref:?}"))).with_method(pref);
            let m = make(&store, tmp.path(), b"the exact original bytes", Some(30));
            let method = m.method.expect("the method is recorded");
            assert!(allowed.contains(&method), "{pref:?} gave {method:?}");
            assert_eq!(
                fs::read(store.original_path(&m).unwrap()).unwrap(),
                b"the exact original bytes"
            );
            // The manifest on disk says it too.
            assert_eq!(store.read(&m.id).unwrap().method, Some(method));
        }
    }

    /// A link or a clone shares the data of the original; the original being replaced afterwards
    /// (a new file renamed over the path) leaves the backup as it was.
    #[test]
    fn replacing_the_original_does_not_touch_a_linked_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join("backups")).with_method(MethodPref::Hardlink);
        let m = make(&store, tmp.path(), b"original", Some(30));
        let src = tmp.path().join("a.jpg");
        let t = crate::commit::write_temp(tmp.path(), b"cropped output", None).unwrap();
        crate::commit::swap(&t.path, &src).unwrap();
        assert_eq!(fs::read(&src).unwrap(), b"cropped output");
        assert_eq!(
            fs::read(store.original_path(&m).unwrap()).unwrap(),
            b"original"
        );
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
