// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The EXIF of a source that is not a JPEG, as the bare TIFF blob (`II*\0` or `MM\0*` first), so a
//! conversion can carry it into the new file (PLAN 3.6): the `eXIf` chunk of a PNG, the `EXIF` chunk
//! of a WebP and the `Exif` item of a HEIC or AVIF file (the 4-byte TIFF-header offset skipped).
//! A JPEG's EXIF is read by [`crate::jpeg_meta::read_meta`]; TIFF and BMP carry none this reads.
//!
//! Bounded and total: every offset is range-checked, the blob is capped by
//! [`DecodeLimits::max_metadata_bytes`], and anything odd gives `None` (no EXIF is better than a
//! wrong one: a stale Orientation would turn the picture a second time).

use crate::{DecodeLimits, Format};

fn is_tiff(b: &[u8]) -> bool {
    b.starts_with(b"II*\0") || b.starts_with(b"MM\0*")
}

fn be32(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        b.get(o..o.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn le32(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        b.get(o..o.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn png(b: &[u8], cap: u64) -> Option<Vec<u8>> {
    let mut i = 8;
    while i + 8 <= b.len() {
        let len = be32(b, i)? as usize;
        let kind = b.get(i + 4..i + 8)?;
        let data = i + 8;
        if kind == b"eXIf" {
            let blob = b.get(data..data.checked_add(len)?)?;
            return (len as u64 <= cap && is_tiff(blob)).then(|| blob.to_vec());
        }
        if kind == b"IDAT" || kind == b"IEND" {
            // The chunk must precede the image data.
            return None;
        }
        i = data.checked_add(len)?.checked_add(4)?;
    }
    None
}

fn webp(b: &[u8], cap: u64) -> Option<Vec<u8>> {
    let mut i = 12;
    while i + 8 <= b.len() {
        let kind = b.get(i..i + 4)?;
        let len = le32(b, i + 4)? as usize;
        let data = i + 8;
        if kind == b"EXIF" {
            let blob = b.get(data..data.checked_add(len)?)?;
            // Some writers keep the JPEG `Exif\0\0` prefix.
            let blob = blob.strip_prefix(b"Exif\0\0").unwrap_or(blob);
            return (len as u64 <= cap && is_tiff(blob)).then(|| blob.to_vec());
        }
        i = data.checked_add(len)?.checked_add(len & 1)?;
    }
    None
}

fn png_dpi(b: &[u8]) -> Option<(u32, u32)> {
    let mut i = 8;
    while i + 8 <= b.len() {
        let len = be32(b, i)? as usize;
        let kind = b.get(i + 4..i + 8)?;
        let data = i + 8;
        if kind == b"pHYs" && len == 9 && b.get(data + 8) == Some(&1) {
            let per_inch = |v: u32| {
                let d = (f64::from(v) * 0.0254).round();
                (1.0..=100_000.0).contains(&d).then_some(d as u32)
            };
            return per_inch(be32(b, data)?).zip(per_inch(be32(b, data + 4)?));
        }
        if kind == b"IDAT" || kind == b"IEND" {
            return None;
        }
        i = data.checked_add(len)?.checked_add(4)?;
    }
    None
}

/// The pixel density of a PNG (`pHYs`, in metres, as dots per inch) or of a TIFF (its resolution
/// tags), so a re-save keeps the physical size.
pub fn extract_dpi(bytes: &[u8], format: Format) -> Option<(u32, u32)> {
    crate::guard::guard_item(|| {
        Ok(match format {
            Format::Png => png_dpi(bytes),
            Format::Tiff => crate::parse::tiff::read_meta(bytes).and_then(|m| m.dpi),
            _ => None,
        })
    })
    .ok()
    .flatten()
}

pub use crate::parse::tiff::{RawTag, TAG_IPTC, TAG_XMP, TiffMeta};

/// The descriptive tags of a TIFF (resolution, XMP, IPTC, text) and whether it has directories a
/// TIFF this build writes cannot carry.
pub fn extract_tiff_meta(bytes: &[u8]) -> Option<TiffMeta> {
    crate::guard::guard_item(|| Ok(crate::parse::tiff::read_meta(bytes)))
        .ok()
        .flatten()
}

/// The EXIF TIFF blob of `bytes` (a file of `format`), if it has a well-formed one. For a JPEG use
/// [`crate::jpeg_meta::read_meta`].
pub fn extract_exif(bytes: &[u8], format: Format, limits: &DecodeLimits) -> Option<Vec<u8>> {
    let cap = limits.max_metadata_bytes;
    crate::guard::guard_item(|| {
        Ok(match format {
            Format::Png => png(bytes, cap),
            Format::Webp => webp(bytes, cap),
            Format::Heic | Format::Avif => crate::parse::heif::exif_blob(bytes, limits),
            _ => None,
        })
    })
    .ok()
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::exif_blob;

    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        // The reader does not check CRCs; a zero CRC keeps the test file small.
        let mut c = (data.len() as u32).to_be_bytes().to_vec();
        c.extend_from_slice(kind);
        c.extend_from_slice(data);
        c.extend_from_slice(&[0; 4]);
        c
    }

    #[test]
    fn a_png_exif_chunk_is_found_only_before_the_image_data() {
        let blob = exif_blob(6, false);
        let mut f = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        f.extend(chunk(b"IHDR", &[0; 13]));
        f.extend(chunk(b"eXIf", &blob));
        f.extend(chunk(b"IDAT", &[]));
        f.extend(chunk(b"IEND", &[]));
        let l = DecodeLimits::default();
        assert_eq!(extract_exif(&f, Format::Png, &l), Some(blob.clone()));
        // After the image data it is not trusted.
        let mut g = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        g.extend(chunk(b"IHDR", &[0; 13]));
        g.extend(chunk(b"IDAT", &[]));
        g.extend(chunk(b"eXIf", &blob));
        assert_eq!(extract_exif(&g, Format::Png, &l), None);
    }

    #[test]
    fn a_webp_exif_chunk_may_carry_the_jpeg_prefix() {
        let blob = exif_blob(3, false);
        for prefix in [&b""[..], &b"Exif\0\0"[..]] {
            let mut payload = prefix.to_vec();
            payload.extend_from_slice(&blob);
            let mut f = b"RIFF\0\0\0\0WEBP".to_vec();
            f.extend_from_slice(b"VP8 ");
            f.extend_from_slice(&2u32.to_le_bytes());
            f.extend_from_slice(&[0, 0]);
            f.extend_from_slice(b"EXIF");
            f.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            f.extend_from_slice(&payload);
            if payload.len() % 2 == 1 {
                f.push(0);
            }
            assert_eq!(
                extract_exif(&f, Format::Webp, &DecodeLimits::default()),
                Some(blob.clone())
            );
        }
    }

    #[test]
    fn the_exif_item_of_a_heif_file_is_found_and_its_header_offset_skipped() {
        // Written by libavif with an Exif item whose Orientation is 6 and no `irot`.
        let avif = include_bytes!("../tests/fixtures/heif/exif-orient6-noirot-48x32.avif");
        let blob = extract_exif(avif, Format::Avif, &DecodeLimits::default())
            .expect("the fixture carries an Exif item");
        assert!(is_tiff(&blob));
        assert_eq!(crate::exif_orientation(&blob), Some(6));
        // A file with no Exif item has none.
        let plain = include_bytes!("../tests/fixtures/heif/gradient-444-48x32.avif");
        assert_eq!(
            extract_exif(plain, Format::Avif, &DecodeLimits::default()),
            None
        );
    }

    #[test]
    fn garbage_and_other_formats_give_none() {
        let l = DecodeLimits::default();
        assert_eq!(extract_exif(b"not an image", Format::Png, &l), None);
        assert_eq!(extract_exif(b"RIFF", Format::Webp, &l), None);
        assert_eq!(extract_exif(&[0; 40], Format::Heic, &l), None);
        assert_eq!(extract_exif(&[1, 2, 3], Format::Tiff, &l), None);
    }
}
