// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `auto-crop doctor` and `--version` (ROADMAP M2.48): what this machine and this build can do.
//! Reads only: it never creates the store, never purges, never writes a settings file. No webview
//! is needed (the CLI has none); there is no sandbox yet and it says so.

use crate::env::{Env, avx2_applies, has_avx2};
use crate::exit;
use auto_crop_engine::memory::system_cap;
use auto_crop_engine::store::Store;
use serde::Serialize;
use std::io::Write;

/// The multi-line `--version` text.
pub fn version_text() -> String {
    let feats: Vec<String> = [
        ("heif", cfg!(feature = "heif")),
        ("standin-ort", cfg!(feature = "standin-ort")),
        ("standin-rten", cfg!(feature = "standin-rten")),
        ("standin-canny", cfg!(feature = "standin-canny")),
    ]
    .iter()
    .map(|(n, on)| format!("{n}={}", if *on { "on" } else { "off" }))
    .collect();
    format!(
        "auto-crop {} ({})\n\
         target:    {} ({})\n\
         features:  {}\n\
         decoders:  {}\n\
         encoders:  jpeg png\n\
         network:   none (no network code is linked)\n",
        env!("CARGO_PKG_VERSION"),
        env!("AUTO_CROP_GIT_SHA"),
        env!("AUTO_CROP_TARGET"),
        env!("AUTO_CROP_PROFILE"),
        feats.join(" "),
        auto_crop_codecs::supported_input_formats().join(" ")
    )
}

#[derive(Serialize)]
struct Cpu {
    model: String,
    logical_cores: usize,
    /// `null` where there is no AVX2 floor (not x86-64).
    avx2: Option<bool>,
    floor_ok: bool,
}

#[derive(Serialize)]
struct Memory {
    total_bytes: u64,
    available_bytes: u64,
    /// The cap on the decoded pixels of the running jobs (`--mem-limit` changes it).
    job_cap_bytes: u64,
    default_jobs: usize,
}

#[derive(Serialize)]
struct StoreInfo {
    path: String,
    exists: bool,
    entries: usize,
    used_bytes: u64,
    /// Group saves interrupted by a crash that the next start will finish or undo.
    pending_journals: usize,
}

#[derive(Serialize)]
struct Heif {
    feature: bool,
    runtime: Option<String>,
    hevc: Option<bool>,
    av1: Option<bool>,
}

#[derive(Serialize)]
struct Report {
    schema: &'static str,
    v: u32,
    version: &'static str,
    ok: bool,
    cpu: Cpu,
    memory: Memory,
    store: StoreInfo,
    settings_file: String,
    settings_present: bool,
    decoders: Vec<&'static str>,
    encoders: Vec<&'static str>,
    heif: Heif,
    sandbox: &'static str,
    network: &'static str,
}

#[cfg(feature = "heif")]
fn heif_info() -> Heif {
    Heif {
        feature: true,
        runtime: Some(auto_crop_codecs::heif::runtime_version()),
        hevc: Some(auto_crop_codecs::heif::have_hevc_decoder()),
        av1: Some(auto_crop_codecs::heif::have_av1_decoder()),
    }
}

#[cfg(not(feature = "heif"))]
fn heif_info() -> Heif {
    Heif {
        feature: false,
        runtime: None,
        hevc: None,
        av1: None,
    }
}

fn gather(env: &Env) -> Report {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    sys.refresh_cpu_all();
    let model = sys
        .cpus()
        .first()
        .map_or_else(|| "unknown".to_owned(), |c| c.brand().trim().to_owned());
    let floor_ok = has_avx2();
    let dir = env.paths.backups_dir();
    let store = Store::new(dir.clone());
    let exists = dir.is_dir();
    let (entries, used, pending) = if exists {
        (
            store.list().len(),
            store.used_bytes(),
            auto_crop_engine::group::pending_journals(&store),
        )
    } else {
        (0, 0, 0)
    };
    let settings = env.paths.settings_file();
    Report {
        schema: "auto-crop/doctor",
        v: 1,
        version: env!("CARGO_PKG_VERSION"),
        ok: floor_ok,
        cpu: Cpu {
            model,
            logical_cores: std::thread::available_parallelism().map_or(1, usize::from),
            avx2: avx2_applies().then_some(floor_ok),
            floor_ok,
        },
        memory: Memory {
            total_bytes: sys.total_memory(),
            available_bytes: sys.available_memory(),
            job_cap_bytes: system_cap(),
            default_jobs: crate::process::default_jobs(),
        },
        store: StoreInfo {
            path: dir.to_string_lossy().into_owned(),
            exists,
            entries,
            used_bytes: used,
            pending_journals: pending,
        },
        settings_present: settings.is_file(),
        settings_file: settings.to_string_lossy().into_owned(),
        decoders: auto_crop_codecs::supported_input_formats().to_vec(),
        encoders: vec!["jpeg", "png"],
        heif: heif_info(),
        sandbox: "none",
        network: "none",
    }
}

fn gb(b: u64) -> String {
    format!("{:.1} GB", b as f64 / f64::from(1u32 << 30))
}

pub fn run(env: &Env) -> u8 {
    let r = gather(env);
    let code = if r.ok { exit::OK } else { exit::PRECONDITION };
    let mut out = std::io::stdout().lock();
    if env.global.json {
        let _ = writeln!(
            out,
            "{}",
            serde_json::to_string_pretty(&r).unwrap_or_default()
        );
        return code;
    }
    let tick = |ok: bool| if ok { "ok" } else { "PROBLEM" };
    let _ = writeln!(
        out,
        "auto-crop {} ({})",
        r.version,
        env!("AUTO_CROP_GIT_SHA")
    );
    let _ = writeln!(
        out,
        "cpu:       {} ({} logical cores)",
        r.cpu.model, r.cpu.logical_cores
    );
    match r.cpu.avx2 {
        Some(has) => {
            let _ = writeln!(
                out,
                "avx2:      {}  [{}]{}",
                if has { "yes" } else { "no" },
                tick(has),
                if has {
                    ""
                } else {
                    "  (below the supported floor: processing exits 6)"
                }
            );
        }
        None => {
            let _ = writeln!(out, "avx2:      not applicable on this architecture  [ok]");
        }
    }
    let _ = writeln!(
        out,
        "memory:    {} total, {} available; jobs use at most {} (default {} at once)",
        gb(r.memory.total_bytes),
        gb(r.memory.available_bytes),
        gb(r.memory.job_cap_bytes),
        r.memory.default_jobs
    );
    let _ = writeln!(
        out,
        "backups:   {} ({}), {} entries, {:.1} MB, {} interrupted saves to finish",
        r.store.path,
        if r.store.exists {
            "exists"
        } else {
            "not created yet"
        },
        r.store.entries,
        r.store.used_bytes as f64 / f64::from(1u32 << 20),
        r.store.pending_journals
    );
    let _ = writeln!(
        out,
        "settings:  {} ({}; the CLI reads only the backup retention)",
        r.settings_file,
        if r.settings_present {
            "present"
        } else {
            "absent, defaults"
        }
    );
    let _ = writeln!(out, "decoders:  {}", r.decoders.join(" "));
    let _ = writeln!(out, "encoders:  {}", r.encoders.join(" "));
    if r.heif.feature {
        let _ = writeln!(
            out,
            "heif:      built in; libheif {}, HEVC {}, AV1 {}",
            r.heif.runtime.as_deref().unwrap_or("?"),
            if r.heif.hevc == Some(true) {
                "found"
            } else {
                "NOT FOUND (HEIC will not open)"
            },
            if r.heif.av1 == Some(true) {
                "found"
            } else {
                "NOT FOUND (AVIF will not open)"
            },
        );
    } else {
        let _ = writeln!(
            out,
            "heif:      not in this build (HEIC, HEIF and AVIF do not open; the separate package has them)"
        );
    }
    let _ = writeln!(out, "webview:   not needed (the CLI has none)");
    let _ = writeln!(
        out,
        "sandbox:   none yet (decoders run inside this process)"
    );
    let _ = writeln!(out, "network:   none (no network code is linked)");
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_names_the_build_and_what_it_reads() {
        let v = version_text();
        assert!(v.starts_with("auto-crop "));
        for needle in [
            "target:",
            "features:",
            "decoders:",
            "jpeg",
            "encoders:",
            "network:",
        ] {
            assert!(v.contains(needle), "{needle} in {v}");
        }
    }
}
