// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Typed error codes (PLAN 2.10 `ErrKind`). The enum lives in `auto-crop-core` (M1.09) so every
//! crate shares it; this module re-exports it and holds the conversions that need engine-side
//! types. The engine returns codes, never English: the UI owns the wording.

pub use auto_crop_core::ErrKind;

/// Maps a codec failure to the closest code. (A free function rather than `From`: both types are
/// foreign to this crate.)
pub fn codec_err(e: auto_crop_codecs::CodecError) -> ErrKind {
    use auto_crop_codecs::CodecError as C;
    match e {
        C::Unsupported => ErrKind::UnsupportedFormat,
        C::Corrupt(_) => ErrKind::Corrupt,
        C::TooLarge(_) => ErrKind::TooLarge,
        // The early slice reported these as Internal and the UI has copy for that code only.
        C::Encode(_) => ErrKind::Internal,
    }
}

pub type Result<T> = std::result::Result<T, ErrKind>;
