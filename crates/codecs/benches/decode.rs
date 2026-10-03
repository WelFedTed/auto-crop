// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! ROADMAP M1.56: decode benchmarks behind ADR-0008.
//!
//! * JPEG, 12 MP (4000 x 3000, 4:2:0, q90, photo-like content): the product's pure-Rust path
//!   (`decode`: pre-checks, zune-jpeg, orientation) against libjpeg-turbo (`decode_scaled`, feature
//!   `turbojpeg`) at full size and at 1/2, 1/4 and 1/8 DCT scaling, plus what the safe-Rust
//!   fallback costs for the same output size (full decode, then a block average).
//! * PNG, TIFF (uncompressed, LZW, Deflate) and WebP (lossless), 4 MP.
//!
//! Run: `cargo bench -p auto-crop-codecs --features fixtures[,turbojpeg] --bench decode`. Numbers
//! from a shared or loaded machine are labelled NOISY in the ADR.

use auto_crop_codecs::decode;
use auto_crop_codecs::encoders::PngEncoder;
use auto_crop_codecs::fixtures::{JpegSpec, TiffComp, TiffOpts, photo, tiff_rgb8, webp_lossless};
#[cfg(feature = "turbojpeg")]
use auto_crop_codecs::{DecodeLimits, decode_scaled};
use auto_crop_core::CancelToken;
use auto_crop_core::output::OutputSpec;
#[cfg(feature = "turbojpeg")]
use auto_crop_core::ports::Want;
use auto_crop_core::ports::{EncodeMeta, Encoder, PixelFormat, Raster, Samples};
use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;
use std::time::Duration;

fn jpeg_12mp() -> Vec<u8> {
    let (w, h) = (4000u32, 3000u32);
    let mut s = JpegSpec::new(w, h);
    s.sampling = (2, 2);
    s.quality = 90;
    s.encode(&photo(w, h, 1), jpeg_encoder::ColorType::Rgb)
}

fn bench_jpeg(c: &mut Criterion) {
    let bytes = jpeg_12mp();
    #[cfg(feature = "turbojpeg")]
    let limits = DecodeLimits::default();
    let mut g = c.benchmark_group("jpeg_12MP_4000x3000_420_q90");
    g.sample_size(15)
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(4))
        .throughput(Throughput::Elements(12_000_000));
    // The shipped pure-Rust path.
    g.bench_function("zune_full (decode)", |b| {
        b.iter(|| black_box(decode(black_box(&bytes)).unwrap()))
    });
    // What the safe-Rust path costs for a 1/4-size result: the full decode, then an area
    // average to 1000 x 750 (the fallback of `decode_scaled` does a block average of the same cost
    // class). Compare with `turbo_1_4`.
    g.bench_function("zune_full_then_area_1_4", |b| {
        b.iter(|| {
            let full = decode(&bytes).unwrap().raster;
            black_box(auto_crop_imgproc::scale::resize_area(&full, 1000, 750))
        })
    });
    // libjpeg-turbo through the same entry point: full, 1/2, 1/4, 1/8.
    #[cfg(feature = "turbojpeg")]
    for (name, want) in [
        ("turbo_full", Want::Full),
        ("turbo_1_2", Want::Scaled { min_edge: 2000 }),
        ("turbo_1_4", Want::Scaled { min_edge: 1000 }),
        ("turbo_1_8", Want::Scaled { min_edge: 500 }),
    ] {
        g.bench_function(name, |b| {
            b.iter(|| black_box(decode_scaled(&bytes, want, &limits).unwrap()))
        });
    }
    g.finish();
}

fn bench_other_formats(c: &mut Criterion) {
    let (w, h) = (2000u32, 2000u32);
    let rgb = photo(w, h, 2);
    let raster = Raster::from_samples(w, h, PixelFormat::Rgb8, Samples::U8(rgb)).unwrap();
    let png = PngEncoder
        .encode(
            &raster.view(),
            &OutputSpec::default(),
            &EncodeMeta::default(),
            &CancelToken::never(),
        )
        .unwrap();
    let tiff = |comp| {
        tiff_rgb8(
            w,
            h,
            &TiffOpts {
                comp: Some(comp),
                ..TiffOpts::default()
            },
        )
    };
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("png_rgb8_photo", png),
        ("tiff_uncompressed", tiff(TiffComp::None)),
        ("tiff_lzw", tiff(TiffComp::Lzw)),
        ("tiff_deflate", tiff(TiffComp::Deflate)),
        ("webp_lossless", webp_lossless(w, h)),
    ];
    let mut g = c.benchmark_group("other_formats_4MP_2000x2000");
    g.sample_size(15)
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(3))
        .throughput(Throughput::Elements(4_000_000));
    for (name, bytes) in &cases {
        g.bench_function(*name, |b| {
            b.iter(|| black_box(decode(black_box(bytes)).unwrap()))
        });
    }
    g.finish();
}

criterion_group!(benches, bench_jpeg, bench_other_formats);
criterion_main!(benches);
