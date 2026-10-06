// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Fault injection for the one-file in-place save (ROADMAP M2.58, M2.83; PLAN 2.7): the same
//! protocol as the group commit, driven the same way. Every step is failed and "crashed" in turn
//! (and the process is really killed in `killing_the_process_at_every_step_...`), recovery runs, and
//! the file must hold the old bytes or the verified output, never anything else; the old bytes must
//! always be somewhere (in place, or in the verified backup); nothing may be left over.

use auto_crop_codecs::{Format, encode};
use auto_crop_core::EditState;
use auto_crop_engine::ErrKind;
use auto_crop_engine::commit::{Expect, VerifyMode};
use auto_crop_engine::group::{
    BackupPlan, Fault, FaultHook, GroupError, NoFaults, Output, Produced, SingleRequest,
    SingleSaved, SourceFingerprint, Step, commit_single, pending_journals, recover, stray_temps,
};
use auto_crop_engine::store::{BackupKind, BackupState, NewBackup, Store};
use auto_crop_engine::util::{blake3_hex, unix_ms};
use auto_crop_imgproc::Raster;
use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

fn png(w: u32, h: u32, shade: u8) -> Vec<u8> {
    encode(
        &Raster::filled(w, h, [shade, 90, 200 - shade]),
        Format::Png,
        90,
        None,
    )
    .unwrap()
}

fn mtime() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Which {
    Old,
    New,
}

struct Fx {
    _root: tempfile::TempDir,
    dir: PathBuf,
    store: Store,
    target: PathBuf,
    /// The very first bytes of the file: what the backup must always hold.
    original: Vec<u8>,
    /// What the target holds when this commit starts (the original, or the previous output).
    old: Vec<u8>,
    /// What this commit writes.
    new: Vec<u8>,
    backup_id: Option<String>,
}

impl Fx {
    fn first() -> Self {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("photos");
        fs::create_dir_all(&dir).unwrap();
        let target = dir.join("scan.png");
        let original = png(40, 30, 7);
        fs::write(&target, &original).unwrap();
        fs::File::options()
            .write(true)
            .open(&target)
            .unwrap()
            .set_modified(mtime())
            .unwrap();
        Fx {
            store: Store::new(root.path().join("backups")),
            _root: root,
            dir,
            target,
            old: original.clone(),
            original,
            new: png(24, 18, 90),
            backup_id: None,
        }
    }

    /// A completed first save, then a second edit that replaces its output.
    fn re_save() -> Self {
        let mut fx = Fx::first();
        let saved = fx.commit(&NoFaults).unwrap();
        fx.backup_id = Some(saved.backup_id);
        fx.old = fx.new.clone();
        fx.new = png(20, 12, 150);
        fx
    }

    fn fingerprint(&self) -> SourceFingerprint {
        let m = fs::metadata(&self.target).unwrap();
        SourceFingerprint {
            path: self.target.clone(),
            size: m.len(),
            mtime_ms: unix_ms(m.modified().unwrap()),
            blake3: blake3_hex(&fs::read(&self.target).unwrap()),
        }
    }

    fn commit(&self, hook: &dyn FaultHook) -> Result<SingleSaved, GroupError> {
        let original_hash = blake3_hex(&self.original);
        let backup = match &self.backup_id {
            Some(id) => BackupPlan::Existing(id.clone()),
            None => BackupPlan::New(NewBackup {
                source: &self.target,
                source_blake3: &original_hash,
                source_size: self.original.len() as u64,
                source_mtime_ms: unix_ms(mtime()),
                format_ext: "png",
                run_id: "run",
                run_name: "Run",
                retention_days: Some(30),
                edit: Some(EditState::default()),
            }),
        };
        let req = SingleRequest {
            store: &self.store,
            target: self.fingerprint(),
            backup,
            mtime: mtime(),
            edit: Some(EditState::default()),
            verify: VerifyMode::Full,
        };
        let new = self.new.clone();
        let mut produce = || -> Result<Output, ErrKind> {
            let d = auto_crop_codecs::probe(&new).unwrap();
            Ok(Output {
                produced: Produced {
                    bytes: new.clone(),
                    dims: (d.width, d.height),
                    format: Format::Png,
                },
                expect: Some(Expect::basic((d.width, d.height), Format::Png)),
            })
        };
        commit_single(&req, &mut produce, hook)
    }

    fn recorded_steps(&self) -> Vec<Step> {
        let seen = RefCell::new(Vec::new());
        self.commit(&|s: &Step| {
            seen.borrow_mut().push(*s);
            Fault::Pass
        })
        .unwrap();
        seen.into_inner()
    }

    fn on_disk(&self) -> Option<Vec<u8>> {
        fs::read(&self.target).ok()
    }

    /// The invariant, after recovery.
    fn check(&self, ctx: &str) -> Which {
        let cur = self.on_disk();
        let which = match cur.as_deref() {
            Some(b) if b == self.new => Which::New,
            Some(b) if b == self.old => Which::Old,
            other => panic!(
                "{ctx}: the file holds neither the old bytes nor the verified output ({:?} bytes)",
                other.map(<[u8]>::len)
            ),
        };
        assert!(
            stray_temps(&self.dir).is_empty(),
            "{ctx}: stray temps {:?}",
            stray_temps(&self.dir)
        );
        assert_eq!(pending_journals(&self.store), 0, "{ctx}: a journal is left");
        let manifests = self.store.list();
        let in_backup = manifests.iter().any(|m| {
            self.store
                .original_path(m)
                .and_then(|p| fs::read(p).ok())
                .is_some_and(|b| b == self.original)
        });
        // The very first bytes are in place or in a verified backup, always.
        assert!(
            (self.old == self.original && which == Which::Old) || in_backup,
            "{ctx}: the original is nowhere"
        );
        match (self.backup_id.is_some(), which) {
            // First save, rolled back: nothing is left in the store.
            (false, Which::Old) => {
                assert!(manifests.is_empty(), "{ctx}: an unused backup is left");
            }
            // First save, complete: one Saved one-to-one backup that lists the output.
            (false, Which::New) => {
                assert_eq!(manifests.len(), 1, "{ctx}");
                let m = &manifests[0];
                assert_eq!(
                    (m.state, m.kind),
                    (BackupState::Saved, BackupKind::OneToOne)
                );
                assert_eq!(m.outputs.len(), 1, "{ctx}");
                assert_eq!(m.outputs[0].blake3, blake3_hex(&self.new), "{ctx}");
                assert!(m.outputs[0].item_id.is_none(), "{ctx}");
                assert!(in_backup, "{ctx}");
            }
            // Re-save: the one backup stays, lists what is on disk, and keeps nothing extra after
            // a rollback.
            (true, w) => {
                assert_eq!(manifests.len(), 1, "{ctx}");
                let m = &manifests[0];
                assert_eq!(m.state, BackupState::Saved, "{ctx}");
                let want = if w == Which::New {
                    &self.new
                } else {
                    &self.old
                };
                assert_eq!(m.outputs[0].blake3, blake3_hex(want), "{ctx}");
                assert!(in_backup, "{ctx}");
                if w == Which::Old {
                    let kept = self.store.entry_path(&m.id).unwrap().join("superseded");
                    let n = fs::read_dir(&kept).map(|r| r.count()).unwrap_or(0);
                    assert_eq!(n, 0, "{ctx}: kept copies left behind");
                }
            }
        }
        // The mtime of whatever is there is the original's.
        assert_eq!(
            fs::metadata(&self.target).unwrap().modified().unwrap(),
            mtime(),
            "{ctx}"
        );
        which
    }
}

type Scenario = (&'static str, Box<dyn Fn() -> Fx>);

fn scenarios() -> Vec<Scenario> {
    vec![
        ("first save", Box::new(Fx::first)),
        ("re-save", Box::new(Fx::re_save)),
    ]
}

#[test]
fn a_clean_commit_replaces_the_file_after_a_verified_backup() {
    let fx = Fx::first();
    let saved = fx.commit(&NoFaults).unwrap();
    assert!(!saved.journal_pending);
    assert_eq!(saved.output.blake3, blake3_hex(&fx.new));
    assert_eq!(fx.check("clean"), Which::New);
    let m = &fx.store.list()[0];
    assert_eq!(
        fs::read(fx.store.original_path(m).unwrap()).unwrap(),
        fx.original
    );
    assert_eq!(m.id, saved.backup_id);
    assert!(auto_crop_codecs::decode(&fx.on_disk().unwrap()).is_ok());
}

#[test]
fn the_swap_comes_after_the_backup_and_the_committing_journal_never_before() {
    // At every step before the swap the file still holds the old bytes; at every step after it,
    // the new ones. The backup exists before the first temp is written.
    let fx = Fx::first();
    let (target, original, new) = (fx.target.clone(), fx.original.clone(), fx.new.clone());
    let store = Store::new(fx.store.dir().to_path_buf());
    let log: RefCell<Vec<(Step, bool, bool)>> = RefCell::new(Vec::new());
    fx.commit(&|s: &Step| {
        let backup_ready = store.list().iter().any(|m| {
            store
                .original_path(m)
                .and_then(|p| fs::read(p).ok())
                .is_some_and(|b| b == original)
        });
        let placed = fs::read(&target).ok().as_deref() == Some(&new[..]);
        log.borrow_mut().push((*s, backup_ready, placed));
        Fault::Pass
    })
    .unwrap();
    let mut swapped = false;
    for (s, backup_ready, placed) in log.borrow().iter() {
        if *placed {
            swapped = true;
        }
        assert!(!swapped || *placed, "the old bytes came back at {s:?}");
        if matches!(
            s,
            Step::TempWritten(_) | Step::JournalCommitting | Step::BeforeRename(_)
        ) {
            assert!(*backup_ready, "no verified backup at {s:?}");
        }
        if matches!(s, Step::BeforeRename(_)) {
            assert!(!*placed, "swapped before {s:?}");
        }
        if matches!(s, Step::AfterRename(_)) {
            assert!(*placed, "not swapped at {s:?}");
        }
    }
    assert!(swapped);
}

#[test]
fn a_failure_at_every_step_leaves_the_old_bytes_or_the_verified_output() {
    for (label, make) in scenarios() {
        let steps = make().recorded_steps();
        assert!(steps.len() >= 8, "{label}: {steps:?}");
        for step in &steps {
            let fx = make();
            let target = *step;
            let r = fx.commit(&move |s: &Step| {
                if *s == target {
                    Fault::Fail(ErrKind::DiskFull)
                } else {
                    Fault::Pass
                }
            });
            let ctx = format!("{label}: fail at {step:?}");
            if step.after_commit() {
                let saved = r.unwrap_or_else(|e| panic!("{ctx}: {e:?}"));
                if saved.journal_pending {
                    recover(&fx.store, &NoFaults);
                }
                assert_eq!(fx.check(&ctx), Which::New, "{ctx}");
            } else {
                assert_eq!(
                    r.unwrap_err(),
                    GroupError::Failed(ErrKind::DiskFull),
                    "{ctx}"
                );
                assert_eq!(fx.check(&ctx), Which::Old, "{ctx}");
            }
        }
    }
}

#[test]
fn a_crash_at_every_step_is_recovered_to_the_old_bytes_or_the_verified_output() {
    for (label, make) in scenarios() {
        let steps = make().recorded_steps();
        for step in &steps {
            let fx = make();
            let target = *step;
            let r = fx.commit(&move |s: &Step| {
                if *s == target {
                    Fault::Crash
                } else {
                    Fault::Pass
                }
            });
            let ctx = format!("{label}: crash at {step:?}");
            assert_eq!(r.unwrap_err(), GroupError::Crashed, "{ctx}");
            let report = recover(&fx.store, &NoFaults);
            let which = fx.check(&ctx);
            // Idempotent.
            assert_eq!(
                recover(&fx.store, &NoFaults),
                Default::default(),
                "{ctx}: a second recovery found work"
            );
            assert_eq!(fx.check(&format!("{ctx} (again)")), which);
            // Back until `Committing` is journalled, forward from then on.
            let committed = matches!(
                step,
                Step::JournalCommitting
                    | Step::SourceRestat
                    | Step::BeforeRename(_)
                    | Step::AfterRename(_)
            ) || step.after_commit();
            let expect = if committed { Which::New } else { Which::Old };
            assert_eq!(which, expect, "{ctx}: {report:?}");
        }
    }
}

#[test]
fn a_crash_inside_recovery_is_itself_recovered() {
    for (label, make) in scenarios() {
        let crash_after_swap = |s: &Step| {
            if *s == Step::AfterRename(0) {
                Fault::Crash
            } else {
                Fault::Pass
            }
        };
        let recovery_steps = {
            let fx = make();
            let _ = fx.commit(&crash_after_swap);
            let seen = RefCell::new(Vec::new());
            recover(&fx.store, &|s: &Step| {
                seen.borrow_mut().push(*s);
                Fault::Pass
            });
            seen.into_inner()
        };
        assert!(!recovery_steps.is_empty(), "{label}");
        for step in recovery_steps {
            let fx = make();
            let _ = fx.commit(&crash_after_swap);
            recover(&fx.store, &move |s: &Step| {
                if *s == step {
                    Fault::Crash
                } else {
                    Fault::Pass
                }
            });
            recover(&fx.store, &NoFaults);
            assert_eq!(
                fx.check(&format!("{label}: recovery crashed at {step:?}")),
                Which::New
            );
        }
    }
}

/// Error 1176: the old file was moved away, the replacement did not arrive. The target is missing
/// and the verified temp is on disk.
#[test]
fn an_interrupted_swap_with_the_target_missing_is_finished_from_the_verified_temp() {
    for (label, make) in scenarios() {
        let fx = make();
        let target = fx.target.clone();
        let r = fx.commit(&|s: &Step| {
            if *s == Step::BeforeRename(0) {
                fs::remove_file(&target).unwrap();
                Fault::Crash
            } else {
                Fault::Pass
            }
        });
        assert_eq!(r.unwrap_err(), GroupError::Crashed);
        assert!(!fx.target.exists());
        let report = recover(&fx.store, &NoFaults);
        assert_eq!(report.rolled_forward.len(), 1, "{label}: {report:?}");
        assert_eq!(fx.check(label), Which::New);
    }
}

/// The target is missing AND the temp is gone: the only copy of the old bytes is the backup (or
/// `superseded/`). Recovery puts them back; it never leaves the folder without the file.
#[test]
fn a_missing_target_and_a_missing_temp_are_restored_from_the_backup() {
    for (label, make) in scenarios() {
        let fx = make();
        let (target, dir) = (fx.target.clone(), fx.dir.clone());
        let r = fx.commit(&|s: &Step| {
            if *s == Step::BeforeRename(0) {
                fs::remove_file(&target).unwrap();
                for t in stray_temps(&dir) {
                    fs::remove_file(t).unwrap();
                }
                Fault::Crash
            } else {
                Fault::Pass
            }
        });
        assert_eq!(r.unwrap_err(), GroupError::Crashed);
        let report = recover(&fx.store, &NoFaults);
        assert_eq!(report.rolled_back.len(), 1, "{label}: {report:?}");
        assert_eq!(fx.check(label), Which::Old);
    }
}

/// An edit that lands after Auto Crop read the file but before the swap is never overwritten.
#[test]
fn an_edit_between_the_read_and_the_swap_is_never_overwritten() {
    for (label, make) in scenarios() {
        for (when, keep_stat) in [
            (Step::SourceRestat, false), // size and mtime change: the re-stat sees it
            (Step::BeforeRename(0), true), // same size and mtime: only the re-hash sees it
        ] {
            let fx = make();
            let target = fx.target.clone();
            let theirs: Vec<u8> = (0..fx.old.len()).map(|i| (i % 251) as u8).collect();
            let theirs2 = theirs.clone();
            let r = fx.commit(&|s: &Step| {
                if *s == when {
                    fs::write(&target, &theirs2).unwrap();
                    if keep_stat {
                        fs::File::options()
                            .write(true)
                            .open(&target)
                            .unwrap()
                            .set_modified(mtime())
                            .unwrap();
                    }
                }
                Fault::Pass
            });
            assert_eq!(
                r.unwrap_err(),
                GroupError::Failed(ErrKind::SourceChanged),
                "{label} {when:?}"
            );
            assert_eq!(fs::read(&fx.target).unwrap(), theirs, "{label} {when:?}");
            assert!(stray_temps(&fx.dir).is_empty(), "{label} {when:?}");
            assert_eq!(pending_journals(&fx.store), 0, "{label} {when:?}");
            // PLAN 2.7: the backup of what was read is kept.
            let in_backup = fx.store.list().iter().any(|m| {
                fx.store
                    .original_path(m)
                    .and_then(|p| fs::read(p).ok())
                    .is_some_and(|b| b == fx.original)
            });
            assert!(in_backup, "{label} {when:?}: the backup was discarded");
        }
    }
}

/// The user edits the file while the process is down: recovery leaves their file alone, keeps the
/// backup of what we read, and leaves nothing of ours behind.
#[test]
fn a_file_edited_while_the_process_was_down_is_left_alone_by_recovery() {
    for (label, make) in scenarios() {
        let fx = make();
        let r = fx.commit(&|s: &Step| {
            if *s == Step::JournalCommitting {
                Fault::Crash
            } else {
                Fault::Pass
            }
        });
        assert_eq!(r.unwrap_err(), GroupError::Crashed);
        fs::write(&fx.target, b"edited while Auto Crop was not running").unwrap();
        let report = recover(&fx.store, &NoFaults);
        assert_eq!(report.rolled_back.len(), 1, "{label}: {report:?}");
        assert_eq!(
            fs::read(&fx.target).unwrap(),
            b"edited while Auto Crop was not running"
        );
        assert!(stray_temps(&fx.dir).is_empty() && pending_journals(&fx.store) == 0);
        let in_backup = fx.store.list().iter().any(|m| {
            fx.store
                .original_path(m)
                .and_then(|p| fs::read(p).ok())
                .is_some_and(|b| b == fx.original)
        });
        assert!(
            in_backup,
            "{label}: the backup of the original was discarded"
        );
    }
}

#[test]
fn a_corrupt_temp_fails_verification_and_the_file_is_untouched() {
    for (label, make) in scenarios() {
        let fx = make();
        let (dir, new) = (fx.dir.clone(), fx.new.clone());
        let r = fx.commit(&|s: &Step| {
            if *s == Step::TempWritten(0) {
                let t = stray_temps(&dir)
                    .into_iter()
                    .find(|p| fs::read(p).unwrap() == new)
                    .expect("the temp");
                fs::write(&t, &new[..new.len() / 2]).unwrap();
            }
            Fault::Pass
        });
        assert_eq!(
            r.unwrap_err(),
            GroupError::Failed(ErrKind::VerifyFailed),
            "{label}"
        );
        assert_eq!(fx.check(label), Which::Old);
    }
}

#[test]
fn a_flipped_bit_in_the_temp_is_caught_wherever_it_is() {
    let probe = Fx::first();
    let len = probe.new.len();
    // Every 13th byte position: the hash comparison must refuse each.
    for pos in (0..len).step_by(13) {
        let fx = Fx::first();
        let (dir, new) = (fx.dir.clone(), fx.new.clone());
        let r = fx.commit(&|s: &Step| {
            if *s == Step::TempWritten(0) {
                let t = stray_temps(&dir)
                    .into_iter()
                    .find(|p| fs::read(p).unwrap() == new)
                    .expect("the temp");
                let mut bad = new.clone();
                bad[pos] ^= 0x01;
                fs::write(&t, bad).unwrap();
            }
            Fault::Pass
        });
        assert_eq!(
            r.unwrap_err(),
            GroupError::Failed(ErrKind::VerifyFailed),
            "flip at {pos}"
        );
        assert_eq!(fx.check(&format!("flip at {pos}")), Which::Old);
    }
}

#[test]
fn a_failing_encoder_changes_nothing() {
    let fx = Fx::first();
    let req = SingleRequest {
        store: &fx.store,
        target: fx.fingerprint(),
        backup: BackupPlan::New(NewBackup {
            source: &fx.target,
            source_blake3: &blake3_hex(&fx.original),
            source_size: fx.original.len() as u64,
            source_mtime_ms: unix_ms(mtime()),
            format_ext: "png",
            run_id: "r",
            run_name: "r",
            retention_days: None,
            edit: None,
        }),
        mtime: mtime(),
        edit: None,
        verify: VerifyMode::Full,
    };
    let mut produce = || -> Result<Output, ErrKind> { Err(ErrKind::EncodeFailed) };
    let r = commit_single(&req, &mut produce, &NoFaults);
    assert_eq!(r.unwrap_err(), GroupError::Failed(ErrKind::EncodeFailed));
    assert_eq!(fx.check("encoder failed"), Which::Old);
}

#[test]
fn recovery_leaves_the_journal_of_a_commit_in_this_process_alone() {
    let fx = Fx::first();
    let store = Store::new(fx.store.dir().to_path_buf());
    let during = RefCell::new(None);
    let saved = fx
        .commit(&|s: &Step| {
            if *s == Step::BeforeRename(0) {
                *during.borrow_mut() = Some(recover(&store, &NoFaults));
            }
            Fault::Pass
        })
        .unwrap();
    assert_eq!(saved.output.blake3, blake3_hex(&fx.new));
    let report = during.into_inner().expect("the hook ran");
    assert_eq!(report.busy.len(), 1, "{report:?}");
    assert_eq!(fx.check("a running commit"), Which::New);
}

#[test]
fn a_read_only_look_at_other_files_in_the_folder_never_touches_them() {
    // Recovery and rollback delete only what a journal names: unrelated `.autocrop-*.tmp` files
    // and files with other names stay.
    let fx = Fx::first();
    let stranger = fx.dir.join(".autocrop-notours.tmp");
    let other = fx.dir.join("other.png");
    fs::write(&stranger, b"x").unwrap();
    fs::write(&other, png(5, 5, 1)).unwrap();
    let _ = fx.commit(&|s: &Step| {
        if *s == Step::TempVerified(0) {
            Fault::Crash
        } else {
            Fault::Pass
        }
    });
    recover(&fx.store, &NoFaults);
    assert!(stranger.exists() && other.exists());
    assert_eq!(fx.on_disk().unwrap(), fx.old);
}

// ---------------------------------------------------------------------------------------------
// Real process death
// ---------------------------------------------------------------------------------------------

/// The child: runs the commit and exits (no unwinding, no destructors) at the step named by the
/// environment. This test binary run on `child_single`.
#[test]
fn child_single() {
    let (Ok(root), Ok(step), Ok(kind)) = (
        std::env::var("AC_SINGLE_ROOT"),
        std::env::var("AC_SINGLE_STEP"),
        std::env::var("AC_SINGLE_KIND"),
    ) else {
        return; // an ordinary run of the suite
    };
    let root = PathBuf::from(root);
    let fx = child_fixture(&root, &kind);
    let hook = |s: &Step| {
        if format!("{s:?}") == step {
            std::process::exit(86);
        }
        Fault::Pass
    };
    let _ = fx.commit(&hook);
}

/// Rebuilds the fixture the parent made, on the same root (the files are already there).
fn child_fixture(root: &Path, kind: &str) -> Fx {
    let dir = root.join("photos");
    let target = dir.join("scan.png");
    let original = png(40, 30, 7);
    let (old, new, backup_id) = if kind == "first" {
        (original.clone(), png(24, 18, 90), None)
    } else {
        let id = fs::read_to_string(root.join("backup_id")).unwrap();
        (png(24, 18, 90), png(20, 12, 150), Some(id))
    };
    Fx {
        // The parent owns the directory; the child's handle must not delete it.
        _root: tempfile::tempdir().unwrap(),
        dir,
        store: Store::new(root.join("backups")),
        target,
        original,
        old,
        new,
        backup_id,
    }
}

#[test]
fn killing_the_process_at_every_step_never_loses_the_original_or_leaves_a_partial_file() {
    let exe = std::env::current_exe().unwrap();
    for (label, make) in scenarios() {
        let kind = if label == "first save" { "first" } else { "re" };
        let steps: Vec<String> = make()
            .recorded_steps()
            .iter()
            .map(|s| format!("{s:?}"))
            .collect();
        for step in steps {
            let fx = make();
            let root = fx._root.path().to_path_buf();
            if let Some(id) = &fx.backup_id {
                fs::write(root.join("backup_id"), id).unwrap();
            }
            let out = std::process::Command::new(&exe)
                .args(["--exact", "child_single", "--nocapture", "--test-threads=1"])
                .env("AC_SINGLE_ROOT", &root)
                .env("AC_SINGLE_STEP", &step)
                .env("AC_SINGLE_KIND", kind)
                .output()
                .unwrap();
            assert_eq!(
                out.status.code(),
                Some(86),
                "{label}: the child should have died at {step}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            recover(&fx.store, &NoFaults);
            fx.check(&format!("{label}: killed at {step}"));
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Random faults in a row (the in-process stand-in for the kill loop of tests/kill_loop.rs)
// ---------------------------------------------------------------------------------------------

#[test]
fn random_faults_across_many_runs_never_corrupt_the_file() {
    let scs = scenarios();
    let steps: Vec<Vec<Step>> = scs.iter().map(|(_, m)| m().recorded_steps()).collect();
    let mut rng = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    let mut retried = 0;
    for run in 0..120 {
        let k = (next() % scs.len() as u64) as usize;
        let (label, make) = &scs[k];
        let step = steps[k][(next() % steps[k].len() as u64) as usize];
        let crash = next() % 2 == 0;
        let fault = if crash {
            Fault::Crash
        } else if next() % 2 == 0 {
            Fault::Fail(ErrKind::DiskFull)
        } else {
            Fault::Fail(ErrKind::FileInUse)
        };
        let fx = make();
        let r = fx.commit(&move |s: &Step| if *s == step { fault } else { Fault::Pass });
        let ctx = format!("run {run}: {label}: {fault:?} at {step:?}");
        if crash {
            assert_eq!(r.unwrap_err(), GroupError::Crashed, "{ctx}");
        }
        recover(&fx.store, &NoFaults);
        if fx.check(&ctx) == Which::Old {
            // Nothing lost, nothing stuck: the same save now goes through.
            fx.commit(&NoFaults)
                .unwrap_or_else(|e| panic!("{ctx}: retry {e:?}"));
            assert_eq!(fx.check(&format!("{ctx} (retry)")), Which::New);
            retried += 1;
        }
    }
    assert!(
        retried > 20,
        "only {retried} runs were rolled back and retried"
    );
}
