// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! AVIF output through the engine (ROADMAP M11.22, M11.23, M11.39): the Convert flow, the editor's
//! Save format, the verify step that reads the container back, and the rule that an AVIF source is
//! open-only (never replaced in place; a copy is PNG unless AVIF is chosen).

use auto_crop_codecs::encoders::avif_box::read_info;
use auto_crop_codecs::fixtures::{fake_icc, png_rgb, png_with_icc};
use auto_crop_codecs::{Format, sniff};
use auto_crop_core::{Enhance, Pt, QuadWarp};
use auto_crop_engine::colour::ColourMode;
use auto_crop_engine::commit::{VerifyMode, verify_bytes};
use auto_crop_engine::convert::{
    ConvertError, ConvertOptions, ConvertRequest, ConvertTarget, Converted, Placement,
};
use auto_crop_engine::fsplan::replace_refusal;
use auto_crop_engine::group::stray_temps;
use auto_crop_engine::output::{EngineOptions, SaveFormat, copy_format};
use auto_crop_engine::{
    AppPaths, DerivedAction, Edit, Engine, ErrKind, ItemView, NoSplit, RestoreMode, SaveTarget,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

fn nop(_: ItemView) {}

struct Env {
    _root: tempfile::TempDir,
    engine: Engine,
    dir: PathBuf,
}

fn env() -> Env {
    let root = tempfile::tempdir().unwrap();
    let engine = Engine::new(AppPaths::under(&root.path().join("data")));
    engine.set_item_detector(Arc::new(NoSplit));
    let dir = root.path().join("photos");
    fs::create_dir_all(&dir).unwrap();
    Env {
        _root: root,
        engine,
        dir,
    }
}

fn put(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let p = dir.join(name);
    fs::write(&p, bytes).unwrap();
    fs::File::options()
        .write(true)
        .open(&p)
        .unwrap()
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_500_000_000))
        .unwrap();
    p
}

fn avif_opts(placement: Placement) -> ConvertOptions {
    ConvertOptions {
        target: ConvertTarget::Avif,
        placement,
        colour: ColourMode::Preserve,
        ..ConvertOptions::default()
    }
}

fn convert(e: &Env, path: &Path, o: ConvertOptions) -> Result<Converted, ConvertError> {
    e.engine
        .convert_file(&ConvertRequest::new(path, o, "run1", "test run"))
}

/// An AVIF source is open-only: the replace gate and the copy format say so in every build.
#[test]
fn an_avif_source_is_never_replaced_and_is_copied_as_png_by_default() {
    assert_eq!(
        replace_refusal(Format::Avif, 1, &[]),
        Some("format.write_unavailable")
    );
    assert_eq!(
        copy_format(Format::Avif, &[], SaveFormat::Auto),
        Format::Png
    );
    assert_eq!(
        copy_format(Format::Avif, &[], SaveFormat::Avif),
        Format::Avif
    );
    assert_eq!(
        copy_format(Format::Jpeg, &[], SaveFormat::Avif),
        Format::Avif
    );
}

#[cfg(not(feature = "avif"))]
#[test]
fn without_the_feature_a_conversion_to_avif_is_refused_and_touches_nothing() {
    let e = env();
    let png = png_rgb(40, 30);
    let src = put(&e.dir, "a.png", &png);
    let err = convert(&e, &src, avif_opts(Placement::Replace)).unwrap_err();
    assert_eq!(
        err,
        ConvertError::Failed(
            ErrKind::UnsupportedOutput,
            vec!["format.write_unavailable".to_owned()]
        )
    );
    assert_eq!(fs::read(&src).unwrap(), png);
    assert!(stray_temps(&e.dir).is_empty());
}

#[cfg(feature = "avif")]
mod with_writer {
    use super::*;
    use auto_crop_engine::output::{AvifChromaSetting, QualitySetting, SourceMeta, encode_raster};

    fn fast() -> EngineOptions {
        EngineOptions {
            avif_speed: Some(10),
            ..EngineOptions::default()
        }
    }

    #[test]
    fn a_png_converts_beside_the_untouched_source_with_the_profile_byte_exact() {
        let e = env();
        let icc = fake_icc(900);
        let png = png_with_icc(64, 48, &icc);
        let src = put(&e.dir, "a.png", &png);
        let mut o = avif_opts(Placement::Keep);
        o.quality = QualitySetting::Fixed { value: 70 };
        let done = convert(&e, &src, o).unwrap();
        assert_eq!(done.output, src.with_extension("avif"));
        assert_eq!(done.to, Format::Avif);
        assert!(
            done.notices.iter().any(|n| n == "avif.lossy"),
            "{:?}",
            done.notices
        );
        assert!(!done.source_removed);
        assert_eq!(fs::read(&src).unwrap(), png);
        let bytes = fs::read(&done.output).unwrap();
        assert_eq!(sniff(&bytes), Some(Format::Avif));
        let info = read_info(&bytes).unwrap();
        assert_eq!((info.width, info.height), (64, 48));
        assert_eq!(info.icc.as_deref(), Some(icc.as_slice()));
        assert!(!info.has_irot_or_imir);
        assert!(stray_temps(&e.dir).is_empty());
    }

    #[test]
    fn a_replaced_source_is_restored_byte_for_byte() {
        let e = env();
        let png = png_rgb(64, 48);
        let src = put(&e.dir, "a.png", &png);
        let done = convert(&e, &src, avif_opts(Placement::Replace)).unwrap();
        assert!(done.source_removed && !src.exists());
        assert!(done.output.exists());
        assert!(done.backup_id.is_some(), "a verified backup");
        let r = e
            .engine
            .restore_path(&done.output, RestoreMode::Auto, DerivedAction::Remove, &nop);
        assert!(r.ok, "{r:?}");
        assert_eq!(fs::read(&src).unwrap(), png);
        assert!(!done.output.exists());
    }

    #[test]
    fn avif_to_avif_is_skipped_as_the_same_format() {
        let e = env();
        // Any AVIF-looking file: the format check comes first and nothing is decoded.
        let avif = fs::read("../codecs/tests/fixtures/heif/gradient-444-48x32.avif").unwrap();
        let src = put(&e.dir, "t.avif", &avif);
        let r = convert(&e, &src, avif_opts(Placement::Keep));
        assert!(matches!(r, Err(ConvertError::Skipped(_, _))), "{r:?}");
        assert_eq!(fs::read(&src).unwrap(), avif);
    }

    #[test]
    fn the_engine_verifies_what_it_wrote_and_catches_damage() {
        let r = auto_crop_codecs::decode(&png_rgb(80, 60)).unwrap().raster;
        let enc = encode_raster(&r, Format::Avif, &SourceMeta::default(), &fast()).unwrap();
        verify_bytes(&enc.bytes, &enc.expect, VerifyMode::Full).unwrap();
        // A truncated file, a wrong size and a missing profile are refused.
        assert!(
            verify_bytes(
                &enc.bytes[..enc.bytes.len() - 9],
                &enc.expect,
                VerifyMode::Full
            )
            .is_err()
        );
        let mut wrong = enc.expect.clone();
        wrong.dims = (81, 60);
        assert!(verify_bytes(&enc.bytes, &wrong, VerifyMode::Full).is_err());
        let mut want_icc = enc.expect.clone();
        want_icc.icc = Some(fake_icc(300));
        assert!(verify_bytes(&enc.bytes, &want_icc, VerifyMode::Full).is_err());
    }

    #[test]
    fn chroma_and_speed_options_change_the_file() {
        let r = auto_crop_codecs::decode(&png_rgb(96, 64)).unwrap().raster;
        let meta = SourceMeta::default();
        let a = encode_raster(&r, Format::Avif, &meta, &fast())
            .unwrap()
            .bytes;
        let b = encode_raster(
            &r,
            Format::Avif,
            &meta,
            &EngineOptions {
                avif_chroma: AvifChromaSetting::Yuv444,
                ..fast()
            },
        )
        .unwrap()
        .bytes;
        let (ia, ib) = (read_info(&a).unwrap(), read_info(&b).unwrap());
        assert!(ia.subsampled_x && !ib.subsampled_x);
    }

    fn whole() -> QuadWarp {
        QuadWarp {
            corners: [
                Pt::new(0.0, 0.0),
                Pt::new(1.0, 0.0),
                Pt::new(1.0, 1.0),
                Pt::new(0.0, 1.0),
            ],
            quarter_turns: 0,
            mirror: false,
            fine_deg: 0.0,
        }
    }

    #[test]
    fn an_enhanced_grey_page_is_one_plane_and_one_bit_is_refused() {
        use auto_crop_imgproc::enhance::testpages::{Light, PageSpec, lit, page};
        let p = page(&PageSpec::document(5).sized(300, 420, 3));
        let r = lit(&p.truth, Light::Tilt { dx: 0.5, dy: 0.1 }, [1.0; 3], 2.0, 5);
        let src = auto_crop_codecs::encode(&r, Format::Png, 90, None).unwrap();
        let render = |o: &EngineOptions| {
            auto_crop_engine::outputs::render_outputs_enhanced(
                &src,
                &[whole()],
                &[Enhance::Document { strength: 100 }],
                Some(Format::Avif),
                o,
            )
        };
        let out = render(&fast()).unwrap().remove(0);
        assert_eq!(out.format, Format::Avif);
        assert!(out.notices.iter().any(|n| n == "avif.lossy"));
        let info = read_info(&out.bytes).unwrap();
        assert!(info.monochrome && info.bits == [8]);
        let one_bit = EngineOptions {
            one_bit: true,
            ..fast()
        };
        assert_eq!(render(&one_bit).err(), Some(ErrKind::UnsupportedOutput));
    }

    fn open_cropped(e: &Env, name: &str, bytes: &[u8]) -> ItemView {
        let path = e.dir.join(name);
        fs::write(&path, bytes).unwrap();
        let id = e.engine.open_paths(&[path], false).ids[0];
        e.engine.analyse(id).unwrap();
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
        e.engine.set_edit(id, &edit, false, "crop").unwrap();
        e.engine.item_view(id).unwrap()
    }

    #[test]
    fn the_save_format_avif_writes_a_copy_and_refuses_a_replace() {
        let e = env();
        let png = png_rgb(80, 60);
        let mut s = e.engine.settings();
        s.save_format = SaveFormat::Avif;
        e.engine.set_settings(s);
        let v = open_cropped(&e, "a.png", &png);
        let out = e.engine.save_items(&[v.id], SaveTarget::Replace, "r", &nop);
        assert_eq!(out[0].error, Some(ErrKind::UnsupportedOutput));
        assert_eq!(fs::read(e.dir.join("a.png")).unwrap(), png);
        let out = e.engine.save_items(&[v.id], SaveTarget::Copy, "c", &nop);
        assert!(out[0].ok, "{:?}", out[0]);
        let bytes = fs::read(e.dir.join("AutoCrop").join("a.avif")).unwrap();
        let info = read_info(&bytes).unwrap();
        assert_eq!((info.width, info.height), (64, 48));
        assert!(
            out[0].notices.iter().any(|n| n == "avif.lossy"),
            "{:?}",
            out[0].notices
        );
        assert_eq!(fs::read(e.dir.join("a.png")).unwrap(), png);
    }
}
