// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `cargo xtask perf <host|gen|stages|memory|batch>`: the measurement harness behind
//! `docs/perf/budgets.md` (ROADMAP M1.60 memory profile and batch throughput, M1.63 measured
//! budget table). It drives `auto_crop_engine::skeleton`, the chained pipeline of M1.54.
//!
//! Rules (PLAN 7.2): release builds only (`cargo run --release -p xtask -- perf ...`; the `cargo
//! xtask` alias is a debug build and is refused), every run prints the host and the background load
//! it saw, and a run next to a busy process is labelled NOISY. Synthetic images are generated
//! deterministically into `target/perf/` and never committed. The counting allocator of
//! `alloc_count` is the global allocator of this binary; it adds two relaxed atomics per
//! allocation, which is noise next to the multi-megabyte buffers these pipelines allocate.

mod batch;
mod host;
mod memory;
mod stages;
mod standin;

use auto_crop_engine::skeleton::bench_images::{dims_for_megapixels, jpeg, jpeg_filling_frame};
use std::path::{Path, PathBuf};

const HELP: &str = "\
  perf host
        Print the host fingerprint and the background CPU load (2 s sample).
  perf gen [--mp 12] [--count 200] [--dir target/perf]
        Generate deterministic synthetic JPEGs (never committed) into <dir>/batch-<mp>mp/.
  perf stages [--mp 12,48,100] [--runs 20] [--warmup 3] [--threads all|N] [--enhance otsu|sauvola|off]
              [--fill] [--file <jpeg>] [--json <out>]
              [--analyse classical|standin-net|standin-canny] [--net-backend ort|rten]
              [--net-threads 4] [--net <onnx>]
        Per-stage p50/p95 of the pipeline skeleton against the PROVISIONAL Table A budgets, with
        the stage-sum check. Release builds only. The stand-in analyse modes need
        --features standin-ort,standin-rten,standin-canny (and `cargo xtask fetch-ort` for ort).
  perf standin [--mp 12] [--runs 30] [--warmup 5] [--threads 1,4] [--backends ort,rten]
               [--net <onnx>] [--json <out>]
        The analysis stand-ins (M1.55) side by side on the 1024 px proxy: classical detector,
        Canny + contours, and the random-weight 256x256 net per backend and thread count,
        every row labelled STAND-IN and NOISY when the machine is shared. Release builds only.
  perf memory [--mp 12,48,100] [--json <out>]
        Peak heap per image against pixels * 9 + 64 MiB (3 x RGB8 + 64 MB), per stage.
  perf batch [--mp 12] [--count 200] [--workers 1,2,3,4,5,6,7,8] [--json <out>]
        Batch throughput on N single-threaded workers under a MemoryBudget: images/s, scaling
        efficiency against 70%, peak RSS.
";

pub fn run(args: &[String]) -> Result<(), String> {
    let (sub, rest) = args.split_first().ok_or_else(|| HELP.to_owned())?;
    let flags = Flags(rest);
    if cfg!(debug_assertions) && sub != "gen" && !flags.has("--allow-debug") {
        return Err("perf measurements need a release build: cargo run --release -p xtask -- perf <command> (the `cargo xtask` alias builds debug; --allow-debug overrides for a smoke test, numbers are meaningless)".to_owned());
    }
    match sub.as_str() {
        "host" => {
            let h = host::Host::capture();
            println!("{}", h.markdown());
            println!("{}", host::idle_check().label());
            Ok(())
        }
        "gen" => batch::generate(&flags).map(drop),
        "stages" => stages::run(&flags),
        "standin" => standin::run(&flags),
        "memory" => memory::run(&flags),
        "batch" => batch::run(&flags),
        other => Err(format!("unknown perf command `{other}`\n{HELP}")),
    }
}

/// Minimal `--flag value` access.
pub struct Flags<'a>(pub &'a [String]);

impl Flags<'_> {
    pub fn has(&self, name: &str) -> bool {
        self.0.iter().any(|a| a == name)
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .position(|a| a == name)
            .and_then(|i| self.0.get(i + 1))
            .map(String::as_str)
    }

    pub fn num<T: std::str::FromStr>(&self, name: &str, default: T) -> Result<T, String> {
        match self.get(name) {
            None => Ok(default),
            Some(v) => v
                .parse()
                .map_err(|_| format!("{name} needs a number, got `{v}`")),
        }
    }

    pub fn list<T: std::str::FromStr>(&self, name: &str, default: &str) -> Result<Vec<T>, String> {
        self.get(name)
            .unwrap_or(default)
            .split(',')
            .map(|v| {
                v.trim()
                    .parse()
                    .map_err(|_| format!("{name} needs numbers like {default}, got `{v}`"))
            })
            .collect()
    }

    pub fn dir(&self) -> PathBuf {
        PathBuf::from(self.get("--dir").unwrap_or("target/perf"))
    }
}

/// Nearest-rank percentile of an ascending slice.
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let rank = (p * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

pub fn sorted(mut v: Vec<f64>) -> Vec<f64> {
    v.sort_by(f64::total_cmp);
    v
}

/// Writes JSON to `path` (creating parent directories).
pub fn write_json(path: &str, v: &serde_json::Value) -> Result<(), String> {
    if let Some(parent) = Path::new(path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(
        path,
        serde_json::to_string_pretty(v).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("cannot write {path}: {e}"))
}

/// The file of a cached single synthetic image, generating it first if it is not there.
pub fn ensure_image(
    dir: &Path,
    megapixels: f64,
    index: u64,
    fill: bool,
) -> Result<PathBuf, String> {
    let path = dir.join("img").join(format!(
        "{megapixels}mp-{index}{}.jpg",
        if fill { "-fill" } else { "" }
    ));
    if path.is_file() {
        return Ok(path);
    }
    let (w, h) = dims_for_megapixels(megapixels);
    eprintln!("generating {} ({w}x{h}) ...", path.display());
    let bytes = if fill {
        jpeg_filling_frame(w, h, index)
    } else {
        jpeg(w, h, index)
    };
    write_atomic(&path, &bytes)?;
    Ok(path)
}

/// Writes through a temporary name so an interrupted run never leaves a truncated image.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, bytes).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn percentiles_are_nearest_rank() {
        let s: Vec<f64> = (1..=20).map(f64::from).collect();
        assert_eq!(percentile(&s, 0.5), 10.0);
        assert_eq!(percentile(&s, 0.95), 19.0);
        assert_eq!(percentile(&s, 1.0), 20.0);
        assert_eq!(percentile(&s, 0.0), 1.0);
        assert_eq!(percentile(&[7.0], 0.95), 7.0);
        assert!(percentile(&[], 0.5).is_nan());
    }

    #[test]
    fn flags_parse_numbers_lists_and_switches() {
        let a = v(&["--mp", "12,48", "--runs", "5", "--fill"]);
        let f = Flags(&a);
        assert_eq!(f.list::<f64>("--mp", "12").unwrap(), [12.0, 48.0]);
        assert_eq!(f.num("--runs", 20usize).unwrap(), 5);
        assert_eq!(f.num("--warmup", 3usize).unwrap(), 3);
        assert!(f.has("--fill") && !f.has("--json"));
        assert!(Flags(&v(&["--runs", "x"])).num("--runs", 1usize).is_err());
        assert!(Flags(&v(&["--mp", "a"])).list::<f64>("--mp", "12").is_err());
    }

    #[test]
    fn a_debug_build_refuses_to_measure_but_may_generate() {
        if cfg!(debug_assertions) {
            assert!(run(&v(&["stages"])).unwrap_err().contains("release"));
            assert!(run(&v(&["nope", "--allow-debug"])).is_err());
        }
    }
}
