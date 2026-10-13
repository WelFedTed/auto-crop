// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The Convert flow (ROADMAP M2.36, M6.47, M6.49, M6.50, M6.56): PNG, BMP, TIFF, WebP and JPEG
//! sources become JPEG or PNG, replacing the source after a verified backup or leaving it alone,
//! with the EXIF turn applied once, colours converted to sRGB (or kept), translucent pixels over
//! white, a restore that returns the source byte for byte, and the same fault-injection guarantees
//! as a save. HEIC and AVIF need the `heif` feature and are exercised by `heif.yml` in CI
//! (`tests/convert_heif.rs`).

use auto_crop_codecs::fixtures::{
    JpegSpec, TiffOpts, bmp_rgb24, exif_blob, fake_icc, jpeg_insert_segment, pattern, png_rgb,
    png_rgb16, png_rgba, png_with_exif, png_with_icc, psnr, tiff_rgb8, webp_animated,
    webp_lossless,
};
use auto_crop_codecs::{Format, decode};
use auto_crop_engine::colour::{ColourMode, srgb_icc};
use auto_crop_engine::convert::{
    ConvertError, ConvertOptions, ConvertRequest, ConvertTarget, Converted, Placement, Skip,
};
use auto_crop_engine::convert_batch::ConvertStatus;
use auto_crop_engine::group::{Fault, Step, pending_journals, stray_temps};
use auto_crop_engine::store::Store;
use auto_crop_engine::{
    AppPaths, DerivedAction, Engine, EngineOptions, ItemView, QualitySetting, RestoreMode,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

fn nop(_: ItemView) {}

fn old_mtime() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_500_000_000)
}

fn put(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let p = dir.join(name);
    fs::create_dir_all(dir).unwrap();
    fs::write(&p, bytes).unwrap();
    fs::File::options()
        .write(true)
        .open(&p)
        .unwrap()
        .set_modified(old_mtime())
        .unwrap();
    p
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
    let dir = root.path().join("photos");
    fs::create_dir_all(&dir).unwrap();
    Env {
        _root: root,
        paths,
        engine,
        dir,
    }
}

fn opts(target: ConvertTarget, placement: Placement) -> ConvertOptions {
    ConvertOptions {
        target,
        placement,
        ..ConvertOptions::default()
    }
}

fn convert(
    e: &Env,
    path: &Path,
    o: ConvertOptions,
    folder: Option<&Path>,
) -> Result<Converted, ConvertError> {
    let mut req = ConvertRequest::new(path, o, "run1", "test run");
    req.folder = folder;
    e.engine.convert_file(&req)
}

fn dims(p: &Path) -> (u32, u32) {
    let d = decode(&fs::read(p).unwrap()).unwrap();
    (d.raster.width, d.raster.height)
}

fn mtime(p: &Path) -> SystemTime {
    fs::metadata(p).unwrap().modified().unwrap()
}

// ------------------------------------------------------------------ formats to JPEG and PNG

fn sources() -> Vec<(&'static str, Vec<u8>, (u32, u32))> {
    let rgb = pattern(64, 48);
    vec![
        ("a.png", png_rgb(64, 48), (64, 48)),
        ("b.bmp", bmp_rgb24(64, 48, &rgb, false), (64, 48)),
        ("c.tif", tiff_rgb8(64, 48, &TiffOpts::default()), (64, 48)),
        ("d.webp", webp_lossless(64, 48), (64, 48)),
    ]
}

#[test]
fn png_bmp_tiff_and_webp_become_jpg_the_source_is_backed_up_and_restore_is_byte_identical() {
    for (name, bytes, size) in sources() {
        let e = env();
        let src = put(&e.dir, name, &bytes);
        let want = decode(&bytes).unwrap().raster;
        let done = convert(&e, &src, opts(ConvertTarget::Jpg, Placement::Replace), None)
            .unwrap_or_else(|x| panic!("{name}: {x:?}"));
        let out = src.with_extension("jpg");
        assert_eq!(done.output, out, "{name}");
        assert!(done.source_removed && !src.exists(), "{name}");
        assert_eq!(dims(&out), size, "{name}");
        assert_eq!(mtime(&out), old_mtime(), "{name}: the mtime is kept");
        let got = decode(&fs::read(&out).unwrap()).unwrap().raster;
        assert!(psnr(&want.data, &got.data) > 28.0, "{name}: JPEG fidelity");
        assert!(
            stray_temps(&e.dir).is_empty()
                && pending_journals(&Store::new(e.paths.backups_dir())) == 0
        );

        // The backup holds the source, byte for byte; restore returns it and keeps or removes
        // the converted file as asked.
        let id = done.backup_id.expect("a replace has a backup");
        let m = Store::new(e.paths.backups_dir()).read(&id).unwrap();
        assert_eq!(
            fs::read(Store::new(e.paths.backups_dir()).original_path(&m).unwrap()).unwrap(),
            bytes,
            "{name}"
        );
        let r = e
            .engine
            .restore_path(&out, RestoreMode::Auto, DerivedAction::Remove, &nop);
        assert!(r.ok, "{name}: {r:?}");
        assert_eq!(
            fs::read(&src).unwrap(),
            bytes,
            "{name}: restored byte for byte"
        );
        assert_eq!(mtime(&src), old_mtime(), "{name}");
        assert!(
            !out.exists(),
            "{name}: the converted file moved into the store"
        );
    }
}

#[test]
fn jpeg_tiff_webp_and_bmp_become_png_pixel_exact_when_the_source_is_lossless() {
    let rgb = pattern(40, 30);
    let jpeg = JpegSpec::new(40, 30).build();
    for (name, bytes, lossless) in [
        ("a.jpg", jpeg, false),
        ("b.tif", tiff_rgb8(40, 30, &TiffOpts::default()), true),
        ("c.webp", webp_lossless(40, 30), true),
        ("d.bmp", bmp_rgb24(40, 30, &rgb, false), true),
    ] {
        let e = env();
        let src = put(&e.dir, name, &bytes);
        let want = decode(&bytes).unwrap().raster;
        let done = convert(&e, &src, opts(ConvertTarget::Png, Placement::Replace), None).unwrap();
        let out = src.with_extension("png");
        assert_eq!(done.to, Format::Png);
        let got = decode(&fs::read(&out).unwrap()).unwrap().raster;
        if lossless {
            assert_eq!(got.data, want.data, "{name}: lossless to lossless is exact");
        }
        assert_eq!((got.width, got.height), (40, 30));
    }
}

#[test]
fn a_source_already_in_the_target_format_is_skipped_unless_reencode_is_asked() {
    let e = env();
    let src = put(&e.dir, "a.jpg", &JpegSpec::new(32, 32).build());
    let before = fs::read(&src).unwrap();
    match convert(&e, &src, opts(ConvertTarget::Jpg, Placement::Replace), None) {
        Err(ConvertError::Skipped(Skip::SameFormat, n)) => {
            assert_eq!(n, ["convert.same_format"]);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(fs::read(&src).unwrap(), before);
    assert!(e.engine.list_backups().runs.is_empty());

    // --reencode: the same name, swapped in place after a backup; restore returns the original.
    let mut o = opts(ConvertTarget::Jpg, Placement::Replace);
    o.reencode = true;
    o.quality = QualitySetting::Fixed { value: 40 };
    let done = convert(&e, &src, o, None).unwrap();
    assert_eq!(done.output, src);
    assert_ne!(fs::read(&src).unwrap(), before);
    assert!(done.backup_id.is_some());
    let r = e
        .engine
        .restore_path(&src, RestoreMode::Auto, DerivedAction::Keep, &nop);
    assert!(r.ok, "{r:?}");
    assert_eq!(fs::read(&src).unwrap(), before);
}

#[test]
fn a_png_called_jpg_is_converted_in_place_under_its_own_name() {
    // A file whose content is not what its extension says is simply converted (PLAN 3.1 rule 2).
    let e = env();
    let bytes = png_rgb(30, 20);
    let src = put(&e.dir, "photo.jpg", &bytes);
    let done = convert(&e, &src, opts(ConvertTarget::Jpg, Placement::Replace), None).unwrap();
    assert_eq!(done.output, src);
    assert_eq!(
        auto_crop_codecs::sniff(&fs::read(&src).unwrap()),
        Some(Format::Jpeg)
    );
    assert!(!e.dir.join("photo (2).jpg").exists());
    let r = e
        .engine
        .restore_path(&src, RestoreMode::Auto, DerivedAction::Remove, &nop);
    assert!(r.ok, "{r:?}");
    assert_eq!(fs::read(&src).unwrap(), bytes);
}

#[test]
fn a_taken_name_gets_a_number_and_nothing_is_overwritten() {
    let e = env();
    let src = put(&e.dir, "scan.png", &png_rgb(20, 20));
    let other = put(&e.dir, "scan.jpg", b"an unrelated file called scan.jpg");
    let done = convert(&e, &src, opts(ConvertTarget::Jpg, Placement::Replace), None).unwrap();
    assert_eq!(done.output, e.dir.join("scan (2).jpg"));
    assert_eq!(
        fs::read(&other).unwrap(),
        b"an unrelated file called scan.jpg"
    );
    assert!(!src.exists());
}

// ------------------------------------------------------------------ keep and folder

#[test]
fn keep_originals_writes_beside_the_source_and_touches_nothing_else() {
    let e = env();
    let bytes = png_rgb(50, 40);
    let src = put(&e.dir, "a.png", &bytes);
    let done = convert(&e, &src, opts(ConvertTarget::Jpg, Placement::Keep), None).unwrap();
    assert_eq!(done.output, e.dir.join("a.jpg"));
    assert!(done.backup_id.is_none() && !done.source_removed);
    assert_eq!(fs::read(&src).unwrap(), bytes);
    assert!(
        e.engine.list_backups().runs.is_empty(),
        "no backup is needed"
    );
    assert_eq!(mtime(&done.output), old_mtime());
    // Converting again does not overwrite: the PNG goes to `a (2).jpg`.
    let again = convert(&e, &src, opts(ConvertTarget::Jpg, Placement::Keep), None).unwrap();
    assert_eq!(again.output, e.dir.join("a (2).jpg"));
}

#[test]
fn a_folder_is_created_and_collisions_between_sources_are_numbered() {
    let e = env();
    let a = put(&e.dir, "same.png", &png_rgb(20, 20));
    let b = put(
        &e.dir,
        "same.bmp",
        &bmp_rgb24(20, 20, &pattern(20, 20), false),
    );
    let out_dir = e.dir.join("out").join("deeper");
    let o = opts(ConvertTarget::Jpg, Placement::Folder);
    let one = convert(&e, &a, o, Some(&out_dir)).unwrap();
    let two = convert(&e, &b, o, Some(&out_dir)).unwrap();
    assert_eq!(one.output, out_dir.join("same.jpg"));
    assert_eq!(two.output, out_dir.join("same (2).jpg"));
    assert!(a.exists() && b.exists());
    // Folder placement without a folder is an internal error, not a write next to the source.
    assert!(matches!(
        convert(&e, &a, o, None),
        Err(ConvertError::Failed(auto_crop_engine::ErrKind::Internal, _))
    ));
}

#[test]
fn a_dry_run_reads_the_header_and_writes_nothing() {
    let e = env();
    let src = put(&e.dir, "a.png", &png_rgb(20, 20));
    let before: Vec<_> = fs::read_dir(&e.dir)
        .unwrap()
        .flatten()
        .map(|x| x.path())
        .collect();
    let mut req = ConvertRequest::new(&src, opts(ConvertTarget::Jpg, Placement::Replace), "r", "n");
    req.dry_run = true;
    let done = e.engine.convert_file(&req).unwrap();
    assert!(done.dry_run && done.output == e.dir.join("a.jpg"));
    let after: Vec<_> = fs::read_dir(&e.dir)
        .unwrap()
        .flatten()
        .map(|x| x.path())
        .collect();
    assert_eq!(before, after);
    assert!(e.engine.list_backups().runs.is_empty());
}

// ------------------------------------------------------------------ never replaced

#[test]
fn a_source_with_several_frames_is_never_replaced_but_keep_converts_its_first_frame() {
    let e = env();
    let animated = webp_animated(24, 16, 3);
    let multipage = tiff_rgb8(
        24,
        16,
        &TiffOpts {
            extra_pages: 2,
            ..TiffOpts::default()
        },
    );
    for (name, bytes, notice) in [
        ("anim.webp", animated, "anim.first_frame_only"),
        ("pages.tif", multipage, "tiff.multi_page"),
    ] {
        let src = put(&e.dir, name, &bytes);
        match convert(&e, &src, opts(ConvertTarget::Png, Placement::Replace), None) {
            Err(ConvertError::Skipped(Skip::NotReplaceable(n), notices)) => {
                assert_eq!(n, notice, "{name}");
                assert_eq!(notices, [notice]);
            }
            other => panic!("{name}: {other:?}"),
        }
        assert_eq!(fs::read(&src).unwrap(), bytes, "{name}: byte-identical");
        let kept = convert(&e, &src, opts(ConvertTarget::Png, Placement::Keep), None).unwrap();
        assert!(
            kept.notices.iter().any(|n| n == notice),
            "{name}: {:?}",
            kept.notices
        );
        assert!(src.exists());
    }
    assert!(e.engine.list_backups().runs.is_empty());
}

#[test]
fn files_this_build_cannot_read_are_skipped_not_failed() {
    let e = env();
    let junk = put(&e.dir, "a.png", b"this is not an image at all");
    assert!(matches!(
        convert(
            &e,
            &junk,
            opts(ConvertTarget::Jpg, Placement::Replace),
            None
        ),
        Err(ConvertError::Skipped(Skip::Unsupported, _))
    ));
    let gif = put(&e.dir, "b.gif", &auto_crop_codecs::fixtures::gif_stub());
    assert!(matches!(
        convert(&e, &gif, opts(ConvertTarget::Jpg, Placement::Replace), None),
        Err(ConvertError::Skipped(Skip::Unsupported, _))
    ));
    // A corrupt file of a known format is a failure, and the source stays.
    let mut bad = png_rgb(30, 30);
    bad.truncate(bad.len() / 2);
    let src = put(&e.dir, "c.png", &bad);
    assert!(matches!(
        convert(&e, &src, opts(ConvertTarget::Jpg, Placement::Replace), None),
        Err(ConvertError::Failed(_, _))
    ));
    assert_eq!(fs::read(&src).unwrap(), bad);
    if !Format::Heic.is_decodable() {
        let heic = put(&e.dir, "d.heic", &auto_crop_codecs::fixtures::heic_stub());
        assert!(matches!(
            convert(
                &e,
                &heic,
                opts(ConvertTarget::Jpg, Placement::Replace),
                None
            ),
            Err(ConvertError::Skipped(Skip::HeifUnavailable, _))
        ));
    }
}

// ------------------------------------------------------------------ orientation, alpha, depth

#[test]
fn the_exif_turn_is_applied_once_and_the_output_says_upright() {
    for (target, name) in [(ConvertTarget::Png, "png"), (ConvertTarget::Jpg, "jpg")] {
        // A 40 x 24 JPEG that says "turn 90 degrees": the output is 24 x 40 and says orientation 1.
        let e = env();
        let blob = exif_blob(6, true);
        let mut payload = b"Exif\0\0".to_vec();
        payload.extend_from_slice(&blob);
        let jpeg = jpeg_insert_segment(&JpegSpec::new(40, 24).build(), 0xE1, &payload);
        let src = put(&e.dir, "turned.jpg", &jpeg);
        assert_eq!(dims(&src), (24, 40), "the decode already turned it");
        let mut o = opts(target, Placement::Keep);
        o.reencode = true;
        let done = convert(&e, &src, o, None).unwrap();
        assert_eq!(done.dims, (24, 40), "{name}");
        let out = fs::read(&done.output).unwrap();
        let d = decode(&out).unwrap();
        assert_eq!(
            (d.raster.width, d.raster.height),
            (24, 40),
            "{name}: turned once"
        );
        assert_eq!(d.exif_orientation, 1, "{name}: the output says upright");
    }
    // A PNG with an eXIf orientation 6, to JPEG.
    let e = env();
    let src = put(&e.dir, "p.png", &png_with_exif(30, 20, 6));
    let done = convert(&e, &src, opts(ConvertTarget::Jpg, Placement::Keep), None).unwrap();
    assert_eq!(done.dims, (20, 30));
    let d = decode(&fs::read(&done.output).unwrap()).unwrap();
    assert_eq!(
        (d.raster.width, d.raster.height, d.exif_orientation),
        (20, 30, 1)
    );
}

#[test]
fn translucent_pixels_are_flattened_on_white_with_a_notice() {
    let e = env();
    let src = put(&e.dir, "alpha.png", &png_rgba(20, 20));
    let done = convert(&e, &src, opts(ConvertTarget::Jpg, Placement::Keep), None).unwrap();
    assert!(
        done.notices.iter().any(|n| n == "alpha.flattened"),
        "{:?}",
        done.notices
    );
    let flat = decode(&fs::read(&done.output).unwrap()).unwrap().raster;
    let orig = decode(&png_rgba(20, 20)).unwrap().raster;
    // Every fifth pixel is half transparent: it is the pattern colour halfway to white.
    let (x, y) = (0u32, 0u32);
    let c = orig.pixel(x, y);
    let want: Vec<u8> = c
        .iter()
        .map(|v| ((u32::from(*v) + 255) / 2) as u8)
        .collect();
    let got = flat.pixel(x, y);
    for i in 0..3 {
        assert!(
            (i32::from(got[i]) - i32::from(want[i])).abs() <= 6,
            "channel {i}: {got:?} vs {want:?}"
        );
    }
    // And to PNG too (the PNG writer here has no alpha channel).
    let p = convert(&e, &src, opts(ConvertTarget::Png, Placement::Keep), None);
    // A PNG source to PNG is the same format: skipped without --reencode.
    assert!(matches!(p, Err(ConvertError::Skipped(Skip::SameFormat, _))));
}

#[test]
fn sixteen_bit_sources_are_reduced_with_a_notice() {
    let e = env();
    let src = put(&e.dir, "deep.png", &png_rgb16(24, 24));
    let done = convert(&e, &src, opts(ConvertTarget::Jpg, Placement::Replace), None).unwrap();
    assert!(
        done.notices.iter().any(|n| n == "depth.reduced_to_8"),
        "{:?}",
        done.notices
    );
    assert!(done.output.exists());
}

// ------------------------------------------------------------------ colour

fn p3_icc() -> Vec<u8> {
    // The encoder stamps the profile with the current time (to the minute), so two calls that
    // straddle a minute boundary give different bytes: build it once per process.
    static P3: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    P3.get_or_init(|| moxcms::ColorProfile::new_display_p3().encode().unwrap())
        .clone()
}

#[test]
fn a_display_p3_source_is_converted_to_srgb_by_default_and_kept_when_asked() {
    // A saturated P3 green in an 8-bit PNG tagged Display P3.
    let mut rgb = PngRaw::new(16, 16, [30, 220, 60]);
    rgb.icc = Some(p3_icc());
    let png = rgb.build();
    let e = env();
    let src = put(&e.dir, "p3.png", &png);
    let want_icc = srgb_icc().to_vec();

    let done = convert(&e, &src, opts(ConvertTarget::Jpg, Placement::Keep), None).unwrap();
    assert!(done.notices.iter().any(|n| n == "colour.converted_to_srgb"));
    let d = decode(&fs::read(&done.output).unwrap()).unwrap();
    assert_eq!(
        d.icc.as_deref(),
        Some(want_icc.as_slice()),
        "the output embeds sRGB"
    );
    let px = d.raster.pixel(8, 8);
    assert_ne!(px, [30, 220, 60], "the values moved: {px:?}");
    // P3 (30, 220, 60) is inside sRGB's gamut only barely: green goes up, red and blue go down.
    assert!(px[1] >= 220 - 8, "{px:?}");

    // Preserve: pixels as they are (within JPEG error), the source profile byte-exact.
    let mut o = opts(ConvertTarget::Jpg, Placement::Keep);
    o.colour = ColourMode::Preserve;
    let kept = convert(&e, &src, o, None).unwrap();
    assert!(!kept.notices.iter().any(|n| n.starts_with("colour.")));
    let d = decode(&fs::read(&kept.output).unwrap()).unwrap();
    assert_eq!(d.icc.as_deref(), Some(p3_icc().as_slice()));
    let px = d.raster.pixel(8, 8);
    for i in 0..3 {
        assert!((i32::from(px[i]) - [30, 220, 60][i]).abs() <= 6, "{px:?}");
    }
}

#[test]
fn an_srgb_profile_and_no_profile_leave_the_colours_and_the_profile_alone() {
    let e = env();
    let icc = srgb_icc().to_vec();
    let src = put(&e.dir, "s.png", &png_with_icc(16, 16, &icc));
    let done = convert(&e, &src, opts(ConvertTarget::Jpg, Placement::Keep), None).unwrap();
    let d = decode(&fs::read(&done.output).unwrap()).unwrap();
    assert_eq!(d.icc.as_deref(), Some(icc.as_slice()), "byte-exact");
    assert!(!done.notices.iter().any(|n| n.starts_with("colour.")));
    let plain = put(&e.dir, "n.png", &png_rgb(16, 16));
    let done = convert(&e, &plain, opts(ConvertTarget::Jpg, Placement::Keep), None).unwrap();
    assert_eq!(decode(&fs::read(&done.output).unwrap()).unwrap().icc, None);
    // An unusable profile keeps pixels and profile and says so.
    let junk = fake_icc(500);
    let src = put(&e.dir, "j.png", &png_with_icc(16, 16, &junk));
    let done = convert(&e, &src, opts(ConvertTarget::Jpg, Placement::Keep), None).unwrap();
    assert!(
        done.notices.iter().any(|n| n == "colour.profile_unusable"),
        "{:?}",
        done.notices
    );
    let d = decode(&fs::read(&done.output).unwrap()).unwrap();
    assert_eq!(d.icc.as_deref(), Some(junk.as_slice()));
}

// ------------------------------------------------------------------ metadata

/// The EXIF of a camera: Orientation 6, an Exif IFD with a ColorSpace of "uncalibrated" and the
/// pixel dimensions, and a GPS IFD with a marker that must vanish with `strip_location`.
fn camera_exif() -> Vec<u8> {
    const GPS: [u8; 8] = [0xAB, 0xCD, 0xAB, 0xCD, 0xAB, 0xCD, 0xAB, 0xCD];
    let mut v: Vec<u8> = b"II".to_vec();
    let p16 = |v: &mut Vec<u8>, x: u16| v.extend_from_slice(&x.to_le_bytes());
    let p32 = |v: &mut Vec<u8>, x: u32| v.extend_from_slice(&x.to_le_bytes());
    p16(&mut v, 42);
    p32(&mut v, 8);
    // IFD0 at 8: Orientation, ExifIFD, GPS (3 entries: 2 + 36 + 4 = 42 bytes).
    let exif_ifd = 8 + 42;
    let gps_ifd = exif_ifd + 2 + 3 * 12 + 4;
    p16(&mut v, 3);
    for (tag, ty, val) in [
        (0x0112u16, 3u16, 6u32),
        (0x8769, 4, exif_ifd),
        (0x8825, 4, gps_ifd),
    ] {
        p16(&mut v, tag);
        p16(&mut v, ty);
        p32(&mut v, 1);
        p32(&mut v, val);
    }
    p32(&mut v, 0);
    assert_eq!(v.len() as u32, exif_ifd);
    // Exif IFD: ColorSpace (SHORT 65535), PixelX, PixelY.
    p16(&mut v, 3);
    for (tag, ty, val) in [
        (0xA001u16, 3u16, 65535u32),
        (0xA002, 4, 4000),
        (0xA003, 4, 3000),
    ] {
        p16(&mut v, tag);
        p16(&mut v, ty);
        p32(&mut v, 1);
        p32(&mut v, val);
    }
    p32(&mut v, 0);
    assert_eq!(v.len() as u32, gps_ifd);
    // GPS IFD: one RATIONAL[3] entry pointing at the marker.
    p16(&mut v, 1);
    p16(&mut v, 0x0002);
    p16(&mut v, 5);
    p32(&mut v, 3);
    p32(&mut v, gps_ifd + 2 + 12 + 4);
    p32(&mut v, 0);
    for _ in 0..3 {
        v.extend_from_slice(&GPS);
    }
    v
}

fn has(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn exif_is_carried_with_the_orientation_reset_the_colour_space_set_and_the_location_strippable() {
    let gps = [0xABu8, 0xCD, 0xAB, 0xCD, 0xAB, 0xCD, 0xAB, 0xCD];
    let mut payload = b"Exif\0\0".to_vec();
    payload.extend_from_slice(&camera_exif());
    let jpeg = jpeg_insert_segment(&JpegSpec::new(40, 24).build(), 0xE1, &payload);

    for (target, strip) in [
        (ConvertTarget::Jpg, false),
        (ConvertTarget::Jpg, true),
        (ConvertTarget::Png, false),
        (ConvertTarget::Png, true),
    ] {
        let e = env();
        let src = put(&e.dir, "cam.jpg", &jpeg);
        let mut o = opts(target, Placement::Keep);
        o.reencode = true;
        o.strip_location = strip;
        let done = convert(&e, &src, o, None).unwrap();
        let out = fs::read(&done.output).unwrap();
        let label = format!("{target:?} strip={strip}");
        // The EXIF survives (the marker of GPS is in it unless stripped) and says upright.
        let exif = match target {
            ConvertTarget::Jpg => auto_crop_codecs::jpeg_meta::read_meta(&out).unwrap().exif,
            ConvertTarget::Png => auto_crop_codecs::extract_exif(
                &out,
                Format::Png,
                &auto_crop_codecs::DecodeLimits::default(),
            ),
        }
        .unwrap_or_else(|| panic!("{label}: EXIF carried"));
        assert_eq!(
            auto_crop_codecs::exif_orientation(&exif),
            Some(1),
            "{label}"
        );
        assert_eq!(
            has(&exif, &gps),
            !strip,
            "{label}: GPS present exactly when not stripped"
        );
    }
}

#[test]
fn the_exif_color_space_is_set_to_srgb_when_the_pixels_were_converted() {
    let mut payload = b"Exif\0\0".to_vec();
    payload.extend_from_slice(&camera_exif());
    let mut spec = JpegSpec::new(40, 24);
    spec.icc = Some(p3_icc());
    let jpeg = jpeg_insert_segment(&spec.build(), 0xE1, &payload);
    let e = env();
    let src = put(&e.dir, "p3.jpg", &jpeg);
    let mut o = opts(ConvertTarget::Jpg, Placement::Keep);
    o.reencode = true;
    let done = convert(&e, &src, o, None).unwrap();
    let out = fs::read(&done.output).unwrap();
    let exif = auto_crop_codecs::jpeg_meta::read_meta(&out)
        .unwrap()
        .exif
        .unwrap();
    // ColorSpace (0xA001) was 65535; now 1. Find the entry: tag bytes 01 A0 then type 3, count 1.
    let at = exif
        .windows(4)
        .position(|w| w == [0x01, 0xA0, 0x03, 0x00])
        .expect("the ColorSpace entry");
    assert_eq!(u16::from_le_bytes([exif[at + 8], exif[at + 9]]), 1);
}

// ------------------------------------------------------------------ the batch and the session

#[test]
fn a_batch_is_listed_previewed_run_and_restored() {
    let e = env();
    let rgb = pattern(40, 30);
    put(&e.dir, "a.png", &png_rgb(40, 30));
    put(&e.dir, "b.bmp", &bmp_rgb24(40, 30, &rgb, false));
    put(&e.dir, "c.jpg", &JpegSpec::new(40, 30).build());
    put(&e.dir, "d.webp", &webp_lossless(40, 30));
    put(&e.dir, "notes.txt", b"not an image");
    put(&e.dir, "bad.png", b"junk");
    let heif = Format::Heic.is_decodable();
    let stub = auto_crop_codecs::fixtures::heic_stub();
    let heic = (!heif).then(|| put(&e.dir, "e.heic", &stub));
    let listed = 5 + usize::from(!heif);
    let added = e.engine.convert_add(std::slice::from_ref(&e.dir), false);
    assert_eq!(added.added, listed, "{added:?}");
    assert_eq!(
        e.engine
            .convert_add(std::slice::from_ref(&e.dir), false)
            .duplicates,
        listed
    );

    let o = opts(ConvertTarget::Jpg, Placement::Replace);
    let p = e.engine.convert_preview(&o);
    assert_eq!((p.total, p.same_format), (listed, 1), "{p:?}");
    assert_eq!(p.heif_available, heif);
    // png, bmp, webp, the junk png (known only by its name here: unknown content) and the heic.
    assert_eq!(p.cannot, 1 + usize::from(!heif), "{p:?}");
    assert_eq!(p.will_convert + p.same_format + p.cannot, p.total);
    assert!(
        p.by_format
            .iter()
            .any(|f| f.format == "jpeg" && f.count == 1)
    );
    if !heif {
        assert_eq!(p.heif_unavailable, 1);
        assert!(p.cannot_files.iter().any(|c| c.code == "HEIF_UNAVAILABLE"));
    }

    let seen = std::sync::Mutex::new(Vec::new());
    let summary = e.engine.convert_run(&o, "Convert test", 2, &|pr| {
        seen.lock().unwrap().push((pr.done, pr.total));
    });
    assert_eq!(summary.converted, 3, "{summary:#?}");
    assert_eq!(summary.skipped, 2 + usize::from(!heif), "{summary:#?}");
    assert_eq!(summary.failed, 0, "{summary:#?}");
    let mut seen = seen.into_inner().unwrap();
    seen.sort();
    assert_eq!(seen.len(), summary.results.len());
    assert!(seen.iter().all(|(_, t)| *t == summary.results.len()));
    for n in ["a.jpg", "b.jpg", "d.jpg"] {
        assert!(e.dir.join(n).exists(), "{n}");
    }
    for n in ["a.png", "b.bmp", "d.webp"] {
        assert!(!e.dir.join(n).exists(), "{n} was replaced");
    }
    if let Some(h) = heic {
        assert_eq!(fs::read(&h).unwrap(), stub);
    }
    // One backup run, three originals, restorable by the ids the results carry.
    let backups = e.engine.list_backups();
    assert_eq!(backups.runs.len(), 1);
    assert_eq!(backups.runs[0].id, summary.run_id);
    assert_eq!(backups.runs[0].file_count, 3);
    assert!(backups.runs[0].files.iter().all(|f| f.derived.len() == 1));
    let converted: Vec<_> = summary
        .results
        .iter()
        .filter(|r| r.status == ConvertStatus::Converted)
        .collect();
    let restored = e.engine.restore_file_derived(
        &format!("{}/0", converted[0].backup_id.as_ref().unwrap()),
        RestoreMode::Auto,
        DerivedAction::Remove,
        &nop,
    );
    assert!(restored.ok, "{restored:?}");

    // A second run converts nothing more: the converted files are done, the rest unchanged.
    let again = e.engine.convert_run(&o, "again", 2, &|_| {});
    assert_eq!(again.converted, 0);
    assert!(e.engine.convert_output_dir(None).is_some());
}

#[test]
fn cancelling_a_run_leaves_no_file_half_converted() {
    let e = env();
    for i in 0..6 {
        put(&e.dir, &format!("f{i}.png"), &png_rgb(300, 200));
    }
    e.engine.convert_add(std::slice::from_ref(&e.dir), false);
    let engine = e.engine.clone();
    let o = opts(ConvertTarget::Jpg, Placement::Keep);
    let summary = e.engine.convert_run(&o, "cancel", 1, &move |p| {
        if p.done == 2 {
            engine.convert_cancel();
        }
    });
    assert!(summary.cancelled);
    assert!(
        summary.converted >= 2 && summary.converted < 6,
        "{summary:#?}"
    );
    assert!(
        summary
            .results
            .iter()
            .filter(|r| r.status == ConvertStatus::Skipped)
            .all(|r| r.code.as_deref() == Some("CANCELLED"))
    );
    assert!(stray_temps(&e.dir).is_empty());
    let jpgs = fs::read_dir(&e.dir)
        .unwrap()
        .flatten()
        .filter(|x| x.path().extension().is_some_and(|e| e == "jpg"))
        .count();
    assert_eq!(jpgs, summary.converted);
}

// ------------------------------------------------------------------ fault injection

fn every_step() -> Vec<Step> {
    vec![
        Step::Planned,
        Step::BackupCreated,
        Step::JournalWriting,
        Step::TempWritten(0),
        Step::TempVerified(0),
        Step::JournalCommitting,
        Step::SourceRestat,
        Step::BeforeRename(0),
        Step::AfterRename(0),
        Step::RenamesDone,
        Step::DirSynced,
        Step::BeforeUnlink,
        Step::AfterUnlink,
        Step::ManifestSaved,
        Step::JournalRemoved,
    ]
}

/// A crash at any step of a replace-conversion leaves the source alone, or the new file (with the
/// source beside it if it could not be removed), and the source is always restorable.
#[test]
fn a_crash_at_every_step_of_a_replace_conversion_is_recovered_and_restorable() {
    for step in every_step() {
        let e = env();
        let bytes = tiff_rgb8(48, 32, &TiffOpts::default());
        let src = put(&e.dir, "scan.tif", &bytes);
        let o = opts(ConvertTarget::Jpg, Placement::Replace);
        let req = ConvertRequest::new(&src, o, "r", "n");
        let _ = e.engine.convert_file_with_faults(&req, &move |s: &Step| {
            if *s == step {
                Fault::Crash
            } else {
                Fault::Pass
            }
        });
        let engine = Engine::new(e.paths.clone()); // the next start recovers
        let out = e.dir.join("scan.jpg");
        let committed = matches!(
            step,
            Step::JournalCommitting
                | Step::SourceRestat
                | Step::BeforeRename(_)
                | Step::AfterRename(_)
                | Step::RenamesDone
                | Step::DirSynced
                | Step::BeforeUnlink
                | Step::AfterUnlink
                | Step::ManifestSaved
                | Step::JournalRemoved
        );
        if out.exists() {
            assert!(
                decode(&fs::read(&out).unwrap()).is_ok(),
                "{step:?}: a whole JPEG"
            );
        }
        if src.exists() {
            assert_eq!(
                fs::read(&src).unwrap(),
                bytes,
                "{step:?}: the source is intact"
            );
        } else {
            assert!(
                out.exists(),
                "{step:?}: if the source is gone the JPEG is there"
            );
        }
        if !committed {
            assert!(!out.exists() && src.exists(), "{step:?}: rolled back");
        }
        assert!(stray_temps(&e.dir).is_empty(), "{step:?}");
        assert_eq!(
            pending_journals(&Store::new(e.paths.backups_dir())),
            0,
            "{step:?}"
        );
        if out.exists() && committed {
            let r = engine.restore_path(&out, RestoreMode::Auto, DerivedAction::Remove, &nop);
            assert!(r.ok, "{step:?}: {r:?}");
            assert_eq!(fs::read(&src).unwrap(), bytes, "{step:?}: restored");
        }
    }
}

/// The same for the in-place swap (`--reencode`): at every step the file is the old bytes or a
/// whole new JPEG, never a mix, and the original is in the backup once the swap happened.
#[test]
fn a_crash_at_every_step_of_an_in_place_reencode_is_recovered() {
    for step in every_step() {
        let e = env();
        let bytes = JpegSpec::new(64, 48).build();
        let src = put(&e.dir, "a.jpg", &bytes);
        let mut o = opts(ConvertTarget::Jpg, Placement::Replace);
        o.reencode = true;
        o.quality = QualitySetting::Fixed { value: 30 };
        let req = ConvertRequest::new(&src, o, "r", "n");
        let _ = e.engine.convert_file_with_faults(&req, &move |s: &Step| {
            if *s == step {
                Fault::Crash
            } else {
                Fault::Pass
            }
        });
        let engine = Engine::new(e.paths.clone());
        let now = fs::read(&src).unwrap();
        assert!(
            now == bytes || decode(&now).is_ok(),
            "{step:?}: old bytes or a whole JPEG"
        );
        assert!(stray_temps(&e.dir).is_empty(), "{step:?}");
        assert_eq!(
            pending_journals(&Store::new(e.paths.backups_dir())),
            0,
            "{step:?}"
        );
        if now != bytes {
            let r = engine.restore_path(&src, RestoreMode::Auto, DerivedAction::Keep, &nop);
            assert!(r.ok, "{step:?}: {r:?}");
            assert_eq!(fs::read(&src).unwrap(), bytes, "{step:?}: restored");
        }
    }
}

/// A copy (keep or folder) has no backup and nothing that exists changes: a crash leaves the source
/// alone and at most a whole new file.
#[test]
fn a_crash_while_keeping_the_original_never_touches_it() {
    for step in every_step() {
        let e = env();
        let bytes = png_rgb(48, 32);
        let src = put(&e.dir, "a.png", &bytes);
        let req = ConvertRequest::new(&src, opts(ConvertTarget::Jpg, Placement::Keep), "r", "n");
        let _ = e.engine.convert_file_with_faults(&req, &move |s: &Step| {
            if *s == step {
                Fault::Crash
            } else {
                Fault::Pass
            }
        });
        let _engine = Engine::new(e.paths.clone());
        assert_eq!(fs::read(&src).unwrap(), bytes, "{step:?}");
        let out = e.dir.join("a.jpg");
        if out.exists() {
            assert!(decode(&fs::read(&out).unwrap()).is_ok(), "{step:?}");
        }
        assert!(stray_temps(&e.dir).is_empty(), "{step:?}");
        assert!(e.engine.list_backups().runs.is_empty(), "{step:?}");
    }
}

/// A failure (not a crash) at each step reports an error and leaves the world as it was.
#[test]
fn a_failure_at_each_step_leaves_the_source_and_no_stray_file() {
    for step in every_step() {
        if step.after_commit() {
            continue; // after the commit point a failure never rolls the set back
        }
        let e = env();
        let bytes = png_rgb(48, 32);
        let src = put(&e.dir, "a.png", &bytes);
        let req = ConvertRequest::new(&src, opts(ConvertTarget::Jpg, Placement::Replace), "r", "n");
        let r = e.engine.convert_file_with_faults(&req, &move |s: &Step| {
            if *s == step {
                Fault::Fail(auto_crop_engine::ErrKind::DiskFull)
            } else {
                Fault::Pass
            }
        });
        if r.is_ok() {
            continue; // the hook's step does not exist on this path
        }
        assert_eq!(fs::read(&src).unwrap(), bytes, "{step:?}");
        assert!(!e.dir.join("a.jpg").exists(), "{step:?}");
        assert!(stray_temps(&e.dir).is_empty(), "{step:?}");
    }
}

// ------------------------------------------------------------------ the options round trip

#[test]
fn the_options_are_camel_case_json_with_defaults_for_missing_keys() {
    let o: ConvertOptions = serde_json::from_str(
        r#"{"target":"png","colour":"preserve","quality":{"kind":"fixed","value":70},"stripLocation":true,"placement":"folder","reencode":true}"#,
    )
    .unwrap();
    assert_eq!(o.target, ConvertTarget::Png);
    assert_eq!(o.colour, ColourMode::Preserve);
    assert_eq!(o.placement, Placement::Folder);
    assert!(o.strip_location && o.reencode);
    assert_eq!(o.quality, QualitySetting::Fixed { value: 70 });
    let d: ConvertOptions = serde_json::from_str("{}").unwrap();
    assert_eq!(d, ConvertOptions::default());
    assert_eq!(d.target, ConvertTarget::Jpg);
    assert_eq!(d.placement, Placement::Replace);
    assert_eq!(d.colour, ColourMode::Srgb);
    let _ = EngineOptions::default();
}

// ------------------------------------------------------------------ helpers

/// A PNG of one flat colour with an optional `iCCP` profile.
struct PngRaw {
    w: u32,
    h: u32,
    rgb: [u8; 3],
    icc: Option<Vec<u8>>,
}

impl PngRaw {
    fn new(w: u32, h: u32, rgb: [u8; 3]) -> Self {
        Self {
            w,
            h,
            rgb,
            icc: None,
        }
    }

    fn build(&self) -> Vec<u8> {
        let mut spec = auto_crop_codecs::fixtures::PngSpec::rgb8(self.w, self.h);
        spec.data = (0..self.w * self.h).flat_map(|_| self.rgb).collect();
        if let Some(p) = &self.icc {
            let mut d = b"t\0\0".to_vec();
            d.extend(auto_crop_codecs::fixtures::zlib(p));
            spec.before_idat.push((*b"iCCP", d));
        }
        spec.build()
    }
}
