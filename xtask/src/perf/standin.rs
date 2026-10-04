// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `perf standin`: the analysis stand-ins timed on the same 1024 px detection proxy (ROADMAP
//! M1.55), every row labelled STAND-IN until M2 and M4 replace them.
//!
//! * the classical detector (`auto-crop-imgproc`), the current `analyse` row of `perf stages`;
//! * Canny + contours with `imageproc` (feature `standin-canny`);
//! * the random-weight 256x256 MobileNetV3-class net on every compiled backend (features
//!   `standin-ort`, `standin-rten`), as inference only and as the whole stage (squash to 256x256,
//!   normalise, run, decode corners), at each thread count asked for (default 1 and 4).
//!
//! Thread counts: the rayon-based kernels run in a pool of exactly that many threads; the net
//! backends get the same number of intra-op threads. Every figure is printed with the host and the
//! background load, and tagged NOISY when the machine was shared.

use super::host::{Host, Monitor, idle_check};
use super::{Flags, percentile, sorted, write_json};
use crate::standin::{dev_runtime, make_standin_net, read_pinned};
use auto_crop_engine::skeleton::bench_images::{dims_for_megapixels, raster};
use auto_crop_engine::skeleton::standin::{StandinNet, net_input};
use auto_crop_engine::skeleton::thread_pool;
use auto_crop_imgproc::Raster;
use auto_crop_imgproc::detect::detect;
use auto_crop_imgproc::pyramid::{Level, Pyramid};
use serde_json::json;
use std::sync::Arc;
use std::time::Instant;

/// Table A: the analysis row (ms): p50 budget and the p95 ceiling over T1 exits.
const ANALYSIS_BUDGET_MS: f64 = 40.0;

struct Row {
    what: String,
    backend: String,
    threads: usize,
    min: f64,
    p50: f64,
    p95: f64,
}

fn time(warmup: usize, runs: usize, mut f: impl FnMut()) -> (f64, f64, f64) {
    for _ in 0..warmup {
        f();
    }
    let ms = sorted(
        (0..runs)
            .map(|_| {
                let t = Instant::now();
                f();
                t.elapsed().as_secs_f64() * 1e3
            })
            .collect(),
    );
    (ms[0], percentile(&ms, 0.5), percentile(&ms, 0.95))
}

pub fn run(f: &Flags) -> Result<(), String> {
    let mp: f64 = f.num("--mp", 12.0)?;
    let runs: usize = f.num("--runs", 30)?;
    let warmup: usize = f.num("--warmup", 5)?;
    if runs == 0 {
        return Err("--runs must be at least 1".into());
    }
    let threads: Vec<usize> = f.list("--threads", "1,4")?;
    let backends: Vec<String> = match f.get("--backends") {
        Some(b) => b.split(',').map(str::to_owned).collect(),
        None => auto_crop_infer::compiled_backends()
            .into_iter()
            .map(str::to_owned)
            .collect(),
    };
    let net_path = f.get("--net").map(std::path::PathBuf::from);

    let host = Host::capture();
    println!("## Host\n\n{}\n", host.markdown());
    let pre = idle_check();
    println!("Load before: {}\n", pre.label());

    // The detection proxy of a synthetic page-on-a-desk photo of `mp` megapixels.
    let (w, h) = dims_for_megapixels(mp);
    let pyramid = Pyramid::build_shared(Arc::new(raster(w, h, 1)));
    let proxy: &Raster = pyramid.level(Level::Detect);
    println!(
        "Source {w}x{h} ({mp} MP, synthetic), detection proxy {}x{} ({} px)\n",
        proxy.width,
        proxy.height,
        u64::from(proxy.width) * u64::from(proxy.height)
    );

    let model = if backends.is_empty() {
        None
    } else {
        // The default net is generated on first use; an explicit `--net` is only read.
        let path = match net_path {
            Some(p) => p,
            None => make_standin_net(std::path::Path::new("target/standin"))?,
        };
        Some(read_pinned(&path)?)
    };

    let runtime = dev_runtime();
    let monitor = Monitor::start();
    let mut rows: Vec<Row> = Vec::new();
    for &t in &threads {
        let pool = thread_pool(t).map_err(|e| e.to_string())?;
        let mut push = |what: &str, backend: &str, (min, p50, p95): (f64, f64, f64)| {
            rows.push(Row {
                what: what.to_owned(),
                backend: backend.to_owned(),
                threads: t,
                min,
                p50,
                p95,
            });
        };
        push(
            "classical detector",
            "-",
            time(warmup, runs, || {
                pool.install(|| drop(std::hint::black_box(detect(proxy))));
            }),
        );
        #[cfg(feature = "standin-canny")]
        push(
            "Canny + contours (imageproc)",
            "-",
            time(warmup, runs, || {
                pool.install(|| {
                    std::hint::black_box(auto_crop_engine::skeleton::standin::canny_quad(proxy));
                });
            }),
        );
        if let Some(model) = &model {
            for b in &backends {
                let mut backend =
                    match auto_crop_infer::make_backend_at(b, model, t, runtime.as_deref()) {
                        Ok(x) => x,
                        Err(e) => {
                            eprintln!("skipping backend {b}: {e}");
                            continue;
                        }
                    };
                let input = net_input(proxy);
                push(
                    "net inference only (256x256, random weights)",
                    b,
                    time(warmup, runs, || {
                        let out = backend
                            .run(&input, [1, 3, 256, 256])
                            .expect("the stand-in net runs");
                        std::hint::black_box(out);
                    }),
                );
                let net = StandinNet::new(backend);
                push(
                    "net analysis stage (squash + normalise + run + decode)",
                    b,
                    time(warmup, runs, || {
                        pool.install(|| {
                            let q = net.analyse(proxy).expect("the stand-in net runs");
                            std::hint::black_box(q);
                        });
                    }),
                );
            }
        }
    }
    let during = monitor.finish();
    let noisy = pre.noisy() || during.noisy();

    println!(
        "### `analyse` stand-ins on the 1024 px proxy, {runs} runs after {warmup} warm-ups (STAND-IN, PROVISIONAL budget {ANALYSIS_BUDGET_MS:.0} ms p50 and p95)\n"
    );
    println!(
        "| what (STAND-IN) | backend | threads | min ms | p50 ms | p95 ms | p95 / 40 ms | verdict |"
    );
    println!("|---|---|---:|---:|---:|---:|---:|---|");
    for r in &rows {
        let ratio = r.p95 / ANALYSIS_BUDGET_MS;
        let verdict = if r.p95 <= ANALYSIS_BUDGET_MS {
            "within"
        } else if ratio <= 1.5 {
            "over (<= 1.5x)"
        } else {
            "OVER 1.5x"
        };
        println!(
            "| {} | {} | {} | {:.2} | {:.2} | {:.2} | {ratio:.2} | {verdict}{} |",
            r.what,
            r.backend,
            r.threads,
            r.min,
            r.p50,
            r.p95,
            if noisy {
                " [NOISY: re-measure idle]"
            } else {
                ""
            }
        );
    }
    println!("\nLoad during: {}", during.label());
    if noisy {
        println!(
            "\nNOISY: another process was using the CPU. Treat every number above as a pessimistic bound, not a measurement of this code; re-run when the machine is idle."
        );
    }
    if backends.is_empty() {
        println!(
            "\nNo net rows: this build has no inference backend (cargo run --release -p xtask --features standin-ort,standin-rten,standin-canny -- perf standin)."
        );
    }
    if let Some(out) = f.get("--json") {
        write_json(
            out,
            &json!({
                "host": host.json(), "load_before": pre.json(), "load_during": during.json(),
                "noisy": noisy, "label": "STAND-IN", "mp": mp, "runs": runs,
                "proxy": [proxy.width, proxy.height],
                "rows": rows.iter().map(|r| json!({
                    "what": r.what, "backend": r.backend, "threads": r.threads,
                    "min_ms": r.min, "p50_ms": r.p50, "p95_ms": r.p95})).collect::<Vec<_>>(),
            }),
        )?;
    }
    Ok(())
}
