// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Linux confinement of the worker (ADR-0006, ADR-0013): resource limits, a Landlock ruleset that
//! handles every filesystem and TCP access and grants none, and a seccomp allow-list. Applied
//! *before* the first byte of input is read, after everything the decoders need from disk
//! (libheif, its plugins, the codec libraries) has been loaded. Each layer that cannot be applied
//! is named in the level text; none is skipped silently and the seccomp filter is never switched
//! off (a failure to install it is an error and the worker exits without reading any input).
//!
//! The allow-list is what a pixel-pushing worker with decoder threads needs: memory management,
//! futexes, signals for panics, clocks, thread creation (`clone` only with `CLONE_THREAD`, so a
//! process cannot be forked; `clone3` answers `ENOSYS` and glibc falls back to `clone`), and
//! `read`/`write` on the descriptors that are already open. `openat`, `socket`, `connect`,
//! `execve`, `ptrace`, `mount` and the rest answer `EPERM`. It was widened by test until real
//! libheif, dav1d and libde265 decodes worked (`docs/testing/sandbox-worker.md`).

use crate::worker_rt::{ConfineConfig, Confinement, probe_escapes};
use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
    SeccompRule, TargetArch, apply_filter_all_threads,
};
use std::collections::BTreeMap;

/// `CLONE_THREAD`: the only kind of `clone` the worker may do.
const CLONE_THREAD: u64 = 0x0001_0000;

/// The syscalls a decode needs, by name for the report and by number for the filter.
fn allowed() -> Vec<(&'static str, i64)> {
    macro_rules! sys {
        ($($n:ident),* $(,)?) => { vec![$((stringify!($n), libc::$n)),*] };
    }
    #[allow(unused_mut)]
    let mut v = sys![
        SYS_read,
        SYS_write,
        SYS_readv,
        SYS_writev,
        SYS_close,
        SYS_fstat,
        SYS_newfstatat,
        SYS_lseek,
        SYS_mmap,
        SYS_mprotect,
        SYS_munmap,
        SYS_mremap,
        SYS_brk,
        SYS_madvise,
        SYS_futex,
        SYS_exit,
        SYS_exit_group,
        SYS_rt_sigprocmask,
        SYS_rt_sigaction,
        SYS_rt_sigreturn,
        SYS_sigaltstack,
        SYS_getpid,
        SYS_gettid,
        SYS_tgkill,
        SYS_clock_gettime,
        SYS_clock_getres,
        SYS_clock_nanosleep,
        SYS_nanosleep,
        SYS_gettimeofday,
        SYS_getrandom,
        SYS_pread64,
        SYS_pwrite64,
        SYS_fcntl,
        SYS_dup,
        SYS_sched_yield,
        SYS_sched_getaffinity,
        SYS_set_robust_list,
        SYS_rseq,
        SYS_set_tid_address,
        SYS_getrlimit,
        SYS_getrusage,
        SYS_uname,
        SYS_ioctl,
    ];
    v.sort_by_key(|(_, n)| *n);
    v
}

fn arch() -> TargetArch {
    if cfg!(target_arch = "aarch64") {
        TargetArch::aarch64
    } else {
        TargetArch::x86_64
    }
}

/// Installs the allow-list on every thread of the process. Returns the number of syscalls
/// allowed without conditions.
fn apply_seccomp() -> Result<usize, String> {
    let list = allowed();
    let mut rules: BTreeMap<i64, Vec<SeccompRule>> = BTreeMap::new();
    for (_, n) in &list {
        rules.insert(*n, vec![]);
    }
    // ioctl: only the terminal-size style queries libraries make on stderr (TCGETS / FIONREAD).
    rules.insert(
        libc::SYS_ioctl,
        vec![
            SeccompRule::new(vec![
                SeccompCondition::new(1, SeccompCmpArgLen::Dword, SeccompCmpOp::Eq, 0x5401)
                    .map_err(|e| e.to_string())?,
            ])
            .map_err(|e| e.to_string())?,
            SeccompRule::new(vec![
                SeccompCondition::new(1, SeccompCmpArgLen::Dword, SeccompCmpOp::Eq, 0x541B)
                    .map_err(|e| e.to_string())?,
            ])
            .map_err(|e| e.to_string())?,
        ],
    );
    // clone: threads only (CLONE_THREAD set); never a new process.
    rules.insert(
        libc::SYS_clone,
        vec![
            SeccompRule::new(vec![
                SeccompCondition::new(
                    0,
                    SeccompCmpArgLen::Qword,
                    SeccompCmpOp::MaskedEq(CLONE_THREAD),
                    CLONE_THREAD,
                )
                .map_err(|e| e.to_string())?,
            ])
            .map_err(|e| e.to_string())?,
        ],
    );
    // prctl: naming a thread (PR_SET_NAME 15, PR_GET_NAME 16) and naming anonymous memory
    // (PR_SET_VMA 0x53564d41) only.
    rules.insert(
        libc::SYS_prctl,
        [15u64, 16, 0x5356_4d41]
            .iter()
            .map(|op| {
                SeccompRule::new(vec![
                    SeccompCondition::new(0, SeccompCmpArgLen::Dword, SeccompCmpOp::Eq, *op)
                        .map_err(|e| e.to_string())?,
                ])
                .map_err(|e| e.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?,
    );
    // clone3 is allowed by this filter and answered ENOSYS by the filter below, so glibc falls
    // back to clone (whose flags the rule above can inspect; clone3 passes them in memory).
    rules.insert(libc::SYS_clone3, vec![]);
    let main: BpfProgram = SeccompFilter::new(
        rules,
        SeccompAction::Errno(libc::EPERM as u32),
        SeccompAction::Allow,
        arch(),
    )
    .map_err(|e| e.to_string())?
    .try_into()
    .map_err(|e: seccompiler::BackendError| e.to_string())?;
    let mut enosys_rules: BTreeMap<i64, Vec<SeccompRule>> = BTreeMap::new();
    enosys_rules.insert(libc::SYS_clone3, vec![]);
    let enosys: BpfProgram = SeccompFilter::new(
        enosys_rules,
        SeccompAction::Allow,
        SeccompAction::Errno(libc::ENOSYS as u32),
        arch(),
    )
    .map_err(|e| e.to_string())?
    .try_into()
    .map_err(|e: seccompiler::BackendError| e.to_string())?;
    // Both programs apply; the more restrictive answer of the two wins for every syscall.
    apply_filter_all_threads(&enosys).map_err(|e| e.to_string())?;
    apply_filter_all_threads(&main).map_err(|e| e.to_string())?;
    Ok(list.len())
}

fn apply_landlock(cfg: &ConfineConfig) -> Result<String, String> {
    if cfg.no_landlock {
        // Test hook: simulate a kernel without Landlock so the fallback is exercised and recorded.
        return Err("disabled by the test hook".to_owned());
    }
    use landlock::{ABI, Access, AccessFs, AccessNet, Ruleset, RulesetAttr, RulesetStatus};
    let abi = ABI::V4; // filesystem + TCP connect and bind (kernel 6.7+); older kernels degrade
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

/// Applies the layers in order and describes the level reached.
pub fn confine(cfg: &ConfineConfig) -> Result<Confinement, String> {
    crate::os::ffi::unix::set_resource_limits(cfg.address_space, cfg.cpu_seconds)?;
    let mut level = vec![format!(
        "rlimit(as={}MiB,cpu={}s,core=0)",
        cfg.address_space >> 20,
        cfg.cpu_seconds
    )];
    match apply_landlock(cfg) {
        Ok(s) => level.push(format!("landlock={s}")),
        Err(e) => level.push(format!("landlock=unavailable({e})")),
    }
    let n = apply_seccomp().map_err(|e| format!("seccomp: {e}"))?;
    level.push(format!("seccomp({n} syscalls)"));
    Ok(Confinement {
        level: level.join("+"),
        probes: probe_escapes(),
    })
}
