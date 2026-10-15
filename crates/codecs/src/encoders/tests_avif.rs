// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The AVIF container (always built) and, with the `avif` feature, the writer. The decode side of
//! the picture is checked in the `heif` workflow (libheif and dav1d) and in `output-formats.yml`
//! (Pillow, ImageMagick); these tests check what no decoder reports: box structure, ICC and EXIF
//! bytes, damage handling.

use super::avif_box::{Picture, exif_item_payload, read_info, write};

fn tiff_blob() -> Vec<u8> {
    crate::fixtures::exif_blob(1, true)
}

fn av1_stub() -> Vec<u8> {
    // A temporal delimiter OBU then a sequence-header-shaped OBU; the container does not parse AV1.
    let mut v = vec![0x12, 0x00, 0x0A, 0x04, 1, 2, 3, 4];
    v.extend(std::iter::repeat_n(0x32u8, 500));
    v
}

fn av1c() -> Vec<u8> {
    vec![0x81, 0x00, 0x0C, 0x00]
}

fn picture<'a>(
    data: &'a [u8],
    c: &'a [u8],
    icc: Option<&'a [u8]>,
    exif: Option<&'a [u8]>,
) -> Picture<'a> {
    Picture {
        width: 123,
        height: 45,
        monochrome: false,
        subsampled: true,
        av1c: c,
        av1_data: data,
        icc,
        nclx_unspecified: icc.is_some(),
        exif,
    }
}

#[test]
fn the_container_reads_back_what_was_written() {
    let (data, c) = (av1_stub(), av1c());
    let icc = crate::fixtures::fake_icc(3000);
    let payload = exif_item_payload(&tiff_blob()).unwrap();
    let file = write(&picture(&data, &c, Some(&icc), Some(&payload)));
    assert_eq!(&file[4..12], b"ftypavif");
    let info = read_info(&file).unwrap();
    assert_eq!((info.width, info.height), (123, 45));
    assert_eq!(info.bits, vec![8, 8, 8]);
    assert!(!info.monochrome && info.subsampled_x && info.subsampled_y);
    assert_eq!(info.icc.as_deref(), Some(icc.as_slice()), "ICC byte-exact");
    assert_eq!(info.exif.as_deref(), Some(tiff_blob().as_slice()));
    assert!(!info.has_irot_or_imir);
    assert_eq!(info.av1_len, data.len());
    assert!(info.av1_header_ok);
    let nclx = info.nclx.unwrap();
    assert_eq!(
        (nclx.primaries, nclx.transfer, nclx.matrix, nclx.full_range),
        (2, 2, 6, true),
        "with a profile the nclx claims only the matrix"
    );
}

#[test]
fn without_a_profile_the_colour_is_srgb_and_nothing_extra_is_written() {
    let (data, c) = (av1_stub(), av1c());
    let file = write(&picture(&data, &c, None, None));
    let info = read_info(&file).unwrap();
    assert!(info.icc.is_none() && info.exif.is_none());
    let nclx = info.nclx.unwrap();
    assert_eq!((nclx.primaries, nclx.transfer, nclx.matrix), (1, 13, 6));
    // The picture data is the tail of the file, untouched.
    assert!(file.ends_with(&data));
    // No irot, imir, clap or alpha property anywhere.
    for tag in [&b"irot"[..], b"imir", b"clap", b"auxC"] {
        assert!(!file.windows(4).any(|w| w == tag), "{tag:?}");
    }
}

#[test]
fn a_monochrome_picture_has_one_channel() {
    let (data, c) = (av1_stub(), av1c());
    let mut p = picture(&data, &c, None, None);
    p.monochrome = true;
    let c2 = [0x81, 0x00, 0x1C, 0x00];
    p.av1c = &c2;
    let info = read_info(&write(&p)).unwrap();
    assert_eq!(info.bits, vec![8]);
    assert!(info.monochrome);
}

#[test]
fn exif_payloads_need_a_tiff_header() {
    let blob = tiff_blob();
    let p = exif_item_payload(&blob).unwrap();
    assert_eq!(&p[..4], &[0, 0, 0, 0]);
    assert_eq!(&p[4..], blob.as_slice());
    let mut prefixed = b"Exif\0\0".to_vec();
    prefixed.extend_from_slice(&blob);
    assert_eq!(exif_item_payload(&prefixed).unwrap(), p);
    assert!(exif_item_payload(b"not exif at all").is_none());
    assert!(exif_item_payload(b"").is_none());
}

#[test]
fn damaged_files_are_refused_or_read_without_panicking() {
    let (data, c) = (av1_stub(), av1c());
    let payload = exif_item_payload(&tiff_blob()).unwrap();
    let icc = crate::fixtures::fake_icc(300);
    let file = write(&picture(&data, &c, Some(&icc), Some(&payload)));
    // Every truncation is refused.
    for cut in 0..file.len() {
        let r = read_info(&file[..cut]);
        assert!(r.is_none(), "cut at {cut} of {}", file.len());
    }
    // A flipped byte anywhere never panics.
    for i in 0..file.len().min(400) {
        let mut f = file.clone();
        f[i] ^= 0xFF;
        let _ = read_info(&f);
    }
}

#[cfg(feature = "avif")]
mod encoder {
    use super::super::{AvifChroma, AvifEncoder, avif};
    use super::*;
    use crate::fixtures::{fake_icc, photo};
    use auto_crop_core::output::{Bits, Quality};
    use auto_crop_core::ports::{EncodeMeta, Encoder, PixelFormat, Raster, Samples};
    use auto_crop_core::{CancelToken, ErrKind, OutputSpec};
    use std::sync::Arc;

    fn spec(q: u8) -> OutputSpec {
        OutputSpec {
            quality: Quality::Fixed { value: q },
            ..OutputSpec::default()
        }
    }

    fn rgb(w: u32, h: u32) -> Raster {
        Raster::from_samples(w, h, PixelFormat::Rgb8, Samples::U8(photo(w, h, 7))).unwrap()
    }

    fn fast() -> AvifEncoder {
        AvifEncoder {
            speed: 10,
            threads: Some(2),
            ..AvifEncoder::default()
        }
    }

    #[test]
    fn the_quantiser_falls_as_the_quality_rises() {
        let q: Vec<u8> = (1..=100).map(avif::quality_to_quantizer).collect();
        assert!(q.windows(2).all(|w| w[0] >= w[1]), "{q:?}");
        assert_eq!(q[99], 0);
        assert_eq!(avif::quality_to_quantizer(75), 128);
        assert_eq!(avif::quality_to_quantizer(90), 66);
        assert_eq!(avif::quality_to_quantizer(0), avif::quality_to_quantizer(1));
    }

    #[test]
    fn colour_pictures_are_written_in_both_chroma_layouts() {
        let src = rgb(67, 45);
        for (chroma, sub) in [(AvifChroma::Yuv420, true), (AvifChroma::Yuv444, false)] {
            let enc = AvifEncoder { chroma, ..fast() };
            let out = enc
                .encode(
                    &src.view(),
                    &spec(80),
                    &EncodeMeta::default(),
                    &CancelToken::never(),
                )
                .unwrap();
            assert_eq!(crate::format::sniff(&out), Some(crate::Format::Avif));
            let info = read_info(&out).unwrap();
            assert_eq!((info.width, info.height), (67, 45), "odd sizes stay exact");
            assert_eq!((info.subsampled_x, info.subsampled_y), (sub, sub));
            assert_eq!(info.bits, vec![8, 8, 8]);
            assert!(!info.has_irot_or_imir);
        }
    }

    #[test]
    fn a_grey_picture_is_written_as_one_plane() {
        let g =
            Raster::from_samples(40, 30, PixelFormat::Gray8, Samples::U8(vec![90; 1200])).unwrap();
        let out = fast()
            .encode(
                &g.view(),
                &spec(80),
                &EncodeMeta::default(),
                &CancelToken::never(),
            )
            .unwrap();
        let info = read_info(&out).unwrap();
        assert!(info.monochrome);
        assert_eq!(info.bits, vec![8]);
    }

    #[test]
    fn the_profile_and_the_exif_are_embedded_byte_exact() {
        let src = rgb(48, 32);
        let icc = fake_icc(2048);
        let meta = EncodeMeta {
            icc: Some(Arc::from(icc.as_slice())),
            dpi: Some((300, 300)),
        };
        let enc = AvifEncoder {
            exif: Some(tiff_blob()),
            ..fast()
        };
        let out = enc
            .encode(&src.view(), &spec(80), &meta, &CancelToken::never())
            .unwrap();
        let info = read_info(&out).unwrap();
        assert_eq!(info.icc.as_deref(), Some(icc.as_slice()));
        assert_eq!(info.exif.as_deref(), Some(tiff_blob().as_slice()));
        // A blob that is not TIFF is dropped, not written.
        let bad = AvifEncoder {
            exif: Some(b"junk".to_vec()),
            ..fast()
        };
        let out = bad
            .encode(
                &src.view(),
                &spec(80),
                &EncodeMeta::default(),
                &CancelToken::never(),
            )
            .unwrap();
        assert!(read_info(&out).unwrap().exif.is_none());
    }

    #[test]
    fn higher_quality_is_larger_and_match_source_uses_the_default() {
        let src = rgb(96, 64);
        let enc = fast();
        let size = |s: &OutputSpec| {
            enc.encode(
                &src.view(),
                s,
                &EncodeMeta::default(),
                &CancelToken::never(),
            )
            .unwrap()
            .len()
        };
        assert!(size(&spec(30)) < size(&spec(60)));
        assert!(size(&spec(60)) < size(&spec(95)));
        let ms = OutputSpec {
            quality: Quality::MatchSource { floor: 40, cap: 90 },
            ..OutputSpec::default()
        };
        assert_eq!(size(&ms), size(&spec(75)));
    }

    #[test]
    fn what_avif_cannot_write_is_refused() {
        let src = rgb(16, 16);
        let meta = EncodeMeta::default();
        let never = CancelToken::never();
        let lossless = OutputSpec {
            quality: Quality::Lossless,
            ..OutputSpec::default()
        };
        assert_eq!(
            fast().encode(&src.view(), &lossless, &meta, &never),
            Err(ErrKind::UnsupportedOutput)
        );
        let one_bit = OutputSpec {
            bits: Bits::One {
                bilevel: auto_crop_core::output::Bilevel::Png,
            },
            ..spec(80)
        };
        assert_eq!(
            fast().encode(&src.view(), &one_bit, &meta, &never),
            Err(ErrKind::UnsupportedOutput)
        );
    }

    #[test]
    fn a_cancelled_encode_returns_at_once() {
        let src = rgb(64, 64);
        let t = CancelToken::new_batch();
        t.cancel();
        assert_eq!(
            fast().encode(&src.view(), &spec(80), &EncodeMeta::default(), &t),
            Err(ErrKind::Cancelled)
        );
        // Cancelled while the encoder runs: returns well before a slow encode would finish.
        let big = rgb(1600, 1200);
        let t = CancelToken::new_batch();
        let t2 = t.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(150));
            t2.cancel();
        });
        let slow = AvifEncoder {
            speed: 2,
            threads: Some(1),
            ..AvifEncoder::default()
        };
        let start = std::time::Instant::now();
        let r = slow.encode(&big.view(), &spec(90), &EncodeMeta::default(), &t);
        assert_eq!(r, Err(ErrKind::Cancelled));
        assert!(start.elapsed().as_millis() < 2000, "{:?}", start.elapsed());
    }

    #[test]
    fn the_encoder_is_deterministic() {
        let src = rgb(80, 60);
        let one = || {
            fast()
                .encode(
                    &src.view(),
                    &spec(70),
                    &EncodeMeta::default(),
                    &CancelToken::never(),
                )
                .unwrap()
        };
        assert_eq!(one(), one());
    }

    /// Needs libheif and dav1d: runs in the `heif` workflow. The picture must come back within the
    /// documented bar (PSNR >= 33 dB at quality 80 on the synthetic photo, both chroma layouts).
    #[cfg(feature = "heif")]
    #[test]
    fn libheif_decodes_what_this_writer_makes() {
        let src = rgb(160, 120);
        for chroma in [AvifChroma::Yuv444, AvifChroma::Yuv420] {
            let enc = AvifEncoder { chroma, ..fast() };
            let out = enc
                .encode(
                    &src.view(),
                    &spec(80),
                    &EncodeMeta::default(),
                    &CancelToken::never(),
                )
                .unwrap();
            let d = crate::decode_with(&out, &crate::DecodeLimits::default()).unwrap();
            assert_eq!(d.format, crate::Format::Avif);
            assert_eq!((d.raster.width, d.raster.height), (160, 120));
            assert_eq!(d.exif_orientation, 1);
            let p = crate::fixtures::psnr(&photo(160, 120, 7), &d.raster.data);
            eprintln!("avif {chroma:?} q80 psnr {p:.2} dB, {} bytes", out.len());
            assert!(p >= 33.0, "{chroma:?}: {p}");
        }
    }
}
