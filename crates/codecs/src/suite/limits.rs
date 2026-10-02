// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! ROADMAP M1.13: one test per `DecodeLimits` field, plus the "60000 x 60000 header" rule.

use crate::fixtures::*;
use crate::{CodecError, DecodeLimits, Limit, decode, decode_with, probe_with};
use std::time::{Duration, Instant};

fn limit_of(r: Result<crate::Decoded, CodecError>) -> Limit {
    match r {
        Err(CodecError::LimitExceeded { limit, .. }) => limit,
        Err(CodecError::TooLarge(_)) => Limit::Pixels,
        other => panic!("expected a limit error, got {other:?}"),
    }
}

/// A PNG header declaring `w x h` gray pixels with a tiny, valid-looking IDAT.
fn png_claiming(w: u32, h: u32) -> Vec<u8> {
    let mut b = png_gray(8, 8);
    b[16..20].copy_from_slice(&w.to_be_bytes());
    b[20..24].copy_from_slice(&h.to_be_bytes());
    // The IHDR CRC is not checked by the sniffer or the probe; keep it consistent anyway.
    let crc = crc32(&b[12..29]);
    b[29..33].copy_from_slice(&crc.to_be_bytes());
    b
}

#[test]
fn max_pixels_rejects_before_any_allocation() {
    let l = DecodeLimits::default();
    // 100 MP + 1 pixel is refused, naming the declared count.
    match decode_with(&png_claiming(10_000, 10_001), &l) {
        Err(CodecError::TooLarge(n)) => assert_eq!(n, 100_010_000),
        other => panic!("{other:?}"),
    }
    // Raising the cap through `with_max_pixels` moves the line (and the ceiling clamps it).
    let raised = DecodeLimits::default().with_max_pixels(200_000_000);
    assert_eq!(raised.max_pixels, 200_000_000);
    assert!(!matches!(
        decode_with(&png_claiming(10_000, 10_001), &raised),
        Err(CodecError::TooLarge(_))
    ));
}

#[test]
fn a_60000_by_60000_header_fails_fast_in_every_format_that_can_declare_it() {
    let png = png_claiming(60_000, 60_000);
    let tiff = tiff_classic(
        true,
        &[
            (256, 4, 1, 60_000),
            (257, 4, 1, 60_000),
            (258, 3, 1, 8),
            (259, 3, 1, 1),
            (262, 3, 1, 1),
            (273, 4, 1, 0),
            (277, 3, 1, 1),
            (278, 4, 1, 60_000),
            (279, 4, 1, 16),
        ],
        0,
        &[0; 16],
        Some(5),
    );
    let jpeg = jpeg_patch_sof(&jpeg_baseline(16, 16), None, Some((60_000, 60_000)), None);
    for (name, bytes) in [("png", png), ("tiff", tiff), ("jpeg", jpeg)] {
        let t = Instant::now();
        let r = decode(&bytes);
        assert!(
            matches!(r, Err(CodecError::TooLarge(3_600_000_000))),
            "{name}: {r:?}"
        );
        assert!(
            t.elapsed() < Duration::from_secs(1),
            "{name} took {:?}",
            t.elapsed()
        );
    }
}

#[test]
fn max_file_bytes_is_checked_before_parsing() {
    let jpeg = jpeg_baseline(16, 16);
    let l = DecodeLimits {
        max_file_bytes: 100,
        ..DecodeLimits::default()
    };
    assert!(jpeg.len() > 100);
    assert_eq!(limit_of(decode_with(&jpeg, &l)), Limit::FileBytes);
    assert!(matches!(
        probe_with(&jpeg, &l),
        Err(CodecError::LimitExceeded {
            limit: Limit::FileBytes,
            ..
        })
    ));
    // PNG and TIFF use the streamed-file cap instead.
    let png = png_rgb(16, 16);
    assert!(decode_with(&png, &l).is_ok());
    let l2 = DecodeLimits {
        max_file_bytes_streamed: 100,
        ..DecodeLimits::default()
    };
    assert_eq!(limit_of(decode_with(&png, &l2)), Limit::FileBytes);
    assert!(decode_with(&jpeg, &l2).is_ok());
}

#[test]
fn max_metadata_bytes_caps_exif_xmp_and_icc_blobs() {
    let l = DecodeLimits {
        max_metadata_bytes: 1000,
        ..DecodeLimits::default()
    };
    // ICC in a JPEG (sum of segments), PNG iCCP, TIFF tag and WebP ICCP.
    let mut s = JpegSpec::new(16, 16);
    s.icc = Some(fake_icc(5000));
    let big = [
        s.build(),
        png_with_icc(16, 16, &fake_icc(5000)),
        tiff_rgb8(
            16,
            16,
            &TiffOpts {
                icc: Some(fake_icc(5000)),
                ..TiffOpts::default()
            },
        ),
        webp_extended(16, 16, None, Some(&fake_icc(5000))),
        // EXIF blobs: WebP EXIF chunk and PNG eXIf.
        webp_extended(16, 16, Some(&vec![0u8; 5000]), None),
    ];
    for (i, b) in big.iter().enumerate() {
        assert_eq!(
            limit_of(decode_with(b, &l)),
            Limit::MetadataBytes,
            "case {i}"
        );
    }
    // Under the cap they decode.
    let mut s = JpegSpec::new(16, 16);
    s.icc = Some(fake_icc(500));
    assert!(decode_with(&s.build(), &l).is_ok());
    // A PNG declaring a 2 GB iCCP chunk is refused on the declared size alone.
    let mut png = png_with_icc(16, 16, &fake_icc(100));
    let at = png.windows(4).position(|w| w == b"iCCP").unwrap() - 4;
    png[at..at + 4].copy_from_slice(&2_000_000_000u32.to_be_bytes());
    assert_eq!(
        limit_of(decode_with(&png, &DecodeLimits::default())),
        Limit::MetadataBytes
    );
}

#[test]
fn max_scans_caps_progressive_jpegs() {
    let mut s = JpegSpec::new(32, 32);
    s.progressive = true;
    let prog = s.build();
    let scans = probe_with(&prog, &DecodeLimits::default()).unwrap().scans;
    assert!(scans >= 4, "fixture has {scans} scans");
    let l = DecodeLimits {
        max_scans: scans - 1,
        ..DecodeLimits::default()
    };
    assert_eq!(limit_of(decode_with(&prog, &l)), Limit::Scans);
    let l = DecodeLimits {
        max_scans: scans,
        ..DecodeLimits::default()
    };
    assert!(decode_with(&prog, &l).is_ok());
    // A scan bomb under the default cap of 100.
    let bomb = jpeg_repeat_scan(&jpeg_baseline(16, 16), 150);
    assert_eq!(limit_of(decode(&bomb)), Limit::Scans);
}

#[test]
fn max_frames_caps_pages_frames_and_directories() {
    let l = DecodeLimits {
        max_frames: 2,
        ..DecodeLimits::default()
    };
    let o = TiffOpts {
        extra_pages: 2,
        ..TiffOpts::default()
    };
    assert_eq!(
        limit_of(decode_with(&tiff_rgb8(8, 8, &o), &l)),
        Limit::Frames
    );
    assert_eq!(
        limit_of(decode_with(&webp_animated(8, 8, 3), &l)),
        Limit::Frames
    );
    assert!(decode_with(&webp_animated(8, 8, 2), &l).is_ok());
    // APNG declaring four billion frames.
    let mut apng = png_apng(8, 8);
    let at = apng.windows(4).position(|w| w == b"acTL").unwrap() + 4;
    apng[at..at + 4].copy_from_slice(&4_000_000_000u32.to_be_bytes());
    assert_eq!(
        limit_of(decode_with(&apng, &DecodeLimits::default())),
        Limit::Frames
    );
}

#[test]
fn max_est_bytes_caps_the_decode_memory_estimate() {
    let png = png_rgb(100, 100);
    // 10,000 px x 9 + 64 MiB: a cap just below it refuses, just at it admits.
    let est = crate::est_bytes(100, 100);
    let l = DecodeLimits {
        max_est_bytes: est - 1,
        ..DecodeLimits::default()
    };
    assert_eq!(limit_of(decode_with(&png, &l)), Limit::EstBytes);
    let l = DecodeLimits {
        max_est_bytes: est,
        ..DecodeLimits::default()
    };
    assert!(decode_with(&png, &l).is_ok());
}

#[test]
fn max_decode_ms_returns_a_timeout_through_decode_guarded() {
    // The soft timeout itself is exercised with a slow closure in `guard`; here the field is wired
    // through `decode_guarded` and a generous value does not trigger.
    let l = DecodeLimits {
        max_decode_ms: Some(30_000),
        ..DecodeLimits::default()
    };
    let png: std::sync::Arc<[u8]> = png_rgb(16, 16).into();
    assert!(crate::decode_guarded(png, &l).is_ok());
}

#[test]
fn image_dimensions_larger_than_the_probe_claim_are_not_trusted() {
    // The decoder re-checks its own idea of the size: a PNG whose IHDR says 100 x 100 but whose
    // data is that of a 16 x 16 image fails as corrupt, not as a mis-sized buffer.
    let mut b = png_rgb(16, 16);
    b[16..20].copy_from_slice(&100u32.to_be_bytes());
    b[20..24].copy_from_slice(&100u32.to_be_bytes());
    let crc = crc32(&b[12..29]);
    b[29..33].copy_from_slice(&crc.to_be_bytes());
    assert!(matches!(decode(&b), Err(CodecError::Corrupt(_))));
}

#[test]
fn a_declared_size_far_beyond_the_data_is_corrupt_without_allocating_it() {
    // At the 100 MP cap, so the pixel limit does not apply: it is the plausibility checks that
    // refuse these tiny files before the decoder reserves its buffers.
    let png = png_claiming(10_000, 10_000);
    assert!(matches!(decode(&png), Err(CodecError::Corrupt(_))));
    let jpeg = jpeg_patch_sof(&jpeg_baseline(16, 16), None, Some((10_000, 10_000)), None);
    assert!(matches!(decode(&jpeg), Err(CodecError::Corrupt(_))));
    let tiff = tiff_classic(
        true,
        &[
            (256, 4, 1, 10_000),
            (257, 4, 1, 10_000),
            (258, 3, 1, 8),
            (259, 3, 1, 1),
            (262, 3, 1, 1),
            (273, 4, 1, 0),
            (277, 3, 1, 1),
            (278, 4, 1, 10_000),
            (279, 4, 1, 64),
        ],
        0,
        &[0; 64],
        Some(5),
    );
    assert!(matches!(decode(&tiff), Err(CodecError::Corrupt(_))));
}

#[test]
fn a_maximally_compressed_png_still_passes_the_plausibility_check() {
    // 3000 x 3000 gray zeros at the best deflate level is about 1030:1, right at the 1100:1 line.
    let (w, h) = (3000u32, 3000u32);
    let raw = vec![0u8; ((w + 1) * h) as usize];
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    std::io::Write::write_all(&mut enc, &raw).unwrap();
    let idat = enc.finish().unwrap();
    let mut png = crate::fixtures::PNG_SIG.to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 0, 0, 0, 0]);
    png.extend(png_chunk(b"IHDR", &ihdr));
    png.extend(png_chunk(b"IDAT", &idat));
    png.extend(png_chunk(b"IEND", &[]));
    eprintln!("ratio {}:1", raw.len() / png.len());
    let d = decode(&png).unwrap();
    assert_eq!((d.raster.width, d.raster.height), (w, h));
}
