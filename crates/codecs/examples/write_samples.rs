// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `write_samples <dir>`: writes one file per output writer (TIFF in every compression, 1-bit G4
//! TIFF, lossless WebP, PDF in every page type) plus the reference PNGs they must decode to, for
//! the independent readers of `tools/interop/check-output-formats.sh` (libtiff, ImageMagick, `dwebp`,
//! `qpdf`, Poppler). The pictures are the repo's synthetic test patterns, never real photographs.
//!
//! Run: `cargo run -p auto-crop-codecs --features fixtures[,avif] --example write_samples -- target/interop`

use auto_crop_codecs::encoders::bilevel::{BilevelImage, DEFAULT_CUT};
use auto_crop_codecs::encoders::pdf::{PageImage, PdfEncoder, PdfWriter};
use auto_crop_codecs::encoders::{TiffCompression, TiffEncoder, WebpEncoder};
use auto_crop_codecs::fixtures::{pattern, pattern_gray, photo};
use auto_crop_core::output::{Bilevel, Bits, Quality};
use auto_crop_core::ports::{EncodeMeta, Encoder, PixelFormat, Raster, Samples};
use auto_crop_core::{CancelToken, OutputSpec};
use std::fs;
use std::path::Path;

fn write(dir: &Path, name: &str, bytes: &[u8]) {
    fs::write(dir.join(name), bytes).expect("write sample");
    println!("{name}\t{}", bytes.len());
}

fn png_of_rgb(w: u32, h: u32, rgb: Vec<u8>) -> Vec<u8> {
    auto_crop_codecs::encode(
        &auto_crop_imgproc::Raster::from_raw(w, h, rgb).unwrap(),
        auto_crop_codecs::Format::Png,
        0,
        None,
    )
    .unwrap()
}

fn main() {
    let dir = std::env::args().nth(1).expect("usage: write_samples <dir>");
    let dir = Path::new(&dir);
    fs::create_dir_all(dir).unwrap();
    let never = CancelToken::never();
    let (w, h) = (251u32, 173u32);
    let rgb = pattern(w, h);
    let grey = pattern_gray(w, h);
    let icc_free = EncodeMeta {
        icc: None,
        dpi: Some((300, 300)),
    };

    // References.
    write(dir, "ref_rgb.png", &png_of_rgb(w, h, rgb.clone()));
    write(
        dir,
        "ref_gray.png",
        &png_of_rgb(w, h, grey.iter().flat_map(|v| [*v; 3]).collect()),
    );

    let rgb8 = Raster::from_samples(w, h, PixelFormat::Rgb8, Samples::U8(rgb.clone())).unwrap();
    let gray8 = Raster::from_samples(w, h, PixelFormat::Gray8, Samples::U8(grey.clone())).unwrap();
    for (name, compression) in [
        ("none", TiffCompression::None),
        ("lzw", TiffCompression::Lzw),
        ("deflate", TiffCompression::Deflate),
        ("packbits", TiffCompression::PackBits),
    ] {
        let enc = TiffEncoder {
            compression,
            rows_per_strip: Some(32),
            ..TiffEncoder::default()
        };
        for (kind, r) in [("rgb", &rgb8), ("gray", &gray8)] {
            let bytes = enc
                .encode(&r.view(), &OutputSpec::default(), &icc_free, &never)
                .unwrap();
            write(dir, &format!("tiff_{kind}_{name}.tif"), &bytes);
        }
    }

    // A bilevel page: the thresholded pattern, as a G4 TIFF and a G4 PDF, with its reference.
    let b = BilevelImage::from_view(&gray8.view(), DEFAULT_CUT, &never).unwrap();
    let mut ref_bits = Vec::with_capacity((w * h) as usize * 3);
    for y in 0..h {
        for x in 0..w {
            ref_bits.extend_from_slice(&[if b.is_black(x, y) { 0 } else { 255 }; 3]);
        }
    }
    write(dir, "ref_bilevel.png", &png_of_rgb(w, h, ref_bits));
    let g4 = OutputSpec {
        bits: Bits::One {
            bilevel: Bilevel::TiffG4,
        },
        ..OutputSpec::default()
    };
    for (name, rps) in [("onestrip", None), ("strips", Some(16))] {
        let enc = TiffEncoder {
            rows_per_strip: rps,
            ..TiffEncoder::default()
        };
        let meta = EncodeMeta {
            icc: None,
            dpi: Some((600, 600)),
        };
        write(
            dir,
            &format!("tiff_g4_{name}.tif"),
            &enc.encode(&gray8.view(), &g4, &meta, &never).unwrap(),
        );
    }
    write(
        dir,
        "pdf_g4.pdf",
        &PdfEncoder::default()
            .encode(&gray8.view(), &g4, &icc_free, &never)
            .unwrap(),
    );

    // WebP, lossless.
    write(
        dir,
        "webp_rgb.webp",
        &WebpEncoder::default()
            .encode(&rgb8.view(), &OutputSpec::default(), &icc_free, &never)
            .unwrap(),
    );
    write(
        dir,
        "webp_gray.webp",
        &WebpEncoder::default()
            .encode(&gray8.view(), &OutputSpec::default(), &icc_free, &never)
            .unwrap(),
    );

    // PDF: a JPEG page (lossy, so no pixel reference), a Flate page, and the three together.
    let lossless = OutputSpec {
        quality: Quality::Lossless,
        ..OutputSpec::default()
    };
    write(
        dir,
        "pdf_flate_rgb.pdf",
        &PdfEncoder::default()
            .encode(&rgb8.view(), &lossless, &icc_free, &never)
            .unwrap(),
    );
    write(
        dir,
        "pdf_flate_gray.pdf",
        &PdfEncoder::default()
            .encode(&gray8.view(), &lossless, &icc_free, &never)
            .unwrap(),
    );
    let photo_px = photo(w, h, 11);
    let photo_r = Raster::from_samples(w, h, PixelFormat::Rgb8, Samples::U8(photo_px)).unwrap();
    write(
        dir,
        "pdf_jpeg.pdf",
        &PdfEncoder::default()
            .encode(&photo_r.view(), &OutputSpec::default(), &icc_free, &never)
            .unwrap(),
    );
    let mut doc = PdfWriter::new(Vec::new()).unwrap();
    doc.set_title("Auto Crop interop sample");
    let jpeg = auto_crop_codecs::encoders::default_jpeg_encoder()
        .encode(&photo_r.view(), &OutputSpec::default(), &icc_free, &never)
        .unwrap();
    doc.add_page(
        PageImage::Jpeg {
            bytes: &jpeg,
            icc: None,
        },
        Some((300, 300)),
    )
    .unwrap();
    doc.add_page(
        PageImage::Flate {
            width: w,
            height: h,
            channels: 3,
            samples: &rgb,
            icc: None,
        },
        Some((150, 150)),
    )
    .unwrap();
    doc.add_page(PageImage::G4(&b), Some((600, 600))).unwrap();
    // A page of unknown dpi: nominal 300.
    doc.add_page(
        PageImage::Flate {
            width: w,
            height: h,
            channels: 1,
            samples: &grey,
            icc: None,
        },
        None,
    )
    .unwrap();
    write(dir, "pdf_multi.pdf", &doc.finish().unwrap());

    #[cfg(feature = "avif")]
    write_avif_samples(dir);
}

/// AVIF (the `avif` feature): colour in both chroma layouts, grey, and one with a profile and EXIF,
/// each with the exact picture it must come back as (within the bar of
/// `tools/interop/check-avif.py`). `avif_icc.bin` and `avif_exif.bin` are the bytes that must be
/// found in `avif_meta.avif` again.
#[cfg(feature = "avif")]
fn write_avif_samples(dir: &Path) {
    use auto_crop_codecs::encoders::{AvifChroma, AvifEncoder};
    use auto_crop_codecs::fixtures::{exif_blob, fake_icc};
    use std::sync::Arc;
    let never = CancelToken::never();
    let (w, h) = (251u32, 173u32);
    let spec = OutputSpec {
        quality: Quality::Fixed { value: 80 },
        ..OutputSpec::default()
    };
    let none = EncodeMeta::default();
    let px = photo(w, h, 11);
    write(dir, "ref_photo.png", &png_of_rgb(w, h, px.clone()));
    let rgb = Raster::from_samples(w, h, PixelFormat::Rgb8, Samples::U8(px)).unwrap();
    for (name, chroma) in [("444", AvifChroma::Yuv444), ("420", AvifChroma::Yuv420)] {
        let enc = AvifEncoder {
            chroma,
            ..AvifEncoder::default()
        };
        write(
            dir,
            &format!("avif_rgb{name}.avif"),
            &enc.encode(&rgb.view(), &spec, &none, &never).unwrap(),
        );
    }
    let grey = pattern_gray(w, h);
    write(
        dir,
        "ref_photo_gray.png",
        &png_of_rgb(w, h, grey.iter().flat_map(|v| [*v; 3]).collect()),
    );
    let gray8 = Raster::from_samples(w, h, PixelFormat::Gray8, Samples::U8(grey)).unwrap();
    write(
        dir,
        "avif_gray.avif",
        &AvifEncoder::default()
            .encode(&gray8.view(), &spec, &none, &never)
            .unwrap(),
    );
    let (icc, exif) = (fake_icc(1500), exif_blob(1, true));
    fs::write(dir.join("avif_icc.bin"), &icc).unwrap();
    fs::write(dir.join("avif_exif.bin"), &exif).unwrap();
    let meta = EncodeMeta {
        icc: Some(Arc::from(icc.as_slice())),
        dpi: None,
    };
    let enc = AvifEncoder {
        exif: Some(exif),
        ..AvifEncoder::default()
    };
    write(
        dir,
        "avif_meta.avif",
        &enc.encode(&rgb.view(), &spec, &meta, &never).unwrap(),
    );
}
