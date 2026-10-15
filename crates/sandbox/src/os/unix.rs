// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Linux and macOS: the worker is an ordinary child process; it confines itself (rlimits,
//! Landlock, seccomp, Seatbelt) before it reads a byte of input, and reports the level it
//! reached. The pixel arena is an anonymous file inherited as fd 3 that the parent resizes for
//! each job (`memfd` on Linux, an unlinked temp file on macOS).

use super::{Exit, LaunchSpec, Launched, Proc, Shm, ffi};
use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt;
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Variables the worker keeps from the parent: the library search paths of a development
/// build (a packaged build clears them and relies on its RUNPATH or rpath) and the libheif
/// plugin override. Everything else is dropped.
const PASS_THROUGH: &[&str] = &[
    "LD_LIBRARY_PATH",
    "DYLD_LIBRARY_PATH",
    "DYLD_FALLBACK_LIBRARY_PATH",
    "LIBHEIF_PLUGIN_PATH",
];

/// One attempt: the worker confines itself and reports what it reached.
pub fn attempt_names() -> &'static [&'static str] {
    &["self-confining worker"]
}

#[cfg(target_os = "linux")]
fn create_arena() -> io::Result<File> {
    let fd = rustix::fs::memfd_create("autocrop-pixels", rustix::fs::MemfdFlags::CLOEXEC)?;
    Ok(File::from(fd))
}

#[cfg(target_os = "macos")]
fn create_arena() -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let path = std::env::temp_dir().join(format!(
        "autocrop-pixels-{}-{}-{nanos}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    // Unlinked at once: only the open descriptors (ours and the worker's) keep it alive.
    std::fs::remove_file(&path)?;
    Ok(f)
}

struct Arena {
    file: File,
}

impl Shm for Arena {
    fn prepare(&mut self, len: u64, _child: &dyn Proc) -> io::Result<u64> {
        self.file.set_len(len)?;
        Ok(0)
    }

    fn take(&mut self, len: usize) -> io::Result<Vec<u8>> {
        let mut v: Vec<u8> = Vec::new();
        v.try_reserve_exact(len)
            .map_err(|_| io::Error::from(io::ErrorKind::OutOfMemory))?;
        v.resize(len, 0);
        self.file.read_exact_at(&mut v, 0)?;
        Ok(v)
    }

    fn release(&mut self) {
        // Truncating to zero returns the pages at once; the next job sizes it again.
        let _ = self.file.set_len(0);
    }
}

struct UnixProc {
    pid: u32,
    child: Mutex<Child>,
}

fn exit_of(status: std::process::ExitStatus) -> Exit {
    Exit {
        code: status.code().map(i64::from),
        signal: status.signal(),
    }
}

impl Proc for UnixProc {
    fn pid(&self) -> u32 {
        self.pid
    }

    fn kill(&self) {
        if let Ok(mut c) = self.child.lock() {
            let _ = c.kill();
        }
    }

    fn try_wait(&self) -> Option<Exit> {
        let mut c = self.child.lock().ok()?;
        c.try_wait().ok().flatten().map(exit_of)
    }

    fn wait_timeout(&self, d: Duration) -> Option<Exit> {
        let t0 = Instant::now();
        loop {
            if let Some(e) = self.try_wait() {
                return Some(e);
            }
            if t0.elapsed() >= d {
                return None;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn resident_bytes(&self) -> Option<u64> {
        #[cfg(target_os = "macos")]
        {
            ffi::macos::resident_bytes(self.pid)
        }
        #[cfg(not(target_os = "macos"))]
        {
            None
        }
    }
}

impl Drop for UnixProc {
    fn drop(&mut self) {
        // Reap the child so a replaced worker never lingers as a zombie.
        if let Ok(mut c) = self.child.lock() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

pub fn launch(spec: &LaunchSpec) -> io::Result<Launched> {
    let arena = create_arena()?;
    let mut cmd = Command::new(&spec.exe);
    cmd.args(&spec.args);
    if let Some(d) = &spec.plugin_dir {
        cmd.arg("--plugin-dir").arg(d);
    }
    cmd.env_clear();
    for k in PASS_THROUGH {
        if let Some(v) = std::env::var_os(k) {
            cmd.env(k, v);
        }
    }
    // One malloc arena per worker keeps the address-space limit meaningful with many threads.
    cmd.env("MALLOC_ARENA_MAX", "2");
    for (k, v) in &spec.env {
        cmd.env(k, v);
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    ffi::unix::inherit_arena_as_fd3(&mut cmd, &arena);
    let mut child = cmd.spawn()?;
    let pid = child.id();
    let stdin = child.stdin.take().expect("piped");
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
    Ok(Launched {
        stdin: Box::new(stdin),
        stdout: Box::new(stdout),
        stderr: Box::new(stderr),
        proc: std::sync::Arc::new(UnixProc {
            pid,
            child: Mutex::new(child),
        }),
        shm: Box::new(Arena { file: arena }),
    })
}

/// The worker side: applies the sandbox of this OS and reports (level, probes).
pub fn worker_confine(
    cfg: &crate::worker_rt::ConfineConfig,
) -> Result<crate::worker_rt::Confinement, String> {
    #[cfg(target_os = "linux")]
    {
        super::linux::confine(cfg)
    }
    #[cfg(target_os = "macos")]
    {
        super::macos::confine(cfg)
    }
}
