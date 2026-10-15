// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! macOS confinement of the worker (ADR-0006, ADR-0013): a Seatbelt profile that denies
//! everything, entered with `sandbox_init` before the first byte of input is read and after
//! libheif, dav1d and the libde265 plugin have been loaded. The only rules added to
//! `(deny default)` are what a decoder needs to run threads and read its CPU count; no file, no
//! network, no process creation. `RLIMIT_AS` is not enforced by the macOS kernel, so the memory
//! limit is the parent's watchdog (it samples the resident size and kills the worker) and the
//! pixel and memory caps inside libheif; `sandbox_init` is deprecated by Apple but remains what
//! Chromium and Firefox use.

use crate::worker_rt::{ConfineConfig, Confinement, probe_escapes};

/// The profile. Widened by test, never by dropping `(deny default)`.
pub const PROFILE: &str = "(version 1)\n(deny default)\n(allow sysctl-read)\n";

pub fn confine(cfg: &ConfineConfig) -> Result<Confinement, String> {
    crate::os::ffi::unix::set_resource_limits(cfg.address_space, cfg.cpu_seconds)?;
    super::ffi::macos::enter(PROFILE)?;
    Ok(Confinement {
        level: format!(
            "sandbox_init(deny default; allow sysctl-read)+rlimit(cpu={}s,core=0)+parent-rss-watchdog",
            cfg.cpu_seconds
        ),
        probes: probe_escapes(),
    })
}
