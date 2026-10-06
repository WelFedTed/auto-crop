// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Curved pages through the engine (docs/dev/curved-pages.md, ROADMAP M12.34, M12.36 to M12.39):
//! the edit API (`curve_from_quad`, `set_curves`, `clear_curves`), the held-for-review rule, the
//! preview and the save through the one resampler, undo and redo, the refusals of the quad
//! operations, the crash-safe commit, and a save that really flattens a photographed bent page
//! (compared with the analytic ground truth of a forward generator written here).

use auto_crop_codecs::fixtures::{JpegSpec, fake_icc, jpeg_insert_segment};
use auto_crop_core::{Curve, CurveWarp, Geometry, Pt};
use auto_crop_engine::group::{Fault, Step};
use auto_crop_engine::{
    AppPaths, CropImage, Edit, Engine, ErrKind, ItemView, NoSplit, RestoreMode, SaveTarget,
    curved_job_weight,
};
use auto_crop_imgproc::Raster;
use auto_crop_imgproc::curved::{output_size, render_curved};
use auto_crop_imgproc::render::Limits;
use std::f64::consts::PI;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

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

const UNLIMITED: Limits = Limits {
    max_pixels: u64::MAX,
    max_edge: u32::MAX,
};

// ------------------------------------------------------------------ a photographed bent page

const IW: u32 = 720;
const IH: u32 = 540;
const X0: f64 = 90.0;
const X1: f64 = 630.0;
/// Height of the flat page in pixels.
const PH: f64 = 340.0;

/// The top edge of the bowed page (a parabolic sag), `p` along the width.
fn top_y(p: f64) -> f64 {
    80.0 + 30.0 * 4.0 * p * (1.0 - p)
}

/// The printed page: rules (raised cosine, 4 px) every 40 px down and 60 px across.
fn printed(s: f64, v: f64) -> f64 {
    let lat = |x: f64, off: f64, step: f64| x - off - step * ((x - off) / step).round();
    let rule = |d: f64| {
        if d.abs() < 2.0 {
            0.5 * (1.0 + (PI * d / 2.0).cos())
        } else {
            0.0
        }
    };
    (238.0 - 170.0 * rule(lat(v, 20.0, 40.0)) - 170.0 * rule(lat(s, 30.0, 60.0))).clamp(0.0, 255.0)
}

struct Bowed {
    /// Arc length at 4000 values of p.
    arc: Vec<f64>,
}

impl Bowed {
    fn new() -> Self {
        let n = 4000;
        let mut arc = vec![0.0];
        let mut prev = (X0, top_y(0.0));
        for i in 1..=n {
            let p = i as f64 / n as f64;
            let cur = (X0 + p * (X1 - X0), top_y(p));
            arc.push(arc[i - 1] + (cur.0 - prev.0).hypot(cur.1 - prev.1));
            prev = cur;
        }
        Self { arc }
    }

    fn length(&self) -> f64 {
        *self.arc.last().unwrap()
    }

    fn s_of_p(&self, p: f64) -> f64 {
        let f = (p * 4000.0).clamp(0.0, 4000.0);
        let i = (f.floor() as usize).min(3999);
        self.arc[i] + (f - i as f64) * (self.arc[i + 1] - self.arc[i])
    }

    fn p_of_s(&self, s: f64) -> f64 {
        let i = self.arc.partition_point(|a| *a <= s).clamp(1, 4000);
        let span = self.arc[i] - self.arc[i - 1];
        ((i - 1) as f64
            + if span > 0.0 {
                (s - self.arc[i - 1]) / span
            } else {
                0.0
            })
            / 4000.0
    }

    fn photo(&self) -> Raster {
        let mut r = Raster::filled(IW, IH, [70, 70, 70]);
        for iy in 0..IH {
            for ix in 0..IW {
                let (x, y) = (f64::from(ix) + 0.5, f64::from(iy) + 0.5);
                let p = (x - X0) / (X1 - X0);
                if !(0.0..=1.0).contains(&p) {
                    continue;
                }
                let v = y - top_y(p);
                if (0.0..=PH).contains(&v) {
                    let g = printed(self.s_of_p(p), v).round() as u8;
                    r.set_pixel(ix, iy, [g, g, g]);
                }
            }
        }
        r
    }

    /// The edges a person would mark: `n` points along top and bottom at equal arc length.
    fn curves(&self, n: usize) -> CurveWarp {
        let at = |i: usize, dv: f64| {
            let s = self.length() * i as f64 / (n - 1) as f64;
            let p = self.p_of_s(s);
            Pt::new(
                (X0 + p * (X1 - X0)) / f64::from(IW),
                (top_y(p) + dv) / f64::from(IH),
            )
        };
        let top: Vec<Pt> = (0..n).map(|i| at(i, 0.0)).collect();
        let bottom: Vec<Pt> = (0..n).rev().map(|i| at(i, PH)).collect();
        let (tl, tr, br, bl) = (top[0], top[n - 1], bottom[0], bottom[n - 1]);
        CurveWarp {
            top: Curve::new(top).unwrap(),
            right: Curve::new(vec![tr, br]).unwrap(),
            bottom: Curve::new(bottom).unwrap(),
            left: Curve::new(vec![bl, tl]).unwrap(),
            quarter_turns: 0,
            mirror: false,
        }
    }

    /// The flat page at the resolution of an output `w` x `h`.
    fn ideal(&self, w: u32, h: u32) -> Raster {
        let mut r = Raster::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let g = printed(
                    (f64::from(x) + 0.5) / f64::from(w) * self.length(),
                    (f64::from(y) + 0.5) / f64::from(h) * PH,
                )
                .round() as u8;
                r.set_pixel(x, y, [g, g, g]);
            }
        }
        r
    }
}

/// (mean absolute error, share of interior pixels within 40 grey levels) of `got` against `want`.
fn compare(got: &Raster, want: &Raster, margin: u32) -> (f64, f64) {
    assert_eq!((got.width, got.height), (want.width, want.height));
    let (mut sum, mut n, mut ok) = (0.0, 0usize, 0usize);
    for y in margin..got.height - margin {
        for x in margin..got.width - margin {
            let d = (i32::from(got.pixel(x, y)[1]) - i32::from(want.pixel(x, y)[1])).abs();
            sum += f64::from(d);
            n += 1;
            ok += usize::from(d <= 40);
        }
    }
    (sum / n as f64, ok as f64 / n as f64)
}

fn png_of(r: &Raster) -> Vec<u8> {
    auto_crop_codecs::encode(r, auto_crop_codecs::Format::Png, 90, None).unwrap()
}

/// Opens the bent-page photo as `name` and gives it one manual crop on the page's corners.
fn open_page(e: &Env, name: &str) -> (ItemView, Bowed, Vec<u8>) {
    let page = Bowed::new();
    let bytes = png_of(&page.photo());
    let path = e.dir.join(name);
    fs::write(&path, &bytes).unwrap();
    let id = e.engine.open_paths(&[path], false).ids[0];
    let v = e.engine.analyse(id).unwrap();
    assert!(v.error.is_none(), "{:?}", v.error);
    let c = page.curves(2);
    let q = c.corners();
    let edit = Edit {
        quad: q,
        quarter_turns: 0,
        fine_deg: 0.0,
    };
    let v = e.engine.set_edit(id, &edit, false, "page").unwrap();
    (v, page, bytes)
}

fn crop_id(v: &ItemView) -> u32 {
    v.crops[0].id
}

// ------------------------------------------------------------------------------ tests

#[test]
fn analysis_never_proposes_a_curved_page() {
    let e = env();
    let s = e.engine.add_samples().unwrap();
    assert!(s.ids.len() >= 3);
    for id in &s.ids {
        let v = e.engine.analyse(*id).unwrap();
        let st = e.engine.edit_state(*id).unwrap();
        assert!(
            st.items.iter().all(|i| !i.geometry.is_curved()),
            "{}: the detector stays quad-based",
            v.name
        );
        assert!(v.crops.iter().all(|c| c.curves.is_none()));
        // And a plain quad crop reports no curves in the view.
    }
}

#[test]
fn a_curved_crop_is_held_until_accepted_and_the_views_say_so() {
    let e = env();
    let (v, page, _) = open_page(&e, "a.png");
    let crop = crop_id(&v);
    assert!(v.crops[0].curves.is_none());

    let v = e.engine.curve_from_quad(v.id, crop).unwrap();
    let c = v.crops[0].curves.as_ref().expect("curves in the view");
    assert!(c.is_straight(), "four straight edges to start from");
    assert_eq!(
        v.edit.as_ref().unwrap().quad.len(),
        4,
        "the outline is still the edit"
    );
    assert_eq!(v.history_position, 2);
    assert!(v.undo_label.as_deref().unwrap().starts_with("Curve edges"));

    let v = e
        .engine
        .set_curves(v.id, crop, &page.curves(9), false, "Bend", None)
        .unwrap();
    let c = v.crops[0].curves.as_ref().unwrap();
    assert_eq!(c.top.points().len(), 9);
    // Held: band Check, scan triage held, not accepted.
    assert_eq!(v.crops[0].band, Some(auto_crop_core::Band::Check));
    let split = v.split.as_ref().unwrap();
    assert!(!split.accepted);
    assert!(matches!(
        split.triage,
        auto_crop_core::ScanTriage::HeldForReview {
            items_need_check: 1
        }
    ));
    // The JSON a UI sees carries the control points.
    let json = serde_json::to_value(&v.crops[0]).unwrap();
    assert_eq!(json["curves"]["top"].as_array().unwrap().len(), 9);

    // Accepting makes it Good and approved for exactly this state.
    let v = e.engine.accept_scan(v.id).unwrap();
    assert_eq!(v.crops[0].band, Some(auto_crop_core::Band::Good));
    assert!(v.split.as_ref().unwrap().accepted);
    // Any edit voids it.
    let mut moved = page.curves(9);
    let mut pts = moved.top.points().to_vec();
    pts[4].y -= 0.01;
    moved.top = Curve::new(pts).unwrap();
    let v = e
        .engine
        .set_curves(v.id, crop, &moved, false, "Bend", None)
        .unwrap();
    assert!(!v.split.as_ref().unwrap().accepted);
    assert_eq!(v.crops[0].band, Some(auto_crop_core::Band::Check));
}

#[test]
fn an_unaccepted_curved_page_never_replaces_an_original_but_a_copy_is_allowed() {
    let e = env();
    let (v, page, bytes) = open_page(&e, "a.png");
    let crop = crop_id(&v);
    let v = e.engine.curve_from_quad(v.id, crop).unwrap();
    let v = e
        .engine
        .set_curves(v.id, crop, &page.curves(9), false, "Bend", None)
        .unwrap();

    let out = e.engine.save_items(&[v.id], SaveTarget::Replace, "r", &nop);
    assert!(!out[0].ok);
    assert_eq!(out[0].error, Some(ErrKind::HeldForReview));
    assert_eq!(out[0].notices, vec!["curved.held".to_owned()]);
    assert_eq!(
        fs::read(e.dir.join("a.png")).unwrap(),
        bytes,
        "the original is untouched"
    );
    assert!(
        e.engine.list_backups().runs.is_empty(),
        "no backup, nothing was started"
    );

    // A copy overwrites nothing: no acceptance needed, and it is flat.
    let out = e.engine.save_items(&[v.id], SaveTarget::Copy, "r", &nop);
    assert!(out[0].ok, "{:?}", out[0]);
    assert_eq!(fs::read(e.dir.join("a.png")).unwrap(), bytes);
    let copy = fs::read(e.dir.join("AutoCrop").join("a.png")).unwrap();
    let got = auto_crop_codecs::decode(&copy).unwrap().raster;
    let want = page.ideal(got.width, got.height);
    let (mae, share) = compare(&got, &want, 8);
    println!(
        "curved copy: {} x {}, MAE {mae:.2}, within 40: {share:.4}",
        got.width, got.height
    );
    assert!(mae < 5.0 && share > 0.97, "MAE {mae}, share {share}");
}

#[test]
fn an_accepted_curved_page_is_saved_flat_once_and_restores_byte_identical() {
    let e = env();
    let (v, page, bytes) = open_page(&e, "a.png");
    let crop = crop_id(&v);
    let v = e.engine.curve_from_quad(v.id, crop).unwrap();
    let v = e
        .engine
        .set_curves(v.id, crop, &page.curves(9), false, "Bend", None)
        .unwrap();
    let v = e.engine.accept_scan(v.id).unwrap();

    let out = e.engine.save_items(&[v.id], SaveTarget::Replace, "r", &nop);
    assert!(out[0].ok, "{:?}", out[0]);
    assert!(
        !out[0].notices.iter().any(|n| n == "jpeg.lossless"),
        "never the lossless path"
    );
    let saved = fs::read(e.dir.join("a.png")).unwrap();
    assert_ne!(saved, bytes);
    let got = auto_crop_codecs::decode(&saved).unwrap().raster;
    // The size comes from the arc lengths: the page is longer than its chord (540 px).
    let want_size = output_size(IW, IH, &page.curves(9), UNLIMITED).unwrap();
    assert_eq!((got.width, got.height), want_size);
    assert!(got.width > 541, "{}", got.width);
    let want = page.ideal(got.width, got.height);
    let (mae, share) = compare(&got, &want, 8);
    println!(
        "curved save: {} x {}, MAE {mae:.2}, within 40: {share:.4}",
        got.width, got.height
    );
    assert!(mae < 5.0 && share > 0.97, "MAE {mae}, share {share}");
    // One resample from the source: identical to what the resampler produces directly.
    let direct = render_curved(
        &auto_crop_codecs::decode(&bytes).unwrap().raster,
        &page.curves(9),
        UNLIMITED,
    )
    .unwrap();
    assert_eq!(got, direct, "no second warp, no re-processing");

    // The backup holds the original and "Restore original" gives it back byte for byte.
    let runs = e.engine.list_backups().runs;
    assert_eq!(runs[0].files[0].original_bytes, bytes.len() as u64);
    let r = e
        .engine
        .restore_file(&runs[0].files[0].id, RestoreMode::Auto, &nop);
    assert!(r.ok, "{r:?}");
    assert_eq!(fs::read(e.dir.join("a.png")).unwrap(), bytes);
}

#[test]
fn undo_and_redo_restore_the_curves_exactly_and_edits_are_one_step_per_gesture() {
    let e = env();
    let (v, page, _) = open_page(&e, "a.png");
    let crop = crop_id(&v);
    let v = e.engine.curve_from_quad(v.id, crop).unwrap();
    let straight = v.crops[0].curves.clone().unwrap();
    // A drag: three live frames and a commit under one gesture id.
    let mut last = v.clone();
    for n in [3usize, 5, 9] {
        last = e
            .engine
            .set_curves(v.id, crop, &page.curves(n), true, "Bend", Some(7))
            .unwrap();
        assert_eq!(
            last.history_position, v.history_position,
            "live edits record nothing"
        );
        assert_eq!(last.crops[0].curves.as_ref().unwrap().top.points().len(), n);
    }
    let a = e
        .engine
        .set_curves(v.id, crop, &page.curves(5), false, "Bend", Some(7))
        .unwrap();
    let b = e
        .engine
        .set_curves(v.id, crop, &page.curves(9), false, "Bend", Some(7))
        .unwrap();
    assert_eq!(
        b.history_position, a.history_position,
        "one undo step per gesture"
    );
    let _ = last;
    let nine = b.crops[0].curves.clone().unwrap();
    let v = e.engine.undo(v.id).unwrap();
    assert_eq!(v.crops[0].curves.as_ref(), Some(&straight));
    let v = e.engine.undo(v.id).unwrap();
    assert!(v.crops[0].curves.is_none(), "back to the plain quad");
    let v = e.engine.redo(v.id).unwrap();
    assert_eq!(v.crops[0].curves.as_ref(), Some(&straight));
    let v = e.engine.redo(v.id).unwrap();
    assert_eq!(
        v.crops[0].curves.as_ref(),
        Some(&nine),
        "the control points are bit-exact"
    );
    // clear_curves is an undoable step back to the straight quad.
    let v = e.engine.clear_curves(v.id, crop).unwrap();
    assert!(v.crops[0].curves.is_none());
    let v = e.engine.undo(v.id).unwrap();
    assert_eq!(v.crops[0].curves.as_ref(), Some(&nine));
    // The state itself round-trips through the persisted schema.
    let st = e.engine.edit_state(v.id).unwrap();
    assert_eq!(st.version, 3);
    let back =
        auto_crop_engine::migrate::migrate_str(&serde_json::to_string(&st).unwrap()).unwrap();
    assert_eq!(back, st);
}

#[test]
fn quad_operations_refuse_a_curved_crop_and_never_lose_its_curves() {
    let e = env();
    let (v, page, _) = open_page(&e, "a.png");
    let crop = crop_id(&v);
    e.engine.curve_from_quad(v.id, crop).unwrap();
    let v = e
        .engine
        .set_curves(v.id, crop, &page.curves(9), false, "Bend", None)
        .unwrap();
    let before = e.engine.edit_state(v.id).unwrap();
    let pos = v.history_position;
    let quad_edit = Edit {
        quad: [
            Pt::new(0.2, 0.2),
            Pt::new(0.8, 0.2),
            Pt::new(0.8, 0.8),
            Pt::new(0.2, 0.8),
        ],
        quarter_turns: 0,
        fine_deg: 0.0,
    };
    // Every way a quad could replace the page is refused, with a typed error.
    assert_eq!(
        e.engine.set_edit(v.id, &quad_edit, false, "x").unwrap_err(),
        ErrKind::ItemOp
    );
    assert_eq!(
        e.engine.set_edit(v.id, &quad_edit, true, "x").unwrap_err(),
        ErrKind::ItemOp
    );
    assert_eq!(
        e.engine
            .set_crop_edit(v.id, crop, &quad_edit, false, "x", None)
            .unwrap_err(),
        ErrKind::ItemOp
    );
    assert_eq!(
        e.engine
            .set_crop_edit(v.id, crop, &quad_edit, true, "x", None)
            .unwrap_err(),
        ErrKind::ItemOp
    );
    assert_eq!(
        e.engine.set_crop_angle(v.id, crop, 3.0, None).unwrap_err(),
        ErrKind::ItemOp
    );
    assert_eq!(
        e.engine.merge_crops(v.id, &[crop, crop]).unwrap_err(),
        ErrKind::ItemOp
    );
    assert_eq!(
        e.engine
            .cut_crop(
                v.id,
                crop,
                auto_crop_engine::Cut::halves(auto_crop_engine::CutAxis::Vertical)
            )
            .unwrap_err(),
        ErrKind::ItemOp
    );
    // Nothing changed, nothing was recorded.
    assert_eq!(e.engine.edit_state(v.id).unwrap(), before);
    assert_eq!(e.engine.item_view(v.id).unwrap().history_position, pos);

    // Turn and flip are handled and keep the curves.
    let v = e.engine.turn_crop(v.id, crop, true).unwrap();
    let c = v.crops[0].curves.as_ref().unwrap();
    assert_eq!((c.quarter_turns, c.top.points().len()), (1, 9));
    let v = e.engine.flip_crop(v.id, crop).unwrap();
    assert!(v.crops[0].curves.as_ref().unwrap().mirror && v.crops[0].mirror);
    // Re-detection keeps the user's curved crop.
    let v = e
        .engine
        .redetect(v.id, auto_crop_engine::SplitPatch::default())
        .unwrap();
    assert!(v.crops.iter().any(|c| c.curves.is_some()));
    // A curve set that is not a valid outline is refused.
    let mut bad = page.curves(9);
    bad.right = Curve::new(vec![Pt::new(0.9, 0.1), Pt::new(0.95, 0.5)]).unwrap();
    assert_eq!(
        e.engine
            .set_curves(v.id, crop, &bad, false, "x", None)
            .unwrap_err(),
        ErrKind::Degenerate
    );
    // Reset to auto is an explicit, undoable step: it may drop the curves, and undo brings them back.
    let v = e.engine.reset_to_auto(v.id).unwrap();
    let v = e.engine.undo(v.id).unwrap();
    assert!(v.crops.iter().any(|c| c.curves.is_some()));
}

#[test]
fn the_preview_uses_the_curved_resampler_at_preview_resolution_and_can_be_cancelled() {
    let e = env();
    let (v, page, _) = open_page(&e, "a.png");
    let crop = crop_id(&v);
    e.engine.curve_from_quad(v.id, crop).unwrap();
    let v = e
        .engine
        .set_curves(v.id, crop, &page.curves(9), false, "Bend", None)
        .unwrap();
    let (bytes, mime) = e
        .engine
        .crop_image_bytes(v.id, crop, CropImage::Result)
        .unwrap();
    assert_eq!(mime, "image/jpeg");
    let got = auto_crop_codecs::decode(&bytes).unwrap().raster;
    // The proxy is the whole 720 x 540 image (below the preview edge), so the preview is the
    // flat page at the page's own size, not a rectangle of the quad's size.
    let want_size = output_size(IW, IH, &page.curves(9), Limits::pixels(u64::MAX)).unwrap();
    assert_eq!((got.width, got.height), want_size);
    let (mae, share) = compare(&got, &page.ideal(got.width, got.height), 8);
    assert!(mae < 8.0 && share > 0.95, "MAE {mae}, share {share}");
    // Whole-image previews (the Result and the thumbnail) take the same path.
    let (res, _) = e
        .engine
        .image_bytes(v.id, auto_crop_engine::ImageKind::Result)
        .unwrap();
    assert_eq!(
        auto_crop_codecs::decode(&res).unwrap().raster.width,
        got.width
    );
    assert!(
        e.engine
            .image_bytes(v.id, auto_crop_engine::ImageKind::Thumb)
            .is_ok()
    );
    // A candidate set renders without committing anything.
    let pos = e.engine.item_view(v.id).unwrap().history_position;
    let (cand, _) = e
        .engine
        .preview_curves(
            v.id,
            crop,
            &page.curves(5),
            CropImage::Result,
            &AtomicBool::new(false),
        )
        .unwrap();
    assert!(auto_crop_codecs::decode(&cand).is_ok());
    assert_eq!(e.engine.item_view(v.id).unwrap().history_position, pos);
    // A cancelled token returns Cancelled and nothing is cached.
    let stop = AtomicBool::new(true);
    assert_eq!(
        e.engine
            .preview_curves(v.id, crop, &page.curves(5), CropImage::Result, &stop)
            .unwrap_err(),
        ErrKind::Cancelled
    );
    // A change of curves changes the crop's cache key, an unrelated change does not.
    let key = |v: &ItemView| v.crops[0].render_key.clone();
    let k0 = key(&v);
    let v2 = e
        .engine
        .set_curves(v.id, crop, &page.curves(7), false, "Bend", None)
        .unwrap();
    assert_ne!(key(&v2), k0);
}

#[test]
fn a_curved_save_writes_the_exif_orientation_once() {
    let e = env();
    // Stored 128 x 96 with EXIF 6: displayed 96 x 128. The curves are in the displayed space.
    let mut spec = JpegSpec::new(128, 96);
    spec.sampling = (2, 2);
    spec.icc = Some(fake_icc(900));
    let mut exif = b"Exif\0\0".to_vec();
    exif.extend_from_slice(&[
        b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0, 0x12, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0, 0, 0,
    ]);
    let src = jpeg_insert_segment(&spec.build(), 0xE1, &exif);
    let path = e.dir.join("o.jpg");
    fs::write(&path, &src).unwrap();
    let id = e.engine.open_paths(std::slice::from_ref(&path), false).ids[0];
    let v = e.engine.analyse(id).unwrap();
    assert_eq!((v.width, v.height), (96, 128));
    let truth = auto_crop_codecs::decode(&src).unwrap().raster; // oriented once
    assert_eq!((truth.width, truth.height), (96, 128));

    let c = |p: &[(f64, f64)]| Curve::new(p.iter().map(|&(x, y)| Pt::new(x, y)).collect()).unwrap();
    let page = CurveWarp {
        top: c(&[(0.1, 0.1), (0.5, 0.06), (0.9, 0.1)]),
        right: c(&[(0.9, 0.1), (0.9, 0.9)]),
        bottom: c(&[(0.9, 0.9), (0.5, 0.94), (0.1, 0.9)]),
        left: c(&[(0.1, 0.9), (0.1, 0.1)]),
        quarter_turns: 0,
        mirror: false,
    };
    let q = page.corners();
    let edit = Edit {
        quad: q,
        quarter_turns: 0,
        fine_deg: 0.0,
    };
    let v = e.engine.set_edit(id, &edit, false, "page").unwrap();
    let crop = crop_id(&v);
    e.engine.curve_from_quad(id, crop).unwrap();
    e.engine
        .set_curves(id, crop, &page, false, "Bend", None)
        .unwrap();
    e.engine.accept_scan(id).unwrap();
    let out = e.engine.save_items(&[id], SaveTarget::Replace, "r", &nop);
    assert!(out[0].ok, "{:?}", out[0]);
    let saved = fs::read(&path).unwrap();
    assert_eq!(
        auto_crop_codecs::probe(&saved).unwrap().orientation,
        1,
        "the tag is reset"
    );
    let got = auto_crop_codecs::decode(&saved).unwrap().raster;
    let want = render_curved(&truth, &page, UNLIMITED).unwrap();
    assert_eq!((got.width, got.height), (want.width, want.height));
    // Not turned a second time: the picture matches the one-resample render (JPEG noise allowed).
    let mut worst = 0i32;
    for y in (3..got.height - 3).step_by(3) {
        for x in (3..got.width - 3).step_by(3) {
            let (a, b) = (got.pixel(x, y), want.pixel(x, y));
            let d: i32 = (0..3)
                .map(|k| (i32::from(a[k]) - i32::from(b[k])).abs())
                .sum();
            worst = worst.max(d);
        }
    }
    assert!(worst <= 60, "worst {worst}");
}

#[test]
fn a_crash_while_saving_a_curved_page_is_recovered_and_the_original_is_never_lost() {
    let steps = [
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
        let (v, page, bytes) = open_page(&e, "a.png");
        let crop = crop_id(&v);
        e.engine.curve_from_quad(v.id, crop).unwrap();
        e.engine
            .set_curves(v.id, crop, &page.curves(9), false, "Bend", None)
            .unwrap();
        e.engine.accept_scan(v.id).unwrap();
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
        let again = Engine::new(e.paths.clone());
        let now = fs::read(e.dir.join("a.png")).unwrap();
        let committed = matches!(
            step,
            Step::JournalCommitting
                | Step::BeforeRename(_)
                | Step::AfterRename(_)
                | Step::ManifestSaved
        );
        assert_eq!(now == bytes, !committed, "{step:?}");
        if committed {
            assert!(
                auto_crop_codecs::decode(&now).is_ok(),
                "{step:?}: a whole file"
            );
            let runs = again.list_backups().runs;
            let r = again.restore_file(&runs[0].files[0].id, RestoreMode::Auto, &nop);
            assert!(r.ok, "{step:?}: {r:?}");
        }
        assert_eq!(fs::read(e.dir.join("a.png")).unwrap(), bytes, "{step:?}");
    }
    // A failed (not crashed) step leaves the original too.
    let e = env();
    let (v, page, bytes) = open_page(&e, "b.png");
    let crop = crop_id(&v);
    e.engine.curve_from_quad(v.id, crop).unwrap();
    e.engine
        .set_curves(v.id, crop, &page.curves(9), false, "Bend", None)
        .unwrap();
    e.engine.accept_scan(v.id).unwrap();
    let out = e
        .engine
        .save_items_with_faults(&[v.id], SaveTarget::Replace, "r", &|s: &Step| {
            if *s == Step::TempVerified(0) {
                Fault::Fail(ErrKind::VerifyFailed)
            } else {
                Fault::Pass
            }
        });
    assert_eq!(out[0].error, Some(ErrKind::VerifyFailed));
    assert_eq!(fs::read(e.dir.join("b.png")).unwrap(), bytes);
}

#[test]
fn a_scan_with_a_curved_crop_among_others_follows_the_split_hold_rule() {
    let e = env();
    let (v, page, bytes) = open_page(&e, "s.png");
    let crop = crop_id(&v);
    e.engine.curve_from_quad(v.id, crop).unwrap();
    e.engine
        .set_curves(v.id, crop, &page.curves(9), false, "Bend", None)
        .unwrap();
    // A second crop, a plain quad, in the empty lower part of the frame.
    let q = [
        Pt::new(0.1, 0.8),
        Pt::new(0.4, 0.8),
        Pt::new(0.4, 0.97),
        Pt::new(0.1, 0.97),
    ];
    let v = e.engine.add_crop(v.id, Some(q), None).unwrap();
    assert_eq!(v.crops.len(), 2);
    assert!(v.split.as_ref().unwrap().is_split);
    let out = e.engine.save_items(&[v.id], SaveTarget::Replace, "r", &nop);
    assert_eq!(out[0].error, Some(ErrKind::HeldForReview));
    assert_eq!(out[0].notices, vec!["split.held".to_owned()]);
    assert_eq!(fs::read(e.dir.join("s.png")).unwrap(), bytes);
    let v = e.engine.accept_scan(v.id).unwrap();
    let out = e.engine.save_items(&[v.id], SaveTarget::Replace, "r", &nop);
    assert!(out[0].ok, "{:?}", out[0]);
    assert_eq!(out[0].saved.as_ref().unwrap().outputs.len(), 2);
    // The curved crop's output is the flat page, the other is a plain crop.
    let first = &out[0].saved.as_ref().unwrap().outputs[0];
    let got = auto_crop_codecs::decode(&fs::read(e.dir.join(first)).unwrap())
        .unwrap()
        .raster;
    assert!(
        got.width > 541,
        "the curved page is longer than its chord: {}",
        got.width
    );
}

#[test]
fn admission_weight_follows_the_output_pixels_of_a_curved_page() {
    let e = env();
    let (v, page, _) = open_page(&e, "w.png");
    let crop = crop_id(&v);
    let plain = e.engine.save_weight(v.id).unwrap();
    e.engine.curve_from_quad(v.id, crop).unwrap();
    e.engine
        .set_curves(v.id, crop, &page.curves(9), false, "Bend", None)
        .unwrap();
    let curved = e.engine.save_weight(v.id).unwrap();
    assert!(curved > plain, "{curved} vs {plain}");
    // Six bytes per output pixel on top of the source's weight, monotone in the output.
    let src = u64::from(IW) * u64::from(IH);
    assert_eq!(
        curved_job_weight(src, 1_000),
        auto_crop_engine::memory::job_weight(src) + 6_000
    );
    assert!(curved_job_weight(src, 2_000_000) > curved_job_weight(src, 1_000_000));
    // The estimate is the real output size.
    let (w, h) = output_size(IW, IH, &page.curves(9), Limits::pixels(u64::MAX)).unwrap();
    assert_eq!(curved, curved_job_weight(src, u64::from(w) * u64::from(h)));
    let st = e.engine.edit_state(v.id).unwrap();
    assert!(matches!(st.items[0].geometry, Geometry::Curved(_)));
}

#[test]
fn re_editing_a_saved_curved_page_renders_from_the_backup_original_never_dewarping_twice() {
    let e = env();
    let (v, page, bytes) = open_page(&e, "a.png");
    let crop = crop_id(&v);
    e.engine.curve_from_quad(v.id, crop).unwrap();
    e.engine
        .set_curves(v.id, crop, &page.curves(9), false, "Bend", None)
        .unwrap();
    e.engine.accept_scan(v.id).unwrap();
    assert!(e.engine.save_items(&[v.id], SaveTarget::Replace, "r", &nop)[0].ok);
    let first = fs::read(e.dir.join("a.png")).unwrap();

    // Change the curves of the saved image: it is dirty, held again, and a second save renders
    // from the original in the backup, not from the flattened file.
    let v = e
        .engine
        .set_curves(v.id, crop, &page.curves(5), false, "Bend", None)
        .unwrap();
    assert!(v.dirty_since_save);
    assert!(!v.split.as_ref().unwrap().accepted);
    let held = e.engine.save_items(&[v.id], SaveTarget::Replace, "r", &nop);
    assert_eq!(held[0].error, Some(ErrKind::HeldForReview));
    assert_eq!(
        fs::read(e.dir.join("a.png")).unwrap(),
        first,
        "the first output stays until accepted"
    );
    e.engine.accept_scan(v.id).unwrap();
    let out = e.engine.save_items(&[v.id], SaveTarget::Replace, "r", &nop);
    assert!(out[0].ok, "{:?}", out[0]);
    let second = auto_crop_codecs::decode(&fs::read(e.dir.join("a.png")).unwrap())
        .unwrap()
        .raster;
    let direct = render_curved(
        &auto_crop_codecs::decode(&bytes).unwrap().raster,
        &page.curves(5),
        UNLIMITED,
    )
    .unwrap();
    assert_eq!(
        second, direct,
        "one warp of the original, never a warp of a warp"
    );
    // Still one backup, still the pristine original.
    let runs = e.engine.list_backups().runs;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].files.len(), 1);
    let r = e
        .engine
        .restore_file(&runs[0].files[0].id, RestoreMode::Auto, &nop);
    assert!(r.ok, "{r:?}");
    assert_eq!(fs::read(e.dir.join("a.png")).unwrap(), bytes);
}
