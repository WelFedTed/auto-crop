// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `auto-crop dev-pipeline` end to end through the built binary (ROADMAP M1.54).

use auto_crop_codecs::{Format, encode};
use auto_crop_imgproc::synth::{PaperKind, Scene, render_scene};
use std::path::Path;
use std::process::{Command, Output};

fn write_photo(dir: &Path) -> std::path::PathBuf {
    let scene = Scene {
        width: 1200,
        height: 900,
        background: [120, 100, 80],
        paper: [244, 242, 236],
        ink: [60, 64, 76],
        kind: PaperKind::Document,
        corners: [(0.18, 0.10), (0.82, 0.14), (0.80, 0.92), (0.20, 0.88)],
        seed: 5,
        noise: 3.0,
        blur_radius: 1,
        shadow: true,
    };
    let path = dir.join("photo.jpg");
    std::fs::write(
        &path,
        encode(&render_scene(&scene), Format::Jpeg, 90, None).unwrap(),
    )
    .unwrap();
    path
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_auto-crop"))
        .args(args)
        .output()
        .expect("the binary starts")
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

#[test]
fn timings_print_every_stage_and_the_output_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let photo = write_photo(dir.path());
    let out = dir.path().join("out.jpg");
    let r = run(&[
        "dev-pipeline",
        photo.to_str().unwrap(),
        "--timings",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(r.status.success(), "{}", text(&r.stderr));
    let s = text(&r.stdout);
    for stage in [
        "read_probe",
        "decode",
        "proxy",
        "analyse",
        "rectify",
        "enhance",
        "encode",
    ] {
        assert!(s.contains(stage), "{stage} missing in:\n{s}");
    }
    assert!(s.contains("STAND-IN") && s.contains("PROVISIONAL"), "{s}");
    let jpeg = std::fs::read(&out).unwrap();
    assert_eq!(&jpeg[..2], [0xFF, 0xD8]);
}

#[test]
fn json_is_one_parsable_line_with_seven_stages_and_trace_goes_to_stderr() {
    let dir = tempfile::tempdir().unwrap();
    let photo = write_photo(dir.path());
    let r = run(&[
        "dev-pipeline",
        photo.to_str().unwrap(),
        "--json",
        "--trace",
        "--threads",
        "2",
    ]);
    assert!(r.status.success(), "{}", text(&r.stderr));
    let stdout = text(&r.stdout);
    assert_eq!(stdout.trim().lines().count(), 1);
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    let stages: Vec<&str> = v["stages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["stage"].as_str().unwrap())
        .collect();
    assert_eq!(
        stages,
        [
            "read_probe",
            "decode",
            "proxy",
            "analyse",
            "rectify",
            "enhance",
            "encode"
        ]
    );
    assert_eq!(v["quad_found"], true);
    // Tracing goes to stderr, never into the machine-readable stdout.
    assert_eq!(text(&r.stderr).matches("stage_done").count(), 7);
}

#[test]
fn threads_do_not_change_the_output_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let photo = write_photo(dir.path());
    let mut files = Vec::new();
    for n in ["1", "8"] {
        let out = dir.path().join(format!("o{n}.jpg"));
        let r = run(&[
            "dev-pipeline",
            photo.to_str().unwrap(),
            "--threads",
            n,
            "--out",
            out.to_str().unwrap(),
        ]);
        assert!(r.status.success(), "{}", text(&r.stderr));
        files.push(std::fs::read(out).unwrap());
    }
    assert_eq!(files[0], files[1]);
}

#[test]
fn errors_exit_non_zero_with_the_code_and_the_stage() {
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("bad.jpg");
    std::fs::write(&bad, b"not an image").unwrap();
    let r = run(&["dev-pipeline", bad.to_str().unwrap()]);
    assert!(!r.status.success());
    let e = text(&r.stderr);
    assert!(
        e.contains("read_probe") && e.contains("UnsupportedFormat"),
        "{e}"
    );

    let missing = dir.path().join("missing.jpg");
    let r = run(&["dev-pipeline", missing.to_str().unwrap()]);
    assert!(!r.status.success());
    assert!(text(&r.stderr).contains("Unreadable"));

    let r = run(&["dev-pipeline"]);
    assert!(!r.status.success());
    assert!(text(&r.stderr).contains("no input file"));
}
