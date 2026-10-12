// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `auto-crop convert` end to end through the real binary, on synthetic PNG, BMP, TIFF and WebP
//! files, each test in its own sandbox. HEIC and AVIF need the `heif` feature and are checked by
//! `heif.yml` in CI (the same binary, built with the feature).

mod common;

use auto_crop_codecs::fixtures::{
    JpegSpec, TiffOpts, bmp_rgb24, gif_stub, heic_stub, pattern, png_rgb, tiff_rgb8, webp_animated,
    webp_lossless,
};
use auto_crop_codecs::{decode, probe};
use common::*;
use serde_json::Value;
use std::fs;
use std::path::Path;

fn items(doc: &Value) -> &Vec<Value> {
    doc["items"].as_array().expect("items")
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
    fs::create_dir_all(dir).unwrap();
    let p = dir.join(name);
    fs::write(&p, bytes).unwrap();
    p
}

/// PNG, BMP, TIFF and WebP of 64 x 48.
fn four_sources(dir: &Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    let rgb = pattern(64, 48);
    [
        ("a.png", png_rgb(64, 48)),
        ("b.bmp", bmp_rgb24(64, 48, &rgb, false)),
        ("c.tif", tiff_rgb8(64, 48, &TiffOpts::default())),
        ("d.webp", webp_lossless(64, 48)),
    ]
    .into_iter()
    .map(|(n, b)| (write(dir, n, &b), b))
    .collect()
}

fn dims(p: &Path) -> (u32, u32) {
    let pr = probe(&fs::read(p).unwrap()).unwrap();
    (pr.width, pr.height)
}

// ---------------------------------------------------------------- usage

#[test]
fn usage_errors_exit_2_and_touch_nothing() {
    let sb = Sandbox::new();
    let p = write(&sb.work(), "a.png", &png_rgb(20, 20));
    let before = tree(&sb.work());
    for bad in [
        vec!["convert", s(&p)],
        vec!["convert", "--to", "gif", s(&p)],
        vec!["convert", "--to", "jpg"],
        vec!["convert", "--to", "jpg", "--colour", "vivid", s(&p)],
        vec!["convert", "--to", "jpg", "--copy", "--in-place", s(&p)],
        vec!["convert", "--to", "jpg", "--copy", "--output", "x", s(&p)],
        vec!["convert", "--to", "jpg", "--quality", "0", s(&p)],
        vec!["convert", "--to", "jpg", "--nope", s(&p)],
    ] {
        let r = sb.run(&bad);
        assert_eq!(r.code(), 2, "{bad:?}: {}", r.stderr());
        assert!(r.stderr().contains("error:"), "{bad:?}");
    }
    assert_eq!(tree(&sb.work()), before);
    assert!(!sb.home().exists());
    // Help lists the command and its options.
    let h = sb.run(&["convert", "--help"]);
    assert_eq!(h.code(), 0);
    for needle in ["--to", "--colour", "--reencode", "--copy", "--dry-run"] {
        assert!(h.stdout().contains(needle), "{needle}");
    }
}

// ---------------------------------------------------------------- the default: replace with a backup

#[test]
fn the_default_replaces_each_original_after_a_backup_and_restore_brings_it_back() {
    let sb = Sandbox::new();
    let src = four_sources(&sb.work());
    let r = sb.run(&["convert", "--to", "jpg", "--json", s(&sb.work())]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    let doc = r.json();
    assert_valid_manifest(&doc);
    assert_eq!(doc["run"]["command"], "convert");
    assert_eq!(doc["run"]["options"]["mode"], "in_place");
    assert_eq!(doc["run"]["options"]["to"], "jpg");
    assert_eq!(doc["summary"]["saved"], 4);
    assert_eq!(doc["summary"]["files_written"], 4);
    for it in items(&doc) {
        assert_eq!(it["status"], "saved", "{it}");
        assert_eq!(it["written"], true);
        assert!(it["backup_id"].is_string(), "{it}");
        assert_eq!(it["outputs"][0]["format"], "jpeg");
        assert_eq!(
            (
                it["outputs"][0]["width"].as_u64(),
                it["outputs"][0]["height"].as_u64()
            ),
            (Some(64), Some(48))
        );
        assert_eq!(it["convert"]["to"], "jpeg");
        assert!(it["convert"]["bytes_out"].as_u64().unwrap() > 0);
    }
    for (p, _) in &src {
        assert!(!p.exists(), "{} was replaced", p.display());
        let out = p.with_extension("jpg");
        assert_eq!(dims(&out), (64, 48));
        assert!(decode(&fs::read(&out).unwrap()).is_ok());
    }
    assert!(
        r.stderr().contains("replaces your originals"),
        "{}",
        r.stderr()
    );
    assert!(
        r.stdout().trim_start().starts_with('{'),
        "stdout is the JSON only"
    );

    // Restore the whole run: every original comes back byte for byte.
    let run_id = doc["run"]["id"].as_str().unwrap().to_owned();
    let rr = sb.run(&["restore", "--run", &run_id, "--derived", "remove"]);
    assert_eq!(rr.code(), 0, "{}", rr.stderr());
    for (p, bytes) in &src {
        assert_eq!(&fs::read(p).unwrap(), bytes, "{}", p.display());
        assert!(
            !p.with_extension("jpg").exists(),
            "the converted file moved into the store"
        );
    }
}

#[test]
fn restoring_one_converted_file_by_its_path_keeps_or_removes_the_converted_file() {
    let sb = Sandbox::new();
    let p = write(&sb.work(), "a.png", &png_rgb(30, 20));
    let bytes = fs::read(&p).unwrap();
    assert_eq!(sb.run(&["convert", "--to", "jpg", s(&sb.work())]).code(), 0);
    let jpg = sb.work().join("a.jpg");
    assert!(jpg.exists() && !p.exists());
    let r = sb.run(&["restore", s(&jpg)]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    assert_eq!(fs::read(&p).unwrap(), bytes);
    assert!(
        jpg.exists(),
        "keep is the default: the converted file stays"
    );
}

#[test]
fn copy_keeps_the_originals_and_output_mirrors_the_tree() {
    let sb = Sandbox::new();
    let src = four_sources(&sb.work());
    write(
        &sb.work().join("sub").join("deep"),
        "e.png",
        &png_rgb(20, 20),
    );
    let before = tree(&sb.work());

    let r = sb.run(&[
        "convert",
        "--to",
        "png",
        "--copy",
        "-r",
        "--json",
        s(&sb.work()),
    ]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    let doc = r.json();
    assert_valid_manifest(&doc);
    assert_eq!(doc["run"]["options"]["mode"], "copy");
    // a.png is already a PNG: skipped. The other four (b, c, d and e? e is PNG too) convert.
    let saved = doc["summary"]["saved"].as_u64().unwrap();
    assert_eq!(saved, 3, "{doc}");
    assert_eq!(doc["summary"]["skipped"], 2);
    for it in items(&doc).iter().filter(|i| i["status"] == "skipped") {
        assert_eq!(it["code"], "SAME_FORMAT");
    }
    for (p, bytes) in &src {
        assert_eq!(&fs::read(p).unwrap(), bytes, "originals untouched");
    }
    // Nothing was backed up: a copy changes nothing that exists.
    assert!(items(&doc).iter().all(|i| i["backup_id"].is_null()));
    assert!(sb.work().join("b.png").exists() && sb.work().join("c.png").exists());
    assert!(
        !sb.home().join("backups").exists()
            || fs::read_dir(sb.home().join("backups")).unwrap().count() == 0
    );
    // The files that existed before are still there with the same bytes.
    for (name, h) in &before {
        let p = sb.work().join(name);
        assert_eq!(&hash(&p), h, "{name}");
    }

    // --output DIR mirrors the folder tree and keeps the originals.
    let sb2 = Sandbox::new();
    four_sources(&sb2.work());
    write(
        &sb2.work().join("sub").join("deep"),
        "e.bmp",
        &bmp_rgb24(10, 10, &pattern(10, 10), false),
    );
    let out = sb2.dir.path().join("out");
    let r = sb2.run(&[
        "convert",
        "--to",
        "jpg",
        "-r",
        "-o",
        s(&out),
        s(&sb2.work()),
        "--json",
    ]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    assert_valid_manifest(&r.json());
    assert_eq!(r.json()["run"]["options"]["mode"], "output");
    for n in ["a.jpg", "b.jpg", "c.jpg", "d.jpg", "sub/deep/e.jpg"] {
        assert!(out.join(n).exists(), "{n}");
    }
    assert!(sb2.work().join("a.png").exists() && sb2.work().join("sub/deep/e.bmp").exists());
}

#[test]
fn a_dry_run_prints_the_plan_and_writes_nothing_at_all() {
    let sb = Sandbox::new();
    four_sources(&sb.work());
    write(&sb.work(), "ok.jpg", &JpegSpec::new(32, 32).build());
    let before = tree(&sb.work());
    let r = sb.run(&[
        "convert",
        "--to",
        "jpg",
        "--dry-run",
        "--json",
        s(&sb.work()),
    ]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    let doc = r.json();
    assert_valid_manifest(&doc);
    assert_eq!(doc["run"]["dry_run"], true);
    assert_eq!(doc["summary"]["saved"], 4);
    assert_eq!(doc["summary"]["skipped"], 1);
    for it in items(&doc).iter().filter(|i| i["status"] == "saved") {
        assert_eq!(it["written"], false);
        assert!(it["outputs"][0]["path"].as_str().unwrap().ends_with(".jpg"));
        assert!(it["backup_id"].is_null());
    }
    assert_eq!(tree(&sb.work()), before, "not a file changed");
    assert!(
        !sb.home().exists(),
        "no store, journal or notice record was created"
    );
    assert!(!r.stderr().contains("replaces your originals"));
}

// ---------------------------------------------------------------- idempotency, skips and failures

#[test]
fn a_file_in_the_target_format_is_skipped_and_reencode_converts_it() {
    let sb = Sandbox::new();
    let p = write(&sb.work(), "a.jpg", &JpegSpec::new(64, 48).build());
    let before = fs::read(&p).unwrap();
    let r = sb.run(&["convert", "--to", "jpg", "--json", s(&p)]);
    assert_eq!(
        r.code(),
        0,
        "skipping by choice is not a failure: {}",
        r.stderr()
    );
    let doc = r.json();
    assert_eq!(items(&doc)[0]["code"], "SAME_FORMAT");
    assert_eq!(items(&doc)[0]["status"], "skipped");
    assert_eq!(fs::read(&p).unwrap(), before);

    let r = sb.run(&[
        "convert",
        "--to",
        "jpg",
        "--reencode",
        "--quality",
        "20",
        "--json",
        s(&p),
    ]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    let doc = r.json();
    assert_eq!(items(&doc)[0]["status"], "saved");
    assert_eq!(
        items(&doc)[0]["outputs"][0]["path"]
            .as_str()
            .map(Path::new)
            .map(|x| x.file_name()),
        Some(p.file_name())
    );
    assert!(
        fs::read(&p).unwrap().len() < before.len(),
        "quality 20 is smaller"
    );
    assert!(items(&doc)[0]["backup_id"].is_string());
    // And again with --run restores the original.
    let rr = sb.run(&["restore", s(&p)]);
    assert_eq!(rr.code(), 0, "{}", rr.stderr());
    assert_eq!(fs::read(&p).unwrap(), before);
}

#[test]
fn several_frames_are_never_replaced_and_a_copy_converts_the_first() {
    let sb = Sandbox::new();
    let a = write(&sb.work(), "anim.webp", &webp_animated(24, 16, 3));
    let t = write(
        &sb.work(),
        "pages.tif",
        &tiff_rgb8(
            24,
            16,
            &TiffOpts {
                extra_pages: 1,
                ..TiffOpts::default()
            },
        ),
    );
    let before = (fs::read(&a).unwrap(), fs::read(&t).unwrap());
    let r = sb.run(&["convert", "--to", "png", "--json", s(&sb.work())]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    let doc = r.json();
    assert_eq!(doc["summary"]["skipped"], 2);
    for it in items(&doc) {
        assert_eq!(it["code"], "NOT_REPLACEABLE");
        assert!(!it["reasons"].as_array().unwrap().is_empty());
    }
    assert_eq!((fs::read(&a).unwrap(), fs::read(&t).unwrap()), before);
    let r = sb.run(&["convert", "--to", "png", "--copy", "--json", s(&sb.work())]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    let doc = r.json();
    assert_eq!(doc["summary"]["saved"], 2);
    assert!(
        items(&doc)
            .iter()
            .any(|i| i["reasons"].to_string().contains("tiff.multi_page"))
    );
}

#[test]
fn heic_without_the_decoder_is_skipped_with_a_clear_code_and_message() {
    if cfg!(feature = "heif") {
        return; // the HEIC build is checked in CI with real files
    }
    let sb = Sandbox::new();
    let p = write(&sb.work(), "IMG_0001.HEIC", &heic_stub());
    let r = sb.run(&["convert", "--to", "jpg", "--json", s(&sb.work())]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    let doc = r.json();
    assert_eq!(items(&doc)[0]["code"], "HEIF_UNAVAILABLE");
    assert_eq!(items(&doc)[0]["status"], "skipped");
    assert!(r.stderr().contains("HEIF_UNAVAILABLE"), "{}", r.stderr());
    assert_eq!(fs::read(&p).unwrap(), heic_stub());
    assert!(
        !sb.home().join("backups").exists()
            || fs::read_dir(sb.home().join("backups")).unwrap().count() == 0
    );
    // doctor says why.
    let d = sb.run(&["doctor", "--json"]);
    assert_eq!(d.json()["heic_conversion"], false);
    assert!(sb.run(&["doctor"]).stdout().contains("NOT available"));
}

#[test]
fn unreadable_and_unsupported_files_are_reported_and_the_exit_codes_follow_process() {
    let sb = Sandbox::new();
    let mut bad = png_rgb(40, 40);
    bad.truncate(bad.len() / 2);
    let corrupt = write(&sb.work(), "bad.png", &bad);
    write(&sb.work(), "g.gif", &gif_stub());
    write(
        &sb.work(),
        "ok.bmp",
        &bmp_rgb24(8, 8, &pattern(8, 8), false),
    );
    let r = sb.run(&["convert", "--to", "jpg", "--json", s(&sb.work())]);
    assert_eq!(r.code(), 3, "a corrupt file fails the run: {}", r.stderr());
    let doc = r.json();
    assert_valid_manifest(&doc);
    assert_eq!(doc["summary"]["failed"], 1);
    assert_eq!(doc["summary"]["saved"], 1);
    assert_eq!(
        fs::read(&corrupt).unwrap(),
        bad,
        "the corrupt original is untouched"
    );
    // Nothing to convert at all: exit 5.
    let empty = sb.dir.path().join("empty");
    fs::create_dir_all(&empty).unwrap();
    assert_eq!(sb.run(&["convert", "--to", "jpg", s(&empty)]).code(), 5);
    // A missing path fails like in process.
    let r = sb.run(&["convert", "--to", "jpg", "--json", "no-such-file.png"]);
    assert_eq!(r.code(), 3);
    assert_eq!(items(&r.json())[0]["code"], "NOT_FOUND");
}

#[test]
fn ndjson_streams_start_items_and_end_and_the_manifest_file_validates() {
    let sb = Sandbox::new();
    four_sources(&sb.work());
    let m = sb.dir.path().join("m").join("manifest.json");
    let r = sb.run(&[
        "convert",
        "--to",
        "jpg",
        "--copy",
        "--ndjson",
        "--manifest",
        s(&m),
        s(&sb.work()),
    ]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    let lines: Vec<Value> = r
        .stdout()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.first().unwrap()["t"], "start");
    assert_eq!(lines.last().unwrap()["t"], "end");
    assert_eq!(lines.iter().filter(|l| l["t"] == "item").count(), 4);
    let doc: Value = serde_json::from_str(&fs::read_to_string(&m).unwrap()).unwrap();
    assert_valid_manifest(&doc);
    assert_eq!(doc["run"]["command"], "convert");
}

#[test]
fn quality_and_colour_flags_reach_the_files() {
    let sb = Sandbox::new();
    let rgb = pattern(160, 120);
    let src = write(&sb.work(), "big.bmp", &bmp_rgb24(160, 120, &rgb, false));
    let low = sb.dir.path().join("low");
    let high = sb.dir.path().join("high");
    for (q, d) in [("10", &low), ("98", &high)] {
        let r = sb.run(&[
            "convert",
            "--to",
            "jpg",
            "--quality",
            q,
            "-o",
            s(d),
            s(&src),
        ]);
        assert_eq!(r.code(), 0, "{}", r.stderr());
    }
    let (l, h) = (
        fs::metadata(low.join("big.jpg")).unwrap().len(),
        fs::metadata(high.join("big.jpg")).unwrap().len(),
    );
    assert!(l * 2 < h, "q10 is much smaller than q98: {l} vs {h}");
    let r = sb.run(&[
        "convert",
        "--to",
        "png",
        "--colour",
        "preserve",
        "--copy",
        "--json",
        s(&src),
    ]);
    assert_eq!(r.code(), 0);
    assert_eq!(r.json()["run"]["options"]["colour"], "preserve");
    assert_valid_manifest(&r.json());
}
