// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The group commit protocol and its crash recovery (ROADMAP M10.23, M10.24; PLAN 2.7 "1-to-N").
//! One scan becomes N files, and the set is all or nothing: at every instant the folder holds
//! either the scan alone or the complete verified set (plus the scan when it could not be
//! removed), never a partial set, and a verified backup of the scan exists before anything is
//! replaced.
//!
//! ```text
//!  1  backup the scan (verified copy)            nothing visible changed
//!  2  journal Writing {temp names}               crash: delete temps and the unused backup
//!  3  per output: encode, temp, fsync, verify    crash: same
//!  4  keep what a re-save replaces (hard link)   crash: same
//!  5  journal Committing {hashes}  (durable)     crash: roll forward if all N verify, else back
//!  6  re-stat the scan (SourceChanged aborts)    abort: roll back
//!  7  N times: no-clobber move into place        abort: remove what was placed (ours only)
//!  ---------------------------- commit point: the complete set exists ----------------------
//!  8  fsync the folder, retire surplus old outputs
//!  9  re-check the scan, then unlink it LAST     locked: set stays, scan stays (SavedSourceInUse)
//! 10  manifest Saved {outputs}, delete journal   crash: recovery finishes (idempotent)
//! ```
//!
//! The journal is one small file per group in `<store>/.groups/`, replaced atomically and
//! fsynced. It names every temp, final path and hash, so recovery needs nothing else. Recovery
//! ([`recover`]) runs at start: a `Writing` journal deletes its temps and the unused backup;
//! a `Committing` journal rolls forward only if every one of the N outputs can be verified against
//! the journalled hashes (as a placed file or as an intact temp), otherwise it rolls back to the
//! scan plus its backup. It only ever deletes files whose bytes match a journalled hash.
//!
//! **The same protocol serves the one-file in-place save** ([`commit_single`], ROADMAP M2.83):
//! the "set" is one output whose final name is the file being replaced. The backup holds the
//! original (or, for a re-save, the original is already there and the previous output is kept in
//! `superseded/`), the verified temp replaces the file by the atomic swap (`ReplaceFileW` on
//! Windows), and the commit point is that swap. The journal, the fsync order, the hashes, the
//! recovery rule (before `Committing` back, from then on forward) and the fault points are the
//! same code as the group's; only what "back" means differs, because the file being replaced is
//! also the scan: recovery puts the old bytes back from the backup when the target holds the new
//! output or nothing, and leaves a file somebody else wrote alone.
//!
//! **Fault injection.** Every step calls a [`FaultHook`]. The production hook never faults; the
//! tests fail or "crash" (return at once, leaving the disk exactly as it is) at every step and
//! between every pair of renames, then run recovery and assert the invariant. A hook cannot skip
//! the backup or the verification; it can only make steps fail.

use crate::commit::{
    Expect, TempWrite, VerifyMode, retryable, swap, verify_temp_expect, write_temp_at,
};
use crate::error::ErrKind;
use crate::source::hash_file;
use crate::store::{BackupKind, BackupState, NewBackup, OutputRec, Store};
use crate::util::{new_id, now_secs, unix_ms};
use auto_crop_codecs::Format;
use auto_crop_core::EditState;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

// ------------------------------------------------------------------ fault injection

/// A point in the protocol. Indices are output indices (0-based, in output order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Step {
    Planned,
    BackupCreated,
    JournalWriting,
    TempWritten(usize),
    TempVerified(usize),
    Preserved(usize),
    JournalCommitting,
    SourceRestat,
    BeforeRename(usize),
    AfterRename(usize),
    RenamesDone,
    DirSynced,
    Retired(usize),
    BeforeUnlink,
    AfterUnlink,
    ManifestSaved,
    JournalRemoved,
}

impl Step {
    /// True for the steps after the commit point: a failure there never rolls the set back.
    pub fn after_commit(self) -> bool {
        matches!(
            self,
            Step::RenamesDone
                | Step::DirSynced
                | Step::Retired(_)
                | Step::BeforeUnlink
                | Step::AfterUnlink
                | Step::ManifestSaved
                | Step::JournalRemoved
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    Pass,
    /// The step fails with this error; the protocol reacts as it would to the real failure.
    Fail(ErrKind),
    /// The process dies here: the call returns immediately and leaves the disk as it is.
    Crash,
}

pub trait FaultHook {
    fn at(&self, step: &Step) -> Fault;
}

impl<F: Fn(&Step) -> Fault> FaultHook for F {
    fn at(&self, step: &Step) -> Fault {
        self(step)
    }
}

/// The production hook.
pub struct NoFaults;

impl FaultHook for NoFaults {
    fn at(&self, _: &Step) -> Fault {
        Fault::Pass
    }
}

enum Abort {
    Fail(ErrKind),
    Crash,
}

type Flow<T> = Result<T, Abort>;

fn fp(hook: &dyn FaultHook, step: Step) -> Flow<()> {
    match hook.at(&step) {
        Fault::Pass => Ok(()),
        Fault::Fail(k) => Err(Abort::Fail(k)),
        Fault::Crash => Err(Abort::Crash),
    }
}

/// A failpoint after the commit point: a failure is recorded as a skipped step, only a crash
/// aborts.
fn fp_post(hook: &dyn FaultHook, step: Step) -> Flow<Option<ErrKind>> {
    match hook.at(&step) {
        Fault::Pass => Ok(None),
        Fault::Fail(k) => Ok(Some(k)),
        Fault::Crash => Err(Abort::Crash),
    }
}

// ------------------------------------------------------------------ the journal

pub const JOURNAL_SCHEMA: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    /// Temps may exist (their names are listed); nothing is in place yet.
    Writing,
    /// Every temp is verified, the backup and the kept old files exist, hashes are recorded.
    Committing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRec {
    pub path: String,
    pub size: u64,
    pub mtime_ms: i64,
    pub blake3: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JReplace {
    /// The unchanged previous output this one takes over (a re-save).
    pub old_blake3: String,
    /// Where its bytes were kept (empty: a copy-mode re-save, which keeps nothing).
    pub stored: String,
    /// `stored` is the backup's own `original.<ext>` (an in-place first save): it is read to put
    /// the old bytes back and is never deleted here; the backup entry is removed as a whole.
    #[serde(default)]
    pub stored_is_backup: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JOutput {
    pub item_id: u32,
    pub index: u32,
    pub temp: String,
    pub final_path: String,
    /// Empty until the temp is written.
    pub blake3: String,
    pub size: u64,
    pub mtime_ms: i64,
    pub replaces: Option<JReplace>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JRetire {
    pub path: String,
    pub old_blake3: String,
    pub stored: String,
}

/// The process that is running a group commit: recovery in another process leaves a live
/// owner's journal alone (a CLI starting while the GUI is saving must not roll the GUI's
/// half-finished group back).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Owner {
    pub pid: u32,
    /// The process start time (seconds since the epoch) so a reused pid is not mistaken for it.
    pub started: u64,
}

/// When process `pid` started, if it is running (`None` if it is not).
pub fn process_started(pid: u32) -> Option<u64> {
    let mut sys = sysinfo::System::new();
    let p = sysinfo::Pid::from_u32(pid);
    sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[p]), true);
    sys.process(p).map(|p| p.start_time())
}

/// This process's own start time, read once (looking a process up is not free).
fn own_owner() -> Owner {
    static OWN: std::sync::OnceLock<Owner> = std::sync::OnceLock::new();
    OWN.get_or_init(|| {
        let pid = std::process::id();
        Owner {
            pid,
            started: process_started(pid).unwrap_or(0),
        }
    })
    .clone()
}

/// Journals of commits running in THIS process (a second `Engine` in the same process must not
/// recover them either).
static ACTIVE: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

struct ActiveGuard(String);

impl ActiveGuard {
    fn new(id: &str) -> Self {
        ACTIVE
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(id.to_owned());
        Self(id.to_owned())
    }
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        ACTIVE
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|i| *i != self.0);
    }
}

fn is_active_here(id: &str) -> bool {
    ACTIVE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .any(|i| i == id)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Journal {
    pub schema: u32,
    pub id: String,
    /// Who is committing; absent in a journal that nobody is working on any more.
    #[serde(default)]
    pub owner: Option<Owner>,
    pub phase: Phase,
    pub created_at: i64,
    pub source: SourceRec,
    /// Replace mode: the scan goes away once the set is complete.
    pub unlink_source: bool,
    pub backup_id: Option<String>,
    /// This group made the backup (so a rollback may discard it); false for a re-save.
    pub backup_created: bool,
    pub outputs: Vec<JOutput>,
    pub retire: Vec<JRetire>,
    #[serde(default, deserialize_with = "crate::migrate::deserialize_optional")]
    pub edit: Option<EditState>,
    /// A one-file save over the source itself ([`commit_single`]): `outputs[0].final_path` is the
    /// file being replaced and `source` describes what is there now.
    #[serde(default)]
    pub in_place: bool,
}

fn path_str(p: &Path) -> Result<String, ErrKind> {
    p.to_str().map(str::to_owned).ok_or(ErrKind::Internal)
}

fn journal_path(store: &Store, id: &str) -> PathBuf {
    store.groups_dir().join(format!("{id}.json"))
}

fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(f) = fs::File::open(dir) {
        let _ = f.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

fn write_journal(store: &Store, j: &Journal) -> Result<(), ErrKind> {
    let dir = store.groups_dir();
    fs::create_dir_all(&dir).map_err(|e| ErrKind::from_io(&e))?;
    let json = serde_json::to_vec_pretty(j).map_err(|_| ErrKind::Internal)?;
    let tmp = dir.join(format!(".{}.json.tmp", j.id));
    let mut f = fs::File::create(&tmp).map_err(|e| ErrKind::from_io(&e))?;
    f.write_all(&json).map_err(|e| ErrKind::from_io(&e))?;
    f.sync_all().map_err(|e| ErrKind::from_io(&e))?;
    drop(f);
    fs::rename(&tmp, journal_path(store, &j.id)).map_err(|e| ErrKind::from_io(&e))?;
    sync_dir(&dir);
    Ok(())
}

fn read_journal(path: &Path) -> Option<Journal> {
    let j: Journal = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    (j.schema <= JOURNAL_SCHEMA).then_some(j)
}

fn file_hash(p: &Path) -> Option<String> {
    hash_file(p).ok().map(|id| id.to_hex())
}

fn exists(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok()
}

// ------------------------------------------------------------------ the request

/// What a scan looked like when it was opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFingerprint {
    pub path: PathBuf,
    pub size: u64,
    pub mtime_ms: i64,
    pub blake3: String,
}

pub enum BackupPlan<'a> {
    /// Replace mode, first save: copy the scan into the store, verified.
    New(NewBackup<'a>),
    /// A re-save: the scan is already in the store under this id.
    Existing(String),
    /// A copy: the scan stays where it is, no backup is needed.
    None,
}

/// One planned output.
#[derive(Debug, Clone)]
pub struct OutSpec {
    pub item_id: u32,
    /// 1-based rank (`{n}`).
    pub index: u32,
    pub final_path: PathBuf,
    /// The previous output at `final_path` that this one takes over, if it is still unchanged.
    pub replaces_blake3: Option<String>,
}

/// A previous output with no successor (N went down): it is kept in the store, then removed.
#[derive(Debug, Clone)]
pub struct Retire {
    pub path: PathBuf,
    pub expected_blake3: String,
}

pub struct GroupRequest<'a> {
    pub store: &'a Store,
    pub source: SourceFingerprint,
    /// Replace mode: remove the scan after the set is complete.
    pub unlink_source: bool,
    pub backup: BackupPlan<'a>,
    pub outputs: Vec<OutSpec>,
    pub retire: Vec<Retire>,
    /// Applied to every output (the scan's own mtime, kept by default).
    pub mtime: SystemTime,
    /// The edit state recorded in the manifest.
    pub edit: Option<EditState>,
}

/// What one produced output looks like.
pub struct Produced {
    pub bytes: Vec<u8>,
    pub dims: (u32, u32),
    pub format: Format,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupSaved {
    pub outputs: Vec<OutputRec>,
    pub backup_id: Option<String>,
    /// The scan was removed (Replace mode).
    pub source_removed: bool,
    /// Something after the commit point did not go through; the set itself is complete.
    /// `SavedSourceInUse`: the scan could not be removed; `SourceChanged`: it was changed by
    /// another program and was left alone.
    pub notes: Vec<ErrKind>,
    /// The journal is still on disk (recovery will finish the bookkeeping).
    pub journal_pending: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupError {
    /// Nothing is left behind except what the error says.
    Failed(ErrKind),
    /// A fault hook "killed the process" (tests only).
    Crashed,
}

// ------------------------------------------------------------------ small file operations

/// Removes a file, retrying sharing violations like the swap does.
fn remove_with_retry(p: &Path) -> Result<(), ErrKind> {
    let mut last = ErrKind::Internal;
    for (i, wait) in std::iter::once(&0u64)
        .chain(crate::commit::RETRY_MS.iter())
        .enumerate()
    {
        if i > 0 {
            std::thread::sleep(Duration::from_millis(*wait));
        }
        match fs::remove_file(p) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => {
                last = ErrKind::from_io(&e);
                if !retryable(&e) {
                    break;
                }
            }
        }
    }
    Err(match last {
        ErrKind::ReadOnly => ErrKind::FileInUse,
        k => k,
    })
}

enum LinkErr {
    Exists,
    Other(ErrKind),
}

/// Moves `temp` to `dest` without ever replacing an existing file: a hard link (which fails with
/// "already exists" atomically) and then the temp name is dropped. Where links are not available
/// (FAT, some network shares) the name is checked and the file renamed; that fallback has a small
/// check-then-rename window, which the phase-2 recheck and the journal's hash check bound.
fn link_no_clobber(temp: &Path, dest: &Path) -> Result<(), LinkErr> {
    match fs::hard_link(temp, dest) {
        Ok(()) => {
            // The final name now holds the bytes; a leftover temp name is swept later.
            let _ = remove_with_retry(temp);
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(LinkErr::Exists),
        Err(_) => {
            if exists(dest) {
                return Err(LinkErr::Exists);
            }
            fs::rename(temp, dest).map_err(|e| LinkErr::Other(ErrKind::from_io(&e)))
        }
    }
}

/// [`link_no_clobber`] with the engine's error codes: a name that exists is `PlanStale`.
pub(crate) fn move_no_clobber(temp: &Path, dest: &Path) -> Result<(), ErrKind> {
    link_no_clobber(temp, dest).map_err(|e| match e {
        LinkErr::Exists => ErrKind::PlanStale,
        LinkErr::Other(k) => k,
    })
}

/// Keeps a previous output's bytes in the store: a hard link (no copy), else a verified copy.
fn preserve(src: &Path, stored: &Path, expect: &str) -> Result<(), ErrKind> {
    if let Some(dir) = stored.parent() {
        fs::create_dir_all(dir).map_err(|e| ErrKind::from_io(&e))?;
    }
    if exists(stored) {
        return if file_hash(stored).as_deref() == Some(expect) {
            Ok(())
        } else {
            Err(ErrKind::Internal)
        };
    }
    if fs::hard_link(src, stored).is_err() {
        fs::copy(src, stored).map_err(|e| ErrKind::from_io(&e))?;
        fs::OpenOptions::new()
            .write(true)
            .open(stored)
            .and_then(|f| f.sync_all())
            .map_err(|e| ErrKind::from_io(&e))?;
    }
    if file_hash(stored).as_deref() == Some(expect) {
        Ok(())
    } else {
        let _ = fs::remove_file(stored);
        Err(ErrKind::VerifyFailed)
    }
}

// ------------------------------------------------------------------ the commit

/// Commits a scan's N outputs as one group (M10.23). `produce(i)` encodes output `i` (called once
/// per output, in order; the encoded bytes are written at once and dropped, so only one lives in
/// memory at a time). On failure everything is rolled back and the error says why.
pub fn commit_group(
    req: &GroupRequest<'_>,
    produce: &mut dyn FnMut(usize) -> Result<Produced, ErrKind>,
    hook: &dyn FaultHook,
) -> Result<GroupSaved, GroupError> {
    let mut wrapped = |i: usize| {
        produce(i).map(|produced| Output {
            produced,
            expect: None,
        })
    };
    commit_group_verified(req, &mut wrapped, VerifyMode::Full, hook)
}

/// One produced output together with what it must decode to (PLAN 2.7 step 3).
pub struct Output {
    pub produced: Produced,
    /// The full expectation (ICC, pixels or luma fingerprint); `None` checks the size and the
    /// format only.
    pub expect: Option<Expect>,
}

/// [`commit_group`] where each output carries its [`Expect`] and the verify depth is chosen.
pub fn commit_group_verified(
    req: &GroupRequest<'_>,
    produce: &mut dyn FnMut(usize) -> Result<Output, ErrKind>,
    verify: VerifyMode,
    hook: &dyn FaultHook,
) -> Result<GroupSaved, GroupError> {
    commit_impl(
        req,
        produce,
        Opts {
            in_place: false,
            verify,
        },
        hook,
    )
}

/// What one in-place save is asked to do ([`commit_single`]).
pub struct SingleRequest<'a> {
    pub store: &'a Store,
    /// What sits at the target now: the original on a first save, the previous output on a re-save
    /// (its hash is what the swap is allowed to replace).
    pub target: SourceFingerprint,
    /// `New` on a first save, `Existing` on a re-save. A save with no backup is not possible.
    pub backup: BackupPlan<'a>,
    /// Applied to the output (the original's mtime, kept by default).
    pub mtime: SystemTime,
    /// The edit state recorded in the manifest.
    pub edit: Option<EditState>,
    pub verify: VerifyMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SingleSaved {
    pub output: OutputRec,
    pub backup_id: String,
    /// Something after the commit point did not finish; recovery completes the bookkeeping.
    pub journal_pending: bool,
}

/// Replaces `target` by one verified output under the full protocol (ROADMAP M2.83, PLAN 2.7):
/// journal, verified backup, fsynced temp beside the target, re-read and re-decode, `Committing`,
/// re-stat and re-hash of the target, atomic swap, manifest `Saved`, journal removed. At every
/// instant the target holds the old bytes or the verified output, and the old bytes are in the
/// backup (or `superseded/`) before the swap.
pub fn commit_single(
    req: &SingleRequest<'_>,
    produce: &mut dyn FnMut() -> Result<Output, ErrKind>,
    hook: &dyn FaultHook,
) -> Result<SingleSaved, GroupError> {
    if matches!(req.backup, BackupPlan::None) {
        return Err(GroupError::Failed(ErrKind::BackupFailed));
    }
    let g = GroupRequest {
        store: req.store,
        source: req.target.clone(),
        unlink_source: false,
        backup: match &req.backup {
            BackupPlan::New(nb) => BackupPlan::New(nb.clone()),
            BackupPlan::Existing(id) => BackupPlan::Existing(id.clone()),
            BackupPlan::None => BackupPlan::None,
        },
        outputs: vec![OutSpec {
            item_id: 0,
            index: 1,
            final_path: req.target.path.clone(),
            replaces_blake3: Some(req.target.blake3.clone()),
        }],
        retire: Vec::new(),
        mtime: req.mtime,
        edit: req.edit.clone(),
    };
    let mut once = |_: usize| produce();
    let saved = commit_impl(
        &g,
        &mut once,
        Opts {
            in_place: true,
            verify: req.verify,
        },
        hook,
    )?;
    let output = saved
        .outputs
        .into_iter()
        .next()
        .ok_or(GroupError::Failed(ErrKind::Internal))?;
    Ok(SingleSaved {
        output,
        backup_id: saved.backup_id.unwrap_or_default(),
        journal_pending: saved.journal_pending,
    })
}

#[derive(Clone, Copy)]
struct Opts {
    in_place: bool,
    verify: VerifyMode,
}

fn commit_impl(
    req: &GroupRequest<'_>,
    produce: &mut dyn FnMut(usize) -> Result<Output, ErrKind>,
    opts: Opts,
    hook: &dyn FaultHook,
) -> Result<GroupSaved, GroupError> {
    let mut state = RunState::default();
    match run(req, produce, opts, hook, &mut state) {
        Ok(saved) => Ok(saved),
        Err(Abort::Crash) => Err(GroupError::Crashed),
        Err(Abort::Fail(kind)) => {
            // Before the commit point: undo exactly what this call did. (No journal yet means
            // nothing was written at all.)
            if let Some(j) = &state.journal
                && rollback(req.store, j).is_err()
            {
                return Err(GroupError::Failed(ErrKind::GroupCommitFailed));
            }
            Err(GroupError::Failed(kind))
        }
    }
}

#[derive(Default)]
struct RunState {
    journal: Option<Journal>,
    active: Option<ActiveGuard>,
}

fn run(
    req: &GroupRequest<'_>,
    produce: &mut dyn FnMut(usize) -> Result<Output, ErrKind>,
    opts: Opts,
    hook: &dyn FaultHook,
    st: &mut RunState,
) -> Flow<GroupSaved> {
    let in_place = opts.in_place;
    if req.outputs.is_empty() {
        return Err(Abort::Fail(ErrKind::NoCrop));
    }
    let dir = req.outputs[0]
        .final_path
        .parent()
        .ok_or(Abort::Fail(ErrKind::Internal))?
        .to_path_buf();
    if req
        .outputs
        .iter()
        .any(|o| o.final_path.parent() != Some(dir.as_path()))
    {
        return Err(Abort::Fail(ErrKind::Internal));
    }
    fp(hook, Step::Planned)?;
    fs::create_dir_all(&dir).map_err(|e| Abort::Fail(ErrKind::from_io(&e)))?;

    // 1. Decide the backup's identity, so the journal can name it before it exists.
    let (backup_id, backup_created) = match &req.backup {
        BackupPlan::New(_) => (Some(new_id()), true),
        BackupPlan::Existing(id) => {
            let m = req
                .store
                .read(id)
                .ok_or(Abort::Fail(ErrKind::OriginalExpired))?;
            let orig = req
                .store
                .original_path(&m)
                .ok_or(Abort::Fail(ErrKind::OriginalExpired))?;
            if file_hash(&orig).as_deref() != Some(m.original_blake3.as_str()) {
                return Err(Abort::Fail(ErrKind::OriginalExpired));
            }
            (Some(m.id), false)
        }
        BackupPlan::None => (None, false),
    };

    // 2. The journal, before the backup or the first temp exists.
    let jid = new_id();
    // Held in `st`, so it outlives `run` and covers a rollback in `commit_group` too.
    st.active = Some(ActiveGuard::new(&jid));
    let mut j = Journal {
        schema: JOURNAL_SCHEMA,
        id: jid.clone(),
        owner: Some(own_owner()),
        phase: Phase::Writing,
        created_at: now_secs(),
        source: SourceRec {
            path: path_str(&req.source.path).map_err(Abort::Fail)?,
            size: req.source.size,
            mtime_ms: req.source.mtime_ms,
            blake3: req.source.blake3.clone(),
        },
        unlink_source: req.unlink_source,
        backup_id: backup_id.clone(),
        backup_created,
        outputs: Vec::new(),
        retire: Vec::new(),
        edit: req.edit.clone(),
        in_place,
    };
    for (i, o) in req.outputs.iter().enumerate() {
        let temp = dir.join(format!(".autocrop-{}.tmp", new_id()));
        let (stored, stored_is_backup) = match (&o.replaces_blake3, &backup_id, &req.backup) {
            // A first in-place save: the old bytes are the backup's own `original.<ext>`.
            (Some(_), Some(bid), BackupPlan::New(nb)) if in_place => (
                req.store
                    .entry_path(bid)
                    .map(|d| d.join(format!("original.{}", nb.format_ext)))
                    .and_then(|p| path_str(&p).ok())
                    .unwrap_or_default(),
                true,
            ),
            (Some(_), Some(bid), _) => (
                req.store
                    .entry_path(bid)
                    .map(|d| d.join("superseded").join(format!("{jid}_{i}")))
                    .and_then(|p| path_str(&p).ok())
                    .unwrap_or_default(),
                false,
            ),
            _ => (String::new(), false),
        };
        j.outputs.push(JOutput {
            item_id: o.item_id,
            index: o.index,
            temp: path_str(&temp).map_err(Abort::Fail)?,
            final_path: path_str(&o.final_path).map_err(Abort::Fail)?,
            blake3: String::new(),
            size: 0,
            mtime_ms: unix_ms(req.mtime),
            replaces: o.replaces_blake3.clone().map(|old| JReplace {
                old_blake3: old,
                stored,
                stored_is_backup,
            }),
        });
    }
    for (k, r) in req.retire.iter().enumerate() {
        let stored = backup_id
            .as_ref()
            .and_then(|bid| req.store.entry_path(bid))
            .map(|d| d.join("superseded").join(format!("{jid}_r{k}")))
            .and_then(|p| path_str(&p).ok())
            .unwrap_or_default();
        j.retire.push(JRetire {
            path: path_str(&r.path).map_err(Abort::Fail)?,
            old_blake3: r.expected_blake3.clone(),
            stored,
        });
    }
    write_journal(req.store, &j).map_err(Abort::Fail)?;
    st.journal = Some(j.clone());
    fp(hook, Step::JournalWriting)?;

    // 2b. The backup: a verified copy, durable before anything else is written.
    if let (BackupPlan::New(nb), Some(id)) = (&req.backup, &backup_id) {
        req.store
            .create_with_id(id.clone(), nb)
            .map_err(Abort::Fail)?;
        fp(hook, Step::BackupCreated)?;
    }

    // 3. Encode, write and verify every temp.
    for i in 0..j.outputs.len() {
        let out = produce(i).map_err(Abort::Fail)?;
        let Output {
            produced: p,
            expect,
        } = out;
        let temp = PathBuf::from(&j.outputs[i].temp);
        // The new file takes the mode of the one it will replace (or, for a new name, of the scan).
        let mode_from = if in_place {
            Some(req.outputs[i].final_path.as_path())
        } else {
            Some(req.source.path.as_path())
        };
        write_temp_at(&temp, &p.bytes, Some(req.mtime), mode_from).map_err(Abort::Fail)?;
        let t = TempWrite {
            path: temp,
            blake3: crate::util::blake3_hex(&p.bytes),
            size: p.bytes.len() as u64,
        };
        j.outputs[i].blake3 = t.blake3.clone();
        j.outputs[i].size = t.size;
        drop(p.bytes);
        st.journal = Some(j.clone());
        fp(hook, Step::TempWritten(i))?;
        let expect = expect.unwrap_or_else(|| Expect::basic(p.dims, p.format));
        verify_temp_expect(&t, &expect, opts.verify).map_err(Abort::Fail)?;
        fp(hook, Step::TempVerified(i))?;
    }

    // 4. Keep what a re-save replaces or retires.
    let mut kept = 0usize;
    for o in &j.outputs {
        if let Some(r) = &o.replaces {
            if file_hash(Path::new(&o.final_path)).as_deref() != Some(r.old_blake3.as_str()) {
                // The file being replaced changed since we read it: an edit by someone else
                // (in place), or a previous output the user edited (a group re-save).
                return Err(Abort::Fail(if in_place {
                    ErrKind::SourceChanged
                } else {
                    ErrKind::PlanStale
                }));
            }
            if !r.stored.is_empty() && !r.stored_is_backup {
                preserve(
                    Path::new(&o.final_path),
                    Path::new(&r.stored),
                    &r.old_blake3,
                )
                .map_err(Abort::Fail)?;
            }
            fp(hook, Step::Preserved(kept))?;
            kept += 1;
        }
    }
    for r in &j.retire {
        if file_hash(Path::new(&r.path)).as_deref() == Some(r.old_blake3.as_str())
            && !r.stored.is_empty()
        {
            preserve(Path::new(&r.path), Path::new(&r.stored), &r.old_blake3)
                .map_err(Abort::Fail)?;
        }
        fp(hook, Step::Preserved(kept))?;
        kept += 1;
    }

    // 5. Committing: everything recovery relies on is durable now.
    j.phase = Phase::Committing;
    write_journal(req.store, &j).map_err(Abort::Fail)?;
    st.journal = Some(j.clone());
    fp(hook, Step::JournalCommitting)?;

    // 6. The scan must still be what we read.
    fp(hook, Step::SourceRestat)?;
    if in_place || req.unlink_source || matches!(req.backup, BackupPlan::None) {
        let now = fs::metadata(&req.source.path)
            .ok()
            .map(|m| (m.len(), m.modified().map(unix_ms).unwrap_or(0)));
        if now != Some((req.source.size, req.source.mtime_ms)) {
            return Err(Abort::Fail(ErrKind::SourceChanged));
        }
    }

    // 7. N no-clobber moves (or, in place, the one atomic swap).
    for i in 0..j.outputs.len() {
        fp(hook, Step::BeforeRename(i))?;
        place(&j.outputs[i], in_place).map_err(Abort::Fail)?;
        fp(hook, Step::AfterRename(i))?;
    }

    // ---- the commit point: the complete set exists; from here only forward.
    finish_forward(req.store, &j, hook)
}

/// Puts one verified temp at its final name. Create: never replaces. Replace: only the previous
/// output this group owns, and only while it is still unchanged.
fn place(o: &JOutput, in_place: bool) -> Result<(), ErrKind> {
    let (temp, fin) = (Path::new(&o.temp), Path::new(&o.final_path));
    match &o.replaces {
        None => link_no_clobber(temp, fin).map_err(|e| match e {
            LinkErr::Exists => ErrKind::PlanStale,
            LinkErr::Other(k) => k,
        }),
        Some(r) => {
            // In place, a missing target (an interrupted `ReplaceFileW`, error 1176) is finished
            // by a no-clobber move of the verified temp; a target holding anything else is
            // somebody's edit and is never replaced.
            if in_place && !exists(fin) {
                return move_no_clobber(temp, fin);
            }
            if file_hash(fin).as_deref() != Some(r.old_blake3.as_str()) {
                return Err(if in_place {
                    ErrKind::SourceChanged
                } else {
                    ErrKind::PlanStale
                });
            }
            swap(temp, fin)
        }
    }
}

/// Steps 8 to 10. Idempotent: recovery calls it on a half-finished group. Only a crash aborts
/// it; a failing step is recorded and the rest continues.
fn finish_forward(store: &Store, j: &Journal, hook: &dyn FaultHook) -> Flow<GroupSaved> {
    let mut notes: Vec<ErrKind> = Vec::new();
    fp_post(hook, Step::RenamesDone)?;
    if let Some(dir) = j
        .outputs
        .first()
        .and_then(|o| Path::new(&o.final_path).parent())
    {
        sync_dir(dir);
    }
    fp_post(hook, Step::DirSynced)?;

    // Surplus old outputs: kept in the store, then removed, only while unchanged.
    for (k, r) in j.retire.iter().enumerate() {
        let skip = fp_post(hook, Step::Retired(k))?;
        let p = Path::new(&r.path);
        if skip.is_none() && file_hash(p).as_deref() == Some(r.old_blake3.as_str()) {
            let kept = r.stored.is_empty()
                || file_hash(Path::new(&r.stored)).as_deref() == Some(r.old_blake3.as_str());
            if kept {
                let _ = remove_with_retry(p);
            }
        }
    }

    // The scan goes last, after one more look at it.
    let mut source_removed = false;
    if j.unlink_source {
        let src = Path::new(&j.source.path);
        let skip = fp_post(hook, Step::BeforeUnlink)?;
        if let Some(k) = skip {
            notes.push(if k == ErrKind::FileInUse {
                ErrKind::SavedSourceInUse
            } else {
                k
            });
        } else if !exists(src) {
            source_removed = true;
        } else if file_hash(src).as_deref() == Some(j.source.blake3.as_str()) {
            match remove_with_retry(src) {
                Ok(()) => source_removed = true,
                Err(ErrKind::FileInUse | ErrKind::ReadOnly | ErrKind::Unreadable) => {
                    notes.push(ErrKind::SavedSourceInUse);
                }
                Err(k) => notes.push(k),
            }
        } else {
            // Someone changed the scan since we read it: it is theirs now, leave it.
            notes.push(ErrKind::SourceChanged);
        }
        fp_post(hook, Step::AfterUnlink)?;
    }

    // Bookkeeping: the manifest says Saved with the outputs, then the journal goes.
    let outputs: Vec<OutputRec> = j
        .outputs
        .iter()
        .map(|o| OutputRec {
            path: o.final_path.clone(),
            blake3: o.blake3.clone(),
            size: o.size,
            mtime_ms: o.mtime_ms,
            // An in-place save is an ordinary one-to-one output.
            item_id: (!j.in_place).then_some(o.item_id),
            index: (!j.in_place).then_some(o.index),
        })
        .collect();
    let mut journal_pending = false;
    if let Some(bid) = &j.backup_id {
        let injected = fp_post(hook, Step::ManifestSaved)?;
        let wrote = match (injected, store.read(bid)) {
            (None, Some(mut m)) => {
                m.state = BackupState::Saved;
                m.kind = if j.in_place {
                    BackupKind::OneToOne
                } else {
                    BackupKind::OneToN
                };
                m.outputs = outputs.clone();
                if j.edit.is_some() {
                    m.edit = j.edit.clone();
                }
                store.write(&m).is_ok()
            }
            _ => false,
        };
        journal_pending = !wrote;
    }
    for o in &j.outputs {
        let _ = remove_with_retry(Path::new(&o.temp));
    }
    if !journal_pending {
        let _ = fs::remove_file(journal_path(store, &j.id));
        sync_dir(&store.groups_dir());
        fp_post(hook, Step::JournalRemoved)?;
    }
    Ok(GroupSaved {
        outputs,
        backup_id: j.backup_id.clone(),
        source_removed,
        notes,
        journal_pending,
    })
}

// ------------------------------------------------------------------ rollback and recovery

/// Undoes a group that did not reach the commit point: removes its temps and the outputs it
/// placed (only files whose bytes match the journal), gives a replaced previous output its old
/// bytes back, puts the scan back from the backup if it is missing, discards a backup this group
/// made if the scan is intact, and removes the journal. Safe to run twice.
fn rollback(store: &Store, j: &Journal) -> Result<(), ErrKind> {
    if j.in_place {
        return rollback_in_place(store, j);
    }
    let mut failed = false;
    for o in &j.outputs {
        let _ = remove_with_retry(Path::new(&o.temp));
        let fin = Path::new(&o.final_path);
        if o.blake3.is_empty() || file_hash(fin).as_deref() != Some(o.blake3.as_str()) {
            // Never placed, or not ours. A kept copy of the previous output is redundant while
            // the previous output is still in place.
            if let Some(r) = &o.replaces
                && !r.stored.is_empty()
                && file_hash(fin).as_deref() == Some(r.old_blake3.as_str())
            {
                let _ = fs::remove_file(&r.stored);
            }
            continue;
        }
        match &o.replaces {
            None => {
                if remove_with_retry(fin).is_err() {
                    failed = true;
                }
            }
            Some(r) => {
                let stored = Path::new(&r.stored);
                if r.stored.is_empty()
                    || file_hash(stored).as_deref() != Some(r.old_blake3.as_str())
                {
                    // Cannot put the old bytes back; the new output is valid, keep it.
                    failed = true;
                    continue;
                }
                let tmp = fin.with_file_name(format!(".autocrop-{}.tmp", new_id()));
                let restored = fs::copy(stored, &tmp)
                    .map_err(|e| ErrKind::from_io(&e))
                    .and_then(|_| swap(&tmp, fin));
                if restored.is_err() {
                    let _ = fs::remove_file(&tmp);
                    failed = true;
                } else {
                    let _ = fs::remove_file(stored);
                }
            }
        }
    }
    for r in &j.retire {
        // Retiring happens only after the commit point, so these files are still in place.
        if !r.stored.is_empty()
            && file_hash(Path::new(&r.path)).as_deref() == Some(r.old_blake3.as_str())
        {
            let _ = fs::remove_file(&r.stored);
        }
    }
    if let Some(dir) = j
        .backup_id
        .as_ref()
        .and_then(|id| store.entry_path(id))
        .map(|d| d.join("superseded"))
    {
        let _ = fs::remove_dir(&dir); // only if now empty
    }
    // The scan: back from the backup if it is gone.
    let src = Path::new(&j.source.path);
    // (Only a group that was going to remove the scan has any business putting it back: a
    // re-save runs after the scan was moved to the store, on purpose.)
    let mut source_ok =
        !j.unlink_source || file_hash(src).as_deref() == Some(j.source.blake3.as_str());
    if j.unlink_source && !exists(src) {
        if let Some(m) = j.backup_id.as_ref().and_then(|id| store.read(id))
            && let Some(orig) = store.original_path(&m)
            && file_hash(&orig).as_deref() == Some(j.source.blake3.as_str())
        {
            let tmp = src.with_file_name(format!(".autocrop-{}.tmp", new_id()));
            if fs::copy(&orig, &tmp).is_ok()
                && file_hash(&tmp).as_deref() == Some(j.source.blake3.as_str())
                && link_no_clobber(&tmp, src).is_ok()
            {
                source_ok = true;
            } else {
                let _ = fs::remove_file(&tmp);
            }
        }
        if !source_ok {
            failed = true;
        }
    }
    if failed {
        // Leave the journal so the next start tries again; nothing was lost.
        return Err(ErrKind::GroupCommitFailed);
    }
    if j.backup_created
        && source_ok
        && let Some(id) = &j.backup_id
    {
        store.remove_unused(id);
    }
    let _ = fs::remove_file(journal_path(store, &j.id));
    sync_dir(&store.groups_dir());
    Ok(())
}

/// Copies `from` to `to` (which must not exist), sets the mtime, makes it durable and checks the
/// copy against `hash`. Nothing is left at `to` on failure.
fn copy_verified(from: &Path, to: &Path, hash: &str, mtime_ms: i64) -> Result<(), ErrKind> {
    let copied = (|| -> std::io::Result<()> {
        fs::copy(from, to)?;
        let f = fs::OpenOptions::new().write(true).open(to)?;
        f.set_modified(SystemTime::UNIX_EPOCH + Duration::from_millis(mtime_ms.max(0) as u64))?;
        f.sync_all()
    })();
    let kind = match &copied {
        Err(e) => Some(ErrKind::from_io(e)),
        Ok(()) if file_hash(to).as_deref() != Some(hash) => Some(ErrKind::VerifyFailed),
        Ok(()) => None,
    };
    match kind {
        Some(k) => {
            let _ = fs::remove_file(to);
            Err(k)
        }
        None => Ok(()),
    }
}

/// The in-place twin of [`rollback`]. The file being replaced is also the scan, so "back" means:
/// the target holds the old bytes. If it already does, nothing but the temp, the unused backup and
/// the journal go. If it holds our new output, or is missing (an interrupted swap), the old bytes
/// come back from the backup (first save) or from `superseded/` (re-save) through a verified temp.
/// If it holds anything else, somebody edited it after we read it: it is theirs, it is left alone,
/// and the backup of what we read stays. If the old bytes cannot be put back the journal stays so
/// the next start tries again; nothing is lost either way.
fn rollback_in_place(store: &Store, j: &Journal) -> Result<(), ErrKind> {
    let Some(o) = j.outputs.first() else {
        let _ = fs::remove_file(journal_path(store, &j.id));
        return Ok(());
    };
    let _ = remove_with_retry(Path::new(&o.temp));
    let fin = Path::new(&o.final_path);
    let old = j.source.blake3.as_str();
    let replaces = o.replaces.as_ref();
    let old_copy: Option<PathBuf> = replaces
        .filter(|r| !r.stored.is_empty())
        .map(|r| PathBuf::from(&r.stored));
    let put_back = |via_swap: bool| -> bool {
        let Some(src) = &old_copy else { return false };
        if file_hash(src).as_deref() != Some(old) {
            return false;
        }
        let tmp = fin.with_file_name(format!(".autocrop-{}.tmp", new_id()));
        if copy_verified(src, &tmp, old, j.source.mtime_ms).is_err() {
            return false;
        }
        let r = if via_swap {
            swap(&tmp, fin)
        } else {
            move_no_clobber(&tmp, fin)
        };
        if r.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        r.is_ok()
    };
    let (mut intact, mut failed) = (false, false);
    match file_hash(fin).as_deref() {
        Some(h) if h == old => intact = true,
        Some(h) if !o.blake3.is_empty() && h == o.blake3 => {
            intact = put_back(true);
            failed = !intact;
        }
        // Somebody else's file: not ours to touch. The backup of what we read stays.
        Some(_) => {}
        None if exists(fin) => failed = true, // present but unreadable right now: try again later
        None => {
            intact = put_back(false);
            failed = !intact;
        }
    }
    if failed {
        return Err(ErrKind::GroupCommitFailed);
    }
    if intact {
        if let Some(r) = replaces
            && !r.stored_is_backup
            && !r.stored.is_empty()
        {
            let _ = fs::remove_file(&r.stored);
        }
        if let Some(dir) = j
            .backup_id
            .as_ref()
            .and_then(|id| store.entry_path(id))
            .map(|d| d.join("superseded"))
        {
            let _ = fs::remove_dir(&dir); // only if now empty
        }
        if j.backup_created
            && let Some(id) = &j.backup_id
        {
            store.remove_unused(id);
        }
    }
    let _ = fs::remove_file(journal_path(store, &j.id));
    sync_dir(&store.groups_dir());
    Ok(())
}

/// What start-up recovery did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecoveryReport {
    pub rolled_forward: Vec<String>,
    pub rolled_back: Vec<String>,
    /// Journals of a commit that is still running (this process or another): left alone.
    pub busy: Vec<String>,
    /// Journals that could not be read (left alone) or rolled back (kept for the next start).
    pub left: Vec<String>,
}

/// Finishes or undoes every group a crash interrupted (M10.24). `Writing` journals roll back.
/// `Committing` journals roll forward only if all N outputs verify against the journalled hashes
/// (as a placed file, or as an intact temp whose target is free or the previous output), else
/// they roll back to the scan and its backup, so a partial set is never left.
pub fn recover(store: &Store, hook: &dyn FaultHook) -> RecoveryReport {
    let mut report = RecoveryReport::default();
    let mut files: Vec<PathBuf> = fs::read_dir(store.groups_dir())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    for path in files {
        let Some(j) = read_journal(&path) else {
            report.left.push(path.to_string_lossy().into_owned());
            continue;
        };
        // A commit that is running right now is not ours to recover.
        let busy = match &j.owner {
            Some(o) if o.pid == std::process::id() => is_active_here(&j.id),
            Some(o) => process_started(o.pid) == Some(o.started),
            None => false,
        };
        if busy {
            report.busy.push(j.id.clone());
            continue;
        }
        let complete = j.phase == Phase::Committing
            && j.outputs.iter().all(|o| {
                let fin = Path::new(&o.final_path);
                let fin_h = file_hash(fin);
                if fin_h.as_deref() == Some(o.blake3.as_str()) {
                    return true;
                }
                let temp_ok = file_hash(Path::new(&o.temp)).as_deref() == Some(o.blake3.as_str());
                let target_ok = match &o.replaces {
                    None => !exists(fin),
                    // In place, a missing target is an interrupted swap (error 1176): the verified
                    // temp is still there and is moved in.
                    Some(r) => {
                        fin_h.as_deref() == Some(r.old_blake3.as_str())
                            || (j.in_place && !exists(fin))
                    }
                };
                temp_ok && target_ok
            });
        if complete {
            let placed = j.outputs.iter().try_for_each(|o| {
                if file_hash(Path::new(&o.final_path)).as_deref() == Some(o.blake3.as_str()) {
                    Ok(())
                } else {
                    place(o, j.in_place)
                }
            });
            if placed.is_ok() {
                match finish_forward(store, &j, hook) {
                    Ok(_) => report.rolled_forward.push(j.id.clone()),
                    Err(_) => report.left.push(j.id.clone()),
                }
                continue;
            }
        }
        match rollback(store, &j) {
            Ok(()) => report.rolled_back.push(j.id.clone()),
            Err(_) => report.left.push(j.id.clone()),
        }
    }
    report
}

/// Is `name` exactly a temp name this engine makes: `.autocrop-<28 hex digits>.tmp`?
fn is_own_temp_name(name: &str) -> bool {
    name.strip_prefix(".autocrop-")
        .and_then(|r| r.strip_suffix(".tmp"))
        .is_some_and(|id| id.len() == 28 && id.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Removes orphan temp files from `dir` (ROADMAP M2.83, PLAN 2.7 "stray `.autocrop-*.tmp` files
/// are swept"): files named exactly like ours (`.autocrop-<id>.tmp`) that no journal names (a
/// journalled temp is recovery's to deal with) and that are older than `min_age`. The age comes
/// from the id itself (its first 12 hex digits are the creation time in milliseconds), so it
/// survives a copy of the folder and needs no file times. Anything else, including a file merely
/// called `.autocrop-something.tmp`, is left alone. Returns what was removed.
pub fn sweep_orphans(store: &Store, dir: &Path, min_age: Duration) -> Vec<PathBuf> {
    let named: std::collections::HashSet<PathBuf> = fs::read_dir(store.groups_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| read_journal(&e.path()))
        .flat_map(|j| j.outputs.into_iter().map(|o| PathBuf::from(o.temp)))
        .collect();
    let now_ms = unix_ms(SystemTime::now());
    let mut removed = Vec::new();
    for e in fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !is_own_temp_name(&name) || named.contains(&e.path()) {
            continue;
        }
        let created = i64::from_str_radix(&name[".autocrop-".len()..".autocrop-".len() + 12], 16)
            .unwrap_or(i64::MAX);
        let old_enough = now_ms.saturating_sub(created) >= min_age.as_millis() as i64;
        let is_file = e.file_type().is_ok_and(|t| t.is_file());
        if old_enough && is_file && remove_with_retry(&e.path()).is_ok() {
            removed.push(e.path());
        }
    }
    removed
}

/// Journals still on disk (tests and diagnostics).
pub fn pending_journals(store: &Store) -> usize {
    fs::read_dir(store.groups_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .count()
}

/// Stray `.autocrop-*.tmp` files in `dir` (tests and diagnostics).
pub fn stray_temps(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with(".autocrop-"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_named(dir: &Path, age_ms: i64) -> PathBuf {
        let id = format!("{:012x}{:016x}", unix_ms(SystemTime::now()) - age_ms, 7u64);
        let p = dir.join(format!(".autocrop-{id}.tmp"));
        fs::write(&p, b"orphan").unwrap();
        p
    }

    #[test]
    fn only_our_own_old_unjournalled_temps_are_swept() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::new(root.path().join("backups"));
        let dir = root.path().join("photos");
        fs::create_dir_all(&dir).unwrap();
        let old = temp_named(&dir, 3_600_000);
        let fresh = temp_named(&dir, 1_000);
        let stranger = dir.join(".autocrop-notours.tmp");
        let wrong_ext = dir.join(format!(".autocrop-{:028x}.png", 1));
        let photo = dir.join("a.jpg");
        for p in [&stranger, &wrong_ext, &photo] {
            fs::write(p, b"x").unwrap();
        }
        let gone = sweep_orphans(&store, &dir, Duration::from_secs(300));
        assert_eq!(gone, std::slice::from_ref(&old));
        assert!(
            !old.exists()
                && fresh.exists()
                && stranger.exists()
                && wrong_ext.exists()
                && photo.exists()
        );
        // A temp that a journal names is recovery's, not the sweep's.
        let journalled = temp_named(&dir, 3_600_000);
        fs::create_dir_all(store.groups_dir()).unwrap();
        let j = Journal {
            schema: JOURNAL_SCHEMA,
            id: "j".repeat(28),
            owner: None,
            phase: Phase::Writing,
            created_at: 0,
            source: SourceRec {
                path: photo.to_string_lossy().into_owned(),
                size: 1,
                mtime_ms: 0,
                blake3: String::new(),
            },
            unlink_source: false,
            backup_id: None,
            backup_created: false,
            outputs: vec![JOutput {
                item_id: 0,
                index: 1,
                temp: journalled.to_string_lossy().into_owned(),
                final_path: photo.to_string_lossy().into_owned(),
                blake3: String::new(),
                size: 0,
                mtime_ms: 0,
                replaces: None,
            }],
            retire: Vec::new(),
            edit: None,
            in_place: false,
        };
        write_journal(&store, &j).unwrap();
        assert!(sweep_orphans(&store, &dir, Duration::from_secs(300)).is_empty());
        assert!(journalled.exists());
    }
}
