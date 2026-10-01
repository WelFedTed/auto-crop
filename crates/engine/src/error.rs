// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Typed error codes (PLAN 2.10 `ErrKind`). The engine returns codes, never English: the UI owns
//! the wording.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrKind {
    #[error("corrupt or unreadable image")]
    Corrupt,
    #[error("unsupported format")]
    UnsupportedFormat,
    #[error("image too large")]
    TooLarge,
    #[error("file could not be read")]
    Unreadable,
    #[error("the file changed while Auto Crop was working")]
    SourceChanged,
    #[error("the file is in use by another program")]
    FileInUse,
    #[error("the disk is full")]
    DiskFull,
    #[error("the file or folder is read-only")]
    ReadOnly,
    #[error("the new file could not be verified")]
    VerifyFailed,
    #[error("the original could not be backed up")]
    BackupFailed,
    #[error("the backup of the original has expired")]
    OriginalExpired,
    #[error("there is no crop to apply")]
    NoCrop,
    #[error("internal error")]
    Internal,
}

impl ErrKind {
    /// Maps an I/O error from a write or replace to the closest code.
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

impl From<auto_crop_codecs::CodecError> for ErrKind {
    fn from(e: auto_crop_codecs::CodecError) -> Self {
        use auto_crop_codecs::CodecError as C;
        match e {
            C::Unsupported => ErrKind::UnsupportedFormat,
            C::Corrupt(_) => ErrKind::Corrupt,
            C::TooLarge(_) => ErrKind::TooLarge,
            C::Encode(_) => ErrKind::Internal,
        }
    }
}

pub type Result<T> = std::result::Result<T, ErrKind>;
