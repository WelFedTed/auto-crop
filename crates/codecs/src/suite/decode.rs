// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! ROADMAP M1.15 (JPEG, PNG) and M1.16 (TIFF, WebP) decode behaviour, plus ICC and notices.

use crate::fixtures::*;
use crate::{CodecError, Format, decode};

/// Mean absolute difference per sample.
fn mad(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b)
        .map(|(x, y)| (i32::from(*x) - i32::from(*y)).unsigned_abs() as f64)
        .sum::<f64>()
        / a.len() as f64
}

fn max_diff(a: &[u8], b: &[u8]) -> i32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (i32::from(*x) - i32::from(*y)).abs())
        .max()
        .unwrap_or(0)
}

fn rgb_of_gray(g: &[u8]) -> Vec<u8> {
    g.iter().flat_map(|&v| [v, v, v]).collect()
}

// ------------------------------------------------------------------------------------- JPEG

#[test]
fn jpeg_variants_decode_close_to_the_source() {
    let (w, h) = (45, 29);
    let want = smooth(w, h);
    let mut variants: Vec<(&str, JpegSpec)> = Vec::new();
    variants.push(("4:4:4", JpegSpec::new(w, h)));
    let mut s = JpegSpec::new(w, h);
    s.sampling = (2, 2);
    variants.push(("4:2:0", s));
    let mut s = JpegSpec::new(w, h);
    s.sampling = (2, 1);
    variants.push(("4:2:2", s));
    let mut s = JpegSpec::new(w, h);
    s.progressive = true;
    variants.push(("progressive", s));
    let mut s = JpegSpec::new(w, h);
    s.restart = Some(3);
    variants.push(("restart interval", s));
    let mut s = JpegSpec::new(w, h);
    s.optimize = true;
    variants.push(("optimised huffman", s));
    for (name, spec) in variants {
        let d = decode(&spec.build()).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!((d.raster.width, d.raster.height), (w, h), "{name}");
        assert_eq!(d.format, Format::Jpeg);
        let m = mad(&d.raster.data, &want);
        assert!(m < 3.0, "{name}: mean abs diff vs source {m}");
        assert!(d.notices.is_empty(), "{name}: {:?}", d.notices);
    }
}

#[test]
fn gray_jpeg_decodes_to_equal_channels() {
    let mut s = JpegSpec::new(32, 24);
    s.gray = true;
    let d = decode(&s.build()).unwrap();
    for p in d.raster.data.as_chunks::<3>().0 {
        assert!(p[0] == p[1] && p[1] == p[2]);
    }
}

#[test]
fn cmyk_and_ycck_jpegs_give_rgb_or_a_typed_error_never_a_panic() {
    for ycck in [false, true] {
        match decode(&jpeg_cmyk(24, 16, ycck)) {
            Ok(d) => {
                assert_eq!((d.raster.width, d.raster.height), (24, 16));
                assert!(
                    d.notices.contains(&"cmyk.naive_conversion"),
                    "ycck={ycck}: conversion must be announced, got {:?}",
                    d.notices
                );
            }
            Err(CodecError::UnsupportedFeature(_) | CodecError::Corrupt(_)) => {}
            Err(other) => panic!("ycck={ycck}: unexpected {other:?}"),
        }
    }
}

#[test]
fn twelve_bit_arithmetic_and_lossless_jpegs_are_unsupported_features() {
    let base = jpeg_baseline(16, 16);
    for (name, bytes) in [
        ("12-bit", jpeg_patch_sof(&base, Some(12), None, Some(0xC1))),
        ("arithmetic", jpeg_patch_sof(&base, None, None, Some(0xC9))),
        ("lossless", jpeg_patch_sof(&base, None, None, Some(0xC3))),
    ] {
        match decode(&bytes) {
            Err(CodecError::UnsupportedFeature(why)) => {
                assert!(!why.is_empty(), "{name}");
            }
            other => panic!("{name}: {other:?}"),
        }
    }
}

/// Found by the `limits` fuzz target (M1.70, `fuzz/regressions/limits/`): zune-jpeg 0.5.15 panics
/// in its AVX2 IDCT on a subsampled sequential JPEG with one scan per component, so it is refused up front.
#[test]
fn a_non_interleaved_sequential_jpeg_is_an_unsupported_feature_not_a_decoder_panic() {
    let base = JpegSpec {
        sampling: (2, 2),
        ..JpegSpec::new(24, 16)
    }
    .build();
    let sos = base.windows(2).position(|w| w == [0xFF, 0xDA]).unwrap();
    let header_len = 2 + usize::from(u16::from_be_bytes([base[sos + 2], base[sos + 3]]));
    // One component in the scan (id 1, tables 0/0), spectral selection 0..63, no approximation.
    let mut bytes = base[..sos].to_vec();
    bytes.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00, 0x3F, 0x00]);
    bytes.extend_from_slice(&base[sos + header_len..]);
    assert!(
        matches!(decode(&bytes), Err(CodecError::UnsupportedFeature(why)) if why.contains("non-interleaved")),
        "{:?}",
        decode(&bytes).map(|d| d.frames)
    );
    // The same walk must not refuse a normal interleaved file.
    assert!(decode(&base).is_ok());
}

#[test]
fn a_truncated_jpeg_is_refused_not_decoded_with_grey_fill() {
    let full = jpeg_baseline(64, 48);
    for frac in [3, 5, 8] {
        let cut = &full[..full.len() * frac / 10];
        assert!(
            matches!(decode(cut), Err(CodecError::Corrupt(_))),
            "cut at {frac}/10"
        );
    }
}

#[test]
fn jpeg_icc_in_three_segments_is_kept_byte_exact() {
    let icc = fake_icc(150_000); // 3 APP2 segments of at most 65,519 bytes
    let mut s = JpegSpec::new(30, 20);
    s.icc = Some(icc.clone());
    let d = decode(&s.build()).unwrap();
    assert_eq!(d.icc.as_deref(), Some(&icc[..]));
    assert!(d.notices.is_empty());
}

#[test]
fn a_jpeg_icc_with_a_missing_segment_is_dropped_with_a_notice() {
    let icc = fake_icc(150_000);
    let mut s = JpegSpec::new(30, 20);
    s.icc = Some(icc);
    let mut bytes = s.build();
    // Turn the second ICC segment's sequence number into a duplicate of the first.
    let pos = bytes
        .windows(14)
        .enumerate()
        .filter(|(_, w)| &w[..12] == b"ICC_PROFILE\0")
        .map(|(i, _)| i)
        .nth(1)
        .unwrap();
    bytes[pos + 12] = 1;
    let d = decode(&bytes).unwrap();
    assert!(d.icc.is_none());
    assert!(d.notices.contains(&"icc.invalid"));
}

// -------------------------------------------------------------------------------------- PNG

#[test]
fn png_variants_are_bit_exact() {
    let (w, h) = (23, 14);
    let want = pattern(w, h);
    let d = decode(&png_rgb(w, h)).unwrap();
    assert_eq!(d.raster.data, want);
    assert!(d.notices.is_empty());
    // Adam7 interlacing gives the same pixels as the plain file.
    assert_eq!(decode(&png_interlaced(w, h)).unwrap().raster.data, want);
    // Greyscale.
    assert_eq!(
        decode(&png_gray(w, h)).unwrap().raster.data,
        rgb_of_gray(&pattern_gray(w, h))
    );
    // Palette: index i maps to (i, 255 - i, i / 2).
    let d = decode(&png_palette(w, h)).unwrap();
    for (i, p) in d.raster.data.as_chunks::<3>().0.iter().enumerate() {
        let k = (i % 256) as u8;
        assert_eq!(*p, [k, 255 - k, k / 2], "palette pixel {i}");
    }
}

#[test]
fn png_16_bit_reduces_to_8_with_a_notice() {
    let (w, h) = (23, 14);
    let d = decode(&png_rgb16(w, h)).unwrap();
    assert!(d.notices.contains(&"depth.reduced_to_8"));
    assert_eq!(d.source_bit_depth, 16);
    // The fixture's high byte is the 8-bit pattern; rounding the low byte may move it by one.
    assert!(max_diff(&d.raster.data, &pattern(w, h)) <= 1);
}

#[test]
fn png_alpha_is_dropped_with_a_notice_only_when_it_matters() {
    let d = decode(&png_rgba(16, 10)).unwrap();
    assert!(d.notices.contains(&"alpha.dropped"));
    // Fully opaque RGBA loses nothing.
    let mut s = PngSpec::rgb8(8, 8);
    s.colour = 6;
    s.data = pattern(8, 8)
        .as_chunks::<3>()
        .0
        .iter()
        .flat_map(|p| [p[0], p[1], p[2], 255])
        .collect();
    assert!(decode(&s.build()).unwrap().notices.is_empty());
}

#[test]
fn apng_decodes_the_default_image_and_announces_it() {
    let d = decode(&png_apng(16, 10)).unwrap();
    assert_eq!(d.raster.data, pattern(16, 10));
    assert_eq!(d.frames, 2);
    assert!(d.notices.contains(&"anim.first_frame_only"));
}

#[test]
fn png_icc_is_kept_byte_exact() {
    let icc = fake_icc(3000);
    let d = decode(&png_with_icc(16, 10, &icc)).unwrap();
    assert_eq!(d.icc.as_deref(), Some(&icc[..]));
}

#[test]
fn corrupt_pngs_fail_cleanly() {
    let good = png_rgb(32, 32);
    assert!(decode(&good[..good.len() / 2]).is_err());
    let mut bad = good.clone();
    let n = bad.len();
    bad[n / 2] ^= 0xFF; // flips IDAT bytes: CRC and inflate errors
    assert!(decode(&bad).is_err());
    let mut bad_colour = good;
    bad_colour[25] = 9; // invalid colour type
    assert!(matches!(decode(&bad_colour), Err(CodecError::Corrupt(_))));
}

// ------------------------------------------------------------------------------------ TIFF

#[test]
fn tiff_compressions_decode_exactly() {
    let (w, h) = (31, 17);
    let want = pattern(w, h);
    for c in [
        TiffComp::None,
        TiffComp::Lzw,
        TiffComp::Deflate,
        TiffComp::PackBits,
    ] {
        let o = TiffOpts {
            comp: Some(c),
            ..TiffOpts::default()
        };
        let d = decode(&tiff_rgb8(w, h, &o)).unwrap_or_else(|e| panic!("{c:?}: {e}"));
        assert_eq!(d.raster.data, want, "{c:?}");
        assert_eq!(d.format, Format::Tiff);
    }
}

#[test]
fn tiff_16_bit_and_gray_decode_with_the_right_values() {
    let (w, h) = (31, 17);
    let o = TiffOpts::default();
    let d = decode(&tiff_rgb16(w, h, &o)).unwrap();
    assert!(d.notices.contains(&"depth.reduced_to_8"));
    assert_eq!(d.source_bit_depth, 16);
    assert!(max_diff(&d.raster.data, &pattern(w, h)) <= 1);
    let d = decode(&tiff_gray8(w, h, &o)).unwrap();
    assert_eq!(d.raster.data, rgb_of_gray(&pattern_gray(w, h)));
    let d = decode(&tiff_gray16(w, h, &o)).unwrap();
    assert!(max_diff(&d.raster.data, &rgb_of_gray(&pattern_gray(w, h))) <= 1);
}

#[test]
fn bigtiff_and_multi_page_tiff_decode_the_first_page_with_a_notice() {
    let (w, h) = (20, 12);
    let o = TiffOpts {
        big: true,
        ..TiffOpts::default()
    };
    assert_eq!(
        decode(&tiff_rgb8(w, h, &o)).unwrap().raster.data,
        pattern(w, h)
    );
    let o = TiffOpts {
        extra_pages: 2,
        ..TiffOpts::default()
    };
    let d = decode(&tiff_rgb8(w, h, &o)).unwrap();
    assert_eq!(d.frames, 3);
    assert_eq!(d.raster.data, pattern(w, h));
    assert!(d.notices.contains(&"tiff.multi_page"));
}

fn bilevel_rgb(w: u32, h: u32) -> Vec<u8> {
    bilevel(w, h)
        .iter()
        .flat_map(|&black| if black { [0, 0, 0] } else { [255, 255, 255] })
        .collect()
}

#[test]
fn one_bit_tiffs_read_without_inversion() {
    let (w, h) = (40, 20);
    let want = bilevel_rgb(w, h);
    for (name, bytes) in [
        ("raw white-is-zero", tiff_bilevel_raw(w, h, true)),
        ("raw black-is-zero", tiff_bilevel_raw(w, h, false)),
        ("group 4", tiff_g4(w, h)),
    ] {
        match decode(&bytes) {
            Ok(d) => assert_eq!(d.raster.data, want, "{name}"),
            Err(e) => panic!("{name}: {e}"),
        }
    }
}

#[test]
fn tiff_icc_is_kept_byte_exact() {
    let icc = fake_icc(2000);
    let o = TiffOpts {
        icc: Some(icc.clone()),
        ..TiffOpts::default()
    };
    let d = decode(&tiff_rgb8(12, 8, &o)).unwrap();
    assert_eq!(d.icc.as_deref(), Some(&icc[..]));
}

// ------------------------------------------------------------------------------------ WebP

#[test]
fn webp_lossless_is_bit_exact_and_icc_is_kept() {
    let (w, h) = (27, 15);
    assert_eq!(
        decode(&webp_lossless(w, h)).unwrap().raster.data,
        pattern(w, h)
    );
    let icc = fake_icc(500);
    let d = decode(&webp_extended(w, h, None, Some(&icc))).unwrap();
    assert_eq!(d.raster.data, pattern(w, h));
    assert_eq!(d.icc.as_deref(), Some(&icc[..]));
}

#[test]
fn lossy_webp_decodes() {
    let d = decode(&webp_lossy_1x1()).unwrap();
    assert_eq!((d.raster.width, d.raster.height), (1, 1));
}

#[test]
fn animated_webp_gives_the_first_frame_and_the_flag() {
    let d = decode(&webp_animated(18, 10, 3)).unwrap();
    assert_eq!(d.frames, 3);
    assert!(d.notices.contains(&"anim.first_frame_only"));
    assert_eq!((d.raster.width, d.raster.height), (18, 10));
    // image-webp composites animation frames through an alpha blend, which costs one level on some
    // samples (observed: 255 -> 254); the picture is otherwise the first frame.
    assert!(max_diff(&d.raster.data, &pattern(18, 10)) <= 1);
}

// --------------------------------------------------------------------------------- general

#[test]
fn recognised_only_and_unknown_inputs_are_typed_errors() {
    assert!(matches!(
        decode(&heic_stub()),
        Err(CodecError::NotDecodable(Format::Heic))
    ));
    assert!(matches!(
        decode(b"plain text"),
        Err(CodecError::Unsupported)
    ));
    assert!(matches!(decode(&[]), Err(CodecError::Unsupported)));
}

// ------------------------------------------------------------------------ libjpeg-turbo oracle

/// Decodes `jpeg` with the `magick` command (ImageMagick built on libjpeg-turbo) to raw RGB, if the
/// tool is installed. The reference for the M1.15 acceptance number.
fn magick_rgb(name: &str, bytes: &[u8]) -> Option<Vec<u8>> {
    let dir = std::env::temp_dir().join(format!("auto-crop-magick-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    let src = dir.join(format!("{name}.jpg"));
    let dst = dir.join(format!("{name}.rgb"));
    std::fs::write(&src, bytes).ok()?;
    let out = std::process::Command::new("magick")
        .arg(&src)
        .args(["-depth", "8"])
        .arg(format!("rgb:{}", dst.display()))
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let raw = std::fs::read(&dst).ok()?;
    let _ = std::fs::remove_dir_all(&dir);
    Some(raw)
}

#[test]
fn jpeg_decode_matches_libjpeg_turbo_within_the_provisional_bound() {
    let (w, h) = (96, 64);
    let mut specs: Vec<(&str, JpegSpec)> = vec![("c444", JpegSpec::new(w, h))];
    let mut s = JpegSpec::new(w, h);
    s.sampling = (2, 2);
    specs.push(("c420", s));
    let mut s = JpegSpec::new(w, h);
    s.sampling = (2, 1);
    specs.push(("c422", s));
    let mut s = JpegSpec::new(w, h);
    s.progressive = true;
    s.sampling = (2, 2);
    specs.push(("prog420", s));
    let mut ran = 0;
    for (name, spec) in specs {
        let bytes = spec.build();
        let Some(reference) = magick_rgb(name, &bytes) else {
            eprintln!("magick not available: skipping the libjpeg-turbo comparison");
            return;
        };
        let ours = decode(&bytes).unwrap().raster.data;
        let m = mad(&ours, &reference);
        eprintln!(
            "zune-jpeg vs libjpeg-turbo, {name}: mean abs diff {m:.3} LSB, max {}",
            max_diff(&ours, &reference)
        );
        assert!(
            m <= 1.5,
            "{name}: {m} LSB exceeds the PROVISIONAL 1.5 bound"
        );
        ran += 1;
    }
    assert!(ran > 0);
}

#[test]
fn cmyk_and_ycck_conversions_equal_libjpeg_turbo_through_imagemagick() {
    for (name, ycck) in [("cmyk", false), ("ycck", true)] {
        let bytes = jpeg_cmyk(24, 16, ycck);
        let Some(reference) = magick_rgb(name, &bytes) else {
            eprintln!("magick not available: skipping the CMYK comparison");
            return;
        };
        let ours = decode(&bytes).unwrap().raster.data;
        // Measured: identical (0.000 LSB) on ImageMagick 7.1.2 with libjpeg-turbo 3.2.0.
        assert!(mad(&ours, &reference) <= 1.5, "{name}");
    }
}

// ------------------------------------------------------------------ ImageMagick (libtiff, libwebp)

/// Runs `magick` in a scratch directory: `write` creates input files, `args` get the directory
/// prepended to relative names; returns the content of `out` or `None` when ImageMagick is absent.
fn magick_make(tag: &str, inputs: &[(&str, Vec<u8>)], args: &[&str], out: &str) -> Option<Vec<u8>> {
    let dir = std::env::temp_dir().join(format!("auto-crop-magick-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    for (name, bytes) in inputs {
        std::fs::write(dir.join(name), bytes).ok()?;
    }
    let status = std::process::Command::new("magick")
        .current_dir(&dir)
        .args(args)
        .output()
        .ok()?;
    let data = status
        .status
        .success()
        .then(|| std::fs::read(dir.join(out)).ok())
        .flatten();
    let _ = std::fs::remove_dir_all(&dir);
    data
}

fn pbm_of(w: u32, h: u32) -> Vec<u8> {
    // Plain PBM: 1 = black.
    let mut s = format!("P1\n{w} {h}\n");
    for row in bilevel(w, h).chunks(w as usize) {
        for &b in row {
            s.push(if b { '1' } else { '0' });
        }
        s.push('\n');
    }
    s.into_bytes()
}

#[test]
fn a_libtiff_made_one_bit_group4_tiff_reads_or_the_failure_is_recorded() {
    let (w, h) = (53, 31);
    let Some(tif) = magick_make(
        "g4",
        &[("in.pbm", pbm_of(w, h))],
        &["in.pbm", "-compress", "Group4", "out.tif"],
        "out.tif",
    ) else {
        eprintln!("magick not available: skipping the libtiff G4 comparison");
        return;
    };
    // Confirm ImageMagick really wrote G4 (compression tag 259 = 4) in a 1-bit file.
    let p = crate::probe(&tif).unwrap();
    assert_eq!((p.width, p.height, p.bit_depth), (w, h, 1));
    let d = decode(&tif).unwrap_or_else(|e| panic!("libtiff G4 file failed to decode: {e}"));
    assert_eq!(d.raster.data, bilevel_rgb(w, h));
}

#[test]
fn libtiff_made_compressions_and_16_bit_decode() {
    let (w, h) = (37, 23);
    let ppm = {
        let mut v = format!("P6\n{w} {h}\n255\n").into_bytes();
        v.extend(pattern(w, h));
        v
    };
    for comp in ["LZW", "ZIP", "RLE", "None"] {
        let Some(tif) = magick_make(
            comp,
            &[("in.ppm", ppm.clone())],
            &["in.ppm", "-compress", comp, "out.tif"],
            "out.tif",
        ) else {
            eprintln!("magick not available: skipping libtiff comparisons");
            return;
        };
        let d = decode(&tif).unwrap_or_else(|e| panic!("{comp}: {e}"));
        assert_eq!(d.raster.data, pattern(w, h), "{comp}");
    }
    let Some(tif) = magick_make(
        "t16",
        &[("in.ppm", ppm.clone())],
        &["in.ppm", "-depth", "16", "-compress", "LZW", "out.tif"],
        "out.tif",
    ) else {
        eprintln!("magick could not write a 16-bit TIFF: skipping");
        return;
    };
    let d = decode(&tif).unwrap();
    assert_eq!(d.source_bit_depth, 16);
    assert!(max_diff(&d.raster.data, &pattern(w, h)) <= 1);
}

fn psnr(a: &[u8], b: &[u8]) -> f64 {
    let mse = a
        .iter()
        .zip(b)
        .map(|(x, y)| {
            let d = f64::from(*x) - f64::from(*y);
            d * d
        })
        .sum::<f64>()
        / a.len() as f64;
    if mse == 0.0 {
        f64::INFINITY
    } else {
        10.0 * (255.0f64 * 255.0 / mse).log10()
    }
}

#[test]
fn lossy_webp_made_by_libwebp_decodes_within_the_provisional_psnr() {
    let (w, h) = (64, 48);
    let ppm = {
        let mut v = format!("P6\n{w} {h}\n255\n").into_bytes();
        v.extend(smooth(w, h));
        v
    };
    let Some(webp) = magick_make(
        "webp",
        &[("in.ppm", ppm)],
        &["in.ppm", "-quality", "85", "out.webp"],
        "out.webp",
    ) else {
        eprintln!("magick not available: skipping the libwebp comparison");
        return;
    };
    assert_eq!(
        &webp[12..16],
        b"VP8 ",
        "libwebp must have produced a lossy file"
    );
    let Some(reference) = magick_make(
        "webpdec",
        &[("in.webp", webp.clone())],
        &["in.webp", "-depth", "8", "out.rgb"],
        "out.rgb",
    ) else {
        eprintln!("magick could not decode the WebP: skipping");
        return;
    };
    let d = decode(&webp).unwrap();
    assert_eq!((d.raster.width, d.raster.height), (w, h));
    let p = psnr(&d.raster.data, &reference);
    eprintln!("image-webp vs libwebp decode of a lossy file: {p:.1} dB");
    assert!(p >= 40.0, "PSNR {p} dB");
}
