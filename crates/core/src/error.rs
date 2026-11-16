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
    #[error("the image uses a HEIF codec this build cannot decode")]
    UnsupportedHeifCodec,
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
    // Multi-item scans (M10)
    #[error("an output name was taken since the plan was made")]
    PlanStale,
    #[error("the set of files could not be written as a whole")]
    GroupCommitFailed,
    #[error("the saved scan could not be removed because another program uses it")]
    SavedSourceInUse,
    #[error("the scan is held for review and was not written")]
    HeldForReview,
    #[error("that change to the items is not possible")]
    ItemOp,
    #[error("this source is never replaced")]
    NotReplaceable,
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
    pub const ALL: [ErrKind; 35] = [
        ErrKind::Corrupt,
        ErrKind::UnsupportedFormat,
        ErrKind::UnsupportedFeature,
        ErrKind::TooLarge,
        ErrKind::Unreadable,
        ErrKind::ReadOnly,
        ErrKind::CloudNotLocal,
        ErrKind::HevcDecoderMissing,
        ErrKind::UnsupportedHeifCodec,
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
        ErrKind::PlanStale,
        ErrKind::GroupCommitFailed,
        ErrKind::SavedSourceInUse,
        ErrKind::HeldForReview,
        ErrKind::ItemOp,
        ErrKind::NotReplaceable,
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
            ErrKind::UnsupportedHeifCodec => 8,
            ErrKind::DecoderCrashed => 9,
            ErrKind::DecodeTimeout => 10,
            ErrKind::MemoryLimit => 11,
            ErrKind::ModelLoadFailed => 12,
            ErrKind::OutOfMemory => 13,
            ErrKind::InternalPanic => 14,
            ErrKind::Degenerate => 15,
            ErrKind::NoCrop => 16,
            ErrKind::EncodeFailed => 17,
            ErrKind::UnsupportedOutput => 18,
            ErrKind::SourceChanged => 19,
            ErrKind::FileInUse => 20,
            ErrKind::DiskFull => 21,
            ErrKind::VerifyFailed => 22,
            ErrKind::BackupFailed => 23,
            ErrKind::OriginalExpired => 24,
            ErrKind::PlanStale => 25,
            ErrKind::GroupCommitFailed => 26,
            ErrKind::SavedSourceInUse => 27,
            ErrKind::HeldForReview => 28,
            ErrKind::ItemOp => 29,
            ErrKind::NotReplaceable => 30,
            ErrKind::SchemaTooNew => 31,
            ErrKind::Cancelled => 32,
            ErrKind::DeadlineExceeded => 33,
            ErrKind::Internal => 34,
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
            ErrKind::UnsupportedHeifCodec => "err.unsupported_heif_codec",
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
            ErrKind::PlanStale => "err.plan_stale",
            ErrKind::GroupCommitFailed => "err.group_commit_failed",
            ErrKind::SavedSourceInUse => "err.saved_source_in_use",
            ErrKind::HeldForReview => "err.held_for_review",
            ErrKind::ItemOp => "err.item_op",
            ErrKind::NotReplaceable => "err.not_replaceable",
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
        assert_eq!(
            serde_json::to_string(&ErrKind::UnsupportedHeifCodec).unwrap(),
            "\"UNSUPPORTED_HEIF_CODEC\""
        );
    }

    /// The UI mirrors every code in `ui/src/lib/types.ts` (`ErrorCode`) and words it in
    /// `ui/src/lib/strings.ts` (`ERRORS`). A variant added here without both fails this test.
    #[test]
    fn the_ui_lists_exactly_these_codes_in_this_order_with_copy() {
        let ui = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/src/lib");
        let read =
            |f: &str| std::fs::read_to_string(ui.join(f)).unwrap_or_else(|e| panic!("{f}: {e}"));
        let want: Vec<String> = ErrKind::ALL
            .iter()
            .map(|k| {
                serde_json::to_value(k)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();

        let types = read("types.ts");
        let block = types
            .split("export type ErrorCode =")
            .nth(1)
            .expect("ErrorCode in types.ts");
        let mut in_types: Vec<String> = Vec::new();
        for line in block.lines() {
            // comments hold quotes and semicolons; the union ends at the line with the `;`
            let code = line.split("//").next().unwrap_or("");
            if let Some(name) = code.split('\'').nth(1) {
                in_types.push(name.to_owned());
            }
            if code.contains(';') {
                break;
            }
        }
        assert_eq!(in_types, want, "ui/src/lib/types.ts ErrorCode");

        let strings = read("strings.ts").replace("\r\n", "\n");
        let block = strings
            .split("export const ERRORS: Record<ErrorCode, string> = {")
            .nth(1)
            .and_then(|r| r.split("\n};").next())
            .expect("ERRORS in strings.ts");
        let mut in_strings: Vec<String> = block
            .lines()
            .filter(|l| {
                l.starts_with("  ") && !l.starts_with("   ") && !l.trim_start().starts_with("//")
            })
            .filter_map(|l| l.trim_start().split(':').next())
            .map(str::to_owned)
            .collect();
        in_strings.sort();
        let mut sorted = want.clone();
        sorted.sort();
        assert_eq!(in_strings, sorted, "ui/src/lib/strings.ts ERRORS");
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
