// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Enhancement through the engine (PLAN 5, ROADMAP M7.19, M7.67; `docs/dev/enhance-engine.md`):
//! the edit API (one undo step, live drags, per-crop override), the suggestion (precise, cached,
//! applies nothing), the preview, and above all the save path: geometry -> flatten -> threshold ->
//! encode with ONE resample, the lossless JPEG path and the colour profile, byte-identical
//! `Original`, restore, multi-item isolation and curved pages that compose with it.

use auto_crop_codecs::fixtures::{JpegSpec, fake_icc};
use auto_crop_codecs::{Format, decode, encode, probe};
use auto_crop_core::{Curve, CurveWarp, Enhance, EnhanceMode, Pt, QuadWarp};
use auto_crop_engine::group::{Fault, Step};
use auto_crop_engine::{
    AppPaths, CropImage, Edit, Engine, ErrKind, ItemView, NoSplit, PageShape, RestoreMode,
    SaveTarget,
};
use auto_crop_imgproc::Raster;
use auto_crop_imgproc::cancel::NeverCancel;
use auto_crop_imgproc::enhance::testpages::{Light, PageSpec, PhotoKind, fade, lit, page, photo};
use auto_crop_imgproc::enhance::{self as ie, Enhanced, Params};
use auto_crop_imgproc::render::{Limits, render_quad};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

struct Env {
    _root: tempfile::TempDir,
    engine: Engine,
    dir: PathBuf,
}

fn env() -> Env {
    let root = tempfile::tempdir().unwrap();
    let paths = AppPaths::under(root.path());
    let engine = Engine::new(paths);
    engine.set_item_detector(Arc::new(NoSplit));
    let dir = root.path().join("photos");
    fs::create_dir_all(&dir).unwrap();
    Env {
        _root: root,
        engine,
        dir,
    }
}

fn nop(_: ItemView) {}

fn png(r: &Raster) -> Vec<u8> {
    encode(r, Format::Png, 90, None).unwrap()
}

/// A shaded receipt-and-text page as a photo, with its ground truth.
fn shaded_page(seed: u64) -> (Raster, Vec<bool>) {
    let spec = PageSpec::document(seed).sized(600, 840, 3);
    let p = page(&spec);
    (
        lit(
            &p.truth,
            Light::Tilt { dx: 0.5, dy: 0.15 },
            [1.0; 3],
            2.0,
            seed,
        ),
        p.ink,
    )
}

fn open(e: &Env, name: &str, bytes: &[u8]) -> ItemView {
    let path = e.dir.join(name);
    fs::write(&path, bytes).unwrap();
    let id = e.engine.open_paths(&[path], false).ids[0];
    let v = e.engine.analyse(id).unwrap();
    assert!(v.error.is_none(), "{name}: {:?}", v.error);
    v
}

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> [Pt; 4] {
    [
        Pt::new(x0, y0),
        Pt::new(x1, y0),
        Pt::new(x1, y1),
        Pt::new(x0, y1),
    ]
}

fn crop(e: &Env, v: &ItemView, r: [f64; 4], deg: f32) -> ItemView {
    let edit = Edit {
        quad: rect(r[0], r[1], r[2], r[3]),
        quarter_turns: 0,
        fine_deg: deg,
    };
    e.engine.set_edit(v.id, &edit, false, "crop").unwrap()
}

/// A page opened with the whole frame as its crop.
fn open_page(e: &Env, name: &str, seed: u64) -> (ItemView, Raster, Vec<bool>) {
    let (img, ink) = shaded_page(seed);
    let v = open(e, name, &png(&img));
    let v = crop(e, &v, [0.0, 0.0, 1.0, 1.0], 0.0);
    (v, img, ink)
}

fn saved(e: &Env, id: u32, target: SaveTarget) -> auto_crop_engine::SaveOutcome {
    let out = e.engine.save_items(&[id], target, "run", &nop);
    assert!(out[0].ok, "{:?}", out[0]);
    out.into_iter().next().unwrap()
}

fn copy_of(e: &Env, name: &str) -> Vec<u8> {
    fs::read(e.dir.join("AutoCrop").join(name)).unwrap()
}

fn set(e: &Env, v: &ItemView, mode: EnhanceMode, s: f32) -> ItemView {
    let c = v.crops[0].id;
    e.engine
        .set_enhance(v.id, c, mode, s, false, "", None)
        .unwrap()
}

// ------------------------------------------------------------------------------ the edit API

#[test]
fn the_default_is_original_and_a_change_is_one_undo_step_that_names_the_mode() {
    let e = env();
    let (v, _, _) = open_page(&e, "a.png", 1);
    assert_eq!(v.enhance, Enhance::Original, "never forced (B13)");
    assert_eq!(v.crops[0].enhance, Enhance::Original);
    assert_eq!(v.crops[0].enhance_override, None);
    let key0 = v.crops[0].render_key.clone();
    let pos = v.history_position;

    let v1 = set(&e, &v, EnhanceMode::Document, 1.0);
    assert_eq!(v1.crops[0].enhance, Enhance::Document { strength: 100 });
    assert_eq!(
        v1.crops[0].enhance_override,
        Some(Enhance::Document { strength: 100 })
    );
    assert_eq!(v1.enhance, Enhance::Original, "the default did not change");
    assert_eq!(v1.history_position, pos + 1);
    assert!(v1.undo_label.as_deref().unwrap().contains("document"));
    assert_ne!(v1.crops[0].render_key, key0, "the crop's picture key moved");
    assert!(v1.edited && v1.dirty_since_save == v.dirty_since_save);

    // Undo restores the exact state; redo brings it back.
    let u = e.engine.undo(v.id).unwrap();
    assert_eq!(u.crops[0].enhance, Enhance::Original);
    assert_eq!(u.crops[0].render_key, key0);
    let r = e.engine.redo(v.id).unwrap();
    assert_eq!(r.crops[0].enhance, Enhance::Document { strength: 100 });

    // Strength is clamped, rounded to percent; Original ignores it.
    let c = v.crops[0].id;
    let v2 = e
        .engine
        .set_enhance(v.id, c, EnhanceMode::Clean, 0.456, false, "", None)
        .unwrap();
    assert_eq!(v2.crops[0].enhance, Enhance::Clean { strength: 46 });
    let v3 = e
        .engine
        .set_enhance(v.id, c, EnhanceMode::Receipt, 9.0, false, "", None)
        .unwrap();
    assert_eq!(v3.crops[0].enhance, Enhance::Receipt { strength: 100 });
    // The identity again, and clearing the override.
    let v4 = e.engine.clear_enhance(v.id, c).unwrap();
    assert_eq!(v4.crops[0].enhance_override, None);
    assert_eq!(v4.crops[0].render_key, key0);
    // An unknown crop is refused and changes nothing.
    let before = e.engine.item_view(v.id).unwrap().history_position;
    assert_eq!(
        e.engine
            .set_enhance(v.id, 99, EnhanceMode::Clean, 1.0, false, "", None)
            .unwrap_err(),
        ErrKind::ItemOp
    );
    assert_eq!(e.engine.item_view(v.id).unwrap().history_position, before);
}

#[test]
fn a_slider_drag_is_one_history_entry_and_live_updates_record_nothing() {
    let e = env();
    let (v, _, _) = open_page(&e, "a.png", 2);
    let c = v.crops[0].id;
    let pos = v.history_position;
    // Live: the view says what the crop would be, nothing is recorded.
    for s in [0.2, 0.5, 0.8] {
        let live = e
            .engine
            .set_enhance(v.id, c, EnhanceMode::Clean, s, true, "", None)
            .unwrap();
        assert_eq!(live.crops[0].enhance.mode(), EnhanceMode::Clean);
        assert_eq!(e.engine.item_view(v.id).unwrap().history_position, pos);
        assert_eq!(
            e.engine.item_view(v.id).unwrap().crops[0].enhance,
            Enhance::Original
        );
    }
    // 60 committed ticks of one gesture are one step.
    for k in 0..60u8 {
        e.engine
            .set_enhance(
                v.id,
                c,
                EnhanceMode::Clean,
                f32::from(k) / 100.0,
                false,
                "Strength",
                Some(7),
            )
            .unwrap();
    }
    let after = e.engine.item_view(v.id).unwrap();
    assert_eq!(after.history_position, pos + 1);
    assert_eq!(after.crops[0].enhance, Enhance::Clean { strength: 59 });
    let u = e.engine.undo(v.id).unwrap();
    assert_eq!(u.crops[0].enhance, Enhance::Original);
    // The default (all crops) is its own step and does not touch the override.
    let d = e
        .engine
        .set_default_enhance(v.id, EnhanceMode::Receipt, 1.0, false, "", None)
        .unwrap();
    assert_eq!(d.enhance, Enhance::Receipt { strength: 100 });
    assert_eq!(d.crops[0].enhance, Enhance::Receipt { strength: 100 });
    assert_eq!(d.crops[0].enhance_override, None);
}

// ------------------------------------------------------------------------------ the suggestion

#[test]
fn the_suggestion_fires_on_a_shaded_page_applies_nothing_and_is_cached() {
    let e = env();
    let (v, _, _) = open_page(&e, "a.png", 3);
    let c = v.crops[0].id;
    let pos = v.history_position;
    let s = e
        .engine
        .suggest_enhance(v.id, c)
        .unwrap()
        .expect("a shaded text page is suggested");
    assert_eq!(s.mode, EnhanceMode::Document);
    assert_eq!(s.strength, 100);
    assert_eq!(s.reason, "uneven_light");
    assert!(s.hint.gray && s.hint.recommend_png);
    assert!(s.measures.shading > 0.3 && s.measures.text_components > 100);
    assert_eq!(s.enhance(), Enhance::Document { strength: 100 });
    // Nothing changed: no history entry, no override, the same picture key.
    let after = e.engine.item_view(v.id).unwrap();
    assert_eq!(after.history_position, pos);
    assert_eq!(after.crops[0].enhance, Enhance::Original);
    assert_eq!(after.crops[0].render_key, v.crops[0].render_key);
    // The JSON the UI reads.
    let j = serde_json::to_value(&s).unwrap();
    assert_eq!(j["mode"], "document");
    assert_eq!(j["hint"]["recommendPng"], true);
    assert!(j["measures"]["paperFraction"].is_number());
    // Asking again is answered from the cache, and enhancing meanwhile does not change the answer
    // (it measures the page, not the enhancement).
    let again = e.engine.suggest_enhance(v.id, c).unwrap().unwrap();
    assert_eq!(again, s);
    set(&e, &v, EnhanceMode::Clean, 1.0);
    assert_eq!(e.engine.suggest_enhance(v.id, c).unwrap(), Some(s));
    // A different crop of the same image is measured again.
    let tight = crop(&e, &v, [0.1, 0.1, 0.6, 0.5], 0.0);
    assert!(
        e.engine
            .suggest_enhance(tight.id, tight.crops[0].id)
            .unwrap()
            .is_some()
    );
    assert_eq!(
        e.engine.suggest_enhance(v.id, 99).unwrap_err(),
        ErrKind::NoCrop
    );
}

#[test]
fn the_suggestion_is_silent_on_a_photograph_and_on_a_clean_scan() {
    let e = env();
    for (i, kind) in [
        PhotoKind::Landscape,
        PhotoKind::Foliage,
        PhotoKind::Interior,
    ]
    .into_iter()
    .enumerate()
    {
        let img = photo(kind, 40 + i as u64, 800, 600);
        let v = open(&e, &format!("p{i}.png"), &png(&img));
        let v = crop(&e, &v, [0.0, 0.0, 1.0, 1.0], 0.0);
        assert_eq!(
            e.engine.suggest_enhance(v.id, v.crops[0].id).unwrap(),
            None,
            "{kind:?}"
        );
    }
    // A flat, bright, black-on-white scan has nothing to improve.
    let mut spec = PageSpec::document(9).sized(600, 840, 3);
    spec.paper = [252; 3];
    spec.ink = [10; 3];
    let p = page(&spec);
    let img = lit(&p.truth, Light::Flat, [1.0; 3], 1.0, 1);
    let v = open(&e, "scan.png", &png(&img));
    let v = crop(&e, &v, [0.0, 0.0, 1.0, 1.0], 0.0);
    assert_eq!(e.engine.suggest_enhance(v.id, v.crops[0].id).unwrap(), None);
}

#[test]
fn a_faded_long_page_is_a_receipt_suggestion() {
    let e = env();
    let spec = PageSpec::receipt(5).sized(300, 900, 3);
    let p = page(&spec);
    let img = lit(
        &fade(&p.truth, spec.paper, 0.35),
        Light::Tilt { dx: 0.3, dy: 0.1 },
        [1.0, 0.93, 0.85],
        2.0,
        3,
    );
    let v = open(&e, "r.png", &png(&img));
    let v = crop(&e, &v, [0.0, 0.0, 1.0, 1.0], 0.0);
    let s = e
        .engine
        .suggest_enhance(v.id, v.crops[0].id)
        .unwrap()
        .expect("suggested");
    assert_eq!(s.mode, EnhanceMode::Receipt);
    assert!(s.reasons.iter().any(|r| r == "faded_print"));
}

// ------------------------------------------------------------------------------ the preview

#[test]
fn the_committed_crop_image_and_the_live_preview_show_the_enhancement() {
    let e = env();
    let (v, _, _) = open_page(&e, "a.png", 4);
    let c = v.crops[0].id;
    let (plain, _) = e
        .engine
        .crop_image_bytes(v.id, c, CropImage::Result)
        .unwrap();
    // A candidate is rendered without recording or caching anything.
    let pos = e.engine.item_view(v.id).unwrap().history_position;
    let (cand, mime) = e
        .engine
        .preview_enhance(
            v.id,
            c,
            Enhance::Document { strength: 100 },
            CropImage::Result,
            &NeverCancel,
        )
        .unwrap();
    assert_eq!(mime, "image/jpeg");
    assert_ne!(*cand, *plain);
    assert_eq!(e.engine.item_view(v.id).unwrap().history_position, pos);
    let grey_share = |jpeg: &[u8]| {
        let d = decode(jpeg).unwrap().raster;
        let n = d.data.len() / 3;
        let extreme = d
            .data
            .as_chunks::<3>()
            .0
            .iter()
            .filter(|p| p[0] < 40 || p[0] > 215)
            .count();
        extreme as f64 / n as f64
    };
    // The binarised preview is nearly all black or white; the plain one is a shaded gradient.
    assert!(grey_share(&cand) > 0.9, "{}", grey_share(&cand));
    assert!(grey_share(&plain) < 0.5);
    // Committing the same enhancement serves the same picture from the crop route.
    set(&e, &v, EnhanceMode::Document, 1.0);
    let (committed, _) = e
        .engine
        .crop_image_bytes(v.id, c, CropImage::Result)
        .unwrap();
    assert_eq!(*committed, *cand);
    // The whole-image result and the thumbnail follow the first crop's enhancement too.
    let (whole, _) = e
        .engine
        .image_bytes(v.id, auto_crop_engine::ImageKind::Result)
        .unwrap();
    assert!(grey_share(&whole) > 0.9);
    // A cancelled preview returns nothing.
    let flag = AtomicBool::new(true);
    assert_eq!(
        e.engine
            .preview_enhance(
                v.id,
                c,
                Enhance::Receipt { strength: 100 },
                CropImage::Result,
                &flag
            )
            .unwrap_err(),
        ErrKind::Cancelled
    );
}

// ------------------------------------------------------------------------------ the save path

#[test]
fn original_is_byte_identical_to_the_path_that_knows_no_enhancement() {
    for fmt in [Format::Png, Format::Jpeg] {
        let (img, _) = shaded_page(6);
        let src = encode(&img, fmt, 90, None).unwrap();
        let ext = if fmt == Format::Png { "png" } else { "jpg" };
        let skew = |e: &Env, name: &str, touch: bool| {
            let v = open(e, name, &src);
            // A skewed crop: never the lossless path, so the pixels are rendered and encoded.
            let v = crop(e, &v, [0.05, 0.05, 0.95, 0.9], 1.0);
            let v = if touch {
                // Explicit Original, and a mode set and cleared again: still the same bytes.
                let v = set(e, &v, EnhanceMode::Original, 1.0);
                let v = set(e, &v, EnhanceMode::Document, 1.0);
                e.engine.clear_enhance(v.id, v.crops[0].id).unwrap()
            } else {
                v
            };
            saved(e, v.id, SaveTarget::Copy);
            copy_of(e, name)
        };
        let (e1, e2) = (env(), env());
        let name = format!("a.{ext}");
        let plain = skew(&e1, &name, false);
        let touched = skew(&e2, &name, true);
        assert_eq!(plain, touched, "{fmt:?}");
        // And both equal what the pure render + encode path (no enhancement code) writes.
        let want = auto_crop_engine::outputs::render_outputs(
            &src,
            &[QuadWarp {
                corners: rect(0.05, 0.05, 0.95, 0.9),
                quarter_turns: 0,
                mirror: false,
                fine_deg: 1.0,
            }],
            Some(fmt),
            &auto_crop_engine::EngineOptions::default(),
        )
        .unwrap();
        assert_eq!(plain, want[0].bytes, "{fmt:?}");
    }
}

fn is_gray(bytes: &[u8]) -> bool {
    probe(bytes).unwrap().channels == 1
}

#[test]
fn a_document_save_is_flatten_then_threshold_of_the_one_resample_and_one_grey_channel() {
    let e = env();
    let (v, img, ink) = open_page(&e, "a.png", 7);
    let v = crop(&e, &v, [0.0, 0.0, 1.0, 1.0], 0.0);
    let v = set(&e, &v, EnhanceMode::Document, 1.0);
    let out = saved(&e, v.id, SaveTarget::Copy);
    let file = copy_of(&e, &out.saved.as_ref().unwrap().output);
    // One 8-bit grey channel, lossless PNG, black on white.
    assert!(is_gray(&file));
    let got = decode(&file).unwrap().raster;
    assert_eq!((got.width, got.height), (600, 840));
    // The analytic expectation: the engine's pixels are exactly the enhancer applied to the one
    // resample of the geometry.
    let src = decode(&png(&img)).unwrap().raster;
    let quad = QuadWarp {
        corners: rect(0.0, 0.0, 1.0, 1.0),
        quarter_turns: 0,
        mirror: false,
        fine_deg: 0.0,
    };
    let page = render_quad(&src, &quad, Limits::pixels(u64::MAX)).unwrap();
    let (want, _) = ie::enhance(
        page,
        &Params::new(ie::Mode::Document, 1.0),
        None,
        &NeverCancel,
    )
    .unwrap();
    let Enhanced::Gray(want) = want else {
        panic!("document is grey")
    };
    assert_eq!(
        got.data
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| p[0])
            .collect::<Vec<u8>>(),
        want.data
    );
    assert!(
        got.data
            .as_chunks::<3>()
            .0
            .iter()
            .all(|p| p[0] == p[1] && p[1] == p[2])
    );
    // And it did its job: the strokes are black, the shaded paper is white.
    let (mut hit, mut tot, mut white, mut paper) = (0u64, 0u64, 0u64, 0u64);
    for (i, is_ink) in ink.iter().enumerate() {
        let v = got.data[i * 3];
        if *is_ink {
            tot += 1;
            hit += u64::from(v < 128);
        } else {
            paper += 1;
            white += u64::from(v > 200);
        }
    }
    assert!(hit as f64 / tot as f64 > 0.85, "{hit}/{tot}");
    // (The "paper" of the truth mask includes the blurred halo of every stroke.)
    assert!(white as f64 / paper as f64 > 0.90, "{white}/{paper}");
}

#[test]
fn clean_keeps_colour_and_png_is_lossless_for_the_save_that_follows() {
    let e = env();
    let (v, _, _) = open_page(&e, "a.png", 8);
    let v = set(&e, &v, EnhanceMode::Clean, 1.0);
    let out = saved(&e, v.id, SaveTarget::Copy);
    let file = copy_of(&e, &out.saved.as_ref().unwrap().output);
    assert!(!is_gray(&file), "Clean keeps the colour channels");
    assert!(out.notices.is_empty(), "{:?}", out.notices);
    // The paper came out whiter than it went in (the tilt is flattened).
    let d = decode(&file).unwrap().raster;
    let right_edge = d.pixel(d.width - 4, d.height / 2);
    assert!(right_edge.iter().all(|c| *c >= 235), "{right_edge:?}");
}

#[test]
fn a_non_original_enhancement_turns_the_lossless_jpeg_path_off_and_the_profile_follows_the_mode() {
    let mut spec = JpegSpec::new(160, 128);
    spec.sampling = (2, 2);
    spec.icc = Some(fake_icc(1500));
    let src = spec.build();
    // The MCU-aligned crop of the lossless test of `single_save.rs`.
    let aligned = [0.2, 0.125, 0.8, 0.75];
    let go = |mode: EnhanceMode| {
        let e = env();
        let v = open(&e, "a.jpg", &src);
        let v = crop(&e, &v, aligned, 0.0);
        let v = set(&e, &v, mode, 1.0);
        let out = saved(&e, v.id, SaveTarget::Replace);
        (fs::read(e.dir.join("a.jpg")).unwrap(), out.notices)
    };
    // Original: lossless, the profile byte-exact.
    let (orig, n) = go(EnhanceMode::Original);
    assert!(n.contains(&"jpeg.lossless".to_owned()), "{n:?}");
    assert_eq!(
        decode(&orig).unwrap().icc.as_deref(),
        Some(fake_icc(1500).as_slice())
    );
    // Clean: re-encoded (no lossless notice), colour, the profile still byte-exact.
    let (clean, n) = go(EnhanceMode::Clean);
    assert!(!n.contains(&"jpeg.lossless".to_owned()));
    assert!(!is_gray(&clean));
    assert_eq!(
        decode(&clean).unwrap().icc.as_deref(),
        Some(fake_icc(1500).as_slice())
    );
    // Document: one grey channel, no colour profile (with a notice), PNG recommended, quality >= 90.
    let (doc, n) = go(EnhanceMode::Document);
    assert!(is_gray(&doc));
    assert_eq!(decode(&doc).unwrap().icc, None);
    assert!(n.contains(&"enhance.icc_dropped".to_owned()), "{n:?}");
    assert!(n.contains(&"enhance.png_recommended".to_owned()), "{n:?}");
    assert!(!n.contains(&"jpeg.lossless".to_owned()));
    let q = auto_crop_codecs::jpeg_meta::read_meta(&doc)
        .unwrap()
        .quality
        .unwrap();
    assert!(q.q >= 90, "{q:?}");
    // Strength 0 is Original: the lossless path comes back.
    let e = env();
    let v = open(&e, "a.jpg", &src);
    let v = crop(&e, &v, aligned, 0.0);
    let v = set(&e, &v, EnhanceMode::Document, 0.0);
    let out = saved(&e, v.id, SaveTarget::Replace);
    assert!(out.notices.contains(&"jpeg.lossless".to_owned()));
}

#[test]
fn an_enhanced_save_replaces_after_a_verified_backup_and_restore_is_byte_identical() {
    let e = env();
    let (img, _) = shaded_page(10);
    let src = encode(&img, Format::Jpeg, 92, None).unwrap();
    let v = open(&e, "a.jpg", &src);
    let v = crop(&e, &v, [0.02, 0.02, 0.98, 0.97], 0.4);
    let v = set(&e, &v, EnhanceMode::Receipt, 1.0);
    let out = saved(&e, v.id, SaveTarget::Replace);
    let backup = out.saved.as_ref().unwrap().backup_id.clone().unwrap();
    let written = fs::read(e.dir.join("a.jpg")).unwrap();
    assert!(is_gray(&written));
    assert_ne!(written, src);
    // The guard recognises the enhanced file as ours.
    assert!(e.engine.processed_by(&e.dir.join("a.jpg")).is_some());
    // Restore: the original, byte for byte.
    let r = e
        .engine
        .restore_file(&format!("{backup}/0"), RestoreMode::Auto, &nop);
    assert!(r.ok, "{r:?}");
    assert_eq!(fs::read(e.dir.join("a.jpg")).unwrap(), src);
    // A crop-only save followed by an enhanced re-save keeps ONE backup of the pristine original.
    let e2 = env();
    let v = open(&e2, "b.jpg", &src);
    let v = crop(&e2, &v, [0.02, 0.02, 0.98, 0.97], 0.4);
    saved(&e2, v.id, SaveTarget::Replace);
    let v = set(&e2, &v, EnhanceMode::Document, 1.0);
    let out = saved(&e2, v.id, SaveTarget::Replace);
    assert!(out.saved.unwrap().backup_id.is_some());
    assert_eq!(e2.engine.list_backups().runs.len(), 1);
    assert!(is_gray(&fs::read(e2.dir.join("b.jpg")).unwrap()));
}

#[test]
fn a_fault_during_an_enhanced_save_never_leaves_a_lossy_result_without_a_backup() {
    let e = env();
    let (img, _) = shaded_page(11);
    let src = encode(&img, Format::Jpeg, 92, None).unwrap();
    let v = open(&e, "a.jpg", &src);
    let v = crop(&e, &v, [0.02, 0.02, 0.98, 0.97], 0.4);
    let v = set(&e, &v, EnhanceMode::Document, 1.0);
    for step in [
        Step::TempWritten(0),
        Step::TempVerified(0),
        Step::JournalCommitting,
        Step::BeforeRename(0),
    ] {
        let hook = move |s: &Step| {
            if *s == step {
                Fault::Fail(ErrKind::VerifyFailed)
            } else {
                Fault::Pass
            }
        };
        let out = e
            .engine
            .save_items_with_faults(&[v.id], SaveTarget::Replace, "r", &hook);
        assert!(!out[0].ok, "{step:?}");
        // Whatever the step, the folder holds the original, unchanged: a failed save never
        // leaves a lossy result behind.
        assert_eq!(fs::read(e.dir.join("a.jpg")).unwrap(), src, "{step:?}");
    }
    // Without a fault the same state saves, with a backup of the pristine original.
    let out = saved(&e, v.id, SaveTarget::Replace);
    assert!(out.saved.unwrap().backup_id.is_some());
    assert!(is_gray(&fs::read(e.dir.join("a.jpg")).unwrap()));
}

#[test]
fn enhancing_item_two_leaves_item_ones_bytes_unchanged_and_estimates_never_mix() {
    // Two receipts side by side, one under a deep shadow: item 2's estimate must not leak into
    // item 1 and enhancing item 2 must not touch item 1's file.
    let spec = PageSpec::receipt(12).sized(400, 840, 3);
    let left = page(&spec).truth;
    let right = page(&PageSpec::receipt(13).sized(400, 840, 3)).truth;
    let mut both = Raster::new(800, 840);
    for y in 0..840 {
        for x in 0..400 {
            both.set_pixel(x, y, left.pixel(x, y));
            both.set_pixel(400 + x, y, right.pixel(x, y));
        }
    }
    let both = lit(
        &both,
        Light::Shadow {
            depth: 0.5,
            width: 200.0,
        },
        [1.0; 3],
        2.0,
        5,
    );
    let go = |enhance_two: bool| {
        let e = env();
        let v = open(&e, "s.png", &png(&both));
        let v = crop(&e, &v, [0.0, 0.0, 0.5, 1.0], 0.0);
        let v = e
            .engine
            .add_crop(v.id, Some(rect(0.5, 0.0, 1.0, 1.0)), None)
            .unwrap();
        assert_eq!(v.crops.len(), 2);
        let (c1, c2) = (v.crops[0].id, v.crops[1].id);
        let v = if enhance_two {
            e.engine
                .set_enhance(v.id, c2, EnhanceMode::Document, 1.0, false, "", None)
                .unwrap()
        } else {
            v
        };
        assert_eq!(v.crops[0].enhance_override, None);
        let _ = c1;
        saved(&e, v.id, SaveTarget::Copy);
        (copy_of(&e, "s_01.png"), copy_of(&e, "s_02.png"))
    };
    let (one_a, two_a) = go(false);
    let (one_b, two_b) = go(true);
    assert_eq!(
        one_a, one_b,
        "item 1's file is untouched by enhancing item 2"
    );
    assert_ne!(two_a, two_b);
    assert!(is_gray(&two_b) && !is_gray(&one_b));
    // Each is the same as enhancing that crop alone: the estimates are the item's own.
    let alone = {
        let e = env();
        let v = open(&e, "s.png", &png(&both));
        let v = crop(&e, &v, [0.5, 0.0, 1.0, 1.0], 0.0);
        let v = set(&e, &v, EnhanceMode::Document, 1.0);
        saved(&e, v.id, SaveTarget::Copy);
        copy_of(&e, "s.png")
    };
    assert_eq!(
        decode(&two_b).unwrap().raster,
        decode(&alone).unwrap().raster,
        "item 2 alone equals item 2 of the pair"
    );
}

fn bent_curves() -> CurveWarp {
    let c = |p: &[(f64, f64)]| Curve::new(p.iter().map(|&(x, y)| Pt::new(x, y)).collect()).unwrap();
    CurveWarp {
        top: c(&[(0.05, 0.06), (0.5, 0.03), (0.95, 0.06)]),
        right: c(&[(0.95, 0.06), (0.95, 0.94)]),
        bottom: c(&[(0.95, 0.94), (0.5, 0.97), (0.05, 0.94)]),
        left: c(&[(0.05, 0.94), (0.05, 0.06)]),
        quarter_turns: 0,
        mirror: false,
    }
}

#[test]
fn a_curved_page_composes_with_enhancement_in_one_resample() {
    let e = env();
    let (img, _) = shaded_page(14);
    let bytes = png(&img);
    let v = open(&e, "c.png", &bytes);
    let c = bent_curves();
    let q = c.corners();
    let edit = Edit {
        quad: q,
        quarter_turns: 0,
        fine_deg: 0.0,
    };
    let v = e.engine.set_edit(v.id, &edit, false, "page").unwrap();
    let crop_id = v.crops[0].id;
    e.engine.curve_from_quad(v.id, crop_id).unwrap();
    let v = e
        .engine
        .set_curves(v.id, crop_id, &c, false, "Bend", None)
        .unwrap();
    let v = e
        .engine
        .set_enhance(v.id, crop_id, EnhanceMode::Document, 1.0, false, "", None)
        .unwrap();
    // Held until accepted, as every curved page (the rule is unchanged by enhancement).
    let held = e.engine.save_items(&[v.id], SaveTarget::Replace, "r", &nop);
    assert_eq!(held[0].error, Some(ErrKind::HeldForReview));
    let v = e.engine.accept_scan(v.id).unwrap();
    saved(&e, v.id, SaveTarget::Replace);
    let file = fs::read(e.dir.join("c.png")).unwrap();
    assert!(is_gray(&file));
    // The expectation: the curved render of the source, enhanced, byte for byte.
    let src = decode(&bytes).unwrap().raster;
    let flat = PageShape::Curved(c)
        .render(&src, Limits::pixels(u64::MAX))
        .unwrap();
    let (want, _) = ie::enhance(
        flat,
        &Params::new(ie::Mode::Document, 1.0),
        None,
        &NeverCancel,
    )
    .unwrap();
    let got = decode(&file).unwrap().raster;
    assert_eq!(got, want.into_rgb());
}

#[test]
fn pixels_the_geometry_takes_from_outside_the_frame_are_not_paper() {
    // A full-frame crop turned by a fine angle pulls black from outside the source into its
    // corners: the validity mask keeps them out of the estimate and white in the result.
    let e = env();
    let (img, _) = shaded_page(15);
    let v = open(&e, "r.png", &png(&img));
    let v = crop(&e, &v, [0.0, 0.0, 1.0, 1.0], 6.0);
    let render = |mode: EnhanceMode| {
        let (bytes, _) = e
            .engine
            .preview_enhance(
                v.id,
                v.crops[0].id,
                Enhance::new(mode, 100),
                CropImage::Result,
                &NeverCancel,
            )
            .unwrap();
        decode(&bytes).unwrap().raster
    };
    let plain = render(EnhanceMode::Original);
    let doc = render(EnhanceMode::Document);
    assert_eq!((plain.width, plain.height), (doc.width, doc.height));
    let corner = |r: &Raster| r.pixel(2, 2)[0];
    assert!(corner(&plain) < 30, "outside the source is black before");
    assert!(corner(&doc) > 240, "and white after: {}", corner(&doc));
    // The page itself is still enhanced (a mid-page pixel is not the corner's white by default).
    let ink = doc
        .data
        .as_chunks::<3>()
        .0
        .iter()
        .filter(|p| p[0] < 100)
        .count();
    assert!(ink > 1000);
}

#[test]
fn memory_admission_counts_the_enhancement_by_output_pixels() {
    let e = env();
    let (v, _, _) = open_page(&e, "a.png", 16);
    let plain = e.engine.save_weight(v.id).unwrap();
    set(&e, &v, EnhanceMode::Original, 1.0);
    assert_eq!(e.engine.save_weight(v.id).unwrap(), plain);
    set(&e, &v, EnhanceMode::Clean, 1.0);
    let clean = e.engine.save_weight(v.id).unwrap();
    set(&e, &v, EnhanceMode::Document, 1.0);
    let doc = e.engine.save_weight(v.id).unwrap();
    assert!(clean > plain && doc > clean, "{plain} < {clean} < {doc}");
    // Two bytes per output pixel (600 x 840) for the grey planes, plus the proxy.
    assert!(doc - plain >= 2 * 600 * 840);
}

#[test]
fn a_noop_crop_is_not_a_noop_once_it_is_enhanced() {
    let e = env();
    let (img, _) = shaded_page(17);
    let src = encode(&img, Format::Jpeg, 92, None).unwrap();
    let v = open(&e, "n.jpg", &src);
    let v = crop(&e, &v, [0.0, 0.0, 1.0, 1.0], 0.0);
    assert!(e.engine.is_noop(v.id));
    set(&e, &v, EnhanceMode::Clean, 1.0);
    assert!(
        !e.engine.is_noop(v.id),
        "an enhanced crop is worth a rewrite"
    );
}
