// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `analyze` reports whether "Trim to content" is available (docs/cli.md): a print on a wide white
//! mat carries `trim_to_content` (and is still held with `DETECTORS_DISAGREE`, the detector's
//! answer being the paper edge), an ordinary photographed page carries no such key, and `analyze`
//! writes nothing.

mod common;

use auto_crop_engine::samples::{MATTED_SAMPLE, write_samples};
use common::*;
use std::fs;

#[test]
fn analyze_reports_a_trim_for_a_print_on_a_mat_and_for_nothing_else() {
    let sb = Sandbox::new();
    let samples = write_samples(&sb.work().join("samples")).unwrap();
    let matted = samples
        .iter()
        .find(|p| p.ends_with(MATTED_SAMPLE))
        .expect("the matted sample");
    fs::copy(matted, sb.work().join("print.jpg")).unwrap();
    good_photo(&sb.work().join("plain.jpg"), 2);
    // Only the two pictures are analysed (the samples folder is not an input).
    let r = sb.run(&[
        "analyze",
        "--json",
        s(&sb.work().join("print.jpg")),
        s(&sb.work().join("plain.jpg")),
    ]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    let doc = r.json();
    let items = doc["items"].as_array().unwrap();
    let find = |name: &str| {
        items
            .iter()
            .find(|i| i["input"].as_str().unwrap().ends_with(name))
            .unwrap_or_else(|| panic!("{name} in {doc}"))
    };
    let print = find("print.jpg");
    assert_eq!(print["decision"], "held", "{print}");
    assert!(
        print["reasons"].to_string().contains("DETECTORS_DISAGREE")
            || print["hold_code"]
                .to_string()
                .contains("DETECTORS_DISAGREE")
            || print["confidence"]
                .to_string()
                .contains("DETECTORS_DISAGREE"),
        "{print}"
    );
    let trim = &print["crops"][0]["trim_to_content"];
    assert_eq!(trim["reason"], "mat", "{print}");
    assert_eq!(trim["quad"].as_array().unwrap().len(), 4);
    assert!((0.5..=0.95).contains(&trim["confidence"].as_f64().unwrap()));
    // The crop itself is still the detector's: the paper edge, not the picture.
    let crop = &print["crops"][0]["quad"];
    assert!(crop[0][0].as_f64().unwrap() < trim["quad"][0]["x"].as_f64().unwrap());
    // A page with nothing inside has no key at all.
    assert!(
        find("plain.jpg")["crops"][0]
            .get("trim_to_content")
            .is_none()
    );
}
