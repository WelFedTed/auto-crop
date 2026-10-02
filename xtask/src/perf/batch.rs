// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `perf gen` and `perf batch`: batch throughput of the skeleton (ROADMAP M1.60). N workers, one
//! image per worker, every parallel kernel on a private one-thread pool (PLAN 7.3 rank 2: no
//! nesting of the two levels), jobs admitted by a [`MemoryBudget`] at `pixels * 9 + 64 MiB` each.
//! Reported per worker count: images/s, scaling efficiency against one worker (the plan assumes
//! 70%), peak resident set (sampled every 25 ms), and a digest of all outputs, which must be the
//! same at every worker count.
//!
//! The images are generated deterministically from the index (`skeleton::bench_images`), written
//! under `target/perf/` and never committed. They are synthetic stand-ins, not a real corpus.

use super::host::{Host, Monitor, idle_check};
use super::{Flags, percentile, sorted, write_atomic, write_json};
use auto_crop_core::CancelToken;
use auto_crop_engine::memory::{MIB, MemoryBudget};
use auto_crop_engine::skeleton::bench_images::{dims_for_megapixels, jpeg};
use auto_crop_engine::skeleton::{Input, Options, Stage, run as run_pipeline, thread_pool};
use serde_json::json;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// The efficiency the plan assumes (PLAN 7.1: "workers x 70%").
pub const ASSUMED_EFFICIENCY: f64 = 0.70;

fn batch_dir(f: &Flags, megapixels: f64) -> PathBuf {
    f.dir().join(format!("batch-{megapixels}mp"))
}

fn file_name(i: u64) -> String {
    format!("img-{i:04}.jpg")
}

/// `perf gen`: writes the missing files of the batch set, in parallel.
pub fn generate(f: &Flags) -> Result<Vec<PathBuf>, String> {
    let mp: f64 = f.num("--mp", 12.0)?;
    let count: u64 = f.num("--count", 200)?;
    let dir = batch_dir(f, mp);
    let (w, h) = dims_for_megapixels(mp);
    let files: Vec<PathBuf> = (0..count).map(|i| dir.join(file_name(i))).collect();
    let todo: Vec<u64> = (0..count)
        .filter(|i| !files[*i as usize].is_file())
        .collect();
    if !todo.is_empty() {
        let threads = std::thread::available_parallelism().map_or(2, |n| n.get().min(8));
        eprintln!(
            "generating {} of {count} images ({w}x{h}) into {} on {threads} threads ...",
            todo.len(),
            dir.display()
        );
        let next = AtomicUsize::new(0);
        let failed = std::sync::Mutex::new(None::<String>);
        std::thread::scope(|s| {
            for _ in 0..threads {
                s.spawn(|| {
                    loop {
                        let k = next.fetch_add(1, Ordering::Relaxed);
                        let Some(&i) = todo.get(k) else { break };
                        let bytes = jpeg(w, h, i);
                        if let Err(e) = write_atomic(&files[i as usize], &bytes) {
                            *failed.lock().unwrap_or_else(|p| p.into_inner()) = Some(e);
                            break;
                        }
                        if (k + 1).is_multiple_of(20) {
                            eprintln!("  {} / {}", k + 1, todo.len());
                        }
                    }
                });
            }
        });
        if let Some(e) = failed.into_inner().unwrap_or_else(|p| p.into_inner()) {
            return Err(e);
        }
    }
    let bytes: u64 = files
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .sum();
    println!(
        "{count} images of {w}x{h} in {} ({:.1} MB total, {:.2} MB each)",
        dir.display(),
        bytes as f64 / 1e6,
        bytes as f64 / 1e6 / count.max(1) as f64
    );
    Ok(files)
}

#[derive(Debug, Clone)]
pub struct Pass {
    pub workers: usize,
    pub images: usize,
    pub failures: usize,
    pub wall_s: f64,
    pub peak_rss: u64,
    /// Mean CPU share of all other processes during this pass (percent of all CPUs).
    pub other_load_pct: f32,
    pub budget_peak_in_use: u64,
    /// Mean sum of the stage times per image, in ms (the CPU cost of one image on one thread).
    pub cpu_ms_per_image: f64,
    /// Mean per stage, ms.
    pub stage_means: Vec<(Stage, f64)>,
    /// Order-independent digest of every output file.
    pub digest: u64,
}

impl Pass {
    pub fn images_per_s(&self) -> f64 {
        self.images as f64 / self.wall_s
    }
}

/// Scaling efficiency: throughput relative to `workers` times the one-worker throughput.
pub fn efficiency(ips: f64, workers: usize, ips_one: f64) -> f64 {
    ips / (workers as f64 * ips_one)
}

fn digest_of(bytes: &[u8]) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

/// Peak resident set of this process, sampled until `stop`.
fn rss_sampler(stop: Arc<AtomicBool>) -> std::thread::JoinHandle<u64> {
    std::thread::spawn(move || {
        let pid = Pid::from_u32(std::process::id());
        let mut sys = System::new();
        let mut peak = 0u64;
        while !stop.load(Ordering::Relaxed) {
            sys.refresh_processes_specifics(
                ProcessesToUpdate::Some(&[pid]),
                true,
                ProcessRefreshKind::nothing().with_memory(),
            );
            if let Some(p) = sys.process(pid) {
                peak = peak.max(p.memory());
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        peak
    })
}

/// Processes every file once on `workers` workers.
pub fn run_pass(files: &[PathBuf], workers: usize, budget: &MemoryBudget) -> Result<Pass, String> {
    let next = AtomicUsize::new(0);
    let stop = Arc::new(AtomicBool::new(false));
    let sampler = rss_sampler(Arc::clone(&stop));
    let token = CancelToken::never();
    let t0 = Instant::now();
    let results: Vec<Result<WorkerOut, String>> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..workers)
            .map(|_| s.spawn(|| worker(files, &next, budget, &token)))
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_else(|_| Err("worker panicked".into())))
            .collect()
    });
    let wall_s = t0.elapsed().as_secs_f64();
    stop.store(true, Ordering::Relaxed);
    let peak_rss = sampler.join().unwrap_or(0);
    let mut total = WorkerOut::default();
    for r in results {
        total.merge(r?);
    }
    let after = budget.stats();
    let n = total.images.max(1) as f64;
    Ok(Pass {
        workers,
        images: total.images,
        failures: total.failures,
        wall_s,
        peak_rss,
        other_load_pct: 0.0,
        budget_peak_in_use: after.peak,
        cpu_ms_per_image: total.stage_ms.iter().sum::<f64>() / n,
        stage_means: Stage::ALL
            .iter()
            .zip(&total.stage_ms)
            .map(|(s, ms)| (*s, ms / n))
            .collect(),
        digest: total.digest,
    })
}

#[derive(Default)]
struct WorkerOut {
    images: usize,
    failures: usize,
    waited: bool,
    stage_ms: [f64; 7],
    digest: u64,
}

impl WorkerOut {
    fn merge(&mut self, o: WorkerOut) {
        self.images += o.images;
        self.failures += o.failures;
        self.waited |= o.waited;
        for (a, b) in self.stage_ms.iter_mut().zip(o.stage_ms) {
            *a += b;
        }
        self.digest ^= o.digest;
    }
}

fn worker(
    files: &[PathBuf],
    next: &AtomicUsize,
    budget: &MemoryBudget,
    token: &CancelToken,
) -> Result<WorkerOut, String> {
    // One image per worker, every kernel single-threaded: a private one-thread pool.
    let opts = Options {
        pool: Some(thread_pool(1).map_err(|e| e.to_string())?),
        ..Options::default()
    };
    let mut out = WorkerOut::default();
    loop {
        let i = next.fetch_add(1, Ordering::Relaxed);
        let Some(path) = files.get(i) else { break };
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let probe =
            auto_crop_codecs::probe(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        let px = u64::from(probe.width) * u64::from(probe.height);
        let before = Instant::now();
        let permit = budget
            .acquire_pixels(px, token)
            .map_err(|e| format!("admission: {e}"))?;
        out.waited |= before.elapsed() > Duration::from_millis(50);
        match run_pipeline(Input::Bytes(&bytes), &opts, token) {
            Ok(done) => {
                out.images += 1;
                out.digest ^= digest_of(&done.bytes).rotate_left((i % 63) as u32);
                for (slot, st) in out.stage_ms.iter_mut().zip(Stage::ALL) {
                    *slot += done.report.ms_of(st).unwrap_or(0.0);
                }
            }
            Err(_) => out.failures += 1,
        }
        drop(permit);
    }
    Ok(out)
}

pub fn run(f: &Flags) -> Result<(), String> {
    let mp: f64 = f.num("--mp", 12.0)?;
    let workers: Vec<usize> = f.list("--workers", "1,2,3,4,5,6,7,8")?;
    if workers.contains(&0) {
        return Err("--workers needs numbers of at least 1".into());
    }
    let files = generate(f)?;
    let host = Host::capture();
    println!("## Host\n\n{}\n", host.markdown());
    let budget = MemoryBudget::from_system();
    println!(
        "Memory budget: cap {} MiB (min of 25% of RAM, 4 GiB, 50% of free RAM); weight per {mp} MP job {} MiB.\n",
        budget.cap() / MIB,
        auto_crop_engine::memory::job_weight(
            (dims_for_megapixels(mp).0 as u64) * dims_for_megapixels(mp).1 as u64
        ) / MIB
    );
    let pre = idle_check();
    println!("Load before: {}\n", pre.label());

    let per_pass: usize = f.num("--pass-images", files.len())?;
    let rounds: usize = f.num("--rounds", 1)?;
    if per_pass == 0 || rounds == 0 {
        return Err("--pass-images and --rounds need numbers of at least 1".into());
    }
    let used = &files[..per_pass.min(files.len())];
    let monitor = Monitor::start();
    let mut passes: Vec<Pass> = Vec::new();
    for round in 1..=rounds {
        for &w in &workers {
            let pass_monitor = Monitor::start();
            // A fresh budget per pass, so its peak is this pass's own.
            let mut p = run_pass(used, w, &MemoryBudget::new(budget.cap()))?;
            p.other_load_pct = pass_monitor.finish().mean_other_pct;
            eprintln!(
                "round {round}, {w} worker(s): {:.2} images/s, {:.1} s, peak RSS {} MB, other CPU {:.0}%",
                p.images_per_s(),
                p.wall_s,
                p.peak_rss / 1_000_000,
                p.other_load_pct
            );
            passes.push(p);
        }
    }
    let during = monitor.finish();
    println!("{}", markdown(&passes, used.len()));
    println!("Load during: {}", during.label());
    if during.noisy() || pre.noisy() {
        println!(
            "\nNOISY: another process was using the CPU while this ran. Throughput and scaling efficiency above are pessimistic and unreliable; re-run on an idle machine before quoting them."
        );
    }
    if let Some(out) = f.get("--json") {
        write_json(
            out,
            &json!({"host": host.json(), "load_before": pre.json(), "load_during": during.json(),
                "megapixels": mp, "images": used.len(), "rounds": rounds, "memory_cap_bytes": budget.cap(),
                "passes": passes.iter().map(|p| json!({
                    "workers": p.workers, "images": p.images, "failures": p.failures,
                    "wall_s": p.wall_s, "images_per_s": p.images_per_s(),
                    "peak_rss_bytes": p.peak_rss, "other_load_pct": p.other_load_pct, "budget_peak_in_use_bytes": p.budget_peak_in_use,
                    "cpu_ms_per_image": p.cpu_ms_per_image,
                    "stage_mean_ms": p.stage_means.iter().map(|(s, m)| json!({"stage": s.name(), "ms": m})).collect::<Vec<_>>(),
                    "digest": format!("{:016x}", p.digest)})).collect::<Vec<_>>()}),
        )?;
    }
    if passes.iter().any(|p| p.failures > 0) {
        return Err("some images failed".into());
    }
    if passes.windows(2).any(|w| w[0].digest != w[1].digest) {
        return Err("outputs differ between worker counts".into());
    }
    Ok(())
}

/// One row of the batch table: every pass at one worker count, over all rounds.
#[derive(Debug, Clone)]
pub struct Summary {
    pub workers: usize,
    pub rounds: usize,
    pub ips_median: f64,
    pub ips_best: f64,
    pub peak_rss: u64,
    pub budget_peak_in_use: u64,
    /// CPU ms per image of the best pass.
    pub cpu_ms_best: f64,
    /// Mean other-process load over the passes of this row.
    pub load_mean: f32,
}

/// Groups passes by worker count, in order of first appearance.
pub fn summarise(passes: &[Pass]) -> Vec<Summary> {
    let mut order: Vec<usize> = Vec::new();
    for p in passes {
        if !order.contains(&p.workers) {
            order.push(p.workers);
        }
    }
    order
        .into_iter()
        .map(|w| {
            let of: Vec<&Pass> = passes.iter().filter(|p| p.workers == w).collect();
            let ips = sorted(of.iter().map(|p| p.images_per_s()).collect());
            let best = of
                .iter()
                .max_by(|a, b| a.images_per_s().total_cmp(&b.images_per_s()))
                .expect("a group is never empty");
            Summary {
                workers: w,
                rounds: of.len(),
                ips_median: percentile(&ips, 0.5),
                ips_best: best.images_per_s(),
                peak_rss: of.iter().map(|p| p.peak_rss).max().unwrap_or(0),
                budget_peak_in_use: of.iter().map(|p| p.budget_peak_in_use).max().unwrap_or(0),
                cpu_ms_best: best.cpu_ms_per_image,
                load_mean: of.iter().map(|p| p.other_load_pct).sum::<f32>() / of.len() as f32,
            }
        })
        .collect()
}

pub fn markdown(passes: &[Pass], images: usize) -> String {
    use std::fmt::Write as _;
    let rows = summarise(passes);
    let one = rows.iter().find(|r| r.workers == 1);
    let mut s = format!(
        "{images} images per pass, {} round(s) per worker count (rounds are interleaved so a drifting background load hits every row alike; `best` is the least-disturbed pass, noise only ever slows a run).\n\n| workers | images/s median | images/s best | efficiency (median) | efficiency (best) | >= 70% (best)? | peak RSS MB | budget peak in use MB | CPU ms/image (best) | other CPU % (mean) |\n|---:|---:|---:|---:|---:|---|---:|---:|---:|---:|\n",
        rows.first().map_or(0, |r| r.rounds)
    );
    for r in &rows {
        let (em, eb, ok) = match one {
            Some(o) => {
                let eb = efficiency(r.ips_best, r.workers, o.ips_best);
                (
                    format!(
                        "{:.0}%",
                        100.0 * efficiency(r.ips_median, r.workers, o.ips_median)
                    ),
                    format!("{:.0}%", 100.0 * eb),
                    if eb >= ASSUMED_EFFICIENCY {
                        "yes"
                    } else {
                        "no"
                    },
                )
            }
            None => ("n/a".into(), "n/a".into(), "-"),
        };
        let _ = writeln!(
            s,
            "| {} | {:.2} | {:.2} | {em} | {eb} | {ok} | {:.0} | {:.0} | {:.0} | {:.0} |",
            r.workers,
            r.ips_median,
            r.ips_best,
            r.peak_rss as f64 / 1e6,
            r.budget_peak_in_use as f64 / 1e6,
            r.cpu_ms_best,
            r.load_mean
        );
    }
    if let Some(first) = passes.first() {
        let _ = write!(
            s,
            "\nMean stage time per image at {} worker(s), ms:",
            first.workers
        );
        for (st, m) in &first.stage_means {
            let _ = write!(s, " {} {:.0},", st.name(), m);
        }
        s.pop();
        s.push('.');
    }
    let same = passes.windows(2).all(|w| w[0].digest == w[1].digest);
    let _ = write!(
        s,
        "\nOutputs identical across worker counts and rounds: {}.",
        if same { "yes" } else { "NO" }
    );
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn flags(dir: &Path, extra: &[&str]) -> Vec<String> {
        let mut v = vec!["--dir".to_owned(), dir.display().to_string()];
        v.extend(extra.iter().map(|s| (*s).to_owned()));
        v
    }

    #[test]
    fn efficiency_is_throughput_over_workers_times_one_worker() {
        assert!((efficiency(8.0, 4, 2.0) - 1.0).abs() < 1e-12);
        assert!((efficiency(5.6, 4, 2.0) - 0.7).abs() < 1e-12);
    }

    fn pass(workers: usize, wall_s: f64, load: f32) -> Pass {
        Pass {
            workers,
            images: 100,
            failures: 0,
            wall_s,
            peak_rss: 1_000_000 * workers as u64,
            other_load_pct: load,
            budget_peak_in_use: 175_000_000 * workers as u64,
            cpu_ms_per_image: 1000.0,
            stage_means: Stage::ALL.iter().map(|s| (*s, 10.0)).collect(),
            digest: 7,
        }
    }

    #[test]
    fn rounds_are_grouped_with_median_best_and_efficiency_against_one_worker() {
        // 1 worker: 10, 12 and 20 images/s (a disturbed round, a normal one, the best);
        // 4 workers: 32 and 36 (nearest-rank median of two is the lower one).
        let passes = [
            pass(1, 10.0, 90.0),
            pass(4, 100.0 / 32.0, 90.0),
            pass(1, 100.0 / 12.0, 80.0),
            pass(4, 100.0 / 36.0, 80.0),
            pass(1, 5.0, 10.0),
        ];
        let rows = summarise(&passes);
        assert_eq!(
            rows.iter().map(|r| r.workers).collect::<Vec<_>>(),
            [1, 4],
            "order of first appearance"
        );
        assert_eq!((rows[0].rounds, rows[1].rounds), (3, 2));
        assert!((rows[0].ips_median - 12.0).abs() < 1e-9);
        assert!((rows[0].ips_best - 20.0).abs() < 1e-9);
        assert!((rows[1].ips_best - 36.0).abs() < 1e-9);
        assert!((rows[0].load_mean - 60.0).abs() < 1e-4);
        assert_eq!(rows[1].peak_rss, 4_000_000);
        let md = markdown(&passes, 100);
        // Best-of: 36 / (4 x 20) = 45%, below the 70% line; median: 32 / (4 x 12) = 67%.
        assert!(
            md.contains("| 4 | 32.00 | 36.00 | 67% | 45% | no |"),
            "{md}"
        );
        assert!(
            md.contains("| 1 | 12.00 | 20.00 | 100% | 100% | yes |"),
            "{md}"
        );
    }

    #[test]
    fn generation_is_deterministic_and_resumable() {
        let dir = tempfile::tempdir().unwrap();
        let args = flags(dir.path(), &["--mp", "0.2", "--count", "3"]);
        let a = generate(&Flags(&args)).unwrap();
        assert_eq!(a.len(), 3);
        let first = std::fs::read(&a[1]).unwrap();
        // Delete one, regenerate: same bytes (determinism), others untouched.
        std::fs::remove_file(&a[1]).unwrap();
        let b = generate(&Flags(&args)).unwrap();
        assert_eq!(std::fs::read(&b[1]).unwrap(), first);
    }

    #[test]
    fn a_pass_processes_every_image_and_the_digest_ignores_the_worker_count() {
        let dir = tempfile::tempdir().unwrap();
        let args = flags(dir.path(), &["--mp", "0.3", "--count", "6"]);
        let files = generate(&Flags(&args)).unwrap();
        let budget = MemoryBudget::new(2 * 1024 * MIB);
        let one = run_pass(&files, 1, &budget).unwrap();
        let three = run_pass(&files, 3, &budget).unwrap();
        for p in [&one, &three] {
            assert_eq!((p.images, p.failures), (6, 0));
            assert!(p.images_per_s() > 0.0 && p.peak_rss > 0);
            assert_eq!(p.stage_means.len(), 7);
        }
        assert_eq!(one.digest, three.digest, "outputs depend on worker count");
        assert!(budget.stats().in_use == 0, "every permit is released");
        let md = markdown(&[one, three], 6);
        assert!(md.contains("yes"), "{md}");
    }

    #[test]
    fn a_tiny_budget_still_finishes_by_running_jobs_one_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let args = flags(dir.path(), &["--mp", "0.3", "--count", "4"]);
        let files = generate(&Flags(&args)).unwrap();
        // The cap is below one job's weight: each job is admitted alone, never deadlocked.
        let budget = MemoryBudget::new(1024 * 1024);
        let p = run_pass(&files, 3, &budget).unwrap();
        assert_eq!((p.images, p.failures), (4, 0));
        assert_eq!(
            budget.stats().peak,
            auto_crop_engine::memory::job_weight(
                u64::from(dims_for_megapixels(0.3).0) * u64::from(dims_for_megapixels(0.3).1)
            )
        );
    }
}
