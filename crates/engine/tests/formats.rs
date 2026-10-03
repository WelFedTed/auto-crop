// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Formats the engine can open but not write (PLAN 3.2.3): WebP, TIFF and, with the `heif` feature,
//! AVIF and HEIC. Their sources are never replaced in place (no writer exists), the source stays
//! byte-identical, and "save as copy" writes a PNG (JPEG for HEIC) beside it.

use auto_crop_codecs::fixtures as fx;
use auto_crop_core::Pt;
use auto_crop_engine::{
    AppPaths, Edit, Engine, ErrKind, ItemStatus, ItemView, OpenSummary, SaveTarget,
};
use std::fs;
use std::path::PathBuf;

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

fn nop(_: ItemView) {}

/// Writes `bytes` as `name`, opens and analyses it, and gives it a crop that keeps the middle.
fn open(e: &Env, name: &str, bytes: &[u8]) -> (OpenSummary, Option<ItemView>) {
    let path = e.dir.join(name);
    fs::write(&path, bytes).unwrap();
    let summary = e.engine.open_paths(&[path], false);
    let Some(id) = summary.ids.first().copied() else {
        return (summary, None);
    };
    let v = e.engine.analyse(id).unwrap();
    assert_eq!(v.status, ItemStatus::Ready, "{name}: {:?}", v.error);
    let edit = Edit {
        quad: [
            Pt::new(0.1, 0.1),
            Pt::new(0.9, 0.1),
            Pt::new(0.9, 0.9),
            Pt::new(0.1, 0.9),
        ],
        quarter_turns: 0,
        fine_deg: 0.0,
    };
    let v = e.engine.set_edit(id, &edit, false, "test crop").unwrap();
    (summary, Some(v))
}

/// A source with no writer: replacing is refused, the bytes stay, and a copy is a PNG or JPEG.
fn check_not_replaceable(e: &Env, name: &str, bytes: &[u8], copy_ext: &str) {
    let (summary, v) = open(e, name, bytes);
    assert_eq!(summary.added, 1, "{name}");
    let v = v.unwrap();
    let path = e.dir.join(name);

    let out = e
        .engine
        .save_items(&[v.id], SaveTarget::Replace, "replace run", &nop);
    assert!(
        !out[0].ok,
        "{name}: a replace of a format without a writer succeeded"
    );
    assert_eq!(out[0].error, Some(ErrKind::UnsupportedOutput), "{name}");
    assert_eq!(
        fs::read(&path).unwrap(),
        bytes,
        "{name}: the source changed"
    );
    assert!(
        e.engine.list_backups().runs.is_empty(),
        "{name}: a backup was made"
    );
    // Not even a temp file was left behind.
    assert!(
        fs::read_dir(&e.dir)
            .unwrap()
            .flatten()
            .all(|f| !f.file_name().to_string_lossy().starts_with(".autocrop-")),
        "{name}: a temp file was left behind"
    );

    let out = e
        .engine
        .save_items(&[v.id], SaveTarget::Copy, "copy run", &nop);
    assert!(out[0].ok, "{name}: {:?}", out[0]);
    assert!(out[0].saved.as_ref().unwrap().copy);
    assert_eq!(
        fs::read(&path).unwrap(),
        bytes,
        "{name}: the source changed"
    );
    let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
    let copy = e.dir.join("AutoCrop").join(format!("{stem}.{copy_ext}"));
    let copy_bytes = fs::read(&copy).unwrap_or_else(|er| panic!("{}: {er}", copy.display()));
    let d = auto_crop_codecs::decode(&copy_bytes).unwrap();
    assert_eq!(
        d.format.extension(),
        if copy_ext == "png" { "png" } else { "jpg" },
        "{name}"
    );
    assert!(
        d.raster.width < v.width && d.raster.height < v.height,
        "{name}: not cropped"
    );
    assert!(
        e.engine.list_backups().runs.is_empty(),
        "{name}: a copy needs no backup"
    );
}

#[test]
fn webp_and_tiff_sources_open_but_are_never_replaced_and_copy_as_png() {
    let e = env();
    check_not_replaceable(&e, "scan.webp", &fx::webp_lossless(64, 48), "png");
    check_not_replaceable(
        &e,
        "page.tif",
        &fx::tiff_rgb8(64, 48, &fx::TiffOpts::default()),
        "png",
    );
}

#[test]
fn jpeg_and_png_sources_are_still_replaced_in_place_with_a_backup() {
    let e = env();
    for (name, bytes) in [
        ("a.jpg", fx::jpeg_baseline(64, 48)),
        ("b.png", fx::png_rgb(64, 48)),
    ] {
        let (_, v) = open(&e, name, &bytes);
        let v = v.unwrap();
        let out = e
            .engine
            .save_items(&[v.id], SaveTarget::Replace, "replace run", &nop);
        assert!(out[0].ok, "{name}: {:?}", out[0]);
        assert_ne!(fs::read(e.dir.join(name)).unwrap(), bytes, "{name}");
        assert!(out[0].saved.as_ref().unwrap().backup_id.is_some(), "{name}");
    }
}

#[cfg(feature = "heif")]
#[test]
fn avif_sources_open_are_never_replaced_and_copy_as_png() {
    let avif = include_bytes!("../../codecs/tests/fixtures/heif/gradient-444-48x32.avif");
    let e = env();
    check_not_replaceable(&e, "photo.avif", avif, "png");
    // The picture really is the AVIF's (not a placeholder): 48x32 cropped to 80% by the quad.
    let copy = fs::read(e.dir.join("AutoCrop/photo.png")).unwrap();
    let d = auto_crop_codecs::decode(&copy).unwrap();
    assert!(
        d.raster.width.abs_diff(38) <= 2 && d.raster.height.abs_diff(26) <= 2,
        "{}x{}",
        d.raster.width,
        d.raster.height
    );
}

#[cfg(feature = "heif")]
#[test]
fn an_avif_with_an_icc_profile_keeps_it_in_the_png_copy() {
    let avif = include_bytes!("../../codecs/tests/fixtures/heif/icc-srgb-32x24.avif");
    let icc = include_bytes!("../../codecs/tests/fixtures/heif/icc-srgb.icc");
    let e = env();
    check_not_replaceable(&e, "tagged.avif", avif, "png");
    let copy = fs::read(e.dir.join("AutoCrop/tagged.png")).unwrap();
    assert_eq!(
        auto_crop_codecs::decode(&copy).unwrap().icc.as_deref(),
        Some(&icc[..]),
        "the ICC profile of the AVIF must survive into the copy"
    );
}

#[cfg(feature = "heif")]
#[test]
fn a_rotated_avif_is_opened_upright_and_exported_upright() {
    // irot/imir are applied once by libheif; the engine sees an upright raster.
    let rotated = include_bytes!("../../codecs/tests/fixtures/heif/orient6-444-48x32.avif");
    let e = env();
    let (_, v) = open(&e, "turned.avif", rotated);
    let v = v.unwrap();
    assert_eq!(
        (v.width, v.height),
        (32, 48),
        "orientation 6 turns 48x32 into 32x48"
    );
}

#[cfg(not(feature = "heif"))]
#[test]
fn avif_and_heic_files_are_not_candidates_without_the_heif_feature() {
    let avif = include_bytes!("../../codecs/tests/fixtures/heif/gradient-444-48x32.avif");
    let e = env();
    for name in ["photo.avif", "photo.heic", "photo.heif"] {
        let (summary, v) = open(&e, name, avif);
        assert_eq!((summary.added, v.is_none()), (0, true), "{name}");
        assert_eq!(
            fs::read(e.dir.join(name)).unwrap(),
            avif,
            "{name} must stay untouched"
        );
    }
}
