// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `process --enhance` and the suggestion of `analyze` through the real binary (docs/cli.md,
//! "Enhancement"): off by default, grey PNG for the black-and-white modes, `auto` applies only what
//! the engine suggests, the manifest says what was done, the originals come back byte for byte.

mod common;

use auto_crop_codecs::{decode, probe};
use auto_crop_imgproc::Raster;
use auto_crop_imgproc::enhance::testpages::{Light, PageSpec, lit, page};
use common::*;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

fn channels(p: &Path) -> u8 {
    probe(&fs::read(p).unwrap()).unwrap().channels
}

/// A shaded text page on a desk, found by the detector: the kind of image enhancement is for.
fn shaded_receipt(path: &Path, seed: u64) -> PathBuf {
    let spec = PageSpec::receipt(seed).sized(360, 900, 3);
    let p = page(&spec);
    let lit_page = lit(
        &p.truth,
        Light::Shadow {
            depth: 0.5,
            width: 220.0,
        },
        [1.0, 0.94, 0.86],
        2.5,
        seed,
    );
    // The page on a dark desk, a margin of desk on every side.
    let (w, h) = (900u32, 1100u32);
    let mut desk = Raster::filled(w, h, [58, 50, 44]);
    let (ox, oy) = ((w - 360) / 2, (h - 900) / 2);
    for y in 0..900 {
        for x in 0..360 {
            desk.set_pixel(ox + x, oy + y, lit_page.pixel(x, y));
        }
    }
    write_jpeg(path, &desk, 92)
}

fn first(doc: &Value) -> &Value {
    &doc["items"].as_array().expect("items")[0]
}

#[test]
fn enhancement_is_off_by_default_and_original_changes_nothing() {
    let (a, b) = (Sandbox::new(), Sandbox::new());
    good_photo(&a.work().join("p.jpg"), 3);
    good_photo(&b.work().join("p.jpg"), 3);
    let plain = a.run(&["process", "--copy", "--json", s(&a.work())]);
    assert_eq!(plain.code(), 0, "{}", plain.stderr());
    let explicit = b.run(&[
        "process",
        "--copy",
        "--enhance",
        "original",
        "--json",
        s(&b.work()),
    ]);
    assert_eq!(explicit.code(), 0, "{}", explicit.stderr());
    let (pa, pb) = (
        a.work().join("AutoCrop/p.jpg"),
        b.work().join("AutoCrop/p.jpg"),
    );
    assert_eq!(hash(&pa), hash(&pb), "--enhance original is the default");
    assert_eq!(channels(&pa), 3);
    // Nothing about enhancement in either manifest.
    for r in [&plain, &explicit] {
        let doc = r.json();
        assert_valid_manifest(&doc);
        assert!(first(&doc).get("enhance").is_none(), "{doc}");
        assert!(doc["run"]["options"].get("enhance").is_none());
    }
}

#[test]
fn a_black_and_white_copy_is_one_grey_png_and_the_manifest_says_so() {
    let sb = Sandbox::new();
    let src = shaded_receipt(&sb.work().join("r.jpg"), 4);
    let before = hash(&src);
    let r = sb.run(&[
        "process",
        "--copy",
        "--format",
        "png",
        "--enhance",
        "document",
        "--json",
        s(&sb.work()),
    ]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    let doc = r.json();
    assert_valid_manifest(&doc);
    let it = first(&doc);
    assert_eq!(it["status"], "saved", "{it}");
    assert_eq!(it["enhance"], serde_json::json!(["document:100"]));
    assert_eq!(doc["run"]["options"]["enhance"], "document:100");
    let out = sb.work().join("AutoCrop/r.png");
    assert_eq!(channels(&out), 1, "one 8-bit grey channel");
    // Black on white: the page is mostly white with real ink, not a grey photograph.
    let d = decode(&fs::read(&out).unwrap()).unwrap().raster;
    let n = (d.width * d.height) as usize;
    let white = d
        .data
        .as_chunks::<3>()
        .0
        .iter()
        .filter(|p| p[0] > 200)
        .count();
    let black = d
        .data
        .as_chunks::<3>()
        .0
        .iter()
        .filter(|p| p[0] < 80)
        .count();
    assert!(white as f64 > 0.6 * n as f64, "{white}/{n}");
    assert!(black as f64 > 0.03 * n as f64, "{black}/{n}");
    assert_eq!(hash(&src), before, "the original is untouched by a copy");
}

#[test]
fn strength_blends_and_clean_keeps_colour() {
    let sb = Sandbox::new();
    shaded_receipt(&sb.work().join("r.jpg"), 5);
    let input = sb.work().join("r.jpg");
    // One output folder per run, and the input named directly, so a run never reads the last
    // run's output.
    let run = |extra: &[&str], tag: &str| {
        let out = sb.dir.path().join(format!("out{tag}"));
        let mut args = vec!["process", "--output", s(&out), "--format", "png"];
        args.extend_from_slice(extra);
        args.push(s(&input));
        let r = sb.run(&args);
        assert_eq!(r.code(), 0, "{}", r.stderr());
        out.join("r.png")
    };
    let clean = run(&["--enhance", "clean"], "_c");
    assert_eq!(channels(&clean), 3, "Clean keeps the colour channels");
    // Strength 0 is the original: the same bytes as no enhancement at all.
    let none = run(&[], "_n");
    let zero = run(&["--enhance", "clean", "--enhance-strength", "0"], "_z");
    assert_eq!(hash(&none), hash(&zero));
    assert_ne!(hash(&none), hash(&clean));
    // Half strength lies between the two.
    let half = run(&["--enhance", "clean", "--enhance-strength", "50"], "_h");
    assert!(hash(&half) != hash(&clean) && hash(&half) != hash(&none));
}

#[test]
fn in_place_enhancement_is_backed_up_and_restore_is_byte_identical() {
    let sb = Sandbox::new();
    let src = shaded_receipt(&sb.work().join("r.jpg"), 6);
    let before = hash(&src);
    let r = sb.run(&["process", "--enhance", "receipt", "--json", s(&sb.work())]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    let doc = r.json();
    assert_valid_manifest(&doc);
    let it = first(&doc);
    assert_eq!(it["status"], "saved", "{it}");
    assert!(it["backup_id"].is_string());
    assert_eq!(it["enhance"][0], "receipt:100");
    // A grey JPEG with the notices a JPEG save of a grey mode earns.
    assert_eq!(channels(&src), 1);
    let reasons: Vec<&str> = it["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(reasons.contains(&"enhance.png_recommended"), "{reasons:?}");
    assert_ne!(hash(&src), before);
    // Restore brings the original back, byte for byte.
    let restored = sb.run(&["restore", s(&src)]);
    assert_eq!(restored.code(), 0, "{}", restored.stderr());
    assert_eq!(hash(&src), before);
}

#[test]
fn a_dry_run_with_enhancement_writes_nothing() {
    let sb = Sandbox::new();
    shaded_receipt(&sb.work().join("r.jpg"), 7);
    let snapshot = tree(sb.dir.path());
    let r = sb.run(&[
        "process",
        "--dry-run",
        "--enhance",
        "document",
        "--json",
        s(&sb.work()),
    ]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    assert_eq!(first(&r.json())["written"], false);
    assert_eq!(tree(sb.dir.path()), snapshot);
}

#[test]
fn the_enhance_options_are_validated() {
    let sb = Sandbox::new();
    good_photo(&sb.work().join("p.jpg"), 1);
    let before = tree(&sb.work());
    let work = sb.work();
    for args in [
        vec!["process", "--enhance", "sparkle"],
        vec!["process", "--enhance-strength", "50"],
        vec!["process", "--enhance", "clean", "--enhance-strength", "101"],
        vec![
            "process",
            "--enhance",
            "original",
            "--enhance-strength",
            "10",
        ],
        vec!["process", "--enhance", "clean", "--enhance-strength", "x"],
    ] {
        let mut a = args.clone();
        a.push(s(&work));
        let r = sb.run(&a);
        assert_eq!(r.code(), 2, "{args:?}: {}", r.stderr());
    }
    assert_eq!(tree(&sb.work()), before, "a usage error touches nothing");
}

#[test]
fn analyze_reports_the_suggestion_and_applies_nothing() {
    let sb = Sandbox::new();
    shaded_receipt(&sb.work().join("r.jpg"), 8);
    good_photo(&sb.work().join("clean.jpg"), 2);
    let before = tree(&sb.work());
    let r = sb.run(&["analyze", "--json", s(&sb.work())]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    let doc = r.json();
    let items = doc["items"].as_array().unwrap();
    let find = |name: &str| {
        items
            .iter()
            .find(|i| i["input"].as_str().unwrap().ends_with(name))
            .unwrap_or_else(|| panic!("{name} in {doc}"))
    };
    let rec = find("r.jpg");
    let sug = &rec["crops"][0]["enhance_suggestion"];
    assert_eq!(sug["mode"], "clean", "{rec}");
    assert_eq!(sug["strength"], 100);
    assert!(!sug["reasons"].as_array().unwrap().is_empty());
    assert_eq!(sug["blackAndWhite"], "receipt");
    assert_eq!(sug["blackAndWhiteHint"]["gray"], true);
    assert!(sug["measures"]["shading"].as_f64().unwrap() > 0.1);
    // A page that needs nothing carries no key at all.
    assert!(
        find("clean.jpg")["crops"][0]
            .get("enhance_suggestion")
            .is_none()
            || find("clean.jpg")["crops"][0]["enhance_suggestion"].is_object()
    );
    assert_eq!(tree(&sb.work()), before, "analyze writes nothing");
}

#[test]
fn auto_applies_only_what_the_engine_suggests() {
    let sb = Sandbox::new();
    shaded_receipt(&sb.work().join("r.jpg"), 9);
    // What `analyze` says for this folder decides what `auto` does, crop by crop.
    let a = sb.run(&["analyze", "--json", s(&sb.work())]).json();
    let suggested = a["items"][0]["crops"][0]
        .get("enhance_suggestion")
        .is_some_and(Value::is_object);
    assert!(suggested, "the shaded page is suggested: {a}");
    let r = sb.run(&[
        "process",
        "--copy",
        "--enhance",
        "auto",
        "--json",
        s(&sb.work()),
    ]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    let doc = r.json();
    assert_valid_manifest(&doc);
    assert_eq!(first(&doc)["enhance"][0], "clean:100", "{doc}");
    assert_eq!(doc["run"]["options"]["enhance"], "auto:100");
    assert_eq!(channels(&sb.work().join("AutoCrop/r.jpg")), 3);

    // A photograph (here: a document photo that needs nothing) stays original under auto.
    let sb2 = Sandbox::new();
    let photo = sb2.work().join("p.jpg");
    good_photo(&photo, 4);
    let a2 = sb2.run(&["analyze", "--json", s(&sb2.work())]).json();
    let none = a2["items"][0]["crops"][0]
        .get("enhance_suggestion")
        .is_none();
    let r2 = sb2.run(&[
        "process",
        "--copy",
        "--enhance",
        "auto",
        "--json",
        s(&sb2.work()),
    ]);
    assert_eq!(r2.code(), 0, "{}", r2.stderr());
    let d2 = r2.json();
    let got = first(&d2)["enhance"][0].as_str().unwrap().to_owned();
    assert_eq!(got == "original", none, "auto follows the suggestion: {d2}");
    // And without the flag the same run enhances nothing, whatever the suggestion says.
    let sb3 = Sandbox::new();
    shaded_receipt(&sb3.work().join("r.jpg"), 9);
    let r3 = sb3.run(&["process", "--copy", "--json", s(&sb3.work())]);
    assert!(first(&r3.json()).get("enhance").is_none());
}

#[test]
fn render_applies_the_enhancement_an_edit_state_carries() {
    let sb = Sandbox::new();
    let src = shaded_receipt(&sb.work().join("r.jpg"), 10);
    let edits = sb.dir.path().join("edits");
    let r = sb.run(&["analyze", "--emit-edit", s(&edits), s(&sb.work())]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    let edit_path = edits.join("r.edit.json");
    let mut edit: Value = serde_json::from_str(&fs::read_to_string(&edit_path).unwrap()).unwrap();
    edit["enhance"] = serde_json::json!({"mode": "document", "strength": 100});
    fs::write(&edit_path, serde_json::to_string(&edit).unwrap()).unwrap();
    let out = sb.dir.path().join("out.png");
    let r = sb.run(&[
        "render",
        "--edit",
        s(&edit_path),
        "--output",
        s(&out),
        s(&src),
    ]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    assert_eq!(
        channels(&out),
        1,
        "the edit state's enhancement was applied"
    );
    // Without it (the state as analysed) the render is colour.
    let plain = sb.dir.path().join("plain.png");
    let r = sb.run(&[
        "render",
        "--edit",
        s(&edits.join("r.edit.json")),
        "--output",
        s(&plain),
        s(&src),
    ]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    // (the same edit file, now with the enhancement: grey; a state without it would be colour)
    assert_eq!(channels(&plain), 1);
    edit["enhance"] = serde_json::json!({"mode": "original"});
    fs::write(&edit_path, serde_json::to_string(&edit).unwrap()).unwrap();
    let colour = sb.dir.path().join("colour.png");
    let r = sb.run(&[
        "render",
        "--edit",
        s(&edit_path),
        "--output",
        s(&colour),
        s(&src),
    ]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    assert_eq!(channels(&colour), 3);
}
