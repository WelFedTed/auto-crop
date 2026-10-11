// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `Engine::suggest_curves` (docs/dev/curved-pages.md, "Suggestion API"): on the curved sample of
//! "Try sample images" (a receipt whose four edges are bent by a known analytic model) the engine
//! proposes curves that follow the real edges better than the straight quad does and says the page
//! looks curved; on a flat sample it says it does not; the suggestion never changes the crop by
//! itself, and applying it goes through `set_curves`.

use auto_crop_core::{Curve, Pt};
use auto_crop_engine::samples::{CURVED_SAMPLE, CURVED_SIZE, curved_sample_edges, write_samples};
use auto_crop_engine::{AppPaths, Edit, Engine, ErrKind, ItemStatus, ItemView, NoSplit};
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
    assert_eq!(v.crops.len(), 1, "one page");
    (root, engine, v)
}

/// Densely sampled boundary of four curves, in picture pixels.
fn boundary(edges: &[Vec<Pt>; 4]) -> Vec<(f64, f64)> {
    let (w, h) = (f64::from(CURVED_SIZE.0), f64::from(CURVED_SIZE.1));
    edges
        .iter()
        .flat_map(|e| {
            Curve::new(e.clone())
                .expect("a curve")
                .polyline()
                .into_iter()
                .map(|p| (p.x * w, p.y * h))
        })
        .collect()
}

/// Mean distance from each of `a`'s points to the nearest of `b`'s, both ways, averaged.
fn chamfer(a: &[(f64, f64)], b: &[(f64, f64)]) -> f64 {
    let one = |from: &[(f64, f64)], to: &[(f64, f64)]| {
        from.iter()
            .map(|p| {
                to.iter()
                    .map(|q| (p.0 - q.0).hypot(p.1 - q.1))
                    .fold(f64::MAX, f64::min)
            })
            .sum::<f64>()
            / from.len() as f64
    };
    0.5 * (one(a, b) + one(b, a))
}

#[test]
fn the_suggestion_follows_the_bent_edges_better_than_the_straight_quad() {
    let (_root, e, v) = open(CURVED_SAMPLE);
    let (id, crop) = (v.id, v.crops[0].id);
    // The straight quad a person would have dragged onto the page: its four corners, a few
    // pixels off (the detector alone stops half way down this sample, which no fit can mend).
    let truth: [Vec<Pt>; 4] =
        curved_sample_edges(25).map(|e| e.into_iter().map(|(x, y)| Pt::new(x, y)).collect());
    let nudge = [
        (0.006, 0.008),
        (-0.005, 0.006),
        (0.007, -0.005),
        (-0.006, -0.007),
    ];
    let corners: [Pt; 4] =
        std::array::from_fn(|i| Pt::new(truth[i][0].x + nudge[i].0, truth[i][0].y + nudge[i].1));
    let straight_view = e
        .set_crop_edit(
            id,
            crop,
            &Edit {
                quad: corners,
                quarter_turns: 0,
                fine_deg: 0.0,
            },
            false,
            "Place corners",
            None,
        )
        .unwrap();

    let s = e
        .suggest_curves(id, crop)
        .unwrap()
        .expect("the image is ready");
    assert!(!s.fell_back, "the fit was accepted");
    s.curves.validate().expect("a valid curve set");
    assert!(
        s.curves.corners().iter().zip(&s.quad).all(|(a, b)| a == b),
        "the quad is the curves' corners"
    );
    assert!(
        s.looks_curved,
        "a page sagging by 6% of its width looks curved: {s:?}"
    );
    assert!(s.max_curvature > 3.0, "{s:?}");
    assert!(
        s.confidence.iter().all(|c| *c > 0.5),
        "a clear receipt on a plain desk: {:?}",
        s.confidence
    );

    // The truth: the analytic edges of the sample.
    let truth: [Vec<Pt>; 4] =
        curved_sample_edges(25).map(|e| e.into_iter().map(|(x, y)| Pt::new(x, y)).collect());
    let gt = boundary(&truth);
    let fitted = boundary(&[
        s.curves.top.points().to_vec(),
        s.curves.right.points().to_vec(),
        s.curves.bottom.points().to_vec(),
        s.curves.left.points().to_vec(),
    ]);
    let q = s.quad;
    let straight = boundary(&[
        vec![q[0], q[1]],
        vec![q[1], q[2]],
        vec![q[2], q[3]],
        vec![q[3], q[0]],
    ]);
    let (d_fit, d_straight) = (chamfer(&fitted, &gt), chamfer(&straight, &gt));
    assert!(
        d_fit < 8.0 && d_fit * 2.0 < d_straight,
        "the curves are within {d_fit:.1} px of the truth (the straight outline {d_straight:.1} px)"
    );

    // A suggestion is only a proposal: asking changed nothing about the crop.
    let after = e.item_view(id).unwrap();
    assert_eq!(after.crops[0].curves, None);
    assert_eq!(after.crops[0].render_key, straight_view.crops[0].render_key);
    assert_eq!(after.history_position, straight_view.history_position);

    // Applying it goes through `set_curves` and makes a held, curved crop.
    let applied = e
        .set_curves(id, crop, &s.curves, false, "Suggested curves", Some(1))
        .unwrap();
    let c = applied.crops[0].curves.as_ref().expect("curved");
    assert_eq!(c, &s.curves);
    assert_eq!(applied.history_position, straight_view.history_position + 1);
    assert_eq!(applied.crops[0].band, Some(auto_crop_core::Band::Check));

    // A curved crop can be suggested again (from its corners), and the hint is the same story.
    let again = e.suggest_curves(id, crop).unwrap().expect("ready");
    assert!(again.looks_curved);
    let hint = e.curve_hint(id, crop).unwrap().expect("ready");
    assert!(hint.looks_curved && hint.curvature > 3.0);
}

#[test]
fn a_flat_page_is_not_called_curved() {
    // The sample set's flat receipts and sheets on beds.
    let root = tempfile::tempdir().unwrap();
    let engine = Engine::new(AppPaths::under(root.path()));
    engine.set_item_detector(Arc::new(NoSplit));
    let files = write_samples(&root.path().join("samples")).unwrap();
    let mut checked = 0;
    let flat = |p: &&std::path::PathBuf| {
        let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        (n.starts_with("receipt_") || n.starts_with("document_"))
            && !n.starts_with("document_4")
            && n != CURVED_SAMPLE
    };
    for path in files.iter().filter(flat) {
        let summary = engine.open_paths(std::slice::from_ref(path), false);
        let Some(id) = summary.ids.first().copied() else {
            continue;
        };
        let v = engine.analyse(id).expect("analysed");
        if v.status != ItemStatus::Ready {
            continue;
        }
        for c in &v.crops {
            let s = engine
                .suggest_curves(id, c.id)
                .unwrap()
                .expect("ready")
                .clone();
            assert!(
                !s.looks_curved,
                "{} looks curved: {:?}",
                path.display(),
                s.curvature
            );
            checked += 1;
        }
    }
    assert!(checked >= 3, "some flat crops were checked ({checked})");
}

#[test]
fn unknown_images_and_crops_are_errors() {
    let (_root, e, v) = open(CURVED_SAMPLE);
    assert!(e.suggest_curves(9999, 1).is_err());
    assert_eq!(e.suggest_curves(v.id, 9999).unwrap_err(), ErrKind::NoCrop);
}
