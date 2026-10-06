// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The free-space preflight (ROADMAP M2.34, M2.83; PLAN 2.7 "Preflight"): before any byte of a
//! save is written, the volumes involved must hold twice what the save needs (a verified backup
//! copy of the original on the store's volume, the temp output beside the target) and never fall
//! below a floor of free space. A shortfall is `DiskFull` before anything exists, so nothing needs
//! undoing; the CLI maps it to exit 6, the GUI pauses and offers to free space.
//!
//! The numbers are PROVISIONAL (PLAN 2.7): the 2x headroom and the 500 MB floor.

use crate::error::{ErrKind, Result};
use std::cell::Cell;
use std::io;
use std::path::Path;

/// The floor: a save never starts when the volume would be left with less than this free.
pub const MIN_FREE_BYTES: u64 = 500 * 1024 * 1024;
/// Free space must cover this many times what a save writes (PLAN 2.7).
pub const HEADROOM: u64 = 2;

thread_local! {
    /// Test hook: pretend every volume has this many bytes free (`with_free_space`).
    static FAKE_FREE: Cell<Option<u64>> = const { Cell::new(None) };
}

/// Runs `f` with every free-space query on this thread answering `bytes`. For tests of the
/// disk-full paths; production code never calls it.
#[doc(hidden)]
pub fn with_free_space<R>(bytes: u64, f: impl FnOnce() -> R) -> R {
    struct Reset(Option<u64>);
    impl Drop for Reset {
        fn drop(&mut self) {
            FAKE_FREE.with(|c| c.set(self.0));
        }
    }
    let _reset = Reset(FAKE_FREE.with(|c| c.replace(Some(bytes))));
    f()
}

/// Bytes available to this user on the volume that holds `path` (a folder, or a file inside one).
pub fn available_space(path: &Path) -> io::Result<u64> {
    if let Some(b) = FAKE_FREE.with(Cell::get) {
        return Ok(b);
    }
    let dir = nearest_dir(path);
    #[cfg(windows)]
    {
        crate::ffi::windows::available_space(&dir)
    }
    #[cfg(unix)]
    {
        let s = rustix::fs::statvfs(&dir)?;
        Ok(s.f_bavail.saturating_mul(s.f_frsize))
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = dir;
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }
}

/// `path` if it is a folder, else its parent, else the closest existing ancestor (the target folder
/// of a first copy does not exist yet).
fn nearest_dir(path: &Path) -> std::path::PathBuf {
    let mut p = path.to_path_buf();
    loop {
        if p.is_dir() {
            return p;
        }
        match p.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => p = parent.to_path_buf(),
            _ => return std::path::PathBuf::from("."),
        }
    }
}

/// Do both paths sit on one volume? Compares the drive or share prefix on Windows and the device
/// number on Unix. A wrong "no" only makes the check stricter; a mount point or junction can make
/// it a wrong "yes" on Windows, which the real write then reports as `DiskFull`.
fn same_volume(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (
            std::fs::metadata(nearest_dir(a)),
            std::fs::metadata(nearest_dir(b)),
        ) {
            (Ok(x), Ok(y)) => x.dev() == y.dev(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        let prefix = |p: &Path| {
            std::path::absolute(p).ok().and_then(|p| {
                p.components()
                    .next()
                    .map(|c| c.as_os_str().to_ascii_lowercase())
            })
        };
        matches!((prefix(a), prefix(b)), (Some(x), Some(y)) if x == y)
    }
}

/// What a save is about to write.
#[derive(Debug, Clone, Copy)]
pub struct Need<'a> {
    /// The folder of the output (the temp is written there).
    pub target_dir: &'a Path,
    /// Bytes the outputs may take (the source's size is a fair estimate).
    pub output_bytes: u64,
    /// The backup store's folder and the bytes the backup copy takes; `None` for a save that
    /// makes no backup (a copy, or a re-save that reuses one).
    pub backup: Option<(&'a Path, u64)>,
    /// The floor of free space that must remain; [`MIN_FREE_BYTES`] unless a test lowers it.
    pub floor: u64,
}

impl<'a> Need<'a> {
    pub fn new(target_dir: &'a Path, output_bytes: u64, backup: Option<(&'a Path, u64)>) -> Self {
        Self {
            target_dir,
            output_bytes,
            backup,
            floor: MIN_FREE_BYTES,
        }
    }
}

/// Checks the headroom. `Err(DiskFull)` names a volume too full to start; an unreadable volume is
/// not a reason to refuse (the real write will say).
pub fn preflight(n: &Need<'_>) -> Result<()> {
    let enough = |dir: &Path, bytes: u64| -> bool {
        match available_space(dir) {
            // Twice the bytes written, and never less than the floor left afterwards.
            Ok(free) => {
                free >= bytes.saturating_mul(HEADROOM) && free.saturating_sub(bytes) >= n.floor
            }
            Err(_) => true,
        }
    };
    match n.backup {
        Some((store, backup_bytes)) if same_volume(store, n.target_dir) => {
            if !enough(n.target_dir, n.output_bytes.saturating_add(backup_bytes)) {
                return Err(ErrKind::DiskFull);
            }
        }
        Some((store, backup_bytes)) => {
            if !enough(store, backup_bytes) || !enough(n.target_dir, n.output_bytes) {
                return Err(ErrKind::DiskFull);
            }
        }
        None => {
            if !enough(n.target_dir, n.output_bytes) {
                return Err(ErrKind::DiskFull);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_real_volume_reports_some_free_space() {
        let d = tempfile::tempdir().unwrap();
        assert!(available_space(d.path()).unwrap() > 0);
        // A folder that does not exist yet is measured on its closest existing parent.
        assert!(available_space(&d.path().join("a").join("b")).unwrap() > 0);
    }

    #[test]
    fn a_save_needs_twice_its_bytes_and_a_floor() {
        let d = tempfile::tempdir().unwrap();
        let mb = 1024 * 1024;
        let mut need = Need::new(d.path(), 10 * mb, Some((d.path(), 10 * mb)));
        need.floor = 100 * mb;
        // 20 MB written: twice that is 40 MB, and 100 MB must remain after it, so 120 MB.
        with_free_space(119 * mb, || {
            assert_eq!(preflight(&need), Err(ErrKind::DiskFull));
        });
        with_free_space(121 * mb, || assert_eq!(preflight(&need), Ok(())));
        // Below the floor even a tiny save is refused.
        let tiny = Need {
            floor: 100 * mb,
            ..Need::new(d.path(), 1, None)
        };
        with_free_space(50 * mb, || {
            assert_eq!(preflight(&tiny), Err(ErrKind::DiskFull));
        });
        with_free_space(0, || assert_eq!(preflight(&tiny), Err(ErrKind::DiskFull)));
        with_free_space(200 * mb, || assert_eq!(preflight(&tiny), Ok(())));
    }

    #[test]
    fn headroom_is_twice_the_bytes_even_with_a_zero_floor() {
        let d = tempfile::tempdir().unwrap();
        let need = Need {
            floor: 0,
            ..Need::new(d.path(), 1000, None)
        };
        with_free_space(1999, || {
            assert_eq!(preflight(&need), Err(ErrKind::DiskFull))
        });
        with_free_space(2000, || assert_eq!(preflight(&need), Ok(())));
    }

    #[test]
    fn the_fake_is_scoped_to_the_closure() {
        let d = tempfile::tempdir().unwrap();
        with_free_space(0, || assert_eq!(available_space(d.path()).unwrap(), 0));
        assert!(available_space(d.path()).unwrap() > 0);
    }
}
