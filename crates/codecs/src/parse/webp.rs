// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! WebP RIFF walk: the bitstream header (`VP8 `, `VP8L`) or `VP8X` canvas, plus `EXIF`, `ICCP`,
//! `ANIM` and `ANMF` chunks.

use super::{Header, IccLoc, check_metadata, le16, le32};
use crate::{CodecError, DecodeLimits, Format, Limit, exif};

fn le24(b: &[u8], o: usize) -> Option<u32> {
    let s = b.get(o..o.checked_add(3)?)?;
    Some(u32::from(s[0]) | u32::from(s[1]) << 8 | u32::from(s[2]) << 16)
}

pub(crate) fn parse(b: &[u8], limits: &DecodeLimits) -> Result<Header, CodecError> {
    if b.len() < 20 {
        return Err(CodecError::corrupt("truncated WebP"));
    }
    let riff_len = le32(b, 4).unwrap_or(0) as usize;
    // The RIFF size counts everything after its own 8 bytes; tolerate trailing junk but not a
    // file that stops short of the declared size.
    let end = riff_len.saturating_add(8);
    let truncated = end > b.len();
    let end = end.min(b.len());

    let mut h = Header::new(Format::Webp, 0, 0);
    let mut have_dims = false;
    let mut anmf = 0u32;
    let mut have_exif = false;
    let mut i = 12;
    while i + 8 <= end {
        let kind = &b[i..i + 4];
        let len = le32(b, i + 4).unwrap_or(0) as usize;
        let data = i + 8;
        let data_end = data.saturating_add(len);
        let in_file = data_end <= end;
        match kind {
            b"VP8X" if len >= 10 => {
                let flags = b[data];
                let w = le24(b, data + 4).unwrap_or(0) + 1;
                let ht = le24(b, data + 7).unwrap_or(0) + 1;
                h.width = w;
                h.height = ht;
                have_dims = true;
                h.animated = flags & 0x02 != 0;
                h.channels = if flags & 0x10 != 0 { 4 } else { 3 };
            }
            b"VP8 " if !have_dims && len >= 10 => {
                // Frame tag (3 bytes), start code 9D 01 2A, then 14-bit width and height.
                if b.get(data + 3..data + 6) != Some(&[0x9D, 0x01, 0x2A]) {
                    return Err(CodecError::corrupt("bad VP8 start code"));
                }
                h.width = u32::from(le16(b, data + 6).unwrap_or(0) & 0x3FFF);
                h.height = u32::from(le16(b, data + 8).unwrap_or(0) & 0x3FFF);
                have_dims = true;
                h.channels = 3;
            }
            b"VP8L" if !have_dims && len >= 5 => {
                if b.get(data) != Some(&0x2F) {
                    return Err(CodecError::corrupt("bad VP8L signature"));
                }
                let bits = le32(b, data + 1).unwrap_or(0);
                h.width = (bits & 0x3FFF) + 1;
                h.height = ((bits >> 14) & 0x3FFF) + 1;
                have_dims = true;
                h.channels = if (bits >> 28) & 1 == 1 { 4 } else { 3 };
            }
            b"ALPH" => h.channels = 4,
            b"ANMF" => {
                anmf += 1;
                if anmf > limits.max_frames {
                    return Err(CodecError::LimitExceeded {
                        limit: Limit::Frames,
                        actual: u64::from(anmf),
                        cap: u64::from(limits.max_frames),
                    });
                }
            }
            b"EXIF" if !have_exif => {
                have_exif = true;
                check_metadata(len as u64, limits)?;
                if in_file {
                    h.orientation = exif::orientation(&b[data..data_end]).unwrap_or(1);
                }
            }
            b"ICCP" => {
                check_metadata(len as u64, limits)?;
                if in_file && h.icc == IccLoc::None {
                    h.icc = IccLoc::Range(data..data_end);
                    h.icc_len = u32::try_from(len).ok();
                }
            }
            _ => {}
        }
        if !in_file {
            h.truncated = true;
            break;
        }
        // Chunks are padded to an even size.
        i = data_end.saturating_add(len & 1);
    }
    if !have_dims {
        return Err(CodecError::corrupt("WebP has no image header"));
    }
    h.truncated |= truncated;
    h.frames = anmf.max(1);
    h.animated |= anmf > 0;
    Ok(h)
}
