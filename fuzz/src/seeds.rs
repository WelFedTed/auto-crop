// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The seed corpus (ROADMAP M1.70): generated fixtures, the hostile-file corpus of M1.69 and the
//! `EditState` fixtures of the engine. Nothing binary is committed; `cargo run --manifest-path
//! fuzz/Cargo.toml --bin make-seeds -- fuzz/corpus` writes them, and `xtask/tests/fuzz_regressions.rs`
//! runs every one of them through its target on all three OSes so a seed that starts to fail (or to
//! take long) is caught without libFuzzer.

use crate::{CARRIERS, GENEROUS, MAX_INPUT, carrier, limits_input, metadata_input};
use auto_crop_codecs::fixtures::{self as fx, JpegSpec, TiffComp, TiffOpts};
use auto_crop_codecs::hostile;
use std::path::Path;

/// One seed: the target it belongs to, a file name and the bytes.
pub struct Seed {
    pub target: &'static str,
    pub name: String,
    pub bytes: Vec<u8>,
}

/// Valid and near-valid files of every format the codecs read or recognise.
pub fn sample_files() -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<(String, Vec<u8>)> = Vec::new();
    let mut add = |name: &str, bytes: Vec<u8>| v.push((name.to_owned(), bytes));

    add("jpeg-444", JpegSpec::new(24, 16).build());
    add(
        "jpeg-gray",
        JpegSpec {
            gray: true,
            ..JpegSpec::new(24, 16)
        }
        .build(),
    );
    add(
        "jpeg-420",
        JpegSpec {
            sampling: (2, 2),
            ..JpegSpec::new(40, 30)
        }
        .build(),
    );
    add(
        "jpeg-progressive",
        JpegSpec {
            progressive: true,
            ..JpegSpec::new(24, 16)
        }
        .build(),
    );
    add(
        "jpeg-restart",
        JpegSpec {
            restart: Some(2),
            ..JpegSpec::new(40, 16)
        }
        .build(),
    );
    add(
        "jpeg-optimized",
        JpegSpec {
            optimize: true,
            ..JpegSpec::new(24, 16)
        }
        .build(),
    );
    add(
        "jpeg-icc-exif",
        JpegSpec {
            icc: Some(fx::fake_icc(300)),
            exif_orientation: Some(6),
            ..JpegSpec::new(24, 16)
        }
        .build(),
    );
    add("jpeg-cmyk", fx::jpeg_cmyk(16, 16, false));
    add("jpeg-ycck", fx::jpeg_cmyk(16, 16, true));
    add(
        "jpeg-exif-cyclic",
        fx::jpeg_with_exif_blob(
            &fx::jpeg_baseline(16, 16),
            &fx::exif_blob_with(6, true, Some(8)),
        ),
    );

    add("png-rgb", fx::png_rgb(16, 16));
    add("png-rgba", fx::png_rgba(16, 16));
    add("png-gray", fx::png_gray(16, 16));
    add("png-rgb16", fx::png_rgb16(16, 16));
    add("png-palette", fx::png_palette(16, 16));
    add("png-adam7", fx::png_interlaced(17, 13));
    add("png-apng", fx::png_apng(8, 8));
    add("png-exif", fx::png_with_exif(16, 16, 3));
    add("png-icc", fx::png_with_icc(16, 16, &fx::fake_icc(500)));

    let t = TiffOpts::default();
    add("tiff-rgb", fx::tiff_rgb8(16, 16, &t));
    add("tiff-gray", fx::tiff_gray8(16, 16, &t));
    add("tiff-rgb16", fx::tiff_rgb16(16, 16, &t));
    add("tiff-gray16", fx::tiff_gray16(16, 16, &t));
    for (n, comp) in [
        ("lzw", TiffComp::Lzw),
        ("deflate", TiffComp::Deflate),
        ("packbits", TiffComp::PackBits),
    ] {
        add(
            &format!("tiff-{n}"),
            fx::tiff_rgb8(
                16,
                16,
                &TiffOpts {
                    comp: Some(comp),
                    ..Default::default()
                },
            ),
        );
    }
    add(
        "tiff-icc-orient-pages",
        fx::tiff_rgb8(
            16,
            16,
            &TiffOpts {
                orientation: Some(8),
                icc: Some(fx::fake_icc(200)),
                extra_pages: 2,
                ..Default::default()
            },
        ),
    );
    add(
        "tiff-big",
        fx::tiff_rgb8(
            16,
            16,
            &TiffOpts {
                big: true,
                ..Default::default()
            },
        ),
    );
    add("tiff-g4", fx::tiff_g4(32, 16));
    add("tiff-bilevel", fx::tiff_bilevel_raw(32, 16, true));

    add("webp-lossless", fx::webp_lossless(16, 16));
    add("webp-lossy-1x1", fx::webp_lossy_1x1());
    add("webp-exif", fx::webp_with_exif(16, 16, 6));
    add(
        "webp-icc-exif",
        fx::webp_extended(
            16,
            16,
            Some(&fx::exif_blob(8, true)),
            Some(&fx::fake_icc(120)),
        ),
    );
    add("webp-animated", fx::webp_animated(8, 8, 3));

    add("heic-stub", fx::heic_stub());
    add("avif-stub", fx::avif_stub());
    // HEIF and AVIF: the committed real files (header walk always; libheif and dav1d when the
    // fuzz build enables `auto-crop-codecs/heif`) and generated containers with transformations,
    // an ICC profile, extra items, a grid and a sequence, so the mutator starts from valid boxes.
    for (name, bytes) in fx::heif_real_files() {
        add(
            &format!("heif-real-{}", name.trim_end_matches(".avif")),
            bytes.to_vec(),
        );
    }
    let mut s = fx::HeifSpec::avif(100, 60);
    s.props = vec![fx::heif_clap(50, 40), fx::heif_irot(1), fx::heif_imir(0)];
    s.icc = Some(fx::fake_icc(300));
    add("heif-mock-avif-transforms-icc", s.build());
    let mut s = fx::HeifSpec::heic(1024, 1024);
    s.grid_tiles = Some(16);
    s.exif_len = Some(64);
    add("heif-mock-heic-grid-exif", s.build());
    let mut s = fx::HeifSpec::avif(64, 48);
    (s.extra_items, s.sequence) = (3, true);
    add("heif-mock-avif-items-sequence", s.build());
    add("gif-stub", fx::gif_stub());
    add("bmp-stub", fx::bmp_stub());
    add("jxl-stub", fx::jxl_stub());
    v
}

/// EXIF blobs for the `metadata` target (and raw EXIF seeds).
fn exif_blobs() -> Vec<(String, Vec<u8>)> {
    let mut v = Vec::new();
    for o in [1u16, 6, 8, 9] {
        for le in [true, false] {
            v.push((
                format!("exif-o{o}-{}", if le { "le" } else { "be" }),
                fx::exif_blob(o, le),
            ));
        }
    }
    v.push((
        "exif-cyclic".to_owned(),
        fx::exif_blob_with(3, true, Some(8)),
    ));
    v.push((
        "exif-offpage".to_owned(),
        fx::exif_blob_with(2, false, Some(0xFFFF_FF00)),
    ));
    v
}

/// ICC-like blobs: a plausible profile, and a two-chunk header (sequence 1 of 2).
fn icc_blobs() -> Vec<(String, Vec<u8>)> {
    let mut chunked = vec![1u8, 2];
    chunked.extend(fx::fake_icc(64));
    vec![
        ("icc-128".to_owned(), fx::fake_icc(128)),
        ("icc-2000".to_owned(), fx::fake_icc(2000)),
        ("icc-chunk-1of2".to_owned(), chunked),
        ("icc-empty".to_owned(), Vec::new()),
    ]
}

fn editstate_seeds() -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<(String, Vec<u8>)> = [
        ("empty-object", "{}"),
        ("version-only", r#"{"version":1}"#),
        ("too-new", r#"{"version":2,"items":[]}"#),
        ("version-zero", r#"{"version":0}"#),
        ("version-huge", r#"{"version":18446744073709551615}"#),
        ("early-slice-null", r#"{"version":1,"geometry":null}"#),
        ("not-an-object", "[1,2,3]"),
        ("deep", &"[".repeat(300)),
    ]
    .iter()
    .map(|(n, s)| ((*n).to_owned(), s.as_bytes().to_vec()))
    .collect();
    // The engine's fixture documents (current schema and the early slice).
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates/engine/fixtures/editstate");
    if let Ok(rd) = std::fs::read_dir(dir) {
        let mut files: Vec<_> = rd.filter_map(Result::ok).map(|e| e.path()).collect();
        files.sort();
        for p in files {
            if let (Some(stem), Ok(bytes)) = (p.file_stem(), std::fs::read(&p)) {
                v.push((format!("fixture-{}", stem.to_string_lossy()), bytes));
            }
        }
    }
    v
}

/// Every seed of every target.
pub fn seeds() -> Vec<Seed> {
    let mut out = Vec::new();
    let mut push = |target: &'static str, name: String, bytes: Vec<u8>| {
        if bytes.len() <= MAX_INPUT {
            out.push(Seed {
                target,
                name,
                bytes,
            });
        }
    };

    let mut files = sample_files();
    for h in hostile::corpus() {
        files.push((format!("hostile-{}", h.name), h.bytes));
    }
    for (name, bytes) in &files {
        push("probe", name.clone(), bytes.clone());
        push("limits", name.clone(), limits_input(GENEROUS, bytes));
    }
    // A few tight limit headers over the valid samples, so the cap checks are hit at the start.
    for (name, bytes) in sample_files().iter().take(12) {
        push(
            "limits",
            format!("{name}-tight"),
            limits_input([3, 3, 0, 0, 1, 6, 0, 0], bytes),
        );
    }

    for (bname, blob) in exif_blobs().into_iter().chain(icc_blobs()) {
        for kind in 0..CARRIERS {
            if carrier(kind, &blob).is_some() || kind == 0 {
                push(
                    "metadata",
                    format!("{bname}-k{kind}"),
                    metadata_input(kind, &blob),
                );
            }
        }
    }

    for (name, bytes) in editstate_seeds() {
        push("editstate_json", name, bytes);
    }
    out
}

/// Writes `seeds()` below `dir/<target>/`.
pub fn write_all(dir: &Path) -> std::io::Result<usize> {
    let all = seeds();
    for s in &all {
        let d = dir.join(s.target);
        std::fs::create_dir_all(&d)?;
        std::fs::write(d.join(&s.name), &s.bytes)?;
    }
    Ok(all.len())
}
