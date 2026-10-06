// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The backup and restore matrix (ROADMAP M2.59, M2.36, M2.37; gate "restore matrix"): for every
//! combination of format, mode (overwrite or convert), backup method, volume, restart and lost
//! index, a save followed by a restore gives back the original byte for byte, with its mtime.
//!
//! Dimensions this machine can exercise are run; the rest are named in `MATRIX_NOT_RUN` at the
//! bottom of the report each test prints, so a gap is visible rather than silently absent.
//! `library.db` does not exist yet (the manifests are the index, PLAN 2.7: "each directory is
//! self-describing"), so "index lost" deletes every file of the store that is not a backup entry.

use auto_crop_codecs::fixtures::{JpegSpec, bmp_rgb24, fake_icc, pattern};
use auto_crop_core::Pt;
use auto_crop_engine::convert::conversion_target;
use auto_crop_engine::group::{Fault, Step, pending_journals, stray_temps};
use auto_crop_engine::store::{BackupMethod, MethodPref, Store};
use auto_crop_engine::{
    AppPaths, DerivedAction, Edit, Engine, EngineOptions, ErrKind, ItemView, NoSplit, RestoreMode,
    SaveTarget,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

fn nop(_: ItemView) {}

fn old_mtime() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_500_000_000)
}

fn set_mtime(p: &Path) {
    fs::File::options()
        .write(true)
        .open(p)
        .unwrap()
        .set_modified(old_mtime())
        .unwrap();
}

fn mtime(p: &Path) -> SystemTime {
    fs::metadata(p).unwrap().modified().unwrap()
}

#[derive(Clone, Copy, Debug)]
enum Fmt {
    Jpeg,
    Png,
}

fn source(fmt: Fmt) -> (&'static str, Vec<u8>) {
    match fmt {
        Fmt::Jpeg => {
            let mut s = JpegSpec::new(160, 128);
            s.sampling = (2, 2);
            s.icc = Some(fake_icc(1200));
            ("a.jpg", s.build())
        }
        Fmt::Png => (
            "a.png",
            auto_crop_codecs::fixtures::png_with_icc(120, 90, &fake_icc(800)),
        ),
    }
}

fn crop(engine: &Engine, id: u32) {
    let edit = Edit {
        quad: [
            Pt::new(0.1, 0.1),
            Pt::new(0.9, 0.1),
            Pt::new(0.9, 0.9),
            Pt::new(0.1, 0.9),
        ],
        quarter_turns: 0,
        fine_deg: 0.3,
    };
    engine.set_edit(id, &edit, false, "crop").unwrap();
}

/// A second volume for the "cross-volume" column, where one exists: `/dev/shm` is a tmpfs on Linux.
fn other_volume() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        let p = PathBuf::from("/dev/shm");
        if p.is_dir() && fs::create_dir_all(p.join(".ac-probe")).is_ok() {
            let _ = fs::remove_dir(p.join(".ac-probe"));
            return Some(p);
        }
    }
    None
}

struct Row {
    fmt: Fmt,
    method: MethodPref,
    restart: bool,
    index_lost: bool,
    cross: bool,
}

/// The files of a store that are not backup entries: the index, markers, journals.
fn lose_the_index(data_dir: &Path) {
    let store = data_dir.join("backups");
    for e in fs::read_dir(&store).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let is_entry = name.len() == 28 && name.bytes().all(|b| b.is_ascii_hexdigit());
        if !is_entry {
            let p = e.path();
            if p.is_dir() {
                let _ = fs::remove_dir_all(&p);
            } else {
                let _ = fs::remove_file(&p);
            }
        }
    }
    let _ = fs::remove_file(data_dir.join("library.db"));
}

fn run_overwrite(row: &Row) -> Option<BackupMethod> {
    let root = tempfile::tempdir().unwrap();
    let shm = row.cross.then(other_volume).flatten();
    if row.cross && shm.is_none() {
        return None; // no second volume here: named in the report
    }
    let data = match &shm {
        Some(s) => s.join(format!(
            "ac-matrix-{}-{}",
            std::process::id(),
            root.path().file_name().unwrap().to_string_lossy()
        )),
        None => root.path().join("data"),
    };
    let paths = AppPaths::new(data.clone(), root.path().join("config"));
    let mut engine = Engine::new(paths.clone());
    engine.set_item_detector(Arc::new(NoSplit));
    engine.set_options(EngineOptions {
        backup_method: row.method,
        ..EngineOptions::default()
    });
    let dir = root.path().join("photos");
    fs::create_dir_all(&dir).unwrap();
    let (name, bytes) = source(row.fmt);
    let path = dir.join(name);
    fs::write(&path, &bytes).unwrap();
    set_mtime(&path);
    let s = engine.open_paths(std::slice::from_ref(&path), false);
    let id = s.ids[0];
    assert!(engine.analyse(id).unwrap().error.is_none());
    crop(&engine, id);
    let out = engine.save_items(&[id], SaveTarget::Replace, "matrix", &nop);
    assert!(out[0].ok, "{:?}: {:?}", row.fmt, out[0]);
    assert_ne!(fs::read(&path).unwrap(), bytes, "the save changed the file");

    // The method the backup was made by, as recorded.
    let manifest = Store::new(paths.backups_dir()).list().remove(0);
    let method = manifest.method.expect("method recorded");
    match row.method {
        MethodPref::Copy => assert_eq!(method, BackupMethod::Copy),
        MethodPref::Hardlink => assert!(matches!(
            method,
            BackupMethod::Hardlink | BackupMethod::Copy
        )),
        MethodPref::Auto => {}
    }
    if row.cross {
        assert_eq!(
            method,
            BackupMethod::Copy,
            "a backup on another volume is a copy"
        );
    }

    if row.index_lost {
        lose_the_index(&data);
    }
    if row.restart {
        engine = Engine::new(paths.clone());
    }
    let r = if row.restart || row.index_lost {
        // After a restart the engine knows no items; restore by path through the manifests.
        engine.restore_path(&path, RestoreMode::Auto, DerivedAction::Keep, &nop)
    } else {
        let f = engine.list_backups().runs[0].files[0].id.clone();
        engine.restore_file(&f, RestoreMode::Auto, &nop)
    };
    assert!(r.ok && !r.needs_choice, "{:?}: {r:?}", row.fmt);
    assert_eq!(
        fs::read(&path).unwrap(),
        bytes,
        "{:?}: restored bytes",
        row.fmt
    );
    assert_eq!(mtime(&path), old_mtime(), "{:?}: restored mtime", row.fmt);
    assert!(stray_temps(&dir).is_empty());
    assert_eq!(pending_journals(&Store::new(paths.backups_dir())), 0);
    // Restoring again is a no-op that still reports success.
    let again = engine.restore_path(&path, RestoreMode::Auto, DerivedAction::Keep, &nop);
    assert!(
        again.ok || again.error == Some(ErrKind::OriginalExpired),
        "{again:?}"
    );
    assert_eq!(fs::read(&path).unwrap(), bytes);
    if let Some(s) = shm {
        let _ = fs::remove_dir_all(s.join(data.file_name().unwrap()));
    }
    Some(method)
}

#[test]
fn overwrite_then_restore_is_byte_identical_across_the_matrix() {
    let mut rows = 0;
    let mut cross_rows = 0;
    let mut methods = std::collections::BTreeMap::new();
    for fmt in [Fmt::Jpeg, Fmt::Png] {
        for method in [MethodPref::Copy, MethodPref::Hardlink, MethodPref::Auto] {
            for restart in [false, true] {
                for index_lost in [false, true] {
                    for cross in [false, true] {
                        let row = Row {
                            fmt,
                            method,
                            restart,
                            index_lost,
                            cross,
                        };
                        if cross && other_volume().is_none() {
                            continue;
                        }
                        if let Some(m) = run_overwrite(&row) {
                            *methods.entry(format!("{m:?}")).or_insert(0) += 1;
                        }
                        rows += 1;
                        cross_rows += usize::from(cross);
                    }
                }
            }
        }
    }
    eprintln!(
        "MATRIX overwrite: {rows} rows run ({cross_rows} cross-volume), backup methods seen \
         {methods:?}; MATRIX_NOT_RUN: {}",
        if other_volume().is_some() {
            "none"
        } else {
            "cross-volume (no second volume on this machine)"
        }
    );
    assert!(rows >= 24);
}

/// Formats with no writer are never overwritten: the "overwrite" row of the matrix is that the
/// bytes and the mtime stay exactly as they were, and no backup is made.
#[test]
fn formats_without_a_writer_stay_byte_identical_under_overwrite() {
    let root = tempfile::tempdir().unwrap();
    let engine = Engine::new(AppPaths::under(root.path()));
    engine.set_item_detector(Arc::new(NoSplit));
    let dir = root.path().join("photos");
    fs::create_dir_all(&dir).unwrap();
    let files: Vec<(&str, Vec<u8>)> = vec![
        ("a.webp", auto_crop_codecs::fixtures::webp_lossless(64, 48)),
        (
            "a.tif",
            auto_crop_codecs::fixtures::tiff_rgb8(64, 48, &Default::default()),
        ),
    ];
    for (name, bytes) in files {
        let p = dir.join(name);
        fs::write(&p, &bytes).unwrap();
        set_mtime(&p);
        let s = engine.open_paths(std::slice::from_ref(&p), false);
        let id = s.ids[0];
        engine.analyse(id).unwrap();
        crop(&engine, id);
        let out = engine.save_items(&[id], SaveTarget::Replace, "r", &nop);
        assert_eq!(out[0].error, Some(ErrKind::NotReplaceable), "{name}");
        assert_eq!(fs::read(&p).unwrap(), bytes, "{name}");
        assert_eq!(mtime(&p), old_mtime(), "{name}");
    }
    assert!(engine.list_backups().runs.is_empty());
}

// ------------------------------------------------------------------ conversion

fn bmp_env() -> (
    tempfile::TempDir,
    AppPaths,
    Engine,
    PathBuf,
    Vec<u8>,
    Vec<u8>,
) {
    let root = tempfile::tempdir().unwrap();
    let paths = AppPaths::under(root.path());
    let engine = Engine::new(paths.clone());
    engine.set_item_detector(Arc::new(NoSplit));
    let dir = root.path().join("photos");
    fs::create_dir_all(&dir).unwrap();
    let rgb = pattern(37, 21);
    let bmp = bmp_rgb24(37, 21, &rgb, false);
    let path = dir.join("scan.bmp");
    fs::write(&path, &bmp).unwrap();
    set_mtime(&path);
    (root, paths, engine, path, bmp, rgb)
}

#[test]
fn a_bmp_is_converted_to_png_the_source_is_backed_up_and_restore_returns_it() {
    assert_eq!(
        conversion_target(auto_crop_codecs::Format::Bmp, 1),
        Some(auto_crop_codecs::Format::Png)
    );
    for restart in [false, true] {
        for method in [MethodPref::Copy, MethodPref::Hardlink, MethodPref::Auto] {
            let (_root, paths, mut engine, path, bmp, rgb) = bmp_env();
            engine.set_options(EngineOptions {
                backup_method: method,
                ..EngineOptions::default()
            });
            let s = engine.open_paths(std::slice::from_ref(&path), false);
            let id = s.ids[0];
            let v = engine.analyse(id).unwrap();
            assert!(v.error.is_none(), "{:?}", v.error);
            let out = engine.convert_items(&[id], "convert run", &nop);
            assert!(out[0].ok, "{:?}", out[0]);
            let png = path.with_extension("png");
            // The source path is gone; the PNG holds exactly the BMP's pixels.
            assert!(!path.exists() && png.exists());
            let d = auto_crop_codecs::decode(&fs::read(&png).unwrap()).unwrap();
            assert_eq!(d.raster.data, rgb);
            assert_eq!(mtime(&png), old_mtime(), "the mtime is kept");
            assert!(stray_temps(path.parent().unwrap()).is_empty());
            assert_eq!(pending_journals(&Store::new(paths.backups_dir())), 0);
            // The backup holds the BMP, byte for byte.
            let m = Store::new(paths.backups_dir()).list().remove(0);
            assert_eq!(
                fs::read(Store::new(paths.backups_dir()).original_path(&m).unwrap()).unwrap(),
                bmp
            );
            if restart {
                engine = Engine::new(paths.clone());
            }
            // Restore: the source returns to its path; the converted file moves into the backup.
            let r = engine.restore_path(&png, RestoreMode::Auto, DerivedAction::Remove, &nop);
            assert!(r.ok, "{r:?}");
            assert_eq!(fs::read(&path).unwrap(), bmp);
            assert_eq!(mtime(&path), old_mtime());
            assert!(!png.exists(), "the converted file was moved into the store");
            let m = Store::new(paths.backups_dir()).list().remove(0);
            assert_eq!(m.derived_moved.len(), 1);
        }
    }
}

#[test]
fn a_taken_target_name_gets_a_number_and_nothing_is_overwritten() {
    let (_root, _paths, engine, path, bmp, _rgb) = bmp_env();
    let other = path.with_extension("png");
    fs::write(&other, b"an unrelated file called scan.png").unwrap();
    let s = engine.open_paths(std::slice::from_ref(&path), false);
    engine.analyse(s.ids[0]).unwrap();
    let out = engine.convert_items(&[s.ids[0]], "r", &nop);
    assert!(out[0].ok, "{:?}", out[0]);
    assert_eq!(
        fs::read(&other).unwrap(),
        b"an unrelated file called scan.png"
    );
    let numbered = path.with_file_name("scan (2).png");
    assert!(numbered.exists() && !path.exists());
    let _ = bmp;
}

#[test]
fn jpeg_png_and_sources_without_a_conversion_are_not_converted() {
    let root = tempfile::tempdir().unwrap();
    let engine = Engine::new(AppPaths::under(root.path()));
    engine.set_item_detector(Arc::new(NoSplit));
    let dir = root.path().join("photos");
    fs::create_dir_all(&dir).unwrap();
    for (name, bytes) in [
        ("a.jpg", JpegSpec::new(32, 32).build()),
        ("a.webp", auto_crop_codecs::fixtures::webp_lossless(32, 32)),
    ] {
        let p = dir.join(name);
        fs::write(&p, &bytes).unwrap();
        let s = engine.open_paths(std::slice::from_ref(&p), false);
        engine.analyse(s.ids[0]).unwrap();
        let out = engine.convert_items(&[s.ids[0]], "r", &nop);
        assert!(!out[0].ok, "{name}");
        assert_eq!(fs::read(&p).unwrap(), bytes, "{name}");
    }
    assert!(engine.list_backups().runs.is_empty());
}

/// A crash at any step of a conversion leaves the scan alone, or the PNG (with the BMP beside it
/// when it could not be removed), and the original is always restorable.
#[test]
fn a_crash_at_every_step_of_a_conversion_is_recovered() {
    let steps = [
        Step::JournalWriting,
        Step::BackupCreated,
        Step::TempWritten(0),
        Step::TempVerified(0),
        Step::JournalCommitting,
        Step::BeforeRename(0),
        Step::AfterRename(0),
        Step::BeforeUnlink,
        Step::AfterUnlink,
        Step::ManifestSaved,
    ];
    for step in steps {
        let (_root, paths, engine, path, bmp, _rgb) = bmp_env();
        let s = engine.open_paths(std::slice::from_ref(&path), false);
        engine.analyse(s.ids[0]).unwrap();
        let out = engine.convert_items_with_faults(
            &s.ids,
            "r",
            &move |st: &Step| {
                if *st == step {
                    Fault::Crash
                } else {
                    Fault::Pass
                }
            },
            &nop,
        );
        assert!(!out[0].ok, "{step:?}");
        let engine = Engine::new(paths.clone()); // the next start recovers
        let dir = path.parent().unwrap();
        let png = path.with_extension("png");
        let committed = matches!(
            step,
            Step::JournalCommitting
                | Step::BeforeRename(_)
                | Step::AfterRename(_)
                | Step::BeforeUnlink
                | Step::AfterUnlink
                | Step::ManifestSaved
        );
        if committed {
            assert!(png.exists(), "{step:?}: rolled forward, the PNG is there");
            assert!(auto_crop_codecs::decode(&fs::read(&png).unwrap()).is_ok());
            // The scan is gone (unlinked) or still there beside it (it could not be removed):
            // either way its bytes are intact.
            if path.exists() {
                assert_eq!(fs::read(&path).unwrap(), bmp, "{step:?}");
            }
        } else {
            assert!(!png.exists(), "{step:?}: rolled back, no PNG");
            assert_eq!(
                fs::read(&path).unwrap(),
                bmp,
                "{step:?}: the scan is untouched"
            );
        }
        assert!(stray_temps(dir).is_empty(), "{step:?}");
        assert_eq!(
            pending_journals(&Store::new(paths.backups_dir())),
            0,
            "{step:?}"
        );
        if committed {
            let r = engine.restore_path(&png, RestoreMode::Auto, DerivedAction::Remove, &nop);
            assert!(r.ok, "{step:?}: {r:?}");
            assert_eq!(fs::read(&path).unwrap(), bmp, "{step:?}: restored");
        }
    }
}

#[test]
fn the_no_op_rule_skips_a_crop_of_the_whole_frame_and_nothing_else() {
    let root = tempfile::tempdir().unwrap();
    let engine = Engine::new(AppPaths::under(root.path()));
    engine.set_item_detector(Arc::new(NoSplit));
    let dir = root.path().join("photos");
    fs::create_dir_all(&dir).unwrap();
    let (name, bytes) = source(Fmt::Jpeg);
    let p = dir.join(name);
    fs::write(&p, &bytes).unwrap();
    let s = engine.open_paths(&[p], false);
    let id = s.ids[0];
    engine.analyse(id).unwrap();
    let set = |x0: f64, y0: f64, x1: f64, y1: f64, deg: f32| {
        let edit = Edit {
            quad: [
                Pt::new(x0, y0),
                Pt::new(x1, y0),
                Pt::new(x1, y1),
                Pt::new(x0, y1),
            ],
            quarter_turns: 0,
            fine_deg: deg,
        };
        engine.set_edit(id, &edit, false, "x").unwrap();
    };
    set(0.0, 0.0, 1.0, 1.0, 0.0);
    assert!(engine.is_noop(id), "the whole frame");
    set(0.005, 0.005, 0.995, 0.995, 0.05);
    assert!(engine.is_noop(id), "98% of the frame area, 0.05 degrees");
    set(0.1, 0.1, 0.9, 0.9, 0.0);
    assert!(!engine.is_noop(id), "64% of the frame");
    set(0.0, 0.0, 1.0, 1.0, 0.3);
    assert!(!engine.is_noop(id), "skewed by 0.3 degrees");
}

#[test]
fn a_file_already_written_by_a_save_is_recognised_by_its_hash() {
    let root = tempfile::tempdir().unwrap();
    let engine = Engine::new(AppPaths::under(root.path()));
    engine.set_item_detector(Arc::new(NoSplit));
    let dir = root.path().join("photos");
    fs::create_dir_all(&dir).unwrap();
    let (name, bytes) = source(Fmt::Jpeg);
    let p = dir.join(name);
    fs::write(&p, &bytes).unwrap();
    assert!(engine.processed_by(&p).is_none());
    let s = engine.open_paths(std::slice::from_ref(&p), false);
    engine.analyse(s.ids[0]).unwrap();
    crop(&engine, s.ids[0]);
    assert!(engine.save_items(&s.ids, SaveTarget::Replace, "run", &nop)[0].ok);
    // The output is known; the original (restored) no longer is.
    let info = engine
        .processed_by(&p)
        .expect("a file we wrote is recognised");
    assert_eq!(info.output_count, 1);
    assert!(!info.restored);
}
