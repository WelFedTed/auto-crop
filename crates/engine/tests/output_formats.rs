// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! TIFF, lossless WebP and PDF output through the engine (ROADMAP M11.21, M11.29, M11.32 to M11.38,
//! M11.40; PLAN 3.3, 3.6): the Convert flow to the three targets, the in-place re-save of a TIFF
//! keeping its density, profile and descriptive tags, the enhanced grey pages (one channel, 1-bit
//! Group 4), the verify step that reads a PDF back, and the editor's Save format (a copy only).

use auto_crop_codecs::encoders::pdf::{PageCodec, read_pages};
use auto_crop_codecs::encoders::{TiffEncoder, WebpEncoder};
use auto_crop_codecs::fixtures::{
    TiffOpts, fake_icc, pattern, png_rgb, png_with_icc, psnr, tiff_rgb8,
};
use auto_crop_codecs::{Format, RawTag, decode, extract_dpi, extract_tiff_meta, probe};
use auto_crop_core::ports::{EncodeMeta, Encoder, PixelFormat, Raster as CoreRaster, Samples};
use auto_crop_core::{CancelToken, Enhance, OutputSpec, Pt, QuadWarp};
use auto_crop_engine::colour::ColourMode;
use auto_crop_engine::commit::{VerifyMode, verify_bytes};
use auto_crop_engine::convert::{
    ConvertError, ConvertOptions, ConvertRequest, ConvertTarget, Converted, Placement, Skip,
};
use auto_crop_engine::group::{pending_journals, stray_temps};
use auto_crop_engine::output::{EngineOptions, SaveFormat, SourceMeta, encode_raster};
use auto_crop_engine::store::Store;
use auto_crop_engine::{
    AppPaths, DerivedAction, Edit, Engine, ErrKind, ItemView, NoSplit, RestoreMode, SaveTarget,
};
use auto_crop_imgproc::Raster;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

fn nop(_: ItemView) {}

fn old_mtime() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_500_000_000)
}

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

fn put(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let p = dir.join(name);
    fs::write(&p, bytes).unwrap();
    fs::File::options()
        .write(true)
        .open(&p)
        .unwrap()
        .set_modified(old_mtime())
        .unwrap();
    p
}

fn opts(target: ConvertTarget, placement: Placement) -> ConvertOptions {
    ConvertOptions {
        target,
        placement,
        colour: ColourMode::Preserve,
        ..ConvertOptions::default()
    }
}

fn convert(e: &Env, path: &Path, o: ConvertOptions) -> Result<Converted, ConvertError> {
    e.engine
        .convert_file(&ConvertRequest::new(path, o, "run1", "test run"))
}

// ------------------------------------------------------------------ Convert to the new targets

#[test]
fn a_png_converts_to_tiff_webp_and_pdf_beside_the_untouched_source() {
    let png = png_rgb(64, 48);
    let want = decode(&png).unwrap().raster;
    for (target, ext) in [
        (ConvertTarget::Tiff, "tif"),
        (ConvertTarget::Webp, "webp"),
        (ConvertTarget::Pdf, "pdf"),
    ] {
        let e = env();
        let src = put(&e.dir, "a.png", &png);
        let done = convert(&e, &src, opts(target, Placement::Keep)).unwrap();
        let out = src.with_extension(ext);
        assert_eq!(done.output, out, "{target:?}");
        assert!(!done.source_removed && fs::read(&src).unwrap() == png);
        let bytes = fs::read(&out).unwrap();
        match target {
            ConvertTarget::Pdf => {
                let pages = read_pages(&bytes).expect("a PDF this build can read back");
                assert_eq!(pages.len(), 1);
                let p = &pages[0];
                assert_eq!((p.width_px, p.height_px, p.codec), (64, 48, PageCodec::Dct));
                // 64 px at the nominal 300 dpi.
                assert_eq!(p.media_box, (15.36, 11.52));
                let (_, _, rgb) = p.to_rgb().unwrap();
                assert!(psnr(&want.data, &rgb) > 28.0, "JPEG page fidelity");
            }
            _ => {
                let got = decode(&bytes).unwrap();
                assert_eq!(got.raster.data, want.data, "{target:?}: lossless is exact");
                assert_eq!(got.exif_orientation, 1);
                assert_eq!(got.format, done.to);
            }
        }
        assert!(stray_temps(&e.dir).is_empty());
    }
}

#[test]
fn a_png_replaced_by_a_tiff_or_a_webp_is_restored_byte_for_byte() {
    let png = png_with_icc(64, 48, &fake_icc(700));
    for (target, ext, format) in [
        (ConvertTarget::Tiff, "tif", Format::Tiff),
        (ConvertTarget::Webp, "webp", Format::Webp),
    ] {
        let e = env();
        let src = put(&e.dir, "a.png", &png);
        let done = convert(&e, &src, opts(target, Placement::Replace)).unwrap();
        let out = src.with_extension(ext);
        assert!(done.source_removed && !src.exists() && out.exists());
        let got = decode(&fs::read(&out).unwrap()).unwrap();
        assert_eq!(got.format, format);
        assert_eq!(got.icc, Some(fake_icc(700)), "the profile is byte-exact");
        assert!(
            pending_journals(&Store::new(e.paths.backups_dir())) == 0
                && stray_temps(&e.dir).is_empty()
        );
        let r = e
            .engine
            .restore_path(&out, RestoreMode::Auto, DerivedAction::Remove, &nop);
        assert!(r.ok, "{target:?}: {r:?}");
        assert_eq!(fs::read(&src).unwrap(), png, "{target:?}: restored");
        assert!(!out.exists());
    }
}

#[test]
fn a_source_already_a_tiff_is_skipped_and_a_multi_page_one_is_never_replaced() {
    let e = env();
    let one = put(&e.dir, "a.tif", &tiff_rgb8(32, 32, &TiffOpts::default()));
    assert!(matches!(
        convert(&e, &one, opts(ConvertTarget::Tiff, Placement::Replace)),
        Err(ConvertError::Skipped(Skip::SameFormat, _))
    ));
    let pages = put(
        &e.dir,
        "b.tif",
        &tiff_rgb8(
            32,
            32,
            &TiffOpts {
                extra_pages: 1,
                ..TiffOpts::default()
            },
        ),
    );
    let before = fs::read(&pages).unwrap();
    assert!(matches!(
        convert(&e, &pages, opts(ConvertTarget::Webp, Placement::Replace)),
        Err(ConvertError::Skipped(
            Skip::NotReplaceable("tiff.multi_page"),
            _
        ))
    ));
    assert_eq!(fs::read(&pages).unwrap(), before);
    // A copy of its first page is fine.
    assert!(convert(&e, &pages, opts(ConvertTarget::Webp, Placement::Keep)).is_ok());
}

#[test]
fn the_targets_serialise_as_the_names_the_ui_and_cli_use() {
    for (t, name) in [
        (ConvertTarget::Jpg, "jpg"),
        (ConvertTarget::Png, "png"),
        (ConvertTarget::Tiff, "tiff"),
        (ConvertTarget::Webp, "webp"),
        (ConvertTarget::Pdf, "pdf"),
        (ConvertTarget::Avif, "avif"),
    ] {
        assert_eq!(t.name(), name);
        assert_eq!(serde_json::to_string(&t).unwrap(), format!("\"{name}\""));
        assert_eq!(
            serde_json::from_str::<ConvertTarget>(&format!("\"{name}\"")).unwrap(),
            t
        );
    }
    assert_eq!(ConvertTarget::ALL.len(), 6);
}

// ------------------------------------------------------------------ TIFF re-save keeps what it can

fn core_raster(w: u32, h: u32) -> CoreRaster {
    CoreRaster::from_samples(w, h, PixelFormat::Rgb8, Samples::U8(pattern(w, h))).unwrap()
}

fn scanner_tiff() -> Vec<u8> {
    let enc = TiffEncoder {
        extra: vec![
            RawTag {
                tag: 270,
                ty: 2,
                count: 14,
                data: b"Kitchen desk.\0".to_vec(),
            },
            RawTag {
                tag: 271,
                ty: 2,
                count: 7,
                data: b"Acme Co\0".to_vec()[..7].to_vec(),
            },
            RawTag {
                tag: 306,
                ty: 2,
                count: 20,
                data: b"2026:10:14 09:08:07\0".to_vec(),
            },
            RawTag {
                tag: 700,
                ty: 1,
                count: 12,
                data: b"<x:xmpmeta/>".to_vec(),
            },
        ],
        ..TiffEncoder::default()
    };
    let meta = EncodeMeta {
        icc: Some(fake_icc(1500).into()),
        dpi: Some((300, 300)),
    };
    enc.encode(
        &core_raster(80, 60).view(),
        &OutputSpec::default(),
        &meta,
        &CancelToken::never(),
    )
    .unwrap()
}

fn tag(bytes: &[u8], t: u16) -> Option<Vec<u8>> {
    extract_tiff_meta(bytes)?
        .carried
        .into_iter()
        .find(|x| x.tag == t)
        .map(|x| x.data)
}

#[test]
fn a_tiff_saved_in_place_keeps_its_density_profile_and_descriptive_tags_by_policy() {
    for (strip_location, label) in [(false, "keep all"), (true, "strip location")] {
        let e = env();
        let src = put(&e.dir, "scan.tif", &scanner_tiff());
        let mut o = opts(ConvertTarget::Tiff, Placement::Replace);
        o.reencode = true;
        o.strip_location = strip_location;
        let done = convert(&e, &src, o).unwrap();
        assert_eq!(done.output, src, "{label}: swapped in place");
        let out = fs::read(&src).unwrap();
        assert_eq!(extract_dpi(&out, Format::Tiff), Some((300, 300)), "{label}");
        assert_eq!(decode(&out).unwrap().icc, Some(fake_icc(1500)), "{label}");
        assert_eq!(
            tag(&out, 306).as_deref(),
            Some(&b"2026:10:14 09:08:07\0"[..]),
            "{label}"
        );
        assert!(tag(&out, 271).is_some(), "{label}: Make stays");
        // Free text, XMP and IPTC are what "strip location" removes (as for a JPEG's comments).
        assert_eq!(
            tag(&out, 270).is_some(),
            !strip_location,
            "{label}: description"
        );
        assert_eq!(tag(&out, 700).is_some(), !strip_location, "{label}: XMP");
        // The same pixels.
        assert_eq!(
            decode(&out).unwrap().raster.data,
            pattern(80, 60),
            "{label}"
        );
        assert_eq!(probe(&out).unwrap().orientation, 1);
    }
}

#[test]
fn strip_all_leaves_no_descriptive_tag_but_keeps_the_profile_and_density() {
    let src = scanner_tiff();
    let r = decode(&src).unwrap().raster;
    let meta = SourceMeta::read(Format::Tiff, &src, decode(&src).unwrap().icc.map(Arc::new));
    let o = EngineOptions {
        strip_metadata: true,
        ..EngineOptions::default()
    };
    let enc = encode_raster(&r, Format::Tiff, &meta, &o).unwrap();
    for t in [270u16, 271, 306, 700] {
        assert!(tag(&enc.bytes, t).is_none(), "tag {t}");
    }
    assert_eq!(extract_dpi(&enc.bytes, Format::Tiff), Some((300, 300)));
    assert_eq!(decode(&enc.bytes).unwrap().icc, Some(fake_icc(1500)));
}

#[test]
fn a_grey_colour_raster_without_a_profile_is_written_as_one_grey_channel() {
    let grey = Raster::filled(40, 30, [120, 120, 120]);
    let enc = encode_raster(
        &grey,
        Format::Tiff,
        &SourceMeta::default(),
        &EngineOptions::default(),
    )
    .unwrap();
    assert_eq!(probe(&enc.bytes).unwrap().channels, 1);
    verify_bytes(&enc.bytes, &enc.expect, VerifyMode::Full).unwrap();
    // With a profile the three channels stay (a grey file cannot carry an RGB profile).
    let meta = SourceMeta {
        icc: Some(Arc::new(fake_icc(900))),
        ..SourceMeta::default()
    };
    let enc = encode_raster(&grey, Format::Tiff, &meta, &EngineOptions::default()).unwrap();
    assert_eq!(probe(&enc.bytes).unwrap().channels, 3);
    verify_bytes(&enc.bytes, &enc.expect, VerifyMode::Full).unwrap();
}

#[test]
fn every_compression_setting_verifies() {
    use auto_crop_engine::output::TiffCompressionSetting as C;
    let r = decode(&png_rgb(48, 32)).unwrap().raster;
    for c in [C::None, C::Lzw, C::Deflate, C::PackBits] {
        let o = EngineOptions {
            tiff_compression: Some(c),
            ..EngineOptions::default()
        };
        let enc = encode_raster(&r, Format::Tiff, &SourceMeta::default(), &o).unwrap();
        verify_bytes(&enc.bytes, &enc.expect, VerifyMode::Full).unwrap();
    }
}

#[test]
fn a_tiff_keeps_its_own_compression_unless_one_is_chosen() {
    use auto_crop_codecs::TiffMeta;
    use auto_crop_codecs::encoders::TiffCompression as C;
    let r = decode(&png_rgb(48, 32)).unwrap().raster;
    let tag = |b: &[u8]| -> u16 {
        extract_tiff_meta(b)
            .map(|m: TiffMeta| m.compression)
            .unwrap()
    };
    let enc_meta = EncodeMeta::default();
    let spec = OutputSpec::default();
    let core = core_raster(48, 32);
    for (c, want) in [
        (C::None, 1u16),
        (C::Lzw, 5),
        (C::Deflate, 8),
        (C::PackBits, 32773),
    ] {
        let src = TiffEncoder {
            compression: c,
            ..TiffEncoder::default()
        }
        .encode(&core.view(), &spec, &enc_meta, &CancelToken::never())
        .unwrap();
        assert_eq!(tag(&src), want);
        let meta = SourceMeta::read(Format::Tiff, &src, None);
        let out = encode_raster(&r, Format::Tiff, &meta, &EngineOptions::default()).unwrap();
        assert_eq!(tag(&out.bytes), want, "{c:?}: the source's class is kept");
    }
    // Chosen explicitly: wins over the source's. A source that is not a TIFF gets LZW.
    let src = TiffEncoder {
        compression: C::None,
        ..TiffEncoder::default()
    }
    .encode(&core.view(), &spec, &enc_meta, &CancelToken::never())
    .unwrap();
    let meta = SourceMeta::read(Format::Tiff, &src, None);
    let o = EngineOptions {
        tiff_compression: Some(auto_crop_engine::output::TiffCompressionSetting::Deflate),
        ..EngineOptions::default()
    };
    assert_eq!(
        tag(&encode_raster(&r, Format::Tiff, &meta, &o).unwrap().bytes),
        8
    );
    let plain = encode_raster(
        &r,
        Format::Tiff,
        &SourceMeta::default(),
        &EngineOptions::default(),
    )
    .unwrap();
    assert_eq!(tag(&plain.bytes), 5);
}

// ------------------------------------------------------------------ PDF verification

#[test]
fn the_pdf_verify_step_reads_the_page_back_and_catches_damage() {
    let r = decode(&png_rgb(64, 48)).unwrap().raster;
    let meta = SourceMeta {
        icc: Some(Arc::new(pdf_icc())),
        ..SourceMeta::default()
    };
    let enc = encode_raster(&r, Format::Pdf, &meta, &EngineOptions::default()).unwrap();
    verify_bytes(&enc.bytes, &enc.expect, VerifyMode::Full).unwrap();
    verify_bytes(&enc.bytes, &enc.expect, VerifyMode::Fast).unwrap();
    // The profile went into an ICCBased colour space and came back byte-exact.
    assert_eq!(read_pages(&enc.bytes).unwrap()[0].icc, Some(pdf_icc()));
    // Truncation, a flipped byte in the structure, a wrong size, a missing profile.
    for cut in [10, enc.bytes.len() / 2, enc.bytes.len() - 7] {
        assert_eq!(
            verify_bytes(&enc.bytes[..cut], &enc.expect, VerifyMode::Full),
            Err(ErrKind::VerifyFailed),
            "cut {cut}"
        );
    }
    let mut bad = enc.expect.clone();
    bad.dims = (65, 48);
    assert_eq!(
        verify_bytes(&enc.bytes, &bad, VerifyMode::Full),
        Err(ErrKind::VerifyFailed)
    );
    let mut bad = enc.expect.clone();
    bad.icc = Some(vec![1, 2, 3]);
    assert_eq!(
        verify_bytes(&enc.bytes, &bad, VerifyMode::Full),
        Err(ErrKind::VerifyFailed)
    );
    // Another picture entirely.
    let other = encode_raster(
        &Raster::filled(64, 48, [10, 200, 30]),
        Format::Pdf,
        &meta,
        &EngineOptions::default(),
    )
    .unwrap();
    assert_eq!(
        verify_bytes(&other.bytes, &enc.expect, VerifyMode::Full),
        Err(ErrKind::VerifyFailed)
    );
    // A PNG is not a PDF.
    let png = encode_raster(
        &r,
        Format::Png,
        &SourceMeta::default(),
        &EngineOptions::default(),
    )
    .unwrap();
    assert_eq!(
        verify_bytes(&png.bytes, &enc.expect, VerifyMode::Full),
        Err(ErrKind::VerifyFailed)
    );
}

fn pdf_icc() -> Vec<u8> {
    let mut v = fake_icc(1800);
    v[16..20].copy_from_slice(b"RGB ");
    v
}

#[test]
fn a_pdf_page_follows_the_source_density_and_carries_no_exif_or_location() {
    // A JPEG with EXIF (GPS) and a 150 dpi JFIF density becomes a 150 dpi page, no metadata.
    let jpeg = auto_crop_codecs::fixtures::JpegSpec::new(150, 75).build();
    let mut payload = b"Exif\0\0".to_vec();
    payload.extend_from_slice(&auto_crop_codecs::fixtures::exif_blob(1, true));
    let jpeg = auto_crop_codecs::fixtures::jpeg_insert_segment(&jpeg, 0xE1, &payload);
    let e = env();
    let src = put(&e.dir, "photo.jpg", &jpeg);
    let done = convert(&e, &src, opts(ConvertTarget::Pdf, Placement::Keep)).unwrap();
    let bytes = fs::read(&done.output).unwrap();
    assert!(!bytes.windows(4).any(|w| w == b"Exif"));
    assert!(!String::from_utf8_lossy(&bytes).contains("/Font"));
    let p = &read_pages(&bytes).unwrap()[0];
    assert_eq!((p.width_px, p.height_px), (150, 75));
}

// ------------------------------------------------------------------ enhanced grey pages

fn page_png() -> Vec<u8> {
    use auto_crop_imgproc::enhance::testpages::{Light, PageSpec, lit, page};
    let p = page(&PageSpec::document(5).sized(300, 420, 3));
    let r = lit(&p.truth, Light::Tilt { dx: 0.5, dy: 0.1 }, [1.0; 3], 2.0, 5);
    auto_crop_codecs::encode(&r, Format::Png, 90, None).unwrap()
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

fn render(src: &[u8], fmt: Format, o: &EngineOptions) -> Result<Vec<u8>, ErrKind> {
    auto_crop_engine::outputs::render_outputs_enhanced(
        src,
        &[whole()],
        &[Enhance::Document { strength: 100 }],
        Some(fmt),
        o,
    )
    .map(|mut v| v.remove(0).bytes)
}

#[test]
fn enhanced_grey_pages_are_one_channel_and_equal_the_png_in_every_lossless_format() {
    let src = page_png();
    let o = EngineOptions::default();
    let png = render(&src, Format::Png, &o).unwrap();
    assert_eq!(probe(&png).unwrap().channels, 1);
    let want = decode(&png).unwrap().raster;
    for fmt in [Format::Tiff, Format::Webp] {
        let b = render(&src, fmt, &o).unwrap();
        assert_eq!(decode(&b).unwrap().raster.data, want.data, "{fmt:?}");
        if fmt == Format::Tiff {
            assert_eq!(
                probe(&b).unwrap().channels,
                1,
                "one grey channel in the TIFF"
            );
        }
    }
    let pdf = render(&src, Format::Pdf, &o).unwrap();
    let p = &read_pages(&pdf).unwrap()[0];
    assert_eq!((p.codec, p.channels), (PageCodec::Flate, 1));
    assert_eq!(p.to_rgb().unwrap().2, want.data, "a lossless grey page");
}

#[test]
fn one_bit_output_is_group_4_in_a_tiff_or_a_pdf_and_a_typed_error_anywhere_else() {
    let src = page_png();
    let grey = render(&src, Format::Png, &EngineOptions::default()).unwrap();
    let want = decode(&grey).unwrap().raster;
    let o = EngineOptions {
        one_bit: true,
        ..EngineOptions::default()
    };
    // TIFF: thresholded at 128, no dithering, far smaller than the 8-bit file.
    let tif = render(&src, Format::Tiff, &o).unwrap();
    let t = decode(&tif).unwrap().raster;
    for (a, b) in want.data.chunks(3).zip(t.data.chunks(3)) {
        assert_eq!(b[0], if a[0] < 128 { 0 } else { 255 });
    }
    assert!(
        tif.len()
            < render(&src, Format::Tiff, &EngineOptions::default())
                .unwrap()
                .len()
                / 2
    );
    assert!(decode(&tif).unwrap().notices.contains(&"tiff.low_depth"));
    // PDF: a CCITT page.
    let pdf = render(&src, Format::Pdf, &o).unwrap();
    let p = &read_pages(&pdf).unwrap()[0];
    assert_eq!(p.codec, PageCodec::G4);
    assert_eq!(p.to_rgb().unwrap().2, t.data);
    // Everything else is refused, never written as 8-bit silently.
    for fmt in [Format::Png, Format::Jpeg, Format::Webp] {
        assert_eq!(
            render(&src, fmt, &o),
            Err(ErrKind::UnsupportedOutput),
            "{fmt:?}"
        );
    }
    // ... and so is a colour page.
    let colour = auto_crop_engine::outputs::render_outputs_enhanced(
        &src,
        &[whole()],
        &[Enhance::Original],
        Some(Format::Tiff),
        &o,
    );
    assert_eq!(colour.err(), Some(ErrKind::UnsupportedOutput));
}

// ------------------------------------------------------------------ the editor's Save format

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
fn the_save_format_applies_to_copies_and_a_replace_in_another_format_is_refused() {
    let png = png_rgb(80, 60);
    for (choice, ext, format) in [
        (SaveFormat::Tiff, "tif", Some(Format::Tiff)),
        (SaveFormat::Webp, "webp", Some(Format::Webp)),
        (SaveFormat::Jpeg, "jpg", Some(Format::Jpeg)),
        (SaveFormat::Pdf, "pdf", None),
        (SaveFormat::Auto, "png", Some(Format::Png)),
    ] {
        let e = env();
        let mut s = e.engine.settings();
        s.save_format = choice;
        e.engine.set_settings(s);
        let v = open_cropped(&e, "a.png", &png);
        // Replacing under another format is a conversion: refused, the file untouched.
        let out = e.engine.save_items(&[v.id], SaveTarget::Replace, "r", &nop);
        if choice == SaveFormat::Auto {
            assert!(out[0].ok, "{:?}", out[0]);
            continue;
        }
        assert_eq!(out[0].error, Some(ErrKind::UnsupportedOutput), "{choice:?}");
        assert_eq!(fs::read(e.dir.join("a.png")).unwrap(), png);
        // A copy takes the format.
        let out = e.engine.save_items(&[v.id], SaveTarget::Copy, "c", &nop);
        assert!(out[0].ok, "{choice:?}: {:?}", out[0]);
        let copy = e.dir.join("AutoCrop").join(format!("a.{ext}"));
        let bytes = fs::read(&copy).unwrap_or_else(|er| panic!("{choice:?}: {er}"));
        match format {
            Some(f) => {
                let d = decode(&bytes).unwrap();
                assert_eq!(d.format, f, "{choice:?}");
                assert!(d.raster.width < 80 && d.raster.height < 60);
            }
            None => {
                let pages = read_pages(&bytes).unwrap();
                assert_eq!((pages[0].width_px, pages[0].height_px), (64, 48));
            }
        }
        assert_eq!(fs::read(e.dir.join("a.png")).unwrap(), png);
    }
}

#[test]
fn a_tiff_source_copies_as_tiff_and_a_webp_source_as_webp_by_default() {
    let e = env();
    let v = open_cropped(&e, "a.tif", &tiff_rgb8(80, 60, &TiffOpts::default()));
    let out = e.engine.save_items(&[v.id], SaveTarget::Copy, "c", &nop);
    assert!(out[0].ok, "{:?}", out[0]);
    assert!(e.dir.join("AutoCrop/a.tif").exists());
    let v = open_cropped(
        &e,
        "b.webp",
        &auto_crop_codecs::fixtures::webp_lossless(80, 60),
    );
    let out = e.engine.save_items(&[v.id], SaveTarget::Copy, "c", &nop);
    assert!(out[0].ok, "{:?}", out[0]);
    let copy = fs::read(e.dir.join("AutoCrop/b.webp")).unwrap();
    assert_eq!(decode(&copy).unwrap().format, Format::Webp);
}

#[test]
fn the_webp_writer_is_what_the_engine_calls() {
    // The engine's WebP bytes are the codecs encoder's: lossless, with no lossy fallback.
    let r = decode(&png_rgb(40, 40)).unwrap().raster;
    let enc = encode_raster(
        &r,
        Format::Webp,
        &SourceMeta::default(),
        &EngineOptions::default(),
    )
    .unwrap();
    let direct = WebpEncoder::default()
        .encode(
            &CoreRaster::from_samples(40, 40, PixelFormat::Rgb8, Samples::U8(r.data.clone()))
                .unwrap()
                .view(),
            &OutputSpec::default(),
            &EncodeMeta::default(),
            &CancelToken::never(),
        )
        .unwrap();
    assert_eq!(enc.bytes, direct);
}
