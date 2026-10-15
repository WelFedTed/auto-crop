// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! AVIF output through the real binary (ROADMAP M11.22, M11.23, M11.53): `convert --to avif`,
//! `process --format avif`, `--avif-speed` and `--avif-chroma`, the manifest, the usage error of a
//! build without the writer, and restore of a converted original.

mod common;

use auto_crop_codecs::encoders::avif_box::read_info;
use auto_crop_codecs::fixtures::png_rgb;
use auto_crop_codecs::{Format, sniff};
use common::*;
use std::fs;
use std::path::{Path, PathBuf};

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    let p = dir.join(name);
    fs::write(&p, bytes).unwrap();
    p
}

#[cfg(not(feature = "avif"))]
#[test]
fn a_build_without_the_writer_refuses_avif_as_a_usage_error_and_touches_nothing() {
    let sb = Sandbox::new();
    let p = write(&sb.work(), "a.png", &png_rgb(20, 20));
    let before = tree(&sb.work());
    for bad in [
        vec!["convert", "--to", "avif", s(&p)],
        vec!["process", "--format", "avif", "--copy", s(&p)],
    ] {
        let r = sb.run(&bad);
        assert_eq!(r.code(), 2, "{bad:?}: {}", r.stderr());
        assert!(r.stderr().contains("avif"), "{}", r.stderr());
    }
    assert_eq!(tree(&sb.work()), before);
}

#[cfg(feature = "avif")]
mod with_writer {
    use super::*;

    #[test]
    fn convert_to_avif_writes_beside_the_original_and_says_so_in_the_manifest() {
        let sb = Sandbox::new();
        let png = png_rgb(64, 48);
        let src = write(&sb.work(), "a.png", &png);
        let r = sb.run(&[
            "convert",
            "--to",
            "avif",
            "--copy",
            "--colour",
            "preserve",
            "--avif-speed",
            "10",
            "--avif-chroma",
            "444",
            "--quality",
            "70",
            "--json",
            s(&sb.work()),
        ]);
        assert_eq!(r.code(), 0, "{}", r.stderr());
        let doc = r.json();
        assert_valid_manifest(&doc);
        let it = &doc["items"][0];
        assert_eq!(it["status"], "saved", "{it}");
        assert_eq!(it["outputs"][0]["format"], "avif");
        assert_eq!(it["convert"]["to"], "avif");
        assert!(
            it["reasons"]
                .as_array()
                .is_some_and(|a| a.iter().any(|n| n == "avif.lossy")),
            "{it}"
        );
        assert_eq!(doc["run"]["options"]["to"], "avif");
        assert_eq!(doc["run"]["options"]["avif_speed"], 10);
        assert_eq!(doc["run"]["options"]["avif_chroma"], "444");
        assert_eq!(fs::read(&src).unwrap(), png, "the original is untouched");
        let bytes = fs::read(sb.work().join("a.avif")).unwrap();
        assert_eq!(sniff(&bytes), Some(Format::Avif));
        let info = read_info(&bytes).unwrap();
        assert_eq!((info.width, info.height), (64, 48));
        assert!(!info.subsampled_x, "4:4:4 was asked for");
    }

    #[test]
    fn convert_to_avif_replaces_after_a_backup_and_restore_brings_the_original_back() {
        let sb = Sandbox::new();
        let png = png_rgb(64, 48);
        let src = write(&sb.work(), "a.png", &png);
        let r = sb.run(&[
            "convert",
            "--to",
            "avif",
            "--avif-speed",
            "10",
            "--json",
            s(&sb.work()),
        ]);
        assert_eq!(r.code(), 0, "{}", r.stderr());
        let doc = r.json();
        assert!(doc["items"][0]["backup_id"].is_string());
        assert!(!src.exists() && sb.work().join("a.avif").exists());
        let run_id = doc["run"]["id"].as_str().unwrap().to_owned();
        let rr = sb.run(&["restore", "--run", &run_id, "--derived", "remove"]);
        assert_eq!(rr.code(), 0, "{}", rr.stderr());
        assert_eq!(fs::read(&src).unwrap(), png);
        assert!(!sb.work().join("a.avif").exists());
    }

    #[test]
    fn process_writes_an_avif_copy_of_the_crop() {
        let sb = Sandbox::new();
        good_photo(&sb.work().join("p.jpg"), 4);
        let out = sb.dir.path().join("out-avif");
        let r = sb.run(&[
            "process",
            "--output",
            s(&out),
            "--format",
            "avif",
            "--avif-speed",
            "10",
            "--json",
            s(&sb.work().join("p.jpg")),
        ]);
        assert_eq!(r.code(), 0, "{}", r.stderr());
        let doc = r.json();
        assert_valid_manifest(&doc);
        assert_eq!(doc["items"][0]["outputs"][0]["format"], "avif");
        let bytes = fs::read(out.join("p.avif")).unwrap();
        let info = read_info(&bytes).unwrap();
        assert!(info.width > 100 && info.height > 100);
        assert!(!info.has_irot_or_imir);
    }

    #[test]
    fn bad_avif_options_are_usage_errors() {
        let sb = Sandbox::new();
        let p = write(&sb.work(), "a.png", &png_rgb(20, 20));
        for bad in [
            vec!["convert", "--to", "avif", "--avif-speed", "11", s(&p)],
            vec!["convert", "--to", "avif", "--avif-chroma", "422", s(&p)],
        ] {
            let r = sb.run(&bad);
            assert_eq!(r.code(), 2, "{bad:?}: {}", r.stderr());
        }
    }
}
