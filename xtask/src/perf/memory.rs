// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `perf memory`: peak heap of one image through the skeleton at 12, 48 and 100 MP against the
//! per-job weight `pixels * 9 + 64 MiB` (3 x decoded RGB8 + 64 MB: 175 MB, 500 MB, 1.0 GB;
//! PROVISIONAL; ROADMAP M1.60). Heap is counted exactly by the global counting allocator
//! (`alloc_count`), per stage and over the whole run, above the heap live when the run starts (the
//! file is read inside `read_probe`, so its bytes are in the peak, like a real job).
//!
//! Two page layouts per size: the page on a desk (half the frame) and the worst case where the
//! page fills the frame (the rectified output is about source-sized). One thread (batch mode: one
//! image per worker) and all threads (a lone interactive job). A peak above the weight is a
//! failure of the command, never a footnote.

use super::{Flags, ensure_image, write_json};
use crate::alloc_count;
use auto_crop_core::CancelToken;
use auto_crop_engine::memory::job_weight;
use auto_crop_engine::skeleton::{Input, Options, Stage, run_observed, thread_pool};
use serde_json::{Value, json};

pub struct Peak {
    pub megapixels: f64,
    pub fill: bool,
    pub threads: String,
    pub px: u64,
    /// Peak heap above the start, whole run.
    pub peak: usize,
    pub budget: usize,
    /// Peak above the start inside each stage.
    pub per_stage: Vec<(Stage, usize)>,
}

impl Peak {
    pub fn ratio(&self) -> f64 {
        self.peak as f64 / self.budget as f64
    }
}

/// Runs the pipeline once from `path` and records heap peaks.
pub fn profile(
    path: &std::path::Path,
    megapixels: f64,
    fill: bool,
    threads: Option<usize>,
) -> Result<Peak, String> {
    let opts = Options {
        pool: match threads {
            Some(n) => Some(thread_pool(n).map_err(|e| e.to_string())?),
            None => None,
        },
        ..Options::default()
    };
    let token = CancelToken::never();
    // Warm the pool and the lazily built tables on the image itself would double the memory
    // measured; a 64 x 48 image is enough to start threads and fill the weight tables.
    let tiny = auto_crop_engine::skeleton::bench_images::jpeg(64, 48, 1);
    let _ = run_observed(Input::Bytes(&tiny), &opts, &token, &mut |_| {});
    drop(tiny);

    let baseline = alloc_count::live();
    alloc_count::reset_peak();
    let mut per_stage: Vec<(Stage, usize)> = Vec::new();
    let out = run_observed(Input::Path(path), &opts, &token, &mut |t| {
        per_stage.push((t.stage, alloc_count::peak().saturating_sub(baseline)));
        // The next stage's peak starts from what is live now.
        alloc_count::reset_peak();
    })
    .map_err(|f| format!("{}: {f}", path.display()))?;
    let peak = per_stage.iter().map(|(_, p)| *p).max().unwrap_or(0);
    let px = u64::from(out.report.source.0) * u64::from(out.report.source.1);
    Ok(Peak {
        megapixels,
        fill,
        threads: threads.map_or_else(|| "all".to_owned(), |n| n.to_string()),
        px,
        peak,
        budget: job_weight(px) as usize,
        per_stage,
    })
}

fn markdown(rows: &[Peak]) -> String {
    use std::fmt::Write as _;
    let mut s = String::from(
        "| MP | page | threads | peak heap MB | budget MB (9 px + 64 MiB) | ratio | verdict | peak stage |\n|---:|---|---:|---:|---:|---:|---|---|\n",
    );
    for p in rows {
        let worst = p
            .per_stage
            .iter()
            .max_by_key(|(_, b)| *b)
            .map_or("-", |(st, _)| st.name());
        let _ = writeln!(
            s,
            "| {} | {} | {} | {:.1} | {:.1} | {:.2} | {} | {worst} |",
            p.megapixels,
            if p.fill { "fills frame" } else { "on a desk" },
            p.threads,
            p.peak as f64 / 1e6,
            p.budget as f64 / 1e6,
            p.ratio(),
            if p.peak <= p.budget {
                "within"
            } else {
                "OVER BUDGET"
            }
        );
    }
    s.push_str("\nPer-stage peak heap above the start (MB):\n\n| MP | page | threads |");
    for st in Stage::ALL {
        let _ = write!(s, " {} |", st.name());
    }
    s.push_str("\n|---:|---|---:|");
    for _ in Stage::ALL {
        s.push_str("---:|");
    }
    s.push('\n');
    for p in rows {
        let _ = write!(
            s,
            "| {} | {} | {} |",
            p.megapixels,
            if p.fill { "fills frame" } else { "on a desk" },
            p.threads
        );
        for st in Stage::ALL {
            match p.per_stage.iter().find(|(x, _)| *x == st) {
                Some((_, b)) => {
                    let _ = write!(s, " {:.1} |", *b as f64 / 1e6);
                }
                None => s.push_str(" - |"),
            }
        }
        s.push('\n');
    }
    s
}

pub fn run(f: &Flags) -> Result<(), String> {
    let mps: Vec<f64> = f.list("--mp", "12,48,100")?;
    let host = super::host::Host::capture();
    println!("## Host\n\n{}\n", host.markdown());
    let mut rows = Vec::new();
    for mp in mps {
        for fill in [false, true] {
            let path = ensure_image(&f.dir(), mp, 1, fill)?;
            // One thread is batch mode (one image per worker); all threads is a lone job.
            for threads in [Some(1), None] {
                rows.push(profile(&path, mp, fill, threads)?);
            }
        }
    }
    println!("{}", markdown(&rows));
    if let Some(out) = f.get("--json") {
        let v: Vec<Value> = rows
            .iter()
            .map(|p| {
                json!({"megapixels": p.megapixels, "fills_frame": p.fill, "threads": p.threads,
                    "px": p.px, "peak_heap_bytes": p.peak, "budget_bytes": p.budget,
                    "ratio": p.ratio(),
                    "per_stage_bytes": p.per_stage.iter().map(|(s, b)| json!({"stage": s.name(), "bytes": b})).collect::<Vec<_>>()})
            })
            .collect();
        write_json(out, &json!({"host": host.json(), "peaks": v}))?;
    }
    match rows.iter().find(|p| p.peak > p.budget) {
        Some(p) => Err(format!(
            "{} MP ({} threads, page fills frame: {}): peak heap {:.1} MB exceeds the {:.1} MB budget",
            p.megapixels,
            p.threads,
            p.fill,
            p.peak as f64 / 1e6,
            p.budget as f64 / 1e6
        )),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_small_run_reports_every_stage_and_a_sane_peak() {
        let dir = tempfile::tempdir().unwrap();
        let (w, h) = (640, 480);
        let path = dir.path().join("a.jpg");
        std::fs::write(
            &path,
            auto_crop_engine::skeleton::bench_images::jpeg(w, h, 2),
        )
        .unwrap();
        let p = profile(&path, 0.3, false, Some(1)).unwrap();
        assert_eq!(p.per_stage.len(), 7);
        assert_eq!(p.px, 640 * 480);
        // At least the decoded raster (3 bytes per pixel), at most the job weight (64 MiB + 9x).
        assert!(p.peak >= 640 * 480 * 3, "{}", p.peak);
        assert!(p.peak <= p.budget, "{} > {}", p.peak, p.budget);
        let md = markdown(&[p]);
        assert!(md.contains("within") && md.contains("rectify"));
    }

    #[test]
    fn a_peak_above_the_budget_is_marked_over() {
        let p = Peak {
            megapixels: 12.0,
            fill: true,
            threads: "1".into(),
            px: 12_000_000,
            peak: 200_000_000,
            budget: 175_000_000,
            per_stage: vec![(Stage::Decode, 200_000_000)],
        };
        assert!(markdown(&[p]).contains("OVER BUDGET"));
    }
}
