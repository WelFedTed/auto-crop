// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The one-file save through the engine (ROADMAP M2.83, M2.84, M2.22 to M2.25, M2.29): the codecs
//! crate does the encoding, the EXIF Orientation is written once, metadata and ICC follow the
//! policy, the lossless JPEG path is used when it is exact, the free-space preflight and the file
//! state checks refuse before anything is written, and a crash during a save is finished or undone
//! at the next start.

use auto_crop_codecs::fixtures::{JpegSpec, fake_icc, jpeg_insert_segment};
use auto_crop_codecs::jpeg_meta;
use auto_crop_core::Pt;
use auto_crop_engine::group::{Fault, Step};
use auto_crop_engine::util::blake3_hex;
use auto_crop_engine::{
    AppPaths, Edit, Engine, EngineOptions, ErrKind, ItemView, NoSplit, SaveTarget,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

struct Env {
    _root: tempfile::TempDir,
    paths: AppPaths,
    engine: Engine,
    dir: PathBuf,
}

fn env() -> Env {
    let root = tempfile::tempdir().unwrap();
    let paths = AppPaths::under(root.path());
    let engine = Engine::new(paths.clone());
    engine.set_item_detector(Arc::new(NoSplit));
    let dir = root.path().join("photos");
    fs::create_dir_all(&dir).unwrap();
    Env {
        _root: root,
        paths,
        engine,
        dir,
    }
}

fn nop(_: ItemView) {}

const GPS_MARK: [u8; 8] = [0xAB, 0xCD, 0xAB, 0xCD, 0xAB, 0xCD, 0xAB, 0xCD];

/// A TIFF blob with Orientation `o`, a GPS IFD (one value holding `GPS_MARK`) and a tiny
/// thumbnail.
fn exif_with_gps(o: u16) -> Vec<u8> {
    let mut v: Vec<u8> = Vec::new();
    let (p16, p32) = (
        |v: &mut Vec<u8>, x: u16| v.extend_from_slice(&x.to_le_bytes()),
        |v: &mut Vec<u8>, x: u32| v.extend_from_slice(&x.to_le_bytes()),
    );
    v.extend_from_slice(b"II");
    p16(&mut v, 42);
    p32(&mut v, 8);
    // IFD0: Orientation, GPS pointer; next = 0. Table = 2 + 24 + 4 = 30, GPS IFD at 38.
    p16(&mut v, 2);
    p16(&mut v, 0x0112);
    p16(&mut v, 3);
    p32(&mut v, 1);
    p16(&mut v, o);
    p16(&mut v, 0);
    p16(&mut v, 0x8825);
    p16(&mut v, 4);
    p32(&mut v, 1);
    p32(&mut v, 38);
    p32(&mut v, 0);
    // GPS IFD: one RATIONAL x1 (8 bytes) out of line at 38 + 2 + 12 + 4 = 56.
    p16(&mut v, 1);
    p16(&mut v, 0x0002);
    p16(&mut v, 5);
    p32(&mut v, 1);
    p32(&mut v, 56);
    p32(&mut v, 0);
    v.extend_from_slice(&GPS_MARK);
    v
}

/// A 4:2:0 JPEG of `w` x `h` (a gradient) with an ICC profile and an EXIF segment.
fn camera_jpeg(w: u32, h: u32, orientation: u16) -> Vec<u8> {
    let mut spec = JpegSpec::new(w, h);
    spec.sampling = (2, 2);
    spec.icc = Some(fake_icc(1500));
    let j = spec.build();
    let mut e = b"Exif\0\0".to_vec();
    e.extend_from_slice(&exif_with_gps(orientation));
    jpeg_insert_segment(&j, 0xE1, &e)
}

fn open(e: &Env, name: &str, bytes: &[u8]) -> ItemView {
    let path = e.dir.join(name);
    fs::write(&path, bytes).unwrap();
    let s = e.engine.open_paths(&[path], false);
    let id = s.ids[0];
    let v = e.engine.analyse(id).unwrap();
    assert!(v.error.is_none(), "{name}: {:?}", v.error);
    v
}

fn crop(e: &Env, v: &ItemView, r: [f64; 4], turns: u8, deg: f32) -> ItemView {
    let [x0, y0, x1, y1] = r;
    let edit = Edit {
        quad: [
            Pt::new(x0, y0),
            Pt::new(x1, y0),
            Pt::new(x1, y1),
            Pt::new(x0, y1),
        ],
        quarter_turns: turns,
        fine_deg: deg,
    };
    e.engine.set_edit(v.id, &edit, false, "test crop").unwrap()
}

fn has(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fn strays(dir: &Path) -> usize {
    fs::read_dir(dir)
        .unwrap()
        .flatten()
        .filter(|f| f.file_name().to_string_lossy().starts_with(".autocrop-"))
        .count()
}

fn dqt_bytes(jpeg: &[u8]) -> Vec<Vec<u8>> {
    jpeg_meta::header_segments(jpeg)
        .unwrap()
        .iter()
        .filter(|s| s.marker == 0xDB)
        .map(|s| jpeg[s.payload.clone()].to_vec())
        .collect()
}

#[test]
fn a_mcu_aligned_crop_takes_the_lossless_path_and_keeps_the_quantisation_tables() {
    let e = env();
    let src = camera_jpeg(160, 128, 1);
    let v = open(&e, "a.jpg", &src);
    // 32..128 x 16..96 of 160 x 128: on the 16 px grid.
    let v = crop(&e, &v, [0.2, 0.125, 0.8, 0.75], 0, 0.0);
    let out = e.engine.save_items(&[v.id], SaveTarget::Replace, "r", &nop);
    assert!(out[0].ok, "{:?}", out[0]);
    assert!(
        out[0].notices.contains(&"jpeg.lossless".to_owned()),
        "{:?}",
        out[0]
    );
    let saved = fs::read(e.dir.join("a.jpg")).unwrap();
    let d = auto_crop_codecs::decode(&saved).unwrap();
    assert_eq!((d.raster.width, d.raster.height), (96, 80));
    // No re-quantisation: the output has the source's tables, byte for byte.
    assert_eq!(dqt_bytes(&saved), dqt_bytes(&src));
    // The ICC profile is the source's, byte-exact, and the orientation tag says upright.
    assert_eq!(d.icc.as_deref(), Some(fake_icc(1500).as_slice()));
    assert_eq!(auto_crop_codecs::probe(&saved).unwrap().orientation, 1);
    // The backup holds the original, the journal and temps are gone.
    let m = &e.engine.list_backups().runs[0].files[0];
    assert_eq!(m.original_bytes, src.len() as u64);
    assert_eq!(strays(&e.dir), 0);
    assert_eq!(
        auto_crop_engine::group::pending_journals(&auto_crop_engine::store::Store::new(
            e.paths.backups_dir()
        )),
        0
    );
}

#[test]
fn a_skewed_crop_is_re_encoded_and_the_quality_follows_the_source() {
    let e = env();
    let src = camera_jpeg(160, 128, 1);
    let v = open(&e, "a.jpg", &src);
    let v = crop(&e, &v, [0.2, 0.125, 0.8, 0.75], 0, 1.5);
    let out = e.engine.save_items(&[v.id], SaveTarget::Replace, "r", &nop);
    assert!(out[0].ok, "{:?}", out[0]);
    assert!(!out[0].notices.contains(&"jpeg.lossless".to_owned()));
    let saved = fs::read(e.dir.join("a.jpg")).unwrap();
    // The fixture is q92 (IJG tables): Balanced is q_est + 5 clamped to 95.
    let q = jpeg_meta::read_meta(&saved).unwrap().quality.unwrap();
    assert!(q.ijg && (i32::from(q.q) - 95).abs() <= 3, "{q:?}");
    assert_ne!(dqt_bytes(&saved), dqt_bytes(&src));
}

/// Orientation once, on both paths: a source stored sideways (EXIF 6) comes out upright with the
/// tag reset, and processing the output again does not turn it a second time.
#[test]
fn the_orientation_is_applied_once_on_both_paths_and_not_again_on_reprocessing() {
    for lossless in [true, false] {
        let e = env();
        e.engine.set_options(EngineOptions {
            lossless_jpeg: lossless,
            ..EngineOptions::default()
        });
        // Stored 128 x 96, displayed 96 x 128 (a quarter turn).
        let src = camera_jpeg(128, 96, 6);
        let v = open(&e, "a.jpg", &src);
        assert_eq!((v.width, v.height), (96, 128));
        let truth = auto_crop_codecs::decode(&src).unwrap().raster; // upright, oriented once
        let v = crop(&e, &v, [0.0, 0.0, 1.0, 1.0], 0, 0.0);
        let out = e.engine.save_items(&[v.id], SaveTarget::Replace, "r", &nop);
        assert!(out[0].ok, "lossless={lossless}: {:?}", out[0]);
        let saved = fs::read(e.dir.join("a.jpg")).unwrap();
        assert_eq!(
            auto_crop_codecs::probe(&saved).unwrap().orientation,
            1,
            "lossless={lossless}: the tag must be reset"
        );
        let got = auto_crop_codecs::decode(&saved).unwrap().raster;
        assert_eq!((got.width, got.height), (96, 128), "lossless={lossless}");
        // Upright: the same picture as the oriented decode (interior; JPEG noise allowed).
        for y in (4..124).step_by(5) {
            for x in (4..92).step_by(5) {
                let (a, b) = (truth.pixel(x, y), got.pixel(x, y));
                let d: i32 = (0..3)
                    .map(|c| (i32::from(a[c]) - i32::from(b[c])).abs())
                    .sum();
                assert!(d <= 40, "lossless={lossless} ({x},{y}): {a:?} vs {b:?}");
            }
        }
        // Reprocess the output as a new file: it is already upright, so nothing turns again.
        let copy = e.dir.join("again.jpg");
        fs::copy(e.dir.join("a.jpg"), &copy).unwrap();
        let s = e.engine.open_paths(&[copy], false);
        let v2 = e.engine.analyse(s.ids[0]).unwrap();
        assert_eq!((v2.width, v2.height), (96, 128), "lossless={lossless}");
    }
}

#[test]
fn exif_is_carried_and_strip_location_leaves_no_gps_byte() {
    let src = camera_jpeg(160, 128, 1);
    for strip in [false, true] {
        let e = env();
        e.engine.set_options(EngineOptions {
            strip_location: strip,
            ..EngineOptions::default()
        });
        let v = open(&e, "a.jpg", &src);
        let v = crop(&e, &v, [0.1, 0.1, 0.9, 0.9], 0, 0.4); // re-encoded
        let out = e.engine.save_items(&[v.id], SaveTarget::Replace, "r", &nop);
        assert!(out[0].ok, "{:?}", out[0]);
        let saved = fs::read(e.dir.join("a.jpg")).unwrap();
        assert_eq!(has(&saved, &GPS_MARK), !strip, "strip_location={strip}");
        let m = jpeg_meta::read_meta(&saved).unwrap();
        let exif = m.exif.expect("EXIF carried");
        assert_eq!(auto_crop_codecs::exif_orientation(&exif), Some(1));
        // The source is untouched in the backup, GPS and all.
        let backup = &e.engine.list_backups().runs[0].files[0];
        assert_eq!(backup.original_bytes, src.len() as u64);
    }
}

#[test]
fn png_keeps_its_profile_and_is_written_by_the_png_encoder() {
    let e = env();
    let raster = auto_crop_imgproc::Raster::filled(80, 60, [30, 120, 200]);
    let icc = fake_icc(900);
    let src = auto_crop_codecs::fixtures::png_with_icc(80, 60, &icc);
    let _ = raster;
    let v = open(&e, "a.png", &src);
    let v = crop(&e, &v, [0.1, 0.1, 0.9, 0.9], 0, 0.0);
    let out = e.engine.save_items(&[v.id], SaveTarget::Replace, "r", &nop);
    assert!(out[0].ok, "{:?}", out[0]);
    let saved = fs::read(e.dir.join("a.png")).unwrap();
    let d = auto_crop_codecs::decode(&saved).unwrap();
    assert_eq!((d.raster.width, d.raster.height), (64, 48));
    assert_eq!(d.icc.as_deref(), Some(icc.as_slice()));
}

#[test]
fn a_full_disk_refuses_before_anything_is_written() {
    let e = env();
    let src = camera_jpeg(160, 128, 1);
    let v = open(&e, "a.jpg", &src);
    let v = crop(&e, &v, [0.1, 0.1, 0.9, 0.9], 0, 0.4);
    let out = auto_crop_engine::space::with_free_space(1000, || {
        e.engine.save_items(&[v.id], SaveTarget::Replace, "r", &nop)
    });
    assert_eq!(out[0].error, Some(ErrKind::DiskFull));
    assert_eq!(fs::read(e.dir.join("a.jpg")).unwrap(), src);
    assert!(e.engine.list_backups().runs.is_empty());
    assert_eq!(strays(&e.dir), 0);
    assert!(
        !e.paths.backups_dir().join(".groups").exists()
            || fs::read_dir(e.paths.backups_dir().join(".groups"))
                .unwrap()
                .count()
                == 0
    );
    // Copies are checked too.
    let out = auto_crop_engine::space::with_free_space(1000, || {
        e.engine.save_items(&[v.id], SaveTarget::Copy, "r", &nop)
    });
    assert_eq!(out[0].error, Some(ErrKind::DiskFull));
    assert!(!e.dir.join("AutoCrop").exists());
    // With room it goes through.
    let out = e.engine.save_items(&[v.id], SaveTarget::Replace, "r", &nop);
    assert!(out[0].ok, "{:?}", out[0]);
}

#[test]
fn a_read_only_file_is_refused_with_a_code_and_left_alone() {
    let e = env();
    let src = camera_jpeg(160, 128, 1);
    let v = open(&e, "a.jpg", &src);
    let v = crop(&e, &v, [0.1, 0.1, 0.9, 0.9], 0, 0.4);
    let path = e.dir.join("a.jpg");
    let mut perm = fs::metadata(&path).unwrap().permissions();
    perm.set_readonly(true);
    fs::set_permissions(&path, perm.clone()).unwrap();
    let out = e.engine.save_items(&[v.id], SaveTarget::Replace, "r", &nop);
    assert_eq!(out[0].error, Some(ErrKind::ReadOnly), "{:?}", out[0]);
    assert_eq!(fs::read(&path).unwrap(), src);
    assert!(e.engine.list_backups().runs.is_empty());
    assert_eq!(strays(&e.dir), 0);
    // A copy of a read-only file is fine: it reads, it does not replace.
    let out = e.engine.save_items(&[v.id], SaveTarget::Copy, "r", &nop);
    assert!(out[0].ok, "{:?}", out[0]);
    #[allow(clippy::permissions_set_readonly_false)]
    perm.set_readonly(false);
    fs::set_permissions(&path, perm).unwrap();
}

#[cfg(windows)]
#[test]
fn a_cloud_placeholder_is_skipped_and_never_read() {
    let e = env();
    let src = camera_jpeg(160, 128, 1);
    let path = e.dir.join("online-only.jpg");
    fs::write(&path, &src).unwrap();
    // The OFFLINE attribute (what a placeholder has among its flags) is settable by any user.
    let set = std::process::Command::new("attrib")
        .args(["+O"])
        .arg(&path)
        .status();
    if !matches!(set, Ok(s) if s.success()) {
        eprintln!("attrib +O is not available here; the unit tests of fsstate cover the mask");
        return;
    }
    use std::os::windows::fs::MetadataExt;
    if fs::metadata(&path).unwrap().file_attributes() & 0x1000 == 0 {
        eprintln!("the offline attribute did not stick; skipping");
        return;
    }
    let s = e.engine.open_paths(std::slice::from_ref(&path), false);
    // Opened (the user chose it) but analysis refuses without reading a byte.
    let v = e.engine.analyse(s.ids[0]).unwrap();
    assert_eq!(v.error, Some(ErrKind::CloudNotLocal));
    // With the opt-in it is read like any file.
    e.engine.set_options(EngineOptions {
        hydrate_cloud_files: true,
        ..EngineOptions::default()
    });
    let v = e.engine.analyse(s.ids[0]).unwrap();
    assert!(v.error.is_none(), "{:?}", v.error);
}

/// A crash at any step of an engine save is finished or undone by the next engine that starts on
/// the same data.
#[test]
fn a_crash_during_a_save_is_recovered_when_the_next_engine_starts() {
    let steps = [
        Step::JournalWriting,
        Step::BackupCreated,
        Step::TempWritten(0),
        Step::TempVerified(0),
        Step::JournalCommitting,
        Step::BeforeRename(0),
        Step::AfterRename(0),
        Step::ManifestSaved,
    ];
    for step in steps {
        let e = env();
        let src = camera_jpeg(160, 128, 1);
        let v = open(&e, "a.jpg", &src);
        let v = crop(&e, &v, [0.1, 0.1, 0.9, 0.9], 0, 0.4);
        let out =
            e.engine
                .save_items_with_faults(&[v.id], SaveTarget::Replace, "r", &move |s: &Step| {
                    if *s == step {
                        Fault::Crash
                    } else {
                        Fault::Pass
                    }
                });
        assert!(!out[0].ok, "{step:?}");
        // "Restart": a new engine on the same data folders recovers.
        let again = Engine::new(e.paths.clone());
        let path = e.dir.join("a.jpg");
        let now = fs::read(&path).unwrap();
        let committed = matches!(
            step,
            Step::JournalCommitting
                | Step::BeforeRename(_)
                | Step::AfterRename(_)
                | Step::ManifestSaved
        );
        assert_eq!(now == src, !committed, "{step:?}: the file after recovery");
        if committed {
            assert!(auto_crop_codecs::decode(&now).is_ok(), "{step:?}");
        }
        assert_eq!(strays(&e.dir), 0, "{step:?}");
        // The original is restorable in both cases, or never left the folder.
        let runs = again.list_backups().runs;
        if committed {
            let r = again.restore_file(
                &runs[0].files[0].id,
                auto_crop_engine::RestoreMode::Auto,
                &nop,
            );
            assert!(r.ok, "{step:?}: {r:?}");
        } else {
            assert!(runs.is_empty(), "{step:?}: an unused backup was left");
        }
        assert_eq!(fs::read(&path).unwrap(), src, "{step:?}");
    }
}

#[test]
fn a_source_edited_after_opening_is_never_replaced_and_its_backup_is_not_made() {
    let e = env();
    let src = camera_jpeg(160, 128, 1);
    let v = open(&e, "a.jpg", &src);
    let v = crop(&e, &v, [0.1, 0.1, 0.9, 0.9], 0, 0.4);
    fs::write(e.dir.join("a.jpg"), b"someone else's edit").unwrap();
    let out = e.engine.save_items(&[v.id], SaveTarget::Replace, "r", &nop);
    assert_eq!(out[0].error, Some(ErrKind::SourceChanged));
    assert_eq!(
        fs::read(e.dir.join("a.jpg")).unwrap(),
        b"someone else's edit"
    );
    assert!(e.engine.list_backups().runs.is_empty());
}

#[test]
fn a_re_save_replaces_the_previous_output_and_keeps_one_backup() {
    let e = env();
    let src = camera_jpeg(160, 128, 1);
    let v = open(&e, "a.jpg", &src);
    let v = crop(&e, &v, [0.1, 0.1, 0.9, 0.9], 0, 0.4);
    assert!(e.engine.save_items(&[v.id], SaveTarget::Replace, "r", &nop)[0].ok);
    let first = fs::read(e.dir.join("a.jpg")).unwrap();
    let v = crop(&e, &v, [0.2, 0.2, 0.8, 0.8], 0, 0.4);
    let out = e.engine.save_items(&[v.id], SaveTarget::Replace, "r", &nop);
    assert!(out[0].ok, "{:?}", out[0]);
    let second = fs::read(e.dir.join("a.jpg")).unwrap();
    assert_ne!(first, second);
    let runs = e.engine.list_backups().runs;
    assert_eq!(runs.iter().map(|r| r.file_count).sum::<usize>(), 1);
    // The superseded first output is kept in the store, not lost.
    let id = out[0].saved.as_ref().unwrap().backup_id.clone().unwrap();
    let kept: Vec<Vec<u8>> = fs::read_dir(e.paths.backups_dir().join(&id).join("superseded"))
        .unwrap()
        .flatten()
        .map(|f| fs::read(f.path()).unwrap())
        .collect();
    assert!(kept.contains(&first));
    // Restore returns the very first bytes.
    let r = e.engine.restore_file(
        &runs[0].files[0].id,
        auto_crop_engine::RestoreMode::Auto,
        &nop,
    );
    assert!(r.ok, "{r:?}");
    assert_eq!(
        blake3_hex(&fs::read(e.dir.join("a.jpg")).unwrap()),
        blake3_hex(&src)
    );
}

#[test]
fn a_300_character_path_round_trips() {
    let e = env();
    let mut deep = e.dir.clone();
    while deep.as_os_str().len() < 300 {
        deep.push("a_folder_with_a_rather_long_name");
    }
    fs::create_dir_all(&deep).unwrap();
    let src = camera_jpeg(160, 128, 1);
    let path = deep.join("photo with spaces and ünïcödé.jpg");
    fs::write(&path, &src).unwrap();
    let s = e.engine.open_paths(std::slice::from_ref(&path), false);
    let v = e.engine.analyse(s.ids[0]).unwrap();
    assert!(v.error.is_none(), "{:?}", v.error);
    let v = crop(&e, &v, [0.1, 0.1, 0.9, 0.9], 0, 0.4);
    let out = e.engine.save_items(&[v.id], SaveTarget::Replace, "r", &nop);
    assert!(out[0].ok, "{:?}", out[0]);
    assert_ne!(fs::read(&path).unwrap(), src);
    let runs = e.engine.list_backups().runs;
    let r = e.engine.restore_file(
        &runs[0].files[0].id,
        auto_crop_engine::RestoreMode::Auto,
        &nop,
    );
    assert!(r.ok, "{r:?}");
    assert_eq!(fs::read(&path).unwrap(), src);
}

#[cfg(unix)]
#[test]
fn a_symlinked_file_is_replaced_at_its_target_and_the_link_stays_valid() {
    let e = env();
    let src = camera_jpeg(160, 128, 1);
    let real_dir = e.dir.join("real");
    fs::create_dir_all(&real_dir).unwrap();
    let real = real_dir.join("a.jpg");
    fs::write(&real, &src).unwrap();
    let link = e.dir.join("link.jpg");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    // The engine does not follow links when it scans, so the real path is what gets opened; the
    // swap itself (tested in `commit`) keeps a link valid.
    let s = e.engine.open_paths(std::slice::from_ref(&real), false);
    let v = e.engine.analyse(s.ids[0]).unwrap();
    let v = crop(&e, &v, [0.1, 0.1, 0.9, 0.9], 0, 0.4);
    assert!(e.engine.save_items(&[v.id], SaveTarget::Replace, "r", &nop)[0].ok);
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read(&link).unwrap(), fs::read(&real).unwrap());
}
