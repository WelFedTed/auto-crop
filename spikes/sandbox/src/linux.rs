// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Linux back end: memfd shared memory, rlimits, Landlock (filesystem + TCP) and a seccomp
//! allow-list. Modes: none (control), rlimit, landlock, seccomp, full (all three layers).

use super::Expect;
use std::collections::BTreeMap;
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::FileExt;
use std::os::unix::process::CommandExt;
use std::process::{Command, Output};

pub type Shm = File;

pub fn modes() -> Vec<&'static str> {
    vec!["none", "rlimit", "landlock", "seccomp", "full"]
}

pub fn shared_create(size: usize) -> io::Result<Shm> {
    let name = std::ffi::CString::new("autocrop-shm").unwrap();
    let fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let f = unsafe { File::from_raw_fd(fd) };
    f.set_len(size as u64)?;
    Ok(f)
}

/// Runs the worker: input through its stdin (an inherited handle, no path), shared memory
/// inherited as fd 3, stdout and stderr captured.
pub fn run(job: &super::Job, shm: &Shm, _mode: &str) -> io::Result<Output> {
    let fd = shm.as_raw_fd();
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.args(&job.args);
    cmd.stdin(std::process::Stdio::from(File::open(job.input_path)?));
    cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    unsafe {
        cmd.pre_exec(move || {
            if fd == 3 {
                let flags = libc::fcntl(3, libc::F_GETFD);
                libc::fcntl(3, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
            } else if libc::dup2(fd, 3) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    cmd.spawn()?.wait_with_output()
}

/// Maps the inherited shared memory (fd 3) and writes the pixels.
pub fn write_shared(data: &[u8]) -> io::Result<()> {
    unsafe {
        let p = libc::mmap(std::ptr::null_mut(), data.len(), libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, 3, 0);
        if p == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        std::ptr::copy_nonoverlapping(data.as_ptr(), p.cast::<u8>(), data.len());
        libc::munmap(p, data.len());
    }
    Ok(())
}

pub fn shared_read(shm: &Shm) -> Vec<u8> {
    let mut buf = vec![0u8; super::PIXELS];
    shm.read_exact_at(&mut buf, 0).expect("read shared memory");
    buf
}

fn set_rlimits() -> Result<(), String> {
    let lim = libc::rlimit { rlim_cur: 512 << 20, rlim_max: 512 << 20 };
    if unsafe { libc::setrlimit(libc::RLIMIT_AS, &lim) } != 0 {
        return Err(format!("setrlimit(AS): {}", io::Error::last_os_error()));
    }
    let cpu = libc::rlimit { rlim_cur: 20, rlim_max: 20 };
    if unsafe { libc::setrlimit(libc::RLIMIT_CPU, &cpu) } != 0 {
        return Err(format!("setrlimit(CPU): {}", io::Error::last_os_error()));
    }
    Ok(())
}

fn apply_landlock() -> Result<String, String> {
    // Test hook: simulate a kernel without Landlock so the fallback is exercised and recorded.
    if std::env::var_os("AUTOCROP_SANDBOX_NO_LANDLOCK").is_some() {
        return Err("disabled by AUTOCROP_SANDBOX_NO_LANDLOCK".to_owned());
    }
    use landlock::{Access, AccessFs, AccessNet, ABI, Ruleset, RulesetAttr, RulesetStatus};
    let abi = ABI::V4; // filesystem + TCP connect/bind (kernel 6.7+); older kernels degrade (best effort)
    let status = Ruleset::default()
        .handle_access(AccessFs::from_all(abi))
        .map_err(|e| e.to_string())?
        .handle_access(AccessNet::from_all(abi))
        .map_err(|e| e.to_string())?
        .create()
        .map_err(|e| e.to_string())?
        .restrict_self()
        .map_err(|e| e.to_string())?;
    Ok(match status.ruleset {
        RulesetStatus::FullyEnforced => "FullyEnforced",
        RulesetStatus::PartiallyEnforced => "PartiallyEnforced",
        RulesetStatus::NotEnforced => "NotEnforced",
    }
    .to_owned())
}

/// Allow-list: only what a pixel-pushing worker needs. Everything else (socket, connect, openat,
/// execve, clone, ptrace, ...) returns EPERM.
fn apply_seccomp() -> Result<(), String> {
    use seccompiler::{BpfProgram, SeccompAction, SeccompFilter, SeccompRule, TargetArch};
    let allowed: &[i64] = &[
        libc::SYS_read,
        libc::SYS_write,
        libc::SYS_readv,
        libc::SYS_writev,
        libc::SYS_close,
        libc::SYS_fstat,
        libc::SYS_lseek,
        libc::SYS_mmap,
        libc::SYS_mprotect,
        libc::SYS_munmap,
        libc::SYS_mremap,
        libc::SYS_brk,
        libc::SYS_madvise,
        libc::SYS_futex,
        libc::SYS_exit,
        libc::SYS_exit_group,
        libc::SYS_rt_sigprocmask,
        libc::SYS_rt_sigaction,
        libc::SYS_rt_sigreturn,
        libc::SYS_sigaltstack,
        libc::SYS_getpid,
        libc::SYS_gettid,
        libc::SYS_clock_gettime,
        libc::SYS_clock_nanosleep,
        libc::SYS_nanosleep,
        libc::SYS_getrandom,
        libc::SYS_ftruncate,
        libc::SYS_pread64,
        libc::SYS_pwrite64,
        libc::SYS_fcntl,
        libc::SYS_dup,
        libc::SYS_sched_yield,
    ];
    let rules: BTreeMap<i64, Vec<SeccompRule>> = allowed.iter().map(|&s| (s, vec![])).collect();
    let arch = if cfg!(target_arch = "aarch64") { TargetArch::aarch64 } else { TargetArch::x86_64 };
    let filter = SeccompFilter::new(rules, SeccompAction::Errno(libc::EPERM as u32), SeccompAction::Allow, arch).map_err(|e| e.to_string())?;
    let bpf: BpfProgram = filter.try_into().map_err(|e: seccompiler::BackendError| e.to_string())?;
    seccompiler::apply_filter(&bpf).map_err(|e| e.to_string())
}

/// Applies the layers of `mode` and describes the level reached.
pub fn apply(mode: &str) -> Result<String, String> {
    let mut level: Vec<String> = Vec::new();
    if matches!(mode, "rlimit" | "full") {
        set_rlimits()?;
        level.push("rlimit".into());
    }
    if matches!(mode, "landlock" | "full") {
        match apply_landlock() {
            Ok(s) => level.push(format!("landlock={s}")),
            Err(e) => level.push(format!("landlock=unavailable({e})")),
        }
    }
    if matches!(mode, "seccomp" | "full") {
        apply_seccomp()?;
        level.push("seccomp".into());
    }
    Ok(if level.is_empty() { "none".to_owned() } else { level.join("+") })
}

pub fn expect(mode: &str, level: &str) -> Expect {
    let ll_full = level.contains("landlock=FullyEnforced");
    let ll_any = ll_full || level.contains("landlock=PartiallyEnforced");
    let by = |c: bool| if c { "blocked" } else { "any" };
    match mode {
        "none" => Expect { file: "allowed", tcp: "allowed", spawn: "allowed", alloc: "allowed" },
        "rlimit" => Expect { file: "allowed", tcp: "allowed", spawn: "allowed", alloc: "blocked" },
        "landlock" => Expect { file: by(ll_any), tcp: by(ll_full), spawn: by(ll_any), alloc: "allowed" },
        "seccomp" => Expect { file: "blocked", tcp: "blocked", spawn: "blocked", alloc: "allowed" },
        _ => Expect { file: "blocked", tcp: "blocked", spawn: "blocked", alloc: "blocked" },
    }
}

/// Nothing to clean up on this OS.
pub fn cleanup() {}
