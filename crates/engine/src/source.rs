// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Source identity (ROADMAP M1.02, PLAN 2.3): `SourceId` is the blake3 hash of the whole file,
//! computed lazily and streamed so a 100 MP JPEG never has to sit in memory just to be hashed.
//! Core defines the types; this module does the reading.

use crate::error::{ErrKind, Result};
use auto_crop_core::{ExifOrientation, SourceId, SourceRef};
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

/// Read size of the streamed hash.
pub const HASH_CHUNK: usize = 1 << 20;

/// Hashes everything `r` yields, `HASH_CHUNK` bytes at a time.
pub fn hash_reader(mut r: impl Read) -> std::io::Result<SourceId> {
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; HASH_CHUNK];
    loop {
        match r.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                hasher.update(&buf[..n]);
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(SourceId::from_bytes(*hasher.finalize().as_bytes()))
}

/// The id of the file at `path`: blake3 of every byte, whatever the name or timestamps.
pub fn hash_file(path: &Path) -> Result<SourceId> {
    let f = File::open(path).map_err(|e| ErrKind::from_io(&e))?;
    hash_reader(f).map_err(|e| ErrKind::from_io(&e))
}

/// A `SourceRef` from a `stat` and facts the caller already has from a header probe. Does not
/// read the file: the id stays lazy until [`source_id`] is called.
pub fn source_ref(
    path: &Path,
    dims: (u32, u32),
    exif_orientation: ExifOrientation,
    icc: Option<Arc<[u8]>>,
) -> Result<SourceRef> {
    let meta = std::fs::metadata(path).map_err(|e| ErrKind::from_io(&e))?;
    Ok(SourceRef::new(
        path.to_path_buf(),
        meta.len(),
        meta.modified().ok(),
        dims,
        exif_orientation,
        icc,
    ))
}

/// The id of `source`, streamed from its path on first use and cached in the `SourceRef`.
pub fn source_id(source: &SourceRef) -> Result<SourceId> {
    source.id_with(hash_file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{Duration, SystemTime};

    /// Deterministic filler that does not compress to nothing.
    fn bytes(len: usize, seed: u8) -> Vec<u8> {
        (0..len)
            .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
            .collect()
    }

    fn set_mtime(path: &Path, t: SystemTime) {
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(t)
            .unwrap();
    }

    #[test]
    fn the_id_is_blake3_of_the_whole_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.bin");
        let data = bytes(3 * HASH_CHUNK + 12_345, 1);
        fs::write(&p, &data).unwrap();
        let id = hash_file(&p).unwrap();
        assert_eq!(id.as_bytes(), blake3::hash(&data).as_bytes());
        assert_eq!(id.to_hex(), blake3::hash(&data).to_hex().as_str());
        // Empty files have an id too.
        let e = dir.path().join("empty");
        fs::write(&e, b"").unwrap();
        assert_eq!(
            hash_file(&e).unwrap().as_bytes(),
            blake3::hash(b"").as_bytes()
        );
        assert_eq!(
            hash_file(&dir.path().join("missing")),
            Err(ErrKind::Unreadable)
        );
    }

    #[test]
    fn the_id_is_stable_across_rename_and_mtime_change() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("IMG_0001.jpg");
        fs::write(&a, bytes(200_000, 7)).unwrap();
        let before = hash_file(&a).unwrap();

        let b = dir.path().join("renamed elsewhere.JPEG");
        fs::rename(&a, &b).unwrap();
        set_mtime(
            &b,
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000),
        );
        assert_eq!(hash_file(&b).unwrap(), before);

        // A copy anywhere has the same id.
        let c = dir.path().join("copy.bin");
        fs::copy(&b, &c).unwrap();
        assert_eq!(hash_file(&c).unwrap(), before);
    }

    #[test]
    fn same_size_files_differing_in_the_middle_get_different_ids() {
        let dir = tempfile::tempdir().unwrap();
        let len = 5 * HASH_CHUNK;
        let mut one = bytes(len, 3);
        let two_path = dir.path().join("two");
        let one_path = dir.path().join("one");
        fs::write(&one_path, &one).unwrap();
        one[len / 2] ^= 1; // one bit, far from the head and the tail
        fs::write(&two_path, &one).unwrap();
        assert_eq!(
            fs::metadata(&one_path).unwrap().len(),
            fs::metadata(&two_path).unwrap().len()
        );
        assert_ne!(hash_file(&one_path).unwrap(), hash_file(&two_path).unwrap());
    }

    #[test]
    fn hashing_is_streamed_in_bounded_reads() {
        struct Spy {
            left: usize,
            biggest: usize,
            reads: usize,
        }
        impl Read for Spy {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                self.biggest = self.biggest.max(buf.len());
                self.reads += 1;
                let n = buf.len().min(self.left).min(100_000);
                buf[..n].fill(0xAB);
                self.left -= n;
                Ok(n)
            }
        }
        // 64 MiB of input, never more than one chunk requested at a time.
        let total = 64 << 20;
        let mut spy = Spy {
            left: total,
            biggest: 0,
            reads: 0,
        };
        let id = hash_reader(&mut spy).unwrap();
        assert!(spy.biggest <= HASH_CHUNK, "asked for {} bytes", spy.biggest);
        assert!(spy.reads > 600);
        let mut h = blake3::Hasher::new();
        h.update(&vec![0xAB; total]);
        assert_eq!(id.as_bytes(), h.finalize().as_bytes());
    }

    #[test]
    fn the_source_ref_is_lazy_and_caches_the_id() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.jpg");
        let data = bytes(10_000, 9);
        fs::write(&p, &data).unwrap();
        let r = source_ref(&p, (100, 50), ExifOrientation::Rotate90Cw, None).unwrap();
        assert_eq!(r.size, 10_000);
        assert!(r.mtime.is_some());
        assert_eq!(r.oriented_dims(), (50, 100));
        // Building the ref did not read the file.
        assert_eq!(r.id_if_known(), None);
        let id = source_id(&r).unwrap();
        assert_eq!(id.as_bytes(), blake3::hash(&data).as_bytes());
        assert_eq!(r.id_if_known(), Some(id));
        // Cached: the file may now vanish without affecting the answer.
        fs::remove_file(&p).unwrap();
        assert_eq!(source_id(&r), Ok(id));
        assert!(source_ref(&p, (1, 1), ExifOrientation::Normal, None).is_err());
    }
}
