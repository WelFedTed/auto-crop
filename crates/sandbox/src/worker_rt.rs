// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The worker side (everything `auto-crop-worker` does apart from calling the decoders):
//! start-up order, confinement, the job loop and the reply. The order is the security argument:
//!
//! 1. resource limits (so they cover everything below);
//! 2. **preload**: whatever the decoders read from disk (libheif, its plugin folder, the codec
//!    libraries) is loaded now, because after step 3 nothing can be opened;
//! 3. **confine**: Windows reports the AppContainer, token and job the parent created the process
//!    in; Linux applies rlimits, Landlock and seccomp; macOS enters the Seatbelt profile. The
//!    worker then *probes* the confinement from inside (a file open, a TCP connect and a process
//!    spawn must fail) and puts the result in its `Hello`;
//! 4. only then does the first byte of input arrive (a `Job` frame on stdin).

use crate::os;
use crate::proto::{self, ErrReply, Hello, JobHeader, Kind, OkReply, Reply};
use auto_crop_codecs::{CodecError, DecodeLimits, Decoded};
use std::fs::File;
use std::io::{self, Write};
use std::path::PathBuf;

/// What confinement the worker is asked to apply (Unix; Windows is decided by the parent).
#[derive(Debug, Clone)]
pub struct ConfineConfig {
    /// `RLIMIT_AS` in bytes (Linux; 0 = none).
    pub address_space: u64,
    /// `RLIMIT_CPU` in seconds (0 = none). A backstop only: the pool enforces wall-clock time.
    pub cpu_seconds: u64,
    /// Test hook: pretend the kernel has no Landlock.
    pub no_landlock: bool,
}

/// What [`os::worker_confine`] reached.
#[derive(Debug, Clone)]
pub struct Confinement {
    pub level: String,
    pub probes: String,
}

/// What the preload step found.
#[derive(Debug, Clone, Default)]
pub struct Preload {
    pub libheif: String,
    pub hevc: bool,
    pub av1: bool,
}

/// The worker's command line.
#[derive(Debug, Clone)]
pub struct WorkerOptions {
    pub plugin_dir: Option<PathBuf>,
    pub mem_limit: u64,
    pub cpu_seconds: u64,
    pub max_input: u64,
    pub no_landlock: bool,
}

impl WorkerOptions {
    pub fn parse(args: &[String]) -> Result<WorkerOptions, String> {
        let mut o = WorkerOptions {
            plugin_dir: None,
            mem_limit: 0,
            cpu_seconds: 0,
            max_input: (2 << 30) + (1 << 20),
            no_landlock: false,
        };
        let mut it = args.iter();
        while let Some(a) = it.next() {
            let mut val = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
            match a.as_str() {
                "--plugin-dir" => o.plugin_dir = Some(PathBuf::from(val("--plugin-dir")?)),
                "--mem-limit" => {
                    o.mem_limit = val("--mem-limit")?.parse().map_err(|_| "bad --mem-limit")?;
                }
                "--cpu-secs" => {
                    o.cpu_seconds = val("--cpu-secs")?.parse().map_err(|_| "bad --cpu-secs")?;
                }
                "--max-input" => {
                    o.max_input = val("--max-input")?.parse().map_err(|_| "bad --max-input")?;
                }
                #[cfg(feature = "escape-tests")]
                "--no-landlock" => o.no_landlock = true,
                other => return Err(format!("unknown argument {other}")),
            }
        }
        Ok(o)
    }
}

/// Opens the three escapes of ADR-0006 from inside the confinement and says what happened. A
/// confined worker must see `denied` (or `refused` for a closed port on a platform that does
/// not block the connect) for all three; `OPEN` means the escape worked.
pub fn probe_escapes() -> String {
    use std::net::{SocketAddr, TcpStream};
    use std::process::{Command, Stdio};
    use std::time::Duration;
    // Windows: a system file such as `hosts` is readable by every AppContainer and proves
    // nothing, so the probe lists the user's profile folder (private to the user: an
    // AppContainer or restricted token cannot). Elsewhere: a file every user can read.
    let file = if cfg!(windows) {
        let dir = std::env::var_os("AUTOCROP_PROBE_DIR")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .unwrap_or_default();
        match std::fs::read_dir(&dir) {
            Ok(_) => "OPEN".to_owned(),
            Err(e) => format!("denied({:?})", e.kind()),
        }
    } else {
        let path = if cfg!(target_os = "macos") {
            "/etc/hosts"
        } else {
            "/etc/passwd"
        };
        match File::open(path) {
            Ok(_) => "OPEN".to_owned(),
            Err(e) => format!("denied({:?})", e.kind()),
        }
    };
    let addr: SocketAddr = ([127, 0, 0, 1], 9).into();
    let net = match TcpStream::connect_timeout(&addr, Duration::from_millis(500)) {
        Ok(_) => "OPEN".to_owned(),
        Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => "refused".to_owned(),
        Err(e) if e.kind() == io::ErrorKind::TimedOut => "timeout".to_owned(),
        Err(e) => format!("denied({:?})", e.kind()),
    };
    let (prog, args): (&str, &[&str]) = if cfg!(windows) {
        ("C:\\Windows\\System32\\cmd.exe", &["/C", "exit", "0"])
    } else if cfg!(target_os = "macos") {
        ("/usr/bin/true", &[])
    } else {
        ("/bin/true", &[])
    };
    let spawn = match Command::new(prog)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(_) => "OPEN".to_owned(),
        Err(e) => format!("denied({:?})", e.kind()),
    };
    format!("file={file} net={net} spawn={spawn}")
}

/// The decode the worker runs: bytes, limits, apply the orientation, flatten alpha.
pub type DecodeFn = dyn Fn(&[u8], &DecodeLimits, bool, bool) -> Result<Decoded, CodecError>;

#[cfg(unix)]
type Pixels = os::ffi::unix::Mapping;
#[cfg(windows)]
type Pixels = os::ffi::windows::Mapping;

fn map_pixels(h: &JobHeader, len: usize) -> io::Result<Pixels> {
    #[cfg(unix)]
    {
        let _ = h;
        Pixels::map(len)
    }
    #[cfg(windows)]
    {
        Pixels::map(h.shm_handle, len)
    }
}

fn release_pixels_handle(h: &JobHeader) {
    #[cfg(windows)]
    if h.shm_handle != 0 {
        os::ffi::windows::release_handle(h.shm_handle);
    }
    #[cfg(unix)]
    let _ = h;
}

/// Test builds only: a marker after the image bytes (`AUTOCROP-FAULT:<kind>`) makes the worker
/// misbehave the way a compromised or buggy decoder could, so the pool's crash isolation can be
/// shown on every OS (ROADMAP M6.10).
#[cfg(feature = "escape-tests")]
fn inject_fault(input: &[u8]) {
    const MARK: &[u8] = b"AUTOCROP-FAULT:";
    let Some(at) = input.windows(MARK.len()).rposition(|w| w == MARK) else {
        return;
    };
    let kind = String::from_utf8_lossy(&input[at + MARK.len()..]).into_owned();
    match kind.trim() {
        "abort" => std::process::abort(),
        "exit" => std::process::exit(0),
        "hang" => loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
        },
        "alloc" => {
            // An infallible allocation far past any limit: the process dies in the allocator.
            let v = vec![1u8; 48 << 30];
            std::hint::black_box(&v);
        }
        "panic" => panic!("injected panic"),
        _ => {}
    }
}

fn run_job(payload: &[u8], decode: &DecodeFn) -> Reply {
    let (header, input) = match JobHeader::decode(payload) {
        Ok(x) => x,
        Err(e) => {
            return Reply::Err(proto::err_to_wire(
                0,
                &CodecError::Corrupt(format!("bad job: {e}")),
            ));
        }
    };
    let id = header.job_id;
    #[cfg(feature = "escape-tests")]
    inject_fault(input);
    let decoded = match decode(
        input,
        &header.limits,
        header.flags & proto::FLAG_APPLY_ORIENTATION != 0,
        header.flags & proto::FLAG_FLATTEN_ALPHA != 0,
    ) {
        Ok(d) => d,
        Err(e) => {
            release_pixels_handle(&header);
            return Reply::Err(proto::err_to_wire(id, &e));
        }
    };
    let data = &decoded.raster.data;
    if data.len() as u64 > header.shm_len {
        release_pixels_handle(&header);
        return Reply::Err(proto::err_to_wire(
            id,
            &CodecError::Corrupt("the decoded image does not fit the shared memory".into()),
        ));
    }
    match map_pixels(&header, data.len()) {
        Ok(mut m) => m.as_mut_slice().copy_from_slice(data),
        Err(e) => {
            return Reply::Err(proto::err_to_wire(
                id,
                &CodecError::InternalPanic(format!("shared memory: {e}")),
            ));
        }
    }
    Reply::Ok(OkReply {
        job_id: id,
        width: decoded.raster.width,
        height: decoded.raster.height,
        exif_orientation: decoded.exif_orientation,
        source_bit_depth: decoded.source_bit_depth,
        frames: decoded.frames,
        notices: decoded.notices.iter().map(|s| (*s).to_owned()).collect(),
        icc: decoded.icc,
    })
}

#[cfg(feature = "escape-tests")]
fn escape_report() -> Option<String> {
    // Test builds only (never in a shipped worker): after the sandbox is applied, attempt each
    // escape and report. `AUTOCROP_ESCAPE=<path of a secret file>,<tcp port>`.
    let spec = std::env::var("AUTOCROP_ESCAPE").ok()?;
    let (path, port) = spec.rsplit_once(',')?;
    let file = match File::open(path) {
        Ok(_) => "allowed".to_owned(),
        Err(e) => format!("blocked({:?})", e.kind()),
    };
    let addr: std::net::SocketAddr = ([127, 0, 0, 1], port.parse().ok()?).into();
    let tcp =
        match std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(1500)) {
            Ok(_) => "allowed".to_owned(),
            Err(e) => format!("blocked({:?})", e.kind()),
        };
    let (prog, args): (&str, &[&str]) = if cfg!(windows) {
        ("C:\\Windows\\System32\\cmd.exe", &["/C", "exit", "0"])
    } else if cfg!(target_os = "macos") {
        ("/usr/bin/true", &[])
    } else {
        ("/bin/true", &[])
    };
    let spawn = match std::process::Command::new(prog)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .status()
    {
        Ok(_) => "allowed".to_owned(),
        Err(e) => format!("blocked({:?})", e.kind()),
    };
    let alloc = {
        let mut v: Vec<u8> = Vec::new();
        match v.try_reserve_exact(1 << 30) {
            Ok(()) => {
                v.resize(1 << 30, 1);
                "allowed".to_owned()
            }
            Err(_) => "blocked(alloc)".to_owned(),
        }
    };
    Some(format!(
        "file {file}\ntcp {tcp}\nspawn {spawn}\nalloc {alloc}"
    ))
}

/// Runs the worker to the end; returns the process exit code. `preload` runs before the
/// confinement; `decode` is the only thing that touches input.
pub fn run(
    args: &[String],
    preload: impl FnOnce(&WorkerOptions) -> Result<Preload, String>,
    decode: &DecodeFn,
) -> i32 {
    let opts = match WorkerOptions::parse(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("auto-crop-worker: {e}");
            return 2;
        }
    };
    let cfg = ConfineConfig {
        address_space: opts.mem_limit,
        cpu_seconds: opts.cpu_seconds,
        no_landlock: opts.no_landlock,
    };
    #[cfg(unix)]
    if let Err(e) = os::ffi::unix::set_resource_limits(cfg.address_space, cfg.cpu_seconds) {
        eprintln!("auto-crop-worker: {e}");
        return 3;
    }
    #[cfg(unix)]
    let channel = os::ffi::unix::control_channel();
    #[cfg(windows)]
    let channel = os::ffi::windows::control_channel();
    let (mut input, mut output) = match channel {
        Ok(c) => c,
        Err(e) => {
            eprintln!("auto-crop-worker: no control channel: {e}");
            return 3;
        }
    };
    let pre = match preload(&opts) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("auto-crop-worker: preload failed: {e}");
            return 4;
        }
    };
    let confinement = match os::worker_confine(&cfg) {
        Ok(c) => c,
        Err(e) => {
            // No input has been read: refuse to run unconfined.
            eprintln!("auto-crop-worker: could not apply the sandbox: {e}");
            return 5;
        }
    };
    let hello = Hello {
        pid: std::process::id(),
        level: confinement.level,
        probes: confinement.probes,
        libheif: pre.libheif,
        hevc: pre.hevc,
        av1: pre.av1,
    };
    if proto::write_frame(&mut output, Kind::Hello, &hello.encode(), &[]).is_err() {
        return 6;
    }
    #[cfg(feature = "escape-tests")]
    if let Some(text) = escape_report() {
        let _ = proto::write_frame(&mut output, Kind::Escape, text.as_bytes(), &[]);
    }
    let max_input = opts.max_input;
    loop {
        let frame = proto::read_frame(&mut input, |k| match k {
            Kind::Job => max_input,
            _ => 0,
        });
        let (kind, payload) = match frame {
            Ok(f) => f,
            // The pool went away or closed the pipe: the normal end of a worker.
            Err(proto::ProtoError::Closed) => return 0,
            Err(e) => {
                eprintln!("auto-crop-worker: bad frame: {e}");
                return 7;
            }
        };
        match kind {
            Kind::Shutdown => return 0,
            Kind::Job => {
                #[cfg(feature = "escape-tests")]
                {
                    // The one fault that needs the control channel itself: noise instead of a frame.
                    let m: &[u8] = b"AUTOCROP-FAULT:garbage";
                    if payload.windows(m.len()).any(|w| w == m) {
                        let _ =
                            output.write_all(b"this is not a frame at all, just noise, 16+ bytes");
                        let _ = output.flush();
                        std::thread::sleep(std::time::Duration::from_secs(30));
                    }
                }
                let id = JobHeader::decode(&payload).map_or(0, |(h, _)| h.job_id);
                let reply = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_job(&payload, decode)
                }))
                .unwrap_or_else(|_| {
                    Reply::Err(ErrReply {
                        job_id: id,
                        code: 9,
                        a: 0,
                        b: 0,
                        text: "the worker panicked".into(),
                    })
                });
                drop(payload);
                if write_reply(&mut output, &reply).is_err() {
                    return 6;
                }
            }
            _ => {
                eprintln!("auto-crop-worker: unexpected frame");
                return 7;
            }
        }
    }
}

fn write_reply(out: &mut File, reply: &Reply) -> io::Result<()> {
    proto::write_frame(out, Kind::Result, &reply.encode(), &[])?;
    out.flush()
}
