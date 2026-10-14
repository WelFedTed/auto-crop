// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! "Trim to content" through the engine, on the matted sample of "Try sample images": the detector
//! answers with the paper edge and holds it (`DETECTORS_DISAGREE`), the engine proposes the picture
//! inside the mat without touching the crop, applying it is one undo step, and pages that are not
//! a picture in a border (the other samples, a curved page) get no proposal or a polite refusal.

use auto_crop_core::{Band, Forced, ReasonCode};
use auto_crop_engine::samples::{
    CURVED_SAMPLE, MATTED_PAPER, MATTED_SAMPLE, matted_sample_picture, write_samples,
};
use auto_crop_engine::{
    AppPaths, CropOrigin, Engine, ErrKind, ItemStatus, ItemView, NoSplit, TRIM_LABEL,
};
use std::sync::Arc;

fn open(name: &str) -> (tempfile::TempDir, Engine, ItemView) {
    let root = tempfile::tempdir().unwrap();
    let engine = Engine::new(AppPaths::under(root.path()));
    engine.set_item_detector(Arc::new(NoSplit));
    let files = write_samples(&root.path().join("samples")).unwrap();
    let path = files
        .iter()
        .find(|p| p.ends_with(name))
        .unwrap_or_else(|| panic!("{name} is in the sample set"));
    let summary = engine.open_paths(std::slice::from_ref(path), false);
    let id = *summary.ids.first().expect("opened");
    let v = engine.analyse(id).expect("analysed");
    assert_eq!(v.status, ItemStatus::Ready, "{:?}", v.error);
    (root, engine, v)
}

/// Largest corner distance as a share of the diagonal (the sample is 1800 x 1350).
fn worst(found: &[auto_crop_core::Pt; 4], truth: &[(f64, f64); 4]) -> f64 {
    let (w, h) = (1800.0f64, 1350.0f64);
    (0..4)
        .map(|i| ((found[i].x - truth[i].0) * w).hypot((found[i].y - truth[i].1) * h))
        .fold(0.0, f64::max)
        / w.hypot(h)
}

#[test]
fn the_matted_sample_is_held_with_two_outlines_and_the_paper_edge_is_the_answer() {
    let (_root, _e, v) = open(MATTED_SAMPLE);
    assert_eq!(v.crops.len(), 1);
    let c = v.confidence.as_ref().expect("a confidence");
    assert_eq!(c.forced, Some(Forced::Check), "{c:?}");
    assert!(
        c.reasons
            .iter()
            .any(|r| r.code == ReasonCode::DetectorsDisagree),
        "{:?}",
        c.reasons
    );
    let q = v.crops[0].edit.as_ref().expect("an edit").quad;
    assert!(worst(&q, &MATTED_PAPER) < 0.012, "the paper edge: {q:?}");
    // The single-image path keeps the confidence on the image, not on the crop.
    assert_eq!(v.crops[0].band, None);
}

#[test]
fn the_proposal_is_the_picture_and_records_nothing() {
    let (_root, e, v) = open(MATTED_SAMPLE);
    let (id, crop) = (v.id, v.crops[0].id);
    let before = e.item_view(id).unwrap();
    let p = e
        .trim_to_content(id, crop)
        .expect("the engine answers")
        .expect("a picture sits inside the mat");
    let err = worst(&p.quad, &matted_sample_picture());
    assert!(err < 0.006, "corner error {err:.4}: {:?}", p.quad);
    assert!((0.5..=0.95).contains(&p.confidence));
    // Nothing changed: same crop, same history, same generation.
    let after = e.item_view(id).unwrap();
    assert_eq!(before, after);
    assert!(!after.can_undo);
    // The JSON is camelCase.
    let j = serde_json::to_value(&p).unwrap();
    assert_eq!(j["reason"], "mat");
    assert_eq!(j["quad"].as_array().unwrap().len(), 4);
}

#[test]
fn applying_it_is_one_undo_step_and_an_edited_crop() {
    let (_root, e, v) = open(MATTED_SAMPLE);
    let (id, crop) = (v.id, v.crops[0].id);
    let paper = v.crops[0].edit.clone().expect("an edit");
    let view = e.apply_trim(id, crop).expect("applied");
    assert_eq!(view.undo_label.as_deref(), Some(TRIM_LABEL));
    assert!(view.can_undo && !view.can_redo);
    let c = &view.crops[0];
    assert!(worst(&c.edit.as_ref().unwrap().quad, &matted_sample_picture()) < 0.006);
    assert_eq!(c.origin, CropOrigin::AutoThenEdited);
    assert!(c.edited, "an edited crop");
    // The user applied it: reviewed, so Good; the confidence recorded is the trim's.
    assert_eq!(c.band, Some(Band::Good));
    let conf = c.confidence.as_ref().expect("the trim's confidence");
    assert!(conf.forced.is_none() && conf.reasons.is_empty());
    assert!((0.5..=0.95).contains(&conf.score));
    // The same picture is now the crop, so there is no second trim to make inside it.
    let again = e.trim_to_content(id, crop).unwrap();
    assert!(again.is_none(), "{again:?}");

    // One step back: the paper edge, held as it was.
    let undone = e.undo(id).unwrap();
    assert_eq!(undone.crops[0].edit.as_ref().unwrap().quad, paper.quad);
    assert_eq!(undone.crops[0].origin, CropOrigin::Auto);
    assert_eq!(undone.crops[0].band, None);
    assert_eq!(
        undone.confidence.as_ref().unwrap().forced,
        Some(Forced::Check)
    );
    assert!(!undone.can_undo && undone.can_redo);
    let redone = e.redo(id).unwrap();
    assert_eq!(redone.undo_label.as_deref(), Some(TRIM_LABEL));
    assert!(
        worst(
            &redone.crops[0].edit.as_ref().unwrap().quad,
            &matted_sample_picture()
        ) < 0.006
    );
}

#[test]
fn turns_and_mirror_are_kept_and_the_fine_angle_is_baked_in() {
    let (_root, e, v) = open(MATTED_SAMPLE);
    let (id, crop) = (v.id, v.crops[0].id);
    e.turn_crop(id, crop, true).unwrap();
    e.flip_crop(id, crop).unwrap();
    let view = e.apply_trim(id, crop).expect("applied");
    let edit = view.crops[0].edit.as_ref().unwrap();
    assert_eq!(edit.quarter_turns, 1);
    assert!(view.crops[0].mirror);
    assert_eq!(edit.fine_deg, 0.0);
    assert!(worst(&edit.quad, &matted_sample_picture()) < 0.006);
}

#[test]
fn a_page_with_no_picture_inside_has_nothing_to_trim_to() {
    for name in ["receipt_1_tilted.jpg", "document_1_perspective.jpg"] {
        let (_root, e, v) = open(name);
        let (id, crop) = (v.id, v.crops[0].id);
        assert_eq!(e.trim_to_content(id, crop), Ok(None), "{name}");
        // Applying nothing is refused and changes nothing.
        assert_eq!(
            e.apply_trim(id, crop).unwrap_err(),
            ErrKind::ItemOp,
            "{name}"
        );
        assert!(!e.item_view(id).unwrap().can_undo);
    }
}

#[test]
fn a_curved_crop_is_refused_politely() {
    let (_root, e, v) = open(CURVED_SAMPLE);
    let (id, crop) = (v.id, v.crops[0].id);
    e.curve_from_quad(id, crop).unwrap();
    assert_eq!(e.trim_to_content(id, crop).unwrap_err(), ErrKind::ItemOp);
    assert_eq!(e.apply_trim(id, crop).unwrap_err(), ErrKind::ItemOp);
    // The crop is still the curved page it was.
    assert!(e.item_view(id).unwrap().crops[0].curves.is_some());
}

#[test]
fn unknown_images_and_crops_are_errors_not_panics() {
    let (_root, e, v) = open(MATTED_SAMPLE);
    assert!(e.trim_to_content(9999, 1).is_err());
    assert_eq!(e.trim_to_content(v.id, 9999).unwrap_err(), ErrKind::NoCrop);
    assert!(e.apply_trim(v.id, 9999).is_err());
}
