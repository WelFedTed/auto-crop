// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Unix glue shared by Linux and macOS: the file descriptor plumbing of the worker (the pixel
//! arena is inherited as fd 3, the control channel is stdin and a private copy of stdout),
//! `mmap` of the arena, resource limits, and the `pre_exec` hook of the parent.
#![allow(unsafe_code)]

use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::Command;

/// The descriptor number the pixel arena has in the worker.
pub const SHM_FD: RawFd = 3;
/// Exit code of a worker whose allocator ran out (the memory limit).
pub const EXIT_MEMORY_LIMIT: i32 = 77;

/// Arranges for the child of `cmd` to see `shm` as fd 3 (inheritable), and on Linux to be killed
/// when the parent dies.
pub fn inherit_arena_as_fd3(cmd: &mut Command, shm: &File) {
    let fd = shm.as_raw_fd();
    // SAFETY: the closure runs in the forked child before exec and only calls async-signal-safe
    // functions (fcntl, dup2, prctl); it allocates nothing.
    unsafe {
        cmd.pre_exec(move || {
            if fd == SHM_FD {
                let flags = libc::fcntl(SHM_FD, libc::F_GETFD);
                if flags < 0 || libc::fcntl(SHM_FD, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                    return Err(io::Error::last_os_error());
                }
            } else if libc::dup2(fd, SHM_FD) < 0 {
                return Err(io::Error::last_os_error());
            }
            #[cfg(target_os = "linux")]
            {
                // Never outlive the parent: no orphaned workers.
                libc::prctl(
                    libc::PR_SET_PDEATHSIG,
                    libc::SIGKILL as libc::c_ulong,
                    0,
                    0,
                    0,
                );
            }
            Ok(())
        });
    }
}

/// The worker's control channel: fd 0 (the pipe from the pool) and a private duplicate of fd 1,
/// after which fd 1 is pointed at stderr so a library that prints to stdout cannot corrupt the
/// frames.
pub fn control_channel() -> io::Result<(File, File)> {
    // SAFETY: fds 0, 1 and 2 are open (the parent set them up); dup and dup2 have no other
    // preconditions; the new descriptors are owned by the returned Files.
    unsafe {
        let out = libc::dup(1);
        if out < 0 {
            return Err(io::Error::last_os_error());
        }
        let inp = libc::dup(0);
        if inp < 0 {
            let e = io::Error::last_os_error();
            libc::close(out);
            return Err(e);
        }
        if libc::dup2(2, 1) < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((File::from_raw_fd(inp), File::from_raw_fd(out)))
    }
}

/// A writable shared mapping of the arena.
pub struct Mapping {
    ptr: *mut libc::c_void,
    len: usize,
}

// SAFETY: the mapping is plain memory owned by this value; moving it between threads is fine.
unsafe impl Send for Mapping {}

impl Mapping {
    /// Maps `len` bytes of fd 3 read-write. The parent has already sized the file.
    pub fn map(len: usize) -> io::Result<Mapping> {
        if len == 0 {
            return Err(io::Error::other("empty shared memory request"));
        }
        // SAFETY: a fresh shared mapping of an inherited descriptor; the result is checked.
        let p = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                SHM_FD,
                0,
            )
        };
        if p == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Mapping { ptr: p, len })
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY: `ptr` is a live mapping of `len` bytes owned by this value; &mut self makes the
        // slice unique in this process.
        unsafe { std::slice::from_raw_parts_mut(self.ptr.cast::<u8>(), self.len) }
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: unmapping the mapping created in `map`, once.
        unsafe { libc::munmap(self.ptr, self.len) };
    }
}

/// Caps the address space and CPU time of this process and forbids core dumps. Called first
/// thing in the worker (before anything is loaded), so the limits cover the preload too.
/// `address_space` is ignored on macOS, where the kernel does not enforce `RLIMIT_AS`.
pub fn set_resource_limits(address_space: u64, cpu_seconds: u64) -> Result<(), String> {
    let set = |res, v: u64, name: &str| -> Result<(), String> {
        let lim = libc::rlimit {
            rlim_cur: v as libc::rlim_t,
            rlim_max: v as libc::rlim_t,
        };
        // SAFETY: `lim` is a valid rlimit for the duration of the call.
        if unsafe { libc::setrlimit(res, &lim) } != 0 {
            return Err(format!("setrlimit({name}): {}", io::Error::last_os_error()));
        }
        Ok(())
    };
    set(libc::RLIMIT_CORE, 0, "CORE")?;
    if cpu_seconds > 0 {
        set(libc::RLIMIT_CPU, cpu_seconds, "CPU")?;
    }
    #[cfg(target_os = "linux")]
    if address_space > 0 {
        set(libc::RLIMIT_AS, address_space, "AS")?;
    }
    #[cfg(not(target_os = "linux"))]
    let _ = address_space;
    #[cfg(target_os = "linux")]
    {
        // SAFETY: prctl with constant arguments and no pointers.
        unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) };
    }
    Ok(())
}

/// `_exit` without running destructors or atexit handlers (the allocator-failure path).
pub fn exit_now(code: i32) -> ! {
    // SAFETY: `_exit` never returns and has no preconditions.
    unsafe { libc::_exit(code) }
}
