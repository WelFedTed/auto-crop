// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The sandboxed decode worker pool (decision D3, B12, ADR-0006, ADR-0013).
//!
//! HEIC and AVIF files are untrusted input parsed by C libraries (libheif, dav1d, libde265).
//! This crate runs them in `auto-crop-worker` processes that confine themselves before the first
//! byte of input arrives, with pixel, memory and time caps, crash isolation and cancellation:
//!
//! * [`proto`]: the framed, versioned control channel (no general serialisation of untrusted data);
//! * [`pool`]: the pool client (spawn on demand, reuse, replace, time out, back-pressure);
//! * [`worker_rt`]: what the worker binary does, shared so the worker is a thin `main`;
//! * `os`: starting a worker per OS (AppContainer + job object, restricted token, Landlock +
//!   seccomp, Seatbelt) and the shared-memory mechanism (file mapping, `memfd`, unlinked file).
//!
//! The application installs a [`PoolDecoder`] as the `auto-crop-codecs` remote decoder at
//! start-up ([`install`]); `AUTOCROP_DECODE=inproc|worker|auto` switches the behaviour.

pub mod os;
pub mod pool;
pub mod proto;
pub mod worker_rt;

pub use pool::{PoolConfig, PoolFailure, PoolStatus, Tier, WorkerPool, tier_of, with_cancel};

use auto_crop_codecs::{CodecError, Decoded, Format, RemoteDecoder, RemoteRequest};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

/// File name of the worker executable.
pub const WORKER_NAME: &str = if cfg!(windows) {
    "auto-crop-worker.exe"
} else {
    "auto-crop-worker"
};

/// `AUTOCROP_DECODE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Decode in this process (no worker).
    InProc,
    /// Every decodable format goes through the worker; no fallback.
    Worker,
    /// HEIC and AVIF go through the worker when it is found beside the executable and can start;
    /// otherwise in this process, reported (default).
    Auto,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Mode> {
        match s.trim().to_ascii_lowercase().as_str() {
            "inproc" | "in-process" | "off" => Some(Mode::InProc),
            "worker" | "on" => Some(Mode::Worker),
            "auto" | "" => Some(Mode::Auto),
            _ => None,
        }
    }

    pub fn from_env() -> Mode {
        std::env::var("AUTOCROP_DECODE")
            .ok()
            .and_then(|v| Mode::parse(&v))
            .unwrap_or(Mode::Auto)
    }

    pub fn name(self) -> &'static str {
        match self {
            Mode::InProc => "inproc",
            Mode::Worker => "worker",
            Mode::Auto => "auto",
        }
    }
}

/// The worker beside `exe`, if there is one (every package layout puts it in the executable's
/// folder: Windows zip, `Contents/MacOS`, Linux `bin`).
pub fn worker_beside(exe: &Path) -> Option<PathBuf> {
    let p = exe.parent()?.join(WORKER_NAME);
    p.is_file().then_some(p)
}

/// The remote decoder the application installs.
pub struct PoolDecoder {
    mode: Mode,
    pool: Option<WorkerPool>,
    worker: Option<PathBuf>,
    /// Why decoding happens in this process, when it does.
    reason: std::sync::Mutex<Option<String>>,
    fell_back: AtomicBool,
    noticed: AtomicBool,
}

impl PoolDecoder {
    fn note_in_process(&self, reason: &str) {
        if let Ok(mut r) = self.reason.lock() {
            *r = Some(reason.to_owned());
        }
        if !self.noticed.swap(true, Ordering::Relaxed) {
            eprintln!(
                "auto-crop: HEIC and AVIF files are decoded inside this process, without the \
                 sandboxed worker ({reason}). Sandbox level: none."
            );
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn pool(&self) -> Option<&WorkerPool> {
        self.pool.as_ref()
    }

    pub fn worker_path(&self) -> Option<&Path> {
        self.worker.as_deref()
    }

    /// True when decodes currently run in this process.
    pub fn in_process(&self) -> bool {
        self.pool.is_none() || self.fell_back.load(Ordering::Relaxed)
    }

    pub fn reason(&self) -> Option<String> {
        self.reason.lock().ok().and_then(|r| r.clone())
    }
}

impl RemoteDecoder for PoolDecoder {
    fn handles(&self, format: Format) -> bool {
        match self.mode {
            Mode::InProc => false,
            Mode::Worker => format.is_decodable(),
            Mode::Auto => matches!(format, Format::Heic | Format::Avif),
        }
    }

    fn decode(&self, req: RemoteRequest<'_>) -> Result<Decoded, CodecError> {
        let in_process = |req: &RemoteRequest<'_>| {
            auto_crop_codecs::decode_in_process(
                req.bytes,
                req.limits,
                req.apply_orientation,
                req.flatten_alpha,
            )
        };
        let Some(pool) = &self.pool else {
            self.note_in_process(
                self.reason()
                    .as_deref()
                    .unwrap_or("the worker was not found beside the executable"),
            );
            return in_process(&req);
        };
        if self.fell_back.load(Ordering::Relaxed) {
            return in_process(&req);
        }
        match pool.decode(
            req.bytes,
            req.limits,
            req.apply_orientation,
            req.flatten_alpha,
        ) {
            Ok(d) => Ok(d),
            Err(PoolFailure::Codec(e)) => Err(e),
            Err(PoolFailure::Unavailable(why)) => {
                if self.mode == Mode::Auto {
                    self.fell_back.store(true, Ordering::Relaxed);
                    self.note_in_process(&format!("the worker could not start: {why}"));
                    in_process(&req)
                } else {
                    Err(CodecError::WorkerCrashed(format!(
                        "the sandboxed decoder could not start: {why}"
                    )))
                }
            }
        }
    }
}

static INSTALLED: OnceLock<Arc<PoolDecoder>> = OnceLock::new();

/// What [`install`] needs from the application.
#[derive(Debug, Clone)]
pub struct InstallOptions {
    /// The running executable (the worker is looked for beside it).
    pub exe: PathBuf,
    /// The libheif plugin folder of the package, if any.
    pub plugin_dir: Option<PathBuf>,
    /// Cap on the bytes in flight (the engine's memory budget cap).
    pub inflight_cap: Option<u64>,
    /// Overrides `AUTOCROP_DECODE` (tests).
    pub mode: Option<Mode>,
}

/// Installs the pool as the remote decoder of `auto-crop-codecs` (once per process; later calls
/// return the first). With `AUTOCROP_DECODE=inproc` nothing is installed. Starts no worker: the
/// first HEIC or AVIF decode does.
pub fn install(opts: InstallOptions) -> Arc<PoolDecoder> {
    INSTALLED
        .get_or_init(|| {
            let mode = opts.mode.unwrap_or_else(Mode::from_env);
            let worker = worker_beside(&opts.exe);
            let pool = match (&worker, mode) {
                (Some(w), Mode::Auto | Mode::Worker) => {
                    let mut cfg = PoolConfig::new(w.clone());
                    cfg.plugin_dir = opts.plugin_dir.clone();
                    if let Some(c) = opts.inflight_cap {
                        cfg.inflight_cap = c.max(1 << 28);
                    }
                    Some(WorkerPool::new(cfg))
                }
                _ => None,
            };
            let d = Arc::new(PoolDecoder {
                mode,
                pool,
                worker,
                reason: std::sync::Mutex::new(match mode {
                    Mode::InProc => Some("AUTOCROP_DECODE=inproc".to_owned()),
                    _ => None,
                }),
                fell_back: AtomicBool::new(false),
                noticed: AtomicBool::new(false),
            });
            if d.pool.is_none()
                && mode != Mode::InProc
                && let Ok(mut r) = d.reason.lock()
            {
                *r = Some("the worker was not found beside the executable".to_owned());
            }
            if mode == Mode::Worker && d.pool.is_none() {
                // Forced worker mode without a worker: decodes fail loudly instead of silently
                // running unsandboxed.
                eprintln!("auto-crop: AUTOCROP_DECODE=worker but no worker was found beside the executable");
            }
            if mode != Mode::InProc {
                auto_crop_codecs::set_remote_decoder(Some(d.clone()));
            }
            d
        })
        .clone()
}

/// The installed decoder, if [`install`] ran.
pub fn installed() -> Option<Arc<PoolDecoder>> {
    INSTALLED.get().cloned()
}

/// The facts `doctor` prints about decoding and the sandbox.
#[derive(Debug, Clone)]
pub struct SandboxInfo {
    pub mode: Mode,
    pub worker: Option<PathBuf>,
    /// True when HEIC and AVIF decode in the application's own process.
    pub in_process: bool,
    pub reason: Option<String>,
    /// What a probe worker reported (starts one when `probe` is true).
    pub status: Option<PoolStatus>,
    pub probe_error: Option<String>,
}

impl SandboxInfo {
    /// The level text, or `none`.
    pub fn level(&self) -> String {
        match (&self.status, self.in_process) {
            (Some(s), false) => s.level.clone().unwrap_or_else(|| "none".into()),
            _ => "none".into(),
        }
    }
}

/// Describes decoding and the sandbox; with `probe` a worker is started to read its own report.
pub fn describe(probe: bool) -> SandboxInfo {
    let Some(d) = installed() else {
        return SandboxInfo {
            mode: Mode::InProc,
            worker: None,
            in_process: true,
            reason: Some("AUTOCROP_DECODE=inproc or no pool installed".into()),
            status: None,
            probe_error: None,
        };
    };
    let (mut status, mut probe_error) = (None, None);
    if let Some(pool) = d.pool() {
        if probe {
            match pool.probe() {
                Ok(s) => status = Some(s),
                Err(e) => probe_error = Some(e),
            }
        } else {
            status = Some(pool.status());
        }
    }
    let in_process = d.in_process() || (probe && probe_error.is_some() && d.mode != Mode::Worker);
    SandboxInfo {
        mode: d.mode,
        worker: d.worker.clone(),
        in_process,
        reason: d.reason().or_else(|| probe_error.clone()),
        status,
        probe_error,
    }
}
