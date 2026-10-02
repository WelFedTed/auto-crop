// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Typed error codes (PLAN 2.10 `ErrKind`). Errors are stable codes, never English: the UI and the
//! CLI localise them by looking up [`ErrKind::user_message_key`] (B20). The `Display` text is for
//! logs and developers only.

use serde::{Deserialize, Serialize};

/// Errors produced by the core crate itself.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CoreError {
    #[error("unsupported edit-state version {0}")]
    UnsupportedVersion(u32),
}

/// The enum behind the PLAN 2.10 taxonomy. Later milestones (M2.05) extend it; the serialised form
/// is `SCREAMING_SNAKE_CASE` and is mirrored by `ui/src/lib/types.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrKind {
    // Input
    #[error("corrupt or unreadable image")]
    Corrupt,
    #[error("unsupported format")]
    UnsupportedFormat,
    #[error("unsupported feature of a supported format")]
    UnsupportedFeature,
    #[error("image too large")]
    TooLarge,
    #[error("file could not be read")]
    Unreadable,
    #[error("the file or folder is read-only")]
    ReadOnly,
    #[error("the file is a cloud placeholder and not available locally")]
    CloudNotLocal,
    #[error("no HEVC decoder is installed")]
    HevcDecoderMissing,
    // Decode
    #[error("the decoder process crashed")]
    DecoderCrashed,
    #[error("decoding took too long")]
    DecodeTimeout,
    #[error("a memory limit was exceeded")]
    MemoryLimit,
    // Analyse, render
    #[error("a model could not be loaded")]
    ModelLoadFailed,
    #[error("out of memory")]
    OutOfMemory,
    #[error("internal panic")]
    InternalPanic,
    #[error("the crop quadrilateral is degenerate")]
    Degenerate,
    #[error("there is no crop to apply")]
    NoCrop,
    // Encode
    #[error("encoding failed")]
    EncodeFailed,
    #[error("this output cannot be written")]
    UnsupportedOutput,
    // Write
    #[error("the file changed while Auto Crop was working")]
    SourceChanged,
    #[error("the file is in use by another program")]
    FileInUse,
    #[error("the disk is full")]
    DiskFull,
    #[error("the new file could not be verified")]
    VerifyFailed,
    #[error("the original could not be backed up")]
    BackupFailed,
    #[error("the backup of the original has expired")]
    OriginalExpired,
    // Stored data
    #[error("the saved edit was written by a newer version")]
    SchemaTooNew,
    // Control flow
    #[error("cancelled")]
    Cancelled,
    #[error("the deadline passed")]
    DeadlineExceeded,
    #[error("internal error")]
    Internal,
}

impl ErrKind {
    /// Every variant, in declaration order (the exhaustive [`ErrKind::ordinal`] match and the test
    /// `all_lists_every_variant` keep this honest).
    pub const ALL: [ErrKind; 28] = [
        ErrKind::Corrupt,
        ErrKind::UnsupportedFormat,
        ErrKind::UnsupportedFeature,
        ErrKind::TooLarge,
        ErrKind::Unreadable,
        ErrKind::ReadOnly,
        ErrKind::CloudNotLocal,
        ErrKind::HevcDecoderMissing,
        ErrKind::DecoderCrashed,
        ErrKind::DecodeTimeout,
        ErrKind::MemoryLimit,
        ErrKind::ModelLoadFailed,
        ErrKind::OutOfMemory,
        ErrKind::InternalPanic,
        ErrKind::Degenerate,
        ErrKind::NoCrop,
        ErrKind::EncodeFailed,
        ErrKind::UnsupportedOutput,
        ErrKind::SourceChanged,
        ErrKind::FileInUse,
        ErrKind::DiskFull,
        ErrKind::VerifyFailed,
        ErrKind::BackupFailed,
        ErrKind::OriginalExpired,
        ErrKind::SchemaTooNew,
        ErrKind::Cancelled,
        ErrKind::DeadlineExceeded,
        ErrKind::Internal,
    ];

    /// Position in declaration order. Exhaustive on purpose: adding a variant fails to compile
    /// until it is given an ordinal and a message key.
    pub const fn ordinal(self) -> usize {
        match self {
            ErrKind::Corrupt => 0,
            ErrKind::UnsupportedFormat => 1,
            ErrKind::UnsupportedFeature => 2,
            ErrKind::TooLarge => 3,
            ErrKind::Unreadable => 4,
            ErrKind::ReadOnly => 5,
            ErrKind::CloudNotLocal => 6,
            ErrKind::HevcDecoderMissing => 7,
            ErrKind::DecoderCrashed => 8,
            ErrKind::DecodeTimeout => 9,
            ErrKind::MemoryLimit => 10,
            ErrKind::ModelLoadFailed => 11,
            ErrKind::OutOfMemory => 12,
            ErrKind::InternalPanic => 13,
            ErrKind::Degenerate => 14,
            ErrKind::NoCrop => 15,
            ErrKind::EncodeFailed => 16,
            ErrKind::UnsupportedOutput => 17,
            ErrKind::SourceChanged => 18,
            ErrKind::FileInUse => 19,
            ErrKind::DiskFull => 20,
            ErrKind::VerifyFailed => 21,
            ErrKind::BackupFailed => 22,
            ErrKind::OriginalExpired => 23,
            ErrKind::SchemaTooNew => 24,
            ErrKind::Cancelled => 25,
            ErrKind::DeadlineExceeded => 26,
            ErrKind::Internal => 27,
        }
    }

    /// Fluent message key for this error (B20): the UI and CLI look the wording up here, `core`
    /// never builds English. Format: `err.<snake_case_variant>`.
    pub const fn user_message_key(self) -> &'static str {
        match self {
            ErrKind::Corrupt => "err.corrupt",
            ErrKind::UnsupportedFormat => "err.unsupported_format",
            ErrKind::UnsupportedFeature => "err.unsupported_feature",
            ErrKind::TooLarge => "err.too_large",
            ErrKind::Unreadable => "err.unreadable",
            ErrKind::ReadOnly => "err.read_only",
            ErrKind::CloudNotLocal => "err.cloud_not_local",
            ErrKind::HevcDecoderMissing => "err.hevc_decoder_missing",
            ErrKind::DecoderCrashed => "err.decoder_crashed",
            ErrKind::DecodeTimeout => "err.decode_timeout",
            ErrKind::MemoryLimit => "err.memory_limit",
            ErrKind::ModelLoadFailed => "err.model_load_failed",
            ErrKind::OutOfMemory => "err.out_of_memory",
            ErrKind::InternalPanic => "err.internal_panic",
            ErrKind::Degenerate => "err.degenerate",
            ErrKind::NoCrop => "err.no_crop",
            ErrKind::EncodeFailed => "err.encode_failed",
            ErrKind::UnsupportedOutput => "err.unsupported_output",
            ErrKind::SourceChanged => "err.source_changed",
            ErrKind::FileInUse => "err.file_in_use",
            ErrKind::DiskFull => "err.disk_full",
            ErrKind::VerifyFailed => "err.verify_failed",
            ErrKind::BackupFailed => "err.backup_failed",
            ErrKind::OriginalExpired => "err.original_expired",
            ErrKind::SchemaTooNew => "err.schema_too_new",
            ErrKind::Cancelled => "err.cancelled",
            ErrKind::DeadlineExceeded => "err.deadline_exceeded",
            ErrKind::Internal => "err.internal",
        }
    }

    /// Maps an I/O error from a read, write or replace to the closest code. Only inspects the
    /// error value; no I/O happens here.
    pub fn from_io(e: &std::io::Error) -> Self {
        use std::io::ErrorKind as K;
        match (e.raw_os_error(), e.kind()) {
            // ERROR_DISK_FULL, ERROR_HANDLE_DISK_FULL, ENOSPC
            (Some(112 | 39 | 28), _) | (_, K::StorageFull) => ErrKind::DiskFull,
            // ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION
            (Some(32 | 33), _) => ErrKind::FileInUse,
            (_, K::PermissionDenied) => ErrKind::ReadOnly,
            (_, K::ReadOnlyFilesystem) => ErrKind::ReadOnly,
            _ => ErrKind::Unreadable,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn all_lists_every_variant() {
        let ordinals: Vec<usize> = ErrKind::ALL.iter().map(|k| k.ordinal()).collect();
        assert_eq!(ordinals, (0..ErrKind::ALL.len()).collect::<Vec<_>>());
    }

    #[test]
    fn every_variant_has_a_message_key_and_a_display_string() {
        let mut keys = HashSet::new();
        let mut codes = HashSet::new();
        for k in ErrKind::ALL {
            let key = k.user_message_key();
            assert!(key.starts_with("err.") && key.len() > 4, "{k:?}");
            assert!(!k.to_string().is_empty(), "{k:?}");
            // The key is the lower-cased serialised code.
            let code: String = serde_json::from_str(&serde_json::to_string(&k).unwrap()).unwrap();
            assert_eq!(key, format!("err.{}", code.to_lowercase()), "{k:?}");
            keys.insert(key);
            codes.insert(code);
        }
        assert_eq!(keys.len(), ErrKind::ALL.len());
        assert_eq!(codes.len(), ErrKind::ALL.len());
    }

    #[test]
    fn serialised_codes_are_stable() {
        // These strings are mirrored in ui/src/lib/types.ts.
        assert_eq!(
            serde_json::to_string(&ErrKind::OriginalExpired).unwrap(),
            "\"ORIGINAL_EXPIRED\""
        );
        assert_eq!(
            serde_json::from_str::<ErrKind>("\"SCHEMA_TOO_NEW\"").unwrap(),
            ErrKind::SchemaTooNew
        );
    }

    #[test]
    fn io_errors_map_to_codes() {
        let e = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        assert_eq!(ErrKind::from_io(&e), ErrKind::ReadOnly);
        let e = std::io::Error::from_raw_os_error(112);
        assert_eq!(ErrKind::from_io(&e), ErrKind::DiskFull);
        let e = std::io::Error::from_raw_os_error(32);
        assert_eq!(ErrKind::from_io(&e), ErrKind::FileInUse);
    }
}
