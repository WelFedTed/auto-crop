// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! macOS glue: the Seatbelt `sandbox_init` call (deprecated by Apple, still what Chromium and
//! Firefox use for their renderers) and a resident-size reading of another process for the
//! parent's memory watchdog (`RLIMIT_AS` is not enforced on macOS).
#![allow(unsafe_code)]

use std::ffi::{CStr, CString, c_char, c_int};

// SAFETY: the two declarations match the signatures in <sandbox.h> (`sandbox_init` and
// `sandbox_free_error`, both in libSystem); they are only called from `enter` below.
unsafe extern "C" {
    fn sandbox_init(profile: *const c_char, flags: u64, errorbuf: *mut *mut c_char) -> c_int;
    fn sandbox_free_error(errorbuf: *mut c_char);
}

/// Enters the sandbox described by the Seatbelt `profile` text (flags 0: the argument is a
/// profile, not a named one). Irreversible for this process.
pub fn enter(profile: &str) -> Result<(), String> {
    let profile = CString::new(profile).map_err(|e| e.to_string())?;
    let mut err: *mut c_char = std::ptr::null_mut();
    // SAFETY: `profile` is a valid NUL-terminated string for the call; `err` is a valid
    // out-pointer; the error string, when set, is read once and freed with sandbox_free_error.
    unsafe {
        if sandbox_init(profile.as_ptr(), 0, &mut err) == 0 {
            return Ok(());
        }
        let msg = if err.is_null() {
            "unknown".to_owned()
        } else {
            let m = CStr::from_ptr(err).to_string_lossy().into_owned();
            sandbox_free_error(err);
            m
        };
        Err(format!("sandbox_init failed: {msg}"))
    }
}

/// Resident memory of `pid` in bytes (best effort: `None` when the process is gone or the call
/// is refused).
pub fn resident_bytes(pid: u32) -> Option<u64> {
    let mut info: libc::proc_taskinfo = {
        // SAFETY: proc_taskinfo is plain old data; all-zero is a valid value.
        unsafe { std::mem::zeroed() }
    };
    let size = std::mem::size_of::<libc::proc_taskinfo>() as c_int;
    // SAFETY: `info` is a valid buffer of `size` bytes for PROC_PIDTASKINFO.
    let n = unsafe {
        libc::proc_pidinfo(
            pid as c_int,
            libc::PROC_PIDTASKINFO,
            0,
            (&raw mut info).cast(),
            size,
        )
    };
    (n == size).then_some(info.pti_resident_size)
}
