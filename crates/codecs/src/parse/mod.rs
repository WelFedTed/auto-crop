// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Bounded container parsers: read the header, the metadata sizes and the frame and scan counts of
//! a file without allocating pixels (ROADMAP M1.12). Each parser reads only the bytes it is given,
//! never indexes out of range, and does work proportional to the file size at worst (and to a small
//! constant for the dimension fields). The pixel decoders run only after these checks pass.

use crate::{CodecError, DecodeLimits, Format, Limit};
use std::ops::Range;

pub(crate) mod jpeg;
pub(crate) mod png;
pub(crate) mod tiff;
pub(crate) mod webp;

/// Where the embedded ICC profile of a file lives, so decode can copy it out byte-exact.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) enum IccLoc {
    #[default]
    None,
    /// One contiguous span of the file (TIFF tag, WebP `ICCP` chunk).
    Range(Range<usize>),
    /// JPEG APP2 segments: (sequence number 1.., declared total, payload span).
    JpegChunks(Vec<(u8, u8, Range<usize>)>),
    /// PNG `iCCP`: zlib-compressed, expanded by the PNG decoder.
    PngCompressed,
}

/// Everything the probe learns from the container headers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Header {
    pub format: Format,
    pub width: u32,
    pub height: u32,
    pub bit_depth: u8,
    pub channels: u8,
    pub frames: u32,
    pub scans: u32,
    /// 1..=8, 1 when absent or invalid.
    pub orientation: u8,
    pub icc: IccLoc,
    /// Stored size of the ICC profile, when known.
    pub icc_len: Option<u32>,
    /// The file declares more than one frame/page/image or an animation.
    pub animated: bool,
    /// The file ends inside the image data.
    pub truncated: bool,
    /// Why the decoder cannot read this file even though the container is fine.
    pub unsupported: Option<String>,
}

impl Header {
    pub(crate) fn new(format: Format, width: u32, height: u32) -> Self {
        Self {
            format,
            width,
            height,
            bit_depth: 8,
            channels: 3,
            frames: 1,
            scans: 1,
            orientation: 1,
            icc: IccLoc::None,
            icc_len: None,
            animated: false,
            truncated: false,
            unsupported: None,
        }
    }
}

/// Parses the header of `bytes`, which `format` (from the sniffer) says is decodable.
pub(crate) fn parse(
    bytes: &[u8],
    format: Format,
    limits: &DecodeLimits,
) -> Result<Header, CodecError> {
    let h = match format {
        Format::Jpeg => jpeg::parse(bytes, limits)?,
        Format::Png => png::parse(bytes, limits)?,
        Format::Tiff => tiff::parse(bytes, limits)?,
        Format::Webp => webp::parse(bytes, limits)?,
        other => return Err(CodecError::NotDecodable(other)),
    };
    if h.width == 0 || h.height == 0 {
        return Err(CodecError::corrupt("image has a zero dimension"));
    }
    Ok(h)
}

/// Rejects a metadata blob of `len` bytes above the cap.
pub(crate) fn check_metadata(len: u64, limits: &DecodeLimits) -> Result<(), CodecError> {
    if len > limits.max_metadata_bytes {
        Err(CodecError::LimitExceeded {
            limit: Limit::MetadataBytes,
            actual: len,
            cap: limits.max_metadata_bytes,
        })
    } else {
        Ok(())
    }
}

pub(crate) fn be16(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes(
        b.get(o..o.checked_add(2)?)?.try_into().ok()?,
    ))
}

pub(crate) fn be32(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        b.get(o..o.checked_add(4)?)?.try_into().ok()?,
    ))
}

pub(crate) fn le16(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        b.get(o..o.checked_add(2)?)?.try_into().ok()?,
    ))
}

pub(crate) fn le32(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        b.get(o..o.checked_add(4)?)?.try_into().ok()?,
    ))
}
