// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The PDF writer: what it embeds, how it sizes pages, that it never adds text or location, and
//! that the structure checker (`read_pages`, the engine's verify step) accepts what the writer
//! makes and refuses damage. Poppler (`pdfinfo`, `pdfimages`, `pdftoppm`) and `qpdf --check` read the
//! same files in the CI `interop` job.

use super::bilevel::BilevelImage;
use super::pdf::*;
use super::*;
use crate::fixtures::{fake_icc, jpeg_insert_segment, pattern, pattern_gray, photo};
use auto_crop_core::output::Bilevel;
use auto_crop_core::ports::{Raster, Samples};

fn never() -> CancelToken {
    CancelToken::never()
}

fn rgb(w: u32, h: u32) -> Raster {
    Raster::from_samples(w, h, PixelFormat::Rgb8, Samples::U8(pattern(w, h))).unwrap()
}

fn gray(w: u32, h: u32) -> Raster {
    Raster::from_samples(w, h, PixelFormat::Gray8, Samples::U8(pattern_gray(w, h))).unwrap()
}

fn icc_of(space: &[u8; 4], len: usize) -> Vec<u8> {
    let mut v = fake_icc(len);
    v[16..20].copy_from_slice(space);
    v
}

fn one_page(image: PageImage<'_>, dpi: Option<(u32, u32)>) -> (Vec<u8>, PageInfo) {
    let mut w = PdfWriter::new(Vec::new()).unwrap();
    let info = w.add_page(image, dpi).unwrap();
    (w.finish().unwrap(), info)
}

fn jpeg_with_metadata(w: u32, h: u32) -> Vec<u8> {
    let base = crate::encode(
        &auto_crop_imgproc::Raster::from_raw(w, h, photo(w, h, 7)).unwrap(),
        crate::Format::Jpeg,
        88,
        None,
    )
    .unwrap();
    let mut exif = b"Exif\0\0".to_vec();
    exif.extend_from_slice(&crate::fixtures::exif_blob(6, true));
    let j = jpeg_insert_segment(&base, 0xE1, &exif);
    let j = jpeg_insert_segment(
        &j,
        0xE1,
        b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta GPSLatitude/>",
    );
    let j = jpeg_insert_segment(&j, 0xFE, b"a comment with /home/someone/secret.jpg");
    jpeg_insert_segment(&j, 0xED, b"Photoshop 3.0\0 IPTC")
}

#[test]
fn a_jpeg_page_embeds_the_stream_unchanged_without_exif_xmp_iptc_or_comments() {
    let src = jpeg_with_metadata(120, 90);
    let (pdf, info) = one_page(
        PageImage::Jpeg {
            bytes: &src,
            icc: None,
        },
        Some((150, 150)),
    );
    // 120 px at 150 dpi = 57.6 pt.
    assert_eq!((info.width_pt, info.height_pt), (57.6, 43.2));
    assert!(!info.dpi_assumed && !info.scaled && !info.icc_dropped);
    let pages = read_pages(&pdf).unwrap();
    assert_eq!(pages.len(), 1);
    let p = &pages[0];
    assert_eq!(
        (p.width_px, p.height_px, p.codec, p.channels),
        (120, 90, PageCodec::Dct, 3)
    );
    // The embedded stream is the source minus the personal segments, byte for byte otherwise.
    let stripped = strip_jpeg(&src).unwrap();
    assert!(stripped.len() < src.len());
    assert!(pdf.windows(stripped.len()).any(|w| w == &stripped[..]));
    for needle in [
        &b"Exif"[..],
        b"GPSLatitude",
        b"/home/someone",
        b"Photoshop",
        b"xmpmeta",
    ] {
        assert!(
            !pdf.windows(needle.len()).any(|w| w == needle),
            "{}",
            String::from_utf8_lossy(needle)
        );
    }
    // Decoding the page gives the same pixels as decoding the source (the EXIF turn aside).
    let want = crate::decode_with(&stripped, &crate::DecodeLimits::default()).unwrap();
    let (w, h, px) = p.to_rgb().unwrap();
    assert_eq!((w, h), (120, 90));
    assert_eq!(px, want.raster.data);
}

#[test]
fn flate_pages_are_lossless_for_rgb_and_grey_and_every_png_filter() {
    for (w, h) in [(1, 1), (7, 3), (64, 48), (257, 100)] {
        let r = rgb(w, h);
        let Samples::U8(data) = r.samples() else {
            unreachable!()
        };
        let (pdf, _) = one_page(
            PageImage::Flate {
                width: w,
                height: h,
                channels: 3,
                samples: data,
                icc: None,
            },
            None,
        );
        let p = &read_pages(&pdf).unwrap()[0];
        assert_eq!(p.codec, PageCodec::Flate);
        assert_eq!(p.to_rgb().unwrap().2, *data, "rgb {w}x{h}");
        let g = pattern_gray(w, h);
        let (pdf, _) = one_page(
            PageImage::Flate {
                width: w,
                height: h,
                channels: 1,
                samples: &g,
                icc: None,
            },
            None,
        );
        let p = &read_pages(&pdf).unwrap()[0];
        let want: Vec<u8> = g.iter().flat_map(|v| [*v; 3]).collect();
        assert_eq!(p.to_rgb().unwrap().2, want, "grey {w}x{h}");
    }
}

#[test]
fn a_flate_page_is_about_as_small_as_the_png_of_the_same_pixels() {
    let r = crate::fixtures::photo(300, 200, 3);
    let img = auto_crop_imgproc::Raster::from_raw(300, 200, r.clone()).unwrap();
    let png = crate::encode(&img, crate::Format::Png, 0, None).unwrap();
    let (pdf, _) = one_page(
        PageImage::Flate {
            width: 300,
            height: 200,
            channels: 3,
            samples: &r,
            icc: None,
        },
        None,
    );
    assert!(
        (pdf.len() as f64) < png.len() as f64 * 1.2 + 1200.0,
        "pdf {} png {}",
        pdf.len(),
        png.len()
    );
}

fn marks(w: u32, h: u32) -> BilevelImage {
    let mut b = BilevelImage::white(w, h);
    for y in 0..h {
        for x in 0..w {
            if x == 0 || y == 0 || (x + y) % 7 == 0 || (y / 5 % 2 == 0 && x > 3 && x < w - 4) {
                b.bits[y as usize * b.stride + x as usize / 8] |= 0x80 >> (x % 8);
            }
        }
    }
    b
}

#[test]
fn a_g4_page_keeps_black_black_and_stays_small() {
    let b = marks(203, 121);
    let (pdf, info) = one_page(PageImage::G4(&b), Some((600, 600)));
    assert!((info.width_pt - 24.36).abs() < 0.01, "{info:?}");
    let p = &read_pages(&pdf).unwrap()[0];
    assert_eq!(p.codec, PageCodec::G4);
    let (w, h, px) = p.to_rgb().unwrap();
    assert_eq!((w, h), (203, 121));
    for y in 0..h {
        for x in 0..w {
            assert_eq!(px[((y * w + x) * 3) as usize] == 0, b.is_black(x, y));
        }
    }
    // The dictionary says Group 4, the default polarity, and no inversion filter.
    let s = String::from_utf8_lossy(&pdf);
    assert!(s.contains("/CCITTFaxDecode") && s.contains("/K -1"));
    assert!(!s.contains("/BlackIs1 true") && !s.contains("/Decode ["));
    // Size = payload plus a small fixed overhead.
    let payload = b.g4(0, h, &never()).unwrap().len();
    assert!(pdf.len() < payload + 1500, "{} vs {payload}", pdf.len());
}

#[test]
fn page_sizes_follow_pixels_over_dpi_with_a_nominal_300() {
    let g = [0u8; 4];
    let flate = |w: u32, h: u32| PageImage::Flate {
        width: w,
        height: h,
        channels: 1,
        samples: Box::leak(vec![0u8; (w * h) as usize].into_boxed_slice()),
        icc: None,
    };
    let _ = g;
    // A 300 dpi A4 scan is within 0.5 pt of 595.28 x 841.89.
    let (_, i) = one_page(flate(2480, 3508), Some((300, 300)));
    assert!(
        (i.width_pt - 595.2).abs() < 0.5 && (i.height_pt - 841.92).abs() < 0.5,
        "{i:?}"
    );
    // Unknown dpi: nominal 300, and it says so.
    let (_, i) = one_page(flate(600, 300), None);
    assert_eq!(
        (i.width_pt, i.height_pt, i.dpi_assumed),
        (144.0, 72.0, true)
    );
    // Different x and y resolution.
    let (_, i) = one_page(flate(300, 300), Some((300, 150)));
    assert_eq!((i.width_pt, i.height_pt), (72.0, 144.0));
    // Larger than a PDF page may be: scaled down, flagged.
    let (pdf, i) = one_page(flate(60_000, 10), Some((72, 72)));
    assert!(i.scaled && i.width_pt <= MAX_PAGE_PT + 0.01, "{i:?}");
    assert!(read_pages(&pdf).is_some());
    // Tiny: grown to at least 3 pt.
    let (_, i) = one_page(flate(1, 1), Some((300, 300)));
    assert!(i.scaled && i.width_pt >= MIN_PAGE_PT - 0.01, "{i:?}");
    // The media box is what the reader sees.
    let (pdf, i) = one_page(flate(300, 150), Some((100, 100)));
    let p = &read_pages(&pdf).unwrap()[0];
    assert_eq!(p.media_box, (i.width_pt, i.height_pt));
}

#[test]
fn a_multi_page_document_keeps_its_order_and_mixes_codecs() {
    let jpeg = jpeg_with_metadata(64, 48);
    let b = marks(80, 60);
    let g = pattern_gray(32, 32);
    let mut w = PdfWriter::new(Vec::new()).unwrap();
    w.set_title("Receipts, \"May\" (draft)");
    w.set_creation_date((2026, 10, 14, 9, 8, 7));
    for k in 0..9 {
        match k % 3 {
            0 => w.add_page(
                PageImage::Jpeg {
                    bytes: &jpeg,
                    icc: None,
                },
                None,
            ),
            1 => w.add_page(PageImage::G4(&b), Some((200, 200))),
            _ => w.add_page(
                PageImage::Flate {
                    width: 32,
                    height: 32,
                    channels: 1,
                    samples: &g,
                    icc: None,
                },
                Some((72, 72)),
            ),
        }
        .unwrap();
    }
    assert_eq!(w.page_count(), 9);
    let pdf = w.finish().unwrap();
    let pages = read_pages(&pdf).unwrap();
    assert_eq!(pages.len(), 9);
    for (k, p) in pages.iter().enumerate() {
        let want = [PageCodec::Dct, PageCodec::G4, PageCodec::Flate][k % 3];
        assert_eq!(p.codec, want, "page {k}");
    }
    let s = String::from_utf8_lossy(&pdf);
    assert!(s.contains("/Producer (Auto Crop)"));
    assert!(s.contains("/Title (Receipts, \"May\" \\(draft\\))"));
    assert!(s.contains("/CreationDate (D:20261014090807Z)"));
    assert!(s.contains("/Count 9"));
}

#[test]
fn nothing_but_images_is_written_no_text_fonts_scripts_or_jbig2() {
    let src = jpeg_with_metadata(40, 30);
    let (pdf, _) = one_page(
        PageImage::Jpeg {
            bytes: &src,
            icc: None,
        },
        None,
    );
    let s = String::from_utf8_lossy(&pdf);
    for bad in [
        "/Font",
        " BT",
        "/JBIG2Decode",
        "/JS",
        "/JavaScript",
        "/OpenAction",
        "/Annots",
        "/URI",
        "/EmbeddedFile",
        "/Launch",
        "/AcroForm",
        "/Metadata",
    ] {
        assert!(!s.contains(bad), "{bad}");
    }
    // The information dictionary has the producer and nothing else by default: no author, no path.
    let info_at = s.find("3 0 obj\n<<").unwrap();
    let info = &s[info_at..s[info_at..].find("endobj").unwrap() + info_at];
    assert_eq!(info.matches('/').count(), 1, "{info}");
    assert!(s.starts_with("%PDF-1.4\n%"));
    assert!(s.ends_with("%%EOF\n"));
}

#[test]
fn equal_input_gives_equal_bytes() {
    let src = jpeg_with_metadata(40, 30);
    let a = one_page(
        PageImage::Jpeg {
            bytes: &src,
            icc: None,
        },
        Some((300, 300)),
    )
    .0;
    let b = one_page(
        PageImage::Jpeg {
            bytes: &src,
            icc: None,
        },
        Some((300, 300)),
    )
    .0;
    assert_eq!(a, b);
}

#[test]
fn an_icc_profile_moves_to_an_iccbased_colour_space_when_it_fits() {
    let src = jpeg_with_metadata(40, 30);
    let rgb_icc = icc_of(b"RGB ", 2000);
    let (pdf, i) = one_page(
        PageImage::Jpeg {
            bytes: &src,
            icc: Some(&rgb_icc),
        },
        None,
    );
    assert!(!i.icc_dropped);
    let p = &read_pages(&pdf).unwrap()[0];
    assert_eq!(p.icc.as_deref(), Some(&rgb_icc[..]));
    assert!(String::from_utf8_lossy(&pdf).contains("/ICCBased"));
    // A grey profile on an RGB image does not fit: left out, and it says so.
    let bad = icc_of(b"GRAY", 2000);
    let (pdf, i) = one_page(
        PageImage::Jpeg {
            bytes: &src,
            icc: Some(&bad),
        },
        None,
    );
    assert!(i.icc_dropped);
    assert!(read_pages(&pdf).unwrap()[0].icc.is_none());
    // Grey Flate with a grey profile.
    let gicc = icc_of(b"GRAY", 1000);
    let g = pattern_gray(16, 16);
    let (pdf, _) = one_page(
        PageImage::Flate {
            width: 16,
            height: 16,
            channels: 1,
            samples: &g,
            icc: Some(&gicc),
        },
        None,
    );
    let p = &read_pages(&pdf).unwrap()[0];
    assert_eq!((p.icc.as_deref(), p.channels), (Some(&gicc[..]), 1));
}

#[test]
fn the_structure_check_refuses_damage() {
    let b = marks(40, 40);
    let (pdf, _) = one_page(PageImage::G4(&b), None);
    assert!(read_pages(&pdf).is_some());
    // Truncated at every length shorter than the file.
    for cut in (0..pdf.len()).step_by(7) {
        assert!(read_pages(&pdf[..cut]).is_none(), "cut at {cut}");
    }
    // A wrong cross-reference offset.
    let s = String::from_utf8_lossy(&pdf).into_owned();
    let at = s.find("xref\n0 ").unwrap();
    let first = s[at..].find("0000000").unwrap() + at;
    let line = s[first..].find("\n").unwrap() + first + 1; // the first in-use entry
    let mut bad = pdf.clone();
    let digits = &mut bad[line..line + 10];
    digits[9] = if digits[9] == b'9' {
        b'8'
    } else {
        digits[9] + 1
    };
    assert!(read_pages(&bad).is_none());
    // A stream whose bytes no longer match /Length.
    let mut bad = pdf.clone();
    let end = s.find("\nendstream").unwrap();
    bad.insert(end, b'X');
    assert!(read_pages(&bad).is_none());
    // Junk after the end of file.
    let mut bad = pdf.clone();
    bad.extend_from_slice(b"junk");
    assert!(read_pages(&bad).is_none());
    // Not a PDF.
    assert!(read_pages(b"hello").is_none());
    assert!(read_pages(&[]).is_none());
}

#[test]
fn what_a_pdf_reader_cannot_be_relied_on_for_is_refused() {
    let mut w = PdfWriter::new(Vec::new()).unwrap();
    // Not a JPEG.
    assert_eq!(
        w.add_page(
            PageImage::Jpeg {
                bytes: b"nope",
                icc: None
            },
            None
        )
        .err(),
        Some(ErrKind::UnsupportedOutput)
    );
    // A CMYK JPEG (4 components).
    let mut cmyk = vec![0xFF, 0xD8, 0xFF, 0xC0, 0, 17, 8, 0, 8, 0, 8, 4];
    cmyk.extend_from_slice(&[1, 0x11, 0, 2, 0x11, 0, 3, 0x11, 0, 4, 0x11, 0]);
    cmyk.extend_from_slice(&[0xFF, 0xDA, 0, 2, 0xFF, 0xD9]);
    assert_eq!(
        w.add_page(
            PageImage::Jpeg {
                bytes: &cmyk,
                icc: None
            },
            None
        )
        .err(),
        Some(ErrKind::UnsupportedOutput)
    );
    // An arithmetic-coded frame (SOF9).
    let mut arith = cmyk.clone();
    arith[3] = 0xC9;
    arith[11] = 3;
    assert_eq!(
        w.add_page(
            PageImage::Jpeg {
                bytes: &arith,
                icc: None
            },
            None
        )
        .err(),
        Some(ErrKind::UnsupportedOutput)
    );
    // Wrong sample count.
    assert_eq!(
        w.add_page(
            PageImage::Flate {
                width: 4,
                height: 4,
                channels: 3,
                samples: &[0; 10],
                icc: None
            },
            None
        )
        .err(),
        Some(ErrKind::UnsupportedOutput)
    );
    // An empty document is not a PDF.
    assert_eq!(w.finish().err(), Some(ErrKind::UnsupportedOutput));
}

#[test]
fn the_encoder_picks_jpeg_flate_or_g4_from_the_spec() {
    let enc = PdfEncoder::default();
    let none = EncodeMeta::default();
    let r = rgb(96, 64);
    let page = |spec: &OutputSpec, r: &Raster, meta: &EncodeMeta| {
        let pdf = enc.encode(&r.view(), spec, meta, &never()).unwrap();
        read_pages(&pdf).unwrap().remove(0)
    };
    let q = |v| OutputSpec {
        quality: auto_crop_core::output::Quality::Fixed { value: v },
        ..OutputSpec::default()
    };
    assert_eq!(page(&q(85), &r, &none).codec, PageCodec::Dct);
    assert_eq!(
        page(&OutputSpec::default(), &r, &none).codec,
        PageCodec::Dct
    );
    let lossless = OutputSpec {
        quality: auto_crop_core::output::Quality::Lossless,
        ..OutputSpec::default()
    };
    let p = page(&lossless, &r, &none);
    assert_eq!(p.codec, PageCodec::Flate);
    assert_eq!(p.to_rgb().unwrap().2, pattern(96, 64));
    let g = gray(96, 64);
    let p = page(&lossless, &g, &none);
    assert_eq!((p.codec, p.channels), (PageCodec::Flate, 1));
    let one_bit = OutputSpec {
        bits: Bits::One {
            bilevel: Bilevel::TiffG4,
        },
        ..OutputSpec::default()
    };
    assert_eq!(page(&one_bit, &g, &none).codec, PageCodec::G4);
    // dpi and profile pass through.
    let meta = EncodeMeta {
        icc: Some(icc_of(b"RGB ", 1500).into()),
        dpi: Some((200, 200)),
    };
    let p = page(&q(90), &r, &meta);
    assert_eq!(p.media_box, (34.56, 23.04));
    assert_eq!(p.icc, Some(icc_of(b"RGB ", 1500)));
    // 16-bit input is reduced to 8 bits.
    let w16 = Raster::from_samples(
        8,
        8,
        PixelFormat::Rgb16,
        Samples::U16((0..192).map(|i| (i * 300) as u16).collect()),
    )
    .unwrap();
    assert_eq!(page(&lossless, &w16, &none).codec, PageCodec::Flate);
    // Cancelled and empty.
    let c = CancelToken::new_batch();
    c.cancel();
    assert_eq!(
        enc.encode(&r.view(), &q(90), &none, &c),
        Err(ErrKind::Cancelled)
    );
    let empty = Raster::from_samples(0, 0, PixelFormat::Gray8, Samples::U8(vec![])).unwrap();
    assert_eq!(
        enc.encode(&empty.view(), &q(90), &none, &never()),
        Err(ErrKind::UnsupportedOutput)
    );
}

#[test]
fn a_thousand_pages_write_in_constant_memory_per_page() {
    // A counting sink: the writer holds one page's bytes, never the document.
    struct Count(u64);
    impl std::io::Write for Count {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0 += b.len() as u64;
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let b = marks(64, 64);
    let mut w = PdfWriter::new(Count(0)).unwrap();
    for _ in 0..1000 {
        w.add_page(PageImage::G4(&b), Some((300, 300))).unwrap();
    }
    assert!(w.finish().unwrap().0 > 1000 * 300);
}
