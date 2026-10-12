// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! HEIC, HEIF and AVIF through the Convert flow (the owner's original requirement: HEIC to JPG).
//! Needs the `heif` feature, hence the native libraries of `cargo xtask build-native`: it runs in
//! `.github/workflows/heif.yml` on Windows, macOS and Linux, never in the default build.
//!
//! Inputs: the committed AVIF fixtures (`crates/codecs/tests/fixtures/heif`) and the HEIC and AVIF
//! files of the pinned libheif source archive, which `build-native` extracts and which are read in
//! place, never committed. `AUTOCROP_REQUIRE_HEIF_SAMPLES=1` (CI) turns a missing archive from a
//! skip into a failure.

#![cfg(feature = "heif")]

use auto_crop_codecs::decode;
use auto_crop_codecs::fixtures::heif_real_files;
use auto_crop_engine::colour::{IccKind, classify};
use auto_crop_engine::convert::{
    ConvertError, ConvertOptions, ConvertRequest, ConvertTarget, Converted, Placement, Skip,
};
use auto_crop_engine::convert_batch::ConvertStatus;
use auto_crop_engine::store::Store;
use auto_crop_engine::{AppPaths, DerivedAction, Engine, ItemView, RestoreMode};
use std::fs;
use std::path::{Path, PathBuf};

fn nop(_: ItemView) {}

struct Env {
    _root: tempfile::TempDir,
    paths: AppPaths,
    engine: Engine,
    dir: PathBuf,
}

fn env() -> Env {
    let root = tempfile::tempdir().unwrap();
    let paths = AppPaths::under(&root.path().join("data"));
    let engine = Engine::new(paths.clone());
    let dir = root.path().join("photos");
    fs::create_dir_all(&dir).unwrap();
    Env {
        _root: root,
        paths,
        engine,
        dir,
    }
}

fn fixture(name: &str) -> Vec<u8> {
    heif_real_files()
        .into_iter()
        .find(|(n, _)| *n == name)
        .unwrap_or_else(|| panic!("no fixture {name}"))
        .1
        .to_vec()
}

/// A file of the extracted libheif source archive, if `build-native` has been run.
fn libheif_sample(rel: &str) -> Option<Vec<u8>> {
    let v = auto_crop_codecs::heif::runtime_version();
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/native")
        .join(format!("libheif-{v}/src/libheif-{v}"));
    match fs::read(dir.join(rel)) {
        Ok(b) => Some(b),
        Err(e) => {
            assert!(
                std::env::var_os("AUTOCROP_REQUIRE_HEIF_SAMPLES").is_none(),
                "{rel} is required but missing: {e}"
            );
            eprintln!("skipped: {rel} not found ({e}); run `cargo xtask build-native`");
            None
        }
    }
}

fn put(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let p = dir.join(name);
    fs::write(&p, bytes).unwrap();
    p
}

fn convert(
    e: &Env,
    path: &Path,
    target: ConvertTarget,
    placement: Placement,
) -> Result<Converted, ConvertError> {
    let o = ConvertOptions {
        target,
        placement,
        ..ConvertOptions::default()
    };
    e.engine
        .convert_file(&ConvertRequest::new(path, o, "run", "heif test"))
}

/// The output is a JPEG or PNG that decodes to the source's own size with the turn applied once
/// (orientation 1 in the file), and whatever profile it carries is sRGB.
fn check_output(label: &str, src_bytes: &[u8], done: &Converted) {
    let want = decode(src_bytes).unwrap_or_else(|e| panic!("{label}: source decode: {e}"));
    let out = fs::read(&done.output).unwrap();
    let got = decode(&out).unwrap_or_else(|e| panic!("{label}: output decode: {e}"));
    assert_eq!(
        (got.raster.width, got.raster.height),
        (want.raster.width, want.raster.height),
        "{label}: dimensions"
    );
    assert_eq!(got.exif_orientation, 1, "{label}: upright, turned once");
    assert!(
        matches!(classify(got.icc.as_deref()), IccKind::None | IccKind::Srgb),
        "{label}: the output profile is sRGB or absent"
    );
    assert_eq!(done.dims, (got.raster.width, got.raster.height), "{label}");
}

#[test]
fn every_committed_avif_converts_to_jpg_and_png_with_the_right_size_and_an_srgb_profile() {
    for (name, bytes) in heif_real_files() {
        if name.starts_with("seq-") {
            continue; // a sequence is never replaced; tested below
        }
        for target in [ConvertTarget::Jpg, ConvertTarget::Png] {
            let e = env();
            let src = put(&e.dir, name, bytes);
            let label = format!("{name} -> {target:?}");
            let done = convert(&e, &src, target, Placement::Keep)
                .unwrap_or_else(|x| panic!("{label}: {x:?}"));
            check_output(&label, bytes, &done);
            assert!(src.exists(), "{label}: keep leaves the source");
        }
    }
}

#[test]
fn avif_special_cases_notices_exif_and_the_embedded_srgb_profile() {
    // 10-bit: reduced with a notice. Alpha: flattened with a notice.
    let e = env();
    let src = put(&e.dir, "d.avif", &fixture("depth10-32x24.avif"));
    let done = convert(&e, &src, ConvertTarget::Jpg, Placement::Keep).unwrap();
    assert!(
        done.notices.iter().any(|n| n == "depth.reduced_to_8"),
        "{:?}",
        done.notices
    );
    let src = put(&e.dir, "a.avif", &fixture("alpha-32x24.avif"));
    let done = convert(&e, &src, ConvertTarget::Jpg, Placement::Keep).unwrap();
    assert!(
        done.notices.iter().any(|n| n == "alpha.flattened"),
        "{:?}",
        done.notices
    );

    // An sRGB ICC profile in the file is kept byte-exact (it is sRGB: nothing to convert).
    let bytes = fixture("icc-srgb-32x24.avif");
    let src = put(&e.dir, "i.avif", &bytes);
    let done = convert(&e, &src, ConvertTarget::Jpg, Placement::Keep).unwrap();
    let want = decode(&bytes)
        .unwrap()
        .icc
        .expect("the fixture has a profile");
    let got = decode(&fs::read(&done.output).unwrap()).unwrap().icc;
    assert_eq!(got.as_deref(), Some(want.as_slice()));
    assert_eq!(done.icc, IccKind::Srgb);

    // EXIF Orientation 6 without `irot`: libheif leaves the pixels alone, so the output is NOT
    // turned, its EXIF says 1, and the EXIF is carried.
    let bytes = fixture("exif-orient6-noirot-48x32.avif");
    let src = put(&e.dir, "x.avif", &bytes);
    let done = convert(&e, &src, ConvertTarget::Jpg, Placement::Keep).unwrap();
    assert_eq!(done.dims, (48, 32));
    let meta = auto_crop_codecs::jpeg_meta::read_meta(&fs::read(&done.output).unwrap()).unwrap();
    let exif = meta.exif.expect("the EXIF item is carried");
    assert_eq!(auto_crop_codecs::exif_orientation(&exif), Some(1));
}

#[test]
fn a_heif_sequence_is_never_replaced_but_keep_converts_its_first_frame() {
    let e = env();
    let bytes = fixture("seq-2frames-32x24.avif");
    let src = put(&e.dir, "seq.avif", &bytes);
    match convert(&e, &src, ConvertTarget::Jpg, Placement::Replace) {
        Err(ConvertError::Skipped(Skip::NotReplaceable(n), _)) => {
            assert!(
                matches!(n, "anim.first_frame_only" | "heic.multi_image"),
                "{n}"
            );
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(fs::read(&src).unwrap(), bytes);
    let done = convert(&e, &src, ConvertTarget::Jpg, Placement::Keep).unwrap();
    assert!(
        done.notices
            .iter()
            .any(|n| n == "anim.first_frame_only" || n == "heic.multi_image"),
        "{:?}",
        done.notices
    );
}

#[test]
fn heic_files_of_the_libheif_archive_convert_replace_and_restore_byte_identical() {
    for (rel, size) in [
        ("tests/data/rainbow-451x461.heic", Some((451, 461))),
        ("tests/data/with-alpha-512x512.heic", Some((512, 512))),
        ("tests/data/clap_cropped.heic", None),
    ] {
        let Some(bytes) = libheif_sample(rel) else {
            return;
        };
        let name = Path::new(rel)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        for target in [ConvertTarget::Jpg, ConvertTarget::Png] {
            let e = env();
            let src = put(&e.dir, &name, &bytes);
            let done = convert(&e, &src, target, Placement::Replace)
                .unwrap_or_else(|x| panic!("{rel} -> {target:?}: {x:?}"));
            check_output(rel, &bytes, &done);
            if let Some(s) = size {
                assert_eq!(done.dims, s, "{rel}");
            }
            assert!(!src.exists() && done.source_removed, "{rel}: replaced");
            assert_eq!(done.output, src.with_extension(target.name()), "{rel}");
            if rel.contains("alpha") {
                assert!(
                    done.notices.iter().any(|n| n == "alpha.flattened"),
                    "{:?}",
                    done.notices
                );
            }
            // Restore returns the HEIC byte for byte and moves the converted file into the store.
            let r =
                e.engine
                    .restore_path(&done.output, RestoreMode::Auto, DerivedAction::Remove, &nop);
            assert!(r.ok, "{rel}: {r:?}");
            assert_eq!(fs::read(&src).unwrap(), bytes, "{rel}: restored");
            assert!(!done.output.exists());
            let store = Store::new(e.paths.backups_dir());
            assert_eq!(store.list().len(), 1);
        }
    }
}

#[test]
fn a_batch_of_heic_and_avif_files_is_previewed_and_converted_with_the_decoder_present() {
    let Some(heic) = libheif_sample("tests/data/rainbow-451x461.heic") else {
        return;
    };
    let e = env();
    put(&e.dir, "IMG_0001.HEIC", &heic);
    put(&e.dir, "IMG_0002.heic", &heic);
    put(&e.dir, "tile.avif", &fixture("gradient-444-48x32.avif"));
    put(
        &e.dir,
        "plain.jpg",
        &auto_crop_codecs::fixtures::JpegSpec::new(32, 32).build(),
    );
    let added = e.engine.convert_add(std::slice::from_ref(&e.dir), false);
    assert_eq!(added.added, 4);
    let o = ConvertOptions::default();
    let p = e.engine.convert_preview(&o);
    assert!(p.heif_available);
    assert_eq!(
        (p.cannot, p.heif_unavailable, p.same_format),
        (0, 0, 1),
        "{p:?}"
    );
    assert_eq!(p.will_convert, 3);
    let s = e.engine.convert_run(&o, "HEIC batch", 2, &|_| {});
    assert_eq!((s.converted, s.skipped, s.failed), (3, 1, 0), "{s:#?}");
    for n in ["IMG_0001.jpg", "IMG_0002.jpg", "tile.jpg"] {
        assert!(e.dir.join(n).exists(), "{n}");
    }
    assert!(s.results.iter().all(|r| r.status != ConvertStatus::Failed));
}
