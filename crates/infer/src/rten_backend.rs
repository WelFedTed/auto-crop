// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The pure-Rust fallback backend (`rten`, ADR-0007): same models, same hashes, no runtime library.

use crate::{InferError, InferenceBackend, Output, VerifiedModel, check_input};
use rten::{Model, RunOptions, ThreadPool};
use rten_tensor::{AsView, Layout, NdTensorView, Tensor};
use std::sync::Arc;

pub struct RtenBackend {
    model: Model,
    opts: RunOptions,
    threads: usize,
}

impl RtenBackend {
    /// Builds a model from the verified bytes with `threads` worker threads (at least 1).
    pub fn new(model: &VerifiedModel, threads: usize) -> Result<Self, InferError> {
        let threads = threads.max(1);
        let m = Model::load(model.bytes().to_vec()).map_err(|e| InferError::Load {
            backend: "rten",
            message: e.to_string(),
        })?;
        let pool = Arc::new(ThreadPool::with_num_threads(threads));
        Ok(Self {
            model: m,
            opts: RunOptions::default().with_thread_pool(Some(pool)),
            threads,
        })
    }
}

impl InferenceBackend for RtenBackend {
    fn name(&self) -> &'static str {
        "rten"
    }

    fn threads(&self) -> usize {
        self.threads
    }

    fn run(&mut self, input: &[f32], shape: [usize; 4]) -> Result<Output, InferError> {
        check_input(input, shape)?;
        let run_err = |message: String| InferError::Run {
            backend: "rten",
            message,
        };
        let tensor = NdTensorView::from_data(shape, input);
        let value = self
            .model
            .run_one(tensor.as_dyn().into(), Some(self.opts.clone()))
            .map_err(|e| run_err(e.to_string()))?;
        let out: Tensor<f32> = value
            .try_into()
            .map_err(|_| run_err("the first output is not an f32 tensor".to_owned()))?;
        Ok(Output {
            shape: out.shape().to_vec(),
            data: out.to_vec(),
        })
    }
}
