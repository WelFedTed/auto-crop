// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Round trips of the TIFF and WebP writers through readers that are not ours: the `tiff` crate
//! (its own decoder, including its CCITT path through `fax`'s decoder), `image-webp` and the
//! repo's `decode`, plus the failure paths. The independent command-line readers (`tiffinfo`,
//! ImageMagick, `dwebp`) run in the CI `interop` job on Linux.

use super::bilevel::BilevelImage;
use super::*;
use crate::fixtures::{fake_icc, pattern, pattern_gray};
use auto_crop_core::output::Bilevel;
use auto_crop_core::ports::{Raster, Samples};
use std::io::Cursor;
use tiff::decoder::{Decoder, DecodingResult};
use tiff::tags::Tag;

fn rgb8(w: u32, h: u32) -> Raster {
    Raster::from_samples(w, h, PixelFormat::Rgb8, Samples::U8(pattern(w, h))).unwrap()
}

fn gray8(w: u32, h: u32) -> Raster {
    Raster::from_samples(w, h, PixelFormat::Gray8, Samples::U8(pattern_gray(w, h))).unwrap()
}

fn rgb16(w: u32, h: u32) -> Raster {
    let v: Vec<u16> = (0..w * h * 3)
        .map(|i| (i.wrapping_mul(2551) % 65_536) as u16)
        .collect();
    Raster::from_samples(w, h, PixelFormat::Rgb16, Samples::U16(v)).unwrap()
}

fn gray16(w: u32, h: u32) -> Raster {
    let v: Vec<u16> = (0..w * h)
        .map(|i| (i.wrapping_mul(40_503) % 65_536) as u16)
        .collect();
    Raster::from_samples(w, h, PixelFormat::Gray16, Samples::U16(v)).unwrap()
}

fn never() -> CancelToken {
    CancelToken::never()
}

fn tiff_of(enc: &TiffEncoder, r: &Raster, meta: &EncodeMeta) -> Vec<u8> {
    enc.encode(&r.view(), &OutputSpec::default(), meta, &never())
        .unwrap()
}

fn read_tiff(bytes: &[u8]) -> (Decoder<Cursor<&[u8]>>, DecodingResult) {
    let mut d = Decoder::new(Cursor::new(bytes)).unwrap();
    let img = d.read_image().unwrap();
    (d, img)
}

fn u8s(r: DecodingResult) -> Vec<u8> {
    match r {
        DecodingResult::U8(v) => v,
        other => panic!("8-bit data expected, got {other:?}"),
    }
}

fn u16s(r: DecodingResult) -> Vec<u16> {
    match r {
        DecodingResult::U16(v) => v,
        other => panic!("16-bit data expected, got {other:?}"),
    }
}

const COMPRESSIONS: [TiffCompression; 4] = [
    TiffCompression::None,
    TiffCompression::Lzw,
    TiffCompression::Deflate,
    TiffCompression::PackBits,
];

#[test]
fn tiff_round_trips_every_compression_and_layout_exactly_in_the_tiff_crate() {
    // 300 x 200 RGB8 is 180 KB, so a small strip size makes several strips; the last one is short.
    for compression in COMPRESSIONS {
        for predictor in [true, false] {
            for rps in [None, Some(7), Some(1)] {
                let enc = TiffEncoder {
                    compression,
                    predictor,
                    rows_per_strip: rps,
                    ..TiffEncoder::default()
                };
                let meta = EncodeMeta::default();
                let r = rgb8(300, 200);
                let (_, img) = read_tiff(&tiff_of(&enc, &r, &meta));
                assert_eq!(
                    u8s(img),
                    pattern(300, 200),
                    "rgb8 {compression:?} predictor {predictor} rps {rps:?}"
                );
                let g = gray8(129, 61);
                let (_, img) = read_tiff(&tiff_of(&enc, &g, &meta));
                assert_eq!(
                    u8s(img),
                    pattern_gray(129, 61),
                    "gray8 {compression:?} predictor {predictor} rps {rps:?}"
                );
                let r16 = rgb16(33, 29);
                let (_, img) = read_tiff(&tiff_of(&enc, &r16, &meta));
                let Samples::U16(want) = r16.samples() else {
                    unreachable!()
                };
                assert_eq!(
                    u16s(img),
                    want.clone(),
                    "rgb16 {compression:?} predictor {predictor} rps {rps:?}"
                );
                let g16 = gray16(40, 17);
                let (_, img) = read_tiff(&tiff_of(&enc, &g16, &meta));
                let Samples::U16(want) = g16.samples() else {
                    unreachable!()
                };
                assert_eq!(
                    u16s(img),
                    want.clone(),
                    "gray16 {compression:?} predictor {predictor} rps {rps:?}"
                );
            }
        }
    }
}

#[test]
fn tiff_is_decoded_back_by_the_repos_own_decoder_too() {
    for compression in COMPRESSIONS {
        let enc = TiffEncoder {
            compression,
            ..TiffEncoder::default()
        };
        let bytes = tiff_of(&enc, &rgb8(97, 53), &EncodeMeta::default());
        let d = crate::decode(&bytes).unwrap();
        assert_eq!(d.format, crate::Format::Tiff);
        assert_eq!(d.raster.data, pattern(97, 53));
        assert_eq!(d.exif_orientation, 1);
        assert_eq!(d.frames, 1);
        assert!(d.notices.is_empty(), "{:?}", d.notices);
        // A grey file decodes as RGB with equal channels.
        let bytes = tiff_of(&enc, &gray8(97, 53), &EncodeMeta::default());
        let d = crate::decode(&bytes).unwrap();
        let want: Vec<u8> = pattern_gray(97, 53).iter().flat_map(|v| [*v; 3]).collect();
        assert_eq!(d.raster.data, want);
    }
}

#[test]
fn tiff_writes_orientation_one_the_icc_profile_and_the_resolution() {
    let icc = fake_icc(5000);
    let meta = EncodeMeta {
        icc: Some(icc.clone().into()),
        dpi: Some((300, 150)),
    };
    let bytes = tiff_of(&TiffEncoder::default(), &rgb8(64, 40), &meta);
    let (mut d, _) = read_tiff(&bytes);
    assert_eq!(d.get_tag_u32(Tag::Orientation).unwrap(), 1);
    assert_eq!(d.get_tag_u8_vec(Tag::IccProfile).unwrap(), icc);
    assert_eq!(d.get_tag_u32(Tag::ResolutionUnit).unwrap(), 2);
    let rat = |d: &mut Decoder<Cursor<&[u8]>>, t| match d.get_tag(t).unwrap() {
        tiff::decoder::ifd::Value::Rational(n, den) => f64::from(n) / f64::from(den),
        v => panic!("{v:?}"),
    };
    assert_eq!(rat(&mut d, Tag::XResolution), 300.0);
    assert_eq!(rat(&mut d, Tag::YResolution), 150.0);
    assert_eq!(d.get_tag_u32(Tag::Predictor).unwrap(), 2);
    assert_eq!(d.get_tag_ascii_string(Tag::Software).unwrap(), "Auto Crop");
    // The repo's decoder reads the profile back byte-exact too.
    assert_eq!(crate::decode(&bytes).unwrap().icc, Some(icc));
    // No EXIF, GPS, XMP or thumbnail tags: nothing about the source can leak.
    for t in [Tag::Unknown(34665), Tag::Unknown(34853), Tag::Unknown(700)] {
        assert!(d.find_tag(t).unwrap().is_none(), "{t:?}");
    }
}

#[test]
fn tiff_without_a_known_dpi_says_unit_none_and_never_invents_one() {
    let bytes = tiff_of(
        &TiffEncoder::default(),
        &gray8(20, 20),
        &EncodeMeta::default(),
    );
    let (mut d, _) = read_tiff(&bytes);
    assert_eq!(d.get_tag_u32(Tag::ResolutionUnit).unwrap(), 1);
    assert!(d.get_tag_u8_vec(Tag::IccProfile).is_err());
}

#[test]
fn tiff_is_deterministic_and_keeps_strips_on_even_offsets() {
    let enc = TiffEncoder {
        rows_per_strip: Some(5),
        compression: TiffCompression::PackBits,
        ..TiffEncoder::default()
    };
    let a = tiff_of(&enc, &rgb8(31, 23), &EncodeMeta::default());
    assert_eq!(a, tiff_of(&enc, &rgb8(31, 23), &EncodeMeta::default()));
    let (mut d, _) = read_tiff(&a);
    for off in d.get_tag_u32_vec(Tag::StripOffsets).unwrap() {
        assert_eq!(off % 2, 0);
    }
}

fn text_like(w: u32, h: u32) -> Raster {
    // Black marks on white: a frame, text-like bars and a diagonal, plus two pixels either side of
    // the cut (127 must be black, 128 white).
    let mut v = vec![255u8; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            let on = x == 0
                || y == 0
                || x == w - 1
                || y == h - 1
                || (y / 4 % 3 == 1 && x > 4 && x < w - 5)
                || x == y;
            if on {
                v[(y * w + x) as usize] = 0;
            }
        }
    }
    v[(h / 2 * w + 2) as usize] = 127;
    v[(h / 2 * w + 3) as usize] = 128;
    Raster::from_samples(w, h, PixelFormat::Gray8, Samples::U8(v)).unwrap()
}

fn g4_spec() -> OutputSpec {
    OutputSpec {
        bits: Bits::One {
            bilevel: Bilevel::TiffG4,
        },
        ..OutputSpec::default()
    }
}

#[test]
fn a_one_bit_g4_tiff_has_the_right_polarity_in_three_readers_and_every_strip_layout() {
    let src = text_like(203, 77);
    let want = BilevelImage::from_view(&src.view(), 128, &never()).unwrap();
    // The cut: 127 is black, 128 is white.
    assert!(want.is_black(2, 77 / 2) && !want.is_black(3, 77 / 2));
    for rps in [None, Some(1), Some(10), Some(77), Some(500)] {
        let enc = TiffEncoder {
            rows_per_strip: rps,
            ..TiffEncoder::default()
        };
        let meta = EncodeMeta {
            icc: None,
            dpi: Some((600, 600)),
        };
        let bytes = enc
            .encode(&src.view(), &g4_spec(), &meta, &never())
            .unwrap();
        // Reader 1: the `tiff` crate (its CCITT path).
        let mut d = Decoder::new(Cursor::new(&bytes[..])).unwrap();
        assert_eq!(d.get_tag_u32(Tag::Compression).unwrap(), 4);
        assert_eq!(d.get_tag_u32(Tag::PhotometricInterpretation).unwrap(), 0);
        assert_eq!(d.get_tag_u32(Tag::BitsPerSample).unwrap(), 1);
        assert_eq!(d.get_tag_u32(Tag::Orientation).unwrap(), 1);
        assert_eq!(d.get_tag_u32(Tag::ResolutionUnit).unwrap(), 2);
        let DecodingResult::U8(packed) = d.read_image().unwrap() else {
            panic!("1-bit data comes back as packed bytes");
        };
        let stride = 203usize.div_ceil(8);
        assert_eq!(packed.len(), stride * 77);
        for y in 0..77u32 {
            for x in 0..203u32 {
                let bit = packed[y as usize * stride + x as usize / 8] & (0x80 >> (x % 8)) != 0;
                // The crate hands back 1 = white (it normalises Group 4 to BlackIsZero).
                assert_eq!(
                    bit,
                    !want.is_black(x, y),
                    "tiff crate ({x},{y}) rps {rps:?}"
                );
            }
        }
        // Reader 2: the repo's decoder: black is 0, white is 255, no inversion.
        let dec = crate::decode(&bytes).unwrap();
        assert_eq!((dec.raster.width, dec.raster.height), (203, 77));
        for y in 0..77u32 {
            for x in 0..203u32 {
                let px = dec.raster.data[((y * 203 + x) * 3) as usize];
                assert_eq!(px == 0, want.is_black(x, y), "repo decode ({x},{y})");
            }
        }
        // Reader 3: the `fax` decoder on the first strip alone.
        let rows = rps.unwrap_or(77).clamp(1, 77);
        let mut d = Decoder::new(Cursor::new(&bytes[..])).unwrap();
        let offs = d.get_tag_u32_vec(Tag::StripOffsets).unwrap();
        let cnts = d.get_tag_u32_vec(Tag::StripByteCounts).unwrap();
        let strip0 = &bytes[offs[0] as usize..(offs[0] + cnts[0]) as usize];
        let back = BilevelImage::from_g4(strip0, 203, rows).unwrap();
        for y in 0..rows {
            for x in 0..203 {
                assert_eq!(back.is_black(x, y), want.is_black(x, y));
            }
        }
        assert_eq!(offs.len(), 77usize.div_ceil(rows as usize));
    }
}

#[test]
fn a_g4_tiff_is_far_smaller_than_the_raw_bitmap_for_text() {
    let src = text_like(800, 1000);
    let bytes = TiffEncoder::default()
        .encode(&src.view(), &g4_spec(), &EncodeMeta::default(), &never())
        .unwrap();
    assert!(bytes.len() < 800 * 1000 / 8 / 4, "{}", bytes.len());
}

#[test]
fn a_one_bit_tiff_of_a_colour_raster_thresholds_the_luma() {
    let mut data = vec![255u8; 8 * 2 * 3];
    // Pixel (0,0): pure red is luma 76, black. Pixel (1,0): pure green is luma 149, white.
    data[0..3].copy_from_slice(&[255, 0, 0]);
    data[3..6].copy_from_slice(&[0, 255, 0]);
    let src = Raster::from_samples(8, 2, PixelFormat::Rgb8, Samples::U8(data)).unwrap();
    let b = BilevelImage::from_view(&src.view(), 128, &never()).unwrap();
    assert!(b.is_black(0, 0) && !b.is_black(1, 0) && !b.is_black(7, 1));
}

#[test]
fn the_tiff_writer_refuses_what_it_cannot_hold_and_honours_cancel() {
    let enc = TiffEncoder::default();
    let none = EncodeMeta::default();
    let src = gray8(16, 16);
    let png_bits = OutputSpec {
        bits: Bits::One {
            bilevel: Bilevel::Png,
        },
        ..OutputSpec::default()
    };
    assert_eq!(
        enc.encode(&src.view(), &png_bits, &none, &never()),
        Err(ErrKind::UnsupportedOutput)
    );
    // Wider than the Group 4 coder takes.
    let wide = Raster::from_samples(
        70_000,
        1,
        PixelFormat::Gray8,
        Samples::U8(vec![255; 70_000]),
    )
    .unwrap();
    assert_eq!(
        enc.encode(&wide.view(), &g4_spec(), &none, &never()),
        Err(ErrKind::UnsupportedOutput)
    );
    // ... but an ordinary compression takes it.
    assert!(
        enc.encode(&wide.view(), &OutputSpec::default(), &none, &never())
            .is_ok()
    );
    let empty = Raster::from_samples(0, 0, PixelFormat::Gray8, Samples::U8(vec![])).unwrap();
    assert_eq!(
        enc.encode(&empty.view(), &OutputSpec::default(), &none, &never()),
        Err(ErrKind::UnsupportedOutput)
    );
    let c = CancelToken::new_batch();
    c.cancel();
    assert_eq!(
        enc.encode(&src.view(), &OutputSpec::default(), &none, &c),
        Err(ErrKind::Cancelled)
    );
}

#[test]
fn packbits_runs_and_literals_unpack_to_the_input() {
    fn unpack(p: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < p.len() {
            let n = p[i] as i8;
            i += 1;
            if n >= 0 {
                out.extend_from_slice(&p[i..i + n as usize + 1]);
                i += n as usize + 1;
            } else if n != -128 {
                out.extend(std::iter::repeat_n(p[i], (1 - i16::from(n)) as usize));
                i += 1;
            }
        }
        out
    }
    let mut seed = 12345u32;
    for len in [0usize, 1, 2, 3, 127, 128, 129, 300, 1000] {
        for mode in 0..3 {
            let row: Vec<u8> = (0..len)
                .map(|i| {
                    seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                    match mode {
                        0 => (seed >> 16) as u8,
                        1 => 7,
                        _ => ((i / 5) % 3) as u8,
                    }
                })
                .collect();
            let mut p = Vec::new();
            super::tiff_encoder::packbits_row(&row, &mut p);
            assert_eq!(unpack(&p), row, "len {len} mode {mode}");
        }
    }
}

// ---------------------------------------------------------------------------------------------
// WebP
// ---------------------------------------------------------------------------------------------

fn webp_of(enc: &WebpEncoder, r: &Raster, meta: &EncodeMeta) -> Vec<u8> {
    enc.encode(&r.view(), &OutputSpec::default(), meta, &never())
        .unwrap()
}

#[test]
fn lossless_webp_is_bit_exact_in_image_webp_and_the_repos_decoder() {
    let none = EncodeMeta::default();
    let r = rgb8(150, 90);
    let bytes = webp_of(&WebpEncoder::default(), &r, &none);
    let d = crate::decode(&bytes).unwrap();
    assert_eq!(d.format, crate::Format::Webp);
    assert_eq!(d.raster.data, pattern(150, 90));
    assert_eq!(d.exif_orientation, 1);
    assert!(d.notices.is_empty(), "{:?}", d.notices);
    // A second, independent decode through image-webp directly.
    let mut dec = image_webp::WebPDecoder::new(Cursor::new(&bytes[..])).unwrap();
    assert_eq!(dec.dimensions(), (150, 90));
    let mut buf = vec![0u8; dec.output_buffer_size().unwrap()];
    dec.read_image(&mut buf).unwrap();
    let rgb: Vec<u8> = if dec.has_alpha() {
        buf.as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect()
    } else {
        buf
    };
    assert_eq!(rgb, pattern(150, 90));
    // Grey comes back as RGB with equal channels.
    let g = gray8(77, 41);
    let d = crate::decode(&webp_of(&WebpEncoder::default(), &g, &none)).unwrap();
    let want: Vec<u8> = pattern_gray(77, 41).iter().flat_map(|v| [*v; 3]).collect();
    assert_eq!(d.raster.data, want);
}

#[test]
fn webp_carries_the_icc_profile_byte_exact_and_exif_and_xmp_chunks() {
    let icc = fake_icc(3333);
    let meta = EncodeMeta {
        icc: Some(icc.clone().into()),
        dpi: Some((300, 300)),
    };
    let exif = crate::fixtures::exif_blob(1, false);
    let enc = WebpEncoder {
        exif: Some(exif.clone()),
        xmp: Some(b"<x:xmpmeta/>".to_vec()),
    };
    let bytes = webp_of(&enc, &rgb8(40, 30), &meta);
    let d = crate::decode(&bytes).unwrap();
    assert_eq!(d.icc, Some(icc));
    assert_eq!(d.exif_orientation, 1);
    assert_eq!(d.raster.data, pattern(40, 30));
    assert_eq!(
        crate::extract_exif(&bytes, crate::Format::Webp, &crate::DecodeLimits::default()),
        Some(exif)
    );
    // The chunks are there, in the extended container.
    assert_eq!(&bytes[12..16], b"VP8X");
    assert!(bytes.windows(4).any(|w| w == b"XMP "));
}

#[test]
fn webp_refuses_what_it_cannot_hold() {
    let enc = WebpEncoder::default();
    let none = EncodeMeta::default();
    let spec = OutputSpec::default();
    let wide16 = rgb16(8, 8);
    assert_eq!(
        enc.encode(&wide16.view(), &spec, &none, &never()),
        Err(ErrKind::UnsupportedOutput)
    );
    let big =
        Raster::from_samples(16_384, 1, PixelFormat::Gray8, Samples::U8(vec![0; 16_384])).unwrap();
    assert_eq!(
        enc.encode(&big.view(), &spec, &none, &never()),
        Err(ErrKind::UnsupportedOutput)
    );
    let ok =
        Raster::from_samples(16_383, 1, PixelFormat::Gray8, Samples::U8(vec![9; 16_383])).unwrap();
    assert!(enc.encode(&ok.view(), &spec, &none, &never()).is_ok());
    let one_bit = OutputSpec {
        bits: Bits::One {
            bilevel: Bilevel::TiffG4,
        },
        ..OutputSpec::default()
    };
    assert_eq!(
        enc.encode(&gray8(8, 8).view(), &one_bit, &none, &never()),
        Err(ErrKind::UnsupportedOutput)
    );
    let c = CancelToken::new_batch();
    c.cancel();
    assert_eq!(
        enc.encode(&gray8(8, 8).view(), &spec, &none, &c),
        Err(ErrKind::Cancelled)
    );
}
