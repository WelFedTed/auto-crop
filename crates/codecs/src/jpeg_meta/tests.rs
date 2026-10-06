// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

use super::*;
use crate::fixtures::{jpeg_baseline, jpeg_insert_segment};
use crate::{decode, exif_orientation, probe};

const THUMB: &[u8] = b"THUMBNAIL-OF-THE-WHOLE-UNCROPPED-SCAN";
const GPS_MARK: [u8; 8] = [0xAB, 0xCD, 0xAB, 0xCD, 0xAB, 0xCD, 0xAB, 0xCD];
const MAKER: &[u8] = b"MAKERNOTE-BLOB-WITH-OFFSETS-THAT-MUST-NOT-MOVE";

struct W {
    v: Vec<u8>,
    le: bool,
}

impl W {
    fn new(le: bool) -> Self {
        Self { v: Vec::new(), le }
    }
    fn p16(&mut self, x: u16) {
        self.v.extend_from_slice(&if self.le {
            x.to_le_bytes()
        } else {
            x.to_be_bytes()
        });
    }
    fn p32(&mut self, x: u32) {
        self.v.extend_from_slice(&if self.le {
            x.to_le_bytes()
        } else {
            x.to_be_bytes()
        });
    }
    fn entry(&mut self, tag: u16, ty: u16, count: u32, value: u32) {
        self.p16(tag);
        self.p16(ty);
        self.p32(count);
        // A SHORT sits in the first two bytes of the value field.
        if ty == 3 && count == 1 {
            self.p16(value as u16);
            self.p16(0);
        } else {
            self.p32(value);
        }
    }
    fn at(&self) -> u32 {
        self.v.len() as u32
    }
}

/// A TIFF blob like a camera writes: IFD0 (Make, Orientation 6, ExifIFD, GPS, next = IFD1), the
/// Exif sub-IFD (PixelXDimension, PixelYDimension, MakerNote), the GPS IFD (latitude as three
/// rationals), IFD1 (a JPEG thumbnail) and the thumbnail at the tail.
fn camera_blob(le: bool) -> Vec<u8> {
    let mut w = W::new(le);
    w.v.extend_from_slice(if le { b"II" } else { b"MM" });
    w.p16(42);
    w.p32(8);
    // IFD0: 4 entries at 8; table is 2 + 4*12 + 4 = 54 bytes; values from 62.
    let make_off = 62u32;
    let make = b"Acme Camera\0";
    let exif_ifd = make_off + make.len() as u32; // 74
    // Exif IFD: 3 entries = 2 + 36 + 4 = 42 bytes, then the maker note.
    let maker_off = exif_ifd + 42; // 116
    let gps_ifd = maker_off + MAKER.len() as u32;
    // GPS IFD: 2 entries = 2 + 24 + 4 = 30, then 24 bytes of rationals.
    let gps_vals = gps_ifd + 30;
    let ifd1 = gps_vals + 24;
    // IFD1: 3 entries = 2 + 36 + 4 = 42 bytes, then the thumbnail.
    let thumb_off = ifd1 + 42;

    w.p16(4);
    w.entry(0x010F, 2, make.len() as u32, make_off);
    w.entry(0x0112, 3, 1, 6);
    w.entry(0x8769, 4, 1, exif_ifd);
    w.entry(0x8825, 4, 1, gps_ifd);
    w.p32(ifd1);
    assert_eq!(w.at(), make_off);
    w.v.extend_from_slice(make);
    assert_eq!(w.at(), exif_ifd);
    w.p16(3);
    w.entry(0xA002, 4, 1, 4000);
    w.entry(0xA003, 4, 1, 3000);
    w.entry(0x927C, 7, MAKER.len() as u32, maker_off);
    w.p32(0);
    assert_eq!(w.at(), maker_off);
    w.v.extend_from_slice(MAKER);
    assert_eq!(w.at(), gps_ifd);
    w.p16(2);
    // GPSLatitudeRef: the ASCII "N\0" sits inline in the value field.
    w.p16(0x0001);
    w.p16(2);
    w.p32(2);
    w.v.extend_from_slice(b"N\0\0\0");
    w.entry(0x0002, 5, 3, gps_vals);
    w.p32(0);
    assert_eq!(w.at(), gps_vals);
    w.v.extend_from_slice(&GPS_MARK);
    w.v.extend_from_slice(&GPS_MARK);
    w.v.extend_from_slice(&GPS_MARK);
    assert_eq!(w.at(), ifd1);
    w.p16(3);
    w.entry(0x0103, 3, 1, 6);
    w.entry(0x0201, 4, 1, thumb_off);
    w.entry(0x0202, 4, 1, THUMB.len() as u32);
    w.p32(0);
    assert_eq!(w.at(), thumb_off);
    w.v.extend_from_slice(THUMB);
    w.v
}

fn has(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fn patch(blob: &[u8], strip: bool) -> Vec<u8> {
    exif_patch(
        blob,
        &ExifPatch {
            dims: (1200, 900),
            strip_location: strip,
        },
    )
    .expect("patched")
}

#[test]
fn the_orientation_is_reset_the_dimensions_updated_and_the_thumbnail_gone() {
    for le in [true, false] {
        let blob = camera_blob(le);
        assert_eq!(exif_orientation(&blob), Some(6));
        let out = patch(&blob, false);
        assert_eq!(exif_orientation(&out), Some(1), "le={le}");
        // The thumbnail's bytes, the only picture of the uncropped scan, are gone.
        assert!(has(&blob, THUMB) && !has(&out, THUMB), "le={le}");
        // The tail that held it is cut.
        assert!(out.len() < blob.len(), "le={le}");
        // GPS stays when it is not asked to go.
        assert!(has(&out, &GPS_MARK), "le={le}");
        // The maker note and the make string did not move.
        let pos = |b: &[u8], n: &[u8]| b.windows(n.len()).position(|w| w == n).unwrap();
        assert_eq!(pos(&out, MAKER), pos(&blob, MAKER), "le={le}");
        assert_eq!(pos(&out, b"Acme Camera"), pos(&blob, b"Acme Camera"));
        // The dimensions: PixelXDimension and PixelYDimension now say 1200 x 900.
        let (x, y) = if le {
            (1200u32.to_le_bytes(), 900u32.to_le_bytes())
        } else {
            (1200u32.to_be_bytes(), 900u32.to_be_bytes())
        };
        assert!(has(&out, &x) && has(&out, &y), "le={le}");
        assert!(!has(
            &out,
            &if le {
                4000u32.to_le_bytes()
            } else {
                4000u32.to_be_bytes()
            }
        ));
    }
}

#[test]
fn strip_location_leaves_no_gps_byte_and_no_gps_entry() {
    for le in [true, false] {
        let blob = camera_blob(le);
        let out = patch(&blob, true);
        assert!(!has(&out, &GPS_MARK), "le={le}: GPS values remain");
        // IFD0 no longer has the GPS pointer (tag 0x8825) and still says 3 entries.
        let tag = if le {
            0x8825u16.to_le_bytes()
        } else {
            0x8825u16.to_be_bytes()
        };
        let ifd0_end = 8 + 2 + 4 * 12;
        assert!(!has(&out[..ifd0_end], &tag), "le={le}");
        let count = if le {
            u16::from_le_bytes([out[8], out[9]])
        } else {
            u16::from_be_bytes([out[8], out[9]])
        };
        assert_eq!(count, 3);
        assert_eq!(exif_orientation(&out), Some(1));
        // Everything else is as before: the maker note, the make string.
        assert!(has(&out, MAKER) && has(&out, b"Acme Camera"));
    }
}

#[test]
fn a_blob_that_is_not_tiff_is_refused_and_nothing_panics() {
    assert!(
        exif_patch(
            b"",
            &ExifPatch {
                dims: (1, 1),
                strip_location: true
            }
        )
        .is_none()
    );
    assert!(
        exif_patch(
            b"XX*\0",
            &ExifPatch {
                dims: (1, 1),
                strip_location: true
            }
        )
        .is_none()
    );
    let blob = camera_blob(true);
    // Every truncation and every single-byte corruption: no panic, whatever comes back.
    for cut in 0..blob.len() {
        let _ = patch_or_none(&blob[..cut]);
    }
    for i in 0..blob.len() {
        for v in [0x00, 0xFF, 0x7F] {
            let mut b = blob.clone();
            b[i] = v;
            let _ = patch_or_none(&b);
        }
    }
}

fn patch_or_none(b: &[u8]) -> Option<Vec<u8>> {
    exif_patch(
        b,
        &ExifPatch {
            dims: (10, 10),
            strip_location: true,
        },
    )
}

#[test]
fn a_cyclic_ifd_chain_cannot_loop() {
    let mut b = camera_blob(true);
    // IFD0's next pointer points back at IFD0.
    let next0 = 8 + 2 + 4 * 12;
    b[next0..next0 + 4].copy_from_slice(&8u32.to_le_bytes());
    let out = exif_patch(
        &b,
        &ExifPatch {
            dims: (1, 1),
            strip_location: false,
        },
    );
    assert!(out.is_some());
}

fn dqt(table: &[u16; 64]) -> Vec<u8> {
    let mut seg = vec![0u8]; // 8-bit precision, id 0
    for &z in ZIGZAG.iter() {
        seg.push(table[z] as u8);
    }
    seg
}

#[test]
fn the_quality_estimate_recovers_every_ijg_quality() {
    for q in 1..=100u32 {
        let t = ijg_table(q);
        let e = estimate_quality(&t).unwrap();
        assert!(e.ijg, "q{q}");
        assert!(
            (i32::from(e.q) - q as i32).abs() <= 3,
            "q{q} estimated as {}",
            e.q
        );
    }
}

#[test]
fn custom_tables_are_not_ijg() {
    let flat = [16u16; 64];
    let e = estimate_quality(&flat).unwrap();
    assert!(!e.ijg);
    let mut odd = ijg_table(80);
    odd[5] = 90;
    odd[20] = 3;
    odd[40] = 120;
    assert!(!estimate_quality(&odd).unwrap().ijg);
    assert!(estimate_quality(&[0u16; 64]).is_none());
}

#[test]
fn the_quality_of_a_real_jpeg_is_read_from_its_tables() {
    let r = auto_crop_imgproc::Raster::filled(64, 48, [10, 20, 30]);
    for q in [60u8, 85, 92] {
        let bytes = crate::encode(&r, crate::Format::Jpeg, q, None).unwrap();
        let m = read_meta(&bytes).unwrap();
        let e = m.quality.expect("a quality");
        assert!(e.ijg, "q{q}");
        assert!(
            (i32::from(e.q) - i32::from(q)).abs() <= 3,
            "q{q} read as {}",
            e.q
        );
    }
    // A table written by hand is read back through the DQT parser.
    let mut jpeg = jpeg_baseline(32, 32);
    let t = ijg_table(50);
    jpeg = jpeg_insert_segment(&jpeg, 0xDB, &dqt(&t));
    let segs = header_segments(&jpeg).unwrap();
    assert_eq!(luma_table(&jpeg, &segs), Some(t));
}

fn camera_jpeg(with_extra: bool) -> Vec<u8> {
    let mut j = jpeg_baseline(64, 48);
    let mut exif = b"Exif\0\0".to_vec();
    exif.extend_from_slice(&camera_blob(true));
    j = jpeg_insert_segment(&j, 0xE1, &exif);
    if with_extra {
        let mut xmp = XMP_TAG.to_vec();
        xmp.extend_from_slice(
            b"<x:xmpmeta><rdf:Description tiff:Orientation=\"6\" exif:GPSLatitude=\"51,30.5N\"/></x:xmpmeta>",
        );
        j = jpeg_insert_segment(&j, 0xE1, &xmp);
        let mut ps = PHOTOSHOP_TAG.to_vec();
        ps.extend_from_slice(b"8BIM\x04\x04IPTC-CITY-LONDON");
        j = jpeg_insert_segment(&j, 0xED, &ps);
        let mut mpf = MPF_TAG.to_vec();
        mpf.extend_from_slice(&[0u8; 32]);
        j = jpeg_insert_segment(&j, 0xE2, &mpf);
        j = jpeg_insert_segment(&j, 0xFE, b"taken at 51.5N 0.1W");
        // A secondary image after the end-of-image marker (MPF, a gain map).
        j.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0xDB, 0x00, 0x04, 1, 2, 0xFF, 0xD9]);
    }
    j
}

fn policy(strip_location: bool) -> MetaPolicy {
    MetaPolicy {
        dims: (64, 48),
        strip_location,
        strip_all: false,
    }
}

#[test]
fn read_meta_finds_the_pieces() {
    let j = camera_jpeg(true);
    let m = read_meta(&j).unwrap();
    assert!(m.exif.is_some() && m.xmp.is_some() && m.iptc.is_some());
    assert_eq!(m.comments.len(), 1);
    assert_eq!(exif_orientation(m.exif.as_ref().unwrap()), Some(6));
}

#[test]
fn rewriting_keeps_the_scan_byte_exact_and_resets_the_orientation_once() {
    let src = camera_jpeg(true);
    let meta = read_meta(&src).unwrap();
    // The "output": a freshly encoded JPEG with no metadata (as the encoders write).
    let encoded = jpeg_baseline(64, 48);
    let out = rewrite_metadata(&encoded, &meta, &policy(false)).unwrap();
    // It decodes, upright, with the same pixels as the encoded one.
    let (a, b) = (decode(&encoded).unwrap(), decode(&out).unwrap());
    assert_eq!(a.raster.data, b.raster.data);
    assert_eq!(probe(&out).unwrap().orientation, 1);
    // The EXIF is carried, patched; the thumbnail is gone; GPS stays (not asked to go).
    let m = read_meta(&out).unwrap();
    let exif = m.exif.expect("exif carried");
    assert_eq!(exif_orientation(&exif), Some(1));
    assert!(!has(&exif, THUMB) && has(&exif, &GPS_MARK));
    // XMP carried with the orientation reset; IPTC and the comment carried.
    let xmp = m.xmp.expect("xmp carried");
    assert!(has(&xmp, b"tiff:Orientation=\"1\""));
    assert!(m.iptc.is_some() && m.comments.len() == 1);
    // No MPF, nothing after the end-of-image marker.
    assert!(!has(&out, MPF_TAG));
    assert_eq!(&out[out.len() - 2..], &[0xFF, 0xD9]);
    // The scan data is the encoder's, byte for byte: everything from SOS on.
    let sos = |j: &[u8]| j.windows(2).position(|w| w == [0xFF, 0xDA]).unwrap();
    assert_eq!(&out[sos(&out)..], &encoded[sos(&encoded)..]);
}

#[test]
fn strip_location_removes_exif_gps_xmp_iptc_and_comments_completely() {
    let src = camera_jpeg(true);
    let meta = read_meta(&src).unwrap();
    let out = rewrite_metadata(&jpeg_baseline(64, 48), &meta, &policy(true)).unwrap();
    assert!(!has(&out, &GPS_MARK));
    assert!(!has(&out, b"GPSLatitude") && !has(&out, b"IPTC-CITY") && !has(&out, b"51.5N"));
    let m = read_meta(&out).unwrap();
    assert!(m.xmp.is_none() && m.iptc.is_none() && m.comments.is_empty());
    // The rest of EXIF (make, maker note) is still there, with orientation 1.
    let exif = m.exif.unwrap();
    assert!(has(&exif, b"Acme Camera"));
    assert_eq!(exif_orientation(&exif), Some(1));
}

#[test]
fn strip_all_carries_nothing() {
    let src = camera_jpeg(true);
    let meta = read_meta(&src).unwrap();
    let p = MetaPolicy {
        strip_all: true,
        ..policy(false)
    };
    let out = rewrite_metadata(&jpeg_baseline(64, 48), &meta, &p).unwrap();
    let m = read_meta(&out).unwrap();
    assert!(m.exif.is_none() && m.xmp.is_none() && m.iptc.is_none() && m.comments.is_empty());
    assert!(decode(&out).is_ok());
}

#[test]
fn an_unpatchable_exif_is_dropped_not_copied_with_a_stale_orientation() {
    let meta = JpegMeta {
        exif: Some(b"garbage that is not tiff".to_vec()),
        ..JpegMeta::default()
    };
    let out = rewrite_metadata(&jpeg_baseline(16, 16), &meta, &policy(false)).unwrap();
    assert!(read_meta(&out).unwrap().exif.is_none());
}

#[test]
fn the_xmp_orientation_reset_handles_both_forms_and_leaves_the_rest() {
    let x = b"<a tiff:Orientation=\"8\" b=\"8\"/><tiff:Orientation>3</tiff:Orientation>";
    let out = xmp_reset_orientation(x);
    assert_eq!(
        out,
        b"<a tiff:Orientation=\"1\" b=\"8\"/><tiff:Orientation>1</tiff:Orientation>".to_vec()
    );
    assert_eq!(
        xmp_reset_orientation(b"nothing here"),
        b"nothing here".to_vec()
    );
    assert_eq!(
        xmp_reset_orientation(b"tiff:Orientation"),
        b"tiff:Orientation".to_vec()
    );
}

#[test]
fn the_header_walk_never_panics_on_damage() {
    let j = camera_jpeg(true);
    for cut in (0..j.len()).step_by(3) {
        let _ = read_meta(&j[..cut]);
        let _ = rewrite_metadata(&j[..cut], &JpegMeta::default(), &policy(false));
    }
    for i in (0..200.min(j.len())).step_by(2) {
        let mut b = j.clone();
        b[i] ^= 0xFF;
        let _ = read_meta(&b);
        let _ = rewrite_metadata(&b, &JpegMeta::default(), &policy(false));
    }
}

#[test]
fn a_multi_scan_jpeg_keeps_every_scan_and_loses_the_trailer() {
    // Two scans (the second is a copy): both stay, only the junk after EOI goes.
    let j = crate::fixtures::jpeg_repeat_scan(&jpeg_baseline(32, 32), 1);
    let mut with_trailer = j.clone();
    with_trailer.extend_from_slice(b"TRAILING-GARBAGE");
    let out = rewrite_metadata(&with_trailer, &JpegMeta::default(), &policy(false)).unwrap();
    assert_eq!(out, j);
}
