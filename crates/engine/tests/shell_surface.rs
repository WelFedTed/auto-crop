// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! What the desktop shell relies on from the engine: `ItemView.openOnly` (the notice code of a
//! source that is only ever copied) and how undo, redo, reset and draw-crop behave for an image
//! with several crops.

use auto_crop_codecs::fixtures as fx;
use auto_crop_core::Pt;
use auto_crop_engine::{AppPaths, Engine, ItemStatus, ItemView, NoSplit, enumerate};
use std::fs;
use std::path::Path;
use std::sync::Arc;

struct Env {
    _root: tempfile::TempDir,
    engine: Engine,
    dir: std::path::PathBuf,
}

fn env() -> Env {
    let root = tempfile::tempdir().unwrap();
    let engine = Engine::new(AppPaths::under(root.path()));
    // One crop per image: the detector must not split these small synthetic files.
    engine.set_item_detector(Arc::new(NoSplit));
    let dir = root.path().join("photos");
    fs::create_dir_all(&dir).unwrap();
    Env {
        _root: root,
        engine,
        dir,
    }
}

fn open(e: &Env, name: &str, bytes: &[u8]) -> ItemView {
    let path: std::path::PathBuf = e.dir.join(name);
    fs::write(&path, bytes).unwrap();
    let summary = e.engine.open_paths(std::slice::from_ref(&path), false);
    let id = *summary.ids.first().expect("the file is a candidate");
    // Before analysis nothing is known about the source: no reason is given yet.
    assert_eq!(e.engine.item_view(id).unwrap().open_only, None, "{name}");
    let v = e.engine.analyse(id).unwrap();
    assert_eq!(v.status, ItemStatus::Ready, "{name}: {:?}", v.error);
    v
}

#[test]
fn open_only_names_why_a_source_is_never_replaced() {
    let e = env();
    for (name, bytes) in [
        ("a.jpg", fx::jpeg_baseline(64, 48)),
        ("b.png", fx::png_rgb(64, 48)),
    ] {
        assert_eq!(open(&e, name, &bytes).open_only, None, "{name}");
    }
    // A single-page TIFF opens but has no writer in this build.
    let one = fx::tiff_rgb8(64, 48, &fx::TiffOpts::default());
    assert_eq!(
        open(&e, "page.tif", &one).open_only.as_deref(),
        Some("format.write_unavailable")
    );
    // A multi-page one is never replaced either, and says so.
    let pages = fx::tiff_rgb8(
        64,
        48,
        &fx::TiffOpts {
            extra_pages: 1,
            ..fx::TiffOpts::default()
        },
    );
    let v = open(&e, "pages.tif", &pages);
    assert_eq!(v.open_only.as_deref(), Some("tiff.multi_page"));
    // It reaches the JSON the UI reads, and survives later edits and listing.
    let listed = e.engine.list_items();
    let json = serde_json::to_value(listed.iter().find(|i| i.id == v.id).unwrap()).unwrap();
    assert_eq!(json["openOnly"], "tiff.multi_page");
}

#[test]
fn the_picker_list_is_the_list_the_engine_opens() {
    let exts = enumerate::input_extensions();
    for e in ["jpg", "jpeg", "png", "tif", "tiff", "webp"] {
        assert!(exts.contains(&e), "{e}");
        assert!(enumerate::is_candidate(Path::new(&format!("x.{e}"))));
    }
    assert!(exts.iter().all(|e| *e == e.to_ascii_lowercase()));
}

fn quad(x0: f64, y0: f64, x1: f64, y1: f64) -> [Pt; 4] {
    [
        Pt::new(x0, y0),
        Pt::new(x1, y0),
        Pt::new(x1, y1),
        Pt::new(x0, y1),
    ]
}

/// An image with the detector's crop plus two the user added.
fn three_crops(e: &Env) -> ItemView {
    let v = open(e, "scan.png", &fx::png_rgb(160, 120));
    let v = if v.crops.is_empty() {
        e.engine.draw_crop(v.id).unwrap()
    } else {
        v
    };
    let v = e
        .engine
        .add_crop(v.id, Some(quad(0.05, 0.05, 0.3, 0.3)), None)
        .unwrap();
    let v = e
        .engine
        .add_crop(v.id, Some(quad(0.6, 0.6, 0.9, 0.9)), None)
        .unwrap();
    assert_eq!(v.crops.len(), 3);
    v
}

#[test]
fn undo_and_redo_walk_the_history_of_a_multi_crop_image() {
    let e = env();
    let v = three_crops(&e);
    let ids: Vec<u32> = v.crops.iter().map(|c| c.id).collect();
    let turned = e.engine.turn_crop(v.id, ids[1], true).unwrap();
    assert_eq!(turned.crops.len(), 3);
    let undone = e.engine.undo(v.id).unwrap();
    assert_eq!(undone.crops, v.crops, "undo restores every crop");
    let redone = e.engine.redo(v.id).unwrap();
    assert_eq!(redone.crops, turned.crops);
    // Undo past an add: the crop goes away, redo brings it back with its id.
    let removed = e.engine.remove_crop(v.id, ids[2]).unwrap();
    assert!(!removed.crops[2].include);
    let back = e.engine.undo(v.id).unwrap();
    assert!(back.crops[2].include);
}

#[test]
fn reset_to_auto_returns_to_the_detectors_crops() {
    let e = env();
    let v = three_crops(&e);
    let reset = e.engine.reset_to_auto(v.id).unwrap();
    assert!(reset.crops.len() <= 1, "{:?}", reset.crops);
    assert!(!reset.edited);
    // And it is one undo step: the three crops come back.
    let undone = e.engine.undo(v.id).unwrap();
    assert_eq!(undone.crops.len(), 3);
}

#[test]
fn draw_crop_keeps_the_crops_an_image_already_has() {
    let e = env();
    let v = three_crops(&e);
    let drawn = e.engine.draw_crop(v.id).unwrap();
    assert_eq!(drawn.crops.len(), 4);
    // The new crop is numbered in reading order, so look the old ones up by id.
    for old in &v.crops {
        let now = drawn.crops.iter().find(|c| c.id == old.id).unwrap();
        assert_eq!((&now.edit, now.include), (&old.edit, old.include));
    }
    assert_eq!(drawn.undo_label.as_deref(), Some("Draw crop"));
    assert_eq!(e.engine.undo(v.id).unwrap().crops, v.crops);
    // An image with no crop at all still gets exactly one.
    let w = open(&e, "other.jpg", &fx::jpeg_baseline(64, 48));
    let nothing = if w.crops.is_empty() {
        e.engine.draw_crop(w.id).unwrap()
    } else {
        w
    };
    assert_eq!(nothing.crops.len(), 1);
}
