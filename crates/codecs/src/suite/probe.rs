// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! ROADMAP M1.12: format sniffing and the header probe over a table of generated fixtures.

use crate::fixtures::*;
use crate::{CodecError, Format, probe, sniff};

struct Case {
    name: &'static str,
    bytes: Vec<u8>,
    format: Format,
    size: (u32, u32),
    depth: u8,
    channels: u8,
    frames: u32,
    orientation: u8,
    icc: bool,
}

fn case(name: &'static str, bytes: Vec<u8>, format: Format, size: (u32, u32)) -> Case {
    Case {
        name,
        bytes,
        format,
        size,
        depth: 8,
        channels: 3,
        frames: 1,
        orientation: 1,
        icc: false,
    }
}

fn cases() -> Vec<Case> {
    let t = TiffOpts::default();
    let tc = |c| TiffOpts {
        comp: Some(c),
        ..TiffOpts::default()
    };
    let mut v = Vec::new();
    // JPEG
    v.push(case(
        "jpeg 4:4:4",
        jpeg_baseline(33, 21),
        Format::Jpeg,
        (33, 21),
    ));
    let mut s = JpegSpec::new(40, 24);
    s.sampling = (2, 2);
    v.push(case("jpeg 4:2:0", s.build(), Format::Jpeg, (40, 24)));
    let mut s = JpegSpec::new(40, 24);
    s.gray = true;
    let mut c = case("jpeg gray", s.build(), Format::Jpeg, (40, 24));
    c.channels = 1;
    v.push(c);
    let mut s = JpegSpec::new(40, 24);
    s.progressive = true;
    v.push(case("jpeg progressive", s.build(), Format::Jpeg, (40, 24)));
    let mut s = JpegSpec::new(40, 24);
    s.restart = Some(2);
    v.push(case(
        "jpeg restart interval",
        s.build(),
        Format::Jpeg,
        (40, 24),
    ));
    let mut s = JpegSpec::new(40, 24);
    s.optimize = true;
    v.push(case(
        "jpeg optimised huffman",
        s.build(),
        Format::Jpeg,
        (40, 24),
    ));
    let mut c = case(
        "jpeg cmyk",
        jpeg_cmyk(24, 16, false),
        Format::Jpeg,
        (24, 16),
    );
    c.channels = 4;
    v.push(c);
    let mut c = case("jpeg ycck", jpeg_cmyk(24, 16, true), Format::Jpeg, (24, 16));
    c.channels = 4;
    v.push(c);
    let mut s = JpegSpec::new(30, 20);
    s.exif_orientation = Some(6);
    let mut c = case("jpeg exif 6", s.build(), Format::Jpeg, (30, 20));
    c.orientation = 6;
    v.push(c);
    let mut s = JpegSpec::new(30, 20);
    s.icc = Some(fake_icc(150_000));
    let mut c = case("jpeg 3-segment icc", s.build(), Format::Jpeg, (30, 20));
    c.icc = true;
    v.push(c);
    let mut c = case(
        "jpeg 12-bit header",
        jpeg_patch_sof(&jpeg_baseline(16, 16), Some(12), None, Some(0xC1)),
        Format::Jpeg,
        (16, 16),
    );
    c.depth = 12;
    v.push(c);
    // PNG
    v.push(case("png rgb", png_rgb(19, 11), Format::Png, (19, 11)));
    let mut c = case("png rgba", png_rgba(19, 11), Format::Png, (19, 11));
    c.channels = 4;
    v.push(c);
    let mut c = case("png gray", png_gray(19, 11), Format::Png, (19, 11));
    c.channels = 1;
    v.push(c);
    let mut c = case("png rgb16", png_rgb16(19, 11), Format::Png, (19, 11));
    c.depth = 16;
    v.push(c);
    let mut c = case("png palette", png_palette(19, 11), Format::Png, (19, 11));
    c.channels = 1;
    v.push(c);
    v.push(case(
        "png adam7",
        png_interlaced(19, 11),
        Format::Png,
        (19, 11),
    ));
    let mut c = case("png apng 2 frames", png_apng(19, 11), Format::Png, (19, 11));
    c.frames = 2;
    v.push(c);
    let mut c = case(
        "png exif 8",
        png_with_exif(19, 11, 8),
        Format::Png,
        (19, 11),
    );
    c.orientation = 8;
    v.push(c);
    let mut c = case(
        "png iccp",
        png_with_icc(19, 11, &fake_icc(3000)),
        Format::Png,
        (19, 11),
    );
    c.icc = true;
    v.push(c);
    // TIFF
    v.push(case(
        "tiff none",
        tiff_rgb8(17, 9, &t),
        Format::Tiff,
        (17, 9),
    ));
    v.push(case(
        "tiff lzw",
        tiff_rgb8(17, 9, &tc(TiffComp::Lzw)),
        Format::Tiff,
        (17, 9),
    ));
    v.push(case(
        "tiff deflate",
        tiff_rgb8(17, 9, &tc(TiffComp::Deflate)),
        Format::Tiff,
        (17, 9),
    ));
    v.push(case(
        "tiff packbits",
        tiff_rgb8(17, 9, &tc(TiffComp::PackBits)),
        Format::Tiff,
        (17, 9),
    ));
    let mut c = case("tiff rgb16", tiff_rgb16(17, 9, &t), Format::Tiff, (17, 9));
    c.depth = 16;
    v.push(c);
    let mut c = case("tiff gray8", tiff_gray8(17, 9, &t), Format::Tiff, (17, 9));
    c.channels = 1;
    v.push(c);
    let mut c = case("tiff gray16", tiff_gray16(17, 9, &t), Format::Tiff, (17, 9));
    c.channels = 1;
    c.depth = 16;
    v.push(c);
    let o = TiffOpts {
        extra_pages: 2,
        ..TiffOpts::default()
    };
    let mut c = case("tiff 3 pages", tiff_rgb8(17, 9, &o), Format::Tiff, (17, 9));
    c.frames = 3;
    v.push(c);
    let o = TiffOpts {
        big: true,
        ..TiffOpts::default()
    };
    v.push(case("bigtiff", tiff_rgb8(17, 9, &o), Format::Tiff, (17, 9)));
    let o = TiffOpts {
        orientation: Some(3),
        ..TiffOpts::default()
    };
    let mut c = case(
        "tiff orientation 3",
        tiff_rgb8(17, 9, &o),
        Format::Tiff,
        (17, 9),
    );
    c.orientation = 3;
    v.push(c);
    let o = TiffOpts {
        icc: Some(fake_icc(2000)),
        ..TiffOpts::default()
    };
    let mut c = case("tiff icc", tiff_rgb8(17, 9, &o), Format::Tiff, (17, 9));
    c.icc = true;
    v.push(c);
    for (name, bytes) in [
        ("tiff g4 1-bit", tiff_g4(40, 20)),
        (
            "tiff raw 1-bit white-is-zero",
            tiff_bilevel_raw(40, 20, true),
        ),
        (
            "tiff raw 1-bit black-is-zero",
            tiff_bilevel_raw(40, 20, false),
        ),
    ] {
        let mut c = case(name, bytes, Format::Tiff, (40, 20));
        c.depth = 1;
        c.channels = 1;
        v.push(c);
    }
    // WebP
    v.push(case(
        "webp lossless",
        webp_lossless(21, 13),
        Format::Webp,
        (21, 13),
    ));
    let mut c = case("webp lossy 1x1", webp_lossy_1x1(), Format::Webp, (1, 1));
    c.channels = 3;
    v.push(c);
    let mut c = case(
        "webp exif 6",
        webp_with_exif(21, 13, 6),
        Format::Webp,
        (21, 13),
    );
    c.orientation = 6;
    v.push(c);
    let mut c = case(
        "webp icc",
        webp_extended(21, 13, None, Some(&fake_icc(500))),
        Format::Webp,
        (21, 13),
    );
    c.icc = true;
    v.push(c);
    let mut c = case(
        "webp animated 3",
        webp_animated(21, 13, 3),
        Format::Webp,
        (21, 13),
    );
    c.frames = 3;
    v.push(c);
    v
}

#[test]
fn at_least_thirty_fixtures_sniff_and_probe_to_the_expected_facts() {
    let all = cases();
    assert!(all.len() >= 30, "only {} fixtures", all.len());
    for c in &all {
        assert_eq!(sniff(&c.bytes), Some(c.format), "sniff {}", c.name);
        let p = probe(&c.bytes).unwrap_or_else(|e| panic!("probe {}: {e}", c.name));
        assert_eq!(p.format, c.format, "{}", c.name);
        assert_eq!((p.width, p.height), c.size, "{} size", c.name);
        assert_eq!(p.bit_depth, c.depth, "{} depth", c.name);
        assert_eq!(p.channels, c.channels, "{} channels", c.name);
        assert_eq!(p.frames, c.frames, "{} frames", c.name);
        assert_eq!(p.orientation, c.orientation, "{} orientation", c.name);
        assert_eq!(p.icc_len.is_some(), c.icc, "{} icc", c.name);
    }
}

#[test]
fn probe_reports_the_stored_icc_size_byte_exact_for_jpeg_tiff_and_webp() {
    let icc = fake_icc(150_000);
    let mut s = JpegSpec::new(30, 20);
    s.icc = Some(icc.clone());
    assert_eq!(probe(&s.build()).unwrap().icc_len, Some(150_000));
    let o = TiffOpts {
        icc: Some(fake_icc(2000)),
        ..TiffOpts::default()
    };
    assert_eq!(probe(&tiff_rgb8(8, 8, &o)).unwrap().icc_len, Some(2000));
    assert_eq!(
        probe(&webp_extended(8, 8, None, Some(&fake_icc(500))))
            .unwrap()
            .icc_len,
        Some(500)
    );
}

#[test]
fn recognised_only_formats_sniff_but_do_not_probe() {
    for (bytes, f) in [
        (heic_stub(), Format::Heic),
        (avif_stub(), Format::Avif),
        (gif_stub(), Format::Gif),
        (bmp_stub(), Format::Bmp),
        (jxl_stub(), Format::Jxl),
    ] {
        assert_eq!(sniff(&bytes), Some(f));
        assert_eq!(probe(&bytes), Err(CodecError::NotDecodable(f)));
        assert!(!f.is_decodable());
    }
}

#[test]
fn unknown_and_truncated_headers_are_typed_errors() {
    assert_eq!(
        probe(b"hello world, not an image"),
        Err(CodecError::Unsupported)
    );
    assert_eq!(probe(&[]), Err(CodecError::Unsupported));
    for full in [
        jpeg_baseline(16, 16),
        png_rgb(16, 16),
        tiff_rgb8(16, 16, &TiffOpts::default()),
        webp_lossless(16, 16),
    ] {
        for cut in 0..full.len().min(64) {
            // Never a panic; either a typed error or (for cuts past the header) a result.
            let _ = probe(&full[..cut]);
        }
    }
    assert!(matches!(
        probe(&[0xFF, 0xD8, 0xFF]),
        Err(CodecError::Corrupt(_))
    ));
    assert!(matches!(
        probe(&png_rgb(8, 8)[..20]),
        Err(CodecError::Corrupt(_))
    ));
}

#[test]
fn a_100_megapixel_declared_file_probes_without_touching_pixels() {
    // 10000 x 10000 gray PNG header: probing reads a few dozen bytes and allocates nothing
    // proportional to the image (the < 1 MB bound is measured by `cargo xtask make-hostile`,
    // which owns a counting allocator).
    let mut png = png_gray(8, 8);
    png[16..20].copy_from_slice(&10_000u32.to_be_bytes());
    png[20..24].copy_from_slice(&10_000u32.to_be_bytes());
    let t = std::time::Instant::now();
    let p = probe(&png).unwrap();
    assert_eq!((p.width, p.height), (10_000, 10_000));
    assert!(t.elapsed() < std::time::Duration::from_millis(250));
}
