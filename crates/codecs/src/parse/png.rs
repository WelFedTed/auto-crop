// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! PNG chunk walk: IHDR, `eXIf`, `iCCP` and `acTL`. Chunk contents are never inflated here.

use super::{Header, IccLoc, be32, check_metadata};
use crate::{CodecError, DecodeLimits, Format, Limit, exif};

pub(crate) fn parse(b: &[u8], limits: &DecodeLimits) -> Result<Header, CodecError> {
    // 8 signature bytes, then IHDR: length, "IHDR", 13 bytes of data.
    if b.len() < 8 + 8 + 13 || &b[12..16] != b"IHDR" || be32(b, 8) != Some(13) {
        return Err(CodecError::corrupt("missing PNG header"));
    }
    let width = be32(b, 16).unwrap_or(0);
    let height = be32(b, 20).unwrap_or(0);
    let (depth, colour, interlace) = (b[24], b[25], b[28]);
    let channels = match colour {
        0 | 3 => 1,
        4 => 2,
        2 => 3,
        6 => 4,
        _ => return Err(CodecError::corrupt("invalid PNG colour type")),
    };
    let mut h = Header::new(Format::Png, width, height);
    h.bit_depth = depth;
    h.channels = channels;
    if interlace > 1 {
        return Err(CodecError::corrupt("invalid PNG interlace method"));
    }

    let mut i = 8 + 8 + 13 + 4; // past IHDR and its CRC
    let mut have_exif = false;
    let mut ended = false;
    while i + 8 <= b.len() {
        let len = be32(b, i).unwrap_or(0) as usize;
        let kind = &b[i + 4..i + 8];
        let data = i + 8;
        let next = data.saturating_add(len).saturating_add(4); // data + CRC
        if len > 0x7FFF_FFFF {
            return Err(CodecError::corrupt("invalid PNG chunk length"));
        }
        match kind {
            b"IEND" => {
                ended = true;
                break;
            }
            b"acTL" if len >= 8 => {
                let frames = be32(b, data).unwrap_or(1);
                if frames > limits.max_frames {
                    return Err(CodecError::LimitExceeded {
                        limit: Limit::Frames,
                        actual: u64::from(frames),
                        cap: u64::from(limits.max_frames),
                    });
                }
                h.frames = frames.max(1);
                h.animated = frames > 1;
            }
            b"eXIf" if !have_exif => {
                have_exif = true;
                check_metadata(len as u64, limits)?;
                if let Some(blob) = b.get(data..data.saturating_add(len)) {
                    h.orientation = exif::orientation(blob).unwrap_or(1);
                }
            }
            b"iCCP" => {
                check_metadata(len as u64, limits)?;
                if h.icc == IccLoc::None {
                    h.icc = IccLoc::PngCompressed;
                    h.icc_len = u32::try_from(len).ok();
                }
            }
            _ => {}
        }
        if next > b.len() {
            h.truncated = true;
            break;
        }
        i = next;
    }
    if !ended {
        h.truncated = true;
    }
    Ok(h)
}
