// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The wire shape of the enhancement calls. The Tauri commands (crates/shell/src/app.rs, behind the `gui`
//! feature) return the engine's values as they are, so what the webview receives is the serde JSON built
//! here from the real engine on the shaded sample: `EnhanceSuggestion`, the `enhance` fields of an
//! `ItemView` and its crops, and `EnhanceBatch`.
//!
//! `AUTOCROP_REGEN_FIXTURES=1 cargo test -p auto-crop-shell --test enhance_json` rewrites
//! `ui/src/lib/fixtures/enhance-*.json`, which `ui/src/lib/enhance.test.ts` reads. A normal run only
//! checks the shape, not the numbers.

use auto_crop_engine::samples::{SHADED_SAMPLES, write_samples};
use auto_crop_engine::{AppPaths, Engine, EnhanceMode, ItemStatus, NoSplit};
use serde_json::{Value, json};
use std::sync::Arc;

fn keys(v: &Value) -> Vec<String> {
    let mut k: Vec<String> = v.as_object().expect("an object").keys().cloned().collect();
    k.sort();
    k
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
fn the_enhance_calls_have_the_wire_shape_the_ui_reads() {
    let root = tempfile::tempdir().unwrap();
    let engine = Engine::new(AppPaths::under(root.path()));
    engine.set_item_detector(Arc::new(NoSplit));
    let files = write_samples(&root.path().join("samples")).unwrap();
    let path = files
        .iter()
        .find(|p| p.ends_with(SHADED_SAMPLES[0]))
        .expect("the shaded sample is in the set");
    let id = *engine
        .open_paths(std::slice::from_ref(path), false)
        .ids
        .first()
        .expect("opened");
    let v = engine.analyse(id).expect("analysed");
    assert_eq!(v.status, ItemStatus::Ready, "{:?}", v.error);
    let crop = v.crops[0].id;

    // ---- the suggestion
    let s = engine
        .suggest_enhance(id, crop)
        .expect("the engine answers")
        .expect("the shaded sample is suggested");
    let sj = serde_json::to_value(Some(&s)).unwrap();
    assert_eq!(
        keys(&sj),
        [
            "blackAndWhite",
            "blackAndWhiteHint",
            "hint",
            "measures",
            "mode",
            "reason",
            "reasons",
            "strength"
        ]
    );
    assert_eq!(sj["mode"], "clean");
    assert!(sj["strength"].is_number() && sj["reason"].is_string() && sj["reasons"].is_array());
    assert_eq!(keys(&sj["hint"]), ["gray", "recommendPng"]);
    assert_eq!(keys(&sj["blackAndWhiteHint"]), ["gray", "recommendPng"]);
    assert_eq!(sj["blackAndWhite"], "document");
    write_fixture("enhance-suggestion.json", &sj);

    // ---- the views: the default and a crop's own value
    let part = |v: &auto_crop_engine::ItemView| {
        json!({
            "itemEnhance": serde_json::to_value(v.enhance).unwrap(),
            "cropEnhance": serde_json::to_value(v.crops[0].enhance).unwrap(),
            "cropEnhanceOverride": serde_json::to_value(v.crops[0].enhance_override).unwrap(),
        })
    };
    let before = part(&engine.item_view(id).unwrap());
    let set = engine
        .set_enhance(id, crop, EnhanceMode::Clean, 0.7, false, "", None)
        .unwrap();
    let after_set = part(&set);
    let scan = engine
        .set_scan_enhance(id, EnhanceMode::Receipt, 1.0, false, "", None)
        .unwrap();
    let after_scan = part(&scan);
    assert_eq!(before["itemEnhance"], json!({"mode": "original"}));
    assert_eq!(before["cropEnhanceOverride"], Value::Null);
    assert_eq!(
        after_set["cropEnhance"],
        json!({"mode": "clean", "strength": 70})
    );
    assert_eq!(after_set["cropEnhanceOverride"], after_set["cropEnhance"]);
    assert_eq!(
        after_scan["itemEnhance"],
        json!({"mode": "receipt", "strength": 100})
    );
    assert_eq!(after_scan["cropEnhanceOverride"], Value::Null);
    write_fixture(
        "enhance-views.json",
        &json!({ "original": before, "set": after_set, "scan": after_scan }),
    );

    // ---- the batch (a fresh image, so the crop has no look yet)
    let path2 = files
        .iter()
        .find(|p| p.ends_with(SHADED_SAMPLES[1]))
        .expect("the second shaded sample is in the set");
    let id2 = *engine
        .open_paths(std::slice::from_ref(path2), false)
        .ids
        .first()
        .expect("opened");
    engine.analyse(id2).expect("analysed");
    let batch = engine.enhance_suggested(&[id2, id]);
    let bj = serde_json::to_value(&batch).unwrap();
    assert_eq!(
        keys(&bj),
        ["already", "crops", "failed", "images", "none", "views"]
    );
    assert_eq!(bj["crops"], 1, "{bj}");
    assert_eq!(bj["already"], 1, "the first image already has a look");
    assert_eq!(bj["views"].as_array().unwrap().len(), 1);
    assert_eq!(bj["views"][0]["crops"][0]["enhance"]["mode"], "clean");
    let mut small = bj.clone();
    small["views"] =
        json!([{ "id": bj["views"][0]["id"], "enhance": bj["views"][0]["crops"][0]["enhance"] }]);
    write_fixture("enhance-batch.json", &small);
}
