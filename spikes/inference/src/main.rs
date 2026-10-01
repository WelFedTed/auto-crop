// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Inference spike harness (ROADMAP M0.32-M0.34).
//!
//! `infer <backend> <model.onnx> <input.f32> <out.f32> <threads> <runs>` loads the model, runs it
//! `runs` times with the raw little-endian f32 input (NCHW), writes the first output to `out.f32`
//! and prints one JSON line: backend, load time, median and p95 of the run times in ms (the first
//! run is a warm-up and is excluded), or the error text when the backend cannot run the model.
//! Exit code is 0 even on a backend error so the orchestrator can tabulate it.

use std::time::Instant;

fn read_f32(path: &str) -> Vec<f32> {
    let b = std::fs::read(path).expect("input file");
    b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

fn shape_for(len: usize) -> [usize; 4] {
    match len {
        n if n == 3 * 256 * 256 => [1, 3, 256, 256],
        n if n == 3 * 224 * 224 => [1, 3, 224, 224],
        n => panic!("unknown input size {n}"),
    }
}

/// Runs `f` once as warm-up and `runs` times timed; returns (load_ms is measured by the caller).
fn time_runs(runs: usize, mut f: impl FnMut() -> Vec<f32>) -> (Vec<f32>, Vec<f64>) {
    let mut out = f();
    let mut times = Vec::with_capacity(runs);
    for _ in 0..runs {
        let t = Instant::now();
        out = f();
        times.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    (out, times)
}

#[cfg(any(feature = "ort-b", feature = "ort-dyn"))]
fn run_ort(model: &str, input: &[f32], threads: usize, runs: usize) -> Result<(f64, Vec<f32>, Vec<f64>), String> {
    use ort::{session::Session, value::Tensor};
    let t = Instant::now();
    let mut session = Session::builder()
        .map_err(|e| e.to_string())?
        .with_intra_threads(threads)
        .map_err(|e| e.to_string())?
        .commit_from_file(model)
        .map_err(|e| e.to_string())?;
    let load = t.elapsed().as_secs_f64() * 1000.0;
    let shape = shape_for(input.len());
    let name = session.inputs()[0].name().to_string();
    let mut err = None;
    let (out, times) = time_runs(runs, || {
        let tensor = Tensor::from_array((shape, input.to_vec())).unwrap();
        match session.run(ort::inputs![name.as_str() => tensor]) {
            Ok(outputs) => outputs[0].try_extract_tensor::<f32>().unwrap().1.to_vec(),
            Err(e) => {
                err = Some(e.to_string());
                Vec::new()
            }
        }
    });
    match err {
        Some(e) => Err(e),
        None => Ok((load, out, times)),
    }
}

#[cfg(feature = "rten-b")]
fn run_rten(model: &str, input: &[f32], threads: usize, runs: usize) -> Result<(f64, Vec<f32>, Vec<f64>), String> {
    use rten::{RunOptions, ThreadPool};
    use rten_tensor::{AsView, NdTensor};
    use std::sync::Arc;
    let t = Instant::now();
    let m = rten::Model::load_file(model).map_err(|e| e.to_string())?;
    let load = t.elapsed().as_secs_f64() * 1000.0;
    let pool = Arc::new(ThreadPool::with_num_threads(threads));
    let opts = RunOptions::default().with_thread_pool(Some(pool));
    let shape = shape_for(input.len());
    let tensor = NdTensor::from_data(shape, input.to_vec());
    let mut err = None;
    let (out, times) = time_runs(runs, || match m.run_one(tensor.view().into(), Some(opts.clone())) {
        Ok(v) => {
            let t: rten_tensor::Tensor<f32> = v.try_into().unwrap();
            t.to_vec()
        }
        Err(e) => {
            err = Some(e.to_string());
            Vec::new()
        }
    });
    match err {
        Some(e) => Err(e),
        None => Ok((load, out, times)),
    }
}

#[cfg(feature = "tract-b")]
fn run_tract(model: &str, input: &[f32], threads: usize, runs: usize) -> Result<(f64, Vec<f32>, Vec<f64>), String> {
    use tract_onnx::prelude::*;
    use tract_linalg::multithread::{Executor, set_default_executor};
    if threads > 1 {
        set_default_executor(Executor::multithread(threads));
    }
    let shape = shape_for(input.len());
    let t = Instant::now();
    let plan = (|| -> TractResult<_> {
        tract_onnx::onnx()
            .model_for_path(model)?
            .with_input_fact(0, f32::fact(shape).into())?
            .into_optimized()?
            .into_runnable()
    })()
    .map_err(|e| format!("{e:#}"))?;
    let load = t.elapsed().as_secs_f64() * 1000.0;
    let tensor: Tensor = tract_ndarray::Array4::from_shape_vec(shape, input.to_vec()).unwrap().into();
    let mut err = None;
    let (out, times) = time_runs(runs, || match plan.run(tvec!(tensor.clone().into())) {
        Ok(o) => o[0].to_plain_array_view::<f32>().unwrap().iter().copied().collect(),
        Err(e) => {
            err = Some(format!("{e:#}"));
            Vec::new()
        }
    });
    match err {
        Some(e) => Err(e),
        None => Ok((load, out, times)),
    }
}

fn pct(sorted: &[f64], p: f64) -> f64 {
    let i = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[i]
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 7 {
        eprintln!("usage: infer <ort|rten|tract> <model.onnx> <input.f32> <out.f32> <threads> <runs>");
        std::process::exit(2);
    }
    let (backend, model, inp, outp) = (a[1].as_str(), a[2].as_str(), a[3].as_str(), a[4].as_str());
    let (threads, runs): (usize, usize) = (a[5].parse().unwrap(), a[6].parse().unwrap());
    let input = read_f32(inp);
    let res = match backend {
        #[cfg(any(feature = "ort-b", feature = "ort-dyn"))]
        "ort" => run_ort(model, &input, threads, runs),
        #[cfg(feature = "rten-b")]
        "rten" => run_rten(model, &input, threads, runs),
        #[cfg(feature = "tract-b")]
        "tract" => run_tract(model, &input, threads, runs),
        other => Err(format!("backend {other} not compiled in")),
    };
    match res {
        Ok((load, out, mut times)) => {
            let bytes: Vec<u8> = out.iter().flat_map(|f| f.to_le_bytes()).collect();
            std::fs::write(outp, bytes).unwrap();
            times.sort_by(|x, y| x.partial_cmp(y).unwrap());
            println!(
                "{{\"backend\":\"{backend}\",\"ok\":true,\"threads\":{threads},\"load_ms\":{load:.1},\"median_ms\":{:.2},\"p95_ms\":{:.2},\"min_ms\":{:.2},\"outputs\":{}}}",
                pct(&times, 0.5),
                pct(&times, 0.95),
                times[0],
                out.len()
            );
        }
        Err(e) => {
            let e = e.replace('\\', "/").replace('"', "'").replace('\n', " ");
            println!("{{\"backend\":\"{backend}\",\"ok\":false,\"threads\":{threads},\"error\":\"{}\"}}", &e[..e.len().min(400)]);
        }
    }
}
