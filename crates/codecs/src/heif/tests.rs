// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The libheif backend on real files: the committed AVIF fixtures (made with Pillow, see
//! docs/provenance.md) and, when `cargo xtask build-native` has extracted the pinned libheif
//! archive, the HEIC and AVIF test files inside it (read in place, never committed; CI sets
//! `AUTOCROP_REQUIRE_HEIF_SAMPLES=1` so a missing archive fails there instead of skipping).

use super::*;
use crate::decode::orient;
use crate::{CodecError, DecodeLimits, Format, decode, decode_with, probe};
use auto_crop_core::ports::Want;
use std::path::PathBuf;

fn fixture(name: &str) -> Vec<u8> {
    crate::fixtures::heif_real_files()
        .into_iter()
        .find(|(n, _)| *n == name)
        .unwrap_or_else(|| panic!("no fixture {name}"))
        .1
        .to_vec()
}

/// A file from the extracted libheif source archive, if build-native has been run.
fn libheif_sample(rel: &str) -> Option<Vec<u8>> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/native")
        .join(format!(
            "libheif-{LINKED_VERSION}/src/libheif-{LINKED_VERSION}"
        ));
    match std::fs::read(dir.join(rel)) {
        Ok(b) => Some(b),
        Err(e) => {
            assert!(
                std::env::var_os("AUTOCROP_REQUIRE_HEIF_SAMPLES").is_none(),
                "{rel} is required but missing: {e}"
            );
            eprintln!("skipped: {rel} not found ({e}); run `cargo xtask build-native`");
            None
        }
    }
}

fn px(d: &Decoded, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * d.raster.width + x) * 3) as usize;
    d.raster.data[i..i + 3].try_into().unwrap()
}

fn close(a: [u8; 3], b: [u8; 3], tol: i32) -> bool {
    a.iter()
        .zip(b)
        .all(|(x, y)| (i32::from(*x) - i32::from(y)).abs() <= tol)
}

#[test]
fn the_linked_library_is_the_pinned_one_with_both_decoders() {
    init().unwrap();
    assert_eq!(runtime_version(), LINKED_VERSION);
    assert!(have_av1_decoder(), "dav1d must be linked into libheif");
    // HEVC comes from the libde265 plugin, found next to the build (CI and local builds).
    assert!(have_hevc_decoder(), "the libde265 plugin was not found");
}

#[test]
fn an_avif_decodes_to_the_pixels_that_were_encoded() {
    let d = decode(&fixture("gradient-444-48x32.avif")).unwrap();
    assert_eq!(d.format, Format::Avif);
    assert_eq!((d.raster.width, d.raster.height), (48, 32));
    assert_eq!(
        (d.exif_orientation, d.frames, d.source_bit_depth),
        (1, 1, 8)
    );
    assert!(d.notices.is_empty(), "{:?}", d.notices);
    assert!(d.icc.is_none());
    // Corner squares (red, green, blue, yellow), the gradient in between.
    assert!(close(px(&d, 2, 2), [235, 30, 30], 12), "{:?}", px(&d, 2, 2));
    assert!(
        close(px(&d, 45, 2), [30, 220, 40], 12),
        "{:?}",
        px(&d, 45, 2)
    );
    assert!(
        close(px(&d, 2, 29), [40, 60, 235], 12),
        "{:?}",
        px(&d, 2, 29)
    );
    assert!(
        close(px(&d, 45, 29), [240, 230, 40], 12),
        "{:?}",
        px(&d, 45, 29)
    );
    assert!(close(
        px(&d, 24, 4),
        [(40 + 24 * 150 / 47) as u8, (40 + 4 * 150 / 31) as u8, 110],
        12
    ));
}

#[test]
fn a_420_avif_and_a_gray_avif_decode() {
    let d = decode(&fixture("gradient-420-64x48.avif")).unwrap();
    assert_eq!((d.raster.width, d.raster.height), (64, 48));
    assert!(close(px(&d, 3, 3), [235, 30, 30], 40), "{:?}", px(&d, 3, 3));
    let g = decode(&fixture("gray-32x24.avif")).unwrap();
    assert_eq!((g.raster.width, g.raster.height), (32, 24));
    assert!(
        g.raster
            .data
            .chunks(3)
            .all(|p| p[0] == p[1] && p[1] == p[2])
    );
}

#[test]
fn probe_and_decode_agree_on_every_committed_file() {
    for (name, bytes) in crate::fixtures::heif_real_files() {
        let p = probe(bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        let d = decode(bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        // `Probe` gives the stored size and the orientation as EXIF would; the decode comes back
        // upright, so the probe's oriented size must be the raster's.
        let (w, h) = if p.orientation >= 5 {
            (p.height, p.width)
        } else {
            (p.width, p.height)
        };
        assert_eq!((d.raster.width, d.raster.height), (w, h), "{name}");
        assert_eq!(d.exif_orientation, p.orientation, "{name}");
        assert_eq!(d.source_bit_depth, p.bit_depth, "{name}");
        assert_eq!(
            d.icc.as_ref().map(Vec::len),
            p.icc_len.map(|l| l as usize),
            "{name}"
        );
    }
}

#[test]
fn irot_and_imir_are_applied_once_and_match_a_rotated_reference() {
    let base = decode(&fixture("gradient-444-48x32.avif")).unwrap();
    for o in 2u8..=8 {
        let name = format!("orient{o}-444-48x32.avif");
        let bytes = fixture(&name);
        let want = orient(base.raster.clone(), o).unwrap();
        let d = decode(&bytes).unwrap();
        assert_eq!(d.exif_orientation, o, "{name}");
        assert_eq!(
            (d.raster.width, d.raster.height),
            (want.width, want.height),
            "{name}: size"
        );
        // 4:4:4 and identical coded data: rotating is a lossless permutation of the pixels, so
        // the decoded file equals the rotated reference exactly (anything else is a double
        // rotation or a wrong turn). Allow one level for a colour-conversion rounding.
        let worst = d
            .raster
            .data
            .iter()
            .zip(&want.data)
            .map(|(a, b)| (i32::from(*a) - i32::from(*b)).abs())
            .max()
            .unwrap();
        assert!(worst <= 1, "{name}: worst difference {worst}");
        // The scaled-decode path must not turn it a second time.
        let s = crate::decode_scaled(&bytes, Want::Full, &DecodeLimits::default()).unwrap();
        assert_eq!(
            s.decoded.raster.data, d.raster.data,
            "{name}: scaled decode turned again"
        );
        assert_eq!((s.source_width, s.source_height), (48, 32), "{name}");
    }
}

#[test]
fn the_exif_orientation_tag_of_a_heif_file_is_ignored() {
    let plain = decode(&fixture("gradient-444-48x32.avif")).unwrap();
    let tagged = decode(&fixture("exif-orient6-noirot-48x32.avif")).unwrap();
    assert_eq!(tagged.exif_orientation, 1);
    assert_eq!((tagged.raster.width, tagged.raster.height), (48, 32));
    assert_eq!(tagged.raster.data, plain.raster.data);
}

#[test]
fn ten_bit_alpha_icc_and_sequence_files_report_their_notices() {
    let d = decode(&fixture("depth10-32x24.avif")).unwrap();
    assert_eq!(d.source_bit_depth, 10);
    assert_eq!(d.notices, ["depth.reduced_to_8"]);
    assert!(close(px(&d, 2, 2), [235, 30, 30], 20), "{:?}", px(&d, 2, 2));

    let a = decode(&fixture("alpha-32x24.avif")).unwrap();
    assert_eq!(a.notices, ["alpha.dropped"]);
    assert_eq!((a.raster.width, a.raster.height), (32, 24));

    let i = decode(&fixture("icc-srgb-32x24.avif")).unwrap();
    let want = std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/heif/icc-srgb.icc"),
    )
    .unwrap();
    assert_eq!(
        i.icc.as_deref(),
        Some(&want[..]),
        "the ICC profile must come back byte-exact"
    );

    let s = decode(&fixture("seq-2frames-32x24.avif")).unwrap();
    assert!(
        s.notices.contains(&"anim.first_frame_only"),
        "{:?}",
        s.notices
    );
    assert_eq!((s.raster.width, s.raster.height), (32, 24));
}

#[test]
fn the_pixel_cap_applies_before_libheif_and_libheif_has_its_own_limits() {
    let bytes = fixture("gradient-444-48x32.avif"); // 1536 pixels
    let tight = DecodeLimits::default().with_max_pixels(1000);
    assert_eq!(
        decode_with(&bytes, &tight).unwrap_err(),
        CodecError::TooLarge(1536)
    );
    // The security limits handed to libheif refuse the same file on their own.
    init().unwrap();
    let lim = ffi::Limits {
        max_pixels: 1000,
        max_tiles: 100,
        max_icc_bytes: 1 << 20,
        max_block_bytes: 1 << 28,
        max_total_bytes: 1 << 28,
    };
    let ctx = ffi::Context::read(&bytes, &lim).unwrap();
    let e = match ctx
        .primary()
        .and_then(|h| h.decode(false, &ffi::Options::new(None, 0).unwrap()))
    {
        Err(e) => e,
        Ok(_) => panic!("libheif decoded past its pixel limit"),
    };
    assert_eq!(
        (e.code, e.subcode),
        (ffi::ERR_MEMORY, ffi::SUBERR_SECURITY_LIMIT),
        "{e}"
    );
    assert_eq!(
        map_err(e, HeifCodec::Av1, &tight, 1536).code(),
        "limit_exceeded"
    );
}

#[test]
fn libheif_errors_map_to_typed_codec_errors() {
    let l = DecodeLimits::default();
    let err = |code, subcode, message: &str| ffi::HeifError {
        code,
        subcode,
        message: message.to_owned(),
    };
    // The no-hevc build: the libde265 plugin is missing.
    let missing = err(
        11,
        6003,
        "No decoding plugin installed for this compression format: HEVC",
    );
    assert_eq!(
        map_err(missing.clone(), HeifCodec::Hevc, &l, 1),
        CodecError::HevcDecoderMissing
    );
    assert_eq!(
        map_err(missing.clone(), HeifCodec::Other, &l, 1),
        CodecError::HevcDecoderMissing
    );
    assert!(matches!(
        map_err(err(11, 6003, "no plugin for AV1"), HeifCodec::Av1, &l, 1),
        CodecError::UnsupportedFeature(_)
    ));
    assert!(matches!(
        map_err(err(4, 3000, "unsupported codec"), HeifCodec::Other, &l, 1),
        CodecError::UnsupportedFeature(_)
    ));
    assert!(matches!(
        map_err(err(6, 1000, "limit"), HeifCodec::Av1, &l, 1),
        CodecError::LimitExceeded { .. }
    ));
    assert_eq!(
        map_err(err(12, 0, "cancelled"), HeifCodec::Av1, &l, 1),
        CodecError::DecodeTimeout
    );
    assert!(matches!(
        map_err(err(2, 101, "bad box"), HeifCodec::Av1, &l, 1),
        CodecError::Corrupt(_)
    ));
    assert!(matches!(
        map_err(err(7, 0, "decoder"), HeifCodec::Av1, &l, 1),
        CodecError::Corrupt(_)
    ));
}

#[test]
fn damaged_files_are_typed_errors_and_the_next_file_decodes() {
    let good = fixture("gradient-444-48x32.avif");
    for cut in [0, 3, 20, 100, good.len() / 2, good.len() - 1] {
        match decode(&good[..cut]) {
            Err(CodecError::InternalPanic(m)) => panic!("cut {cut}: panic {m}"),
            Err(_) => {}
            Ok(_) => panic!("cut {cut}: a truncated AVIF decoded"),
        }
    }
    // Garbage inside the coded data: a typed error or a decode, never a panic or a crash.
    let mut bad = good.clone();
    let n = bad.len();
    for b in &mut bad[n - 120..] {
        *b ^= 0xA5;
    }
    if let Err(CodecError::InternalPanic(m)) = decode(&bad) {
        panic!("panic {m}");
    }
    assert!(decode(&good).is_ok());
}

#[test]
fn decode_guarded_and_threads_setting_work() {
    set_codec_threads(1);
    let bytes: std::sync::Arc<[u8]> = fixture("gradient-444-48x32.avif").into();
    let limits = DecodeLimits {
        max_decode_ms: Some(30_000),
        ..DecodeLimits::default()
    };
    assert!(crate::decode_guarded(bytes, &limits).is_ok());
    set_codec_threads(0);
}

// ----------------------------------------------------------------- files of the libheif archive

#[test]
fn heic_files_of_the_libheif_archive_decode_through_the_libde265_plugin() {
    for (rel, size, alpha) in [
        ("tests/data/rainbow-451x461.heic", (451, 461), false),
        ("tests/data/with-alpha-512x512.heic", (512, 512), true),
        ("tests/data/conformance_window_padding.heic", (0, 0), false),
    ] {
        let Some(bytes) = libheif_sample(rel) else {
            return;
        };
        let p = probe(&bytes).unwrap_or_else(|e| panic!("{rel}: {e}"));
        assert_eq!(p.format, Format::Heic, "{rel}");
        let d = decode(&bytes).unwrap_or_else(|e| panic!("{rel}: {e}"));
        let (w, h) = if p.orientation >= 5 {
            (p.height, p.width)
        } else {
            (p.width, p.height)
        };
        assert_eq!(
            (d.raster.width, d.raster.height),
            (w, h),
            "{rel}: probe vs decode"
        );
        if size != (0, 0) {
            assert_eq!((d.raster.width, d.raster.height), size, "{rel}");
        }
        assert!(d.raster.data.iter().any(|b| *b != 0), "{rel}: all black");
        assert_eq!(
            d.notices.contains(&"alpha.dropped"),
            alpha,
            "{rel}: {:?}",
            d.notices
        );
    }
}

#[test]
fn clap_irot_imir_files_agree_between_the_header_walk_and_libheif() {
    for rel in [
        "tests/data/clap_cropped.avif",
        "tests/data/clap_cropped.heic",
        "tests/data/clap_cropped_irot_imir.avif",
        "tests/data/clap_oversized_ispe_irot90.avif",
        "tests/data/clap_oversized_ispe_irot180.avif",
    ] {
        let Some(bytes) = libheif_sample(rel) else {
            return;
        };
        let p = probe(&bytes).unwrap_or_else(|e| panic!("{rel}: {e}"));
        match decode(&bytes) {
            Ok(d) => {
                let (w, h) = if p.orientation >= 5 {
                    (p.height, p.width)
                } else {
                    (p.width, p.height)
                };
                assert_eq!(
                    (d.raster.width, d.raster.height),
                    (w, h),
                    "{rel}: libheif output vs the header walk (orientation {})",
                    p.orientation
                );
            }
            // Files built to be invalid on purpose (an aperture larger than the image) must be a
            // typed error.
            Err(e) => assert!(
                rel.contains("oversized") && !matches!(e, CodecError::InternalPanic(_)),
                "{rel}: {e}"
            ),
        }
    }
}

#[test]
fn avif_files_of_the_libheif_archive_decode() {
    let Some(bytes) = libheif_sample("examples/example.avif") else {
        return;
    };
    let d = decode(&bytes).unwrap();
    assert_eq!(d.format, Format::Avif);
    let p = probe(&bytes).unwrap();
    assert_eq!((d.raster.width, d.raster.height), (p.width, p.height));
    assert!(d.raster.width > 64 && d.raster.height > 64);
    assert!(d.raster.data.iter().any(|b| *b != 0));
}

#[test]
fn files_in_the_compact_mini_layout_are_a_typed_unsupported_feature() {
    // `mif3` files keep their size in a bit-packed `mini` box that the header walk does not read,
    // so they are refused with a named reason (libheif could decode them; see ADR-0009).
    for rel in [
        "tests/data/simple_osm_tile_alpha.avif",
        "tests/data/simple_osm_tile_meta.avif",
        "tests/data/lightning_mini.heif",
    ] {
        let Some(bytes) = libheif_sample(rel) else {
            return;
        };
        assert!(crate::sniff(&bytes).is_some(), "{rel} is not recognised");
        match decode(&bytes) {
            Err(CodecError::UnsupportedFeature(m)) => assert!(m.contains("mini"), "{rel}: {m}"),
            other => panic!("{rel}: {other:?}"),
        }
    }
}

#[test]
fn unsupported_codecs_in_the_libheif_fuzz_corpus_are_typed_errors() {
    // HEIF files whose image is AVC, JPEG, JPEG 2000 or VVC: libheif is built without those
    // decoders, so each is `UnsupportedFeature` (never a panic, never a silent success).
    for rel in [
        "fuzzing/data/corpus/avc32.heif",
        "fuzzing/data/corpus/jpeg32.heif",
        "fuzzing/data/corpus/j2k32.heif",
        "fuzzing/data/corpus/vvc32.heif",
    ] {
        let Some(bytes) = libheif_sample(rel) else {
            return;
        };
        match decode(&bytes) {
            Err(CodecError::UnsupportedFeature(_)) | Err(CodecError::Corrupt(_)) => {}
            other => panic!("{rel}: {other:?}"),
        }
    }
}
