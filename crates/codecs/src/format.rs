// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Format identification by magic bytes (PLAN 3.1 rule 2): file names are never trusted.

/// An image container this crate can name. JPEG, PNG, TIFF and WebP are decodable; HEIC and AVIF
/// are decodable with the `heif` feature and probed (header only) without it; the rest are
/// recognised only, so the caller can say "this is a GIF" instead of "unknown file".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    Jpeg,
    Png,
    Tiff,
    Webp,
    /// HEIC / HEIF (decoded by libheif with the `heif` feature).
    Heic,
    /// AVIF (decoded by libheif and dav1d with the `heif` feature).
    Avif,
    /// GIF (recognised only).
    Gif,
    /// BMP (recognised only).
    Bmp,
    /// JPEG XL, codestream or container (recognised only).
    Jxl,
}

impl Format {
    /// File extension for output files (and a stable lowercase name).
    pub fn extension(self) -> &'static str {
        match self {
            Format::Jpeg => "jpg",
            Format::Png => "png",
            Format::Tiff => "tif",
            Format::Webp => "webp",
            Format::Heic => "heic",
            Format::Avif => "avif",
            Format::Gif => "gif",
            Format::Bmp => "bmp",
            Format::Jxl => "jxl",
        }
    }

    /// True for the formats [`crate::decode`] reads in this build: JPEG, PNG, TIFF, WebP and BMP
    /// always, HEIC and AVIF with the `heif` feature (libheif, libde265 and dav1d).
    pub fn is_decodable(self) -> bool {
        matches!(
            self,
            Format::Jpeg | Format::Png | Format::Tiff | Format::Webp | Format::Bmp
        ) || (cfg!(feature = "heif") && matches!(self, Format::Heic | Format::Avif))
    }

    /// True for the formats [`crate::probe`] reads: everything decodable, plus HEIC and AVIF in
    /// every build (their header walk is safe Rust and needs no native library).
    pub fn is_probeable(self) -> bool {
        self.is_decodable() || matches!(self, Format::Heic | Format::Avif)
    }

    /// True for the formats [`crate::encode`] writes in this build.
    pub fn is_encodable(self) -> bool {
        matches!(self, Format::Jpeg | Format::Png)
    }
}

const PNG_SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
const JXL_CONTAINER: [u8; 12] = [
    0x00, 0x00, 0x00, 0x0C, b'J', b'X', b'L', b' ', 0x0D, 0x0A, 0x87, 0x0A,
];

/// ISO-BMFF brands (PLAN 3.1) that mean HEIF with HEVC or a generic HEIF image.
const HEIC_BRANDS: [&[u8; 4]; 8] = [
    b"heic", b"heix", b"hevc", b"hevx", b"heim", b"heis", b"mif1", b"msf1",
];
const AVIF_BRANDS: [&[u8; 4]; 2] = [b"avif", b"avis"];

/// Identifies the format from the magic bytes (never from the file name).
pub fn sniff(bytes: &[u8]) -> Option<Format> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(Format::Jpeg);
    }
    if bytes.starts_with(&PNG_SIG) {
        return Some(Format::Png);
    }
    if bytes.len() >= 4 && matches!(&bytes[..4], b"II*\0" | b"MM\0*" | b"II+\0" | b"MM\0+") {
        return Some(Format::Tiff);
    }
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some(Format::Webp);
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some(Format::Gif);
    }
    if bytes.starts_with(&[0xFF, 0x0A]) || bytes.starts_with(&JXL_CONTAINER) {
        return Some(Format::Jxl);
    }
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        return sniff_ftyp(bytes);
    }
    if is_bmp(bytes) {
        return Some(Format::Bmp);
    }
    None
}

fn is_bmp(bytes: &[u8]) -> bool {
    // "BM" is too short to trust alone: also require a known DIB header size.
    if bytes.len() < 18 || &bytes[..2] != b"BM" {
        return false;
    }
    let dib = u32::from_le_bytes([bytes[14], bytes[15], bytes[16], bytes[17]]);
    matches!(dib, 12 | 40 | 52 | 56 | 64 | 108 | 124)
}

fn sniff_ftyp(bytes: &[u8]) -> Option<Format> {
    let size = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    // The box covers `size` bytes (0 means "to the end"); only ever look at the first 64 brands.
    let end = if size < 16 { 16.min(bytes.len()) } else { size }
        .min(bytes.len())
        .min(16 + 64 * 4);
    let major = &bytes[8..12];
    let mut brands: Vec<&[u8]> = vec![major];
    // The compact `mini` layout (major brand `mif3`) names its codec in the minor_version field
    // ("avif", "heic") instead of in a compatible-brand list.
    if major == b"mif3" && bytes.len() >= 16 {
        brands.push(&bytes[12..16]);
    }
    // Skip minor_version (bytes 12..16), then compatible brands.
    let mut i = 16;
    while i + 4 <= end {
        brands.push(&bytes[i..i + 4]);
        i += 4;
    }
    let is_avif = |b: &&[u8]| AVIF_BRANDS.iter().any(|a| a.as_slice() == *b);
    let is_heic = |b: &&[u8]| HEIC_BRANDS.iter().any(|a| a.as_slice() == *b);
    if is_avif(&brands[0]) {
        Some(Format::Avif)
    } else if is_heic(&brands[0]) && !brands[1..].iter().any(is_avif) {
        Some(Format::Heic)
    } else if brands.iter().any(is_avif) {
        Some(Format::Avif)
    } else if brands.iter().any(is_heic) {
        Some(Format::Heic)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ftyp(major: &[u8; 4], compat: &[&[u8; 4]]) -> Vec<u8> {
        let mut v = Vec::new();
        let size = 16 + 4 * compat.len();
        v.extend_from_slice(&(size as u32).to_be_bytes());
        v.extend_from_slice(b"ftyp");
        v.extend_from_slice(major);
        v.extend_from_slice(&[0, 0, 0, 0]);
        for c in compat {
            v.extend_from_slice(*c);
        }
        v
    }

    #[test]
    fn magic_bytes_identify_every_family() {
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0]), Some(Format::Jpeg));
        assert_eq!(sniff(&PNG_SIG), Some(Format::Png));
        assert_eq!(sniff(b"II*\0\x08\0\0\0"), Some(Format::Tiff));
        assert_eq!(sniff(b"MM\0*\0\0\0\x08"), Some(Format::Tiff));
        assert_eq!(sniff(b"II+\0\x08\0\0\0"), Some(Format::Tiff));
        assert_eq!(sniff(b"RIFF\x10\0\0\0WEBPVP8L"), Some(Format::Webp));
        assert_eq!(sniff(b"GIF89a\x01\0\x01\0"), Some(Format::Gif));
        assert_eq!(sniff(&[0xFF, 0x0A, 0, 0]), Some(Format::Jxl));
        assert_eq!(sniff(&JXL_CONTAINER), Some(Format::Jxl));
    }

    #[test]
    fn heif_brands_split_heic_from_avif() {
        assert_eq!(
            sniff(&ftyp(b"heic", &[b"mif1", b"heic"])),
            Some(Format::Heic)
        );
        assert_eq!(sniff(&ftyp(b"mif1", &[b"heix"])), Some(Format::Heic));
        assert_eq!(
            sniff(&ftyp(b"avif", &[b"mif1", b"miaf"])),
            Some(Format::Avif)
        );
        assert_eq!(sniff(&ftyp(b"mif1", &[b"avif"])), Some(Format::Avif));
        assert_eq!(sniff(&ftyp(b"qt  ", &[])), None);
    }

    #[test]
    fn the_mini_layout_names_its_codec_in_the_minor_version() {
        let mini = |minor: &[u8; 4]| {
            let mut v = vec![0, 0, 0, 16];
            v.extend_from_slice(b"ftypmif3");
            v.extend_from_slice(minor);
            v
        };
        assert_eq!(sniff(&mini(b"avif")), Some(Format::Avif));
        assert_eq!(sniff(&mini(b"heic")), Some(Format::Heic));
        assert_eq!(sniff(&mini(b"abcd")), None);
        // Too short to carry a minor version: no panic, no guess.
        assert_eq!(sniff(b"\0\0\0\x10ftypmif3"), None);
    }

    #[test]
    fn bmp_needs_a_plausible_dib_header() {
        let mut bmp = b"BM".to_vec();
        bmp.extend_from_slice(&[0; 12]);
        bmp.extend_from_slice(&40u32.to_le_bytes());
        assert_eq!(sniff(&bmp), Some(Format::Bmp));
        assert_eq!(sniff(b"BM is a text file that starts with BM"), None);
    }

    #[test]
    fn junk_and_short_input_are_unrecognised() {
        assert_eq!(sniff(&[]), None);
        assert_eq!(sniff(&[0xFF]), None);
        assert_eq!(sniff(b"RIFF\0\0\0\0WAVE"), None);
        assert_eq!(sniff(b"%PDF-1.7"), None);
        assert_eq!(sniff(&[0; 64]), None);
    }
}
