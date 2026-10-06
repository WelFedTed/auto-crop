// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! BMP decoding (ROADMAP M2.20): bit-exact for 24-bit files in both row orders, header caps and
//! hostile headers refused before any pixel buffer exists.

use crate::fixtures::{bmp_rgb24, pattern};
use crate::{CodecError, Format, decode, probe};

#[test]
fn a_24_bit_bmp_decodes_bit_exactly_bottom_up_and_top_down() {
    let (w, h) = (13, 9); // a width that needs row padding
    let rgb = pattern(w, h);
    for top_down in [false, true] {
        let bmp = bmp_rgb24(w, h, &rgb, top_down);
        let p = probe(&bmp).unwrap();
        assert_eq!(
            (p.format, p.width, p.height, p.frames),
            (Format::Bmp, w, h, 1)
        );
        let d = decode(&bmp).unwrap();
        assert_eq!((d.raster.width, d.raster.height), (w, h));
        assert_eq!(d.raster.data, rgb, "top_down={top_down}");
        assert_eq!(d.exif_orientation, 1);
        assert!(d.icc.is_none());
    }
}

#[test]
fn a_truncated_bmp_is_corrupt_not_a_grey_picture() {
    let (w, h) = (16, 16);
    let bmp = bmp_rgb24(w, h, &pattern(w, h), false);
    for cut in [bmp.len() - 1, bmp.len() / 2, 60, 54] {
        assert!(
            matches!(decode(&bmp[..cut]), Err(CodecError::Corrupt(_))),
            "cut at {cut}"
        );
    }
}

#[test]
fn a_bomb_header_is_refused_before_any_allocation() {
    // 60000 x 60000 in a 100-byte file.
    let mut bmp = bmp_rgb24(2, 2, &pattern(2, 2), false);
    bmp[18..22].copy_from_slice(&60_000i32.to_le_bytes());
    bmp[22..26].copy_from_slice(&60_000i32.to_le_bytes());
    assert!(decode(&bmp).is_err());
    // Even a file that is long enough for its pixel data is refused above the pixel cap.
    let limits = crate::DecodeLimits::default().with_max_pixels(100);
    let ok = bmp_rgb24(20, 20, &pattern(20, 20), false);
    assert!(matches!(
        crate::decode_with(&ok, &limits),
        Err(CodecError::TooLarge(_))
    ));
}

#[test]
fn odd_headers_never_panic() {
    let bmp = bmp_rgb24(8, 8, &pattern(8, 8), false);
    for i in 0..60 {
        for v in [0u8, 1, 0x7F, 0x80, 0xFF] {
            let mut b = bmp.clone();
            b[i] = v;
            let _ = probe(&b);
            let _ = decode(&b);
        }
    }
    // A negative width, an unsupported depth, embedded JPEG data.
    let mut b = bmp.clone();
    b[18..22].copy_from_slice(&(-8i32).to_le_bytes());
    assert!(decode(&b).is_err());
    let mut b = bmp.clone();
    b[28..30].copy_from_slice(&7u16.to_le_bytes());
    assert!(decode(&b).is_err());
    let mut b = bmp;
    b[30..34].copy_from_slice(&4u32.to_le_bytes());
    assert!(decode(&b).is_err());
}
