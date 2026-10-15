// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `process --pdf FILE` through the real binary (ROADMAP M11.54): every written crop is a page of ONE
//! PDF, in the order asked for (`--order name|given`, numbers numerically), originals untouched, an
//! existing file never replaced, held images left out (exit 4), no document when a page fails.

mod common;

use auto_crop_codecs::encoders::pdf::{PageCodec, read_pages};
use common::*;
use std::fs;
use std::path::{Path, PathBuf};

/// A document photo whose page (the crop) has a size of its own, so pages can be told apart.
fn photo_with(path: &Path, seed: u64, inset: f64) -> PathBuf {
    let c = [
        (0.10 + inset, 0.10 + inset),
        (0.90 - inset, 0.12 + inset),
        (0.88 - inset, 0.90 - inset),
        (0.12 + inset, 0.88 - inset),
    ];
    write_jpeg(path, &doc_scene(seed, c), 90)
}

/// With `AUTO_CROP_KEEP_PDF_SAMPLES=DIR` the documents (and their references) are copied to DIR for
/// the independent readers of `tools/interop/check-output-formats.sh` (CI, job "Output formats").
fn keep(name: &str, from: &Path) {
    if let Ok(dir) = std::env::var("AUTO_CROP_KEEP_PDF_SAMPLES") {
        fs::create_dir_all(&dir).unwrap();
        fs::copy(from, Path::new(&dir).join(name)).unwrap();
    }
}

fn dims_of(pdf: &Path) -> Vec<(u32, u32)> {
    read_pages(&fs::read(pdf).unwrap())
        .expect("a PDF the repo's verifier reads back")
        .iter()
        .map(|p| (p.width_px, p.height_px))
        .collect()
}

fn items(doc: &serde_json::Value) -> &Vec<serde_json::Value> {
    doc["items"].as_array().unwrap()
}

#[test]
fn every_written_crop_is_a_page_of_one_pdf_originals_untouched() {
    let sb = Sandbox::new();
    let a = photo_with(&sb.work().join("a.jpg"), 1, 0.0);
    let b = photo_with(&sb.work().join("b.jpg"), 2, 0.08);
    let before = tree(&sb.work());
    let out = sb.dir.path().join("out.pdf");
    let r = sb.run(&["process", "--pdf", s(&out), "--json", s(&a), s(&b)]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    let doc = r.json();
    assert_valid_manifest(&doc);
    assert_eq!(doc["run"]["options"]["mode"], "pdf");
    assert_eq!(doc["summary"]["saved"], 2);
    assert_eq!(doc["summary"]["files_written"], 1, "one document");
    assert_eq!(doc["pdf"]["pages"], 2);
    assert_eq!(doc["pdf"]["renamed"], false);
    assert_eq!(doc["pdf"]["order"], "name");
    for it in items(&doc) {
        assert_eq!(it["status"], "saved", "{it}");
        assert_eq!(it["outputs"][0]["format"], "pdf");
        assert_eq!(it["written"], true);
        assert!(it["backup_id"].is_null(), "no backup: nothing is replaced");
    }
    let pages = dims_of(&out);
    assert_eq!(pages.len(), 2);
    assert_ne!(pages[0], pages[1], "two different crops");
    keep("pdf_run_two.pdf", &out);
    // Nothing else was written: the folder holds the originals and nothing new, and no backup exists.
    assert_eq!(tree(&sb.work()), before);
    assert!(!sb.work().join("AutoCrop").exists());
    assert!(
        !sb.home().join("backups").exists() || tree(&sb.home().join("backups")).is_empty(),
        "a copy needs no backup"
    );
}

#[test]
fn name_order_reads_numbers_as_numbers_and_given_order_is_the_command_line() {
    let sb = Sandbox::new();
    let p1 = photo_with(&sb.work().join("scan1.jpg"), 1, 0.0);
    let p2 = photo_with(&sb.work().join("scan2.jpg"), 2, 0.07);
    let p10 = photo_with(&sb.work().join("scan10.jpg"), 3, 0.14);
    let run = |args: &[&str]| {
        let r = sb.run(args);
        assert_eq!(r.code(), 0, "{}", r.stderr());
    };
    let given = sb.dir.path().join("given.pdf");
    run(&[
        "process",
        "--pdf",
        s(&given),
        "--order",
        "given",
        s(&p1),
        s(&p2),
        s(&p10),
    ]);
    let d = dims_of(&given);
    assert_eq!(d.len(), 3);
    assert!(d[0] != d[1] && d[1] != d[2] && d[0] != d[2]);
    // Typed in a scrambled order, by name: 1, 2, 10 (not 1, 10, 2).
    let named = sb.dir.path().join("named.pdf");
    run(&["process", "--pdf", s(&named), s(&p10), s(&p1), s(&p2)]);
    assert_eq!(dims_of(&named), d);
    keep("pdf_run_three.pdf", &named);
    // Given order is the order typed.
    let rev = sb.dir.path().join("rev.pdf");
    run(&[
        "process",
        "--pdf",
        s(&rev),
        "--order",
        "given",
        s(&p10),
        s(&p2),
        s(&p1),
    ]);
    assert_eq!(dims_of(&rev), vec![d[2], d[1], d[0]]);
}

#[test]
fn a_held_image_is_left_out_and_the_run_says_so() {
    let sb = Sandbox::new();
    let a = photo_with(&sb.work().join("a.jpg"), 1, 0.0);
    let bad = no_document(&sb.work().join("b.jpg"));
    let out = sb.dir.path().join("out.pdf");
    let r = sb.run(&["process", "--pdf", s(&out), "--json", s(&a), s(&bad)]);
    assert_eq!(r.code(), 4, "held items give exit 4: {}", r.stderr());
    let doc = r.json();
    assert_valid_manifest(&doc);
    assert_eq!(doc["pdf"]["pages"], 1);
    let by_name = |n: &str| {
        items(&doc)
            .iter()
            .find(|i| i["input"].as_str().unwrap().ends_with(n))
            .unwrap()
    };
    assert_eq!(by_name("a.jpg")["status"], "saved");
    assert_eq!(by_name("b.jpg")["status"], "held");
    assert_eq!(dims_of(&out).len(), 1);
    // Only a held image: no document at all.
    let none = sb.dir.path().join("none.pdf");
    let r = sb.run(&["process", "--pdf", s(&none), "--json", s(&bad)]);
    assert_eq!(r.code(), 4);
    assert!(!none.exists());
    assert!(r.json().get("pdf").is_none());
}

#[test]
fn an_existing_file_is_never_replaced() {
    let sb = Sandbox::new();
    let a = photo_with(&sb.work().join("a.jpg"), 1, 0.0);
    let out = sb.dir.path().join("out.pdf");
    fs::write(&out, b"not yours to overwrite").unwrap();
    let r = sb.run(&["process", "--pdf", s(&out), "--json", s(&a)]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    let doc = r.json();
    assert_eq!(doc["pdf"]["renamed"], true);
    assert!(
        doc["pdf"]["path"]
            .as_str()
            .unwrap()
            .ends_with("out (2).pdf")
    );
    assert_eq!(fs::read(&out).unwrap(), b"not yours to overwrite");
    assert_eq!(dims_of(&sb.dir.path().join("out (2).pdf")).len(), 1);
}

#[test]
fn a_dry_run_writes_nothing_and_names_the_document() {
    let sb = Sandbox::new();
    let a = photo_with(&sb.work().join("a.jpg"), 1, 0.0);
    let before = tree(sb.dir.path());
    let out = sb.dir.path().join("out.pdf");
    let r = sb.run(&["process", "--pdf", s(&out), "--dry-run", "--json", s(&a)]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    let doc = r.json();
    assert_valid_manifest(&doc);
    assert!(doc["pdf"]["pages"].is_null());
    assert!(doc["pdf"]["path"].as_str().unwrap().ends_with("out.pdf"));
    assert_eq!(tree(sb.dir.path()), before);
}

#[test]
fn dpi_and_enhance_reach_the_pages() {
    let sb = Sandbox::new();
    let a = photo_with(&sb.work().join("a.jpg"), 1, 0.0);
    let out = sb.dir.path().join("out.pdf");
    let r = sb.run(&["process", "--pdf", s(&out), "--dpi", "150", "--json", s(&a)]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    let pages = read_pages(&fs::read(&out).unwrap()).unwrap();
    let p = &pages[0];
    assert!((p.media_box.0 - f64::from(p.width_px) * 72.0 / 150.0).abs() < 0.01);
    assert_eq!(r.json()["pdf"]["dpi"], 150);
    // Enhanced (document): a lossless grey page.
    let out2 = sb.dir.path().join("doc.pdf");
    let r = sb.run(&[
        "process",
        "--pdf",
        s(&out2),
        "--enhance",
        "document",
        "--json",
        s(&a),
    ]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    let pages = read_pages(&fs::read(&out2).unwrap()).unwrap();
    assert_eq!((pages[0].codec, pages[0].channels), (PageCodec::Flate, 1));
    // The same crop and enhancement as a PNG: the page must be those pixels exactly (an independent
    // reader extracts the page image and compares it with this file).
    let png_dir = sb.dir.path().join("png");
    let r = sb.run(&[
        "process",
        "--output",
        s(&png_dir),
        "--format",
        "png",
        "--enhance",
        "document",
        s(&a),
    ]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    let reference = png_dir.join("a.png");
    let want = auto_crop_codecs::decode(&fs::read(&reference).unwrap())
        .unwrap()
        .raster;
    let (w, h, rgb) = pages[0].to_rgb().unwrap();
    assert_eq!((w, h), (want.width, want.height));
    assert_eq!(
        rgb, want.data,
        "the Flate page is the enhanced crop, exactly"
    );
    keep("pdf_run_enhanced.pdf", &out2);
    keep("pdf_run_enhanced_ref.png", &reference);
}

#[test]
fn what_does_not_fit_a_pdf_is_a_usage_error_and_touches_nothing() {
    let sb = Sandbox::new();
    let a = photo_with(&sb.work().join("a.jpg"), 1, 0.0);
    let out = sb.dir.path().join("out.pdf");
    let before = tree(sb.dir.path());
    for bad in [
        vec!["process", "--pdf", s(&out), "--copy", s(&a)],
        vec!["process", "--pdf", s(&out), "--output", "x", s(&a)],
        vec!["process", "--pdf", s(&out), "--format", "png", s(&a)],
        vec![
            "process",
            "--pdf",
            s(&out),
            "--name-template",
            "{name}",
            s(&a),
        ],
        vec!["process", "--pdf", "out.txt", s(&a)],
        vec!["process", "--order", "name", s(&a)],
        vec!["process", "--dpi", "150", s(&a)],
        vec!["process", "--pdf", s(&out), "--order", "random", s(&a)],
        vec!["process", "--pdf", s(&out), "--dpi", "5", s(&a)],
    ] {
        let r = sb.run(&bad);
        assert_eq!(r.code(), 2, "{bad:?}: {}", r.stderr());
    }
    assert_eq!(tree(sb.dir.path()), before);
}
