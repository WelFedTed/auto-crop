// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Tests of the libjpeg-turbo path (feature `turbojpeg`; CI builds the pinned library on three
//! operating systems and runs these, see `.github/workflows/turbojpeg.yml`).

use super::{Transformer, transform};
use crate::fixtures::{JpegSpec, jpeg_cmyk, jpeg_insert_segment, orient_reference, photo, psnr};
use crate::jpeg_lossless::{self, Op, Policy, Rect};
use crate::{CodecError, DecodeLimits, decode, decode_scaled, probe};
use auto_crop_core::ports::Want;
use auto_crop_imgproc::Raster;

fn limits() -> DecodeLimits {
    DecodeLimits::default()
}

fn gray_of(rgb: &[u8]) -> Vec<u8> {
    rgb.as_chunks::<3>()
        .0
        .iter()
        .map(|p| ((u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2])) / 3) as u8)
        .collect()
}

fn mad(a: &[u8], b: &[u8]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (i32::from(*x) - i32::from(*y)).unsigned_abs() as f64)
        .sum::<f64>()
        / a.len().max(1) as f64
}

struct Variant {
    name: &'static str,
    sampling: (u8, u8),
    gray: bool,
    progressive: bool,
    quality: u8,
}

const VARIANTS: [Variant; 5] = [
    Variant {
        name: "4:2:0",
        sampling: (2, 2),
        gray: false,
        progressive: false,
        quality: 92,
    },
    Variant {
        name: "4:4:4",
        sampling: (1, 1),
        gray: false,
        progressive: false,
        quality: 92,
    },
    Variant {
        name: "4:2:2",
        sampling: (2, 1),
        gray: false,
        progressive: false,
        quality: 85,
    },
    Variant {
        name: "gray",
        sampling: (1, 1),
        gray: true,
        progressive: false,
        quality: 92,
    },
    Variant {
        name: "4:2:0 progressive",
        sampling: (2, 2),
        gray: false,
        progressive: true,
        quality: 90,
    },
];

fn build(w: u32, h: u32, seed: u64, v: &Variant) -> Vec<u8> {
    let rgb = photo(w, h, seed);
    let mut spec = JpegSpec::new(w, h);
    spec.quality = v.quality;
    spec.sampling = v.sampling;
    spec.progressive = v.progressive;
    spec.gray = v.gray;
    if v.gray {
        spec.encode(&gray_of(&rgb), jpeg_encoder::ColorType::Luma)
    } else {
        spec.encode(&rgb, jpeg_encoder::ColorType::Rgb)
    }
}

/// The reference for a scaled decode: the full decode (zune-jpeg) reduced by a block average.
fn reference(bytes: &[u8], denom: u32) -> Raster {
    let full = decode(bytes).unwrap().raster;
    crate::scaled::block_mean(&full, denom)
}

fn scaled_of(bytes: &[u8], denom: u32, long_edge: u32) -> crate::ScaledDecoded {
    let min_edge = long_edge.div_ceil(denom);
    let r = decode_scaled(bytes, Want::Scaled { min_edge }, &limits()).unwrap();
    assert_eq!(
        r.denom, denom,
        "min_edge {min_edge} for a {long_edge} px long edge"
    );
    r
}

#[test]
fn the_linked_library_is_at_least_3_1_4() {
    // build.rs already refused anything older; this also fails the build if the constant drifts.
    const { assert!(LINKED >= 3_001_004) };
    println!("libjpeg-turbo version number {LINKED}");
}

const LINKED: u32 = super::LINKED_VERSION;

#[test]
fn scaled_decode_agrees_with_full_decode_plus_block_average_on_40_fixtures() {
    // 8 sizes (odd ones included) x 5 variants = 40 fixtures; 1/2, 1/4 and 1/8 each.
    let sizes = [
        (64, 48),
        (97, 61),
        (200, 150),
        (333, 257),
        (640, 480),
        (257, 640),
        (1025, 769),
        (800, 600),
    ];
    let mut worst = [f64::INFINITY; 3];
    let mut count = 0;
    for (i, (w, h)) in sizes.into_iter().enumerate() {
        for (j, v) in VARIANTS.iter().enumerate() {
            let bytes = build(w, h, (i * 10 + j) as u64 + 1, v);
            for (k, denom) in [2u32, 4, 8].into_iter().enumerate() {
                let got = scaled_of(&bytes, denom, w.max(h));
                let want = reference(&bytes, denom);
                let r = &got.decoded.raster;
                assert_eq!(
                    (r.width, r.height),
                    (want.width, want.height),
                    "{w}x{h} {}",
                    v.name
                );
                assert_eq!((got.source_width, got.source_height), (w, h));
                let p = psnr(&r.data, &want.data);
                worst[k] = worst[k].min(p);
                assert!(
                    p >= 35.0,
                    "{w}x{h} {} 1/{denom}: PSNR {p:.2} dB < 35",
                    v.name
                );
            }
            count += 1;
        }
    }
    assert_eq!(count, 40);
    println!(
        "scaled vs full+block average over {count} fixtures: worst PSNR 1/2 {:.1} dB, 1/4 {:.1} dB, 1/8 {:.1} dB",
        worst[0], worst[1], worst[2]
    );
}

#[test]
fn a_4001_by_3001_image_scales_to_the_rounded_up_sizes() {
    let v = &VARIANTS[0];
    let bytes = build(4001, 3001, 99, v);
    for (denom, min_edge) in [(8u32, 500u32), (4, 1001), (2, 2001), (1, 4001)] {
        let r = decode_scaled(&bytes, Want::Scaled { min_edge }, &limits()).unwrap();
        // min_edge 500 -> 1/8 (long edge ceil(4001 / 8) = 501), 1001 -> 1/4 (1001), ...
        assert_eq!(r.denom, denom, "min_edge {min_edge}");
        let (w, h) = (4001u32.div_ceil(denom), 3001u32.div_ceil(denom));
        assert_eq!((r.decoded.raster.width, r.decoded.raster.height), (w, h));
        assert!(w.max(h) >= min_edge);
        if denom > 1 {
            let want = reference(&bytes, denom);
            let p = psnr(&r.decoded.raster.data, &want.data);
            println!("4001x3001 1/{denom}: PSNR vs full+block average {p:.2} dB");
            assert!(p >= 35.0, "1/{denom}: {p:.2} dB");
        }
    }
    // One more than the largest reduction can give: the next size up is picked.
    let r = decode_scaled(&bytes, Want::Scaled { min_edge: 502 }, &limits()).unwrap();
    assert_eq!(r.denom, 4);
}

#[test]
fn the_exif_orientation_is_applied_after_scaling() {
    let (w, h) = (97u32, 61u32);
    let rgb = photo(w, h, 5);
    let plain = {
        let mut s = JpegSpec::new(w, h);
        s.sampling = (2, 2);
        s.encode(&rgb, jpeg_encoder::ColorType::Rgb)
    };
    let base = scaled_of(&plain, 2, w.max(h)).decoded.raster;
    for o in 1u8..=8 {
        let tagged = {
            let mut s = JpegSpec::new(w, h);
            s.sampling = (2, 2);
            s.exif_orientation = Some(u16::from(o));
            s.encode(&rgb, jpeg_encoder::ColorType::Rgb)
        };
        let got = scaled_of(&tagged, 2, w.max(h));
        assert_eq!(got.decoded.exif_orientation, o);
        let (want, ww, wh) = orient_reference(&base.data, base.width, base.height, o);
        let r = &got.decoded.raster;
        assert_eq!((r.width, r.height), (ww, wh), "orientation {o}");
        assert_eq!(
            r.data, want,
            "orientation {o}: pixels must be the scaled decode, turned"
        );
        // The stored size stays that of the file; only the pixels are turned.
        assert_eq!((got.source_width, got.source_height), (w, h));
    }
}

#[test]
fn full_decode_agrees_with_zune_jpeg_within_the_documented_bound() {
    for v in &VARIANTS {
        let bytes = build(120, 80, 3, v);
        let a = decode_scaled(&bytes, Want::Full, &limits()).unwrap();
        assert_eq!(a.denom, 1);
        let b = decode(&bytes).unwrap();
        let m = mad(&a.decoded.raster.data, &b.raster.data);
        println!(
            "{}: libjpeg-turbo vs zune-jpeg mean abs diff {m:.3} LSB",
            v.name
        );
        assert!(m <= 1.5, "{}: {m}", v.name);
    }
}

#[test]
fn the_icc_profile_and_exif_orientation_survive_a_turbo_decode() {
    let mut s = JpegSpec::new(64, 48);
    s.icc = Some(crate::fixtures::fake_icc(3000));
    s.exif_orientation = Some(3);
    let bytes = s.build();
    let r = decode_scaled(&bytes, Want::Scaled { min_edge: 8 }, &limits()).unwrap();
    assert_eq!(r.decoded.icc, s.icc);
    assert_eq!(r.decoded.exif_orientation, 3);
}

#[test]
fn a_cmyk_jpeg_falls_back_to_the_safe_path_with_the_same_sizes() {
    let bytes = jpeg_cmyk(80, 48, false);
    let r = decode_scaled(&bytes, Want::Scaled { min_edge: 10 }, &limits()).unwrap();
    assert_eq!(r.denom, 8);
    assert_eq!((r.decoded.raster.width, r.decoded.raster.height), (10, 6));
    assert!(r.decoded.notices.contains(&"cmyk.naive_conversion"));
}

#[test]
fn damaged_and_hostile_jpegs_get_typed_errors_and_never_a_panic() {
    let good = build(96, 64, 7, &VARIANTS[0]);
    // Truncated: refused before libjpeg-turbo sees it (the shared pre-checks).
    assert!(matches!(
        decode_scaled(&good[..good.len() - 40], Want::Full, &limits()),
        Err(CodecError::Corrupt(_))
    ));
    assert!(decode_scaled(b"\xFF\xD8\xFF\xD9", Want::Full, &limits()).is_err());
    assert!(decode_scaled(&[], Want::Full, &limits()).is_err());
    // Scan bomb: the scan cap applies before and inside the library.
    let bomb = crate::fixtures::jpeg_repeat_scan(&good, 500);
    assert!(matches!(
        decode_scaled(&bomb, Want::Full, &limits()),
        Err(CodecError::LimitExceeded { .. })
    ));
    // Every JPEG of the hostile corpus, at the scaled and the full path.
    let mut n = 0;
    for h in crate::hostile::corpus() {
        if crate::sniff(&h.bytes) != Some(crate::Format::Jpeg) {
            continue;
        }
        n += 1;
        for want in [Want::Full, Want::Scaled { min_edge: 64 }] {
            if let Err(CodecError::InternalPanic(m)) = decode_scaled(&h.bytes, want, &limits()) {
                panic!("{}: panic {m}", h.name);
            }
        }
        if let Err(CodecError::InternalPanic(m)) =
            transform(&h.bytes, Op::Rotate90, Policy::Snap, &limits())
        {
            panic!("{}: transform panic {m}", h.name);
        }
    }
    assert!(n > 0, "the corpus has JPEG cases");
}

#[test]
fn byte_mutations_never_panic_or_crash() {
    // Every byte of the header region, three values each, on a baseline and a progressive file.
    let l = DecodeLimits::default().with_max_pixels(1 << 20);
    for v in [&VARIANTS[0], &VARIANTS[4], &VARIANTS[3]] {
        let bytes = build(48, 32, 11, v);
        let mut b = bytes.clone();
        for i in (0..bytes.len()).filter(|i| *i < 400 || i % 9 == 0) {
            let keep = b[i];
            for val in [0x00u8, 0xFF, keep ^ 0x5A] {
                b[i] = val;
                for want in [Want::Full, Want::Scaled { min_edge: 8 }] {
                    if let Err(CodecError::InternalPanic(m)) = decode_scaled(&b, want, &l) {
                        panic!("{}: byte {i} = {val:#x}: {m}", v.name);
                    }
                }
                if let Err(CodecError::InternalPanic(m)) =
                    transform(&b, Op::Rotate90, Policy::Snap, &l)
                {
                    panic!("{}: transform byte {i} = {val:#x}: {m}", v.name);
                }
            }
            b[i] = keep;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Lossless transform (M1.20)
// ---------------------------------------------------------------------------------------------

const OPS: [Op; 7] = [
    Op::Rotate90,
    Op::Rotate180,
    Op::Rotate270,
    Op::FlipH,
    Op::FlipV,
    Op::Transpose,
    Op::Transverse,
];

fn spec_of(w: u32, h: u32, sampling: (u8, u8), gray: bool) -> Vec<u8> {
    let mut s = JpegSpec::new(w, h);
    s.sampling = sampling;
    s.gray = gray;
    s.build()
}

#[test]
fn every_op_gives_the_same_pixels_as_the_safe_rust_transformer() {
    // Both implementations move the same DCT coefficients, so the decoded pixels must be identical
    // (not merely close), and so must the realised rectangle, size and `perfect` flag.
    let inputs = [
        ("gray 64x48", spec_of(64, 48, (1, 1), true)),
        ("4:4:4 64x48", spec_of(64, 48, (1, 1), false)),
        ("4:2:0 64x48", spec_of(64, 48, (2, 2), false)),
        ("4:2:2 64x48", spec_of(64, 48, (2, 1), false)),
        ("4:2:0 70x50 (partial MCUs)", spec_of(70, 50, (2, 2), false)),
        ("4:4:4 70x50", spec_of(70, 50, (1, 1), false)),
    ];
    let mut tj = Transformer::new().unwrap();
    for (name, src) in &inputs {
        for op in OPS {
            for policy in [Policy::Perfect, Policy::Snap] {
                let safe = jpeg_lossless::transform(src, op, policy, &limits());
                let turbo = tj.transform(src, op, policy, &limits());
                match (safe, turbo) {
                    (Ok(a), Ok(b)) => {
                        assert_eq!(
                            (a.width, a.height, a.rect, a.perfect),
                            (b.width, b.height, b.rect, b.perfect),
                            "{name} {op:?} {policy:?}"
                        );
                        let (da, db) = (decode(&a.bytes).unwrap(), decode(&b.bytes).unwrap());
                        assert_eq!(da.raster, db.raster, "{name} {op:?} {policy:?}");
                    }
                    (Err(_), Err(_)) => {}
                    (a, b) => panic!(
                        "{name} {op:?} {policy:?}: safe {:?} vs turbo {:?}",
                        a.map(|t| t.rect),
                        b.map(|t| t.rect)
                    ),
                }
            }
        }
    }
}

#[test]
fn crops_snap_to_the_imcu_grid_and_report_the_realised_rectangle() {
    let src = spec_of(96, 64, (2, 2), false);
    let mut tj = Transformer::new().unwrap();
    let aligned = Rect {
        x: 16,
        y: 16,
        w: 32,
        h: 32,
    };
    let t = tj
        .transform(&src, Op::Crop(aligned), Policy::Perfect, &limits())
        .unwrap();
    assert!(t.perfect && t.rect == aligned && (t.width, t.height) == (32, 32));
    // The cropped pixels are the source pixels of the rectangle, exactly (same coefficients).
    let (base, got) = (decode(&src).unwrap(), decode(&t.bytes).unwrap());
    for y in 0..32usize {
        let (a, b) = ((y + 16) * 96 + 16, y * 32);
        assert_eq!(
            &base.raster.data[a * 3..(a + 32) * 3],
            &got.raster.data[b * 3..(b + 32) * 3]
        );
    }
    let off = Rect {
        x: 20,
        y: 10,
        w: 30,
        h: 30,
    };
    assert!(matches!(
        tj.transform(&src, Op::Crop(off), Policy::Perfect, &limits()),
        Err(CodecError::UnsupportedFeature(_))
    ));
    let t = tj
        .transform(&src, Op::Crop(off), Policy::Snap, &limits())
        .unwrap();
    assert!(!t.perfect);
    assert_eq!(
        t.rect,
        Rect {
            x: 16,
            y: 0,
            w: 48,
            h: 48
        }
    );
    assert_eq!((t.width, t.height), (48, 48));
    // The edge of the image is kept, not grown past.
    let edge = Rect {
        x: 64,
        y: 32,
        w: 32,
        h: 32,
    };
    let t = tj
        .transform(&src, Op::Crop(edge), Policy::Perfect, &limits())
        .unwrap();
    assert!(t.perfect && t.rect == edge);
    for r in [
        Rect {
            x: 0,
            y: 0,
            w: 0,
            h: 5,
        },
        Rect {
            x: 500,
            y: 0,
            w: 5,
            h: 5,
        },
    ] {
        assert!(
            tj.transform(&src, Op::Crop(r), Policy::Snap, &limits())
                .is_err()
        );
    }
}

#[test]
fn four_quarter_turns_and_inverse_ops_restore_the_pixels_exactly() {
    let src = spec_of(64, 48, (2, 2), false);
    let base = decode(&src).unwrap();
    let mut cur = src.clone();
    for _ in 0..4 {
        cur = transform(&cur, Op::Rotate90, Policy::Perfect, &limits())
            .unwrap()
            .bytes;
    }
    assert_eq!(decode(&cur).unwrap().raster, base.raster);
    let flipped = transform(&src, Op::FlipH, Policy::Perfect, &limits())
        .unwrap()
        .bytes;
    let back = transform(&flipped, Op::FlipH, Policy::Perfect, &limits())
        .unwrap()
        .bytes;
    assert_eq!(decode(&back).unwrap().raster, base.raster);
}

#[test]
fn a_perfect_rotation_of_a_partial_mcu_image_is_refused_and_snap_trims() {
    let src = spec_of(70, 50, (2, 2), false);
    assert!(matches!(
        transform(&src, Op::Rotate90, Policy::Perfect, &limits()),
        Err(CodecError::UnsupportedFeature(_))
    ));
    let t = transform(&src, Op::Rotate90, Policy::Snap, &limits()).unwrap();
    assert!(!t.perfect);
    assert_eq!((t.width, t.height), (48, 70)); // 50 -> 48 trimmed on the mirrored axis, then turned
    assert_eq!(
        probe(&t.bytes).map(|p| (p.width, p.height)).unwrap(),
        (48, 70)
    );
    // A transpose mirrors no axis: perfect at any size.
    assert!(
        transform(&src, Op::Transpose, Policy::Perfect, &limits())
            .unwrap()
            .perfect
    );
}

#[test]
fn metadata_and_the_exif_orientation_tag_are_copied_untouched() {
    let mut s = JpegSpec::new(64, 48);
    s.sampling = (2, 2);
    s.icc = Some(crate::fixtures::fake_icc(2500));
    s.exif_orientation = Some(6);
    let src = jpeg_insert_segment(&s.build(), 0xFE, b"a comment");
    let t = transform(&src, Op::Rotate90, Policy::Perfect, &limits()).unwrap();
    let p = probe(&t.bytes).unwrap();
    assert_eq!(
        p.orientation, 6,
        "patching the Orientation tag is the caller's job"
    );
    assert_eq!(decode_icc(&t.bytes), s.icc);
    assert!(t.bytes.windows(9).any(|w| w == b"a comment"));
}

fn decode_icc(bytes: &[u8]) -> Option<Vec<u8>> {
    decode_scaled(bytes, Want::Scaled { min_edge: 1 }, &limits())
        .unwrap()
        .decoded
        .icc
}

#[test]
fn progressive_input_is_transformed_too() {
    // The safe-Rust transformer refuses progressive files; libjpeg-turbo handles them.
    let src = build(64, 48, 21, &VARIANTS[4]);
    assert!(jpeg_lossless::transform(&src, Op::Rotate90, Policy::Perfect, &limits()).is_err());
    let t = transform(&src, Op::Rotate90, Policy::Perfect, &limits()).unwrap();
    assert_eq!((t.width, t.height), (48, 64));
    let (a, b) = (decode(&src).unwrap(), decode(&t.bytes).unwrap());
    let (want, _, _) = orient_reference(&a.raster.data, 64, 48, 6);
    assert!(mad(&b.raster.data, &want) <= 1.5);
}

#[test]
fn a_thousand_transforms_reuse_one_handle_and_one_buffer() {
    // The reuse test of ROADMAP M1.20, run under AddressSanitizer in CI (ubuntu, nightly,
    // libjpeg-turbo built with -fsanitize=address): one Transformer, one caller-owned output
    // buffer that the library writes into, 1000 operations over inputs of different sizes.
    let inputs = [
        spec_of(64, 48, (2, 2), false),
        spec_of(130, 90, (1, 1), false),
        build(200, 120, 4, &VARIANTS[4]),
        spec_of(48, 64, (1, 1), true),
    ];
    let mut tj = Transformer::new().unwrap();
    let mut first: Vec<Option<Vec<u8>>> = vec![None; inputs.len() * OPS.len()];
    let mut cap_after_warmup = 0;
    for i in 0..1000usize {
        let (k, o) = (i % inputs.len(), (i / inputs.len()) % OPS.len());
        let out = tj
            .transform(&inputs[k], OPS[o], Policy::Snap, &limits())
            .unwrap();
        match &first[k * OPS.len() + o] {
            None => first[k * OPS.len() + o] = Some(out.bytes),
            Some(f) => assert_eq!(f, &out.bytes, "iteration {i} differs from the first run"),
        }
        if i == inputs.len() * OPS.len() {
            cap_after_warmup = tj.buffer_capacity();
        }
    }
    assert_eq!(
        tj.buffer_capacity(),
        cap_after_warmup,
        "the reused buffer must not keep growing"
    );
}

#[test]
fn a_destination_that_is_too_small_is_an_error_not_an_overflow() {
    use super::ffi::{
        Handle, Region, TJINIT_TRANSFORM, TJXOP_ROT90, TJXOPT_PERFECT, TransformSpec,
    };
    let src = spec_of(64, 48, (2, 2), false);
    let mut h = Handle::new(TJINIT_TRANSFORM).unwrap();
    let spec = TransformSpec {
        op: TJXOP_ROT90,
        options: TJXOPT_PERFECT,
        region: Region::default(),
    };
    let mut tiny = [0u8; 16];
    assert!(h.transform(&src, &spec, &mut tiny).is_err());
}

#[test]
fn a_non_jpeg_is_refused_by_the_transformer() {
    let png = crate::fixtures::png_rgb(8, 8);
    assert!(matches!(
        transform(&png, Op::Rotate90, Policy::Snap, &limits()),
        Err(CodecError::Unsupported)
    ));
}

// ---------------------------------------------------------------------------------------------
// Encoder (M1.21)
// ---------------------------------------------------------------------------------------------

#[test]
fn compress_round_trips_at_q90_with_icc_and_density() {
    let (w, h) = (160usize, 120usize);
    let rgb = photo(w as u32, h as u32, 8);
    let icc = crate::fixtures::fake_icc(1234);
    let jpeg = super::compress(&rgb, w, h, false, 90, Some((300, 300)), Some(&icc)).unwrap();
    let back = decode(&jpeg).unwrap();
    let p = psnr(&back.raster.data, &rgb);
    println!("libjpeg-turbo q90 4:2:0 round trip: {p:.2} dB");
    assert_eq!(back.icc.as_deref(), Some(&icc[..]));
    assert!(p >= 30.0, "{p}"); // a noisy photo-like image; the smooth-image bound is in encoders.rs
    // JFIF APP0: units (1 = dpi) at offset 13, X and Y density after it.
    assert_eq!(&jpeg[6..11], b"JFIF\0");
    assert_eq!(jpeg[13], 1);
    assert_eq!(u16::from_be_bytes([jpeg[14], jpeg[15]]), 300);
    assert_eq!(u16::from_be_bytes([jpeg[16], jpeg[17]]), 300);
    let gray = super::compress(&gray_of(&rgb), w, h, true, 90, None, None).unwrap();
    assert_eq!(probe(&gray).unwrap().channels, 1);
}
