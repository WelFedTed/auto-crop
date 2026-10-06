// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! BMP header walk (ROADMAP M2.20): the file header, the DIB header (OS/2 core and the Windows
//! `BITMAPINFOHEADER` family) and a plausibility check that an uncompressed bitmap holds the pixel
//! data its size claims, so a 60-byte file cannot make the decoder reserve a huge buffer.

use super::{Header, le16, le32};
use crate::{CodecError, DecodeLimits, Format};

pub(crate) fn parse(b: &[u8], _limits: &DecodeLimits) -> Result<Header, CodecError> {
    let (Some(offset), Some(dib)) = (le32(b, 10), le32(b, 14)) else {
        return Err(CodecError::corrupt("truncated BMP header"));
    };
    let truncated = || CodecError::corrupt("truncated BMP header");
    let (w, h, planes, bpp, compression) = match dib {
        // OS/2 BITMAPCOREHEADER: 16-bit unsigned dimensions.
        12 => (
            i64::from(le16(b, 18).ok_or_else(truncated)?),
            i64::from(le16(b, 20).ok_or_else(truncated)?),
            le16(b, 22).ok_or_else(truncated)?,
            le16(b, 24).ok_or_else(truncated)?,
            0u32,
        ),
        // BITMAPINFOHEADER and its V2 to V5 extensions: signed 32-bit dimensions (a negative
        // height is a top-down bitmap).
        40 | 52 | 56 | 64 | 108 | 124 => (
            i64::from(le32(b, 18).ok_or_else(truncated)? as i32),
            i64::from(le32(b, 22).ok_or_else(truncated)? as i32),
            le16(b, 26).ok_or_else(truncated)?,
            le16(b, 28).ok_or_else(truncated)?,
            le32(b, 30).ok_or_else(truncated)?,
        ),
        _ => return Err(CodecError::corrupt("unknown BMP header size")),
    };
    if w < 0 {
        return Err(CodecError::corrupt("negative BMP width"));
    }
    let (width, height) = (
        u32::try_from(w).map_err(|_| CodecError::corrupt("BMP width out of range"))?,
        u32::try_from(h.unsigned_abs())
            .map_err(|_| CodecError::corrupt("BMP height out of range"))?,
    );
    let mut hdr = Header::new(Format::Bmp, width, height);
    hdr.bit_depth = 8;
    hdr.channels = if bpp == 32 { 4 } else { 3 };
    hdr.compression = u16::try_from(compression).unwrap_or(u16::MAX);
    hdr.unsupported = if planes != 1 {
        Some("BMP with other than one colour plane".into())
    } else if !matches!(bpp, 1 | 4 | 8 | 16 | 24 | 32) {
        Some(format!("{bpp}-bit BMP"))
    } else if matches!(compression, 4 | 5) {
        Some("BMP with embedded JPEG or PNG data".into())
    } else {
        None
    };
    // An uncompressed bitmap must hold `rows x padded row` bytes after the pixel offset. (RLE and
    // bitfield variants carry their own sizes; the decoder reports those.)
    if compression == 0 && width > 0 && height > 0 && hdr.unsupported.is_none() {
        let row = (u64::from(width) * u64::from(bpp)).div_ceil(32) * 4;
        let need = u64::from(offset).saturating_add(row.saturating_mul(u64::from(height)));
        if need > b.len() as u64 {
            return Err(CodecError::corrupt(
                "BMP data is shorter than its header says",
            ));
        }
    }
    Ok(hdr)
}
