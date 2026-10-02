// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! TIFF and BigTIFF header walk.
//!
//! Only IFD0 is read entry by entry. Later IFDs are visited just to count them (entry count, then a
//! jump to the next-IFD pointer), with a visited set so a cyclic chain is an error and a cap on the
//! number of IFDs. No sub-IFD (EXIF, GPS) pointer is ever followed.

use super::{Header, IccLoc, check_metadata};
use crate::{CodecError, DecodeLimits, Format, Limit};
use std::collections::HashSet;

const TAG_WIDTH: u16 = 256;
const TAG_HEIGHT: u16 = 257;
const TAG_BITS: u16 = 258;
const TAG_COMPRESSION: u16 = 259;
const TAG_ORIENTATION: u16 = 274;
const TAG_SAMPLES: u16 = 277;
const TAG_ICC: u16 = 34675;

struct Tiff<'a> {
    b: &'a [u8],
    le: bool,
    big: bool,
}

impl Tiff<'_> {
    fn u16(&self, o: usize) -> Option<u16> {
        let v: [u8; 2] = self.b.get(o..o.checked_add(2)?)?.try_into().ok()?;
        Some(if self.le {
            u16::from_le_bytes(v)
        } else {
            u16::from_be_bytes(v)
        })
    }

    fn u32(&self, o: usize) -> Option<u32> {
        let v: [u8; 4] = self.b.get(o..o.checked_add(4)?)?.try_into().ok()?;
        Some(if self.le {
            u32::from_le_bytes(v)
        } else {
            u32::from_be_bytes(v)
        })
    }

    fn u64(&self, o: usize) -> Option<u64> {
        let v: [u8; 8] = self.b.get(o..o.checked_add(8)?)?.try_into().ok()?;
        Some(if self.le {
            u64::from_le_bytes(v)
        } else {
            u64::from_be_bytes(v)
        })
    }

    /// Size of one IFD entry.
    fn entry_size(&self) -> usize {
        if self.big { 20 } else { 12 }
    }

    /// Number of entries and the offset of the first one, for the IFD at `off`.
    fn ifd_head(&self, off: usize) -> Option<(u64, usize)> {
        if self.big {
            Some((self.u64(off)?, off.checked_add(8)?))
        } else {
            Some((u64::from(self.u16(off)?), off.checked_add(2)?))
        }
    }

    fn offset_at(&self, o: usize) -> Option<u64> {
        if self.big {
            self.u64(o)
        } else {
            self.u32(o).map(u64::from)
        }
    }
}

/// One decoded IFD entry.
struct Entry {
    tag: u16,
    ty: u16,
    count: u64,
    /// Offset of the entry's value field (inside the entry).
    field: usize,
}

fn type_size(ty: u16) -> Option<u64> {
    Some(match ty {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 | 13 => 4,
        5 | 10 | 12 | 16 | 17 | 18 => 8,
        _ => return None,
    })
}

impl Tiff<'_> {
    fn entry(&self, ifd_first: usize, i: usize) -> Option<Entry> {
        let e = ifd_first.checked_add(i.checked_mul(self.entry_size())?)?;
        let tag = self.u16(e)?;
        let ty = self.u16(e + 2)?;
        let (count, field) = if self.big {
            (self.u64(e + 4)?, e + 12)
        } else {
            (u64::from(self.u32(e + 4)?), e + 8)
        };
        Some(Entry {
            tag,
            ty,
            count,
            field,
        })
    }

    /// Where the entry's data lives: inline in the field or at the offset it holds.
    fn data_range(&self, e: &Entry) -> Option<std::ops::Range<usize>> {
        let total = type_size(e.ty)?.checked_mul(e.count)?;
        let inline = if self.big { 8 } else { 4 };
        let start = if total <= inline {
            e.field
        } else {
            usize::try_from(self.offset_at(e.field)?).ok()?
        };
        let end = start.checked_add(usize::try_from(total).ok()?)?;
        (end <= self.b.len()).then_some(start..end)
    }

    /// The first value of an unsigned-integer entry (BYTE, SHORT, LONG, LONG8).
    fn first_uint(&self, e: &Entry) -> Option<u64> {
        if e.count == 0 {
            return None;
        }
        let r = self.data_range(e)?;
        match e.ty {
            1 => self.b.get(r.start).copied().map(u64::from),
            3 => self.u16(r.start).map(u64::from),
            4 => self.u32(r.start).map(u64::from),
            16 => self.u64(r.start),
            _ => None,
        }
    }
}

pub(crate) fn parse(b: &[u8], limits: &DecodeLimits) -> Result<Header, CodecError> {
    let le = match b.get(..2) {
        Some(b"II") => true,
        Some(b"MM") => false,
        _ => return Err(CodecError::corrupt("bad TIFF byte order")),
    };
    let mut t = Tiff { b, le, big: false };
    let first = match t.u16(2) {
        Some(42) => u64::from(t.u32(4).ok_or_else(|| CodecError::corrupt("short TIFF"))?),
        Some(43) => {
            t.big = true;
            if t.u16(4) != Some(8) {
                return Err(CodecError::corrupt("bad BigTIFF header"));
            }
            t.u64(8)
                .ok_or_else(|| CodecError::corrupt("short BigTIFF"))?
        }
        _ => return Err(CodecError::corrupt("bad TIFF version")),
    };

    let mut seen: HashSet<u64> = HashSet::new();
    let mut off = first;
    let mut frames = 0u32;
    let mut h = Header::new(Format::Tiff, 0, 0);
    h.channels = 1;
    h.bit_depth = 1;
    let (mut width, mut height) = (None, None);

    while off != 0 {
        if !seen.insert(off) {
            return Err(CodecError::corrupt("cyclic TIFF directory chain"));
        }
        frames += 1;
        if frames > limits.max_frames {
            return Err(CodecError::LimitExceeded {
                limit: Limit::Frames,
                actual: u64::from(frames),
                cap: u64::from(limits.max_frames),
            });
        }
        let at = usize::try_from(off).map_err(|_| CodecError::corrupt("TIFF offset overflow"))?;
        let (count, first_entry) = t
            .ifd_head(at)
            .ok_or_else(|| CodecError::corrupt("TIFF directory out of range"))?;
        // Entries plus the next-IFD pointer must lie inside the file.
        let table = count
            .checked_mul(t.entry_size() as u64)
            .and_then(|x| x.checked_add(first_entry as u64))
            .ok_or_else(|| CodecError::corrupt("TIFF entry count overflow"))?;
        let ptr_end = table.saturating_add(if t.big { 8 } else { 4 });
        if ptr_end > b.len() as u64 {
            return Err(CodecError::corrupt("TIFF directory runs past the end"));
        }
        if frames == 1 {
            for i in 0..count as usize {
                let Some(e) = t.entry(first_entry, i) else {
                    break;
                };
                match e.tag {
                    TAG_WIDTH => width = t.first_uint(&e),
                    TAG_HEIGHT => height = t.first_uint(&e),
                    TAG_BITS => {
                        h.bit_depth = t.first_uint(&e).unwrap_or(1).min(255) as u8;
                    }
                    TAG_COMPRESSION => {
                        h.compression = t.first_uint(&e).unwrap_or(0).min(65535) as u16;
                    }
                    TAG_SAMPLES => {
                        h.channels = t.first_uint(&e).unwrap_or(1).min(255) as u8;
                    }
                    TAG_ORIENTATION => {
                        h.orientation = match t.first_uint(&e) {
                            Some(v @ 1..=8) => v as u8,
                            _ => 1,
                        };
                    }
                    TAG_ICC => {
                        let len = type_size(e.ty).unwrap_or(1).saturating_mul(e.count);
                        check_metadata(len, limits)?;
                        if let Some(r) = t.data_range(&e) {
                            h.icc_len = u32::try_from(r.len()).ok();
                            h.icc = IccLoc::Range(r);
                        }
                    }
                    _ => {}
                }
            }
        }
        off = t
            .offset_at(table as usize)
            .ok_or_else(|| CodecError::corrupt("TIFF next-directory pointer out of range"))?;
    }

    let dim = |v: Option<u64>, what: &str| -> Result<u32, CodecError> {
        v.and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| CodecError::corrupt(format!("TIFF is missing a valid {what}")))
    };
    h.width = dim(width, "width")?;
    h.height = dim(height, "height")?;
    h.frames = frames;
    h.animated = frames > 1;
    Ok(h)
}
