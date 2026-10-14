// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! TIFF, lossless WebP and PDF output through the real binary (ROADMAP M11.52 to M11.54, M7.31):
//! `convert --to tiff|webp|pdf`, `process --format`, a TIFF or WebP source replaced in place with
//! a backup that restore undoes, `--tiff-compression`, and `--one-bit` (CCITT Group 4) with its
//! refusals.

mod common;

use auto_crop_codecs::encoders::pdf::{PageCodec, read_pages};
use auto_crop_codecs::encoders::{TiffEncoder, WebpEncoder};
use auto_crop_codecs::fixtures::{pattern, png_rgb};
use auto_crop_codecs::{Format, decode, probe};
use auto_crop_core::ports::{EncodeMeta, Encoder, PixelFormat, Raster as CoreRaster, Samples};
use auto_crop_core::{CancelToken, OutputSpec};
use common::*;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

fn items(doc: &Value) -> &Vec<Value> {
    doc["items"].as_array().expect("items")
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    let p = dir.join(name);
    fs::write(&p, bytes).unwrap();
    p
}

/// A detectable document photo as a TIFF or a lossless WebP (the writers of this build).
fn document_as(path: &Path, seed: u64, fmt: Format) -> Vec<u8> {
    let r = doc_scene(seed, GOOD);
    let core = CoreRaster::from_samples(r.width, r.height, PixelFormat::Rgb8, Samples::U8(r.data))
        .unwrap();
    let (spec, meta, never) = (
        OutputSpec::default(),
        EncodeMeta {
            icc: None,
            dpi: Some((300, 300)),
        },
        CancelToken::never(),
    );
    let bytes = match fmt {
        Format::Tiff => TiffEncoder::default().encode(&core.view(), &spec, &meta, &never),
        _ => WebpEncoder::default().encode(&core.view(), &spec, &meta, &never),
    }
    .unwrap();
    write(
        path.parent().unwrap(),
        path.file_name().unwrap().to_str().unwrap(),
        &bytes,
    );
    bytes
}

#[test]
fn convert_to_tiff_webp_and_pdf_writes_beside_the_originals_and_names_the_format_in_the_manifest() {
    for (to, ext, format) in [
        ("tiff", "tif", "tiff"),
        ("tif", "tif", "tiff"),
        ("webp", "webp", "webp"),
        ("pdf", "pdf", "pdf"),
    ] {
        let sb = Sandbox::new();
        let png = png_rgb(64, 48);
        let src = write(&sb.work(), "a.png", &png);
        let r = sb.run(&[
            "convert",
            "--to",
            to,
            "--copy",
            "--colour",
            "preserve",
            "--json",
            s(&sb.work()),
        ]);
        assert_eq!(r.code(), 0, "{to}: {}", r.stderr());
        let doc = r.json();
        assert_valid_manifest(&doc);
        let it = &items(&doc)[0];
        assert_eq!(it["status"], "saved", "{to}: {it}");
        assert_eq!(it["outputs"][0]["format"], format, "{to}");
        assert_eq!(
            (
                it["outputs"][0]["width"].as_u64(),
                it["outputs"][0]["height"].as_u64()
            ),
            (Some(64), Some(48))
        );
        let out = sb.work().join(format!("a.{ext}"));
        assert!(out.exists(), "{to}");
        assert_eq!(
            fs::read(&src).unwrap(),
            png,
            "{to}: the original is untouched"
        );
        let bytes = fs::read(&out).unwrap();
        if to == "pdf" {
            let pages = read_pages(&bytes).expect("a PDF this build reads back");
            assert_eq!(
                (pages.len(), pages[0].width_px, pages[0].codec),
                (1, 64, PageCodec::Dct)
            );
        } else {
            assert_eq!(
                decode(&bytes).unwrap().raster.data,
                pattern(64, 48),
                "{to}: lossless"
            );
        }
    }
}

#[test]
fn convert_to_tiff_replaces_after_a_backup_and_restore_brings_the_png_back() {
    let sb = Sandbox::new();
    let png = png_rgb(64, 48);
    let src = write(&sb.work(), "a.png", &png);
    let r = sb.run(&["convert", "--to", "tiff", "--json", s(&sb.work())]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    let doc = r.json();
    assert!(items(&doc)[0]["backup_id"].is_string());
    assert!(!src.exists() && sb.work().join("a.tif").exists());
    let run_id = doc["run"]["id"].as_str().unwrap().to_owned();
    let rr = sb.run(&["restore", "--run", &run_id, "--derived", "remove"]);
    assert_eq!(rr.code(), 0, "{}", rr.stderr());
    assert_eq!(fs::read(&src).unwrap(), png);
    assert!(!sb.work().join("a.tif").exists());
}

#[test]
fn unknown_targets_and_options_are_usage_errors_and_touch_nothing() {
    let sb = Sandbox::new();
    let p = write(&sb.work(), "a.png", &png_rgb(20, 20));
    let before = tree(&sb.work());
    for bad in [
        vec!["convert", "--to", "avif", s(&p)],
        vec!["convert", "--to", "jxl", s(&p)],
        vec![
            "convert",
            "--to",
            "tiff",
            "--tiff-compression",
            "jpeg",
            s(&p),
        ],
        vec!["process", "--format", "tiff", s(&p)], // a format change needs a copy mode
        vec!["process", "--format", "bmp", "--copy", s(&p)],
    ] {
        let r = sb.run(&bad);
        assert_eq!(r.code(), 2, "{bad:?}: {}", r.stderr());
    }
    assert_eq!(tree(&sb.work()), before);
}

#[test]
fn process_copies_in_every_format_and_keeps_a_tiff_or_webp_source_in_its_own() {
    let sb = Sandbox::new();
    good_photo(&sb.work().join("p.jpg"), 4);
    for (fmt, ext) in [("tiff", "tif"), ("webp", "webp"), ("pdf", "pdf")] {
        let out = sb.dir.path().join(format!("out-{fmt}"));
        let r = sb.run(&[
            "process",
            "--output",
            s(&out),
            "--format",
            fmt,
            "--json",
            s(&sb.work().join("p.jpg")),
        ]);
        assert_eq!(r.code(), 0, "{fmt}: {}", r.stderr());
        let doc = r.json();
        assert_valid_manifest(&doc);
        assert_eq!(items(&doc)[0]["outputs"][0]["format"], fmt);
        let bytes = fs::read(out.join(format!("p.{ext}"))).unwrap();
        if fmt == "pdf" {
            let pages = read_pages(&bytes).unwrap();
            assert_eq!(pages.len(), 1);
            // The crop, at the nominal 300 dpi: 72 pt per 300 px.
            let (w, h) = (pages[0].width_px, pages[0].height_px);
            let (mw, mh) = pages[0].media_box;
            assert!((mw - f64::from(w) * 72.0 / 300.0).abs() < 0.01);
            assert!((mh - f64::from(h) * 72.0 / 300.0).abs() < 0.01);
        } else {
            assert!(decode(&bytes).is_ok(), "{fmt}");
        }
    }
    // Sources that this build writes back as themselves keep their format in copies.
    for (name, fmt, ext) in [
        ("d.tif", Format::Tiff, "tif"),
        ("d.webp", Format::Webp, "webp"),
    ] {
        let sb = Sandbox::new();
        document_as(&sb.work().join(name), 5, fmt);
        let r = sb.run(&["process", "--copy", "--json", s(&sb.work())]);
        assert_eq!(r.code(), 0, "{name}: {}", r.stderr());
        let copy = sb.work().join(format!("AutoCrop/d.{ext}"));
        let d = decode(&fs::read(&copy).unwrap()).unwrap();
        assert_eq!(d.format, fmt, "{name}");
    }
}

#[test]
fn a_tiff_and_a_webp_original_are_replaced_in_place_with_a_backup_and_restore_returns_them() {
    for (name, fmt) in [("scan.tif", Format::Tiff), ("scan.webp", Format::Webp)] {
        let sb = Sandbox::new();
        let src = sb.work().join(name);
        let bytes = document_as(&src, 6, fmt);
        let r = sb.run(&["process", "--json", s(&sb.work())]);
        assert_eq!(r.code(), 0, "{name}: {}", r.stderr());
        let doc = r.json();
        assert_valid_manifest(&doc);
        let it = &items(&doc)[0];
        assert_eq!(it["status"], "saved", "{name}: {it}");
        assert!(it["backup_id"].is_string(), "{name}");
        let written = fs::read(&src).unwrap();
        assert_ne!(written, bytes);
        let d = decode(&written).unwrap();
        assert_eq!(d.format, fmt, "{name}: the same format under the same name");
        assert!(d.raster.width < 1200, "{name}: cropped");
        // Restore.
        let rr = sb.run(&["restore", s(&src)]);
        assert_eq!(rr.code(), 0, "{name}: {}", rr.stderr());
        assert_eq!(fs::read(&src).unwrap(), bytes, "{name}: restored");
    }
}

#[test]
fn tiff_compression_and_one_bit_reach_the_files() {
    let sb = Sandbox::new();
    good_photo(&sb.work().join("p.jpg"), 8);
    let size = |c: &str| {
        let out = sb.dir.path().join(format!("out-{c}"));
        let r = sb.run(&[
            "process",
            "--output",
            s(&out),
            "--format",
            "tiff",
            "--tiff-compression",
            c,
            s(&sb.work().join("p.jpg")),
        ]);
        assert_eq!(r.code(), 0, "{c}: {}", r.stderr());
        let f = out.join("p.tif");
        assert!(decode(&fs::read(&f).unwrap()).is_ok(), "{c}");
        fs::metadata(f).unwrap().len()
    };
    let (none, lzw, deflate) = (size("none"), size("lzw"), size("deflate"));
    assert!(
        lzw < none && deflate < none,
        "none {none} lzw {lzw} deflate {deflate}"
    );
    size("packbits");

    // 1-bit: a shaded receipt through the black-and-white mode, as Group 4 in a TIFF and in a PDF.
    let one = |tag: &str, fmt: &str, ext: &str, extra: &[&str]| {
        let out = sb.dir.path().join(format!("bit-{tag}"));
        let mut args = vec![
            "process",
            "--output",
            s(&out),
            "--format",
            fmt,
            "--enhance",
            "document",
            "--json",
        ];
        args.extend_from_slice(extra);
        let p = sb.work().join("p.jpg");
        args.push(s(&p));
        let r = sb.run(&args);
        (r.code(), r.stderr(), out.join(format!("p.{ext}")))
    };
    let (code, err, tif) = one("a", "tiff", "tif", &["--one-bit"]);
    assert_eq!(code, 0, "{err}");
    let pr = probe(&fs::read(&tif).unwrap()).unwrap();
    assert_eq!((pr.bit_depth, pr.channels), (1, 1), "a 1-bit TIFF");
    let (code, err, pdf) = one("b", "pdf", "pdf", &["--one-bit"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        read_pages(&fs::read(&pdf).unwrap()).unwrap()[0].codec,
        PageCodec::G4
    );
    // Without --one-bit the same page is 8-bit grey.
    let (code, _, grey) = one("c", "tiff", "tif", &[]);
    assert_eq!(code, 0);
    assert_eq!(probe(&fs::read(&grey).unwrap()).unwrap().bit_depth, 8);
    // Any other output is refused, and nothing is written.
    let (code, _, png) = one("d", "png", "png", &["--one-bit"]);
    assert_ne!(code, 0);
    assert!(!png.exists());
}
