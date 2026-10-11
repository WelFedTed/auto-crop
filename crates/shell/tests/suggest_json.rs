// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The wire shape of `suggest_curves`. The Tauri command (crates/shell/src/app.rs, behind the `gui`
//! feature) returns `Engine::suggest_curves` as it is, so what the webview receives is the serde JSON
//! of `CurveSuggestion`. This test builds that JSON from the real engine on the curved sample and
//! checks the field names the TypeScript type (`ui/src/lib/types.ts`) relies on.
//!
//! `AUTOCROP_REGEN_FIXTURES=1 cargo test -p auto-crop-shell --test suggest_json` rewrites
//! `ui/src/lib/fixtures/curve-suggestion-curved.json` and `...-flat.json`, which `ui/src/lib/suggest.test.ts`
//! reads. A normal run only checks the shape, not the numbers (the fit may be improved).

use auto_crop_engine::samples::{CURVED_SAMPLE, write_samples};
use auto_crop_engine::{AppPaths, Engine, ItemStatus, NoSplit};
use serde_json::Value;
use std::sync::Arc;

fn suggestion_json(wanted: &dyn Fn(&std::path::Path) -> bool) -> Value {
    let root = tempfile::tempdir().unwrap();
    let engine = Engine::new(AppPaths::under(root.path()));
    engine.set_item_detector(Arc::new(NoSplit));
    let files = write_samples(&root.path().join("samples")).unwrap();
    let path = files
        .iter()
        .find(|p| wanted(p))
        .expect("the sample is in the set");
    let summary = engine.open_paths(std::slice::from_ref(path), false);
    let id = *summary.ids.first().expect("opened");
    let v = engine.analyse(id).expect("analysed");
    assert_eq!(v.status, ItemStatus::Ready, "{:?}", v.error);
    let crop = v.crops[0].id;
    let s = engine
        .suggest_curves(id, crop)
        .expect("the engine answers")
        .expect("the image is ready");
    // The command's return value is `Option<CurveSuggestion>`: `Some` serialises as the object itself.
    serde_json::to_value(Some(s)).unwrap()
}

fn keys(v: &Value) -> Vec<String> {
    let mut k: Vec<String> = v.as_object().expect("an object").keys().cloned().collect();
    k.sort();
    k
}

fn check_shape(j: &Value) {
    assert_eq!(
        keys(j),
        [
            "bulge",
            "confidence",
            "curvature",
            "curves",
            "fellBack",
            "looksCurved",
            "maxCurvature",
            "quad",
            "supportGain"
        ]
    );
    for k in ["bulge", "confidence", "curvature", "supportGain"] {
        assert_eq!(j[k].as_array().expect("an array").len(), 4, "{k}");
    }
    assert_eq!(j["quad"].as_array().unwrap().len(), 4);
    assert!(j["quad"][0]["x"].is_number() && j["quad"][0]["y"].is_number());
    assert!(j["maxCurvature"].is_number());
    assert!(j["looksCurved"].is_boolean() && j["fellBack"].is_boolean());
    assert_eq!(
        keys(&j["curves"]),
        ["bottom", "left", "mirror", "quarterTurns", "right", "top"]
    );
    for edge in ["top", "right", "bottom", "left"] {
        let pts = j["curves"][edge]
            .as_array()
            .expect("a bare array of points");
        assert!((2..=9).contains(&pts.len()), "{edge}: {} points", pts.len());
        assert!(pts[0]["x"].is_number() && pts[0]["y"].is_number());
    }
}

fn write_fixture(name: &str, j: &Value) {
    if std::env::var_os("AUTOCROP_REGEN_FIXTURES").is_some() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/src/lib/fixtures");
        std::fs::create_dir_all(&dir).unwrap();
        let text = serde_json::to_string_pretty(j).unwrap() + "\n";
        std::fs::write(dir.join(name), text).unwrap();
    }
}

#[test]
fn the_suggestion_on_the_curved_sample_has_the_wire_shape_the_ui_reads() {
    let j = suggestion_json(&|p| p.ends_with(CURVED_SAMPLE));
    check_shape(&j);
    write_fixture("curve-suggestion-curved.json", &j);
}

#[test]
fn the_suggestion_on_a_flat_sample_has_the_same_shape() {
    let j = suggestion_json(&|p| {
        p.extension().is_some_and(|e| e == "jpg") && !p.ends_with(CURVED_SAMPLE)
    });
    check_shape(&j);
    write_fixture("curve-suggestion-flat.json", &j);
}
