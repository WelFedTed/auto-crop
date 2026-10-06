// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `restore`, `backups`, `analyze`, `render`, `doctor` and the help text, through the real binary.

mod common;

use auto_crop_codecs::probe;
use common::*;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

fn items(doc: &Value) -> &Vec<Value> {
    doc["items"].as_array().expect("items")
}

fn size_of(p: &Path) -> (u32, u32) {
    let pr = probe(&fs::read(p).unwrap()).unwrap();
    (pr.width, pr.height)
}

/// The backup folder of the one backup in a sandbox's store.
fn only_backup_dir(sb: &Sandbox) -> PathBuf {
    let root = sb.home().join("data/backups");
    let dirs: Vec<_> = fs::read_dir(&root)
        .unwrap()
        .flatten()
        .filter(|e| e.path().join("manifest.json").is_file())
        .map(|e| e.path())
        .collect();
    assert_eq!(dirs.len(), 1, "{dirs:?}");
    dirs[0].clone()
}

// ---------------------------------------------------------------- restore

#[test]
fn restore_dry_run_changes_nothing_and_a_modified_file_is_never_overwritten_by_default() {
    let sb = Sandbox::new();
    let a = good_photo(&sb.work().join("a.jpg"), 1);
    let original = fs::read(&a).unwrap();
    assert_eq!(sb.run(&["process", "--quiet", s(&a)]).code(), 0);
    let processed = hash(&a);
    let store_before = tree(&sb.home());

    // A dry run says what would happen and writes nothing (not even a purge).
    let dry = sb.run(&["restore", "--dry-run", "--json", s(&a)]);
    assert_eq!(dry.code(), 0, "{}", dry.stderr());
    assert_eq!(items(&dry.json())[0]["status"], "would_restore");
    assert_eq!(items(&dry.json())[0]["current"], "as_saved");
    assert_eq!(
        (hash(&a), tree(&sb.home())),
        (processed.clone(), store_before.clone())
    );

    // The user edited the processed file afterwards.
    write_jpeg(&a, &doc_scene(7, GOOD), 80);
    let edited = hash(&a);
    let r = sb.run(&["restore", "--json", s(&a)]);
    assert_eq!(r.code(), 3, "{}", r.stderr());
    assert_eq!(items(&r.json())[0]["code"], "MODIFIED_SINCE_SAVE");
    assert_eq!(hash(&a), edited, "nothing was replaced");
    let dry = sb.run(&["restore", "--dry-run", "--json", s(&a)]);
    assert_eq!(items(&dry.json())[0]["current"], "modified");
    assert_eq!(
        dry.code(),
        3,
        "a dry run reports the refusal the real run would give"
    );

    // As a copy: the original comes back beside it, nothing is replaced.
    let r = sb.run(&["restore", "--if-modified", "copy", s(&a)]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    assert_eq!(
        fs::read(sb.work().join("a (restored).jpg")).unwrap(),
        original
    );
    assert_eq!(hash(&a), edited);

    // Restore anyway: the original is back, and the edited file is kept inside the backup.
    let r = sb.run(&["restore", "--if-modified", "backup", s(&a)]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    assert_eq!(fs::read(&a).unwrap(), original);
    let kept = only_backup_dir(&sb).join("replaced-by-restore.jpg");
    assert_eq!(hash(&kept), edited, "what was there is not lost");
}

#[test]
fn restore_reports_unknown_targets_and_repeats_gently() {
    let sb = Sandbox::new();
    let a = good_photo(&sb.work().join("a.jpg"), 1);
    let ha = hash(&a);
    let r = sb.run(&["process", "--json", s(&a)]);
    let id = items(&r.json())[0]["backup_id"]
        .as_str()
        .unwrap()
        .to_owned();
    // A file nobody backed up, and an id that does not exist.
    let r = sb.run(&[
        "restore",
        "--json",
        s(&sb.work().join("never.jpg")),
        "0123456789abcdef0123456789ab",
    ]);
    assert_eq!(r.code(), 3);
    assert!(items(&r.json()).iter().all(|i| i["code"] == "NO_BACKUP"));
    // By backup id; again by id: already restored, skipped, not an error.
    let r = sb.run(&["restore", &id]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    assert_eq!(hash(&a), ha);
    let again = sb.run(&["restore", "--json", &id]);
    assert_eq!(again.code(), 0);
    assert_eq!(items(&again.json())[0]["code"], "ALREADY_RESTORED");
    // By path after a restore: there is no saved backup left for it.
    let by_path = sb.run(&["restore", "--json", s(&a)]);
    assert_eq!(by_path.code(), 3);
}

#[test]
fn restoring_a_split_scan_keeps_or_removes_the_derived_files() {
    for remove in [false, true] {
        let sb = Sandbox::new();
        let bed = bed_scan(&sb.work().join("bed.jpg"));
        let hb = hash(&bed);
        let r = sb.run(&["process", "--quiet", "--accept-splits", s(&bed)]);
        assert_eq!(r.code(), 0, "{}", r.stderr());
        assert!(!bed.exists());
        // Naming one of the parts finds the scan's backup.
        let part = sb.work().join("bed_02.jpg");
        let mut args = vec!["restore", "--json"];
        if remove {
            args.extend(["--derived", "remove"]);
        }
        args.push(s(&part));
        let r = sb.run(&args);
        assert_eq!(r.code(), 0, "{}", r.stderr());
        assert_eq!(hash(&bed), hb, "the scan is back byte for byte");
        let derived = items(&r.json())[0]["derived"].as_array().unwrap().clone();
        assert_eq!(derived.len(), 4);
        for n in 1..=4 {
            let exists = sb.work().join(format!("bed_0{n}.jpg")).exists();
            assert_eq!(exists, !remove, "derived file {n} (remove = {remove})");
        }
        let want = if remove { "removed" } else { "unchanged" };
        assert!(derived.iter().all(|d| d["state"] == want), "{derived:?}");
    }
}

// ---------------------------------------------------------------- backups

#[test]
fn backups_list_show_and_purge() {
    let sb = Sandbox::new();
    let a = good_photo(&sb.work().join("a.jpg"), 1);
    let b = good_photo(&sb.work().join("b.jpg"), 2);
    assert_eq!(sb.run(&["process", "--quiet", s(&a)]).code(), 0);
    assert_eq!(sb.run(&["process", "--quiet", s(&b)]).code(), 0);

    let list = sb.run(&["backups", "list", "--json"]);
    assert_eq!(list.code(), 0, "{}", list.stderr());
    let doc = list.json();
    assert_eq!(doc["schema"], "auto-crop/backups");
    let runs = doc["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 2, "one run per invocation");
    let files: Vec<&Value> = runs
        .iter()
        .flat_map(|r| r["files"].as_array().unwrap())
        .collect();
    assert_eq!(files.len(), 2);
    // ids[0] is a.jpg's backup, ids[1] is b.jpg's (the listing is newest first).
    let id_of = |name: &str| -> String {
        let f = files
            .iter()
            .find(|f| f["displayPath"].as_str().unwrap().ends_with(name))
            .unwrap();
        f["id"]
            .as_str()
            .unwrap()
            .split('/')
            .next()
            .unwrap()
            .to_owned()
    };
    let ids = [id_of("a.jpg"), id_of("b.jpg")];
    // Usage equals a walk of the folder.
    let walked: u64 = tree(&sb.home().join("data/backups")).len() as u64;
    assert!(walked >= 4, "an original and a manifest per backup");
    assert!(doc["usedBytes"].as_u64().unwrap() > 0);

    let text = sb.run(&["backups", "list"]);
    assert!(
        text.stdout().contains(&ids[0]) && text.stdout().contains(&ids[1]),
        "{}",
        text.stdout()
    );
    let show = sb.run(&["backups", "show", &ids[0]]);
    assert_eq!(show.code(), 0, "{}", show.stderr());
    assert!(show.stdout().contains("blake3") && show.stdout().contains(&ids[0]));
    let showj = sb.run(&["backups", "show", "--json", &ids[0]]);
    assert_eq!(showj.json()["backup"]["id"], ids[0].as_str());
    assert_eq!(showj.json()["state"], "saved");
    assert_eq!(
        sb.run(&["backups", "show", "0123456789abcdef0123456789ab"])
            .code(),
        3
    );

    // Listing never purges and never writes.
    let before = tree(&sb.home());
    let _ = sb.run(&["backups", "list"]);
    let _ = sb.run(&["backups", "show", &ids[0]]);
    assert_eq!(tree(&sb.home()), before);

    // Purge refuses without --yes when stdin is not a terminal; --dry-run deletes nothing.
    let r = sb.run(&["backups", "purge", "--all"]);
    assert_eq!(r.code(), 2, "{}", r.stderr());
    assert_eq!(tree(&sb.home()), before);
    let r = sb.run(&["backups", "purge", "--all", "--dry-run"]);
    assert_eq!(r.code(), 0);
    assert!(r.stderr().contains("would delete"), "{}", r.stderr());
    assert_eq!(tree(&sb.home()), before);

    // One backup by id; the other file is still restorable.
    let r = sb.run(&["backups", "purge", "--id", &ids[0], "--yes"]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    let list = sb.run(&["backups", "list", "--json"]).json();
    assert_eq!(list["runs"].as_array().unwrap().len(), 1);
    assert_eq!(sb.run(&["restore", "--quiet", s(&b)]).code(), 0);
    // An unknown id is an error and deletes nothing.
    assert_eq!(
        sb.run(&[
            "backups",
            "purge",
            "--id",
            "0123456789abcdef0123456789ab",
            "--yes"
        ])
        .code(),
        3
    );
}

#[test]
fn only_expired_backups_are_purged_by_expired_and_only_on_request() {
    let sb = Sandbox::new();
    let a = good_photo(&sb.work().join("a.jpg"), 1);
    let b = good_photo(&sb.work().join("b.jpg"), 2);
    let r = sb.run(&["process", "--json", s(&a)]);
    let id_a = items(&r.json())[0]["backup_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(sb.run(&["process", "--quiet", s(&b)]).code(), 0);
    // Age the first backup past its retention by editing its manifest.
    let mpath = sb
        .home()
        .join("data/backups")
        .join(&id_a)
        .join("manifest.json");
    let mut m: Value = serde_json::from_str(&fs::read_to_string(&mpath).unwrap()).unwrap();
    m["purge_after"] = Value::from(1);
    fs::write(&mpath, serde_json::to_vec_pretty(&m).unwrap()).unwrap();
    // Listing leaves it alone; so does a dry run.
    assert_eq!(
        sb.run(&["backups", "list", "--json"]).json()["runs"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let r = sb.run(&["backups", "purge", "--expired", "--dry-run"]);
    assert!(r.stderr().contains(&id_a), "{}", r.stderr());
    assert!(mpath.exists());
    let r = sb.run(&["backups", "purge", "--expired", "--yes", "--quiet"]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    assert!(!mpath.exists(), "the expired backup is gone");
    assert_eq!(
        sb.run(&["backups", "list", "--json"]).json()["runs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    // Nothing selected is not an error.
    assert_eq!(
        sb.run(&[
            "backups",
            "purge",
            "--older-than",
            "3650",
            "--yes",
            "--quiet"
        ])
        .code(),
        0
    );
    assert_eq!(
        sb.run(&["backups", "list", "--json"]).json()["runs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

// ---------------------------------------------------------------- analyze

#[test]
fn analyze_reports_without_writing_and_emits_edits() {
    let sb = Sandbox::new();
    let a = good_photo(&sb.work().join("a.jpg"), 1);
    no_document(&sb.work().join("none.jpg"));
    bed_scan(&sb.work().join("bed.jpg"));
    let before = tree(&sb.work());
    let edits = sb.dir.path().join("edits");

    let r = sb.run(&[
        "analyze",
        "--json",
        "--timings",
        "--emit-edit",
        s(&edits),
        s(&sb.work()),
    ]);
    assert_eq!(
        r.code(),
        0,
        "{} (a held image is a report, not a failure)",
        r.stderr()
    );
    let doc = r.json();
    assert_eq!(doc["schema"], "auto-crop/analysis");
    assert_eq!(doc["summary"]["items"], 3);
    assert_eq!(
        doc["summary"]["write"], 2,
        "the good photo and the split scan"
    );
    assert_eq!(doc["summary"]["held"], 1);
    let by = |n: &str| {
        items(&doc)
            .iter()
            .find(|i| i["input"].as_str().unwrap().ends_with(n))
            .unwrap()
            .clone()
    };
    assert_eq!(by("a.jpg")["decision"], "write");
    assert_eq!(by("a.jpg")["crops"].as_array().unwrap().len(), 1);
    assert_eq!(by("a.jpg")["crops"][0]["quad"].as_array().unwrap().len(), 4);
    assert_eq!(by("a.jpg")["confidence"]["band"], "good");
    assert!(by("a.jpg")["ms"]["analyse"].is_number());
    assert_eq!(by("none.jpg")["decision"], "held");
    assert_eq!(by("none.jpg")["hold_code"], "DETECTION_FAILED");
    assert_eq!(by("bed.jpg")["split"], true);
    assert_eq!(by("bed.jpg")["crops"].as_array().unwrap().len(), 4);
    // Nothing was written outside the edits folder: not the tree, not the store.
    assert_eq!(tree(&sb.work()), before);
    assert!(!sb.home().exists());
    assert!(edits.join("a.edit.json").is_file());
    let edit: Value =
        serde_json::from_str(&fs::read_to_string(edits.join("bed.edit.json")).unwrap()).unwrap();
    assert_eq!(edit["items"].as_array().unwrap().len(), 4);

    // Text and ndjson forms.
    let t = sb.run(&["analyze", s(&a)]);
    assert!(
        t.stdout().contains("write") && t.stdout().contains("crop 1:"),
        "{}",
        t.stdout()
    );
    let n = sb.run(&["analyze", "--ndjson", s(&a)]);
    let lines: Vec<Value> = n
        .stdout()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines[0]["t"], "item");
    assert_eq!(lines.last().unwrap()["t"], "end");
    // Failures count; an empty folder is "no input".
    assert_eq!(
        sb.run(&["analyze", s(&sb.work().join("missing.jpg"))])
            .code(),
        3
    );
    let empty = sb.dir.path().join("empty");
    fs::create_dir_all(&empty).unwrap();
    assert_eq!(sb.run(&["analyze", s(&empty)]).code(), 5);
}

#[test]
fn the_cutoff_decides_what_is_written_and_a_held_result_stays_held() {
    let sb = Sandbox::new();
    let a = good_photo(&sb.work().join("a.jpg"), 1);
    let ha = hash(&a);
    // A cut-off above the score turns a good result into a held one, in analyze and in process.
    let an = sb.run(&["analyze", "--json", "--min-confidence", "1.0", s(&a)]);
    assert_eq!(items(&an.json())[0]["decision"], "held");
    assert_eq!(items(&an.json())[0]["hold_code"], "LOW_CONFIDENCE");
    let r = sb.run(&["process", "--json", "--min-confidence", "1.0", s(&a)]);
    assert_eq!(r.code(), 4, "{}", r.stderr());
    assert_eq!(items(&r.json())[0]["code"], "LOW_CONFIDENCE");
    assert_eq!(items(&r.json())[0]["confidence"]["band"], "check");
    assert_eq!(hash(&a), ha);
    let r = sb.run(&["process", "--quiet", "--triage", "aggressive", s(&a)]);
    assert_eq!(r.code(), 0);
    assert_ne!(hash(&a), ha);
}

// ---------------------------------------------------------------- render

#[test]
fn render_matches_process_and_never_touches_the_source() {
    let sb = Sandbox::new();
    let a = good_photo(&sb.work().join("a.jpg"), 1);
    // A twin of a.jpg, processed in place, is what render must reproduce.
    let twin = sb.work().join("twin.jpg");
    fs::copy(&a, &twin).unwrap();
    let ha = hash(&a);
    assert_eq!(sb.run(&["process", "--quiet", s(&twin)]).code(), 0);

    let out = sb.dir.path().join("out/r.jpg");
    let r = sb.run(&["render", "-o", s(&out), s(&a)]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    assert_eq!(
        r.stdout().trim(),
        s(&std::path::absolute(&out).unwrap()),
        "the output path is on stdout"
    );
    assert_eq!(hash(&a), ha, "the source is untouched");
    assert_eq!(
        fs::read(&out).unwrap(),
        fs::read(&twin).unwrap(),
        "render equals process, byte for byte"
    );

    // Through an edit state: analyze --emit-edit, then render --edit gives the same bytes.
    let edits = sb.dir.path().join("edits");
    assert_eq!(
        sb.run(&["analyze", "--quiet", "--emit-edit", s(&edits), s(&a)])
            .code(),
        0
    );
    let out2 = sb.dir.path().join("out/r2.jpg");
    let r = sb.run(&[
        "render",
        "--edit",
        s(&edits.join("a.edit.json")),
        "-o",
        s(&out2),
        s(&a),
    ]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    assert_eq!(fs::read(&out2).unwrap(), fs::read(&out).unwrap());

    // Format and quality.
    let png = sb.dir.path().join("out/r.png");
    assert_eq!(sb.run(&["render", "-o", s(&png), s(&a)]).code(), 0);
    assert_eq!(&fs::read(&png).unwrap()[1..4], b"PNG");
    let q30 = sb.dir.path().join("out/q30.jpg");
    assert_eq!(
        sb.run(&["render", "--quality", "30", "-o", s(&q30), s(&a)])
            .code(),
        0
    );
    assert!(fs::metadata(&q30).unwrap().len() * 2 < fs::metadata(&out).unwrap().len());

    // Refusals: an existing output, then --force; the source as output; no output at all.
    let again = sb.run(&["render", "-o", s(&out), s(&a)]);
    assert_eq!(again.code(), 3);
    assert!(again.stderr().contains("--force"), "{}", again.stderr());
    assert_eq!(
        sb.run(&["render", "--force", "--quality", "50", "-o", s(&out), s(&a)])
            .code(),
        0
    );
    assert_ne!(fs::read(&out).unwrap(), fs::read(&twin).unwrap());
    assert_eq!(sb.run(&["render", "-o", s(&a), s(&a)]).code(), 2);
    assert_eq!(sb.run(&["render", "--force", "-o", s(&a), s(&a)]).code(), 2);
    assert_eq!(hash(&a), ha);
    assert_eq!(sb.run(&["render", s(&a)]).code(), 2);
}

#[test]
fn render_writes_every_item_of_a_split_scan_and_says_when_process_would_hold() {
    let sb = Sandbox::new();
    let bed = bed_scan(&sb.work().join("bed.jpg"));
    let none = no_document(&sb.work().join("none.jpg"));
    let hb = hash(&bed);
    let out = sb.dir.path().join("o/scan.jpg");
    let r = sb.run(&["render", "--json", "-o", s(&out), s(&bed)]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    assert_eq!(r.json()["outputs"].as_array().unwrap().len(), 4);
    for n in 1..=4 {
        assert!(sb.dir.path().join(format!("o/scan_0{n}.jpg")).is_file());
    }
    assert!(size_of(&sb.dir.path().join("o/scan_01.jpg")).0 > 300);
    assert_eq!(hash(&bed), hb);
    // --split never: one crop, and the split is not looked for.
    let one = sb.dir.path().join("o/one.jpg");
    assert_eq!(
        sb.run(&["render", "--split", "never", "-o", s(&one), s(&bed)])
            .code(),
        0
    );
    assert!(one.is_file());
    // No crop found: an error, nothing written.
    let nothing = sb.dir.path().join("o/none.jpg");
    let r = sb.run(&["render", "-o", s(&nothing), s(&none)]);
    assert_eq!(r.code(), 3, "{}", r.stderr());
    assert!(!nothing.exists());
}

// ---------------------------------------------------------------- doctor, version, help

#[test]
fn doctor_and_version_report_without_writing() {
    let sb = Sandbox::new();
    let v = sb.run(&["--version"]);
    assert_eq!(v.code(), 0);
    for needle in [
        "auto-crop 0.",
        "target:",
        "features:",
        "decoders:",
        "jpeg",
        "network:",
    ] {
        assert!(v.stdout().contains(needle), "{needle} in {}", v.stdout());
    }
    let d = sb.run(&["doctor"]);
    assert_eq!(d.code(), 0, "{}", d.stdout());
    for needle in [
        "cpu:",
        "avx2:",
        "memory:",
        "backups:",
        "decoders:",
        "heif:",
        "sandbox:",
        "webview:",
    ] {
        assert!(d.stdout().contains(needle), "{needle} in {}", d.stdout());
    }
    let j = sb.run(&["doctor", "--json"]).json();
    assert_eq!(j["schema"], "auto-crop/doctor");
    assert_eq!(j["ok"], true);
    assert_eq!(j["store"]["exists"], false);
    assert!(j["memory"]["total_bytes"].as_u64().unwrap() > 0);
    assert!(
        j["decoders"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d == "jpeg")
    );
    assert!(!sb.home().exists(), "doctor never creates the store");
    if cfg!(target_arch = "x86_64") {
        let r = sb.run_with_env(&["doctor", "--json"], &[("AUTO_CROP_FORCE_NO_AVX2", "1")]);
        assert_eq!(r.code(), 6);
        assert_eq!(r.json()["cpu"]["avx2"], false);
        assert_eq!(r.json()["ok"], false);
    }
}

#[test]
fn help_and_usage_errors() {
    let sb = Sandbox::new();
    let h = sb.run(&["--help"]);
    assert_eq!(h.code(), 0);
    for c in [
        "process", "analyze", "render", "restore", "backups", "doctor",
    ] {
        assert!(h.stdout().contains(c), "{c}");
    }
    let p = sb.run(&["process", "--help"]);
    assert_eq!(p.code(), 0);
    for f in [
        "--dry-run",
        "--output",
        "--suffix",
        "--copy",
        "--split",
        "--profile",
        "--triage",
        "--manifest",
        "--jobs",
        "Exit codes",
    ] {
        assert!(p.stdout().contains(f), "{f} in {}", p.stdout());
    }
    assert_eq!(sb.run(&["help", "backups"]).code(), 0);
    let bad = sb.run(&["process", "x.jpg", "--dryrun"]);
    assert_eq!(bad.code(), 2);
    assert!(
        bad.stderr().contains("did you mean `--dry-run`"),
        "{}",
        bad.stderr()
    );
    assert!(bad.stderr().contains("--help"));
    assert_eq!(bad.stdout(), "");
}
