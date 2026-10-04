// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Hash-checked model bytes (ADR-0007: "only hash-checked models listed in `models.lock`").
//!
//! A [`VerifiedModel`] owns the bytes that were hashed. Both backends build their session from
//! these bytes in memory (`commit_from_memory`, `rten::Model::load`), never from a path, so the
//! file cannot change between the check and the load.

use crate::InferError;
use sha2::{Digest, Sha256};

/// Model bytes plus their SHA-256.
#[derive(Clone)]
pub struct VerifiedModel {
    bytes: Vec<u8>,
    sha256: String,
}

impl std::fmt::Debug for VerifiedModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VerifiedModel")
            .field("len", &self.bytes.len())
            .field("sha256", &self.sha256)
            .finish()
    }
}

/// Lower-case hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

pub(crate) fn hex(digest: &[u8]) -> String {
    use std::fmt::Write as _;
    digest.iter().fold(String::with_capacity(64), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// `<model>.sha256`, the sidecar digest file next to a generated model.
pub fn sidecar_path(model: &std::path::Path) -> std::path::PathBuf {
    let mut s = model.as_os_str().to_owned();
    s.push(".sha256");
    s.into()
}

/// True for 64 hexadecimal characters.
pub fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

impl VerifiedModel {
    /// Hashes `bytes` and refuses them unless the digest equals `expected` (case-insensitive hex).
    /// This is the `models.lock` check: the pin comes from a reviewed file, not from the bytes.
    pub fn from_bytes_pinned(bytes: Vec<u8>, expected: &str) -> Result<Self, InferError> {
        if !is_sha256_hex(expected) {
            return Err(InferError::BadPin(expected.to_owned()));
        }
        let got = sha256_hex(&bytes);
        if got.eq_ignore_ascii_case(expected) {
            Ok(Self { bytes, sha256: got })
        } else {
            Err(InferError::ModelHashMismatch {
                expected: expected.to_ascii_lowercase(),
                got,
            })
        }
    }

    /// Hashes `bytes` without a pin to compare against, for tests and for stand-in nets that are
    /// generated at build time (`cargo xtask make-standin-net` writes a sidecar digest which the
    /// harness passes to [`VerifiedModel::from_bytes_pinned`]). The digest is still computed and
    /// reported.
    pub fn from_bytes_unpinned(bytes: Vec<u8>) -> Self {
        let sha256 = sha256_hex(&bytes);
        Self { bytes, sha256 }
    }

    /// Reads `path` and checks it against the digest in its sidecar `<path>.sha256` (first
    /// whitespace-separated word). This is the `models.lock` stand-in for nets generated at build
    /// time; a shipped model is pinned in a reviewed lock file instead.
    pub fn from_file_with_sidecar(path: &std::path::Path) -> Result<Self, InferError> {
        let io = |p: &std::path::Path, e: std::io::Error| InferError::Load {
            backend: "model",
            message: format!("{}: {e}", p.display()),
        };
        let bytes = std::fs::read(path).map_err(|e| io(path, e))?;
        let side = sidecar_path(path);
        let text = std::fs::read_to_string(&side).map_err(|e| io(&side, e))?;
        Self::from_bytes_pinned(bytes, text.split_whitespace().next().unwrap_or(""))
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Lower-case hex SHA-256 of [`Self::bytes`].
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // sha256("hello")
    const HELLO: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

    #[test]
    fn a_matching_pin_is_accepted_and_a_wrong_one_refused() {
        let ok = VerifiedModel::from_bytes_pinned(b"hello".to_vec(), HELLO).unwrap();
        assert_eq!(ok.sha256(), HELLO);
        assert_eq!(ok.bytes(), b"hello");
        let upper = VerifiedModel::from_bytes_pinned(b"hello".to_vec(), &HELLO.to_uppercase());
        assert!(upper.is_ok());
        let err = VerifiedModel::from_bytes_pinned(b"hellp".to_vec(), HELLO).unwrap_err();
        match err {
            InferError::ModelHashMismatch { expected, got } => {
                assert_eq!(expected, HELLO);
                assert_ne!(got, HELLO);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_malformed_pin_is_refused_not_ignored() {
        for bad in [
            "",
            "abc",
            &HELLO[..63],
            &format!("{HELLO}0"),
            &HELLO.replace('2', "g"),
        ] {
            assert_eq!(
                VerifiedModel::from_bytes_pinned(b"hello".to_vec(), bad).unwrap_err(),
                InferError::BadPin(bad.to_owned())
            );
        }
    }

    #[test]
    fn unpinned_still_hashes() {
        assert_eq!(
            VerifiedModel::from_bytes_unpinned(b"hello".to_vec()).sha256(),
            HELLO
        );
    }
}
