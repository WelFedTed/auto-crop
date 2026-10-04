// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The ONNX Runtime backend (`ort` with `load-dynamic`, ADR-0007).

use crate::{InferError, InferenceBackend, Output, VerifiedModel, check_input, runtime};
use ort::session::Session;
use ort::value::TensorRef;
use std::path::Path;

/// A session on the pinned ONNX Runtime, CPU provider only.
pub struct OrtBackend {
    session: Session,
    input_name: String,
    threads: usize,
}

impl OrtBackend {
    /// Loads the runtime at `runtime_path` (absolute; its SHA-256 must be the pinned library's, see
    /// [`runtime`]) and builds a session from the verified model bytes in memory, with `threads`
    /// intra-op threads (at least 1).
    pub fn new(
        runtime_path: &Path,
        model: &VerifiedModel,
        threads: usize,
    ) -> Result<Self, InferError> {
        runtime::load(runtime_path)?;
        let threads = threads.max(1);
        fn load<T>(e: ort::Error<T>) -> InferError {
            InferError::Load {
                backend: "ort",
                message: e.to_string(),
            }
        }
        let session = Session::builder()
            .map_err(load)?
            .with_intra_threads(threads)
            .map_err(load)?
            .commit_from_memory(model.bytes())
            .map_err(load)?;
        let input_name = session
            .inputs()
            .first()
            .map(|i| i.name().to_owned())
            .ok_or_else(|| InferError::Load {
                backend: "ort",
                message: "the model has no input".to_owned(),
            })?;
        Ok(Self {
            session,
            input_name,
            threads,
        })
    }
}

impl InferenceBackend for OrtBackend {
    fn name(&self) -> &'static str {
        "ort"
    }

    fn threads(&self) -> usize {
        self.threads
    }

    fn run(&mut self, input: &[f32], shape: [usize; 4]) -> Result<Output, InferError> {
        check_input(input, shape)?;
        let run_err = |e: ort::Error| InferError::Run {
            backend: "ort",
            message: e.to_string(),
        };
        let tensor = TensorRef::from_array_view((shape, input)).map_err(run_err)?;
        let outputs = self
            .session
            .run(ort::inputs![self.input_name.as_str() => tensor])
            .map_err(run_err)?;
        let (out_shape, data) = outputs[0].try_extract_tensor::<f32>().map_err(run_err)?;
        Ok(Output {
            shape: out_shape.iter().map(|&d| d.max(0) as usize).collect(),
            data: data.to_vec(),
        })
    }
}
