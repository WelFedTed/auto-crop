// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The write half of the commit protocol (PLAN 2.7): encode to a temp file in the target
//! directory, make it durable, verify it, then swap it into place. The caller backs the original
//! up between verify and swap. Two properties hold at every instant: the target holds either the
//! original bytes or a fully verified output, and a verified backup exists before any swap.
//!
//! Differences from the full design, tracked in ROADMAP M2: no SQLite journal (the backup
//! manifest carries the state), no crash-recovery sweep of orphan temp files, the swap is
//! `std::fs::rename` (POSIX semantics on Windows 10 1607+) rather than `ReplaceFileW`, and no
//! free-space preflight.

use crate::error::{ErrKind, Result};
use crate::util::{blake3_hex, new_id};
use auto_crop_codecs::Format;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone)]
pub struct TempWrite {
    pub path: PathBuf,
    pub blake3: String,
    pub size: u64,
}

impl TempWrite {
    /// Removes the temp file; used on every failure path.
    pub fn discard(&self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Writes `bytes` to a new `.autocrop-<id>.tmp` in `dir`, applies `mtime` if given, and syncs.
pub fn write_temp(dir: &Path, bytes: &[u8], mtime: Option<SystemTime>) -> Result<TempWrite> {
    let path = dir.join(format!(".autocrop-{}.tmp", new_id()));
    let write = || -> std::io::Result<()> {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        f.write_all(bytes)?;
        if let Some(t) = mtime {
            f.set_modified(t)?;
        }
        f.sync_all()
    };
    if let Err(e) = write() {
        let _ = fs::remove_file(&path);
        return Err(ErrKind::from_io(&e));
    }
    Ok(TempWrite {
        path,
        blake3: blake3_hex(bytes),
        size: bytes.len() as u64,
    })
}

/// Re-reads the temp file and checks it is what was encoded (catches truncation and bit flips),
/// then decodes it and checks it is the expected image.
pub fn verify_temp(t: &TempWrite, expect: (u32, u32), format: Format) -> Result<()> {
    let bytes = fs::read(&t.path).map_err(|_| ErrKind::VerifyFailed)?;
    if bytes.len() as u64 != t.size || blake3_hex(&bytes) != t.blake3 {
        return Err(ErrKind::VerifyFailed);
    }
    let decoded = auto_crop_codecs::decode(&bytes).map_err(|_| ErrKind::VerifyFailed)?;
    if decoded.format != format || (decoded.raster.width, decoded.raster.height) != expect {
        return Err(ErrKind::VerifyFailed);
    }
    Ok(())
}

/// Sharing violations from antivirus, indexers and thumbnail caches are retried at 10 to 640 ms
/// before giving up as a retryable `FileInUse`.
const RETRY_MS: [u64; 7] = [10, 20, 40, 80, 160, 320, 640];

/// Replaces `target` with the verified temp file (atomic on the same volume).
pub fn swap(temp: &Path, target: &Path) -> Result<()> {
    let mut last = None;
    for (i, wait) in std::iter::once(&0u64).chain(RETRY_MS.iter()).enumerate() {
        if i > 0 {
            std::thread::sleep(Duration::from_millis(*wait));
        }
        match fs::rename(temp, target) {
            Ok(()) => return Ok(()),
            Err(e) => {
                let kind = ErrKind::from_io(&e);
                let retryable = matches!(kind, ErrKind::FileInUse | ErrKind::ReadOnly)
                    || e.kind() == std::io::ErrorKind::PermissionDenied;
                last = Some(kind);
                if !retryable {
                    break;
                }
            }
        }
    }
    match last {
        Some(ErrKind::ReadOnly) => Err(ErrKind::FileInUse),
        Some(k) => Err(k),
        None => Err(ErrKind::Internal),
    }
}

/// A free name beside `desired`: `name.ext`, then `name (2).ext`, `name (3).ext`...
pub fn free_name(desired: &Path) -> PathBuf {
    if !desired.exists() {
        return desired.to_path_buf();
    }
    let stem = desired
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = desired
        .extension()
        .map(|e| e.to_string_lossy().into_owned());
    let dir = desired.parent().unwrap_or(Path::new("."));
    for n in 2..10_000 {
        let name = match &ext {
            Some(e) => format!("{stem} ({n}).{e}"),
            None => format!("{stem} ({n})"),
        };
        let p = dir.join(name);
        if !p.exists() {
            return p;
        }
    }
    desired.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;
    use auto_crop_imgproc::Raster;

    fn png(w: u32, h: u32) -> Vec<u8> {
        auto_crop_codecs::encode(&Raster::filled(w, h, [10, 200, 30]), Format::Png, 90, None)
            .unwrap()
    }

    #[test]
    fn write_verify_swap_replaces_the_target_and_leaves_no_temp() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("a.png");
        fs::write(&target, b"old").unwrap();
        let bytes = png(8, 6);
        let t = write_temp(dir.path(), &bytes, None).unwrap();
        verify_temp(&t, (8, 6), Format::Png).unwrap();
        swap(&t.path, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), bytes);
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(".autocrop-"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn verification_catches_a_truncated_or_wrong_file() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = png(8, 6);
        let t = write_temp(dir.path(), &bytes, None).unwrap();
        // Wrong dimensions expected.
        assert_eq!(
            verify_temp(&t, (9, 6), Format::Png),
            Err(ErrKind::VerifyFailed)
        );
        // Truncated on disk after the write.
        fs::write(&t.path, &bytes[..bytes.len() / 2]).unwrap();
        assert_eq!(
            verify_temp(&t, (8, 6), Format::Png),
            Err(ErrKind::VerifyFailed)
        );
        // Wrong format.
        let t2 = write_temp(dir.path(), &bytes, None).unwrap();
        assert_eq!(
            verify_temp(&t2, (8, 6), Format::Jpeg),
            Err(ErrKind::VerifyFailed)
        );
    }

    #[test]
    fn mtime_is_carried_to_the_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let when = SystemTime::UNIX_EPOCH + Duration::from_secs(1_500_000_000);
        let t = write_temp(dir.path(), b"x", Some(when)).unwrap();
        assert_eq!(fs::metadata(&t.path).unwrap().modified().unwrap(), when);
    }

    #[test]
    fn free_name_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("scan.jpg");
        assert_eq!(free_name(&p), p);
        fs::write(&p, b"1").unwrap();
        assert_eq!(free_name(&p), dir.path().join("scan (2).jpg"));
        fs::write(dir.path().join("scan (2).jpg"), b"2").unwrap();
        assert_eq!(free_name(&p), dir.path().join("scan (3).jpg"));
    }
}
