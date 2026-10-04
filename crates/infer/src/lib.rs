// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Neural-net inference behind one trait (ADR-0007, ROADMAP M1.55).
//!
//! The decision: ONNX Runtime through the `ort` crate with `load-dynamic`, CPU only, with `rten`
//! (pure Rust) as the fallback behind the same trait. This crate is where that lives, and it is the
//! only crate that may name `ort` or `rten` (`cargo xtask check-deps`).
//!
//! * [`InferenceBackend`] is the trait; [`OrtBackend`] and [`RtenBackend`] implement it behind the
//!   cargo features `ort` and `rten`. **Neither feature is on by default**, so the default build
//!   has no native or ONNX Runtime dependency at all.
//! * [`model::VerifiedModel`] hashes the model bytes (SHA-256) and refuses them when the pin does
//!   not match; both backends only accept a verified model and build the session from memory
//!   (`commit_from_memory`), so the bytes that were hashed are the bytes that run.
//! * [`runtime`] finds and loads the ONNX Runtime library: an **absolute path** (the
//!   `AUTOCROP_ORT_DYLIB` variable for development, or next to the executable), whose SHA-256 must
//!   equal the pin of the library inside the pinned archive of `native-deps.toml`. A system
//!   `onnxruntime.dll` (Windows ships one), a library of another version or a bare name is never
//!   loaded: nothing is searched for.
//!
//! The nets exercised so far are random-weight STAND-INS (`spikes/inference/gen_models.py`): they
//! have the shape and operator mix of a MobileNetV3-class corner net, so latency, size and backend
//! agreement mean something and accuracy means nothing. No M4 claim is made here.

pub mod model;
pub mod runtime;

#[cfg(feature = "ort")]
mod ort_backend;
#[cfg(feature = "rten")]
mod rten_backend;

pub use model::VerifiedModel;
#[cfg(feature = "ort")]
pub use ort_backend::OrtBackend;
#[cfg(feature = "rten")]
pub use rten_backend::RtenBackend;

/// Everything that can go wrong loading or running a net.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum InferError {
    /// The model bytes do not hash to the pinned value.
    #[error("model REFUSED: sha256 {got} does not match the pin {expected}")]
    ModelHashMismatch { expected: String, got: String },
    /// A pin that is not 64 hex characters.
    #[error("model pin is not a SHA-256 (64 hex characters): `{0}`")]
    BadPin(String),
    /// The ONNX Runtime library was refused or could not be loaded.
    #[error("{0}")]
    Runtime(#[from] runtime::RuntimeError),
    /// The backend could not build a session from the model.
    #[error("{backend}: cannot load the model: {message}")]
    Load {
        backend: &'static str,
        message: String,
    },
    /// The input does not match the shape the caller declared.
    #[error("input has {got} values, shape {shape:?} needs {want}")]
    InputSize {
        got: usize,
        want: usize,
        shape: [usize; 4],
    },
    /// The session failed while running.
    #[error("{backend}: run failed: {message}")]
    Run {
        backend: &'static str,
        message: String,
    },
    /// The backend was not compiled in.
    #[error("backend `{0}` is not compiled into this build (cargo feature `{0}`)")]
    NotCompiled(&'static str),
}

/// The first output tensor of a run: its shape and the row-major data.
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub shape: Vec<usize>,
    pub data: Vec<f32>,
}

/// One loaded net on one backend. `run` takes `&mut self` because ONNX Runtime sessions do.
///
/// The input is a single NCHW `f32` tensor and the output the first output of the graph, which is
/// all the stand-ins and the planned corner and orientation nets need.
pub trait InferenceBackend: Send {
    /// `"ort"` or `"rten"`: what the app reports as the backend that ran (ADR-0007).
    fn name(&self) -> &'static str;

    /// Intra-op threads this backend was built with.
    fn threads(&self) -> usize;

    /// Runs the net once on `input` (NCHW, `shape` product values).
    fn run(&mut self, input: &[f32], shape: [usize; 4]) -> Result<Output, InferError>;
}

#[cfg_attr(not(any(feature = "ort", feature = "rten")), allow(dead_code))]
pub(crate) fn check_input(input: &[f32], shape: [usize; 4]) -> Result<(), InferError> {
    let want: usize = shape.iter().product();
    if input.len() == want {
        Ok(())
    } else {
        Err(InferError::InputSize {
            got: input.len(),
            want,
            shape,
        })
    }
}

/// Builds a backend by name: `"ort"` (loads the runtime from the located absolute path) or
/// `"rten"`.
pub fn make_backend(
    name: &str,
    model: &VerifiedModel,
    threads: usize,
) -> Result<Box<dyn InferenceBackend>, InferError> {
    make_backend_at(name, model, threads, None)
}

/// [`make_backend`] with an explicit ONNX Runtime library path (absolute; verified like any
/// other) instead of `AUTOCROP_ORT_DYLIB` or the executable's directory. `rten` ignores it.
pub fn make_backend_at(
    name: &str,
    model: &VerifiedModel,
    threads: usize,
    runtime_path: Option<&std::path::Path>,
) -> Result<Box<dyn InferenceBackend>, InferError> {
    // Which of these a build uses depends on the cargo features.
    let _ = (model, threads, runtime_path);
    match name {
        #[cfg(feature = "ort")]
        "ort" => {
            let lib = match runtime_path {
                Some(p) => p.to_path_buf(),
                None => runtime::locate_for_current_process()?,
            };
            Ok(Box::new(OrtBackend::new(&lib, model, threads)?))
        }
        #[cfg(feature = "rten")]
        "rten" => Ok(Box::new(RtenBackend::new(model, threads)?)),
        #[cfg(not(feature = "ort"))]
        "ort" => Err(InferError::NotCompiled("ort")),
        #[cfg(not(feature = "rten"))]
        "rten" => Err(InferError::NotCompiled("rten")),
        other => Err(InferError::Load {
            backend: "make_backend",
            message: format!("unknown backend `{other}` (ort or rten)"),
        }),
    }
}

/// The backends compiled into this build, in order of preference (ADR-0007: ONNX Runtime first,
/// rten as the fallback).
pub fn compiled_backends() -> Vec<&'static str> {
    let mut v = Vec::new();
    if cfg!(feature = "ort") {
        v.push("ort");
    }
    if cfg!(feature = "rten") {
        v.push("rten");
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_build_has_no_backend() {
        if !cfg!(any(feature = "ort", feature = "rten")) {
            assert!(compiled_backends().is_empty());
            let m = VerifiedModel::from_bytes_unpinned(vec![1, 2, 3]);
            assert_eq!(
                make_backend("ort", &m, 1).err(),
                Some(InferError::NotCompiled("ort"))
            );
        }
    }

    #[test]
    fn input_size_is_checked() {
        assert!(check_input(&[0.0; 12], [1, 3, 2, 2]).is_ok());
        assert_eq!(
            check_input(&[0.0; 11], [1, 3, 2, 2]),
            Err(InferError::InputSize {
                got: 11,
                want: 12,
                shape: [1, 3, 2, 2]
            })
        );
    }
}
