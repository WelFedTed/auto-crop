// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The identity of a source file (PLAN 2.3). `SourceId` is the blake3 hash of the whole file; core
//! only defines the type and the lazy slot, because hashing means reading a file and core does no
//! I/O. The engine computes it, streamed, through [`SourceRef::id_with`].

use crate::error::ErrKind;
use crate::geometry::ExifOrientation;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::SystemTime;

/// blake3 of the whole file's bytes: stable across rename and mtime change, different for any
/// content change.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SourceId([u8; 32]);

impl SourceId {
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Lower-case hex, 64 characters.
    pub fn to_hex(&self) -> String {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        let mut s = String::with_capacity(64);
        for b in self.0 {
            s.push(DIGITS[usize::from(b >> 4)] as char);
            s.push(DIGITS[usize::from(b & 15)] as char);
        }
        s
    }

    pub fn from_hex(s: &str) -> Option<Self> {
        let b = s.as_bytes();
        if b.len() != 64 {
            return None;
        }
        let nib = |c: u8| match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        };
        let mut out = [0u8; 32];
        for (i, pair) in b.as_chunks::<2>().0.iter().enumerate() {
            out[i] = (nib(pair[0])? << 4) | nib(pair[1])?;
        }
        Some(Self(out))
    }

    /// The first 8 hex digits: enough for logs and cache file names, never an identity check.
    pub fn short(&self) -> String {
        self.to_hex()[..8].to_owned()
    }
}

impl fmt::Display for SourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for SourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SourceId({})", self.short())
    }
}

impl Serialize for SourceId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for SourceId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        SourceId::from_hex(&s).ok_or_else(|| serde::de::Error::custom("not a 64-digit hex id"))
    }
}

/// What the engine knows about a source file without decoding it. The id is lazy: building a
/// `SourceRef` costs a `stat` and a header probe, never a read of the whole file.
#[derive(Debug, Clone)]
pub struct SourceRef {
    pub path: PathBuf,
    pub size: u64,
    pub mtime: Option<SystemTime>,
    /// Stored dimensions, before the EXIF turn; see [`SourceRef::oriented_dims`].
    pub dims: (u32, u32),
    /// The EXIF tag as found; the pixels are turned once at decode and the tag is kept here.
    pub exif_orientation: ExifOrientation,
    pub icc: Option<Arc<[u8]>>,
    id: OnceLock<SourceId>,
}

impl SourceRef {
    pub fn new(
        path: PathBuf,
        size: u64,
        mtime: Option<SystemTime>,
        dims: (u32, u32),
        exif_orientation: ExifOrientation,
        icc: Option<Arc<[u8]>>,
    ) -> Self {
        Self {
            path,
            size,
            mtime,
            dims,
            exif_orientation,
            icc,
            id: OnceLock::new(),
        }
    }

    /// Dimensions after the EXIF turn, the space every `EditState` coordinate lives in.
    pub fn oriented_dims(&self) -> (u32, u32) {
        self.exif_orientation
            .oriented_dims(self.dims.0, self.dims.1)
    }

    /// The id if it has been computed already.
    pub fn id_if_known(&self) -> Option<SourceId> {
        self.id.get().copied()
    }

    /// The id, computed on first use by `hash` (the engine passes a streaming file hasher) and
    /// cached. A failed hash is not cached.
    pub fn id_with(
        &self,
        hash: impl FnOnce(&Path) -> Result<SourceId, ErrKind>,
    ) -> Result<SourceId, ErrKind> {
        if let Some(id) = self.id.get() {
            return Ok(*id);
        }
        let id = hash(&self.path)?;
        Ok(*self.id.get_or_init(|| id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn sref() -> SourceRef {
        SourceRef::new(
            PathBuf::from("a.jpg"),
            10,
            None,
            (400, 300),
            ExifOrientation::Rotate90Cw,
            None,
        )
    }

    #[test]
    fn hex_round_trips_and_rejects_garbage() {
        let id = SourceId::from_bytes(std::array::from_fn(|i| (i * 7) as u8));
        let hex = id.to_hex();
        assert_eq!(hex.len(), 64);
        assert_eq!(SourceId::from_hex(&hex), Some(id));
        assert_eq!(SourceId::from_hex(&hex.to_uppercase()), Some(id));
        assert_eq!(SourceId::from_hex("zz"), None);
        assert_eq!(SourceId::from_hex(&"g".repeat(64)), None);
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, format!("\"{hex}\""));
        assert_eq!(serde_json::from_str::<SourceId>(&json).unwrap(), id);
        assert!(serde_json::from_str::<SourceId>("\"abc\"").is_err());
        assert_eq!(id.short(), &hex[..8]);
    }

    #[test]
    fn the_id_is_lazy_computed_once_and_failures_are_not_cached() {
        let r = sref();
        assert_eq!(r.id_if_known(), None);
        let calls = Cell::new(0);
        let want = SourceId::from_bytes([9; 32]);
        let fail: Result<SourceId, ErrKind> = r.id_with(|_| {
            calls.set(calls.get() + 1);
            Err(ErrKind::Unreadable)
        });
        assert_eq!(fail, Err(ErrKind::Unreadable));
        assert_eq!(r.id_if_known(), None);
        let ok = r.id_with(|p| {
            calls.set(calls.get() + 1);
            assert_eq!(p, Path::new("a.jpg"));
            Ok(want)
        });
        assert_eq!(ok, Ok(want));
        assert_eq!(r.id_with(|_| panic!("must not hash again")), Ok(want));
        assert_eq!(calls.get(), 2);
        assert_eq!(r.id_if_known(), Some(want));
        // Clones keep a computed id.
        assert_eq!(r.clone().id_if_known(), Some(want));
    }

    #[test]
    fn oriented_dims_swap_for_quarter_turn_tags() {
        assert_eq!(sref().oriented_dims(), (300, 400));
    }
}
