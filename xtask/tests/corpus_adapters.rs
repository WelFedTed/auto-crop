// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! ROADMAP M1.37 and M1.38: the corpus adapters, run through `cargo xtask corpus-ingest` over
//! SYNTHETIC trees that mimic each dataset's documented layout. The real datasets were never
//! downloaded, so every adapter is UNVERIFIED against its real layout (see the adapter docs);
//! these tests prove the machinery: manifests in the harness format that validate, clip-level
//! scenes, attribution carried, and the CC0 filter.

mod common;

use auto_crop_eval::manifest;
use common::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

struct Run {
    tmp: Tmp,
    src: PathBuf,
    lock: PathBuf,
}

impl Run {
    fn new(tag: &str, midv_spdx: &str, entries: &[(String, Vec<u8>)]) -> Self {
        let tmp = Tmp::new(tag);
        let src = tmp.join("data");
        write_tree(&src, entries);
        let lock = tmp.join("corpus.lock.toml");
        std::fs::write(&lock, ingest_lock(midv_spdx)).unwrap();
        Self { tmp, src, lock }
    }

    fn ingest(&self, adapter: &str, extra: &[&str]) -> std::process::Output {
        let mut args = vec![
            "corpus-ingest",
            adapter,
            "--src",
            self.src.to_str().unwrap(),
            "--lock",
            self.lock.to_str().unwrap(),
        ];
        args.extend_from_slice(extra);
        xtask(self.tmp.path(), &self.tmp.join("cache"), &args)
    }

    fn manifest(&self) -> manifest::Manifest {
        manifest::load(&self.src.join("manifest.jsonl")).expect("manifest validates")
    }

    fn lines(&self, file: &str) -> Vec<serde_json::Value> {
        std::fs::read_to_string(self.src.join(file))
            .unwrap_or_else(|e| panic!("{file}: {e}"))
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn report(&self) -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(self.src.join("ingest-report.json")).unwrap())
            .unwrap()
    }
}

/// Every scene lives in exactly one split.
fn scenes_are_disjoint(items: &[manifest::ManifestItem]) -> BTreeMap<String, String> {
    let mut m: BTreeMap<String, String> = BTreeMap::new();
    for it in items {
        let split = it.split.clone().expect("split");
        if let Some(prev) = m.insert(it.scene_id.clone(), split.clone()) {
            assert_eq!(prev, split, "scene {} spans two splits", it.scene_id);
        }
    }
    m
}

// ------------------------------------------------------------------------------- SmartDoc

#[test]
fn smartdoc_every_nth_frame_clip_level_scenes_and_attribution() {
    let (entries, clips) = smartdoc_entries();
    let r = Run::new("sd1", "CC-BY-4.0", &entries);
    let out = r.ingest("smartdoc2015-ch1", &[]);
    assert!(out.status.success(), "{}", text(&out));
    let m = r.manifest();
    assert_eq!(clips, 4);
    assert_eq!(m.items.len(), 4 * 3, "frames 0, 10 and 20 of each clip");

    // Clip-level scene ids: 4 scenes, 3 frames each, one split per scene.
    let scenes = scenes_are_disjoint(&m.items);
    assert_eq!(scenes.len(), 4);
    for scene in scenes.keys() {
        assert_eq!(
            m.items.iter().filter(|i| &i.scene_id == scene).count(),
            3,
            "{scene}"
        );
    }
    assert!(
        m.items
            .iter()
            .all(|i| i.scene_id.starts_with("smartdoc15-background0"))
    );
    let ids: BTreeSet<_> = m.items.iter().map(|i| i.id.as_str()).collect();
    assert!(ids.contains("smartdoc15-background01-datasheet001-000010"));

    // Quads are normalised by the frame size from the image header.
    let first = m
        .items
        .iter()
        .find(|i| i.id.ends_with("datasheet001-000000") && i.id.contains("background01"))
        .unwrap();
    assert_eq!((first.width, first.height), (FRAME_W, FRAME_H));
    assert!(
        (first.quad[0][0] - 8.0 / f64::from(FRAME_W)).abs() < 1e-9,
        "{:?}",
        first.quad
    );
    assert!((first.quad[2][1] - (42.0 / f64::from(FRAME_H))).abs() < 1e-9);
    assert_eq!(
        first.tags.get("background").map(String::as_str),
        Some("background01")
    );
    assert_eq!(
        first.tags.get("doctype").map(String::as_str),
        Some("datasheet")
    );
    assert_eq!(first.image, "frames/background01/datasheet001/000000.jpg");

    // CC BY attribution and licence travel in every manifest line.
    for line in r.lines("manifest.jsonl") {
        assert_eq!(line["licence"], "CC-BY-4.0");
        assert!(
            line["attribution"]
                .as_str()
                .unwrap()
                .contains("cite the fixture paper")
        );
        assert_eq!(line["source"], "smartdoc2015-ch1");
    }
    let info: serde_json::Value =
        serde_json::from_slice(&std::fs::read(r.src.join("corpus-info.json")).unwrap()).unwrap();
    assert_eq!(info["licence"], "CC-BY-4.0");

    // 12 <= 20 items: all of them are on the contact sheet, each with its quad outline.
    let sheet = std::fs::read_to_string(r.src.join("contact-sheet.html")).unwrap();
    assert_eq!(sheet.matches("<polygon").count(), 12);

    // Same input, same bytes.
    let before = std::fs::read(r.src.join("manifest.jsonl")).unwrap();
    assert!(r.ingest("smartdoc2015-ch1", &[]).status.success());
    assert_eq!(std::fs::read(r.src.join("manifest.jsonl")).unwrap(), before);
}

#[test]
fn smartdoc_bad_rows_are_counted_not_guessed() {
    let (entries, _) = smartdoc_entries();
    let r = Run::new("sd2", "CC-BY-4.0", &entries);
    let out = r.ingest(
        "smartdoc2015-ch1",
        &["--every", "1", "--contact-sheet", "5"],
    );
    assert!(out.status.success(), "{}", text(&out));
    let m = r.manifest();
    assert_eq!(
        m.items.len(),
        100,
        "25 frames x 4 clips, nothing from the 3 bad rows"
    );
    let rep = r.report();
    assert_eq!(rep["skipped"]["frame-image-not-found"], 1);
    assert_eq!(rep["skipped"]["invalid-quad"], 1, "{rep}");
    assert_eq!(rep["skipped"]["no-document-in-frame"], 1);
    let sheet = std::fs::read_to_string(r.src.join("contact-sheet.html")).unwrap();
    assert_eq!(sheet.matches("<polygon").count(), 5);
}

#[test]
fn smartdoc_scene_by_document_groups_clips_across_backgrounds() {
    let (entries, _) = smartdoc_entries();
    let r = Run::new("sd3", "CC-BY-4.0", &entries);
    assert!(
        r.ingest(
            "smartdoc2015-ch1",
            &["--scene-by", "document", "--dev-percent", "50"]
        )
        .status
        .success()
    );
    let m = r.manifest();
    let scenes = scenes_are_disjoint(&m.items);
    assert_eq!(
        scenes.keys().cloned().collect::<Vec<_>>(),
        vec!["smartdoc15-datasheet001", "smartdoc15-letter001"]
    );
    // Both backgrounds of one model are in one split.
    assert_eq!(
        m.items
            .iter()
            .filter(|i| i.scene_id == "smartdoc15-letter001")
            .map(|i| i.split.clone())
            .collect::<BTreeSet<_>>()
            .len(),
        1
    );
}

#[test]
fn smartdoc_reads_a_gzipped_ground_truth_table() {
    let (mut entries, _) = smartdoc_entries();
    let csv = entries
        .iter()
        .position(|(n, _)| n == "metadata.csv")
        .unwrap();
    let (_, body) = entries.remove(csv);
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    std::io::Write::write_all(&mut gz, &body).unwrap();
    entries.push(("metadata.csv.gz".to_owned(), gz.finish().unwrap()));
    let r = Run::new("sd4", "CC-BY-4.0", &entries);
    let out = r.ingest("smartdoc2015-ch1", &[]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(r.manifest().items.len(), 12);
}

#[test]
fn a_tree_that_does_not_match_the_assumed_layout_writes_nothing() {
    // No metadata table.
    let (entries, _) = smartdoc_entries();
    let no_meta: Vec<_> = entries
        .into_iter()
        .filter(|(n, _)| n != "metadata.csv")
        .collect();
    let r = Run::new("sd5", "CC-BY-4.0", &no_meta);
    let out = r.ingest("smartdoc2015-ch1", &[]);
    assert!(!out.status.success());
    assert!(text(&out).contains("no metadata.csv"), "{}", text(&out));
    assert!(!r.src.join("manifest.jsonl").exists());

    // A table with other column names: the error names what is missing and what is there.
    let r = Run::new(
        "sd6",
        "CC-BY-4.0",
        &[("metadata.csv".to_owned(), b"clip,frame,x\n1,2,3\n".to_vec())],
    );
    let out = r.ingest("smartdoc2015-ch1", &[]);
    assert!(!out.status.success());
    let log = text(&out);
    assert!(
        log.contains("missing column") && log.contains("bg_name") && log.contains("clip"),
        "{log}"
    );

    // A good table but no frames at all.
    let (entries, _) = smartdoc_entries();
    let only_csv: Vec<_> = entries
        .into_iter()
        .filter(|(n, _)| n == "metadata.csv")
        .collect();
    let r = Run::new("sd7", "CC-BY-4.0", &only_csv);
    let out = r.ingest("smartdoc2015-ch1", &[]);
    assert!(!out.status.success());
    assert!(text(&out).contains("no usable records"), "{}", text(&out));
    assert!(!r.src.join("manifest.jsonl").exists());
}

#[test]
fn exif_rotated_frames_are_left_out_because_their_pixel_quads_would_be_wrong() {
    let (mut entries, _) = smartdoc_entries();
    // Replace one selected frame with a JPEG that carries EXIF orientation 6.
    let rotated = auto_crop_codecs::fixtures::jpeg_with_exif_blob(
        &jpeg(FRAME_W, FRAME_H),
        &auto_crop_codecs::fixtures::exif_blob(6, true),
    );
    for e in &mut entries {
        if e.0 == "frames/background01/datasheet001/000010.jpg" {
            e.1 = rotated.clone();
        }
    }
    let r = Run::new("sd8", "CC-BY-4.0", &entries);
    assert!(r.ingest("smartdoc2015-ch1", &[]).status.success());
    assert_eq!(r.manifest().items.len(), 11);
    assert_eq!(r.report()["skipped"]["exif-orientation-not-1"], 1);
}

// ---------------------------------------------------------------------------------- CORD

#[test]
fn cord_quads_come_from_the_roi_and_transcripts_are_written() {
    let r = Run::new("cord", "CC-BY-4.0", &cord_entries());
    let out = r.ingest("cord", &[]);
    assert!(out.status.success(), "{}", text(&out));
    let m = r.manifest();
    assert_eq!(
        m.items.len(),
        5,
        "3 train + 2 test with an outline; no-roi and wrong-size ones left out"
    );
    scenes_are_disjoint(&m.items);
    let it = m
        .items
        .iter()
        .find(|i| i.id == "cord-train-receipt_00000")
        .unwrap();
    // The ROI was given counter-clockwise; the adapter returns clockwise from the top-left.
    assert!(
        (it.quad[0][0] - 6.0 / 60.0).abs() < 1e-9 && (it.quad[0][1] - 8.0 / 90.0).abs() < 1e-9,
        "{:?}",
        it.quad
    );
    assert!(
        (it.quad[2][0] - 52.0 / 60.0).abs() < 1e-9 && (it.quad[2][1] - 80.0 / 90.0).abs() < 1e-9
    );
    assert_eq!(it.tags["cord_split"], "train");
    assert_eq!(it.tags["aspect"], "document");
    assert_eq!(it.scene_id, it.id, "one scene per receipt");
    for line in r.lines("manifest.jsonl") {
        assert_eq!(line["licence"], "CC-BY-4.0");
        assert!(line["attribution"].as_str().unwrap().contains("fixture"));
    }
    // Transcripts exist for every receipt that has text, including the one without an outline.
    let t = r.lines("transcripts.jsonl");
    let by_id: BTreeMap<_, _> = t
        .iter()
        .map(|v| {
            (
                v["id"].as_str().unwrap().to_owned(),
                v["text"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(by_id["cord-train-receipt_00000"], "NASI GORENG\n15.000");
    assert_eq!(by_id["cord-train-receipt_00009"], "TOTAL");
    assert!(t.iter().all(|v| v["licence"] == "CC-BY-4.0"));
    let rep = r.report();
    assert_eq!(rep["skipped"]["no-roi"], 1);
    assert_eq!(rep["skipped"]["image-size-differs-from-meta"], 1);
    assert_eq!(rep["skipped"]["image-not-found"], 1);
}

#[test]
fn cord_without_any_outline_is_an_error_not_an_empty_manifest() {
    let entries: Vec<_> = cord_entries()
        .into_iter()
        .map(|(n, b)| {
            if n.ends_with(".json") {
                let s = String::from_utf8(b).unwrap();
                let v: serde_json::Value = serde_json::from_str(&s).unwrap();
                let mut o = v.as_object().unwrap().clone();
                o.remove("roi");
                (n, serde_json::to_vec(&o).unwrap())
            } else {
                (n, b)
            }
        })
        .collect();
    let r = Run::new("cord2", "CC-BY-4.0", &entries);
    let out = r.ingest("cord", &[]);
    assert!(!out.status.success());
    assert!(text(&out).contains("no usable records"), "{}", text(&out));
    assert!(!r.src.join("manifest.jsonl").exists());
}

// ---------------------------------------------------------------------------------- MIDV

#[test]
fn midv_quads_by_document_scene_and_counted_oddities() {
    let r = Run::new("midv", "CC-BY-4.0", &midv_entries());
    let out = r.ingest("midv-500", &["--every", "1", "--contact-sheet", "0"]);
    assert!(out.status.success(), "{}", text(&out));
    let m = r.manifest();
    assert_eq!(
        m.items.len(),
        96,
        "2 docs x 2 conditions x 2 clips x 12 frames"
    );
    let scenes = scenes_are_disjoint(&m.items);
    assert_eq!(
        scenes.keys().cloned().collect::<Vec<_>>(),
        vec!["midv500-01_alb_id", "midv500-02_aut_drvlic_new"]
    );
    let it = m
        .items
        .iter()
        .find(|i| i.id == "midv500-01_alb_id-TS01_01")
        .unwrap();
    assert_eq!(it.image, "01_alb_id/images/TS/TS01/TS01_01.tif");
    assert_eq!(it.tags["condition"], "TS");
    assert_eq!(it.tags["format"], "tiff");
    assert!(
        (it.quad[0][0] - 6.2 / f64::from(FRAME_W)).abs() < 1e-9,
        "{:?}",
        it.quad
    );
    let rep = r.report();
    assert_eq!(
        rep["skipped"]["invalid-quad"], 1,
        "the counter-clockwise quad is not reordered"
    );
    assert_eq!(rep["skipped"]["no-quad"], 1);
    assert_eq!(rep["skipped"]["frame-image-not-found"], 1);
    assert!(!r.src.join("contact-sheet.html").exists());

    // Per-clip scenes on request, every Nth frame by default behaviour.
    assert!(
        r.ingest(
            "midv-500",
            &["--every", "4", "--scene-by", "clip", "--contact-sheet", "0"]
        )
        .status
        .success()
    );
    let m = r.manifest();
    assert_eq!(scenes_are_disjoint(&m.items).len(), 8);
    assert_eq!(
        m.items.len(),
        24,
        "3 of every clip; the bad frame of 01_alb_id/TS01 is not among them"
    );
}

#[test]
fn midv_is_refused_until_its_licence_is_cleared() {
    let r = Run::new("midv2", "NOASSERTION", &midv_entries());
    let out = r.ingest("midv-500", &["--every", "1"]);
    assert!(!out.status.success());
    let log = text(&out);
    assert!(
        log.contains("REFUSED") && log.contains("not cleared"),
        "{log}"
    );
    assert!(!r.src.join("manifest.jsonl").exists());
}

// ---------------------------------------------------------------------------------- DIBCO

#[test]
fn dibco_pairs_images_with_ground_truth_and_counts_the_rest() {
    let r = Run::new("dibco", "CC-BY-4.0", &dibco_entries());
    let out = r.ingest("dibco", &[]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        !r.src.join("manifest.jsonl").exists(),
        "DIBCO has no quads, so no quad manifest"
    );
    let pairs = r.lines("binarisation.jsonl");
    let ids: Vec<_> = pairs.iter().map(|p| p["id"].as_str().unwrap()).collect();
    assert_eq!(
        ids,
        vec![
            "dibco-2015-x",
            "dibco-2016-1",
            "dibco-2016-2",
            "dibco-2017-hw1"
        ]
    );
    let p = pairs.iter().find(|p| p["id"] == "dibco-2016-1").unwrap();
    assert_eq!(p["image"], "2016/img/1.png");
    assert_eq!(p["gt"], "2016/gt/1_GT.png");
    assert_eq!(p["year"], "2016");
    assert_eq!(p["licence"], "CC0-1.0");
    assert_eq!(p["scene_id"], p["id"]);
    let rep = r.report();
    assert_eq!(rep["skipped"]["ground-truth-without-original"], 1);
    assert_eq!(rep["skipped"]["original-without-ground-truth"], 1);
    assert_eq!(rep["skipped"]["image-and-ground-truth-sizes-differ"], 1);
    assert_eq!(
        rep["skipped"]["size-not-checked"], 1,
        "the BMP pair cannot be probed"
    );
}

// ------------------------------------------------------------------------------ raw.pixls.us

fn pixls_run(tag: &str, index: &str) -> Run {
    Run::new(
        tag,
        "CC-BY-4.0",
        &[("index.jsonl".to_owned(), index.as_bytes().to_vec())],
    )
}

#[test]
fn rawpixls_mixed_licence_fixture_only_cc0_passes() {
    let r = pixls_run("pixls", &pixls_index_lines().join("\n"));
    let out = r.ingest("rawpixls-cc0", &[]);
    assert!(out.status.success(), "{}", text(&out));
    let kept = r.lines("cc0-samples.jsonl");
    let paths: Vec<_> = kept.iter().map(|k| k["path"].as_str().unwrap()).collect();
    assert_eq!(
        paths,
        vec![
            "Canon/EOS_5D/cc0-a.CR2",
            "Nikon/D70/cc0-b.NEF",
            "Sony/A7/cc0-c.ARW"
        ]
    );
    for k in &kept {
        assert_eq!(k["licence"], "CC0-1.0");
        assert_eq!(k["sha256"].as_str().unwrap().len(), 64);
    }
    let rep = r.report();
    let skipped = rep["skipped"].as_object().unwrap();
    assert_eq!(skipped["not-cc0: CC-BY-SA-4.0"], 1);
    assert_eq!(skipped["not-cc0: Public Domain"], 1);
    assert_eq!(
        skipped["not-cc0: missing"], 2,
        "a missing and an empty licence both fail closed"
    );
    assert_eq!(skipped["not-cc0: CC0-1.0 OR CC-BY-SA-4.0"], 1);
    assert_eq!(skipped["not-cc0: CC-BY-NC-4.0"], 1);
    assert_eq!(skipped["cc0-but-no-pinned-sha256"], 1);
    assert_eq!(
        skipped["unsafe-path"], 2,
        "traversal and absolute paths never get through"
    );
    // No non-CC0 path appears anywhere in the output.
    let all = std::fs::read_to_string(r.src.join("cc0-samples.jsonl")).unwrap();
    assert!(
        !all.contains("not-cc0") && !all.contains("rawsamples") && !all.contains("escape"),
        "{all}"
    );
}

#[test]
fn rawpixls_accepts_a_json_array_and_fails_closed_when_nothing_is_cc0() {
    let array = format!("[{}]", pixls_index_lines().join(","));
    let r = pixls_run("pixls2", &array);
    assert!(r.ingest("rawpixls-cc0", &[]).status.success());
    assert_eq!(r.lines("cc0-samples.jsonl").len(), 3);

    let none: Vec<String> = pixls_index_lines()
        .into_iter()
        .filter(|l| l.contains("not-cc0"))
        .collect();
    let r = pixls_run("pixls3", &none.join("\n"));
    let out = r.ingest("rawpixls-cc0", &[]);
    assert!(!out.status.success());
    assert!(text(&out).contains("no CC0 sample"), "{}", text(&out));
    assert!(!r.src.join("cc0-samples.jsonl").exists());
}

// ------------------------------------------------------------------------------ plumbing

#[test]
fn ingest_input_errors_are_reported() {
    let (entries, _) = smartdoc_entries();
    let r = Run::new("plumb", "CC-BY-4.0", &entries);
    let out = r.ingest("nope", &[]);
    assert!(text(&out).contains("unknown corpus"), "{}", text(&out));
    let out = r.ingest("smartdoc2015-ch1", &["--every", "0"]);
    assert!(!out.status.success());
    assert!(
        text(&out).contains("--every must be at least 1"),
        "{}",
        text(&out)
    );
    let out = xtask(
        r.tmp.path(),
        &r.tmp.join("c"),
        &["corpus-ingest", "cord", "--lock", r.lock.to_str().unwrap()],
    );
    assert!(text(&out).contains("needs --src"), "{}", text(&out));
}
