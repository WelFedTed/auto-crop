// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

use super::*;
use crate::fixtures::{fake_icc, smooth};
use auto_crop_core::output::Bilevel;
use auto_crop_core::ports::{Raster, Samples};

fn rgb8(w: u32, h: u32) -> Raster {
    Raster::from_samples(w, h, PixelFormat::Rgb8, Samples::U8(smooth(w, h))).unwrap()
}

fn rgb16(w: u32, h: u32) -> Raster {
    let v: Vec<u16> = (0..w * h * 3).map(|i| (i * 2551 % 65_536) as u16).collect();
    Raster::from_samples(w, h, PixelFormat::Rgb16, Samples::U16(v)).unwrap()
}

fn spec(q: u8) -> OutputSpec {
    OutputSpec {
        quality: Quality::Fixed { value: q },
        ..OutputSpec::default()
    }
}

fn psnr(a: &[u8], b: &[u8]) -> f64 {
    let sse: f64 = a
        .iter()
        .zip(b)
        .map(|(x, y)| (f64::from(*x) - f64::from(*y)).powi(2))
        .sum();
    10.0 * (255.0f64 * 255.0 / (sse / a.len() as f64).max(1e-12)).log10()
}

fn never() -> CancelToken {
    CancelToken::never()
}

fn u8_samples(r: &Raster) -> &[u8] {
    match r.samples() {
        Samples::U8(v) => v,
        Samples::U16(_) => panic!("8-bit raster expected"),
    }
}

#[test]
fn jpeg_rs_round_trips_at_q90_above_40_db_and_embeds_icc_and_density() {
    let src = rgb8(160, 120);
    let meta = EncodeMeta {
        icc: Some(fake_icc(1500).into()),
        dpi: Some((300, 300)),
    };
    let out = JpegRsEncoder::default()
        .encode(&src.view(), &spec(90), &meta, &never())
        .unwrap();
    let back = crate::decode(&out).unwrap();
    let p = psnr(&back.raster.data, u8_samples(&src));
    println!(
        "jpeg-encoder q90 4:2:0 round trip: {p:.2} dB, {} bytes",
        out.len()
    );
    assert!(p >= 40.0, "{p}");
    assert_eq!(back.icc, Some(fake_icc(1500)));
    // JFIF APP0: units = 1 (dpi), X and Y density.
    assert_eq!(&out[6..11], b"JFIF\0");
    assert_eq!(out[13], 1);
    assert_eq!(u16::from_be_bytes([out[14], out[15]]), 300);
}

#[test]
fn jpeg_rs_quality_follows_the_spec_and_defaults_to_90() {
    let src = rgb8(96, 96);
    let enc = JpegRsEncoder::default();
    let none = EncodeMeta::default();
    let size = |q| {
        enc.encode(&src.view(), &spec(q), &none, &never())
            .unwrap()
            .len()
    };
    assert!(size(30) < size(95));
    // MatchSource with no source estimate uses the encoder default, clamped to floor..=cap.
    let ms = |floor, cap| OutputSpec {
        quality: Quality::MatchSource { floor, cap },
        ..OutputSpec::default()
    };
    let at = |q| enc.encode(&src.view(), &spec(q), &none, &never()).unwrap();
    let a = enc
        .encode(&src.view(), &ms(80, 95), &none, &never())
        .unwrap();
    assert_eq!(a, at(90));
    let b = enc
        .encode(&src.view(), &ms(40, 60), &none, &never())
        .unwrap();
    assert_eq!(b, at(60));
}

#[test]
fn jpeg_handles_grey_and_16_bit_input_and_refuses_what_it_cannot_write() {
    let none = EncodeMeta::default();
    let g = Raster::from_samples(40, 30, PixelFormat::Gray8, Samples::U8(vec![77; 1200])).unwrap();
    let out = JpegRsEncoder::default()
        .encode(&g.view(), &spec(90), &none, &never())
        .unwrap();
    assert_eq!(crate::probe(&out).unwrap().channels, 1);
    let wide = rgb16(32, 32);
    let out = JpegRsEncoder::default()
        .encode(&wide.view(), &spec(90), &none, &never())
        .unwrap();
    assert_eq!(crate::probe(&out).unwrap().bit_depth, 8);
    let one_bit = OutputSpec {
        bits: Bits::One {
            bilevel: Bilevel::Png,
        },
        ..spec(90)
    };
    assert_eq!(
        JpegRsEncoder::default().encode(&g.view(), &one_bit, &none, &never()),
        Err(ErrKind::UnsupportedOutput)
    );
    let lossless = OutputSpec {
        quality: Quality::Lossless,
        ..OutputSpec::default()
    };
    assert_eq!(
        JpegRsEncoder::default().encode(&g.view(), &lossless, &none, &never()),
        Err(ErrKind::UnsupportedOutput)
    );
}

#[test]
fn a_cancelled_token_stops_every_encoder() {
    let t = CancelToken::new_batch();
    t.cancel();
    let src = rgb8(16, 16);
    let (s, m) = (OutputSpec::default(), EncodeMeta::default());
    assert_eq!(
        JpegRsEncoder::default().encode(&src.view(), &s, &m, &t),
        Err(ErrKind::Cancelled)
    );
    assert_eq!(
        PngEncoder.encode(&src.view(), &s, &m, &t),
        Err(ErrKind::Cancelled)
    );
}

/// (frame info, pixel bytes, pHYs, iCCP)
fn png_decode(
    bytes: &[u8],
) -> (
    png::OutputInfo,
    Vec<u8>,
    Option<png::PixelDimensions>,
    Option<Vec<u8>>,
) {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::IDENTITY);
    let mut r = dec.read_info().unwrap();
    let mut buf = vec![0; r.output_buffer_size().unwrap()];
    let info = r.next_frame(&mut buf).unwrap();
    let dims = r.info().pixel_dims;
    let icc = r.info().icc_profile.as_ref().map(|p| p.to_vec());
    buf.truncate(info.buffer_size());
    (info, buf, dims, icc)
}

#[test]
fn png_8_bit_is_bit_exact_and_carries_dpi_and_icc() {
    let src = rgb8(50, 40);
    let meta = EncodeMeta {
        icc: Some(fake_icc(900).into()),
        dpi: Some((300, 150)),
    };
    let out = PngEncoder
        .encode(&src.view(), &OutputSpec::default(), &meta, &never())
        .unwrap();
    let (info, data, dims, icc) = png_decode(&out);
    assert_eq!(
        (info.width, info.height, info.bit_depth),
        (50, 40, png::BitDepth::Eight)
    );
    assert_eq!(data, u8_samples(&src));
    let dims = dims.expect("pHYs");
    assert_eq!(
        (dims.xppu, dims.yppu, dims.unit),
        (11_811, 5_906, png::Unit::Meter)
    );
    assert_eq!(icc, Some(fake_icc(900)));
    // And our own decoder agrees.
    let d = crate::decode(&out).unwrap();
    assert_eq!(d.raster.data, u8_samples(&src));
    assert_eq!(d.icc, Some(fake_icc(900)));
}

#[test]
fn png_16_bit_is_bit_exact() {
    for (format, channels) in [(PixelFormat::Rgb16, 3usize), (PixelFormat::Gray16, 1)] {
        let (w, h) = (37u32, 23u32);
        let v: Vec<u16> = (0..w as usize * h as usize * channels)
            .map(|i| (i * 7919 % 65_536) as u16)
            .collect();
        let src = Raster::from_samples(w, h, format, Samples::U16(v.clone())).unwrap();
        let out = PngEncoder
            .encode(
                &src.view(),
                &OutputSpec::default(),
                &EncodeMeta::default(),
                &never(),
            )
            .unwrap();
        let (info, data, _, _) = png_decode(&out);
        assert_eq!(info.bit_depth, png::BitDepth::Sixteen);
        let got: Vec<u16> = data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_be_bytes(*b))
            .collect();
        assert_eq!(got, v, "{format:?}");
    }
}

#[test]
fn png_grey_8_bit_and_row_bands_encode_the_visible_window() {
    let g = Raster::from_samples(8, 4, PixelFormat::Gray8, Samples::U8((0..32).collect())).unwrap();
    let (s, m) = (OutputSpec::default(), EncodeMeta::default());
    let out = PngEncoder.encode(&g.view(), &s, &m, &never()).unwrap();
    assert_eq!(png_decode(&out).1, (0..32).collect::<Vec<u8>>());
    // Rows 1..3 of the same raster.
    let band = g.view().rows(1, 3);
    let out = PngEncoder.encode(&band, &s, &m, &never()).unwrap();
    assert_eq!(png_decode(&out).1, (8..24).collect::<Vec<u8>>());
}

#[cfg(feature = "turbojpeg")]
#[test]
fn the_libjpeg_turbo_encoder_round_trips_at_q90_above_40_db() {
    let src = rgb8(160, 120);
    let meta = EncodeMeta {
        icc: Some(fake_icc(1500).into()),
        dpi: Some((300, 300)),
    };
    let out = JpegTurboEncoder::default()
        .encode(&src.view(), &spec(90), &meta, &never())
        .unwrap();
    let back = crate::decode(&out).unwrap();
    let p = psnr(&back.raster.data, u8_samples(&src));
    println!(
        "libjpeg-turbo q90 4:2:0 round trip: {p:.2} dB, {} bytes",
        out.len()
    );
    assert!(p >= 40.0, "{p}");
    assert_eq!(back.icc, Some(fake_icc(1500)));
    let g = Raster::from_samples(40, 30, PixelFormat::Gray8, Samples::U8(vec![77; 1200])).unwrap();
    let out = JpegTurboEncoder::default()
        .encode(&g.view(), &spec(90), &EncodeMeta::default(), &never())
        .unwrap();
    assert_eq!(crate::probe(&out).unwrap().channels, 1);
}
