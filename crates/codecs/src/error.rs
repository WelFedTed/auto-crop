// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Typed codec errors. Every failure of a decode of untrusted bytes is one of these; nothing in
//! this crate is allowed to panic out of a public function.

use crate::{Format, Limit};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CodecError {
    /// Not an image this crate recognises.
    #[error("unsupported format")]
    Unsupported,
    /// A recognised container that this build does not decode (HEIC, AVIF, GIF, BMP, JXL).
    #[error("{} files cannot be decoded in this build", .0.extension())]
    NotDecodable(Format),
    /// A recognised format that uses a feature this build cannot decode (12-bit JPEG, ...).
    #[error("unsupported feature: {0}")]
    UnsupportedFeature(String),
    #[error("corrupt or unreadable image: {0}")]
    Corrupt(String),
    /// More than the pixel cap (the count is the declared `width x height`).
    #[error("image too large: {0} pixels")]
    TooLarge(u64),
    /// Any other [`crate::DecodeLimits`] field exceeded.
    #[error("limit exceeded: {} ({actual} > {cap})", .limit.name())]
    LimitExceeded { limit: Limit, actual: u64, cap: u64 },
    #[error("encode failed: {0}")]
    Encode(String),
    /// A decoder panicked; the panic was caught and the batch can continue (ROADMAP M1.14).
    #[error("internal error in the image decoder: {0}")]
    InternalPanic(String),
    /// `max_decode_ms` elapsed; the decode result, if any, is discarded.
    #[error("decoding took too long")]
    DecodeTimeout,
}

impl CodecError {
    pub(crate) fn corrupt(msg: impl Into<String>) -> Self {
        CodecError::Corrupt(msg.into())
    }

    /// Stable machine code (the engine maps these to its `ErrKind`).
    pub fn code(&self) -> &'static str {
        match self {
            CodecError::Unsupported => "unsupported",
            CodecError::NotDecodable(_) => "not_decodable",
            CodecError::UnsupportedFeature(_) => "unsupported_feature",
            CodecError::Corrupt(_) => "corrupt",
            CodecError::TooLarge(_) => "too_large",
            CodecError::LimitExceeded { .. } => "limit_exceeded",
            CodecError::Encode(_) => "encode",
            CodecError::InternalPanic(_) => "internal_panic",
            CodecError::DecodeTimeout => "decode_timeout",
        }
    }
}
