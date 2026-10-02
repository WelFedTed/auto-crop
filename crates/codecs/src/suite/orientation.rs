// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! ROADMAP M1.17: EXIF orientation 1-8 across JPEG, TIFF, WebP and PNG, applied exactly once and
//! compared with an independent reference (`fixtures::orient_reference`).

use crate::decode;
use crate::fixtures::*;

const W: u32 = 23;
const H: u32 = 14;

fn jpeg_with(o: u16) -> Vec<u8> {
    let mut s = JpegSpec::new(W, H);
    s.exif_orientation = Some(o);
    s.build()
}

fn tiff_with(o: u16) -> Vec<u8> {
    tiff_rgb8(
        W,
        H,
        &TiffOpts {
            orientation: Some(o),
            ..TiffOpts::default()
        },
    )
}

#[test]
fn eight_orientations_across_four_formats_match_one_reference() {
    // The stored (pre-turn) pixels each format decodes to when the tag is absent: exact for the
    // lossless formats, the decoder's own output for JPEG.
    let stored_jpeg = decode(&{
        let mut s = JpegSpec::new(W, H);
        s.exif_orientation = None;
        s.build()
    })
    .unwrap()
    .raster
    .data;
    let stored_exact = pattern(W, H);
    for o in 1..=8u8 {
        let cases: [(&str, Vec<u8>, &Vec<u8>); 4] = [
            ("jpeg", jpeg_with(u16::from(o)), &stored_jpeg),
            ("tiff", tiff_with(u16::from(o)), &stored_exact),
            ("webp", webp_with_exif(W, H, u16::from(o)), &stored_exact),
            ("png", png_with_exif(W, H, u16::from(o)), &stored_exact),
        ];
        for (name, bytes, stored) in cases {
            let d = decode(&bytes).unwrap_or_else(|e| panic!("{name} o={o}: {e}"));
            let (want, ww, wh) = orient_reference(stored, W, H, o);
            assert_eq!(d.exif_orientation, o, "{name} tag kept");
            assert_eq!(
                (d.raster.width, d.raster.height),
                (ww, wh),
                "{name} o={o} size"
            );
            assert!(
                d.raster.data == want,
                "{name} o={o}: pixels differ from the reference"
            );
        }
    }
}

#[test]
fn big_endian_exif_and_absent_or_invalid_tags_are_handled() {
    // Orientation 6 in a big-endian EXIF block of a JPEG.
    let base = jpeg_baseline(W, H);
    let with = jpeg_with_exif_blob(&base, &exif_blob(6, false));
    let with_le = jpeg_with_exif_blob(&base, &exif_blob(6, true));
    assert_eq!(decode(&with).unwrap().exif_orientation, 6);
    assert_eq!(decode(&with_le).unwrap().exif_orientation, 6);
    // Out-of-range values behave as "no orientation" instead of failing the file.
    for bad in [0u16, 9, 255] {
        let d = decode(&jpeg_with_exif_blob(&base, &exif_blob(bad, true))).unwrap();
        assert_eq!(d.exif_orientation, 1);
        assert_eq!((d.raster.width, d.raster.height), (W, H));
    }
    assert_eq!(decode(&base).unwrap().exif_orientation, 1);
}

#[test]
fn the_turn_is_applied_once_not_twice() {
    // Orientation 6 on a W x H image gives H x W; a second application would give W x H again.
    let d = decode(&png_with_exif(W, H, 6)).unwrap();
    assert_eq!((d.raster.width, d.raster.height), (H, W));
}

#[test]
fn orientation_and_icc_survive_together() {
    let icc = fake_icc(150_000);
    let mut s = JpegSpec::new(W, H);
    s.exif_orientation = Some(8);
    s.icc = Some(icc.clone());
    let d = decode(&s.build()).unwrap();
    assert_eq!(d.exif_orientation, 8);
    assert_eq!(d.icc.as_deref(), Some(&icc[..]));
    assert_eq!((d.raster.width, d.raster.height), (H, W));
}
