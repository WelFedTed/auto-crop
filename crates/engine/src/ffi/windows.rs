// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Win32 bindings of the safe-write path. See the module docs of [`super`].

use std::ffi::c_void;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// `REPLACEFILE_IGNORE_MERGE_ERRORS`: if the replaced file's attributes or ACL cannot be carried
/// over (no `WRITE_DAC`, say) the replace still happens, because a verified output beats no output.
const REPLACEFILE_IGNORE_MERGE_ERRORS: u32 = 0x2;

// SAFETY: the two declarations match the Win32 prototypes of `ReplaceFileW` and
// `GetDiskFreeSpaceExW` in `kernel32` (`BOOL` is `i32`, `DWORD` is `u32`, `LPCWSTR` is a pointer to
// NUL-terminated UTF-16, `PULARGE_INTEGER` is a pointer to a `u64`), which every Windows version
// the app supports exports. Calling them is `unsafe` only because they take raw pointers, which
// the wrappers below pass from live, correctly sized buffers.
#[link(name = "kernel32")]
unsafe extern "system" {
    fn ReplaceFileW(
        replaced: *const u16,
        replacement: *const u16,
        backup: *const u16,
        flags: u32,
        exclude: *mut c_void,
        reserved: *mut c_void,
    ) -> i32;

    fn GetDiskFreeSpaceExW(
        directory: *const u16,
        free_to_caller: *mut u64,
        total: *mut u64,
        total_free: *mut u64,
    ) -> i32;
}

/// The extended-length (`\\?\`) spelling of `p`, made absolute first: no 260-character limit, and
/// no Win32 name normalisation (trailing dots and spaces, reserved device names) on the way.
pub fn verbatim(p: &Path) -> PathBuf {
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let s = abs.to_string_lossy();
    if s.starts_with(r"\\?\") {
        abs
    } else if let Some(unc) = s.strip_prefix(r"\\") {
        PathBuf::from(format!(r"\\?\UNC\{unc}"))
    } else {
        PathBuf::from(format!(r"\\?\{s}"))
    }
}

/// A NUL-terminated UTF-16 copy of the verbatim spelling of `p`. A path that already holds a NUL
/// is refused (it would silently truncate).
fn wide(p: &Path) -> io::Result<Vec<u16>> {
    let mut v: Vec<u16> = verbatim(p).as_os_str().encode_wide().collect();
    if v.contains(&0) {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    v.push(0);
    Ok(v)
}

/// Replaces `replaced` with `replacement` (both on one volume) the way `ReplaceFileW` does: the
/// replacement takes the name, creation time, ACL and named streams of the replaced file and keeps
/// its own contents and last-write time. No backup file is asked for. Errors 1175 to 1177 mean the
/// replace stopped part-way; the caller decides what that leaves (see `commit::swap`).
pub fn replace_file(replaced: &Path, replacement: &Path) -> io::Result<()> {
    let (a, b) = (wide(replaced)?, wide(replacement)?);
    // SAFETY: `a` and `b` are NUL-terminated UTF-16 buffers that live until the call returns; the
    // backup name is null (no backup), and `exclude` and `reserved` must be null per the docs.
    // The function reads the buffers only and has no other preconditions.
    let ok = unsafe {
        ReplaceFileW(
            a.as_ptr(),
            b.as_ptr(),
            std::ptr::null(),
            REPLACEFILE_IGNORE_MERGE_ERRORS,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if ok != 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Bytes available to the calling user on the volume that holds `dir` (quotas respected).
pub fn available_space(dir: &Path) -> io::Result<u64> {
    let d = wide(dir)?;
    let mut free = 0u64;
    // SAFETY: `d` is a NUL-terminated UTF-16 buffer that outlives the call; `free` is a valid,
    // aligned `u64` the function writes to; the two other out-pointers are documented as optional
    // and are null.
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            d.as_ptr(),
            &mut free,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if ok != 0 {
        Ok(free)
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbatim_spells_drive_and_unc_paths() {
        assert_eq!(
            verbatim(Path::new(r"C:\a\b.jpg")),
            PathBuf::from(r"\\?\C:\a\b.jpg")
        );
        assert_eq!(
            verbatim(Path::new(r"\\?\C:\a\b.jpg")),
            PathBuf::from(r"\\?\C:\a\b.jpg")
        );
        assert_eq!(
            verbatim(Path::new(r"\\srv\share\a.jpg")),
            PathBuf::from(r"\\?\UNC\srv\share\a.jpg")
        );
    }

    #[test]
    fn a_nul_in_a_path_is_refused() {
        assert!(wide(Path::new("a\0b")).is_err());
    }

    #[test]
    fn replace_keeps_the_new_contents_and_the_name() {
        let d = tempfile::tempdir().unwrap();
        let (t, n) = (d.path().join("target.bin"), d.path().join("new.bin"));
        std::fs::write(&t, b"old").unwrap();
        std::fs::write(&n, b"new").unwrap();
        replace_file(&t, &n).unwrap();
        assert_eq!(std::fs::read(&t).unwrap(), b"new");
        assert!(!n.exists());
    }

    #[test]
    fn free_space_is_reported_for_a_folder() {
        let d = tempfile::tempdir().unwrap();
        assert!(available_space(d.path()).unwrap() > 0);
    }
}
