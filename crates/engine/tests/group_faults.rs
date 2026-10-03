// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Fault injection for the 1-to-N group commit and its recovery (ROADMAP M10.64, M10.23,
//! M10.24). Every protocol step is failed and "crashed" in turn, and after recovery the folder
//! must hold either the scan with no outputs, or the complete verified set, never a partial one;
//! the scan's bytes must always be somewhere (in place, or in a verified backup); and nothing may
//! be left over (no temp, no journal, no unused backup).

use auto_crop_codecs::{Format, encode};
use auto_crop_core::EditState;
use auto_crop_engine::ErrKind;
use auto_crop_engine::group::{
    BackupPlan, Fault, FaultHook, GroupError, GroupRequest, GroupSaved, NoFaults, OutSpec,
    Produced, Retire, SourceFingerprint, Step, commit_group, pending_journals, recover,
    stray_temps,
};
use auto_crop_engine::store::{BackupKind, BackupState, NewBackup, Store};
use auto_crop_engine::util::{blake3_hex, unix_ms};
use auto_crop_imgproc::Raster;
use std::cell::RefCell;
use std::collections::BTreeMap;
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
enum Mode {
    /// Replace: back the scan up, place the outputs, remove the scan.
    Replace,
    /// Copy: no backup, the scan stays.
    Copy,
}

struct Fx {
    _root: tempfile::TempDir,
    dir: PathBuf,
    store: Store,
    src: PathBuf,
    src_bytes: Vec<u8>,
    mode: Mode,
    /// The outputs of the commit under test: name and bytes.
    new_set: BTreeMap<String, Vec<u8>>,
    /// The outputs of an earlier save that this commit replaces (empty for a first save).
    old_set: BTreeMap<String, Vec<u8>>,
    backup_id: Option<String>,
    shades: Vec<u8>,
}

fn outs_dir(dir: &Path, mode: Mode) -> PathBuf {
    match mode {
        Mode::Replace => dir.to_path_buf(),
        Mode::Copy => dir.join("AutoCrop"),
    }
}

fn name(n: usize) -> String {
    format!("scan_{n:02}.png")
}

impl Fx {
    fn new(mode: Mode, n: usize) -> Self {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("photos");
        fs::create_dir_all(&dir).unwrap();
        let store = Store::new(root.path().join("backups"));
        let src = dir.join("scan.png");
        let src_bytes = png(40, 30, 7);
        fs::write(&src, &src_bytes).unwrap();
        // The scan's mtime is fixed so the re-stat is deterministic.
        fs::File::options()
            .write(true)
            .open(&src)
            .unwrap()
            .set_modified(mtime())
            .unwrap();
        let shades: Vec<u8> = (0..16).map(|i| 20 + i * 10).collect();
        let mut fx = Fx {
            _root: root,
            dir,
            store,
            src,
            src_bytes,
            mode,
            new_set: BTreeMap::new(),
            old_set: BTreeMap::new(),
            backup_id: None,
            shades,
        };
        fx.new_set = fx.set_of(n, 0);
        fx
    }

    /// The set of `n` outputs generated with `variant` (a different variant is a different edit).
    fn set_of(&self, n: usize, variant: u8) -> BTreeMap<String, Vec<u8>> {
        (1..=n)
            .map(|i| {
                (
                    name(i),
                    png(10 + i as u32, 8 + i as u32, self.shades[i] + variant),
                )
            })
            .collect()
    }

    fn fingerprint(&self) -> SourceFingerprint {
        let m = fs::metadata(&self.src).unwrap();
        SourceFingerprint {
            path: self.src.clone(),
            size: m.len(),
            mtime_ms: unix_ms(m.modified().unwrap()),
            blake3: blake3_hex(&fs::read(&self.src).unwrap_or_default()),
        }
    }

    /// Runs one commit of `self.new_set` (replacing `self.old_set` where it overlaps).
    fn commit(&self, hook: &dyn FaultHook) -> Result<GroupSaved, GroupError> {
        let od = outs_dir(&self.dir, self.mode);
        let src_fp = if self.src.exists() {
            self.fingerprint()
        } else {
            SourceFingerprint {
                path: self.src.clone(),
                size: self.src_bytes.len() as u64,
                mtime_ms: unix_ms(mtime()),
                blake3: blake3_hex(&self.src_bytes),
            }
        };
        let hash = blake3_hex(&self.src_bytes);
        let names: Vec<String> = self.new_set.keys().cloned().collect();
        let outputs: Vec<OutSpec> = names
            .iter()
            .enumerate()
            .map(|(i, n)| OutSpec {
                item_id: 10 + i as u32,
                index: i as u32 + 1,
                final_path: od.join(n),
                replaces_blake3: self.old_set.get(n).map(|b| blake3_hex(b)),
            })
            .collect();
        let retire: Vec<Retire> = self
            .old_set
            .iter()
            .filter(|(n, _)| !self.new_set.contains_key(*n))
            .map(|(n, b)| Retire {
                path: od.join(n),
                expected_blake3: blake3_hex(b),
            })
            .collect();
        let backup = match (&self.backup_id, self.mode) {
            (Some(id), _) => BackupPlan::Existing(id.clone()),
            (None, Mode::Replace) => BackupPlan::New(NewBackup {
                source: &self.src,
                source_blake3: &hash,
                source_size: self.src_bytes.len() as u64,
                source_mtime_ms: unix_ms(mtime()),
                format_ext: "png",
                run_id: "run",
                run_name: "Run",
                retention_days: Some(30),
                edit: Some(EditState::default()),
            }),
            (None, Mode::Copy) => BackupPlan::None,
        };
        let req = GroupRequest {
            store: &self.store,
            source: src_fp,
            unlink_source: self.mode == Mode::Replace && self.backup_id.is_none(),
            backup,
            outputs,
            retire,
            mtime: mtime(),
            edit: Some(EditState::default()),
        };
        let new_set = &self.new_set;
        let mut produce = |i: usize| -> Result<Produced, ErrKind> {
            let n = names[i].clone();
            let bytes = new_set[&n].clone();
            let d = auto_crop_codecs::probe(&bytes).unwrap();
            Ok(Produced {
                bytes,
                dims: (d.width, d.height),
                format: Format::Png,
            })
        };
        commit_group(&req, &mut produce, hook)
    }

    /// A completed first save of `old_n` outputs that a later commit then replaces.
    fn with_earlier_save(mut self, old_n: usize, new_n: usize) -> Self {
        assert_eq!(self.mode, Mode::Replace);
        self.new_set = self.set_of(old_n, 0);
        let saved = self.commit(&NoFaults).unwrap();
        self.backup_id = saved.backup_id;
        self.old_set = self.new_set.clone();
        self.new_set = self.set_of(new_n, 100);
        self
    }

    fn recorded_steps(&self) -> Vec<Step> {
        let seen = RefCell::new(Vec::new());
        let hook = |s: &Step| {
            seen.borrow_mut().push(*s);
            Fault::Pass
        };
        // A recording run on an identical, throw-away copy of this fixture is not possible (the
        // commit consumes the disk), so callers build a second fixture for it.
        self.commit(&hook).unwrap();
        seen.into_inner()
    }

    fn files_in_outputs_dir(&self) -> BTreeMap<String, Vec<u8>> {
        let od = outs_dir(&self.dir, self.mode);
        let mut m = BTreeMap::new();
        for e in fs::read_dir(&od).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_file() && p != self.src {
                let n = e.file_name().to_string_lossy().into_owned();
                if !n.starts_with(".autocrop-") {
                    m.insert(n, fs::read(&p).unwrap());
                }
            }
        }
        m
    }

    /// The invariant. Returns whether the old or the new set is in place.
    fn check(&self, ctx: &str) -> Which {
        self.check_with(ctx, false)
    }

    /// `leftover_ok`: an old output that was due to be retired may still be there (its removal
    /// failed); it is not a partial set, the new set is complete beside it.
    fn check_with(&self, ctx: &str, leftover_ok: bool) -> Which {
        let od = outs_dir(&self.dir, self.mode);
        let mut files = self.files_in_outputs_dir();
        if leftover_ok {
            for (n, b) in &self.old_set {
                if !self.new_set.contains_key(n) && files.get(n) == Some(b) {
                    files.remove(n);
                }
            }
        }
        let which = if files == self.new_set {
            Which::New
        } else if files == self.old_set {
            Which::Old
        } else {
            panic!(
                "{ctx}: a partial set: have {:?}, old {:?}, new {:?}",
                files.keys().collect::<Vec<_>>(),
                self.old_set.keys().collect::<Vec<_>>(),
                self.new_set.keys().collect::<Vec<_>>()
            );
        };
        assert!(
            stray_temps(&od).is_empty(),
            "{ctx}: stray temps {:?}",
            stray_temps(&od)
        );
        assert!(stray_temps(&self.dir).is_empty(), "{ctx}: stray temps");
        assert_eq!(pending_journals(&self.store), 0, "{ctx}: a journal is left");

        // The scan's bytes are always somewhere.
        let in_place = fs::read(&self.src).ok();
        let manifests = self.store.list();
        let in_backup = manifests.iter().any(|m| {
            self.store
                .original_path(m)
                .and_then(|p| fs::read(p).ok())
                .is_some_and(|b| b == self.src_bytes)
        });
        assert!(
            in_place.as_deref() == Some(&self.src_bytes[..]) || in_backup,
            "{ctx}: the scan is nowhere"
        );
        // A source that exists is never altered.
        if let Some(b) = &in_place {
            assert_eq!(b, &self.src_bytes, "{ctx}: the scan was altered");
        }

        match (self.mode, which, self.old_set.is_empty()) {
            // First save, rolled back: the scan is in place and nothing is left in the store.
            (Mode::Replace, Which::Old, true) => {
                assert!(in_place.is_some(), "{ctx}");
                assert!(manifests.is_empty(), "{ctx}: an unused backup is left");
            }
            // First save, complete: the set, a Saved OneToN backup, and the scan removed (or, if
            // it could not be, still intact).
            (Mode::Replace, Which::New, true) => {
                assert_eq!(manifests.len(), 1, "{ctx}");
                let m = &manifests[0];
                assert_eq!(
                    (m.state, m.kind),
                    (BackupState::Saved, BackupKind::OneToN),
                    "{ctx}"
                );
                assert_eq!(m.outputs.len(), self.new_set.len(), "{ctx}");
                for (o, (n, b)) in m.outputs.iter().zip(&self.new_set) {
                    assert!(o.path.ends_with(n.as_str()), "{ctx}");
                    assert_eq!(o.blake3, blake3_hex(b), "{ctx}");
                    assert!(o.item_id.is_some() && o.index.is_some(), "{ctx}");
                }
                assert!(in_backup, "{ctx}: no verified backup of the scan");
            }
            // Re-save: either way the backup stays Saved and lists the set that is on disk.
            (Mode::Replace, w, false) => {
                assert_eq!(manifests.len(), 1, "{ctx}");
                let m = &manifests[0];
                assert_eq!(m.state, BackupState::Saved, "{ctx}");
                let listed: BTreeMap<String, String> = m
                    .outputs
                    .iter()
                    .map(|o| {
                        (
                            Path::new(&o.path)
                                .file_name()
                                .unwrap()
                                .to_string_lossy()
                                .into_owned(),
                            o.blake3.clone(),
                        )
                    })
                    .collect();
                let want = if w == Which::New {
                    &self.new_set
                } else {
                    &self.old_set
                };
                let want: BTreeMap<String, String> = want
                    .iter()
                    .map(|(n, b)| (n.clone(), blake3_hex(b)))
                    .collect();
                assert_eq!(
                    listed, want,
                    "{ctx}: the manifest does not list the set on disk"
                );
                assert!(in_backup, "{ctx}");
                assert!(!self.src.exists(), "{ctx}: a re-save put the scan back");
                if w == Which::Old {
                    // A rolled-back re-save keeps nothing extra in the store.
                    let kept = self.store.entry_path(&m.id).unwrap().join("superseded");
                    let n = fs::read_dir(&kept).map(|r| r.count()).unwrap_or(0);
                    assert_eq!(n, 0, "{ctx}: kept copies left behind");
                }
            }
            (Mode::Copy, _, _) => {
                assert_eq!(
                    in_place.as_deref(),
                    Some(&self.src_bytes[..]),
                    "{ctx}: the scan moved"
                );
                assert!(manifests.is_empty(), "{ctx}");
            }
        }
        which
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Which {
    Old,
    New,
}

type Scenario = (&'static str, Box<dyn Fn() -> Fx>);

fn scenarios() -> Vec<Scenario> {
    vec![
        (
            "first save, 3 items, replace",
            Box::new(|| Fx::new(Mode::Replace, 3)),
        ),
        (
            "first save, 2 items, replace",
            Box::new(|| Fx::new(Mode::Replace, 2)),
        ),
        ("copy, 3 items", Box::new(|| Fx::new(Mode::Copy, 3))),
        (
            "re-save 4 to 3",
            Box::new(|| Fx::new(Mode::Replace, 1).with_earlier_save(4, 3)),
        ),
        (
            "re-save 3 to 5",
            Box::new(|| Fx::new(Mode::Replace, 1).with_earlier_save(3, 5)),
        ),
        (
            "re-save 2 to 2",
            Box::new(|| Fx::new(Mode::Replace, 1).with_earlier_save(2, 2)),
        ),
    ]
}

#[test]
fn a_clean_commit_places_every_output_backs_up_and_removes_the_scan_last() {
    let fx = Fx::new(Mode::Replace, 4);
    let saved = fx.commit(&NoFaults).unwrap();
    assert_eq!(saved.outputs.len(), 4);
    assert!(saved.source_removed && saved.notes.is_empty() && !saved.journal_pending);
    assert!(!fx.src.exists());
    assert_eq!(fx.check("clean"), Which::New);
    // The backup restores the scan byte for byte.
    let m = &fx.store.list()[0];
    assert_eq!(
        fs::read(fx.store.original_path(m).unwrap()).unwrap(),
        fx.src_bytes
    );
    // The outputs carry the scan's mtime and are decodable.
    for (n, b) in &fx.new_set {
        let p = fx.dir.join(n);
        assert_eq!(fs::metadata(&p).unwrap().modified().unwrap(), mtime());
        assert!(
            auto_crop_codecs::decode(&fs::read(p).unwrap()).is_ok(),
            "{n} {}",
            b.len()
        );
    }
}

#[test]
fn the_scan_is_removed_after_the_last_output_is_in_place_never_before() {
    // At every step before the unlink the scan is still there; at the first step after it, gone.
    let fx = Fx::new(Mode::Replace, 3);
    let src = fx.src.clone();
    let dir = fx.dir.clone();
    let names: Vec<String> = fx.new_set.keys().cloned().collect();
    let log: RefCell<Vec<(Step, bool, usize)>> = RefCell::new(Vec::new());
    let hook = |s: &Step| {
        let placed = names.iter().filter(|n| dir.join(n).exists()).count();
        log.borrow_mut().push((*s, src.exists(), placed));
        Fault::Pass
    };
    fx.commit(&hook).unwrap();
    let mut gone_at = None;
    for (s, exists, placed) in log.borrow().iter() {
        if !exists && gone_at.is_none() {
            gone_at = Some(*s);
        }
        if *exists {
            assert!(gone_at.is_none(), "the scan came back at {s:?}");
        }
        // The scan disappears only once all three outputs are placed.
        if !exists {
            assert_eq!(
                *placed, 3,
                "the scan was removed with {placed} of 3 outputs at {s:?}"
            );
        }
    }
    assert!(
        matches!(gone_at, Some(Step::AfterUnlink | Step::ManifestSaved)),
        "{gone_at:?}"
    );
}

#[test]
fn a_failure_at_every_step_leaves_the_old_state_or_the_complete_set() {
    for (label, make) in scenarios() {
        let steps = make().recorded_steps();
        assert!(steps.len() >= 8, "{label}: {steps:?}");
        for step in &steps {
            let fx = make();
            let target = *step;
            let hook = move |s: &Step| {
                if *s == target {
                    Fault::Fail(ErrKind::DiskFull)
                } else {
                    Fault::Pass
                }
            };
            let result = fx.commit(&hook);
            let ctx = format!("{label}: fail at {step:?}");
            if step.after_commit() {
                // After the commit point a failure never takes the set back.
                let saved = result.clone().unwrap_or_else(|e| panic!("{ctx}: {e:?}"));
                if saved.journal_pending {
                    // The bookkeeping is finished at the next start.
                    recover(&fx.store, &NoFaults);
                }
                let which = fx.check_with(&ctx, matches!(step, Step::Retired(_)));
                assert_eq!(which, Which::New, "{ctx}");
            } else {
                let which = fx.check(&ctx);
                assert_eq!(
                    result.unwrap_err(),
                    GroupError::Failed(ErrKind::DiskFull),
                    "{ctx}"
                );
                assert_eq!(which, Which::Old, "{ctx}: the set stayed after a failure");
            }
        }
    }
}

#[test]
fn a_crash_at_every_step_is_recovered_to_the_old_state_or_the_complete_set() {
    for (label, make) in scenarios() {
        let steps = make().recorded_steps();
        for step in &steps {
            let fx = make();
            let target = *step;
            let hook = move |s: &Step| {
                if *s == target {
                    Fault::Crash
                } else {
                    Fault::Pass
                }
            };
            let result = fx.commit(&hook);
            let ctx = format!("{label}: crash at {step:?}");
            assert_eq!(result.unwrap_err(), GroupError::Crashed, "{ctx}");
            // The next start.
            let report = recover(&fx.store, &NoFaults);
            let which = fx.check(&ctx);
            // Recovery is idempotent.
            let again = recover(&fx.store, &NoFaults);
            assert_eq!(
                again,
                Default::default(),
                "{ctx}: second recovery found work"
            );
            assert_eq!(fx.check(&format!("{ctx} (again)")), which);
            // The rule: until Committing is journalled, back; from then on, forward.
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
    // Crash the commit after two renames, then crash recovery at each of its steps, then recover.
    for (label, make) in [
        (
            "first save",
            Box::new(|| Fx::new(Mode::Replace, 3)) as Box<dyn Fn() -> Fx>,
        ),
        (
            "re-save",
            Box::new(|| Fx::new(Mode::Replace, 1).with_earlier_save(3, 4)),
        ),
    ] {
        let recovery_steps = {
            let fx = make();
            let hook = |s: &Step| {
                if *s == Step::AfterRename(1) {
                    Fault::Crash
                } else {
                    Fault::Pass
                }
            };
            let _ = fx.commit(&hook);
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
            let _ = fx.commit(&|s: &Step| {
                if *s == Step::AfterRename(1) {
                    Fault::Crash
                } else {
                    Fault::Pass
                }
            });
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

#[test]
fn a_scan_changed_before_the_renames_is_left_alone_and_nothing_is_written() {
    let fx = Fx::new(Mode::Replace, 3);
    let src = fx.src.clone();
    let hook = |s: &Step| {
        if *s == Step::SourceRestat {
            fs::write(&src, b"someone else edited the scan").unwrap();
        }
        Fault::Pass
    };
    let r = fx.commit(&hook);
    assert_eq!(r.unwrap_err(), GroupError::Failed(ErrKind::SourceChanged));
    assert_eq!(fs::read(&fx.src).unwrap(), b"someone else edited the scan");
    assert!(fx.files_in_outputs_dir().is_empty());
    assert!(stray_temps(&fx.dir).is_empty() && pending_journals(&fx.store) == 0);
    // PLAN 2.7: the backup is kept (it holds the version Auto Crop read).
    let ms = fx.store.list();
    assert_eq!(ms.len(), 1);
    assert_eq!(
        fs::read(fx.store.original_path(&ms[0]).unwrap()).unwrap(),
        fx.src_bytes
    );
}

#[test]
fn a_scan_changed_after_the_renames_is_not_unlinked() {
    let fx = Fx::new(Mode::Replace, 3);
    let src = fx.src.clone();
    let hook = |s: &Step| {
        if *s == Step::BeforeUnlink {
            fs::write(&src, b"edited after the set was placed").unwrap();
        }
        Fault::Pass
    };
    let saved = fx.commit(&hook).unwrap();
    assert!(!saved.source_removed);
    assert_eq!(saved.notes, [ErrKind::SourceChanged]);
    assert_eq!(
        fs::read(&fx.src).unwrap(),
        b"edited after the set was placed"
    );
    assert_eq!(
        fx.files_in_outputs_dir(),
        fx.new_set,
        "the complete set is there"
    );
    assert_eq!(pending_journals(&fx.store), 0);
}

#[test]
fn a_corrupt_temp_fails_verification_and_nothing_is_placed() {
    let fx = Fx::new(Mode::Replace, 3);
    let dir = fx.dir.clone();
    let second = fx.new_set[&name(2)].clone();
    let hook = |s: &Step| {
        if *s == Step::TempWritten(1) {
            // The second temp is truncated on disk after it was written.
            let t = stray_temps(&dir)
                .into_iter()
                .find(|p| fs::read(p).unwrap() == second)
                .expect("the second temp");
            fs::write(&t, &second[..second.len() / 2]).unwrap();
        }
        Fault::Pass
    };
    let r = fx.commit(&hook);
    assert_eq!(r.unwrap_err(), GroupError::Failed(ErrKind::VerifyFailed));
    assert_eq!(fx.check("bad temp"), Which::Old);
}

#[test]
fn a_failing_encoder_for_one_item_writes_none() {
    let fx = Fx::new(Mode::Replace, 3);
    let od = outs_dir(&fx.dir, fx.mode);
    let names: Vec<String> = fx.new_set.keys().cloned().collect();
    let req = GroupRequest {
        store: &fx.store,
        source: fx.fingerprint(),
        unlink_source: true,
        backup: BackupPlan::New(NewBackup {
            source: &fx.src,
            source_blake3: &blake3_hex(&fx.src_bytes),
            source_size: fx.src_bytes.len() as u64,
            source_mtime_ms: unix_ms(mtime()),
            format_ext: "png",
            run_id: "r",
            run_name: "r",
            retention_days: None,
            edit: None,
        }),
        outputs: names
            .iter()
            .enumerate()
            .map(|(i, n)| OutSpec {
                item_id: i as u32,
                index: i as u32 + 1,
                final_path: od.join(n),
                replaces_blake3: None,
            })
            .collect(),
        retire: vec![],
        mtime: mtime(),
        edit: None,
    };
    let mut produce = |i: usize| -> Result<Produced, ErrKind> {
        if i == 2 {
            return Err(ErrKind::DiskFull);
        }
        let b = fx.new_set[&names[i]].clone();
        let d = auto_crop_codecs::probe(&b).unwrap();
        Ok(Produced {
            bytes: b,
            dims: (d.width, d.height),
            format: Format::Png,
        })
    };
    let r = commit_group(&req, &mut produce, &NoFaults);
    assert_eq!(r.unwrap_err(), GroupError::Failed(ErrKind::DiskFull));
    assert_eq!(fs::read(&fx.src).unwrap(), fx.src_bytes);
    assert!(fx.files_in_outputs_dir().is_empty());
    assert!(stray_temps(&fx.dir).is_empty() && fx.store.list().is_empty());
    assert_eq!(pending_journals(&fx.store), 0);
}

#[test]
fn a_name_taken_between_the_plan_and_the_renames_stops_the_whole_group() {
    let fx = Fx::new(Mode::Replace, 3);
    let second = fx.dir.join(name(2));
    let hook = |s: &Step| {
        if *s == Step::AfterRename(0) {
            fs::write(&second, b"someone's unrelated file").unwrap();
        }
        Fault::Pass
    };
    let r = fx.commit(&hook);
    assert_eq!(r.unwrap_err(), GroupError::Failed(ErrKind::PlanStale));
    // Their file is untouched, our first output was taken back, the scan is where it was.
    assert_eq!(fs::read(&second).unwrap(), b"someone's unrelated file");
    assert!(!fx.dir.join(name(1)).exists() && !fx.dir.join(name(3)).exists());
    assert_eq!(fs::read(&fx.src).unwrap(), fx.src_bytes);
    assert!(stray_temps(&fx.dir).is_empty() && fx.store.list().is_empty());
    assert_eq!(pending_journals(&fx.store), 0);
}

#[test]
fn no_output_ever_overwrites_an_existing_file_even_when_crashed_and_recovered() {
    // A foreign file appears at the third name while the commit is crashed after two renames:
    // recovery must neither clobber it nor leave a partial set.
    let fx = Fx::new(Mode::Replace, 3);
    let third = fx.dir.join(name(3));
    let _ = fx.commit(&|s: &Step| {
        if *s == Step::AfterRename(1) {
            Fault::Crash
        } else {
            Fault::Pass
        }
    });
    fs::write(&third, b"foreign").unwrap();
    let report = recover(&fx.store, &NoFaults);
    assert_eq!(report.rolled_back.len(), 1, "{report:?}");
    assert_eq!(fs::read(&third).unwrap(), b"foreign");
    assert!(!fx.dir.join(name(1)).exists() && !fx.dir.join(name(2)).exists());
    assert_eq!(fs::read(&fx.src).unwrap(), fx.src_bytes);
    assert!(stray_temps(&fx.dir).is_empty() && fx.store.list().is_empty());
}

#[test]
fn a_scan_that_cannot_be_removed_keeps_the_set_and_says_so() {
    let fx = Fx::new(Mode::Replace, 3);
    // Held open with read and write sharing but not delete sharing: reading works (the re-hash),
    // deleting does not. The lock is taken at BeforeUnlink, through this cell.
    let lock = RefCell::new(None::<fs::File>);
    let src = fx.src.clone();
    #[cfg(windows)]
    let hook = |s: &Step| {
        use std::os::windows::fs::OpenOptionsExt;
        if *s == Step::BeforeUnlink {
            *lock.borrow_mut() = Some(
                fs::OpenOptions::new()
                    .read(true)
                    .share_mode(1 | 2) // FILE_SHARE_READ | FILE_SHARE_WRITE
                    .open(&src)
                    .unwrap(),
            );
        }
        Fault::Pass
    };
    #[cfg(not(windows))]
    let hook = |s: &Step| {
        let _ = (&src, &lock);
        if *s == Step::BeforeUnlink {
            Fault::Fail(ErrKind::FileInUse)
        } else {
            Fault::Pass
        }
    };
    let saved = fx.commit(&hook).unwrap();
    assert!(!saved.source_removed);
    assert_eq!(saved.notes, [ErrKind::SavedSourceInUse]);
    assert!(fx.src.exists(), "the scan stays");
    assert_eq!(
        fx.files_in_outputs_dir(),
        fx.new_set,
        "the complete set is there"
    );
    assert_eq!(pending_journals(&fx.store), 0);
    let m = &fx.store.list()[0];
    assert_eq!(m.state, BackupState::Saved);
}

#[test]
fn a_re_save_never_replaces_a_derived_file_the_user_edited() {
    let fx = Fx::new(Mode::Replace, 1).with_earlier_save(3, 3);
    fs::write(fx.dir.join(name(2)), b"edited by the user").unwrap();
    let r = fx.commit(&NoFaults);
    assert_eq!(r.unwrap_err(), GroupError::Failed(ErrKind::PlanStale));
    assert_eq!(
        fs::read(fx.dir.join(name(2))).unwrap(),
        b"edited by the user"
    );
    // The other two are exactly as the earlier save left them.
    for n in [name(1), name(3)] {
        assert_eq!(fs::read(fx.dir.join(&n)).unwrap(), fx.old_set[&n]);
    }
    assert!(stray_temps(&fx.dir).is_empty() && pending_journals(&fx.store) == 0);
}

#[test]
fn a_re_save_keeps_the_replaced_and_retired_bytes_in_the_store() {
    let fx = Fx::new(Mode::Replace, 1).with_earlier_save(4, 3);
    fx.commit(&NoFaults).unwrap();
    assert_eq!(fx.files_in_outputs_dir(), fx.new_set);
    let id = fx.backup_id.clone().unwrap();
    let kept: Vec<Vec<u8>> = fs::read_dir(fx.store.entry_path(&id).unwrap().join("superseded"))
        .unwrap()
        .flatten()
        .map(|e| fs::read(e.path()).unwrap())
        .collect();
    // Three replaced outputs and one retired output: all four old files are still in the store.
    assert_eq!(kept.len(), 4);
    for b in fx.old_set.values() {
        assert!(kept.contains(b));
    }
}

#[test]
fn recovery_leaves_unreadable_journals_and_unrelated_files_alone() {
    let fx = Fx::new(Mode::Replace, 2);
    fs::create_dir_all(fx.store.groups_dir()).unwrap();
    let junk = fx.store.groups_dir().join("junk.json");
    fs::write(&junk, b"not a journal").unwrap();
    let unrelated = fx.dir.join(".autocrop-notours.tmp");
    fs::write(&unrelated, b"x").unwrap();
    let report = recover(&fx.store, &NoFaults);
    assert_eq!(report.left.len(), 1);
    assert!(
        junk.exists() && unrelated.exists(),
        "never deletes what no journal names"
    );
}

/// A real process death at each step (not just an early return): the child runs the commit and
/// aborts at the step named by an environment variable; this process then recovers and checks the
/// invariant. The child is this test binary run on `child_commit`.
#[test]
fn child_commit() {
    let (Ok(root), Ok(step)) = (
        std::env::var("AC_FAULT_ROOT"),
        std::env::var("AC_FAULT_STEP"),
    ) else {
        return; // an ordinary run of the suite
    };
    let root = PathBuf::from(root);
    let store = Store::new(root.join("backups"));
    let dir = root.join("photos");
    let src = dir.join("scan.png");
    let bytes = fs::read(&src).unwrap();
    let sets: Vec<Vec<u8>> = (1..=3)
        .map(|i| png(10 + i, 8 + i, 30 + 10 * i as u8))
        .collect();
    let hash = blake3_hex(&bytes);
    let m = fs::metadata(&src).unwrap();
    let req = GroupRequest {
        store: &store,
        source: SourceFingerprint {
            path: src.clone(),
            size: m.len(),
            mtime_ms: unix_ms(m.modified().unwrap()),
            blake3: hash.clone(),
        },
        unlink_source: true,
        backup: BackupPlan::New(NewBackup {
            source: &src,
            source_blake3: &hash,
            source_size: bytes.len() as u64,
            source_mtime_ms: unix_ms(m.modified().unwrap()),
            format_ext: "png",
            run_id: "r",
            run_name: "r",
            retention_days: None,
            edit: None,
        }),
        outputs: (0..3)
            .map(|i| OutSpec {
                item_id: i as u32,
                index: i as u32 + 1,
                final_path: dir.join(name(i + 1)),
                replaces_blake3: None,
            })
            .collect(),
        retire: vec![],
        mtime: mtime(),
        edit: None,
    };
    let mut produce = |i: usize| -> Result<Produced, ErrKind> {
        let b = sets[i].clone();
        let d = auto_crop_codecs::probe(&b).unwrap();
        Ok(Produced {
            bytes: b,
            dims: (d.width, d.height),
            format: Format::Png,
        })
    };
    let hook = |s: &Step| {
        if format!("{s:?}") == step {
            // A real process death: no unwinding, no destructors, no cleanup.
            std::process::exit(86);
        }
        Fault::Pass
    };
    let _ = commit_group(&req, &mut produce, &hook);
}

#[test]
fn killing_the_process_at_every_step_never_leaves_a_partial_set() {
    let exe = std::env::current_exe().unwrap();
    let steps: Vec<String> = Fx::new(Mode::Replace, 3)
        .recorded_steps()
        .iter()
        .map(|s| format!("{s:?}"))
        .collect();
    for step in steps {
        let fx = Fx::new(Mode::Replace, 3);
        let root = fx._root.path().to_path_buf();
        let out = std::process::Command::new(&exe)
            .args(["--exact", "child_commit", "--nocapture", "--test-threads=1"])
            .env("AC_FAULT_ROOT", &root)
            .env("AC_FAULT_STEP", &step)
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(86),
            "the child should have died at {step}"
        );
        recover(&fx.store, &NoFaults);
        // The children build their own outputs (shades 40, 50, 60); compare by count and hash.
        let files = fx.files_in_outputs_dir();
        assert!(
            files.is_empty() || files.len() == 3,
            "{step}: partial set {:?}",
            files.keys()
        );
        assert!(stray_temps(&fx.dir).is_empty(), "{step}");
        assert_eq!(pending_journals(&fx.store), 0, "{step}");
        let in_place = fs::read(&fx.src).ok();
        let in_backup = fx.store.list().iter().any(|m| {
            fx.store
                .original_path(m)
                .and_then(|p| fs::read(p).ok())
                .is_some_and(|b| b == fx.src_bytes)
        });
        assert!(
            in_place.as_deref() == Some(&fx.src_bytes[..]) || in_backup,
            "{step}: the scan is nowhere"
        );
        if files.is_empty() {
            assert!(in_place.is_some() && fx.store.list().is_empty(), "{step}");
        } else {
            assert!(in_backup, "{step}");
        }
    }
}
