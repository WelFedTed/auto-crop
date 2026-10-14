// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Formats the engine can open but not write back as themselves (PLAN 3.2.3): 1-bit TIFF, lossy
//! WebP (this build's WebP writer is lossless) and, with the `heif` feature, AVIF and HEIC. Their
//! sources are never replaced in place, the source stays byte-identical, and "save as copy" writes
//! a PNG (JPEG for HEIC) beside it. Single-page 8-bit TIFF and lossless WebP sources have writers
//! (M11) and are replaced in place after a verified backup, which restore undoes byte for byte.

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
    e.engine.set_edit(id, &edit, false, "test crop").unwrap();
    // A scan that the detector split into several crops is held for review; looking at it and
    // accepting it is what the user does, and the format rules below must hold after that too.
    let v = e
        .engine
        .accept_scan(id)
        .unwrap_or_else(|_| e.engine.item_view(id).unwrap());
    (summary, Some(v))
}

/// A source with no writer: replacing is refused, the bytes stay, and a copy is a PNG or JPEG.
fn check_not_replaceable(
    e: &Env,
    name: &str,
    bytes: &[u8],
    copy_ext: &str,
    notice: &str,
) -> PathBuf {
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
    // One code for "this source is never replaced" on the single-item and the split path, with
    // the reason in the notice.
    assert_eq!(out[0].error, Some(ErrKind::NotReplaceable), "{name}");
    assert_eq!(out[0].notices, [notice], "{name}");
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
    // The copy is `<stem>.<ext>`, or a numbered name when the scan was split into several crops.
    let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
    let mut copies: Vec<PathBuf> = fs::read_dir(e.dir.join("AutoCrop"))
        .unwrap()
        .flatten()
        .map(|f| f.path())
        .filter(|p| {
            p.extension().is_some_and(|x| x == copy_ext)
                && p.file_stem().unwrap().to_string_lossy().starts_with(&stem)
        })
        .collect();
    copies.sort();
    let copy = copies
        .first()
        .unwrap_or_else(|| panic!("{name}: no copy was written"))
        .clone();
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
    copy
}

#[test]
fn a_bilevel_tiff_and_a_bmp_open_but_are_never_replaced_and_copy_as_png() {
    let e = env();
    // A 1-bit scan would become a 24-bit file: left alone, a copy is a PNG.
    check_not_replaceable(
        &e,
        "bilevel.tif",
        &fx::tiff_g4(64, 48),
        "png",
        "tiff.low_depth",
    );
    let rgb = fx::pattern(64, 48);
    check_not_replaceable(
        &e,
        "pic.bmp",
        &fx::bmp_rgb24(64, 48, &rgb, false),
        "png",
        "format.write_unavailable",
    );
}

#[test]
fn single_page_tiff_and_lossless_webp_sources_are_replaced_in_place_and_restore_returns_them() {
    let e = env();
    let icc = fx::fake_icc(900);
    for (name, bytes, format) in [
        (
            "page.tif",
            fx::tiff_rgb8(
                64,
                48,
                &fx::TiffOpts {
                    icc: Some(icc.clone()),
                    ..fx::TiffOpts::default()
                },
            ),
            auto_crop_codecs::Format::Tiff,
        ),
        (
            "scan.webp",
            fx::webp_extended(64, 48, None, Some(&icc)),
            auto_crop_codecs::Format::Webp,
        ),
    ] {
        let (_, v) = open(&e, name, &bytes);
        let v = v.unwrap();
        assert_eq!(v.open_only, None, "{name}");
        let out = e
            .engine
            .save_items(&[v.id], SaveTarget::Replace, "replace run", &nop);
        assert!(out[0].ok, "{name}: {:?}", out[0]);
        let path = e.dir.join(name);
        let written = fs::read(&path).unwrap();
        assert_ne!(written, bytes, "{name}");
        // Still the same format under the same name, cropped, with the profile byte-exact.
        let d = auto_crop_codecs::decode(&written).unwrap();
        assert_eq!(d.format, format, "{name}");
        assert!(d.raster.width < 64 && d.raster.height < 48, "{name}");
        assert_eq!(d.icc.as_deref(), Some(&icc[..]), "{name}");
        assert_eq!(d.exif_orientation, 1, "{name}");
        assert!(out[0].saved.as_ref().unwrap().backup_id.is_some(), "{name}");
        // Restore puts the original back byte for byte.
        let file = e.engine.list_backups().runs[0].files[0].id.clone();
        let r = e
            .engine
            .restore_file(&file, auto_crop_engine::RestoreMode::Auto, &nop);
        assert!(r.ok, "{name}: {r:?}");
        assert_eq!(fs::read(&path).unwrap(), bytes, "{name}: restored");
        // Next file in a clean engine state.
        e.engine.remove_items(&[v.id]);
        fs::remove_dir_all(e._root.path().join("backups")).ok();
    }
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
    let copy = check_not_replaceable(&e, "photo.avif", avif, "png", "format.write_unavailable");
    // The picture really is the AVIF's (not a placeholder): smaller than 48x32 and not empty.
    let d = auto_crop_codecs::decode(&fs::read(copy).unwrap()).unwrap();
    assert!(
        d.raster.width > 8 && d.raster.width < 48 && d.raster.height > 8 && d.raster.height < 32,
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
    let copy = fs::read(check_not_replaceable(
        &e,
        "tagged.avif",
        avif,
        "png",
        "format.write_unavailable",
    ))
    .unwrap();
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
