// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `auto-crop-worker`: the sandboxed decode worker (ADR-0013). It is started by the pool in
//! `auto-crop-sandbox`, never by a user. It loads what the decoders need from disk, confines
//! itself (Windows: the parent created it in an AppContainer and job; Linux: rlimits, Landlock,
//! seccomp; macOS: Seatbelt), says `Hello` with the level it reached and what it saw when it
//! probed its own confinement, and only then reads jobs: input bytes on stdin, pixels out through
//! shared memory.
//!
//! This binary is the only place libheif, dav1d and libde265 run once the pool is installed.
//! No network code, no `unsafe`.

use auto_crop_sandbox::worker_rt::{self, Preload, WorkerOptions};

/// A 48x32 AVIF decoded once before the sandbox so lazily loaded or threaded parts of dav1d and
/// libheif are already initialised when system calls stop being available.
#[cfg(feature = "heif")]
const WARM_AVIF: &[u8] = include_bytes!("../../codecs/tests/fixtures/heif/gradient-444-48x32.avif");

#[cfg(feature = "heif")]
fn preload(opts: &WorkerOptions) -> Result<Preload, String> {
    use auto_crop_codecs::heif;
    // The plugin folder is set before libheif initialises; initialising loads the plugins now.
    heif::configure(opts.plugin_dir.clone());
    heif::init().map_err(|e| e.to_string())?;
    let limits = auto_crop_codecs::DecodeLimits::default();
    auto_crop_codecs::decode_in_process(WARM_AVIF, &limits, true, false)
        .map_err(|e| format!("the warm-up decode failed: {e}"))?;
    Ok(Preload {
        libheif: heif::runtime_version(),
        hevc: heif::have_hevc_decoder(),
        av1: heif::have_av1_decoder(),
    })
}

#[cfg(not(feature = "heif"))]
fn preload(_opts: &WorkerOptions) -> Result<Preload, String> {
    Ok(Preload::default())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--version") {
        println!(
            "auto-crop-worker {} (heif={}, escape-tests={})",
            env!("CARGO_PKG_VERSION"),
            if cfg!(feature = "heif") { "on" } else { "off" },
            if cfg!(feature = "escape-tests") {
                "ON"
            } else {
                "off"
            },
        );
        return;
    }
    let code = worker_rt::run(&args, preload, &|bytes, limits, orient, flatten| {
        auto_crop_codecs::decode_in_process(bytes, limits, orient, flatten)
    });
    std::process::exit(code);
}
