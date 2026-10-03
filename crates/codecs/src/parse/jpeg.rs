// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! JPEG marker walk: frame header, EXIF orientation, multi-segment ICC and the scan count.
//!
//! The walk skips entropy-coded data with a linear scan for the next marker, so its cost is one pass
//! over the file; it stops at the first end-of-image marker, and as soon as the scan count passes
//! the cap.

use super::{Header, IccLoc, be16, check_metadata};
use crate::{CodecError, DecodeLimits, Format, Limit, exif};

const ICC_TAG: &[u8] = b"ICC_PROFILE\0";
const EXIF_TAG: &[u8] = b"Exif\0\0";

pub(crate) fn parse(b: &[u8], limits: &DecodeLimits) -> Result<Header, CodecError> {
    let n = b.len();
    let mut i = 2; // after SOI
    let mut hdr: Option<Header> = None;
    let mut scans = 0u32;
    // A sequential frame (SOF0/SOF1) with a subsampled component, and a scan that carries fewer
    // components than the frame (one scan per component).
    let mut sequential_subsampled = false;
    let mut non_interleaved = false;
    let mut eoi = false;
    let mut orientation = 1u8;
    let mut have_exif = false;
    let mut icc_chunks: Vec<(u8, u8, std::ops::Range<usize>)> = Vec::new();

    'walk: while i < n {
        if b[i] != 0xFF {
            return Err(CodecError::corrupt("expected a JPEG marker"));
        }
        while i < n && b[i] == 0xFF {
            i += 1; // fill bytes
        }
        let Some(&m) = b.get(i) else { break };
        i += 1;
        match m {
            0x00 | 0xD8 => return Err(CodecError::corrupt("misplaced JPEG marker")),
            0x01 | 0xD0..=0xD7 => continue,
            0xD9 => {
                eoi = true;
                break;
            }
            _ => {}
        }
        let len = usize::from(be16(b, i).ok_or_else(|| CodecError::corrupt("truncated JPEG"))?);
        if len < 2 || i + len > n {
            return Err(CodecError::corrupt("truncated or malformed JPEG segment"));
        }
        let seg = &b[i + 2..i + len];
        let seg_start = i + 2;
        i += len;
        match m {
            0xC0..=0xCF if m != 0xC4 && m != 0xC8 && m != 0xCC => {
                if hdr.is_some() {
                    continue;
                }
                if seg.len() < 6 {
                    return Err(CodecError::corrupt("short JPEG frame header"));
                }
                let precision = seg[0];
                let height = u32::from(be16(seg, 1).unwrap_or(0));
                let width = u32::from(be16(seg, 3).unwrap_or(0));
                let comps = seg[5];
                if comps == 0 || seg.len() < 6 + 3 * usize::from(comps) {
                    return Err(CodecError::corrupt("bad JPEG component count"));
                }
                sequential_subsampled = matches!(m, 0xC0 | 0xC1)
                    && seg[6..]
                        .as_chunks::<3>()
                        .0
                        .iter()
                        .take(usize::from(comps))
                        .any(|c| c[1] != 0x11);
                let mut h = Header::new(Format::Jpeg, width, height);
                h.bit_depth = precision;
                h.channels = comps;
                h.unsupported = if height == 0 {
                    Some("JPEG with a DNL marker (height defined after the scan)".into())
                } else if m >= 0xC9 {
                    Some("arithmetic-coded JPEG".into())
                } else if matches!(m, 0xC3 | 0xC5..=0xC7) {
                    Some("lossless or hierarchical JPEG".into())
                } else if precision != 8 {
                    Some(format!("{precision}-bit JPEG"))
                } else {
                    None
                };
                hdr = Some(h);
            }
            0xDA => {
                scans += 1;
                if let Some(h) = &hdr {
                    non_interleaved |=
                        sequential_subsampled && seg.first().is_some_and(|&ns| ns < h.channels);
                }
                if scans > limits.max_scans {
                    return Err(CodecError::LimitExceeded {
                        limit: Limit::Scans,
                        actual: u64::from(scans),
                        cap: u64::from(limits.max_scans),
                    });
                }
                // Skip the entropy-coded data up to the next real marker.
                loop {
                    match b[i..].iter().position(|&x| x == 0xFF) {
                        None => break 'walk,
                        Some(p) => {
                            i += p;
                            match b.get(i + 1) {
                                None => break 'walk,
                                Some(0x00) | Some(0xD0..=0xD7) => i += 2,
                                Some(_) => break,
                            }
                        }
                    }
                }
            }
            0xE1 if seg.starts_with(EXIF_TAG) && !have_exif => {
                have_exif = true;
                check_metadata(seg.len() as u64, limits)?;
                orientation = exif::orientation(&seg[EXIF_TAG.len()..]).unwrap_or(1);
            }
            0xE2 if seg.starts_with(ICC_TAG) && seg.len() >= ICC_TAG.len() + 2 => {
                let (seq, total) = (seg[ICC_TAG.len()], seg[ICC_TAG.len() + 1]);
                if seq >= 1 && !icc_chunks.iter().any(|c| c.0 == seq) {
                    let start = seg_start + ICC_TAG.len() + 2;
                    icc_chunks.push((seq, total, start..seg_start + seg.len()));
                }
            }
            _ => {}
        }
    }

    let mut h = hdr.ok_or_else(|| CodecError::corrupt("JPEG has no frame header"))?;
    h.scans = scans;
    if non_interleaved && h.unsupported.is_none() {
        // zune-jpeg 0.5.15 panics (out-of-bounds output rows in its AVX2 IDCT and upsampler, found
        // by the `limits` fuzz target, M1.70) on a subsampled sequential JPEG with one scan per
        // component, so such a file is refused up front instead of failing as a caught decoder
        // panic. Unsubsampled (4:4:4) files with one scan per component decode correctly.
        h.unsupported = Some("non-interleaved subsampled JPEG (one scan per component)".into());
    }
    h.orientation = orientation;
    h.truncated = !eoi;
    if !icc_chunks.is_empty() {
        icc_chunks.sort_by_key(|c| c.0);
        let total: u64 = icc_chunks.iter().map(|c| c.2.len() as u64).sum();
        check_metadata(total, limits)?;
        h.icc_len = u32::try_from(total).ok();
        h.icc = IccLoc::JpegChunks(icc_chunks);
    }
    Ok(h)
}

/// Plausibility: every 8x8 luma block costs at least one bit of entropy-coded data, so a file far
/// smaller than that cannot hold the image its header claims (a bomb header over a tiny scan, or a
/// truncated file). Without this check the decoders "succeed" with grey fill.
pub(crate) fn check_plausible(h: &Header, file_len: usize) -> Result<(), CodecError> {
    let blocks = u64::from(h.width.div_ceil(8)) * u64::from(h.height.div_ceil(8));
    if (file_len as u64) < blocks / 8 {
        Err(CodecError::corrupt(
            "JPEG data is too short for the dimensions in its header",
        ))
    } else {
        Ok(())
    }
}
