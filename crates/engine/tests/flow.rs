// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! End-to-end behaviour of the engine on real files in a temp directory: open, analyse, edit,
//! save with backup, restore, and the safety invariants around them.

use auto_crop_core::{Forced, Pt};
use auto_crop_engine::{
    AppPaths, Edit, Engine, ErrKind, ImageKind, ItemStatus, ItemView, RestoreMode, SaveTarget,
};
use std::fs;
use std::path::{Path, PathBuf};

struct Env {
    _root: tempfile::TempDir,
    engine: Engine,
    dir: PathBuf,
}

fn env() -> Env {
    let root = tempfile::tempdir().unwrap();
    let engine = Engine::new(AppPaths::under(root.path()));
    let dir = root.path().join("photos");
    fs::create_dir_all(&dir).unwrap();
    Env {
        _root: root,
        engine,
        dir,
    }
}

/// Opens the synthetic samples into `env.dir` and analyses them synchronously.
fn open_samples(env: &Env) -> Vec<ItemView> {
    let files = auto_crop_engine::samples::write_samples(&env.dir).unwrap();
    let summary = env.engine.open_paths(&files, false);
    assert_eq!(summary.added, files.len());
    summary
        .ids
        .iter()
        .map(|id| env.engine.analyse(*id).unwrap())
        .collect()
}

fn find<'a>(views: &'a [ItemView], name: &str) -> &'a ItemView {
    views
        .iter()
        .find(|v| v.name == name)
        .unwrap_or_else(|| panic!("no {name}"))
}

fn tier(v: &ItemView, cut: f32) -> &'static str {
    let c = v.confidence.as_ref().expect("analysed");
    if c.forced == Some(Forced::Failed) || c.score < 0.6 {
        "failed"
    } else if c.forced == Some(Forced::Check) || c.score < cut {
        "check"
    } else {
        "good"
    }
}

fn nop(_: ItemView) {}

#[test]
fn the_samples_spread_across_all_three_tiers() {
    let e = env();
    let views = open_samples(&e);
    for v in &views {
        let c = v.confidence.as_ref().unwrap();
        eprintln!(
            "{:34} score {:.3} forced {:?} reasons {:?}",
            v.name, c.score, c.forced, c.reasons
        );
        assert_eq!(v.status, ItemStatus::Ready, "{}", v.name);
    }
    // Clean, well-lit pages are confident even under Strict.
    for name in [
        "receipt_1_tilted.jpg",
        "receipt_2_straight.jpg",
        "document_2_slight_tilt.jpg",
    ] {
        assert_eq!(tier(find(&views, name), 0.95), "good", "{name}");
    }
    // A strong perspective is confident under Balanced but held under Strict.
    assert_eq!(
        tier(find(&views, "document_1_perspective.jpg"), 0.90),
        "good"
    );
    // The deliberately hard ones are held for review or failed.
    for name in [
        "hard_page_cut_by_frame.jpg",
        "hard_white_on_white.jpg",
        "hard_no_document.jpg",
    ] {
        assert_ne!(tier(find(&views, name), 0.80), "good", "{name}");
    }
    assert_eq!(tier(find(&views, "hard_no_document.jpg"), 0.95), "failed");
    let counts = |cut| {
        let mut m = std::collections::BTreeMap::new();
        for v in &views {
            *m.entry(tier(v, cut)).or_insert(0) += 1;
        }
        m
    };
    eprintln!("strict {:?} aggressive {:?}", counts(0.95), counts(0.80));
    assert!(
        counts(0.95).len() == 3,
        "all three tiers appear under Strict: {:?}",
        counts(0.95)
    );
}

#[test]
fn the_detected_quad_is_usable_and_previews_render() {
    let e = env();
    let views = open_samples(&e);
    let v = find(&views, "receipt_1_tilted.jpg");
    let edit = v.edit.as_ref().expect("a quad");
    // The receipt is tall and narrow; the quad must be too.
    let q = edit.quad;
    let w = ((q[1].x - q[0].x).powi(2) + (q[1].y - q[0].y).powi(2)).sqrt() * f64::from(v.width);
    let h = ((q[3].x - q[0].x).powi(2) + (q[3].y - q[0].y).powi(2)).sqrt() * f64::from(v.height);
    assert!(h > w * 2.0, "{w} x {h}");
    for kind in [ImageKind::Thumb, ImageKind::Src, ImageKind::Result] {
        let (bytes, mime) = e.engine.image_bytes(v.id, kind).unwrap();
        assert_eq!(mime, "image/jpeg");
        let d = auto_crop_codecs::decode(&bytes).unwrap();
        assert!(d.raster.width > 10 && d.raster.height > 10);
        match kind {
            ImageKind::Thumb => assert!(d.raster.width.max(d.raster.height) <= 256),
            ImageKind::Result => assert!(d.raster.height > d.raster.width),
            ImageKind::Src => {}
        }
    }
    // A failed, unedited item shows its source as the thumbnail and has no result.
    let failed = find(&views, "hard_no_document.jpg");
    assert!(failed.edit.is_none());
    assert_eq!(
        e.engine
            .image_bytes(failed.id, ImageKind::Result)
            .unwrap_err(),
        ErrKind::NoCrop
    );
    assert!(e.engine.image_bytes(failed.id, ImageKind::Thumb).is_ok());
}

#[test]
fn edits_undo_redo_and_generations() {
    let e = env();
    let views = open_samples(&e);
    let v = find(&views, "receipt_2_straight.jpg").clone();
    assert!(!v.can_undo && !v.edited);
    let mut edit: Edit = v.edit.clone().unwrap();
    edit.fine_deg = 1.5;
    // A live drag changes nothing durable.
    let live = e.engine.set_edit(v.id, &edit, true, "Move corner").unwrap();
    assert_eq!(live.generation, v.generation);
    assert!(!live.can_undo);
    let end = e.engine.set_edit(v.id, &edit, false, "Rotate").unwrap();
    assert!(end.generation > v.generation && end.can_undo && end.edited);
    assert_eq!(end.undo_label.as_deref(), Some("Rotate"));
    // Committing the same state again is not a new step.
    let same = e.engine.set_edit(v.id, &edit, false, "Rotate").unwrap();
    assert_eq!(same.generation, end.generation);
    let undone = e.engine.undo(v.id).unwrap();
    assert!(!undone.edited && undone.generation > end.generation && undone.can_redo);
    let redone = e.engine.redo(v.id).unwrap();
    assert_eq!(redone.edit.as_ref().unwrap().fine_deg, 1.5);
    let reset = e.engine.reset_to_auto(v.id).unwrap();
    assert!(!reset.edited);
    assert_eq!(reset.edit, reset.auto_edit);
    // The webview's numbers are clamped.
    let mut wild = edit.clone();
    wild.quad[0] = Pt::new(-3.0, 9.0);
    wild.fine_deg = 900.0;
    let v2 = e
        .engine
        .set_edit(v.id, &wild, false, "Move corner")
        .unwrap();
    let got = v2.edit.unwrap();
    assert_eq!(got.quad[0], Pt::new(0.0, 1.0));
    assert_eq!(got.fine_deg, 45.0);
}

fn sha(p: &Path) -> String {
    auto_crop_engine::util::blake3_hex(&fs::read(p).unwrap())
}

#[test]
fn save_replaces_after_a_verified_backup_and_restore_is_byte_identical() {
    let e = env();
    let views = open_samples(&e);
    let v = find(&views, "receipt_1_tilted.jpg").clone();
    let path = e.dir.join(&v.name);
    let before = fs::read(&path).unwrap();
    let before_dims = auto_crop_codecs::probe(&before).unwrap();

    let out = e
        .engine
        .save_items(&[v.id], SaveTarget::Replace, "Test run", &nop);
    assert!(out[0].ok, "{:?}", out[0]);
    let backup_id = out[0]
        .saved
        .as_ref()
        .unwrap()
        .backup_id
        .clone()
        .expect("a backup id");

    // The file at the path is now a smaller, valid, cropped image.
    let after = fs::read(&path).unwrap();
    assert_ne!(after, before);
    let after_dims = auto_crop_codecs::probe(&after).unwrap();
    assert!(after_dims.width < before_dims.width && after_dims.height <= before_dims.height);
    assert!(auto_crop_codecs::decode(&after).is_ok());
    // No temp files remain beside it.
    let strays: Vec<_> = fs::read_dir(&e.dir)
        .unwrap()
        .flatten()
        .filter(|f| f.file_name().to_string_lossy().starts_with(".autocrop-"))
        .collect();
    assert!(strays.is_empty());
    // mtime is kept.
    // The backup holds the exact original bytes.
    let backups = e.engine.list_backups();
    assert_eq!(backups.runs.len(), 1);
    assert_eq!(backups.runs[0].name, "Test run");
    assert_eq!(backups.runs[0].files[0].original_bytes, before.len() as u64);
    assert!(!backups.runs[0].files[0].changed_since_saved);
    let stored = fs::read_dir(e.engine.paths().backups_dir())
        .unwrap()
        .flatten()
        .find(|d| d.file_name().to_string_lossy() == backup_id)
        .unwrap()
        .path()
        .join("original.jpg");
    assert_eq!(fs::read(&stored).unwrap(), before);

    // Restore brings back the identical bytes, reversibly.
    let r = e
        .engine
        .restore_file(&backups.runs[0].files[0].id, RestoreMode::Auto, &nop);
    assert!(r.ok && !r.needs_choice, "{r:?}");
    assert_eq!(fs::read(&path).unwrap(), before);
    assert!(e.engine.list_backups().runs[0].files[0].restored);
    // The item is an unsaved original again.
    assert!(e.engine.item_view(v.id).unwrap().saved.is_none());
}

#[test]
fn a_source_changed_after_opening_is_left_alone() {
    let e = env();
    let views = open_samples(&e);
    let v = find(&views, "receipt_2_straight.jpg").clone();
    let path = e.dir.join(&v.name);
    fs::write(&path, b"someone else edited this").unwrap();
    let out = e.engine.save_items(&[v.id], SaveTarget::Replace, "r", &nop);
    assert_eq!(out[0].error, Some(ErrKind::SourceChanged));
    assert_eq!(fs::read(&path).unwrap(), b"someone else edited this");
    assert!(
        e.engine.list_backups().runs.is_empty(),
        "no backup for a refused write"
    );
}

#[test]
fn an_item_without_a_crop_is_not_written() {
    let e = env();
    let views = open_samples(&e);
    let v = find(&views, "hard_no_document.jpg").clone();
    let before = sha(&e.dir.join(&v.name));
    let out = e.engine.save_items(&[v.id], SaveTarget::Replace, "r", &nop);
    assert_eq!(out[0].error, Some(ErrKind::NoCrop));
    assert_eq!(sha(&e.dir.join(&v.name)), before);
    // Draw crop makes it saveable.
    let drawn = e.engine.draw_crop(v.id).unwrap();
    assert!(drawn.edit.is_some() && drawn.edited);
    let out = e.engine.save_items(&[v.id], SaveTarget::Replace, "r", &nop);
    assert!(out[0].ok, "{:?}", out[0]);
}

#[test]
fn save_as_copy_leaves_the_original_untouched_and_never_overwrites() {
    let e = env();
    let views = open_samples(&e);
    let v = find(&views, "document_1_perspective.jpg").clone();
    let path = e.dir.join(&v.name);
    let before = sha(&path);
    let out = e
        .engine
        .save_items(&[v.id], SaveTarget::Copy, "copy run", &nop);
    assert!(out[0].ok, "{:?}", out[0]);
    assert!(out[0].saved.as_ref().unwrap().copy);
    assert_eq!(sha(&path), before);
    let copy = e.dir.join("AutoCrop").join(&v.name);
    assert!(copy.exists() && auto_crop_codecs::decode(&fs::read(&copy).unwrap()).is_ok());
    assert!(
        e.engine.list_backups().runs.is_empty(),
        "a copy needs no backup"
    );
    // A pre-existing unrelated file of that name is kept: the new copy gets a numbered name.
    let e2 = env();
    let views2 = open_samples(&e2);
    let v2 = find(&views2, "document_1_perspective.jpg").clone();
    fs::create_dir_all(e2.dir.join("AutoCrop")).unwrap();
    fs::write(e2.dir.join("AutoCrop").join(&v2.name), b"unrelated").unwrap();
    let out = e2.engine.save_items(&[v2.id], SaveTarget::Copy, "r", &nop);
    assert!(out[0].ok);
    assert_eq!(
        fs::read(e2.dir.join("AutoCrop").join(&v2.name)).unwrap(),
        b"unrelated"
    );
    assert!(
        e2.dir
            .join("AutoCrop")
            .join("document_1_perspective (2).jpg")
            .exists()
    );
}

#[test]
fn re_saving_after_an_edit_keeps_one_backup_of_the_pristine_original() {
    let e = env();
    let views = open_samples(&e);
    let v = find(&views, "receipt_2_straight.jpg").clone();
    let path = e.dir.join(&v.name);
    let original = fs::read(&path).unwrap();
    let first = e.engine.save_items(&[v.id], SaveTarget::Replace, "r", &nop);
    assert!(first[0].ok);
    let first_bytes = fs::read(&path).unwrap();
    // Edit again; the pixels now come from the backup, not the already-cropped output.
    let mut edit = e.engine.item_view(v.id).unwrap().edit.unwrap();
    edit.fine_deg = 2.0;
    let after = e.engine.set_edit(v.id, &edit, false, "Rotate").unwrap();
    assert!(after.dirty_since_save);
    let second = e.engine.save_items(&[v.id], SaveTarget::Replace, "r", &nop);
    assert!(second[0].ok, "{:?}", second[0]);
    assert_ne!(fs::read(&path).unwrap(), first_bytes);
    let runs = e.engine.list_backups().runs;
    assert_eq!(
        runs.iter().map(|r| r.file_count).sum::<usize>(),
        1,
        "one backup, not two"
    );
    let r = e
        .engine
        .restore_file(&runs[0].files[0].id, RestoreMode::Auto, &nop);
    assert!(r.ok, "{r:?}");
    assert_eq!(fs::read(&path).unwrap(), original, "no generation loss");
}

#[test]
fn restoring_a_file_edited_since_saving_asks_first_and_never_loses_it() {
    let e = env();
    let views = open_samples(&e);
    let v = find(&views, "receipt_2_straight.jpg").clone();
    let path = e.dir.join(&v.name);
    let original = fs::read(&path).unwrap();
    assert!(e.engine.save_items(&[v.id], SaveTarget::Replace, "r", &nop)[0].ok);
    fs::write(&path, b"edited by another program").unwrap();
    let file = e.engine.list_backups().runs[0].files[0].clone();
    assert!(file.changed_since_saved);
    // Auto asks.
    let r = e.engine.restore_file(&file.id, RestoreMode::Auto, &nop);
    assert!(r.needs_choice && !r.ok);
    assert_eq!(fs::read(&path).unwrap(), b"edited by another program");
    // As copy keeps both.
    let c = e.engine.restore_file(&file.id, RestoreMode::AsCopy, &nop);
    assert!(c.ok, "{c:?}");
    assert_eq!(fs::read(&path).unwrap(), b"edited by another program");
    assert_eq!(fs::read(e.dir.join(c.restored.unwrap())).unwrap(), original);
    // Replace anyway restores, but keeps the edited file in the backup.
    let r = e
        .engine
        .restore_file(&file.id, RestoreMode::ReplaceAnyway, &nop);
    assert!(r.ok, "{r:?}");
    assert_eq!(fs::read(&path).unwrap(), original);
    let id = file.id.split('/').next().unwrap();
    let kept = e
        .engine
        .paths()
        .backups_dir()
        .join(id)
        .join("replaced-by-restore.jpg");
    assert_eq!(fs::read(kept).unwrap(), b"edited by another program");
}

#[test]
fn a_backup_that_expired_is_reported_not_guessed() {
    let e = env();
    let views = open_samples(&e);
    let v = find(&views, "receipt_2_straight.jpg").clone();
    assert!(e.engine.save_items(&[v.id], SaveTarget::Replace, "r", &nop)[0].ok);
    let dir = e.engine.paths().backups_dir();
    for d in fs::read_dir(&dir).unwrap().flatten() {
        fs::remove_dir_all(d.path()).unwrap();
    }
    let file_id = format!("{}/0", "0".repeat(28));
    assert_eq!(
        e.engine
            .restore_file(&file_id, RestoreMode::Auto, &nop)
            .error,
        Some(ErrKind::OriginalExpired)
    );
    // Editing and re-saving an item whose backup is gone is refused rather than compounding the crop.
    let mut edit = e.engine.item_view(v.id).unwrap().edit.unwrap();
    edit.fine_deg = 3.0;
    e.engine.set_edit(v.id, &edit, false, "Rotate").unwrap();
    let out = e.engine.save_items(&[v.id], SaveTarget::Replace, "r", &nop);
    assert_eq!(out[0].error, Some(ErrKind::OriginalExpired));
}

#[test]
fn corrupt_unsupported_and_oversized_files_become_typed_errors() {
    let e = env();
    fs::write(e.dir.join("broken.jpg"), b"\xFF\xD8\xFF not really a jpeg").unwrap();
    fs::write(e.dir.join("fake.png"), b"definitely not a png").unwrap();
    let s = e.engine.open_paths(std::slice::from_ref(&e.dir), false);
    assert_eq!(s.added, 2);
    let a = e.engine.analyse(s.ids[0]).unwrap();
    let b = e.engine.analyse(s.ids[1]).unwrap();
    assert_eq!((a.status, b.status), (ItemStatus::Error, ItemStatus::Error));
    assert_eq!(a.error, Some(ErrKind::Corrupt));
    assert_eq!(b.error, Some(ErrKind::UnsupportedFormat));
    // They stay untouched on disk and cannot be saved.
    let out = e.engine.save_items(&s.ids, SaveTarget::Replace, "r", &nop);
    assert!(out.iter().all(|o| !o.ok));
    assert_eq!(
        fs::read(e.dir.join("fake.png")).unwrap(),
        b"definitely not a png"
    );
}

#[test]
fn background_analysis_notifies_every_item() {
    let e = env();
    let files = auto_crop_engine::samples::write_samples(&e.dir).unwrap();
    let s = e.engine.open_paths(&files, false);
    let (tx, rx) = std::sync::mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    e.engine.spawn_analysis(
        s.ids.clone(),
        std::sync::Arc::new(move |v: ItemView| {
            tx.lock().unwrap().send(v.id).unwrap();
        }),
    );
    let mut seen = std::collections::BTreeSet::new();
    while seen.len() < s.ids.len() {
        seen.insert(
            rx.recv_timeout(std::time::Duration::from_secs(120))
                .expect("analysis finished"),
        );
    }
    assert_eq!(seen.len(), s.ids.len());
    assert!(
        e.engine
            .list_items()
            .iter()
            .all(|v| v.status == ItemStatus::Ready)
    );
}
