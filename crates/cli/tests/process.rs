// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `auto-crop process` end to end through the real binary, on synthetic images, each test in its
//! own sandbox (its own `--home`, so no test reads or writes a real backup store).

mod common;

use auto_crop_codecs::{Format, decode, probe};
use common::*;
use serde_json::Value;
use std::fs;
use std::path::Path;

fn items(doc: &Value) -> &Vec<Value> {
    doc["items"].as_array().expect("items")
}

fn dims(p: &Path) -> (u32, u32) {
    let pr = probe(&fs::read(p).unwrap()).unwrap();
    (pr.width, pr.height)
}

fn no_temp_files(dir: &Path) -> bool {
    fn walk(d: &Path) -> bool {
        fs::read_dir(d).map_or(true, |rd| {
            rd.flatten().all(|e| {
                let p = e.path();
                if p.is_dir() {
                    walk(&p)
                } else {
                    !e.file_name().to_string_lossy().starts_with(".autocrop-")
                }
            })
        })
    }
    walk(dir)
}

// ---------------------------------------------------------------- the default: overwrite with a backup

#[test]
fn overwrite_in_place_backs_up_and_restore_is_byte_identical() {
    let sb = Sandbox::new();
    let a = good_photo(&sb.work().join("a.jpg"), 1);
    let b = good_photo(&sb.work().join("b.jpg"), 2);
    let (ha, hb) = (hash(&a), hash(&b));

    let r = sb.run(&["process", "--json", s(&sb.work())]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    let doc = r.json();
    assert_valid_manifest(&doc);
    assert_eq!(doc["summary"]["saved"], 2);
    for it in items(&doc) {
        assert_eq!(it["status"], "saved");
        assert_eq!(it["written"], true);
        assert!(it["backup_id"].is_string(), "{it}");
        assert_eq!(it["confidence"]["band"], "good");
    }
    // The originals were replaced by smaller crops; the notice came once, on stderr.
    assert_ne!(hash(&a), ha);
    assert!(dims(&a).0 < 1200 && dims(&a).1 <= 900);
    assert!(
        r.stderr().contains("replaces your originals"),
        "{}",
        r.stderr()
    );
    assert!(
        r.stdout().trim_start().starts_with('{'),
        "stdout is the JSON document only"
    );

    // Run again: the outputs are recognised, nothing is processed twice, the notice is not repeated.
    let again = sb.run(&["process", "--json", s(&sb.work())]);
    assert_eq!(again.code(), 0, "{}", again.stderr());
    let doc2 = again.json();
    assert_eq!(doc2["summary"]["skipped"], 2);
    assert_eq!(items(&doc2)[0]["code"], "ALREADY_PROCESSED");
    assert!(!again.stderr().contains("replaces your originals"));

    // Restore from a new process: byte identical.
    let rr = sb.run(&["restore", s(&a)]);
    assert_eq!(rr.code(), 0, "{}", rr.stderr());
    assert_eq!(hash(&a), ha, "restore by path is byte identical");
    let run_id = doc["run"]["id"].as_str().unwrap().to_owned();
    let rr = sb.run(&["restore", "--run", &run_id]);
    assert_eq!(rr.code(), 0, "{}", rr.stderr());
    assert_eq!(hash(&b), hb, "restore by run is byte identical");
    assert!(no_temp_files(&sb.work()));
}

#[test]
fn the_one_time_notice_is_stderr_only_and_not_given_to_quiet_or_no_config() {
    let sb = Sandbox::new();
    good_photo(&sb.work().join("a.jpg"), 1);
    good_photo(&sb.work().join("b.jpg"), 2);
    let q = sb.run(&["process", "--quiet", s(&sb.work().join("a.jpg"))]);
    assert_eq!(q.code(), 0);
    assert_eq!(q.stderr(), "", "quiet prints nothing on success");
    assert_eq!(q.stdout(), "");
    let r = sb.run(&["process", s(&sb.work().join("b.jpg"))]);
    assert_eq!(r.code(), 0);
    assert_eq!(r.stderr().matches("replaces your originals").count(), 1);
    assert_eq!(
        r.stdout(),
        "",
        "stdout is for machines: empty without --json"
    );
}

fn populate(sb: &Sandbox) {
    good_photo(&sb.work().join("a.jpg"), 1);
    good_photo(&sb.work().join("sub/b.jpg"), 2);
    no_document(&sb.work().join("none.jpg"));
    bed_scan(&sb.work().join("bed.jpg"));
}

/// The plan of a manifest with the sandbox folder taken out of every path, to compare two runs
/// made in two sandboxes.
fn plan_of(doc: &Value, sb: &Sandbox) -> Vec<(String, String, Vec<String>)> {
    let root = sb.dir.path().to_string_lossy().replace('\\', "/");
    let norm = |p: &str| p.replace('\\', "/").replacen(&root, "", 1);
    items(doc)
        .iter()
        .map(|i| {
            (
                norm(i["input"].as_str().unwrap()),
                format!("{}/{}", i["status"], i["code"]),
                i["outputs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|o| norm(o["path"].as_str().unwrap()))
                    .collect(),
            )
        })
        .collect()
}

#[test]
fn dry_run_writes_nothing_and_plans_what_the_real_run_does() {
    // Three modes: the default (in place), --output and --suffix. The dry run and the real run
    // each get their own sandbox with the same files, so the dry run's tree check stays strict.
    for mode in ["in_place", "output", "suffix"] {
        let (dry_sb, real_sb) = (Sandbox::new(), Sandbox::new());
        populate(&dry_sb);
        populate(&real_sb);
        let mode_args = |sb: &Sandbox| -> Vec<String> {
            match mode {
                "output" => vec!["--output".into(), s(&sb.dir.path().join("out")).to_owned()],
                "suffix" => vec!["--suffix".into(), "_c".into()],
                _ => vec![],
            }
        };
        let before = tree(&dry_sb.work());

        let mut args: Vec<String> = vec![
            "process".into(),
            "--dry-run".into(),
            "--json".into(),
            "-r".into(),
        ];
        args.extend(mode_args(&dry_sb));
        args.push(s(&dry_sb.work()).to_owned());
        let argv: Vec<&str> = args.iter().map(String::as_str).collect();
        let dry = dry_sb.run(&argv);
        assert!(matches!(dry.code(), 0 | 4), "{mode}: {}", dry.stderr());
        let dry_doc = dry.json();
        assert_valid_manifest(&dry_doc);
        assert_eq!(dry_doc["run"]["dry_run"], true);
        assert_eq!(
            tree(&dry_sb.work()),
            before,
            "{mode}: the tree is untouched"
        );
        assert!(
            !dry_sb.home().exists(),
            "{mode}: no store, settings or notice record was created"
        );
        assert!(!dry_sb.dir.path().join("out").exists(), "{mode}");
        assert!(
            !dry.stderr().contains("replaces your originals"),
            "no notice on a dry run"
        );
        assert!(items(&dry_doc).iter().all(|i| i["written"] == false));

        let mut args: Vec<String> = vec!["process".into(), "--json".into(), "-r".into()];
        args.extend(mode_args(&real_sb));
        args.push(s(&real_sb.work()).to_owned());
        let argv: Vec<&str> = args.iter().map(String::as_str).collect();
        let real = real_sb.run(&argv);
        assert_eq!(real.code(), dry.code(), "{mode}: {}", real.stderr());
        let real_doc = real.json();
        assert_eq!(
            plan_of(&dry_doc, &dry_sb),
            plan_of(&real_doc, &real_sb),
            "{mode}: the dry run's plan (statuses, codes, output paths) equals the real run"
        );
    }
}

// ---------------------------------------------------------------- output modes

#[test]
fn output_suffix_and_copy_modes_leave_the_originals_alone() {
    let sb = Sandbox::new();
    let a = good_photo(&sb.work().join("a.jpg"), 1);
    let b = good_photo(&sb.work().join("sub/b.jpg"), 2);
    let before = tree(&sb.work());
    let out = sb.dir.path().join("out");

    let r = sb.run(&[
        "process",
        "--json",
        "-r",
        "--output",
        s(&out),
        s(&sb.work()),
    ]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    assert_valid_manifest(&r.json());
    assert_eq!(r.json()["run"]["options"]["mode"], "output");
    assert!(
        out.join("a.jpg").is_file() && out.join("sub/b.jpg").is_file(),
        "the tree is mirrored"
    );
    assert_eq!(tree(&sb.work()), before, "originals untouched");
    assert!(
        !r.stderr().contains("replaces your originals"),
        "no notice for copies"
    );
    for it in items(&r.json()) {
        assert!(it["backup_id"].is_null(), "a copy needs no backup");
    }
    // Nothing went into the backup store.
    let list = sb.run(&["backups", "list", "--json"]);
    assert_eq!(list.json()["runs"].as_array().unwrap().len(), 0);

    // --suffix writes beside the original; a second run numbers instead of overwriting.
    let r = sb.run(&["process", "--suffix", "_c", s(&a)]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    assert!(sb.work().join("a_c.jpg").is_file());
    let first = fs::read(sb.work().join("a_c.jpg")).unwrap();
    let r = sb.run(&["process", "--suffix", "_c", s(&a)]);
    assert_eq!(r.code(), 0);
    assert!(sb.work().join("a (2)_c.jpg").is_file());
    assert_eq!(
        fs::read(sb.work().join("a_c.jpg")).unwrap(),
        first,
        "the first copy was not overwritten"
    );
    let r = sb.run(&[
        "process",
        "--json",
        "--suffix",
        "_c",
        "--if-exists",
        "skip",
        s(&a),
    ]);
    let doc = r.json();
    assert_eq!(items(&doc)[0]["status"], "skipped");
    assert_eq!(items(&doc)[0]["code"], "EXISTS");
    assert_eq!(r.code(), 0, "skipped by choice is not a failure");

    // --copy: an AutoCrop folder beside the original.
    let r = sb.run(&["process", "--copy", s(&b)]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    assert!(sb.work().join("sub/AutoCrop/b.jpg").is_file());
    assert_eq!(hash(&b), hash(&sb.work().join("sub/b.jpg")));
    // The AutoCrop folder is not walked back in.
    let r = sb.run(&[
        "process",
        "--json",
        "--dry-run",
        "--suffix",
        "_x",
        "-r",
        s(&sb.work().join("sub")),
    ]);
    assert_eq!(r.json()["summary"]["items"], 1);
    assert_eq!(r.json()["summary"]["hidden_skipped"], 1);
    assert!(no_temp_files(&sb.work()) && no_temp_files(&out));
}

#[test]
fn format_quality_margin_and_name_template_apply_to_copies() {
    let sb = Sandbox::new();
    let a = good_photo(&sb.work().join("a.jpg"), 1);
    let before = hash(&a);
    let o = |n: &str| sb.dir.path().join(n);
    let run = |args: &[&str]| {
        let mut v = vec!["process", "--quiet"];
        v.extend_from_slice(args);
        let r = sb.run(&v);
        assert_eq!(r.code(), 0, "{}", r.stderr());
    };
    run(&["--output", s(&o("png")), "--format", "png", s(&a)]);
    let png = fs::read(o("png").join("a.png")).unwrap();
    assert_eq!(&png[1..4], b"PNG");
    assert_eq!(decode(&png).unwrap().format, Format::Png);
    run(&["--output", s(&o("q30")), "--quality", "30", s(&a)]);
    run(&["--output", s(&o("q95")), "--quality", "95", s(&a)]);
    let (small, big) = (
        fs::metadata(o("q30").join("a.jpg")).unwrap().len(),
        fs::metadata(o("q95").join("a.jpg")).unwrap().len(),
    );
    assert!(
        small * 2 < big,
        "quality 30 ({small}) is far smaller than 95 ({big})"
    );
    run(&["--output", s(&o("m0")), s(&a)]);
    run(&["--output", s(&o("m5")), "--margin", "5", s(&a)]);
    run(&["--output", s(&o("mneg")), "--margin", "-5", s(&a)]);
    let (w0, w5, wn) = (
        dims(&o("m0").join("a.jpg")).0,
        dims(&o("m5").join("a.jpg")).0,
        dims(&o("mneg").join("a.jpg")).0,
    );
    assert!(
        w5 > w0 && wn < w0,
        "margin grows and trims the crop: {wn} < {w0} < {w5}"
    );
    run(&[
        "--output",
        s(&o("tpl")),
        "--name-template",
        "scan-{name}",
        s(&a),
    ]);
    assert!(o("tpl").join("scan-a.jpg").is_file());
    // The source never changed.
    assert_eq!(hash(&a), before);
}

#[test]
fn the_default_mode_refuses_a_format_change_instead_of_guessing() {
    let sb = Sandbox::new();
    let a = good_photo(&sb.work().join("a.jpg"), 1);
    let before = hash(&a);
    let r = sb.run(&["process", "--format", "png", s(&a)]);
    assert_eq!(r.code(), 2);
    assert!(r.stderr().contains("--output"), "{}", r.stderr());
    assert_eq!(hash(&a), before);
}

// ---------------------------------------------------------------- held results are never written

#[test]
fn held_items_are_not_written_and_exit_four() {
    let sb = Sandbox::new();
    let none = no_document(&sb.work().join("none.jpg"));
    let bed = bed_scan(&sb.work().join("bed.jpg"));
    let (hn, hb) = (hash(&none), hash(&bed));
    let r = sb.run(&["process", "--json", s(&none), s(&bed)]);
    assert_eq!(r.code(), 4, "{}", r.stderr());
    let doc = r.json();
    assert_valid_manifest(&doc);
    assert_eq!(doc["summary"]["held"], 2);
    assert_eq!(items(&doc)[0]["code"], "DETECTION_FAILED");
    assert_eq!(items(&doc)[1]["code"], "SPLIT_HELD");
    assert_eq!(items(&doc)[1]["split"], true);
    assert_eq!(items(&doc)[1]["crops"], 4);
    assert_eq!(
        (hash(&none), hash(&bed)),
        (hn.clone(), hb),
        "held files are byte identical"
    );
    assert!(no_temp_files(&sb.work()));
    // No backup was made for what was not written.
    let list = sb.run(&["backups", "list", "--json"]);
    assert_eq!(list.json()["runs"].as_array().unwrap().len(), 0);
    // --hold-exit-zero is the only thing it changes.
    let r = sb.run(&["process", "--hold-exit-zero", s(&none)]);
    assert_eq!(r.code(), 0);
    assert_eq!(hash(&none), hn);
    // No preset forces a held item: the failed floor holds in every mode.
    for t in ["balanced", "aggressive"] {
        let r = sb.run(&["process", "--triage", t, s(&none)]);
        assert_eq!(r.code(), 4, "{t}");
    }
    let r = sb.run(&["process", "--min-confidence", "0.6", s(&none)]);
    assert_eq!(r.code(), 4);
    assert_eq!(hash(&none), hn);
}

#[test]
fn a_split_scan_is_written_as_copies_and_replaced_only_when_accepted() {
    let sb = Sandbox::new();
    let bed = bed_scan(&sb.work().join("bed.jpg"));
    let hb = hash(&bed);
    // Copies destroy nothing: written without acceptance, named {name}_{n}.
    let r = sb.run(&["process", "--json", "--copy", s(&bed)]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    let doc = r.json();
    assert_valid_manifest(&doc);
    assert_eq!(items(&doc)[0]["outputs"].as_array().unwrap().len(), 4);
    for n in 1..=4 {
        let f = sb.work().join(format!("AutoCrop/bed_0{n}.jpg"));
        assert!(f.is_file(), "{f:?}");
    }
    assert_eq!(hash(&bed), hb);

    // Accepted in place: the scan is replaced by the files, after a verified backup; restore returns it.
    let r = sb.run(&["process", "--json", "--accept-splits", s(&bed)]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    let doc = r.json();
    assert_valid_manifest(&doc);
    assert_eq!(items(&doc)[0]["status"], "saved");
    assert!(items(&doc)[0]["backup_id"].is_string());
    assert!(!bed.exists(), "the scan was replaced by its parts");
    for n in 1..=4 {
        assert!(sb.work().join(format!("bed_0{n}.jpg")).is_file());
    }
    let rr = sb.run(&["restore", s(&bed)]);
    assert_eq!(rr.code(), 0, "{}", rr.stderr());
    assert_eq!(hash(&bed), hb, "the scan is back byte for byte");
    assert!(
        sb.work().join("bed_01.jpg").is_file(),
        "derived files are kept by default"
    );
    assert!(no_temp_files(&sb.work()));
}

// ---------------------------------------------------------------- input expansion and skips

#[test]
fn unsupported_and_never_replaced_files_are_counted_and_left_alone() {
    let sb = Sandbox::new();
    let work = sb.work();
    good_photo(&work.join("good.jpg"), 1);
    fs::write(work.join("pic.gif"), b"GIF89a\x01\x00\x01\x00").unwrap();
    fs::write(work.join("doc.pdf"), b"%PDF-1.7").unwrap();
    fs::write(work.join("notes.txt"), b"hello").unwrap();
    fs::write(
        work.join("scan.webp"),
        auto_crop_codecs::fixtures::webp_lossless(64, 48),
    )
    .unwrap();
    fs::write(
        work.join("scan.tif"),
        auto_crop_codecs::fixtures::tiff_rgb8(
            64,
            48,
            &auto_crop_codecs::fixtures::TiffOpts::default(),
        ),
    )
    .unwrap();
    let (hw, ht) = (hash(&work.join("scan.webp")), hash(&work.join("scan.tif")));

    let r = sb.run(&["process", "--json", s(&work)]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    let doc = r.json();
    assert_valid_manifest(&doc);
    assert_eq!(
        doc["summary"]["ignored_non_image"], 3,
        "gif, pdf and txt are not images this build reads"
    );
    let by = |name: &str| {
        items(&doc)
            .iter()
            .find(|i| i["input"].as_str().unwrap().ends_with(name))
            .unwrap()
            .clone()
    };
    for f in ["scan.webp", "scan.tif"] {
        let it = by(f);
        assert_eq!(it["status"], "skipped", "{f}");
        assert_eq!(it["code"], "NOT_REPLACEABLE");
        assert_eq!(it["reasons"][0], "format.write_unavailable");
    }
    assert_eq!(
        (hash(&work.join("scan.webp")), hash(&work.join("scan.tif"))),
        (hw, ht)
    );

    // Named directly, an unsupported file is a skipped input, and with nothing else there is no input.
    let r = sb.run(&["process", "--json", s(&work.join("pic.gif"))]);
    assert_eq!(r.code(), 5, "{}", r.stderr());
    assert_eq!(items(&r.json())[0]["code"], "UNSUPPORTED_FORMAT");
    assert_eq!(items(&r.json())[0]["status"], "skipped");
}

#[test]
fn folders_walk_without_following_links_and_take_each_file_once() {
    let sb = Sandbox::new();
    let work = sb.work();
    good_photo(&work.join("a.jpg"), 1);
    good_photo(&work.join("sub/b.jpg"), 2);
    good_photo(&work.join("sub/deep/c.jpg"), 3);
    // A directory link back to the top: a symlink on Unix, a junction on Windows (no privilege needed).
    let link = work.join("sub").join("loop");
    let made = if cfg!(windows) {
        let o = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J", s(&link), s(&work)])
            .output();
        if let Ok(o) = &o {
            eprintln!(
                "mklink: {} {}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            );
        }
        o.is_ok_and(|o| o.status.success())
    } else {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&work, &link).is_ok()
        }
        #[cfg(not(unix))]
        {
            false
        }
    };
    if !made {
        eprintln!("cannot create a directory link here; the loop part is skipped");
    }
    let r = sb.run(&["process", "--json", "--dry-run", "-r", s(&work)]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    let doc = r.json();
    assert_valid_manifest(&doc);
    assert_eq!(
        doc["summary"]["items"], 3,
        "three files, each once, and the walk ended"
    );
    if made {
        assert_eq!(doc["summary"]["links_skipped"], 1);
    }
    // Without -r only the top folder is taken; --max-depth limits the walk.
    let flat = sb.run(&["process", "--json", "--dry-run", s(&work)]);
    assert_eq!(flat.json()["summary"]["items"], 1);
    let d1 = sb.run(&[
        "process",
        "--json",
        "--dry-run",
        "-r",
        "--max-depth",
        "1",
        s(&work),
    ]);
    assert_eq!(d1.json()["summary"]["items"], 2);
    assert!(
        d1.json()["warnings"][0]
            .as_str()
            .unwrap()
            .contains("--max-depth")
    );
    let capped = sb.run(&[
        "process",
        "--json",
        "--dry-run",
        "-r",
        "--max-files",
        "2",
        s(&work),
    ]);
    assert_eq!(capped.json()["summary"]["items"], 2);
    assert_eq!(capped.json()["summary"]["truncated"], true);
    // Patterns work where the shell does not expand them.
    let pat = work.join("*.jpg");
    let p = sb.run(&["process", "--json", "--dry-run", s(&pat)]);
    assert_eq!(p.json()["summary"]["items"], 1);
    let inc = sb.run(&[
        "process",
        "--json",
        "--dry-run",
        "-r",
        "--exclude",
        "b*",
        s(&work),
    ]);
    assert_eq!(inc.json()["summary"]["items"], 2);
    assert_eq!(inc.json()["summary"]["filtered_out"], 1);
}

#[test]
fn missing_corrupt_and_unmatched_inputs_fail_without_stopping_the_rest() {
    let sb = Sandbox::new();
    let good = good_photo(&sb.work().join("good.jpg"), 1);
    let bytes = fs::read(&good).unwrap();
    let corrupt = sb.work().join("corrupt.jpg");
    fs::write(&corrupt, &bytes[..bytes.len() / 3]).unwrap();
    let hc = hash(&corrupt);
    let r = sb.run(&[
        "process",
        "--json",
        "--output",
        s(&sb.dir.path().join("out")),
        s(&good),
        s(&corrupt),
        s(&sb.work().join("missing.jpg")),
        s(&sb.work().join("nope*.jpg")),
    ]);
    assert_eq!(r.code(), 3, "{}", r.stderr());
    let doc = r.json();
    assert_valid_manifest(&doc);
    let st: Vec<_> = items(&doc)
        .iter()
        .map(|i| {
            format!(
                "{}:{}",
                i["status"].as_str().unwrap(),
                i["code"].as_str().unwrap_or("")
            )
        })
        .collect();
    assert_eq!(
        st.iter().filter(|s| s.starts_with("failed")).count(),
        3,
        "{st:?}"
    );
    assert!(
        st.contains(&"failed:NOT_FOUND".to_owned()) && st.contains(&"failed:NO_MATCH".to_owned())
    );
    assert!(st.iter().any(|s| s == "failed:CORRUPT"), "{st:?}");
    assert_eq!(
        doc["summary"]["saved"], 1,
        "the good file was still processed"
    );
    assert_eq!(hash(&corrupt), hc);
    assert!(
        r.stderr().contains("failed"),
        "failures are printed even by default: {}",
        r.stderr()
    );
}

// ---------------------------------------------------------------- exit codes

#[test]
fn every_exit_code_is_reachable() {
    let sb = Sandbox::new();
    let good = good_photo(&sb.work().join("good.jpg"), 1);
    let none = no_document(&sb.work().join("none.jpg"));
    let empty = sb.dir.path().join("empty");
    fs::create_dir_all(&empty).unwrap();
    let out = sb.dir.path().join("out");

    assert_eq!(
        sb.run(&["process", "--output", s(&out), s(&good)]).code(),
        0,
        "ok"
    );
    assert_eq!(sb.run(&["process"]).code(), 2, "usage: no input");
    assert_eq!(
        sb.run(&["process", "--bogus", s(&good)]).code(),
        2,
        "usage: unknown flag"
    );
    assert_eq!(sb.run(&[]).code(), 2, "usage: nothing given");
    assert_eq!(
        sb.run(&["process", s(&sb.work().join("missing.jpg"))])
            .code(),
        3,
        "failed"
    );
    assert_eq!(
        sb.run(&["process", "--output", s(&out), s(&none)]).code(),
        4,
        "held"
    );
    assert_eq!(
        sb.run(&["process", s(&empty)]).code(),
        5,
        "no supported input"
    );

    // 6: a precondition. The backup folder cannot be created because its parent is a file.
    let blocker = sb.dir.path().join("blocker");
    fs::write(&blocker, b"x").unwrap();
    let hg = hash(&good);
    let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_auto-crop"));
    c.args(["--home", s(&blocker.join("home")), "process", s(&good)]);
    let r = c.output().unwrap();
    assert_eq!(
        r.status.code(),
        Some(6),
        "{}",
        String::from_utf8_lossy(&r.stderr)
    );
    assert_eq!(hash(&good), hg, "nothing was touched");
    // 6: a CPU below the AVX2 floor (simulated) exits before anything is read.
    if cfg!(target_arch = "x86_64") {
        let fresh = Sandbox::new();
        let g2 = good_photo(&fresh.work().join("g.jpg"), 1);
        let h2 = hash(&g2);
        let r = fresh.run_with_env(&["process", s(&g2)], &[("AUTO_CROP_FORCE_NO_AVX2", "1")]);
        assert_eq!(r.code(), 6);
        assert!(r.stderr().contains("AVX2"), "{}", r.stderr());
        assert_eq!(hash(&g2), h2);
        assert!(!fresh.home().exists(), "nothing at all was created");
    }
}

#[test]
fn a_cancel_stops_cleanly_and_exits_130() {
    let sb = Sandbox::new();
    let mut files = Vec::new();
    for i in 0..8 {
        files.push(good_photo(&sb.work().join(format!("p{i}.jpg")), i + 1));
    }
    let before: Vec<_> = files.iter().map(|f| hash(f)).collect();
    // The hook cancels exactly as Ctrl+C does (the same token), after the first finished item.
    let r = sb.run_with_env(
        &["process", "--json", "--jobs", "1", s(&sb.work())],
        &[
            ("AUTO_CROP_TEST_CANCEL_AFTER", "1"),
            ("AUTO_CROP_TEST_DELAY_MS", "50"),
        ],
    );
    assert_eq!(r.code(), 130, "{}", r.stderr());
    let doc = r.json();
    assert_valid_manifest(&doc);
    assert_eq!(doc["run"]["cancelled"], true);
    assert_eq!(doc["run"]["exit_name"], "cancelled");
    let saved = doc["summary"]["saved"].as_u64().unwrap();
    let skipped = doc["summary"]["skipped"].as_u64().unwrap();
    assert!(
        saved >= 1 && skipped >= 1 && saved + skipped == 8,
        "{saved} saved, {skipped} skipped"
    );
    assert!(items(&doc).iter().any(|i| i["code"] == "CANCELLED"));
    // Every file is either completely processed (with its backup) or byte identical: none is half written.
    for (f, h) in files.iter().zip(&before) {
        let now = hash(f);
        if now != *h {
            assert!(decode(&fs::read(f).unwrap()).is_ok(), "{f:?} decodes");
        }
    }
    assert!(no_temp_files(&sb.work()));
    // The journal is consistent: the finished files can be restored, byte for byte.
    let run_id = doc["run"]["id"].as_str().unwrap().to_owned();
    let rr = sb.run(&["restore", "--run", &run_id]);
    assert_eq!(rr.code(), 0, "{}", rr.stderr());
    let after: Vec<_> = files.iter().map(|f| hash(f)).collect();
    assert_eq!(after, before);
}

/// A real signal, where the platform has one for a child process (a Windows console event cannot
/// be sent without a console; there the hook above covers the same token).
#[cfg(unix)]
#[test]
fn sigint_stops_cleanly_and_exits_130() {
    let sb = Sandbox::new();
    for i in 0..10 {
        good_photo(&sb.work().join(format!("p{i}.jpg")), i + 1);
    }
    let mut c = sb.command();
    c.args(["process", "--json", "--jobs", "1", s(&sb.work())])
        .env("AUTO_CROP_TEST_DELAY_MS", "400")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = c.spawn().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let _ = std::process::Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status();
    let out = child.wait_with_output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(130),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_valid_manifest(&doc);
    assert_eq!(doc["run"]["cancelled"], true);
    assert!(no_temp_files(&sb.work()));
}

// ---------------------------------------------------------------- output modes of the report

#[test]
fn stdout_is_machine_output_and_stderr_is_for_people() {
    let sb = Sandbox::new();
    let a = good_photo(&sb.work().join("a.jpg"), 1);
    let none = no_document(&sb.work().join("none.jpg"));
    let out = sb.dir.path().join("out");
    // Default: nothing on stdout; held items and the summary on stderr.
    let r = sb.run(&["process", "--output", s(&out), s(&a), s(&none)]);
    assert_eq!(r.code(), 4);
    assert_eq!(r.stdout(), "");
    assert!(
        r.stderr().contains("held") && r.stderr().contains("DETECTION_FAILED"),
        "{}",
        r.stderr()
    );
    assert!(r.stderr().contains("1 saved"), "{}", r.stderr());
    // Verbose names the saved one too; quiet prints neither.
    let v = sb.run(&["process", "--verbose", "--suffix", "_v", s(&a)]);
    assert!(v.stderr().contains("saved"), "{}", v.stderr());
    let q = sb.run(&["process", "--quiet", "--suffix", "_q", s(&a), s(&none)]);
    assert_eq!(q.code(), 4);
    assert_eq!(q.stderr(), "");
    // A progress line only when asked for (a terminal turns it on by itself).
    let p = sb.run(&[
        "process",
        "--progress",
        "always",
        "--suffix",
        "_p",
        s(&a),
        s(&none),
    ]);
    assert!(p.stderr().contains("[1/2]"), "{}", p.stderr());
    let n = sb.run(&["process", "--suffix", "_n", s(&a), s(&none)]);
    assert!(
        !n.stderr().contains("[1/2]"),
        "no progress line off a terminal"
    );
}

#[test]
fn ndjson_streams_start_items_and_end() {
    let sb = Sandbox::new();
    let a = good_photo(&sb.work().join("a.jpg"), 1);
    let none = no_document(&sb.work().join("none.jpg"));
    let r = sb.run(&["process", "--ndjson", "--suffix", "_j", s(&a), s(&none)]);
    assert_eq!(r.code(), 4);
    let lines: Vec<Value> = r
        .stdout()
        .lines()
        .map(|l| serde_json::from_str(l).expect(l))
        .collect();
    assert_eq!(lines.first().unwrap()["t"], "start");
    assert_eq!(lines.last().unwrap()["t"], "end");
    assert_eq!(lines.last().unwrap()["exit_code"], 4);
    let item_lines: Vec<_> = lines.iter().filter(|l| l["t"] == "item").collect();
    assert_eq!(item_lines.len(), 2);
    assert!(lines.iter().all(|l| l["v"] == 1));
    // The item events are the manifest's items (schema checked through a wrapped document).
    let schema = manifest_schema();
    for it in item_lines {
        let mut errs = Vec::new();
        validate(&schema, &schema["$defs"]["item"], it, "item", &mut errs);
        assert!(errs.is_empty(), "{errs:?}");
    }
}

#[test]
fn the_manifest_file_is_written_atomically_and_validates() {
    let sb = Sandbox::new();
    let a = good_photo(&sb.work().join("a.jpg"), 1);
    let none = no_document(&sb.work().join("none.jpg"));
    let bytes = fs::read(&a).unwrap();
    fs::write(sb.work().join("bad.jpg"), &bytes[..bytes.len() / 4]).unwrap();
    fs::write(sb.work().join("x.gif"), b"GIF89a").unwrap();
    let m = sb.dir.path().join("reports/run.json");
    let r = sb.run(&[
        "process",
        "--quiet",
        "--manifest",
        s(&m),
        "--output",
        s(&sb.dir.path().join("out")),
        s(&a),
        s(&none),
        s(&sb.work().join("bad.jpg")),
        s(&sb.work().join("x.gif")),
    ]);
    assert_eq!(r.code(), 3, "{}", r.stderr());
    let doc: Value = serde_json::from_str(&fs::read_to_string(&m).unwrap()).unwrap();
    assert_valid_manifest(&doc);
    let statuses: std::collections::BTreeSet<_> = items(&doc)
        .iter()
        .map(|i| i["status"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        statuses.len(),
        4,
        "every status appears in this run: {statuses:?}"
    );
    assert_eq!(doc["run"]["exit_code"], 3);
    assert_eq!(doc["schema"], "auto-crop/run-manifest");
    assert!(no_temp_files(m.parent().unwrap()));
    // Indices run 0..n in manifest order (inputs that gave nothing first, then the files in the
    // order found), whatever the job count.
    let idx: Vec<u64> = items(&doc)
        .iter()
        .map(|i| i["index"].as_u64().unwrap())
        .collect();
    assert_eq!(idx, [0, 1, 2, 3]);
}

#[test]
fn the_job_count_never_changes_the_output() {
    let make = |sb: &Sandbox| {
        for i in 0..6 {
            good_photo(&sb.work().join(format!("p{i}.jpg")), i + 1);
        }
    };
    let (one, many) = (Sandbox::new(), Sandbox::new());
    make(&one);
    make(&many);
    assert_eq!(tree(&one.work()), tree(&many.work()));
    let r1 = one.run(&["process", "--quiet", "--jobs", "1", s(&one.work())]);
    // A tiny memory cap: every job is oversize and runs alone, which must still finish.
    let r4 = many.run(&[
        "process",
        "--quiet",
        "--jobs",
        "4",
        "--mem-limit",
        "64",
        s(&many.work()),
    ]);
    assert_eq!(
        (r1.code(), r4.code()),
        (0, 0),
        "{} {}",
        r1.stderr(),
        r4.stderr()
    );
    assert_eq!(
        tree(&one.work()),
        tree(&many.work()),
        "the outputs are identical for any job count"
    );
}

#[test]
fn unicode_and_long_paths_round_trip() {
    let sb = Sandbox::new();
    let dir = sb
        .work()
        .join("fotos \u{e9}t\u{e9}")
        .join("\u{65e5}\u{672c}\u{8a9e}");
    let a = good_photo(
        &dir.join("\u{c4}rger \u{65e5}\u{672c} \u{fc}n\u{ef}.jpg"),
        1,
    );
    let ha = hash(&a);
    let r = sb.run(&["process", "--json", s(&a)]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    let doc = r.json();
    assert_eq!(
        items(&doc)[0]["input"].as_str().unwrap(),
        s(&a),
        "the name survives the JSON"
    );
    let rr = sb.run(&["restore", s(&a)]);
    assert_eq!(rr.code(), 0, "{}", rr.stderr());
    assert_eq!(hash(&a), ha);
    // A path beyond the old 260-character limit.
    let mut deep = sb.work();
    for i in 0..12 {
        deep = deep.join(format!("level-{i:02}-with-a-reasonably-long-folder-name"));
    }
    assert!(deep.to_string_lossy().len() > 400);
    let long = good_photo(&deep.join("long.jpg"), 2);
    let hl = hash(&long);
    let r = sb.run(&["process", "--quiet", s(&long)]);
    assert_eq!(r.code(), 0, "{}", r.stderr());
    assert_ne!(hash(&long), hl);
    let rr = sb.run(&["restore", "--quiet", s(&long)]);
    assert_eq!(rr.code(), 0, "{}", rr.stderr());
    assert_eq!(hash(&long), hl);
}

#[test]
fn odd_argument_lists_never_crash_the_binary() {
    let sb = Sandbox::new();
    let a = good_photo(&sb.work().join("a.jpg"), 1);
    let ha = hash(&a);
    for args in [
        vec!["process", "--quality", "99999999999999999999", s(&a)],
        vec!["process", "--margin", "NaN", s(&a)],
        vec!["process", "--jobs", "-3", s(&a)],
        vec!["process", "--output", "", s(&a)],
        vec!["process", "--", "--weird-name.jpg"],
        vec!["process", "\u{202e}"],
        vec!["backups", "purge"],
        vec!["restore", "..", "zzzzzzzzzzzzzzzzzzzzzzzzzzzz"],
        vec!["render", s(&a)],
        vec!["--home"],
        vec!["-"],
    ] {
        let r = sb.run(&args);
        let code = r.code();
        assert!(
            matches!(code, 2 | 3 | 5 | 6),
            "{args:?} gave {code}: {}",
            r.stderr()
        );
        assert!(!r.stderr().contains("panicked"), "{args:?}: {}", r.stderr());
    }
    assert_eq!(hash(&a), ha, "no odd command line touched the file");
}
