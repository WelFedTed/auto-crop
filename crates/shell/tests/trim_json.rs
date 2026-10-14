// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The wire shape of `trim_to_content`. The Tauri command (crates/shell/src/app.rs, behind the `gui`
//! feature) returns `Engine::trim_to_content` as it is, so what the webview receives is the serde JSON
//! of `Option<TrimProposal>`. This test builds that JSON from the real engine on the matted sample and
//! checks the field names the TypeScript type (`ui/src/lib/types.ts`) relies on, and that a page with no
//! picture inside answers `null`.
//!
//! `AUTOCROP_REGEN_FIXTURES=1 cargo test -p auto-crop-shell --test trim_json` rewrites
//! `ui/src/lib/fixtures/trim-proposal-matted.json`, which `ui/src/lib/trim.test.ts` reads. A normal run
//! only checks the shape, not the numbers.

use auto_crop_engine::samples::{MATTED_SAMPLE, write_samples};
use auto_crop_engine::{AppPaths, Engine, ItemStatus, NoSplit};
use serde_json::Value;
use std::sync::Arc;

fn answer_json(wanted: &dyn Fn(&std::path::Path) -> bool) -> Value {
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
    let answer = engine
        .trim_to_content(id, crop)
        .expect("the engine answers");
    // The command's return value is `Option<TrimProposal>`: `Some` is the object, `None` is null.
    serde_json::to_value(answer).unwrap()
}

#[test]
fn the_proposal_on_the_matted_sample_has_the_wire_shape_the_ui_reads() {
    let j = answer_json(&|p| p.ends_with(MATTED_SAMPLE));
    let mut keys: Vec<&str> = j
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["confidence", "quad", "reason"]);
    assert_eq!(j["quad"].as_array().unwrap().len(), 4);
    for p in j["quad"].as_array().unwrap() {
        assert!(p["x"].is_number() && p["y"].is_number());
    }
    assert!(j["confidence"].is_number());
    assert!(matches!(j["reason"].as_str(), Some("mat" | "thinBorder")));
    if std::env::var_os("AUTOCROP_REGEN_FIXTURES").is_some() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/src/lib/fixtures");
        std::fs::create_dir_all(&dir).unwrap();
        let text = serde_json::to_string_pretty(&j).unwrap() + "\n";
        std::fs::write(dir.join("trim-proposal-matted.json"), text).unwrap();
    }
}

#[test]
fn a_page_with_nothing_to_trim_to_answers_null() {
    let j = answer_json(&|p| p.ends_with("receipt_1_tilted.jpg"));
    assert!(j.is_null(), "{j}");
}
