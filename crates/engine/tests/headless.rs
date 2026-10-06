// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The small surface a headless front end (the CLI, ROADMAP M2.39) needs on top of the engine:
//! explicit settings that are never persisted, an engine that does no housekeeping, saves grouped
//! into a run the caller names, per-run JPEG quality and crop margin, and removing one backup.

use auto_crop_core::{SplitPolicy, SplitProfile};
use auto_crop_engine::store::Store;
use auto_crop_engine::{
    AppPaths, Engine, EngineOptions, Housekeeping, ItemView, QualitySetting, RestoreMode,
    RunOptions, SaveTarget, Settings,
};
use std::fs;
use std::path::PathBuf;

fn setup() -> (tempfile::TempDir, AppPaths, Vec<PathBuf>) {
    let root = tempfile::tempdir().unwrap();
    let paths = AppPaths::under(&root.path().join("app"));
    let files = auto_crop_engine::samples::write_samples(&root.path().join("photos")).unwrap();
    (root, paths, files)
}

fn pick(files: &[PathBuf], name: &str) -> PathBuf {
    files
        .iter()
        .find(|f| f.file_name().unwrap().to_string_lossy() == name)
        .unwrap()
        .clone()
}

fn nop(_: ItemView) {}

#[test]
fn explicit_settings_are_used_and_never_written_and_no_housekeeping_touches_the_disk() {
    let (_root, paths, files) = setup();
    let settings = Settings {
        split_policy: SplitPolicy::Never,
        split_profile: SplitProfile::Receipts,
        retention_days: Some(7),
        ..Settings::default()
    };
    let engine = Engine::with_settings(paths.clone(), settings.clone(), Housekeeping::None);
    assert_eq!(engine.settings(), settings);
    let id = engine
        .open_paths(&[pick(&files, "receipt_2_straight.jpg")], false)
        .ids[0];
    let view = engine.analyse(id).unwrap();
    assert!(view.edit.is_some());
    // Analysis and a listing created nothing: no data folder, no settings file.
    assert_eq!(engine.list_backups().runs.len(), 0);
    assert!(!paths.data_dir.exists() && !paths.config_dir.exists());
    assert!(!paths.settings_file().exists());
}

#[test]
fn an_engine_without_housekeeping_does_not_purge_and_one_with_it_does() {
    let (_root, paths, files) = setup();
    let engine = Engine::with_settings(
        paths.clone(),
        Settings::default(),
        Housekeeping::RecoverAndPurge,
    );
    let id = engine
        .open_paths(&[pick(&files, "receipt_2_straight.jpg")], false)
        .ids[0];
    engine.analyse(id).unwrap();
    let out = engine.save_in_run(id, SaveTarget::Replace, "run1", "Run 1");
    assert!(out.ok, "{out:?}");
    let bid = out.saved.unwrap().backup_id.unwrap();
    // Age the backup past its retention.
    let store = Store::new(paths.backups_dir());
    let mut m = store.read(&bid).unwrap();
    m.purge_after = Some(1);
    store.write(&m).unwrap();
    let quiet = Engine::with_settings(paths.clone(), Settings::default(), Housekeeping::None);
    assert_eq!(
        quiet.list_backups().runs.len(),
        1,
        "no purge without housekeeping"
    );
    let _busy = Engine::with_settings(
        paths.clone(),
        Settings::default(),
        Housekeeping::RecoverAndPurge,
    );
    assert!(
        store.read(&bid).is_none(),
        "start-up purge removed the expired backup"
    );
}

#[test]
fn saves_named_into_one_run_are_one_backup_run_and_restore_together() {
    let (_root, paths, files) = setup();
    let engine = Engine::with_settings(paths, Settings::default(), Housekeeping::RecoverAndPurge);
    let names = ["receipt_2_straight.jpg", "document_2_slight_tilt.jpg"];
    let originals: Vec<Vec<u8>> = names
        .iter()
        .map(|n| fs::read(pick(&files, n)).unwrap())
        .collect();
    for n in names {
        let id = engine.open_paths(&[pick(&files, n)], false).ids[0];
        engine.analyse(id).unwrap();
        assert!(
            engine
                .save_in_run(id, SaveTarget::Replace, "one-run", "CLI run")
                .ok
        );
    }
    let view = engine.list_backups();
    assert_eq!(view.runs.len(), 1, "two saves, one run");
    assert_eq!(view.runs[0].id, "one-run");
    assert_eq!(view.runs[0].file_count, 2);
    let outcomes = engine.restore_run("one-run", &nop);
    assert!(outcomes.iter().all(|o| o.ok));
    for (n, o) in names.iter().zip(&originals) {
        assert_eq!(
            &fs::read(pick(&files, n)).unwrap(),
            o,
            "{n} is byte identical"
        );
    }
    // restore_file still accepts a restore mode.
    let _ = RestoreMode::Auto;
}

#[test]
fn run_options_grow_or_trim_the_crop_and_the_engine_options_set_the_quality() {
    let (_root, paths, files) = setup();
    let size_with = |q: Option<u8>, margin: f32| -> (u64, (f64, f64)) {
        let engine = Engine::with_settings(paths.clone(), Settings::default(), Housekeeping::None);
        engine.set_run_options(RunOptions { margin_pct: margin });
        if let Some(value) = q {
            engine.set_options(EngineOptions {
                quality: QualitySetting::Fixed { value },
                ..EngineOptions::default()
            });
        }
        let copy = files[0]
            .parent()
            .unwrap()
            .join(format!("c-{q:?}-{margin}.jpg"));
        fs::copy(pick(&files, "receipt_2_straight.jpg"), &copy).unwrap();
        let id = engine.open_paths(std::slice::from_ref(&copy), false).ids[0];
        let view = engine.analyse(id).unwrap();
        let quad = view.edit.unwrap().quad;
        let (w, h) = (quad[1].x - quad[0].x, quad[3].y - quad[0].y);
        let out = engine.save_in_run(id, SaveTarget::Copy, "r", "r");
        assert!(out.ok, "{out:?}");
        let written = copy
            .parent()
            .unwrap()
            .join("AutoCrop")
            .join(out.saved.unwrap().output);
        (fs::metadata(written).unwrap().len(), (w, h))
    };
    let (base_bytes, (w0, h0)) = size_with(None, 0.0);
    let (low_bytes, _) = size_with(Some(30), 0.0);
    assert!(
        low_bytes * 2 < base_bytes,
        "quality 30 ({low_bytes}) is far below the default ({base_bytes})"
    );
    let (_, (w5, h5)) = size_with(None, 5.0);
    let (_, (wn, hn)) = size_with(None, -5.0);
    assert!(w5 > w0 && h5 > h0, "a margin grows the crop");
    assert!(wn < w0 && hn < h0, "a negative margin trims it");
}

#[test]
fn a_margin_never_makes_a_held_result_look_reviewed() {
    let (_root, paths, files) = setup();
    let engine = Engine::with_settings(paths, Settings::default(), Housekeeping::None);
    engine.set_run_options(RunOptions { margin_pct: 10.0 });
    let id = engine
        .open_paths(&[pick(&files, "hard_low_contrast.jpg")], false)
        .ids[0];
    let with = engine.analyse(id).unwrap();
    let plain = Engine::with_settings(
        AppPaths::under(&_root.path().join("other")),
        Settings::default(),
        Housekeeping::None,
    );
    let id2 = plain
        .open_paths(&[pick(&files, "hard_low_contrast.jpg")], false)
        .ids[0];
    let without = plain.analyse(id2).unwrap();
    assert_eq!(
        with.confidence, without.confidence,
        "the confidence is the detector's, whatever the margin"
    );
    assert!(
        !with.edited,
        "a margin is part of the proposal, not a user edit"
    );
    let state = engine.edit_state(id).unwrap();
    assert!(
        state
            .items
            .iter()
            .all(|i| matches!(i.origin, auto_crop_core::Origin::Auto { .. }))
    );
}

#[test]
fn store_remove_deletes_one_valid_entry_and_nothing_else() {
    let (_root, paths, files) = setup();
    let engine = Engine::with_settings(
        paths.clone(),
        Settings::default(),
        Housekeeping::RecoverAndPurge,
    );
    let id = engine
        .open_paths(&[pick(&files, "receipt_2_straight.jpg")], false)
        .ids[0];
    engine.analyse(id).unwrap();
    let bid = engine
        .save_in_run(id, SaveTarget::Replace, "r", "r")
        .saved
        .unwrap()
        .backup_id
        .unwrap();
    let store = Store::new(paths.backups_dir());
    assert!(
        !store.remove("../../etc"),
        "an id that could escape the store is refused"
    );
    assert!(
        !store.remove("0123456789abcdef0123456789ab"),
        "an unknown id removes nothing"
    );
    assert!(store.read(&bid).is_some());
    assert!(store.remove(&bid));
    assert!(store.read(&bid).is_none());
    assert!(paths.backups_dir().is_dir(), "the store itself stays");
}
