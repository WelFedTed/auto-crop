// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! What the sandboxed worker adds to a decode: the same file decoded in this process and through
//! a pooled worker (docs/perf/sandbox-worker.md). Prints one `LATENCY` line:
//!
//! ```text
//! cargo build --release -p auto-crop-worker --features heif --examples
//! cargo build --release -p auto-crop-worker --features heif
//! target/release/examples/latency <file> [repeat]
//! ```
//!
//! `first` is the first decode through a fresh pool (process start, confinement, library
//! preload, warm-up, one decode); `steady` is the mean of the following decodes on the reused
//! worker; `inproc` is the mean of the in-process decodes of the same bytes. Shared runners are
//! NOISY: the numbers are for the order of magnitude, not for a gate.

use auto_crop_codecs::DecodeLimits;
use auto_crop_sandbox::{PoolConfig, WorkerPool, os};
use std::time::Instant;

fn mean_ms(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len().max(1) as f64
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: latency <file> [repeat]");
    let repeat: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(10);
    let bytes = std::fs::read(&path).expect("read the file");
    let limits = DecodeLimits::default();

    let exe = std::env::current_exe().unwrap();
    let worker = exe
        .parent()
        .and_then(|d| d.parent())
        .map(|d| d.join(auto_crop_sandbox::WORKER_NAME))
        .expect("worker path");
    assert!(
        worker.is_file(),
        "build the worker first: {}",
        worker.display()
    );
    // the packaged plugin folder, as the engine finds it (a development build has none and uses
    // LIBHEIF_PLUGIN_PATH, which the pool passes through)
    let _ = os::attempt_names();

    // in-process
    #[cfg(feature = "heif")]
    {
        let _ = auto_crop_codecs::heif::init();
    }
    let mut inproc = Vec::new();
    let mut dims = (0, 0);
    for _ in 0..repeat {
        let t = Instant::now();
        let d = auto_crop_codecs::decode_in_process(&bytes, &limits, true, false)
            .unwrap_or_else(|e| panic!("in-process decode failed: {e}"));
        inproc.push(t.elapsed().as_secs_f64() * 1e3);
        dims = (d.raster.width, d.raster.height);
    }

    // through the worker
    let mut cfg = PoolConfig::new(worker);
    cfg.workers = 1;
    let pool = WorkerPool::new(cfg);
    let t = Instant::now();
    pool.decode(&bytes, &limits, true, false)
        .unwrap_or_else(|e| panic!("worker decode failed: {e:?}"));
    let first = t.elapsed().as_secs_f64() * 1e3;
    let mut steady = Vec::new();
    for _ in 0..repeat {
        let t = Instant::now();
        pool.decode(&bytes, &limits, true, false)
            .unwrap_or_else(|e| panic!("worker decode failed: {e:?}"));
        steady.push(t.elapsed().as_secs_f64() * 1e3);
    }
    let st = pool.status();
    let (a, b) = (mean_ms(&inproc), mean_ms(&steady));
    println!(
        "LATENCY file={} size={}x{} bytes={} repeat={repeat} inproc_ms={a:.1} worker_first_ms={first:.1} worker_steady_ms={b:.1} added_ms={:.1} spawned={} level={}",
        std::path::Path::new(&path)
            .file_name()
            .unwrap()
            .to_string_lossy(),
        dims.0,
        dims.1,
        bytes.len(),
        b - a,
        st.spawned,
        st.level.unwrap_or_default()
    );
}
