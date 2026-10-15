// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Per-OS parts: how the pool starts a worker process (parent side), how pixels come back, and
//! how a worker confines itself (worker side). The rest of the crate sees only the types below.

use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub mod ffi;

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub use unix::{attempt_names, launch, worker_confine};
#[cfg(windows)]
pub use windows::{attempt_names, delete_profile, launch, worker_confine};

/// What to start.
#[derive(Debug, Clone)]
pub struct LaunchSpec {
    pub exe: PathBuf,
    /// Arguments of the worker (the plugin folder is added by [`launch`]).
    pub args: Vec<String>,
    /// Extra environment (the worker starts with a nearly empty one).
    pub env: Vec<(OsString, OsString)>,
    /// Memory limit the worker is given (job object on Windows, `RLIMIT_AS` on Linux).
    pub memory_limit: u64,
    /// Folder of the libheif plugins (the HEVC decoder), when there is one.
    pub plugin_dir: Option<PathBuf>,
    /// Index into [`attempt_names`]: 0 is the strongest confinement, higher ones are the
    /// reported fallbacks.
    pub attempt: usize,
}

/// How a worker process ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Exit {
    pub code: Option<i64>,
    pub signal: Option<i32>,
}

/// What an [`Exit`] means for the pool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitClass {
    /// The allocator of the worker ran out (the memory limit).
    MemoryLimit,
    /// The CPU limit ended the worker.
    CpuLimit,
    /// Anything else: the text says how it ended.
    Crash(String),
}

/// A running worker process.
pub trait Proc: Send + Sync {
    fn pid(&self) -> u32;
    /// Ends the process (and, on Windows, its job).
    fn kill(&self);
    fn try_wait(&self) -> Option<Exit>;
    fn wait_timeout(&self, d: Duration) -> Option<Exit>;
    /// Resident memory, where the OS gives it to another process cheaply (macOS watchdog).
    fn resident_bytes(&self) -> Option<u64>;
}

/// The parent's side of the pixel memory of one worker.
pub trait Shm: Send {
    /// Makes `len` bytes available to the worker for the next job; returns the handle value to
    /// put in the job header (0 where the descriptor is inherited).
    fn prepare(&mut self, len: u64, child: &dyn Proc) -> io::Result<u64>;
    /// Copies the first `len` bytes out.
    fn take(&mut self, len: usize) -> io::Result<Vec<u8>>;
    /// Gives the memory back.
    fn release(&mut self);
}

/// A started worker with its pipes.
pub struct Launched {
    pub stdin: Box<dyn Write + Send>,
    pub stdout: Box<dyn Read + Send>,
    pub stderr: Box<dyn Read + Send>,
    pub proc: Arc<dyn Proc>,
    pub shm: Box<dyn Shm>,
}

/// Classifies an exit for the current OS.
pub fn classify(exit: Exit) -> ExitClass {
    #[cfg(unix)]
    {
        if exit.code == Some(i64::from(ffi::unix::EXIT_MEMORY_LIMIT)) {
            return ExitClass::MemoryLimit;
        }
        if let Some(sig) = exit.signal {
            let name = match sig {
                libc::SIGXCPU => return ExitClass::CpuLimit,
                libc::SIGSYS => "SIGSYS: a system call the sandbox does not allow",
                libc::SIGSEGV => "SIGSEGV",
                libc::SIGABRT => "SIGABRT",
                libc::SIGBUS => "SIGBUS",
                libc::SIGKILL => "SIGKILL",
                libc::SIGILL => "SIGILL",
                libc::SIGFPE => "SIGFPE",
                _ => "a signal",
            };
            return ExitClass::Crash(format!("killed by signal {sig} ({name})"));
        }
        ExitClass::Crash(format!("exited with code {}", exit.code.unwrap_or(-1)))
    }
    #[cfg(windows)]
    {
        let code = exit.code.unwrap_or(-1) as u32;
        // The allocator's own exit code, or the kernel's "not enough memory / commit limit".
        if matches!(
            code,
            ffi::windows::EXIT_MEMORY_LIMIT | 0xC000_0017 | 0xC000_012D
        ) {
            return ExitClass::MemoryLimit;
        }
        let name = match code {
            0xC000_0005 => " (access violation)",
            0xC000_00FD => " (stack overflow)",
            0xC000_0409 => " (fast fail / abort)",
            _ => "",
        };
        ExitClass::Crash(format!("exited with code {code:#x}{name}"))
    }
}
