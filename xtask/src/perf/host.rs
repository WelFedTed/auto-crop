// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Host fingerprint and background-load monitor for `cargo xtask perf` (ROADMAP M1.60, M1.63).
//!
//! A wall-clock number without its machine and its load is not evidence (PLAN 7.2). The monitor
//! samples the CPU use of every *other* process once a second while a measurement runs, so a
//! measurement taken next to a busy `ffmpeg` is labelled NOISY by data, not by guesswork.

use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// Other-process CPU load (percent of all logical CPUs) above which a measurement is NOISY.
pub const NOISY_PCT: f32 = 10.0;

pub struct Host {
    pub cpu: String,
    pub physical: usize,
    pub logical: usize,
    pub ram_gb: f64,
    pub os: String,
    pub power: String,
    pub profile: &'static str,
}

impl Host {
    pub fn capture() -> Host {
        let mut sys = System::new();
        sys.refresh_cpu_all();
        sys.refresh_memory();
        Host {
            cpu: sys
                .cpus()
                .first()
                .map_or_else(|| "unknown".to_owned(), |c| c.brand().trim().to_owned()),
            physical: System::physical_core_count().unwrap_or(0),
            logical: sys.cpus().len(),
            ram_gb: sys.total_memory() as f64 / (1u64 << 30) as f64,
            os: System::long_os_version().unwrap_or_else(|| "unknown".to_owned()),
            power: power_state(),
            profile: if cfg!(debug_assertions) {
                "dev (DEBUG: not for measurement)"
            } else {
                "release (cargo default: LTO off, 16 codegen units, no target-cpu)"
            },
        }
    }

    pub fn markdown(&self) -> String {
        format!(
            "- CPU: {} ({} cores / {} threads)\n- RAM: {:.1} GB\n- OS: {}\n- Power: {}\n- Build: {}",
            self.cpu, self.physical, self.logical, self.ram_gb, self.os, self.power, self.profile
        )
    }

    pub fn json(&self) -> Value {
        json!({"cpu": self.cpu, "physical_cores": self.physical, "logical_cores": self.logical,
               "ram_gb": self.ram_gb, "os": self.os, "power": self.power, "build": self.profile})
    }
}

#[cfg(windows)]
fn power_state() -> String {
    use std::process::Command;
    let run = |args: &[&str]| {
        Command::new(args[0])
            .args(&args[1..])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
    };
    let plan = run(&["powercfg", "/getactivescheme"])
        .and_then(|s| {
            let open = s.rfind('(')?;
            Some(s[open + 1..].trim_end_matches(')').to_owned())
        })
        .unwrap_or_else(|| "unknown plan".to_owned());
    let batteries = run(&[
        "powershell",
        "-NoProfile",
        "-Command",
        "(Get-CimInstance Win32_Battery | Measure-Object).Count",
    ])
    .and_then(|s| s.parse::<u32>().ok());
    let source = match batteries {
        Some(0) => "no battery (desktop, mains power)".to_owned(),
        Some(n) => format!("{n} battery device(s): AC state not checked"),
        None => "battery state unknown".to_owned(),
    };
    format!("{source}; power plan: {plan}")
}

#[cfg(not(windows))]
fn power_state() -> String {
    "not recorded on this OS".to_owned()
}

/// What the other processes did while a measurement ran.
#[derive(Debug, Clone)]
pub struct Load {
    pub mean_other_pct: f32,
    pub max_other_pct: f32,
    pub samples: usize,
    /// Processes by mean CPU (percent of one core), busiest first, at most three.
    pub top: Vec<(String, f32)>,
}

impl Load {
    pub fn noisy(&self) -> bool {
        self.mean_other_pct > NOISY_PCT
    }

    /// `NOISY` or `quiet`, with the numbers.
    pub fn label(&self) -> String {
        let top: Vec<String> = self
            .top
            .iter()
            .map(|(n, c)| format!("{n} {c:.0}%"))
            .collect();
        format!(
            "{}: other processes used {:.1}% of all CPU on average (max {:.1}%, {} samples; busiest readable: {}; per-process figures cover only processes this tool can read, the global counter is authoritative)",
            if self.noisy() { "NOISY" } else { "quiet" },
            self.mean_other_pct,
            self.max_other_pct,
            self.samples,
            if top.is_empty() {
                "none".to_owned()
            } else {
                top.join(", ")
            }
        )
    }

    pub fn json(&self) -> Value {
        json!({"noisy": self.noisy(), "mean_other_pct": self.mean_other_pct,
               "max_other_pct": self.max_other_pct, "samples": self.samples,
               "busiest": self.top.iter().map(|(n, c)| json!({"name": n, "cpu_pct_of_one_core": c})).collect::<Vec<_>>()})
    }
}

/// Samples other-process CPU load once a second until [`Monitor::finish`].
pub struct Monitor {
    stop: Arc<AtomicBool>,
    handle: JoinHandle<Load>,
}

impl Monitor {
    pub fn start() -> Monitor {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let handle = std::thread::spawn(move || sample_loop(&flag));
        Monitor { stop, handle }
    }

    pub fn finish(self) -> Load {
        self.stop.store(true, Ordering::Relaxed);
        self.handle.join().unwrap_or(Load {
            mean_other_pct: 0.0,
            max_other_pct: 0.0,
            samples: 0,
            top: Vec::new(),
        })
    }
}

fn sample_loop(stop: &AtomicBool) -> Load {
    let me = Pid::from_u32(std::process::id());
    let logical = std::thread::available_parallelism().map_or(1, |n| n.get()) as f32;
    let mut sys = System::new();
    // Baselines: CPU use is a delta between two refreshes. The process table is refreshed only
    // here and at the end (it can take a second on a busy machine), so "busiest" is each
    // process's mean over the whole window; the per-second load comes from the cheap global
    // counter minus this process's own share.
    sys.refresh_cpu_usage();
    sys.refresh_processes(ProcessesToUpdate::All, true);
    let own_kind = ProcessRefreshKind::nothing().with_cpu();
    let (mut sum, mut max, mut n) = (0.0f32, 0.0f32, 0usize);
    'outer: loop {
        // One second between samples, in slices so a stop request is honoured quickly.
        for _ in 0..20 {
            if stop.load(Ordering::Relaxed) {
                break 'outer;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        sys.refresh_cpu_usage();
        sys.refresh_processes_specifics(ProcessesToUpdate::Some(&[me]), true, own_kind);
        let own = sys.process(me).map_or(0.0, |p| p.cpu_usage() / logical);
        let others = (sys.global_cpu_usage() - own).clamp(0.0, 100.0);
        sum += others;
        max = max.max(others);
        n += 1;
    }
    sys.refresh_processes(ProcessesToUpdate::All, true);
    let mut per_name: HashMap<String, f32> = HashMap::new();
    for (pid, p) in sys.processes() {
        if *pid != me {
            *per_name
                .entry(p.name().to_string_lossy().into_owned())
                .or_default() += p.cpu_usage();
        }
    }
    let mut top: Vec<(String, f32)> = per_name.into_iter().filter(|(_, v)| *v >= 1.0).collect();
    top.sort_by(|a, b| b.1.total_cmp(&a.1));
    top.truncate(3);
    Load {
        mean_other_pct: if n == 0 { 0.0 } else { sum / n as f32 },
        max_other_pct: max,
        samples: n,
        top,
    }
}

/// A short look at the load before measuring (about 3 s).
pub fn idle_check() -> Load {
    let m = Monitor::start();
    std::thread::sleep(Duration::from_millis(3200));
    m.finish()
}
