// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Scan to PDF through the engine (ROADMAP M11.11 `ManyToOne`, M11.31, M11.36 to M11.38):
//! three crops (a straight one cut from a JPEG, a curved one, an enhanced one) make a three-page
//! PDF in the order given, with the sizes the density says and the enhanced page pixel-equal to the
//! same crop written as a PNG; an existing file is never replaced; a failure at any step, or a
//! cancel, leaves no output and no temp; damage found by the parse-back fails the save; the
//! originals are not touched.

use auto_crop_codecs::encoders::pdf::{PageCodec, read_pages, strip_jpeg};
use auto_crop_codecs::fixtures::{photo, png_rgb};
use auto_crop_codecs::{Format, decode};
use auto_crop_core::{CancelToken, Enhance, EnhanceMode, Pt};
use auto_crop_engine::enhance::quad_enhances;
use auto_crop_engine::group::stray_temps;
use auto_crop_engine::run_pdf::{PdfRunOptions, PdfSelect, PdfStep};
use auto_crop_engine::{AppPaths, Edit, Engine, ErrKind, ItemView, NoSplit};
use auto_crop_imgproc::Raster;
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

fn quad(x0: f64, y0: f64, x1: f64, y1: f64) -> Edit {
    Edit {
        quad: [
            Pt::new(x0, y0),
            Pt::new(x1, y0),
            Pt::new(x1, y1),
            Pt::new(x0, y1),
        ],
        quarter_turns: 0,
        fine_deg: 0.0,
    }
}

/// Opens `bytes` as `name` and gives it the crop `edit`.
fn open(e: &Env, name: &str, bytes: &[u8], edit: &Edit) -> ItemView {
    let path = e.dir.join(name);
    fs::write(&path, bytes).unwrap();
    let id = e.engine.open_paths(&[path], false).ids[0];
    e.engine.analyse(id).unwrap();
    e.engine.set_edit(id, edit, false, "crop").unwrap();
    e.engine.item_view(id).unwrap()
}

fn jpeg_source(w: u32, h: u32, seed: u64) -> Vec<u8> {
    auto_crop_codecs::encode(
        &Raster::from_raw(w, h, photo(w, h, seed)).unwrap(),
        Format::Jpeg,
        90,
        None,
    )
    .unwrap()
}

fn page_png() -> Vec<u8> {
    use auto_crop_imgproc::enhance::testpages::{Light, PageSpec, lit, page};
    let p = page(&PageSpec::document(5).sized(300, 420, 3));
    let r = lit(&p.truth, Light::Tilt { dx: 0.5, dy: 0.1 }, [1.0; 3], 2.0, 5);
    auto_crop_codecs::encode(&r, Format::Png, 90, None).unwrap()
}

fn sel(v: &ItemView) -> PdfSelect {
    PdfSelect {
        item: v.id,
        crop: None,
    }
}

fn no_progress(_: auto_crop_engine::run_pdf::PdfRunProgress) {}

fn never() -> CancelToken {
    CancelToken::never()
}

/// The three kinds of crop: (straight JPEG, curved PNG, enhanced document page).
fn three(e: &Env) -> (ItemView, ItemView, ItemView) {
    let a = open(
        e,
        "a.jpg",
        &jpeg_source(400, 300, 3),
        &quad(0.1, 0.1, 0.9, 0.9),
    );
    let b = open(e, "b.png", &png_rgb(320, 240), &quad(0.05, 0.1, 0.95, 0.9));
    e.engine.curve_from_quad(b.id, b.crops[0].id).unwrap();
    let c = open(e, "c.png", &page_png(), &quad(0.0, 0.0, 1.0, 1.0));
    e.engine
        .set_enhance(
            c.id,
            c.crops[0].id,
            EnhanceMode::Document,
            1.0,
            false,
            "",
            None,
        )
        .unwrap();
    (a, b, c)
}

fn tree_hash(dir: &Path) -> Vec<(String, String)> {
    let mut v: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .flatten()
        .filter(|f| f.path().is_file())
        .map(|f| {
            (
                f.file_name().to_string_lossy().into_owned(),
                auto_crop_engine::util::blake3_hex(&fs::read(f.path()).unwrap()),
            )
        })
        .collect();
    v.sort();
    v
}

#[test]
fn three_crops_make_a_three_page_pdf_in_the_order_given() {
    let e = env();
    let (a, b, c) = three(&e);
    let before = tree_hash(&e.dir);
    let out = e.dir.join("Scans.pdf");
    // The order of the selection is the order of the pages (not the order they were opened).
    let saved = e
        .engine
        .save_run_pdf(
            &[sel(&c), sel(&a), sel(&b)],
            &out,
            &PdfRunOptions::default(),
            &never(),
            &no_progress,
        )
        .unwrap();
    assert_eq!(saved.path, out);
    assert_eq!((saved.pages, saved.page_sizes_pt.len()), (3, 3));
    assert!(!saved.notices.iter().any(|n| n == "pdf.renamed"));
    let bytes = fs::read(&out).unwrap();
    assert_eq!(saved.bytes, bytes.len() as u64);
    let pages = read_pages(&bytes).expect("the repo's verifier reads it back");
    assert_eq!(pages.len(), 3);
    // Page 1 is the enhanced document page: one grey Flate page, the crop whole (300 x 420) at
    // the nominal 300 dpi.
    let (w, h) = (pages[0].width_px, pages[0].height_px);
    assert_eq!((pages[0].codec, pages[0].channels), (PageCodec::Flate, 1));
    assert_eq!((w, h), (300, 420));
    assert_eq!(pages[0].media_box, (72.0, 100.8));
    // Pixel equality with the same crop and enhancement written as a PNG.
    let (_, state) = (c.id, e.engine.edit_state(c.id).unwrap());
    let (quads, enhances): (Vec<_>, Vec<_>) = quad_enhances(&state).into_iter().unzip();
    let png = auto_crop_engine::outputs::render_outputs_enhanced(
        &fs::read(e.dir.join("c.png")).unwrap(),
        &quads,
        &enhances,
        Some(Format::Png),
        &auto_crop_engine::EngineOptions::default(),
    )
    .unwrap()
    .remove(0)
    .bytes;
    let want = decode(&png).unwrap().raster;
    let (pw, ph, rgb) = pages[0].to_rgb().unwrap();
    assert_eq!((pw, ph), (want.width, want.height));
    assert_eq!(
        rgb, want.data,
        "the Flate page is the enhanced crop, exactly"
    );
    // Page 2 is the straight JPEG crop (320 x 240 of a 400 x 300 source; the lossless cut snaps
    // outward to a whole JPEG block, so a few rows more), page 3 the curved one.
    assert_eq!(pages[1].width_px, 320);
    assert!(
        (240..=256).contains(&pages[1].height_px),
        "{}",
        pages[1].height_px
    );
    assert_eq!(pages[1].codec, PageCodec::Dct);
    assert_eq!(pages[2].codec, PageCodec::Dct);
    assert!(pages[2].width_px > 250 && pages[2].width_px <= 320);
    for (p, s) in pages.iter().zip(&saved.page_sizes_pt) {
        assert!((p.media_box.0 - s.0).abs() < 0.01 && (p.media_box.1 - s.1).abs() < 0.01);
        // Pixels over dpi times 72, at the nominal 300 dpi.
        assert!((p.media_box.0 - f64::from(p.width_px) * 72.0 / 300.0).abs() < 0.01);
    }
    // The originals are untouched, no backup was made, nothing else was left in the folder.
    let after = tree_hash(&e.dir);
    assert_eq!(
        after
            .iter()
            .filter(|(n, _)| n != "Scans.pdf")
            .collect::<Vec<_>>(),
        before.iter().collect::<Vec<_>>()
    );
    assert!(stray_temps(&e.dir).is_empty());
    assert!(
        !e.paths.backups_dir().exists()
            || fs::read_dir(e.paths.backups_dir())
                .unwrap()
                .next()
                .is_none()
    );
}

#[test]
fn the_dpi_option_sizes_every_page() {
    let e = env();
    let (a, _, c) = three(&e);
    let o = PdfRunOptions {
        dpi: Some(150),
        title: Some("My scans".to_owned()),
        ..PdfRunOptions::default()
    };
    let out = e.dir.join("x.pdf");
    let saved = e
        .engine
        .save_run_pdf(&[sel(&a), sel(&c)], &out, &o, &never(), &no_progress)
        .unwrap();
    let pages = read_pages(&fs::read(&saved.path).unwrap()).unwrap();
    assert_eq!(
        pages[0].media_box.0,
        f64::from(pages[0].width_px) * 72.0 / 150.0
    );
    let doc = String::from_utf8_lossy(&fs::read(&saved.path).unwrap()).into_owned();
    assert!(doc.contains("/Title (My scans)") && doc.contains("/Producer (Auto Crop)"));
}

#[test]
fn an_unedited_jpeg_crop_is_embedded_without_a_new_generation() {
    let e = env();
    let src = jpeg_source(400, 304, 9);
    // The whole frame: the lossless transform is the identity, so the data is the source's.
    let a = open(&e, "whole.jpg", &src, &quad(0.0, 0.0, 1.0, 1.0));
    let saved = e
        .engine
        .save_run_pdf(
            &[sel(&a)],
            &e.dir.join("p.pdf"),
            &PdfRunOptions::default(),
            &never(),
            &no_progress,
        )
        .unwrap();
    assert_eq!(saved.pass_through_pages, 1);
    assert!(saved.notices.iter().any(|n| n == "pdf.jpeg_pass_through"));
    let pages = read_pages(&fs::read(&saved.path).unwrap()).unwrap();
    assert_eq!((pages[0].width_px, pages[0].height_px), (400, 304));
    let (_, _, rgb) = pages[0].to_rgb().unwrap();
    assert_eq!(rgb, decode(&src).unwrap().raster.data, "no generation loss");
    assert_eq!(
        pages[0].stream_bytes(),
        strip_jpeg(&src).unwrap().as_slice()
    );
}

#[test]
fn an_existing_file_is_never_replaced_the_name_changes() {
    let e = env();
    let (a, _, _) = three(&e);
    let out = e.dir.join("Scans.pdf");
    fs::write(&out, b"someone else's file").unwrap();
    let saved = e
        .engine
        .save_run_pdf(
            &[sel(&a)],
            &out,
            &PdfRunOptions::default(),
            &never(),
            &no_progress,
        )
        .unwrap();
    assert_eq!(saved.path, e.dir.join("Scans (2).pdf"));
    assert!(saved.notices.iter().any(|n| n == "pdf.renamed"));
    assert_eq!(fs::read(&out).unwrap(), b"someone else's file");
    assert!(read_pages(&fs::read(&saved.path).unwrap()).is_some());
    // A second run takes the next free name.
    let again = e
        .engine
        .save_run_pdf(
            &[sel(&a)],
            &out,
            &PdfRunOptions::default(),
            &never(),
            &no_progress,
        )
        .unwrap();
    assert_eq!(again.path, e.dir.join("Scans (3).pdf"));
    assert!(stray_temps(&e.dir).is_empty());
}

fn pdfs(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|f| f.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".pdf"))
        .collect();
    v.sort();
    v
}

#[test]
fn a_failure_at_any_step_leaves_no_output_and_no_temp() {
    for fail_at in [
        PdfStep::Page(0),
        PdfStep::Page(1),
        PdfStep::Page(2),
        PdfStep::Written,
        PdfStep::Verified,
    ] {
        let e = env();
        let (a, b, c) = three(&e);
        let hook: Box<dyn Fn(PdfStep) -> Result<(), ErrKind>> = Box::new(move |s| {
            if s == fail_at {
                Err(ErrKind::DiskFull)
            } else {
                Ok(())
            }
        });
        let r = e.engine.save_run_pdf_with(
            &[sel(&a), sel(&b), sel(&c)],
            &e.dir.join("Scans.pdf"),
            &PdfRunOptions::default(),
            &never(),
            &no_progress,
            Some(hook),
        );
        assert_eq!(r.err(), Some(ErrKind::DiskFull), "{fail_at:?}");
        assert!(pdfs(&e.dir).is_empty(), "{fail_at:?}: an output was left");
        assert!(
            stray_temps(&e.dir).is_empty(),
            "{fail_at:?}: a temp was left"
        );
    }
}

#[test]
fn a_cancel_stops_between_pages_and_leaves_nothing() {
    let e = env();
    let (a, b, c) = three(&e);
    // Already cancelled: nothing is even started.
    let t = CancelToken::new_batch();
    t.cancel();
    let r = e.engine.save_run_pdf(
        &[sel(&a)],
        &e.dir.join("x.pdf"),
        &PdfRunOptions::default(),
        &t,
        &no_progress,
    );
    assert_eq!(r.err(), Some(ErrKind::Cancelled));
    // Cancelled after the first page.
    let t = CancelToken::new_batch();
    let t2 = t.clone();
    let hook: Box<dyn Fn(PdfStep) -> Result<(), ErrKind>> = Box::new(move |s| {
        if s == PdfStep::Page(0) {
            t2.cancel();
        }
        Ok(())
    });
    let r = e.engine.save_run_pdf_with(
        &[sel(&a), sel(&b), sel(&c)],
        &e.dir.join("x.pdf"),
        &PdfRunOptions::default(),
        &t,
        &no_progress,
        Some(hook),
    );
    assert_eq!(r.err(), Some(ErrKind::Cancelled));
    assert!(pdfs(&e.dir).is_empty() && stray_temps(&e.dir).is_empty());
}

/// Damages the temp after it was written, as a failing disk or an antivirus filter might.
fn corrupting(dir: PathBuf, how: &'static str) -> Box<dyn Fn(PdfStep) -> Result<(), ErrKind>> {
    Box::new(move |s| {
        if s == PdfStep::Written {
            let temp = stray_temps(&dir).into_iter().next().expect("the temp");
            let mut b = fs::read(&temp).unwrap();
            match how {
                "flip" => {
                    let mid = b.len() / 2;
                    b[mid] ^= 0xFF;
                }
                "truncate" => b.truncate(b.len() - 30),
                _ => b.extend_from_slice(b"junk"),
            }
            fs::write(&temp, b).unwrap();
        }
        Ok(())
    })
}

#[test]
fn the_parse_back_catches_a_damaged_file_before_it_gets_its_name() {
    for how in ["flip", "truncate", "append"] {
        let e = env();
        let (a, b, _) = three(&e);
        let r = e.engine.save_run_pdf_with(
            &[sel(&a), sel(&b)],
            &e.dir.join("Scans.pdf"),
            &PdfRunOptions::default(),
            &never(),
            &no_progress,
            Some(corrupting(e.dir.clone(), how)),
        );
        assert_eq!(r.err(), Some(ErrKind::VerifyFailed), "{how}");
        assert!(
            pdfs(&e.dir).is_empty() && stray_temps(&e.dir).is_empty(),
            "{how}"
        );
    }
}

#[test]
fn what_cannot_be_a_page_is_a_typed_error_before_anything_is_written() {
    let e = env();
    let (a, _, _) = three(&e);
    let out = e.dir.join("x.pdf");
    let go = |sel: &[PdfSelect], out: &Path| {
        e.engine
            .save_run_pdf(sel, out, &PdfRunOptions::default(), &never(), &no_progress)
            .err()
    };
    assert_eq!(go(&[], &out), Some(ErrKind::NoCrop));
    assert_eq!(
        go(
            &[PdfSelect {
                item: 9999,
                crop: None
            }],
            &out
        ),
        Some(ErrKind::Internal)
    );
    assert_eq!(
        go(
            &[PdfSelect {
                item: a.id,
                crop: Some(777)
            }],
            &out
        ),
        Some(ErrKind::NoCrop)
    );
    assert_eq!(
        go(&[sel(&a)], &e.dir.join("nodir/x.pdf")),
        Some(ErrKind::Unreadable)
    );
    // A colour page cannot be 1 bit.
    let o = PdfRunOptions {
        one_bit: true,
        ..PdfRunOptions::default()
    };
    let r = e
        .engine
        .save_run_pdf(&[sel(&a)], &out, &o, &never(), &no_progress);
    assert_eq!(r.err(), Some(ErrKind::UnsupportedOutput));
    assert!(pdfs(&e.dir).is_empty() && stray_temps(&e.dir).is_empty());
}

#[test]
fn an_enhanced_page_is_group_4_with_one_bit_and_a_selected_crop_picks_one_page() {
    let e = env();
    let (_, _, c) = three(&e);
    let o = PdfRunOptions {
        one_bit: true,
        ..PdfRunOptions::default()
    };
    let saved = e
        .engine
        .save_run_pdf(
            &[PdfSelect {
                item: c.id,
                crop: Some(c.crops[0].id),
            }],
            &e.dir.join("bw.pdf"),
            &o,
            &never(),
            &no_progress,
        )
        .unwrap();
    let pages = read_pages(&fs::read(&saved.path).unwrap()).unwrap();
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].codec, PageCodec::G4);
}

#[test]
fn the_progress_callback_sees_every_page_and_both_stages() {
    let e = env();
    let (a, b, c) = three(&e);
    let seen = std::sync::Mutex::new(Vec::new());
    e.engine
        .save_run_pdf(
            &[sel(&a), sel(&b), sel(&c)],
            &e.dir.join("p.pdf"),
            &PdfRunOptions::default(),
            &never(),
            &|p| seen.lock().unwrap().push((p.stage, p.done, p.total)),
        )
        .unwrap();
    let seen = seen.into_inner().unwrap();
    for i in 0..3u32 {
        assert!(seen.contains(&("rendering", i, 3)), "{seen:?}");
        assert!(seen.contains(&("verifying", i, 3)), "{seen:?}");
    }
    assert_eq!(seen.last(), Some(&("saving", 3, 3)));
    let _ = Enhance::default();
}
