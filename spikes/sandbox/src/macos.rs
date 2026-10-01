// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! macOS back end: unlinked temp file as shared memory and `sandbox_init` with a deny-default
//! Seatbelt profile (a deprecated but working API, also used by Chromium and others).
//! Modes: none (control), sandbox. RLIMIT_AS is not enforced on macOS, so alloc is not checked.

use super::Expect;
use std::ffi::{CStr, CString, c_char, c_int};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::FileExt;
use std::os::unix::process::CommandExt;
use std::process::{Command, Output};

pub type Shm = File;

unsafe extern "C" {
    fn sandbox_init(profile: *const c_char, flags: u64, errorbuf: *mut *mut c_char) -> c_int;
    fn sandbox_free_error(errorbuf: *mut c_char);
}

pub fn modes() -> Vec<&'static str> {
    vec!["none", "sandbox"]
}

pub fn shared_create(size: usize) -> io::Result<Shm> {
    let mut template = std::env::temp_dir().join("autocrop-shm-XXXXXX").into_os_string().into_encoded_bytes();
    template.push(0);
    let fd = unsafe { libc::mkstemp(template.as_mut_ptr().cast()) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    unsafe { libc::unlink(template.as_ptr().cast()) };
    let f = unsafe { File::from_raw_fd(fd) };
    f.set_len(size as u64)?;
    Ok(f)
}

pub fn prepare(cmd: &mut Command, shm: &Shm, _mode: &str) {
    let fd = shm.as_raw_fd();
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
}

pub fn run(mut cmd: Command, _mode: &str) -> io::Result<Output> {
    cmd.spawn()?.wait_with_output()
}

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

pub fn apply(mode: &str) -> Result<String, String> {
    if mode != "sandbox" {
        return Ok("none".to_owned());
    }
    // deny everything; the worker only needs its already-open stdin, stdout and shared memory.
    let profile = CString::new("(version 1)\n(deny default)\n").unwrap();
    let mut err: *mut c_char = std::ptr::null_mut();
    let rc = unsafe { sandbox_init(profile.as_ptr(), 0, &mut err) };
    if rc != 0 {
        let msg = if err.is_null() { "unknown".to_owned() } else { unsafe { CStr::from_ptr(err).to_string_lossy().into_owned() } };
        if !err.is_null() {
            unsafe { sandbox_free_error(err) };
        }
        return Err(format!("sandbox_init failed: {msg}"));
    }
    Ok("sandbox_init(deny default)".to_owned())
}

pub fn expect(mode: &str, _level: &str) -> Expect {
    if mode == "sandbox" {
        Expect { file: "blocked", tcp: "blocked", spawn: "blocked", alloc: "any" }
    } else {
        Expect { file: "allowed", tcp: "allowed", spawn: "allowed", alloc: "allowed" }
    }
}
