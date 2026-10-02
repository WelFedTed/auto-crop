// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! ROADMAP M1.20: lossless JPEG transforms, checked against decode-then-transform in the pixel
//! domain, against a second decoder (libjpeg-turbo through ImageMagick, when installed) and
//! against themselves (the inverse op restores the coefficients exactly).

use crate::fixtures::*;
use crate::jpeg_lossless::{Op, Policy, Rect, Transformed, transform};
use crate::{CodecError, DecodeLimits, decode};

fn limits() -> DecodeLimits {
    DecodeLimits::default()
}

/// Display orientation (EXIF numbering) that each op is equivalent to.
fn orientation_of(op: Op) -> u8 {
    match op {
        Op::Rotate90 => 6,
        Op::Rotate180 => 3,
        Op::Rotate270 => 8,
        Op::FlipH => 2,
        Op::FlipV => 4,
        Op::Transpose => 5,
        Op::Transverse => 7,
        Op::Crop(_) => 1,
    }
}

const OPS: [Op; 7] = [
    Op::Rotate90,
    Op::Rotate180,
    Op::Rotate270,
    Op::FlipH,
    Op::FlipV,
    Op::Transpose,
    Op::Transverse,
];

/// Aligned fixtures: 64 x 48 is a multiple of the MCU for every sampling used here.
fn variants() -> Vec<(&'static str, Vec<u8>)> {
    let mk = |gray: bool, sampling: (u8, u8), restart: Option<u16>| {
        let mut s = JpegSpec::new(64, 48);
        s.gray = gray;
        s.sampling = sampling;
        s.restart = restart;
        s.build()
    };
    vec![
        ("gray", mk(true, (1, 1), None)),
        ("4:4:4", mk(false, (1, 1), None)),
        ("4:2:0", mk(false, (2, 2), None)),
        ("4:2:2", mk(false, (2, 1), None)),
        ("4:4:0", mk(false, (1, 2), None)),
        ("4:2:0 + restart", mk(false, (2, 2), Some(3))),
    ]
}

fn mad(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b)
        .map(|(x, y)| (i32::from(*x) - i32::from(*y)).unsigned_abs() as f64)
        .sum::<f64>()
        / a.len().max(1) as f64
}

fn max_diff(a: &[u8], b: &[u8]) -> i32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (i32::from(*x) - i32::from(*y)).abs())
        .max()
        .unwrap_or(0)
}

#[test]
fn every_op_matches_decode_then_transform_within_the_documented_bound() {
    for (name, src) in variants() {
        let base = decode(&src).unwrap();
        for op in OPS {
            let t = transform(&src, op, Policy::Perfect, &limits())
                .unwrap_or_else(|e| panic!("{name} {op:?}: {e}"));
            assert!(t.perfect);
            let got = decode(&t.bytes).unwrap_or_else(|e| panic!("{name} {op:?} output: {e}"));
            let (want, ww, wh) = orient_reference(
                &base.raster.data,
                base.raster.width,
                base.raster.height,
                orientation_of(op),
            );
            assert_eq!(
                (got.raster.width, got.raster.height),
                (ww, wh),
                "{name} {op:?} size"
            );
            assert_eq!((t.width, t.height), (ww, wh));
            let m = mad(&got.raster.data, &want);
            let mx = max_diff(&got.raster.data, &want);
            eprintln!("{name:>16} {op:?}: mean {m:.3} max {mx}");
            // ADR-0004: integer inverse-DCT rounding is not perfectly symmetric (within 2 levels
            // for 4:4:4 and grey); subsampled chroma adds the asymmetry of the upsampling filter
            // at the picture edge.
            let (mean_bound, max_bound) = if name == "gray" || name == "4:4:4" {
                (0.25, 2)
            } else {
                (1.0, 24)
            };
            assert!(
                m <= mean_bound && mx <= max_bound,
                "{name} {op:?}: mean {m} max {mx}"
            );
        }
    }
}

#[test]
fn the_inverse_op_restores_the_coefficients_exactly() {
    let pairs = [
        (Op::Rotate90, Op::Rotate270),
        (Op::Rotate180, Op::Rotate180),
        (Op::FlipH, Op::FlipH),
        (Op::FlipV, Op::FlipV),
        (Op::Transpose, Op::Transpose),
        (Op::Transverse, Op::Transverse),
    ];
    for (name, src) in variants() {
        let want = decode(&src).unwrap().raster.data;
        for (a, b) in pairs {
            let once = transform(&src, a, Policy::Perfect, &limits()).unwrap();
            let twice = transform(&once.bytes, b, Policy::Perfect, &limits()).unwrap();
            let got = decode(&twice.bytes).unwrap();
            assert!(
                got.raster.data == want,
                "{name}: {a:?} then {b:?} must restore the pixels exactly"
            );
        }
        // Four quarter turns.
        let mut cur = src.clone();
        for _ in 0..4 {
            cur = transform(&cur, Op::Rotate90, Policy::Perfect, &limits())
                .unwrap()
                .bytes;
        }
        assert!(
            decode(&cur).unwrap().raster.data == want,
            "{name}: 4 x rotate90"
        );
    }
}

#[test]
fn rotate90_is_transpose_then_flip_and_composes_with_the_others() {
    let src = JpegSpec::new(64, 48).build();
    let direct = transform(&src, Op::Rotate90, Policy::Perfect, &limits()).unwrap();
    let t = transform(&src, Op::Transpose, Policy::Perfect, &limits()).unwrap();
    let composed = transform(&t.bytes, Op::FlipH, Policy::Perfect, &limits()).unwrap();
    assert!(
        decode(&direct.bytes).unwrap().raster.data == decode(&composed.bytes).unwrap().raster.data
    );
}

#[test]
fn partial_mcus_are_refused_when_perfect_and_trimmed_when_snapping() {
    // 70 x 50 in 4:2:0 (16 x 16 MCU): 70 and 50 are not multiples of 16.
    let mut s = JpegSpec::new(70, 50);
    s.sampling = (2, 2);
    let src = s.build();
    let base = decode(&src).unwrap();
    for op in [
        Op::Rotate90,
        Op::Rotate180,
        Op::Rotate270,
        Op::FlipH,
        Op::FlipV,
        Op::Transverse,
    ] {
        assert!(
            matches!(
                transform(&src, op, Policy::Perfect, &limits()),
                Err(CodecError::UnsupportedFeature(_))
            ),
            "{op:?} must be refused as not perfect"
        );
        let t = transform(&src, op, Policy::Snap, &limits()).unwrap();
        assert!(!t.perfect, "{op:?}");
        // The trimmed result shows the source minus its partial edge MCUs, transformed.
        let (rw, rh) = (t.rect.w as usize, t.rect.h as usize);
        let mirrors_x = matches!(
            op,
            Op::FlipH | Op::Rotate180 | Op::Rotate270 | Op::Transverse
        );
        let mirrors_y = matches!(
            op,
            Op::FlipV | Op::Rotate90 | Op::Rotate180 | Op::Transverse
        );
        // Only the mirrored axes lose their partial MCU (70 -> 64, 50 -> 48).
        assert_eq!(
            (rw, rh),
            (
                if mirrors_x { 64 } else { 70 },
                if mirrors_y { 48 } else { 50 }
            ),
            "{op:?}"
        );
        let mut cropped = Vec::new();
        for y in 0..rh {
            let row = (y * 70) * 3;
            cropped.extend_from_slice(&base.raster.data[row..row + rw * 3]);
        }
        let (want, ww, wh) = orient_reference(&cropped, rw as u32, rh as u32, orientation_of(op));
        let got = decode(&t.bytes).unwrap();
        assert_eq!((got.raster.width, got.raster.height), (ww, wh), "{op:?}");
        assert!(mad(&got.raster.data, &want) <= 1.5, "{op:?}");
    }
    // Transposing never mirrors an axis, so it is perfect at any size.
    let t = transform(&src, Op::Transpose, Policy::Perfect, &limits()).unwrap();
    assert!(t.perfect && (t.width, t.height) == (50, 70));
}

#[test]
fn a_crop_snaps_to_the_mcu_grid_and_reports_what_it_realised() {
    let mut s = JpegSpec::new(96, 64);
    s.sampling = (2, 2);
    let src = s.build();
    let base = decode(&src).unwrap();
    // On the 16-pixel grid: exact and perfect.
    let aligned = Rect {
        x: 16,
        y: 16,
        w: 32,
        h: 32,
    };
    let t = transform(&src, Op::Crop(aligned), Policy::Perfect, &limits()).unwrap();
    assert!(t.perfect && t.rect == aligned && (t.width, t.height) == (32, 32));
    // Off the grid: origin snaps down, far edge snaps up.
    let off = Rect {
        x: 20,
        y: 10,
        w: 30,
        h: 30,
    };
    assert!(matches!(
        transform(&src, Op::Crop(off), Policy::Perfect, &limits()),
        Err(CodecError::UnsupportedFeature(_))
    ));
    let t = transform(&src, Op::Crop(off), Policy::Snap, &limits()).unwrap();
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
    // The cropped pixels are the source pixels of the realised rectangle (chroma upsampling can
    // differ a little at the new border).
    let got = decode(&t.bytes).unwrap();
    let mut want = Vec::new();
    for y in 0..48usize {
        let row = (y * 96 + 16) * 3;
        want.extend_from_slice(&base.raster.data[row..row + 48 * 3]);
    }
    assert!(
        mad(&got.raster.data, &want) <= 1.0,
        "mean {}",
        mad(&got.raster.data, &want)
    );
    // A crop that reaches the image edge keeps the edge (no growth past it).
    let edge = Rect {
        x: 64,
        y: 32,
        w: 32,
        h: 32,
    };
    let t = transform(&src, Op::Crop(edge), Policy::Perfect, &limits()).unwrap();
    assert!(t.perfect && t.rect == edge);
    // Empty and outside rectangles are errors.
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
        assert!(transform(&src, Op::Crop(r), Policy::Snap, &limits()).is_err());
    }
}

#[test]
fn metadata_segments_and_the_exif_orientation_tag_are_copied_untouched() {
    let mut s = JpegSpec::new(64, 48);
    s.exif_orientation = Some(6);
    s.icc = Some(fake_icc(150_000));
    let src = s.build();
    for op in [
        Op::Rotate90,
        Op::FlipH,
        Op::Crop(Rect {
            x: 0,
            y: 0,
            w: 32,
            h: 32,
        }),
    ] {
        let t = transform(&src, op, Policy::Snap, &limits()).unwrap();
        let d = decode(&t.bytes).unwrap();
        // The tag still says 6: patching it is the caller's job; this layer never edits EXIF.
        assert_eq!(d.exif_orientation, 6, "{op:?}");
        assert_eq!(d.icc.as_deref(), Some(&fake_icc(150_000)[..]), "{op:?}");
        // The EXIF segment is byte-identical.
        let exif = |b: &[u8]| {
            let p = b.windows(6).position(|w| w == b"Exif\0\0").unwrap();
            b[p..p + 6 + exif_blob(6, false).len()].to_vec()
        };
        assert_eq!(exif(&t.bytes), exif(&src), "{op:?}");
    }
}

#[test]
fn a_thousand_reused_transforms_are_deterministic() {
    let src = JpegSpec::new(32, 32).build();
    let first = transform(&src, Op::Rotate90, Policy::Perfect, &limits()).unwrap();
    for i in 0..1000 {
        let op = if i % 2 == 0 { Op::Rotate90 } else { Op::FlipH };
        let t = transform(&src, op, Policy::Perfect, &limits()).unwrap();
        if i % 2 == 0 {
            assert_eq!(t, first, "iteration {i}");
        }
    }
}

#[test]
fn unsupported_and_damaged_inputs_are_typed_errors() {
    let base = jpeg_baseline(64, 48);
    let mut prog = JpegSpec::new(64, 48);
    prog.progressive = true;
    for (name, bytes, ok) in [
        ("progressive", prog.build(), false),
        (
            "12-bit",
            jpeg_patch_sof(&base, Some(12), None, Some(0xC1)),
            false,
        ),
        (
            "arithmetic",
            jpeg_patch_sof(&base, None, None, Some(0xC9)),
            false,
        ),
        ("png", png_rgb(16, 16), false),
        ("truncated", base[..base.len() / 2].to_vec(), false),
        ("empty", Vec::new(), false),
        ("baseline", base.clone(), true),
    ] {
        let r = transform(&bytes, Op::Rotate90, Policy::Snap, &limits());
        assert_eq!(r.is_ok(), ok, "{name}: {r:?}");
        if let Err(CodecError::InternalPanic(m)) = r {
            panic!("{name}: panicked: {m}");
        }
    }
    assert!(matches!(
        transform(&prog.build(), Op::FlipH, Policy::Snap, &limits()),
        Err(CodecError::UnsupportedFeature(_))
    ));
}

#[test]
fn byte_mutations_never_panic_the_transformer() {
    let src = {
        let mut s = JpegSpec::new(32, 32);
        s.sampling = (2, 2);
        s.restart = Some(2);
        s.build()
    };
    let l = DecodeLimits::default().with_max_pixels(1 << 20);
    let mut b = src.clone();
    for i in 0..src.len() {
        let keep = b[i];
        for v in [0x00u8, 0xFF, keep ^ 0x5A] {
            b[i] = v;
            let r = transform(&b, Op::Rotate90, Policy::Snap, &l);
            assert!(
                !matches!(r, Err(CodecError::InternalPanic(_))),
                "byte {i} = {v:#x}"
            );
        }
        b[i] = keep;
    }
}

// ----------------------------------------------------------------- second decoder (libjpeg-turbo)

fn magick_rgb(name: &str, bytes: &[u8]) -> Option<Vec<u8>> {
    let dir =
        std::env::temp_dir().join(format!("auto-crop-lossless-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    let src = dir.join("in.jpg");
    let dst = dir.join("out.rgb");
    std::fs::write(&src, bytes).ok()?;
    let out = std::process::Command::new("magick")
        .arg(&src)
        .args(["-depth", "8"])
        .arg(format!("rgb:{}", dst.display()))
        .output()
        .ok()?;
    let raw = out
        .status
        .success()
        .then(|| std::fs::read(&dst).ok())
        .flatten();
    let _ = std::fs::remove_dir_all(&dir);
    raw
}

#[test]
fn libjpeg_turbo_decodes_every_output_to_what_zune_jpeg_decodes() {
    // An independent decoder must accept our entropy coding and the optimised Huffman tables.
    for (name, src) in variants() {
        for op in OPS {
            let t: Transformed = transform(&src, op, Policy::Perfect, &limits()).unwrap();
            let Some(reference) = magick_rgb(&name.replace([':', ' ', '+'], ""), &t.bytes) else {
                eprintln!("magick not available: skipping the libjpeg-turbo check");
                return;
            };
            let ours = decode(&t.bytes).unwrap().raster.data;
            assert_eq!(ours.len(), reference.len(), "{name} {op:?}");
            let m = mad(&ours, &reference);
            assert!(m <= 1.5, "{name} {op:?}: libjpeg-turbo differs by {m} LSB");
        }
    }
}

#[test]
fn a_libjpeg_turbo_made_file_transforms_and_matches_its_own_pixel_rotation() {
    // A 4:2:0 file with libjpeg-turbo's standard tables and a JFIF header, made by ImageMagick.
    let (w, h) = (128u32, 96u32);
    let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
    ppm.extend(smooth(w, h));
    let dir = std::env::temp_dir().join(format!("auto-crop-lossless-im-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("in.ppm"), &ppm).unwrap();
    let made = std::process::Command::new("magick")
        .current_dir(&dir)
        .args([
            "in.ppm",
            "-sampling-factor",
            "2x2",
            "-quality",
            "88",
            "src.jpg",
        ])
        .output();
    let Ok(made) = made else {
        eprintln!("magick not available: skipping");
        let _ = std::fs::remove_dir_all(&dir);
        return;
    };
    assert!(made.status.success());
    let src = std::fs::read(dir.join("src.jpg")).unwrap();
    let rotated = std::process::Command::new("magick")
        .current_dir(&dir)
        .args(["src.jpg", "-rotate", "90", "-depth", "8", "rgb:rot.rgb"])
        .output()
        .unwrap();
    assert!(rotated.status.success());
    let want = std::fs::read(dir.join("rot.rgb")).unwrap();
    let _ = std::fs::remove_dir_all(&dir);

    let t = transform(&src, Op::Rotate90, Policy::Perfect, &limits()).unwrap();
    assert_eq!((t.width, t.height), (h, w));
    let got = decode(&t.bytes).unwrap().raster.data;
    let m = mad(&got, &want);
    eprintln!(
        "rotate90 of a libjpeg-turbo file vs ImageMagick's pixel rotation: mean {m:.3}, max {}",
        max_diff(&got, &want)
    );
    assert!(m <= 1.0, "mean {m}");
}

#[test]
#[ignore = "timing on a 12 MP image; run with --ignored --nocapture"]
fn timing_on_a_12_megapixel_image() {
    let mut s = JpegSpec::new(4000, 3000);
    s.sampling = (2, 2);
    let t0 = std::time::Instant::now();
    let src = s.build();
    eprintln!("encode fixture {:?} ({} bytes)", t0.elapsed(), src.len());
    for op in [
        Op::Rotate90,
        Op::FlipH,
        Op::Crop(Rect {
            x: 800,
            y: 800,
            w: 2000,
            h: 1500,
        }),
    ] {
        let t0 = std::time::Instant::now();
        let t = transform(&src, op, Policy::Snap, &limits()).unwrap();
        eprintln!("{op:?}: {:?} -> {} bytes", t0.elapsed(), t.bytes.len());
    }
}
