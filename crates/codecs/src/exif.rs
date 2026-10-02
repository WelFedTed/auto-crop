// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Minimal, bounded EXIF reader: only the Orientation tag of IFD0 (PLAN 3.6).
//!
//! It never follows a second IFD, a sub-IFD or any pointer other than the IFD0 offset, so a cyclic
//! or flooded EXIF structure cannot loop or allocate: the work is at most one read per IFD0 entry,
//! and the entry count is clamped to what fits in the blob.

const TAG_ORIENTATION: u16 = 0x0112;

/// Returns the EXIF orientation (1..=8) of a TIFF-structured EXIF blob, with or without the
/// JPEG `Exif\0\0` prefix. `None` for a missing, malformed or out-of-range value.
pub fn orientation(blob: &[u8]) -> Option<u8> {
    let blob = blob.strip_prefix(b"Exif\0\0").unwrap_or(blob);
    let le = match blob.get(..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16_at = |o: usize| -> Option<u16> {
        let b: [u8; 2] = blob.get(o..o.checked_add(2)?)?.try_into().ok()?;
        Some(if le {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    };
    let u32_at = |o: usize| -> Option<u32> {
        let b: [u8; 4] = blob.get(o..o.checked_add(4)?)?.try_into().ok()?;
        Some(if le {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    };
    if u16_at(2)? != 42 {
        return None;
    }
    let ifd = u32_at(4)? as usize;
    let declared = usize::from(u16_at(ifd)?);
    let fits = blob.len().saturating_sub(ifd.checked_add(2)?) / 12;
    for i in 0..declared.min(fits) {
        let e = ifd + 2 + i * 12;
        if u16_at(e)? == TAG_ORIENTATION {
            // type 3 (SHORT), count 1: the value sits in the first two bytes of the value field.
            if u16_at(e + 2)? != 3 || u32_at(e + 4)? != 1 {
                return None;
            }
            return match u16_at(e + 8)? {
                v @ 1..=8 => Some(v as u8),
                _ => None,
            };
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a TIFF blob with an Orientation entry.
    fn blob(orientation: u16, le: bool) -> Vec<u8> {
        let mut v = Vec::new();
        let p16 = |v: &mut Vec<u8>, x: u16| {
            v.extend_from_slice(&if le { x.to_le_bytes() } else { x.to_be_bytes() })
        };
        let p32 = |v: &mut Vec<u8>, x: u32| {
            v.extend_from_slice(&if le { x.to_le_bytes() } else { x.to_be_bytes() })
        };
        v.extend_from_slice(if le { b"II" } else { b"MM" });
        p16(&mut v, 42);
        p32(&mut v, 8);
        p16(&mut v, 1);
        p16(&mut v, TAG_ORIENTATION);
        p16(&mut v, 3);
        p32(&mut v, 1);
        p16(&mut v, orientation);
        p16(&mut v, 0);
        p32(&mut v, 0);
        v
    }

    #[test]
    fn reads_both_byte_orders_and_the_jpeg_prefix() {
        for o in 1..=8u16 {
            assert_eq!(orientation(&blob(o, true)), Some(o as u8));
            assert_eq!(orientation(&blob(o, false)), Some(o as u8));
        }
        let mut prefixed = b"Exif\0\0".to_vec();
        prefixed.extend(blob(6, false));
        assert_eq!(orientation(&prefixed), Some(6));
    }

    #[test]
    fn out_of_range_and_malformed_blobs_give_none() {
        assert_eq!(orientation(&blob(0, true)), None);
        assert_eq!(orientation(&blob(9, true)), None);
        assert_eq!(orientation(&[]), None);
        assert_eq!(orientation(b"II"), None);
        assert_eq!(orientation(b"XX\x2a\0\x08\0\0\0"), None);
        let b = blob(6, true);
        for cut in 0..b.len() {
            let _ = orientation(&b[..cut]); // never panics
        }
    }

    #[test]
    fn a_huge_entry_count_is_clamped_to_the_blob() {
        let mut b = blob(6, true);
        b[8] = 0xFF;
        b[9] = 0xFF; // 65535 declared entries, 1 present
        assert_eq!(orientation(&b), Some(6));
        // A self-referencing next-IFD pointer is never followed.
        let n = b.len();
        b[n - 4..].copy_from_slice(&8u32.to_le_bytes());
        assert_eq!(orientation(&b), Some(6));
    }
}
