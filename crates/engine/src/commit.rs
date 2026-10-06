// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The file-level steps of the commit protocol (PLAN 2.7): encode to a temp file in the target
//! directory, make it durable, verify it, then swap it into place. The protocol that orders these
//! steps, journals them and recovers from a crash is [`crate::group`] (it serves both the one-file
//! in-place save and the N-file group); this module holds the pieces it calls.
//!
//! * **Temp write** ([`write_temp`], [`write_temp_at`]): `create_new`, mtime applied, permissions
//!   of the file being replaced copied, `sync_all`.
//! * **Verify** ([`verify_temp`], [`verify_temp_expect`]): re-read the temp and match the encoder's
//!   blake3 (truncation and bit flips), then re-decode and check the size, the format, that the
//!   EXIF Orientation is 1 (it is applied once, never twice), that the ICC profile is byte-exact, and
//!   the content: exact pixel hash for lossless output, a 64x64 luma fingerprint within tolerance
//!   for lossy output. [`VerifyMode::Fast`] decodes JPEG at 1/8 scale (the whole entropy stream is
//!   still decoded, so truncation and corruption are still caught).
//! * **Swap** ([`swap`]): `ReplaceFileW` on Windows (keeps the replaced file's creation time, DACL
//!   and named streams), `rename` elsewhere and wherever `ReplaceFileW` is unavailable, with the
//!   10 to 640 ms retry ladder for antivirus and indexer sharing violations, and the documented
//!   handling of the part-way failures 1175 to 1177.

use crate::error::{ErrKind, Result};
use crate::util::{blake3_hex, new_id};
use auto_crop_codecs::{DecodeLimits, Format};
use auto_crop_core::ports::Want;
use auto_crop_imgproc::Raster;
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

/// Writes `bytes` to `path` (which must not exist), applies `mtime`, copies the permissions of
/// `mode_from` if it exists, and syncs. On failure nothing is left behind.
pub fn write_temp_at(
    path: &Path,
    bytes: &[u8],
    mtime: Option<SystemTime>,
    mode_from: Option<&Path>,
) -> Result<()> {
    let write = || -> std::io::Result<()> {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        f.write_all(bytes)?;
        if let Some(t) = mtime {
            f.set_modified(t)?;
        }
        f.sync_all()?;
        drop(f);
        // Best effort: a rename keeps the new file's mode, so the old file's mode is copied (the
        // Windows swap keeps attributes and ACLs by itself). Read-only is skipped: a read-only
        // temp could not be replaced or removed again.
        if let Some(meta) = mode_from.and_then(|p| fs::metadata(p).ok())
            && !meta.permissions().readonly()
        {
            let _ = fs::set_permissions(path, meta.permissions());
        }
        Ok(())
    };
    write().map_err(|e| {
        let _ = fs::remove_file(path);
        ErrKind::from_io(&e)
    })
}

/// Writes `bytes` to a new `.autocrop-<id>.tmp` in `dir`, applies `mtime` if given, and syncs.
pub fn write_temp(dir: &Path, bytes: &[u8], mtime: Option<SystemTime>) -> Result<TempWrite> {
    let path = dir.join(format!(".autocrop-{}.tmp", new_id()));
    write_temp_at(&path, bytes, mtime, None)?;
    Ok(TempWrite {
        path,
        blake3: blake3_hex(bytes),
        size: bytes.len() as u64,
    })
}

// ------------------------------------------------------------------ verification

/// How thoroughly the re-decode of a temp checks the pixels (PLAN 2.7 "Verification modes"). There
/// is no `Off`: no setting, flag or environment variable can skip verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VerifyMode {
    /// Decode at full size.
    #[default]
    Full,
    /// Decode JPEG at 1/8 size (still runs the whole Huffman stream); lossless formats are
    /// identical to `Full`.
    Fast,
}

/// Mean absolute luma difference allowed between the encoder's input and the decoded temp
/// (PROVISIONAL, PLAN 2.7: 3/255).
pub const LUMA_MEAN_TOLERANCE: f64 = 3.0;
/// No single cell of the fingerprint may differ by more than this (a bit flip or a damaged scan
/// moves a cell far more; honest JPEG noise on a hard edge moves one by a few levels).
pub const LUMA_MAX_TOLERANCE: u8 = 24;

/// What the decoded temp must contain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    /// Only the size, format, orientation and ICC are checked.
    None,
    /// Lossless output: the blake3 of the decoded RGB8 samples ([`pixel_hash`]) must be equal.
    Pixels(String),
    /// Lossy output: the 64x64 luma fingerprint ([`luma_fingerprint`]) must be close.
    Luma(Vec<u8>),
}

/// What an output must look like once decoded again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expect {
    pub dims: (u32, u32),
    pub format: Format,
    /// The ICC profile that must come back byte-exact; `None` = not checked.
    pub icc: Option<Vec<u8>>,
    pub content: Content,
}

impl Expect {
    /// The size and the format only (the early slice's check).
    pub fn basic(dims: (u32, u32), format: Format) -> Self {
        Self {
            dims,
            format,
            icc: None,
            content: Content::None,
        }
    }

    /// The full expectation for output made from `raster`: exact pixels for a lossless format, the
    /// luma fingerprint for a lossy one, and the profile that was embedded.
    pub fn for_raster(raster: &Raster, format: Format, icc: Option<&[u8]>) -> Self {
        Self {
            dims: (raster.width, raster.height),
            format,
            icc: icc.map(<[u8]>::to_vec),
            content: if format == Format::Png {
                Content::Pixels(pixel_hash(raster))
            } else {
                Content::Luma(luma_fingerprint(raster))
            },
        }
    }
}

/// blake3 of the RGB8 samples, row by row.
pub fn pixel_hash(r: &Raster) -> String {
    blake3_hex(&r.data)
}

/// Cells per axis of the fingerprint grid: at most 64, and cells are at least 8 px so JPEG's own
/// block noise averages out.
fn grid(n: u32) -> usize {
    (n / 8).clamp(1, 64) as usize
}

/// The luma fingerprint of an image: the mean Rec.601 luma of a `gx` x `gy` grid of cells covering
/// it (`gx`, `gy` at most 64, cells at least 8 px). Comparable between the full image and a 1/8
/// decode of it because the grid is taken over the whole picture, not over pixels.
pub fn luma_fingerprint(r: &Raster) -> Vec<u8> {
    luma_fingerprint_grid(r, grid(r.width), grid(r.height))
}

/// The grid a full-size image of `dims` is fingerprinted on (so a reduced decode of it can be
/// fingerprinted on the same cells).
pub fn grid_of(dims: (u32, u32)) -> (usize, usize) {
    (grid(dims.0), grid(dims.1))
}

/// [`luma_fingerprint`] on an explicit `gx` x `gy` grid.
pub fn luma_fingerprint_grid(r: &Raster, gx: usize, gy: usize) -> Vec<u8> {
    let (w, h) = (r.width as usize, r.height as usize);
    let (gx, gy) = (gx.clamp(1, w.max(1)), gy.clamp(1, h.max(1)));
    let mut out = Vec::with_capacity(gx * gy);
    for j in 0..gy {
        let (y0, y1) = (j * h / gy, ((j + 1) * h / gy).max(j * h / gy + 1).min(h));
        for i in 0..gx {
            let (x0, x1) = (i * w / gx, ((i + 1) * w / gx).max(i * w / gx + 1).min(w));
            let mut sum = 0u64;
            for y in y0..y1 {
                let row = &r.data[(y * w + x0) * 3..(y * w + x1) * 3];
                for px in row.as_chunks::<3>().0 {
                    sum +=
                        (299 * u64::from(px[0]) + 587 * u64::from(px[1]) + 114 * u64::from(px[2]))
                            / 1000;
                }
            }
            let n = ((y1 - y0) * (x1 - x0)).max(1) as u64;
            out.push(((sum + n / 2) / n) as u8);
        }
    }
    out
}

/// Are two fingerprints close enough? Different grids (a different size) never are.
pub fn luma_close(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() || a.is_empty() {
        return false;
    }
    let (mut sum, mut max) = (0u64, 0u8);
    for (x, y) in a.iter().zip(b) {
        let d = x.abs_diff(*y);
        sum += u64::from(d);
        max = max.max(d);
    }
    max <= LUMA_MAX_TOLERANCE && (sum as f64 / a.len() as f64) <= LUMA_MEAN_TOLERANCE
}

fn limits_for(dims: (u32, u32)) -> DecodeLimits {
    // The size was already admitted when the source was opened; verification must not refuse what
    // the encoder just made because the cap moved.
    let pixels = u64::from(dims.0) * u64::from(dims.1);
    DecodeLimits::default().with_max_pixels(pixels.max(auto_crop_codecs::DEFAULT_MAX_PIXELS))
}

/// Re-reads the temp file and checks it is what was encoded (catches truncation and bit flips),
/// then decodes it and checks it is the expected image. The early check: size and format.
pub fn verify_temp(t: &TempWrite, expect: (u32, u32), format: Format) -> Result<()> {
    verify_temp_expect(t, &Expect::basic(expect, format), VerifyMode::Full)
}

/// [`verify_temp`] with the full expectation of PLAN 2.7 step 3.
pub fn verify_temp_expect(t: &TempWrite, e: &Expect, mode: VerifyMode) -> Result<()> {
    let bytes = fs::read(&t.path).map_err(|_| ErrKind::VerifyFailed)?;
    if bytes.len() as u64 != t.size || blake3_hex(&bytes) != t.blake3 {
        return Err(ErrKind::VerifyFailed);
    }
    verify_bytes(&bytes, e, mode)
}

/// The decode half of the verification, on bytes whose hash was already matched.
pub fn verify_bytes(bytes: &[u8], e: &Expect, mode: VerifyMode) -> Result<()> {
    let limits = limits_for(e.dims);
    let fast = mode == VerifyMode::Fast && e.format == Format::Jpeg;
    let (format, dims, orientation, icc, raster) = if fast {
        let d = auto_crop_codecs::decode_scaled(bytes, Want::Scaled { min_edge: 64 }, &limits)
            .map_err(|_| ErrKind::VerifyFailed)?;
        (
            d.decoded.format,
            (d.source_width, d.source_height),
            d.decoded.exif_orientation,
            d.decoded.icc,
            d.decoded.raster,
        )
    } else {
        let d = auto_crop_codecs::decode_with(bytes, &limits).map_err(|_| ErrKind::VerifyFailed)?;
        let dims = (d.raster.width, d.raster.height);
        (d.format, dims, d.exif_orientation, d.icc, d.raster)
    };
    if format != e.format || dims != e.dims {
        return Err(ErrKind::VerifyFailed);
    }
    // The orientation is applied once, to the pixels; an output that still asks for a turn would
    // be turned again by every viewer.
    if orientation != 1 {
        return Err(ErrKind::VerifyFailed);
    }
    if let Some(want) = &e.icc
        && icc.as_deref() != Some(want.as_slice())
    {
        return Err(ErrKind::VerifyFailed);
    }
    match &e.content {
        Content::None => {}
        Content::Pixels(h) => {
            if pixel_hash(&raster) != *h {
                return Err(ErrKind::VerifyFailed);
            }
        }
        Content::Luma(want) => {
            // On the grid of the full-size picture, whatever size was decoded.
            let (gx, gy) = grid_of(e.dims);
            if !luma_close(&luma_fingerprint_grid(&raster, gx, gy), want) {
                return Err(ErrKind::VerifyFailed);
            }
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ the swap

/// Sharing violations from antivirus, indexers and thumbnail caches are retried at 10 to 640 ms
/// before giving up as a retryable `FileInUse` (PLAN 2.7, PROVISIONAL).
pub const RETRY_MS: [u64; 7] = [10, 20, 40, 80, 160, 320, 640];

/// ERROR_UNABLE_TO_REMOVE_REPLACED, ERROR_UNABLE_TO_MOVE_REPLACEMENT, ERROR_UNABLE_TO_MOVE_REPLACEMENT_2:
/// `ReplaceFileW` stopped part-way.
const REPLACE_PARTWAY: std::ops::RangeInclusive<i32> = 1175..=1177;

/// Is this I/O error worth retrying after a pause (a sharing violation or a part-way replace)?
pub fn retryable(e: &std::io::Error) -> bool {
    matches!(ErrKind::from_io(e), ErrKind::FileInUse | ErrKind::ReadOnly)
        || e.kind() == std::io::ErrorKind::PermissionDenied
        || e.raw_os_error()
            .is_some_and(|c| REPLACE_PARTWAY.contains(&c))
}

/// What to do after `ReplaceFileW` failed with `e` (PLAN 2.7 step 6). Errors 1175 to 1177 say the
/// replace stopped part-way: if the target is gone, the verified temp is renamed into its place (the
/// backup exists, so nothing is lost); if it is still there it is still the old file, so the caller
/// retries. Errors that mean "`ReplaceFileW` does not work on this file system" fall back to a
/// plain rename. Anything else is returned as it is.
pub fn after_failed_replace(e: std::io::Error, temp: &Path, target: &Path) -> std::io::Result<()> {
    match e.raw_os_error() {
        Some(c) if REPLACE_PARTWAY.contains(&c) => {
            match fs::symlink_metadata(target) {
                // Gone: finish the job with the verified temp.
                Err(m) if m.kind() == std::io::ErrorKind::NotFound => fs::rename(temp, target),
                // Still the old file: the caller retries the whole replace.
                _ => Err(e),
            }
        }
        // ERROR_INVALID_FUNCTION, ERROR_NOT_SUPPORTED: no `ReplaceFileW` on this file system.
        Some(1 | 50) => fs::rename(temp, target),
        _ => Err(e),
    }
}

/// One attempt to put `temp` at `target`, replacing it.
fn swap_once(temp: &Path, target: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        // `ReplaceFileW` needs an existing target; a missing one is a plain rename. A symlink is
        // replaced at its resolved target so the link stays valid (PLAN 2.14).
        let resolved = fs::canonicalize(target);
        match resolved {
            Ok(real) => match crate::ffi::windows::replace_file(&real, temp) {
                Ok(()) => Ok(()),
                Err(e) => after_failed_replace(e, temp, &real),
            },
            Err(_) => fs::rename(temp, target),
        }
    }
    #[cfg(not(windows))]
    {
        // `rename(2)` would replace a symlink itself; resolve it so the link stays valid.
        match fs::canonicalize(target) {
            Ok(real) => fs::rename(temp, real),
            Err(_) => fs::rename(temp, target),
        }
    }
}

/// Replaces `target` with the verified temp file (atomic on the same volume), retrying sharing
/// violations along [`RETRY_MS`].
pub fn swap(temp: &Path, target: &Path) -> Result<()> {
    swap_with(temp, target, &RETRY_MS)
}

/// [`swap`] with an explicit retry ladder (milliseconds between attempts).
pub fn swap_with(temp: &Path, target: &Path, ladder: &[u64]) -> Result<()> {
    let mut last = None;
    for (i, wait) in std::iter::once(&0u64).chain(ladder.iter()).enumerate() {
        if i > 0 {
            std::thread::sleep(Duration::from_millis(*wait));
        }
        match swap_once(temp, target) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = Some(ErrKind::from_io(&e));
                if !retryable(&e) {
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

    fn leftovers(dir: &Path) -> usize {
        fs::read_dir(dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(".autocrop-"))
            .count()
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
        assert_eq!(leftovers(dir.path()), 0);
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
    fn a_single_flipped_bit_is_always_caught_by_the_hash() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = png(40, 30);
        let t = write_temp(dir.path(), &bytes, None).unwrap();
        for i in (0..bytes.len()).step_by(7) {
            let mut bad = bytes.clone();
            bad[i] ^= 0x10;
            fs::write(&t.path, &bad).unwrap();
            assert_eq!(
                verify_temp(&t, (40, 30), Format::Png),
                Err(ErrKind::VerifyFailed),
                "flip at {i}"
            );
        }
        // A truncation at every length.
        for cut in (0..bytes.len()).step_by(11) {
            fs::write(&t.path, &bytes[..cut]).unwrap();
            assert_eq!(
                verify_temp(&t, (40, 30), Format::Png),
                Err(ErrKind::VerifyFailed),
                "cut at {cut}"
            );
        }
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

    fn gradient(w: u32, h: u32) -> Raster {
        let mut r = Raster::new(w, h);
        for y in 0..h {
            for x in 0..w {
                r.set_pixel(x, y, [(x * 255 / w) as u8, (y * 255 / h) as u8, 90]);
            }
        }
        r
    }

    #[test]
    fn the_content_check_accepts_the_real_encode_and_refuses_a_different_picture() {
        let dir = tempfile::tempdir().unwrap();
        let r = gradient(160, 120);
        for (format, mode) in [
            (Format::Png, VerifyMode::Full),
            (Format::Png, VerifyMode::Fast),
            (Format::Jpeg, VerifyMode::Full),
            (Format::Jpeg, VerifyMode::Fast),
        ] {
            let bytes = auto_crop_codecs::encode(&r, format, 92, None).unwrap();
            let t = write_temp(dir.path(), &bytes, None).unwrap();
            let e = Expect::for_raster(&r, format, None);
            verify_temp_expect(&t, &e, mode)
                .unwrap_or_else(|k| panic!("{format:?} {mode:?}: {k:?}"));
            // Same size, other picture: only the content check can tell.
            let other = Raster::filled(160, 120, [255, 255, 255]);
            let e2 = Expect::for_raster(&other, format, None);
            assert_eq!(
                verify_temp_expect(&t, &e2, mode),
                Err(ErrKind::VerifyFailed),
                "{format:?} {mode:?}"
            );
            t.discard();
        }
    }

    #[test]
    fn a_missing_icc_profile_fails_the_verification() {
        let dir = tempfile::tempdir().unwrap();
        let r = gradient(64, 48);
        let bytes = auto_crop_codecs::encode(&r, Format::Png, 92, None).unwrap();
        let t = write_temp(dir.path(), &bytes, None).unwrap();
        let mut e = Expect::for_raster(&r, Format::Png, None);
        e.icc = Some(b"not a profile that was written".to_vec());
        assert_eq!(
            verify_temp_expect(&t, &e, VerifyMode::Full),
            Err(ErrKind::VerifyFailed)
        );
    }

    #[test]
    fn the_fingerprint_compares_cells_of_the_same_grid_only() {
        let big = gradient(640, 480);
        let small = auto_crop_imgproc::scale::resize_to_fit(&big, 80);
        let (a, b) = (luma_fingerprint(&big), luma_fingerprint(&small));
        // Different sizes give different grids, which never compare as close.
        assert_ne!(a.len(), b.len());
        assert!(!luma_close(&a, &b));
        assert!(luma_close(&a, &a));
        let mut moved = a.clone();
        moved[0] = moved[0].saturating_add(60);
        assert!(!luma_close(&a, &moved));
    }

    #[test]
    fn a_swap_onto_a_missing_target_is_a_plain_rename() {
        let dir = tempfile::tempdir().unwrap();
        let t = write_temp(dir.path(), b"new", None).unwrap();
        let target = dir.path().join("fresh.bin");
        swap(&t.path, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new");
    }

    /// Error 1176: `ReplaceFileW` moved the old file away but could not move the replacement in.
    /// The target is missing and the verified temp is there: the temp is renamed into place.
    #[test]
    fn a_replace_that_stopped_part_way_with_the_target_gone_is_finished() {
        let dir = tempfile::tempdir().unwrap();
        let t = write_temp(dir.path(), b"verified output", None).unwrap();
        let target = dir.path().join("a.jpg");
        let e = std::io::Error::from_raw_os_error(1176);
        after_failed_replace(e, &t.path, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"verified output");
        assert!(!t.path.exists());
    }

    /// Error 1175 (or 1176 with the old file still there): nothing changed, the temp stays for
    /// the retry, and the error stays retryable.
    #[test]
    fn a_replace_that_stopped_part_way_with_the_target_present_is_retried() {
        let dir = tempfile::tempdir().unwrap();
        let t = write_temp(dir.path(), b"new", None).unwrap();
        let target = dir.path().join("a.jpg");
        fs::write(&target, b"old").unwrap();
        for code in [1175, 1176, 1177] {
            let e = std::io::Error::from_raw_os_error(code);
            let err = after_failed_replace(e, &t.path, &target).unwrap_err();
            assert!(retryable(&err), "{code}");
            assert_eq!(fs::read(&target).unwrap(), b"old");
            assert!(t.path.exists());
        }
        // An unrelated failure is not retried.
        let e = std::io::Error::from_raw_os_error(2);
        assert!(!retryable(
            &after_failed_replace(e, &t.path, &target).unwrap_err()
        ));
    }

    #[test]
    fn a_file_system_without_replacefile_falls_back_to_rename() {
        let dir = tempfile::tempdir().unwrap();
        let t = write_temp(dir.path(), b"new", None).unwrap();
        let target = dir.path().join("a.jpg");
        fs::write(&target, b"old").unwrap();
        let e = std::io::Error::from_raw_os_error(50);
        after_failed_replace(e, &t.path, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new");
    }

    /// A target held open without delete sharing (an antivirus scan, an indexer) blocks the swap
    /// on Windows; the ladder waits it out. A lock that outlasts the ladder is `FileInUse`, with
    /// the old file intact and the temp left for the caller to remove.
    #[cfg(windows)]
    #[test]
    fn a_300_ms_lock_succeeds_and_a_long_one_gives_file_in_use() {
        use std::os::windows::fs::OpenOptionsExt;
        let lock = |p: &Path| {
            fs::OpenOptions::new()
                .read(true)
                .share_mode(1 | 2) // FILE_SHARE_READ | FILE_SHARE_WRITE, not DELETE
                .open(p)
                .unwrap()
        };
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("a.jpg");
        fs::write(&target, b"old").unwrap();

        // 300 ms: released before the ladder (10 + 20 + ... + 640 ms) runs out.
        let t = write_temp(dir.path(), b"new", None).unwrap();
        let held = lock(&target);
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            drop(held);
        });
        let started = std::time::Instant::now();
        swap(&t.path, &target).unwrap();
        release.join().unwrap();
        assert!(started.elapsed() >= Duration::from_millis(250));
        assert_eq!(fs::read(&target).unwrap(), b"new");

        // Held through the whole ladder (about 1.3 s): FileInUse, nothing lost.
        let t = write_temp(dir.path(), b"newer", None).unwrap();
        let held = lock(&target);
        assert_eq!(swap(&t.path, &target), Err(ErrKind::FileInUse));
        assert_eq!(fs::read(&target).unwrap(), b"new");
        assert!(t.path.exists());
        drop(held);
        swap(&t.path, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"newer");
    }

    #[cfg(windows)]
    #[test]
    fn a_300_character_path_swaps() {
        let dir = tempfile::tempdir().unwrap();
        let mut deep = dir.path().to_path_buf();
        while deep.as_os_str().len() < 300 {
            deep.push("a_folder_with_a_long_name");
        }
        fs::create_dir_all(&deep).unwrap();
        let target = deep.join("photo.jpg");
        fs::write(&target, b"old").unwrap();
        let t = write_temp(&deep, b"new", None).unwrap();
        swap(&t.path, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new");
    }

    #[test]
    fn the_swap_keeps_the_new_files_mtime_and_a_symlink_valid() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.bin");
        fs::write(&real, b"old").unwrap();
        let when = SystemTime::UNIX_EPOCH + Duration::from_secs(1_400_000_000);
        let t = write_temp(dir.path(), b"new", Some(when)).unwrap();
        swap(&t.path, &real).unwrap();
        assert_eq!(fs::metadata(&real).unwrap().modified().unwrap(), when);
        // A symlinked target is replaced where it points; creating a symlink needs a privilege
        // on Windows, so the check is skipped when it is not available.
        let link = dir.path().join("link.bin");
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&real, &link).is_ok();
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(&real, &link).is_ok();
        #[cfg(not(any(unix, windows)))]
        let made = false;
        if made {
            let t = write_temp(dir.path(), b"newest", None).unwrap();
            swap(&t.path, &link).unwrap();
            assert!(
                fs::symlink_metadata(&link)
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            assert_eq!(fs::read(&real).unwrap(), b"newest");
        }
    }
}
