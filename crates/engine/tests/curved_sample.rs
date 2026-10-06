// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The curved sample of "Try sample images" (`receipt_curved.jpg`): a receipt whose four edges are bent
//! by a known analytic model. The editor (UI) is meant to be tried on it, so this checks that it opens, that the
//! curves a person would mark on it flatten it (the printed rows come out straight), and that the engine calls
//! the UI makes on it hold together (curve, bend, undo, accept, straighten).

use auto_crop_core::{Curve, CurveWarp, Pt};
use auto_crop_engine::samples::{CURVED_SAMPLE, curved_sample_edges, write_samples};
use auto_crop_engine::{AppPaths, CropImage, Engine, ItemStatus, ItemView, NoSplit};
use std::sync::Arc;

fn open_curved() -> (tempfile::TempDir, Engine, ItemView) {
    let root = tempfile::tempdir().unwrap();
    let engine = Engine::new(AppPaths::under(root.path()));
    engine.set_item_detector(Arc::new(NoSplit));
    let files = write_samples(&root.path().join("samples")).unwrap();
    let path = files
        .iter()
        .find(|p| p.ends_with(CURVED_SAMPLE))
        .expect("the curved sample is in the set");
    let summary = engine.open_paths(std::slice::from_ref(path), false);
    let id = *summary.ids.first().expect("opened");
    let v = engine.analyse(id).expect("analysed");
    assert_eq!(v.status, ItemStatus::Ready, "{:?}", v.error);
    assert_eq!(v.crops.len(), 1, "one page");
    (root, engine, v)
}

fn warp(n: usize) -> CurveWarp {
    let [t, r, b, l] = curved_sample_edges(n)
        .map(|e| Curve::new(e.into_iter().map(|(x, y)| Pt::new(x, y)).collect()).expect("a curve"));
    CurveWarp {
        top: t,
        right: r,
        bottom: b,
        left: l,
        quarter_turns: 0,
        mirror: false,
    }
}

/// Row of the first dark pixel (luma under 120) in column `x`, searching the top fifth.
fn first_dark(r: &auto_crop_imgproc::Raster, x: u32) -> Option<u32> {
    (0..r.height / 5).find(|&y| {
        let p = r.pixel(x, y);
        (u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2])) / 3 < 120
    })
}

/// How far apart the first printed row sits at 15% and 50% of the width.
fn row_skew(bytes: &[u8]) -> u32 {
    let r = auto_crop_codecs::decode(bytes).unwrap().raster;
    // The shop name spans 20%-80% of the page width, so both columns hit it.
    let a = first_dark(&r, r.width * 25 / 100).expect("ink at 25%");
    let b = first_dark(&r, r.width / 2).expect("ink at 50%");
    a.abs_diff(b)
}

#[test]
fn the_curves_a_person_marks_on_the_sample_flatten_it() {
    let (_root, e, v) = open_curved();
    let (id, crop) = (v.id, v.crops[0].id);
    // The plain straight crop of the page: the dipping top edge leaves the printed rows bent.
    let straight = e.crop_image_bytes(id, crop, CropImage::Result).unwrap().0;
    let skew_straight = row_skew(&straight);

    let v = e.curve_from_quad(id, crop).unwrap();
    assert!(v.crops[0].curves.is_some());
    let v = e
        .set_curves(id, crop, &warp(9), false, "Bend edges", Some(1))
        .unwrap();
    let c = v.crops[0].curves.as_ref().expect("curved");
    assert_eq!(c.top.points().len(), 9);
    let flat = e.crop_image_bytes(id, crop, CropImage::Result).unwrap().0;
    let skew_flat = row_skew(&flat);
    assert!(
        skew_flat + 12 < skew_straight && skew_flat <= 6,
        "flattened rows are straight: {skew_flat} px apart against {skew_straight} px in the straight crop"
    );
}

#[test]
fn the_editor_flow_on_the_sample_curve_bend_undo_accept_straighten() {
    let (_root, e, v) = open_curved();
    let (id, crop) = (v.id, v.crops[0].id);
    let straight_key = v.crops[0].render_key.clone();

    // Entering Curved: four straight edges, one undo step, held for review.
    let v = e.curve_from_quad(id, crop).unwrap();
    assert_eq!(v.undo_label.as_deref(), Some("Curve edges"));
    assert_eq!(v.crops[0].band, Some(auto_crop_core::Band::Check));
    assert!(!v.split.as_ref().unwrap().accepted);

    // A drag is previewed live (nothing recorded), then committed as ONE step per gesture id.
    let before = v.history_position;
    let live = e
        .set_curves(id, crop, &warp(5), true, "Bend", Some(2))
        .unwrap();
    assert_eq!(live.history_position, before, "a live edit records nothing");
    e.set_curves(id, crop, &warp(5), false, "Bend", Some(2))
        .unwrap();
    let v = e
        .set_curves(id, crop, &warp(7), false, "Bend", Some(2))
        .unwrap();
    assert_eq!(v.history_position, before + 1, "one gesture, one step");
    assert_ne!(v.crops[0].render_key, straight_key);

    // Undo takes the bend back, redo returns it.
    let v = e.undo(id).unwrap();
    assert_eq!(v.crops[0].curves.as_ref().unwrap().top.points().len(), 2);
    let v = e.redo(id).unwrap();
    assert_eq!(v.crops[0].curves.as_ref().unwrap().top.points().len(), 7);

    // The quad operations are refused on a curved crop and change nothing.
    let edit = v.crops[0].edit.clone().unwrap();
    let key = v.crops[0].render_key.clone();
    assert_eq!(
        e.set_crop_edit(id, crop, &edit, false, "Move corner", None)
            .unwrap_err(),
        auto_crop_engine::ErrKind::ItemOp
    );
    assert_eq!(
        e.set_crop_angle(id, crop, 3.0, None).unwrap_err(),
        auto_crop_engine::ErrKind::ItemOp
    );
    assert_eq!(e.item_view(id).unwrap().crops[0].render_key, key);

    // Accept binds to this exact state; a later edit withdraws it.
    let v = e.accept_scan(id).unwrap();
    assert!(v.split.as_ref().unwrap().accepted);
    assert_eq!(v.crops[0].band, Some(auto_crop_core::Band::Good));
    let v = e
        .set_curves(id, crop, &warp(9), false, "Bend", Some(3))
        .unwrap();
    assert!(!v.split.as_ref().unwrap().accepted);

    // Back to straight, and the preview of a candidate needs no commit.
    let v = e.clear_curves(id, crop).unwrap();
    assert!(v.crops[0].curves.is_none());
    let (bytes, mime) = e
        .preview_curves(
            id,
            crop,
            &warp(9),
            CropImage::Thumb,
            &auto_crop_imgproc::cancel::NeverCancel,
        )
        .unwrap();
    assert_eq!(mime, "image/jpeg");
    assert!(auto_crop_codecs::decode(&bytes).is_ok());
    assert!(
        e.item_view(id).unwrap().crops[0].curves.is_none(),
        "a preview commits nothing"
    );
}
