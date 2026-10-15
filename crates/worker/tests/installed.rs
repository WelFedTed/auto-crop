// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The whole path an application takes: the pool installed as the remote decoder of
//! `auto-crop-codecs` (`AUTOCROP_DECODE=worker`), then the ordinary `decode_with`, `decode_scaled`
//! and `decode_flattened_with` entry points, and the hostile-file corpus through the worker (the
//! HEIF bombs, truncations and oversize headers included when the build has libheif). The parent
//! must never crash, every outcome is a typed result, and a decoder bug that kills a worker is a
//! test failure (it is also a finding: the sandbox contained it, but the bug is real).

use auto_crop_codecs::hostile::{self, Expect};
use auto_crop_codecs::{CodecError, DecodeLimits, fixtures};
use auto_crop_sandbox::{InstallOptions, Mode};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

fn install() -> &'static auto_crop_sandbox::PoolDecoder {
    static D: OnceLock<std::sync::Arc<auto_crop_sandbox::PoolDecoder>> = OnceLock::new();
    D.get_or_init(|| {
        auto_crop_sandbox::install(InstallOptions {
            exe: PathBuf::from(env!("CARGO_BIN_EXE_auto-crop-worker")),
            plugin_dir: None,
            inflight_cap: None,
            mode: Some(Mode::Worker),
        })
    })
}

#[test]
fn the_ordinary_entry_points_go_through_the_worker_and_agree_with_in_process() {
    let d = install();
    let limits = DecodeLimits::default();
    let png = fixtures::png_with_exif(50, 30, 6);
    let want = auto_crop_codecs::decode_in_process(&png, &limits, true, false).unwrap();
    let got = auto_crop_codecs::decode_with(&png, &limits).unwrap();
    assert_eq!(got.raster.data, want.raster.data);
    assert_eq!((got.raster.width, got.raster.height), (30, 50));
    let flat =
        auto_crop_codecs::decode_flattened_with(&fixtures::png_rgba(20, 20), &limits).unwrap();
    assert!(flat.notices.is_empty() || flat.notices.contains(&"alpha.flattened"));
    let scaled = auto_crop_codecs::decode_scaled(
        &fixtures::jpeg_baseline(200, 100),
        auto_crop_core::ports::Want::Scaled { min_edge: 64 },
        &limits,
    )
    .unwrap();
    assert!(scaled.decoded.raster.width >= 25);
    let st = d.pool().unwrap().status();
    assert!(st.jobs >= 3, "{st:?}");
    assert_eq!(st.crashes, 0);
}

#[test]
fn the_parent_checks_run_before_the_worker_is_asked_and_after_it_answers() {
    let d = install();
    let before = d.pool().unwrap().status().jobs;
    let limits = DecodeLimits::default();
    // refused from the header alone: the worker is never involved
    let r = auto_crop_codecs::decode_with(&hostile::png_claiming(60_000, 60_000), &limits);
    assert!(matches!(r, Err(CodecError::TooLarge(_))), "{r:?}");
    assert_eq!(d.pool().unwrap().status().jobs, before);
}

#[test]
fn the_hostile_corpus_through_the_worker_never_hurts_the_parent() {
    let d = install();
    let limits = DecodeLimits::default();
    let corpus = hostile::corpus();
    let mut bad = Vec::new();
    let mut slowest = (Duration::ZERO, String::new());
    for h in &corpus {
        let t0 = Instant::now();
        let r = auto_crop_codecs::decode_with(&h.bytes, &limits);
        let took = t0.elapsed();
        if took > slowest.0 {
            slowest = (took, h.name.clone());
        }
        match (&r, &h.expect) {
            (Ok(_), Expect::Reject) => bad.push(format!("{}: decoded but must be refused", h.name)),
            (Err(CodecError::WorkerCrashed(m)), _) => {
                bad.push(format!("{}: the worker died ({m})", h.name));
            }
            (Err(CodecError::MemoryLimit | CodecError::DecodeTimeout), Expect::Bounded { .. }) => {
                bad.push(format!("{}: {:?} (not bounded)", h.name, r.as_ref().err()));
            }
            (Err(CodecError::InternalPanic(m)), _) => {
                bad.push(format!("{}: panic inside the decoder ({m})", h.name));
            }
            _ => {}
        }
        if took > Duration::from_secs(60) {
            bad.push(format!("{}: took {took:?}", h.name));
        }
    }
    let st = d.pool().unwrap().status();
    println!(
        "HOSTILE-WORKER cases={} through-worker-jobs={} crashes={} timeouts={} slowest={:?} ({})",
        corpus.len(),
        st.jobs,
        st.crashes,
        st.timeouts,
        slowest.0,
        slowest.1
    );
    assert!(
        bad.is_empty(),
        "{} problem(s):\n{}",
        bad.len(),
        bad.join("\n")
    );
}

#[cfg(feature = "heif")]
#[test]
fn mutated_and_truncated_heif_files_are_typed_errors_through_the_worker() {
    let d = install();
    let limits = DecodeLimits::default();
    let mut tried = 0;
    let mut crashes = Vec::new();
    for (name, bytes) in fixtures::heif_real_files() {
        // truncations at many cut points and single-byte mutations spread over the file
        let n = bytes.len();
        let mut inputs: Vec<(String, Vec<u8>)> = Vec::new();
        for cut in (1..n).step_by((n / 40).max(1)) {
            inputs.push((format!("{name}-cut{cut}"), bytes[..cut].to_vec()));
        }
        for i in (0..n).step_by((n / 60).max(1)) {
            let mut b = bytes.to_vec();
            b[i] ^= 0xA5;
            inputs.push((format!("{name}-flip{i}"), b));
        }
        for (label, b) in inputs {
            tried += 1;
            match auto_crop_codecs::decode_with(&b, &limits) {
                Err(CodecError::WorkerCrashed(m)) => crashes.push(format!("{label}: {m}")),
                Err(CodecError::InternalPanic(m)) => crashes.push(format!("{label}: panic {m}")),
                _ => {}
            }
        }
    }
    let st = d.pool().unwrap().status();
    println!(
        "HEIF-MUTATIONS tried={tried} worker-jobs={} crashes={} libheif={:?} level={:?}",
        st.jobs,
        crashes.len(),
        st.libheif,
        st.level
    );
    assert!(
        crashes.is_empty(),
        "{} crash(es):\n{}",
        crashes.len(),
        crashes.join("\n")
    );
}

#[test]
fn the_fuzz_regression_files_through_the_worker() {
    install();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/regressions");
    let limits = DecodeLimits::default();
    let mut n = 0;
    for dir in std::fs::read_dir(&root).unwrap().flatten() {
        if !dir.path().is_dir() {
            continue;
        }
        for f in std::fs::read_dir(dir.path()).unwrap().flatten() {
            let Ok(bytes) = std::fs::read(f.path()) else {
                continue;
            };
            n += 1;
            let r = auto_crop_codecs::decode_with(&bytes, &limits);
            assert!(
                !matches!(
                    r,
                    Err(CodecError::WorkerCrashed(_) | CodecError::InternalPanic(_))
                ),
                "{}: {r:?}",
                f.path().display()
            );
        }
    }
    assert!(n >= 5, "found {n} regression files");
}
