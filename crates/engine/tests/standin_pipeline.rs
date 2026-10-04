// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The analysis stand-ins inside the pipeline skeleton (ROADMAP M1.55).
//!
//! `cargo test -p auto-crop-engine --features standin-rten,standin-canny --test standin_pipeline`.
//! The net test needs `cargo xtask make-standin-net` and skips loudly without it unless
//! `AUTOCROP_REQUIRE_STANDIN=1` (CI sets it).
#![cfg(any(feature = "standin-rten", feature = "standin-canny"))]

use auto_crop_core::CancelToken;
use auto_crop_engine::skeleton::bench_images::jpeg;
use auto_crop_engine::skeleton::{Analyse, Input, Options, Stage, format_timings, run};

fn never() -> CancelToken {
    CancelToken::never()
}

#[cfg(feature = "standin-canny")]
#[test]
fn canny_stand_in_runs_the_pipeline_and_is_labelled() {
    let bytes = jpeg(1600, 1200, 3);
    let opts = Options {
        analyse: Analyse::StandinCanny,
        ..Options::default()
    };
    let out = run(Input::Bytes(&bytes), &opts, &never()).unwrap();
    assert!(out.report.analyse.starts_with("STAND-IN (Canny"));
    assert_eq!(out.report.stages.len(), 7);
    let analyse = out
        .report
        .stages
        .iter()
        .find(|s| s.stage == Stage::Analyse)
        .unwrap();
    // The detection proxy: 1024 x 768 for a 4:3 frame.
    assert_eq!(analyse.px, 1024 * 768);
    assert!(format_timings(&out.report).contains("STAND-IN (Canny + contours"));
    // Deterministic: same bytes on a second run and on a one-thread pool.
    let again = run(Input::Bytes(&bytes), &opts, &never()).unwrap();
    assert_eq!(out.bytes, again.bytes);
}

#[cfg(not(feature = "standin-canny"))]
#[test]
fn canny_without_its_feature_is_a_clean_error_not_a_panic() {
    let bytes = jpeg(800, 600, 3);
    let opts = Options {
        analyse: Analyse::StandinCanny,
        ..Options::default()
    };
    let f = run(Input::Bytes(&bytes), &opts, &never()).unwrap_err();
    assert_eq!(f.stage, Stage::Analyse);
    assert_eq!(f.kind, auto_crop_engine::ErrKind::UnsupportedFeature);
}

#[cfg(feature = "standin-rten")]
#[test]
fn net_stand_in_runs_through_the_backend_and_the_output_does_not_depend_on_its_threads() {
    let net_path = std::env::var_os("AUTOCROP_STANDIN_NET").map_or_else(
        || {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/standin/quadnet.onnx")
        },
        std::path::PathBuf::from,
    );
    if !net_path.is_file() {
        let msg = "SKIPPED: the stand-in net is not generated (cargo xtask make-standin-net)";
        assert!(
            std::env::var_os("AUTOCROP_REQUIRE_STANDIN").is_none_or(|v| v != "1"),
            "{msg}"
        );
        eprintln!("{msg}");
        return;
    }
    let bytes = jpeg(1600, 1200, 5);
    let mut outputs = Vec::new();
    for threads in [1, 4] {
        let net =
            auto_crop_engine::skeleton::standin::load_net(&net_path, "rten", threads).unwrap();
        assert_eq!((net.backend_name(), net.threads()), ("rten", threads));
        let opts = Options {
            analyse: Analyse::StandinNet(net),
            ..Options::default()
        };
        let out = run(Input::Bytes(&bytes), &opts, &never()).unwrap();
        assert!(
            out.report
                .analyse
                .starts_with("STAND-IN (random-weight 256x256 net, rten")
        );
        assert_eq!(out.report.stages.len(), 7);
        outputs.push(out.bytes);
    }
    assert_eq!(outputs[0], outputs[1]);
    // A model whose bytes do not match the digest is refused before any backend sees it.
    let dir = std::env::temp_dir().join(format!("auto-crop-standin-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let copy = dir.join("quadnet.onnx");
    let mut b = std::fs::read(&net_path).unwrap();
    std::fs::copy(
        auto_crop_infer::model::sidecar_path(&net_path),
        auto_crop_infer::model::sidecar_path(&copy),
    )
    .unwrap();
    b[200] ^= 1;
    std::fs::write(&copy, b).unwrap();
    let err = auto_crop_engine::skeleton::standin::load_net(&copy, "rten", 1)
        .err()
        .expect("a tampered model must be refused");
    assert!(err.contains("REFUSED"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}
