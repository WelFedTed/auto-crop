// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Backend parity and timing of one exported net (learned-detector study, ADR 0010).
//!
//! ```text
//! cargo run --release -p auto-crop-infer --features ort,rten --example net_parity -- \
//!     MODEL.onnx INPUTS.f32 N SIZE OUT_PREFIX [THREADS=4] [RUNS=3]
//! ```
//!
//! `INPUTS.f32` holds `N` NCHW inputs (1 x 3 x SIZE x SIZE, little-endian `f32`, already scaled to
//! the net's input range) written by `tools/train/parity.py`. Every compiled backend runs every
//! input; the outputs are written to `OUT_PREFIX.<backend>.f32` (concatenated, the net's first
//! output) for the comparison against PyTorch, and the per-backend network-only timing (median and
//! p95 over `N * RUNS` runs after one warm-up pass) and the load time are printed. The `ort`
//! backend needs the pinned runtime (`cargo xtask fetch-ort`, `AUTOCROP_ORT_DYLIB`).
//!
//! Nothing here reads an image; the inputs are tensors.

use auto_crop_infer::{InferenceBackend, VerifiedModel};
use std::time::Instant;

fn read_f32(path: &str) -> Vec<f32> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"));
    assert_eq!(bytes.len() % 4, 0, "{path}: not a whole number of f32");
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    let i = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[i.min(sorted.len() - 1)]
}

/// The Rust decoder on one backend's outputs: quads (canvas pixels) for the comparison with
/// `tools/train/decode.py`, and the post-processing time.
#[cfg(feature = "learned")]
fn decode_and_time(
    prefix: &str,
    name: &str,
    size: usize,
    out_len: usize,
    n: usize,
    out_bytes: &[u8],
) {
    use auto_crop_infer::corner::decode;
    let outs: Vec<f32> = out_bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect();
    let stride = size / ((out_len / 11) as f64).sqrt() as usize;
    let mut lines = String::new();
    let mut t_ms = Vec::new();
    for i in 0..n {
        let o = &outs[i * out_len..(i + 1) * out_len];
        let t = Instant::now();
        let d = decode(o, size, stride);
        t_ms.push(t.elapsed().as_secs_f64() * 1e3);
        match d {
            Some(d) => {
                let q: Vec<String> = d
                    .quad
                    .iter()
                    .map(|p| format!("{:.6} {:.6}", p[0], p[1]))
                    .collect();
                lines.push_str(&q.join(" "));
                lines.push('\n');
            }
            None => lines.push_str("none\n"),
        }
    }
    std::fs::write(format!("{prefix}.{name}.quads.txt"), lines).expect("write quads");
    t_ms.sort_by(f64::total_cmp);
    println!(
        "{name}: Rust decode {:.3} ms median, {:.3} ms p95 over {n} outputs (stride {stride})",
        percentile(&t_ms, 0.5),
        percentile(&t_ms, 0.95)
    );
}

/// Pre-processing time: letterbox of a typical phone picture (random bytes, RGB8 in memory).
#[cfg(feature = "learned")]
fn time_letterbox(size: usize) {
    use auto_crop_infer::corner::letterbox_rgb8;
    for (w, h) in [(4032usize, 3024usize), (1600, 1200)] {
        let mut s: u64 = 0x9E37_79B9_7F4A_7C15;
        let rgb: Vec<u8> = (0..w * h * 3)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                (s >> 56) as u8
            })
            .collect();
        let _ = letterbox_rgb8(&rgb, w, h, size);
        let mut ms = Vec::new();
        for _ in 0..7 {
            let t = Instant::now();
            let _ = letterbox_rgb8(&rgb, w, h, size);
            ms.push(t.elapsed().as_secs_f64() * 1e3);
        }
        ms.sort_by(f64::total_cmp);
        println!(
            "Rust letterbox {w}x{h} -> {size}: {:.1} ms median (single thread, area average)",
            percentile(&ms, 0.5)
        );
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 6 {
        eprintln!("usage: net_parity MODEL.onnx INPUTS.f32 N SIZE OUT_PREFIX [THREADS=4] [RUNS=3]");
        std::process::exit(2);
    }
    let n: usize = a[3].parse().expect("N");
    let size: usize = a[4].parse().expect("SIZE");
    let threads: usize = a.get(6).and_then(|s| s.parse().ok()).unwrap_or(4);
    let runs: usize = a.get(7).and_then(|s| s.parse().ok()).unwrap_or(3);
    let shape = [1, 3, size, size];
    let per = 3 * size * size;
    let inputs = read_f32(&a[2]);
    assert_eq!(inputs.len(), n * per, "INPUTS.f32 must hold N inputs");
    #[cfg(feature = "learned")]
    time_letterbox(size);
    let model = VerifiedModel::from_bytes_unpinned(std::fs::read(&a[1]).expect("model"));
    for name in auto_crop_infer::compiled_backends() {
        let t0 = Instant::now();
        let mut b: Box<dyn InferenceBackend> =
            match auto_crop_infer::make_backend(name, &model, threads) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("{name}: SKIPPED ({e})");
                    continue;
                }
            };
        let load_ms = t0.elapsed().as_secs_f64() * 1e3;
        let mut out_bytes: Vec<u8> = Vec::new();
        let mut out_len = 0;
        for i in 0..n {
            let o = b.run(&inputs[i * per..(i + 1) * per], shape).expect("run");
            out_len = o.data.len();
            for v in &o.data {
                out_bytes.extend_from_slice(&v.to_le_bytes());
            }
        }
        std::fs::write(format!("{}.{name}.f32", a[5]), &out_bytes).expect("write outputs");
        #[cfg(feature = "learned")]
        decode_and_time(&a[5], name, size, out_len, n, &out_bytes);
        let mut ms = Vec::with_capacity(n * runs);
        for _ in 0..runs {
            for i in 0..n {
                let t = Instant::now();
                let _ = b.run(&inputs[i * per..(i + 1) * per], shape).expect("run");
                ms.push(t.elapsed().as_secs_f64() * 1e3);
            }
        }
        ms.sort_by(f64::total_cmp);
        println!(
            "{name}: threads {threads}, load {load_ms:.1} ms, {n} inputs ({out_len} outputs each), network {:.2} ms median, {:.2} ms p95 over {} runs",
            percentile(&ms, 0.5),
            percentile(&ms, 0.95),
            ms.len()
        );
    }
}
