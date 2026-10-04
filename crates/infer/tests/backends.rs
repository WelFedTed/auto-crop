// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The stand-in net through both backends (ROADMAP M1.55, ADR-0007).
//!
//! Needs the generated net (`cargo xtask make-standin-net`) and, for `ort`, the pinned runtime
//! (`cargo xtask fetch-ort`). Without them a test skips loudly; with `AUTOCROP_REQUIRE_STANDIN=1`
//! (the CI workflow sets it) a missing piece is a failure, so CI cannot pass by skipping.
//! Run: `cargo test -p auto-crop-infer --features ort,rten --test backends -- --nocapture`.
#![cfg(any(feature = "ort", feature = "rten"))]

#[cfg(feature = "ort")]
use auto_crop_infer::runtime;
use auto_crop_infer::{InferError, InferenceBackend, VerifiedModel};
use std::path::{Path, PathBuf};

const SHAPE: [usize; 4] = [1, 3, 256, 256];
/// ADR-0007 measured 5e-7 (fp32); the bar is 1e-3.
#[cfg(all(feature = "ort", feature = "rten"))]
const AGREE: f32 = 1e-3;

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn required() -> bool {
    std::env::var_os("AUTOCROP_REQUIRE_STANDIN").is_some_and(|v| v == "1")
}

/// `Some(reason)` to skip, a panic when the piece is required and missing.
fn missing(what: &str, hint: &str) -> Option<String> {
    let msg = format!("SKIPPED: {what} not found ({hint})");
    assert!(!required(), "{msg}");
    eprintln!("{msg}");
    Some(msg)
}

fn model() -> Result<VerifiedModel, String> {
    let path = std::env::var_os("AUTOCROP_STANDIN_NET").map_or_else(
        || workspace().join("target/standin/quadnet.onnx"),
        PathBuf::from,
    );
    if !path.is_file() {
        return Err(missing("the stand-in net", "cargo xtask make-standin-net").unwrap());
    }
    VerifiedModel::from_file_with_sidecar(&path).map_err(|e| e.to_string())
}

/// Deterministic pseudo-random input in [0, 1).
fn input() -> Vec<f32> {
    let mut s: u64 = 0x2545_F491_4F6C_DD1D;
    (0..SHAPE.iter().product::<usize>())
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 40) as f32 / (1u64 << 24) as f32
        })
        .collect()
}

fn max_abs_diff(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f32::max)
}

#[cfg(feature = "rten")]
fn rten_output(model: &VerifiedModel, threads: usize) -> auto_crop_infer::Output {
    let mut b = auto_crop_infer::RtenBackend::new(model, threads).expect("rten loads the net");
    assert_eq!((b.name(), b.threads()), ("rten", threads));
    b.run(&input(), SHAPE).expect("rten runs")
}

#[cfg(feature = "rten")]
#[test]
fn rten_runs_the_stand_in_net_and_is_thread_count_independent() {
    let Ok(model) = model() else { return };
    let one = rten_output(&model, 1);
    let four = rten_output(&model, 4);
    assert_eq!(one.shape, vec![1, 4, 64, 64]);
    assert!(
        one.data
            .iter()
            .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
    );
    let d = max_abs_diff(&one.data, &four.data);
    eprintln!("rten 1 vs 4 threads: max abs diff {d:e}");
    assert!(d < 1e-5, "{d}");
}

#[cfg(feature = "rten")]
#[test]
fn rten_refuses_a_wrong_input_size() {
    let Ok(model) = model() else { return };
    let mut b = auto_crop_infer::RtenBackend::new(&model, 1).unwrap();
    assert!(matches!(
        b.run(&[0.0; 10], SHAPE),
        Err(InferError::InputSize { got: 10, .. })
    ));
}

#[cfg(feature = "ort")]
fn runtime_path() -> Option<PathBuf> {
    if let Some(v) = std::env::var_os(runtime::ENV_VAR).filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(v));
    }
    let pin = runtime::pin_for_host()?;
    let p = workspace()
        .join("target/native/prefix/lib")
        .join(pin.file_name);
    // Absolute even when the workspace path has `..` segments.
    p.is_file()
        .then(|| std::fs::canonicalize(p).ok())
        .flatten()
        .map(|p| {
            let s = p.to_string_lossy().into_owned();
            PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s))
        })
}

/// One test, in order, because the ONNX Runtime is loaded once per process: the refusals come
/// first (nothing is loaded by them), then the load, then everything that needs the library.
#[cfg(feature = "ort")]
#[test]
fn ort_loader_refuses_wrong_runtimes_and_the_net_agrees_with_rten() {
    let Some(rt) = runtime_path() else {
        missing("the pinned ONNX Runtime", "cargo xtask fetch-ort");
        return;
    };
    let tmp = std::env::temp_dir().join(format!("auto-crop-infer-ort-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();

    // 1. A bare name or a relative path is never loaded, whatever the working directory holds.
    for bare in [
        "onnxruntime.dll",
        "libonnxruntime.so",
        "libonnxruntime.dylib",
        "./x/onnxruntime.dll",
    ] {
        assert!(
            matches!(
                runtime::load(Path::new(bare)),
                Err(runtime::RuntimeError::NotAbsolute(_))
            ),
            "{bare}"
        );
    }
    // 2. A file that is not the pinned library is refused by its hash, before any load: the
    //    pinned library with one byte changed stands for "another build or a planted copy".
    let mut bytes = std::fs::read(&rt).unwrap();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0xFF;
    let planted = tmp.join(rt.file_name().unwrap());
    std::fs::write(&planted, &bytes).unwrap();
    match runtime::load(&planted) {
        Err(runtime::RuntimeError::WrongRuntime { expected, got, .. }) => assert_ne!(expected, got),
        other => panic!("a modified runtime must be refused: {other:?}"),
    }
    // 3. The system library of another version (Windows 11 ships `onnxruntime.dll`) is refused too.
    let system = Path::new(r"C:\Windows\System32\onnxruntime.dll");
    if cfg!(windows) && system.is_file() {
        match runtime::load(system) {
            Err(runtime::RuntimeError::WrongRuntime { path, .. }) => {
                eprintln!("system runtime refused: {path}")
            }
            other => panic!("the system onnxruntime.dll must be refused: {other:?}"),
        }
    } else {
        eprintln!("no system onnxruntime to refuse on this host (the planted copy covers it)");
    }
    // 4. The loader never reads ORT_DYLIB_PATH: pointing it at the planted copy changes nothing.
    // (Not set here: ort reads it only when no library was loaded before its first API call, and
    // `runtime::load` always loads first.)

    // 5. The real thing loads, twice is fine, and a second library is refused as a second load.
    let loaded = runtime::load(&rt).expect("the pinned runtime loads");
    assert_eq!(runtime::load(&rt).unwrap(), loaded);
    let copy = tmp.join("copy-of-the-pinned-runtime.bin");
    std::fs::copy(&rt, &copy).unwrap();
    assert!(matches!(
        runtime::load(&copy),
        Err(runtime::RuntimeError::AlreadyLoaded { .. })
    ));

    let Ok(model) = model() else { return };
    // 6. A model whose bytes changed after pinning never reaches a backend.
    let mut tampered = model.bytes().to_vec();
    tampered[100] ^= 1;
    assert!(matches!(
        VerifiedModel::from_bytes_pinned(tampered, model.sha256()),
        Err(InferError::ModelHashMismatch { .. })
    ));

    let mut one = auto_crop_infer::OrtBackend::new(&rt, &model, 1).expect("ort loads the net");
    let mut four = auto_crop_infer::OrtBackend::new(&rt, &model, 4).unwrap();
    assert_eq!((one.name(), four.threads()), ("ort", 4));
    let (a, b) = (
        one.run(&input(), SHAPE).unwrap(),
        four.run(&input(), SHAPE).unwrap(),
    );
    assert_eq!(a.shape, vec![1, 4, 64, 64]);
    assert!(
        a.data
            .iter()
            .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
    );
    let d14 = max_abs_diff(&a.data, &b.data);
    eprintln!("ort 1 vs 4 threads: max abs diff {d14:e}");
    assert!(d14 < 1e-5, "{d14}");
    assert!(matches!(
        one.run(&[0.0; 10], SHAPE),
        Err(InferError::InputSize { .. })
    ));

    #[cfg(feature = "rten")]
    {
        let r = rten_output(&model, 4);
        assert_eq!(r.shape, a.shape);
        let d = max_abs_diff(&a.data, &r.data);
        eprintln!("ort vs rten, fp32, max abs diff {d:e} (bar {AGREE:e}, ADR-0007 measured 5e-7)");
        assert!(d <= AGREE, "ort and rten disagree by {d}");
    }
    let _ = std::fs::remove_dir_all(&tmp);
}
