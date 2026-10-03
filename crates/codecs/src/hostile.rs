// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The hostile-file corpus (ROADMAP M1.69): bombs and corruptions generated from code, never
//! committed. `cargo xtask make-hostile` writes these to disk and decodes each one in a subprocess;
//! the in-crate test below checks every case in-process for a typed outcome and no panic.

use crate::fixtures::*;
use std::io::Write;

/// What a hostile file must do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expect {
    /// A typed error before any pixel buffer exists: within 1 s and 64 MiB of heap.
    Reject,
    /// A typed error or a decode, within `max_ms` and `max_mib` of heap (the decode peak excludes
    /// the input bytes).
    Bounded { max_ms: u64, max_mib: u64 },
}

pub struct Hostile {
    pub name: String,
    pub bytes: Vec<u8>,
    pub expect: Expect,
}

/// Heap bound for a decode that is allowed to succeed at the 100 MP default cap: the engine's
/// admission estimate, `pixels x 9 + 64 MiB`, rounded up.
pub const DEFAULT_CAP_MIB: u64 = 1024;

/// A PNG header declaring `w x h` 8-bit gray pixels over an 8 x 8 image's data.
pub fn png_claiming(w: u32, h: u32) -> Vec<u8> {
    let mut b = png_gray(8, 8);
    b[16..20].copy_from_slice(&w.to_be_bytes());
    b[20..24].copy_from_slice(&h.to_be_bytes());
    let crc = crc32(&b[12..29]);
    b[29..33].copy_from_slice(&crc.to_be_bytes());
    b
}

/// A zlib stream of `total` zero bytes, built in 1 MiB steps so the generator never holds it.
pub fn zlib_zeros(total: u64) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    let chunk = vec![0u8; 1 << 20];
    let mut left = total;
    while left > 0 {
        let n = left.min(chunk.len() as u64) as usize;
        e.write_all(&chunk[..n]).unwrap();
        left -= n as u64;
    }
    e.finish().unwrap()
}

fn png_from_idat(w: u32, h: u32, idat_zlib: &[u8], extra_before: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
    let mut v = PNG_SIG.to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 0, 0, 0, 0]);
    v.extend(png_chunk(b"IHDR", &ihdr));
    for (k, d) in extra_before {
        v.extend(png_chunk(k, d));
    }
    v.extend(png_chunk(b"IDAT", idat_zlib));
    v.extend(png_chunk(b"IEND", &[]));
    v
}

fn tiff_gray_entries(w: u32, h: u32, comp: u32, bytecount: u32) -> [(u16, u16, u32, u32); 9] {
    [
        (256, 4, 1, w),
        (257, 4, 1, h),
        (258, 3, 1, 8),
        (259, 3, 1, comp),
        (262, 3, 1, 1),
        (273, 4, 1, 0),
        (277, 3, 1, 1),
        (278, 4, 1, h),
        (279, 4, 1, bytecount),
    ]
}

fn tiff_gray_claiming(w: u32, h: u32) -> Vec<u8> {
    tiff_classic(true, &tiff_gray_entries(w, h, 1, 64), 0, &[0; 64], Some(5))
}

/// A classic TIFF made only of IFD structure: `ifds[i]` is the entry count of IFD `i`, chained in
/// order; the last IFD's next pointer is `last_next`.
fn tiff_ifd_chain(ifds: &[u16], last_next: u32) -> Vec<u8> {
    let mut v = b"II*\0".to_vec();
    v.extend_from_slice(&8u32.to_le_bytes());
    let mut off = 8u32;
    for (i, &n) in ifds.iter().enumerate() {
        let size = 2 + u32::from(n) * 12 + 4;
        v.extend_from_slice(&n.to_le_bytes());
        for e in 0..n {
            // Plausible junk tags (never the dimension tags): tag 60000+e, BYTE, count 1.
            v.extend_from_slice(&(60000u16.wrapping_add(e)).to_le_bytes());
            v.extend_from_slice(&1u16.to_le_bytes());
            v.extend_from_slice(&1u32.to_le_bytes());
            v.extend_from_slice(&[0, 0, 0, 0]);
        }
        let next = if i + 1 == ifds.len() {
            last_next
        } else {
            off + size
        };
        v.extend_from_slice(&next.to_le_bytes());
        off += size;
    }
    v
}

/// xorshift bytes after `prefix`.
fn noise(prefix: &[u8], len: usize, mut seed: u64) -> Vec<u8> {
    let mut v = prefix.to_vec();
    while v.len() < len {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        v.extend_from_slice(&seed.to_le_bytes());
    }
    v.truncate(len);
    v
}

/// EXIF blob whose IFD0 points at itself twice: as the next IFD and as the `ExifIFD` sub-directory.
fn exif_cyclic() -> Vec<u8> {
    let mut b = exif_blob_with(6, true, Some(8));
    let n = b.len();
    b[n - 4..].copy_from_slice(&8u32.to_le_bytes());
    b
}

/// A WebP whose chunk list is `chunks` (kind, payload) under a RIFF header.
fn webp_chunks(chunks: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let mut body = Vec::new();
    for (k, d) in chunks {
        body.extend_from_slice(*k);
        body.extend_from_slice(&(d.len() as u32).to_le_bytes());
        body.extend_from_slice(d);
        if d.len() % 2 == 1 {
            body.push(0);
        }
    }
    let mut v = b"RIFF".to_vec();
    v.extend_from_slice(&((body.len() + 4) as u32).to_le_bytes());
    v.extend_from_slice(b"WEBP");
    v.extend(body);
    v
}

fn vp8x_payload(flags: u8, w: u32, h: u32) -> Vec<u8> {
    let mut d = vec![flags, 0, 0, 0];
    d.extend_from_slice(&(w - 1).to_le_bytes()[..3]);
    d.extend_from_slice(&(h - 1).to_le_bytes()[..3]);
    d
}

/// The whole corpus. Generation is deterministic and takes a few seconds (the bombs are real zlib
/// streams of hundreds of MiB, a few hundred KiB once compressed).
pub fn corpus() -> Vec<Hostile> {
    let mut v: Vec<Hostile> = Vec::new();
    let mut add = |name: &str, bytes: Vec<u8>, expect: Expect| {
        v.push(Hostile {
            name: name.to_owned(),
            bytes,
            expect,
        })
    };
    let bounded = |max_ms, max_mib| Expect::Bounded { max_ms, max_mib };

    // ---- Declared sizes: refused from the header alone -------------------------------------
    add(
        "png-60000x60000",
        png_claiming(60_000, 60_000),
        Expect::Reject,
    );
    add(
        "png-100mp-plus-1",
        png_claiming(10_000, 10_001),
        Expect::Reject,
    );
    add(
        "png-65535x65535",
        png_claiming(65_535, 65_535),
        Expect::Reject,
    );
    add("png-width-4g", png_claiming(u32::MAX, 1), Expect::Reject);
    add(
        "tiff-60000x60000",
        tiff_gray_claiming(60_000, 60_000),
        Expect::Reject,
    );
    add(
        "tiff-65535x65535-bigmeta",
        tiff_gray_claiming(u32::MAX, u32::MAX),
        Expect::Reject,
    );
    let jpeg_gray = {
        let mut s = JpegSpec::new(16, 16);
        s.gray = true;
        s.build()
    };
    add(
        "jpeg-65535x65535",
        jpeg_patch_sof(&jpeg_gray, None, Some((65_535, 65_535)), None),
        Expect::Reject,
    );
    add(
        "jpeg-100mp-plus",
        jpeg_patch_sof(&jpeg_gray, None, Some((10_001, 10_000)), None),
        Expect::Reject,
    );
    // WebP: a VP8L header can declare 16384 x 16384 (268 MP); a VP8X canvas up to 2^24 x 2^24.
    let mut vp8l = vec![0x2F];
    vp8l.extend_from_slice(&(16_383u32 | 16_383 << 14).to_le_bytes());
    vp8l.extend_from_slice(&[0; 8]);
    add(
        "webp-vp8l-16384x16384",
        webp_chunks(&[(b"VP8L", vp8l)]),
        Expect::Reject,
    );
    add(
        "webp-vp8x-16m-canvas",
        webp_chunks(&[(b"VP8X", vp8x_payload(0, 1 << 24, 1 << 24))]),
        Expect::Reject,
    );

    // ---- Exactly at the cap: allowed to fail or decode, but bounded ---------------------------
    add(
        "png-100mp-header-truncated-data",
        png_claiming(10_000, 10_000),
        bounded(30_000, DEFAULT_CAP_MIB),
    );
    add(
        "tiff-100mp-header-truncated-data",
        tiff_gray_claiming(10_000, 10_000),
        bounded(30_000, DEFAULT_CAP_MIB),
    );
    add(
        "jpeg-100mp-header-tiny-scan",
        jpeg_patch_sof(&jpeg_gray, None, Some((10_000, 10_000)), None),
        bounded(60_000, DEFAULT_CAP_MIB),
    );
    // A legitimate 100 MP all-zero PNG: 100 MB inflated from about 100 KB. Must decode within the
    // memory estimate (this is the "zlib bomb that is still inside the cap").
    {
        let idat = zlib_zeros(10_001 * 10_000);
        add(
            "png-100mp-zeros-valid",
            png_from_idat(10_000, 10_000, &idat, &[]),
            bounded(60_000, DEFAULT_CAP_MIB),
        );
    }

    // ---- Decompression bombs ---------------------------------------------------------------
    {
        // 64 x 64 image whose IDAT inflates to 256 MiB.
        let idat = zlib_zeros(256 << 20);
        add(
            "png-zlib-bomb-excess-data",
            png_from_idat(64, 64, &idat, &[]),
            bounded(30_000, 64),
        );
        // A tiny image whose iCCP profile inflates to 256 MiB.
        let mut icc = b"t\0\0".to_vec();
        icc.extend(zlib_zeros(256 << 20));
        add(
            "png-iccp-bomb",
            png_from_idat(8, 8, &zlib(&[0u8; 72]), &[(*b"iCCP", icc)]),
            bounded(30_000, 64),
        );
        // A TIFF Deflate strip that inflates to 256 MiB for a 100 x 100 image.
        let strip = zlib_zeros(256 << 20);
        let entries = tiff_gray_entries(100, 100, 8, strip.len() as u32);
        add(
            "tiff-deflate-bomb",
            tiff_classic(true, &entries, 0, &strip, Some(5)),
            bounded(30_000, 64),
        );
    }

    // ---- Counts and floods -----------------------------------------------------------------
    {
        let mut apng = png_apng(8, 8);
        let at = apng.windows(4).position(|w| w == b"acTL").unwrap() + 4;
        apng[at..at + 4].copy_from_slice(&4_000_000_000u32.to_be_bytes());
        add("png-apng-4g-frames", apng, Expect::Reject);

        let mut many = png_rgb(8, 8);
        let iend = many.len() - 12;
        let tail = many.split_off(iend);
        for _ in 0..300_000 {
            many.extend(png_chunk(b"prVt", &[]));
        }
        many.extend(tail);
        add("png-300k-tiny-chunks", many, bounded(30_000, 64));

        let mut bad_len = png_rgb(8, 8);
        let idat = bad_len.windows(4).position(|w| w == b"IDAT").unwrap() - 4;
        bad_len[idat..idat + 4].copy_from_slice(&u32::MAX.to_be_bytes());
        add("png-chunk-length-4g", bad_len, Expect::Reject);
    }
    add(
        "jpeg-scan-bomb-500-scans",
        jpeg_repeat_scan(&jpeg_baseline(16, 16), 500),
        Expect::Reject,
    );
    {
        // 20,000 one-kilobyte APP and COM segments (20 MB).
        let base = jpeg_baseline(16, 16);
        let mut seg = Vec::new();
        for i in 0..20_000 {
            let marker = if i % 2 == 0 { 0xE5u8 } else { 0xFE };
            seg.extend_from_slice(&[0xFF, marker]);
            seg.extend_from_slice(&1026u16.to_be_bytes());
            seg.extend_from_slice(&[0x41; 1024]);
        }
        let mut out = base[..2].to_vec();
        out.extend(seg);
        out.extend_from_slice(&base[2..]);
        add("jpeg-20k-app-segments", out, bounded(30_000, 64));

        // 3,000 ICC segments all claiming to be part 1 of 3.
        let mut seg = Vec::new();
        for _ in 0..3_000 {
            seg.extend_from_slice(&[0xFF, 0xE2]);
            seg.extend_from_slice(&(2u16 + 14 + 1000).to_be_bytes());
            seg.extend_from_slice(b"ICC_PROFILE\0");
            seg.extend_from_slice(&[1, 3]);
            seg.extend_from_slice(&[0x55; 1000]);
        }
        let mut out = base[..2].to_vec();
        out.extend(seg);
        out.extend_from_slice(&base[2..]);
        add("jpeg-3k-duplicate-icc-segments", out, bounded(30_000, 64));
    }
    add(
        "tiff-ifd-65535-entries-no-dims",
        tiff_ifd_chain(&[65_535], 0),
        bounded(10_000, 64),
    );
    {
        // A valid 8 x 8 image whose IFD0 also carries 65,526 junk entries.
        let mut entries = tiff_gray_entries(8, 8, 1, 64).to_vec();
        entries.extend((0..65_526u32).map(|i| ((40_000 + i % 25_000) as u16, 1, 1, i & 0xFF)));
        let strip = entries.iter().position(|e| e.0 == 273);
        add(
            "tiff-ifd-65535-entries-valid-image",
            tiff_classic(true, &entries, 0, &[7; 64], strip),
            bounded(10_000, 64),
        );
    }
    add(
        "tiff-ifd-chain-20000",
        tiff_ifd_chain(&vec![0u16; 20_000], 0),
        Expect::Reject,
    );
    add(
        "tiff-ifd-self-cycle",
        tiff_ifd_chain(&[2], 8),
        Expect::Reject,
    );
    {
        // Two IFDs pointing at each other: IFD0 at 8 (0 entries, size 6), IFD1 at 14.
        let mut v = b"II*\0".to_vec();
        v.extend_from_slice(&8u32.to_le_bytes());
        v.extend_from_slice(&[0, 0]);
        v.extend_from_slice(&14u32.to_le_bytes());
        v.extend_from_slice(&[0, 0]);
        v.extend_from_slice(&8u32.to_le_bytes());
        add("tiff-ifd-two-cycle", v, Expect::Reject);
    }
    {
        // IFD0 of a real 8x8 TIFF plus an ExifIFD tag pointing back at IFD0.
        let mut entries: Vec<(u16, u16, u32, u32)> = vec![
            (256, 4, 1, 8),
            (257, 4, 1, 8),
            (258, 3, 1, 8),
            (259, 3, 1, 1),
            (262, 3, 1, 1),
            (273, 4, 1, 0),
            (277, 3, 1, 1),
            (278, 4, 1, 8),
            (279, 4, 1, 64),
            (34665, 4, 1, 8),
        ];
        entries.sort_by_key(|e| e.0);
        let strip = entries.iter().position(|e| e.0 == 273);
        add(
            "tiff-exif-ifd-cycle",
            tiff_classic(true, &entries, 0, &[7; 64], strip),
            bounded(10_000, 64),
        );
    }
    {
        let mut entries = tiff_gray_entries(8, 8, 1, 64).to_vec();
        entries.push((34675, 7, 0xFFFF_FFF0, 0));
        entries.sort_by_key(|e| e.0);
        let strip = entries.iter().position(|e| e.0 == 273);
        add(
            "tiff-icc-4g",
            tiff_classic(true, &entries, 0, &[7; 64], strip),
            Expect::Reject,
        );
        let mut entries = tiff_gray_entries(100, 100, 1, 0xFFFF_FFF0).to_vec();
        entries.sort_by_key(|e| e.0);
        let strip = entries.iter().position(|e| e.0 == 273);
        add(
            "tiff-strip-bytecount-4g",
            tiff_classic(true, &entries, 0, &[7; 64], strip),
            bounded(10_000, 64),
        );
    }
    {
        // BigTIFF claiming 2^40 IFD entries.
        let mut v = b"II+\0".to_vec();
        v.extend_from_slice(&8u16.to_le_bytes());
        v.extend_from_slice(&0u16.to_le_bytes());
        v.extend_from_slice(&16u64.to_le_bytes());
        v.extend_from_slice(&(1u64 << 40).to_le_bytes());
        add("bigtiff-2e40-entries", v, Expect::Reject);
    }
    {
        let mut many = Vec::new();
        for _ in 0..20_000 {
            let mut d = vec![0u8; 6];
            d.extend_from_slice(&[7, 0, 0, 7, 0, 0, 100, 0, 0, 0]);
            many.push((b"ANMF", d));
        }
        let mut chunks: Vec<(&[u8; 4], Vec<u8>)> = vec![(b"VP8X", vp8x_payload(0x02, 8, 8))];
        chunks.extend(many);
        add(
            "webp-anmf-20000-frames",
            webp_chunks(&chunks),
            Expect::Reject,
        );

        let mut lie = webp_lossless(16, 16);
        lie[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        add("webp-riff-size-4g", lie, bounded(10_000, 64));
    }

    // ---- Cyclic or flooded EXIF ------------------------------------------------------------
    add(
        "jpeg-exif-cyclic-ifd",
        jpeg_with_exif_blob(&jpeg_baseline(16, 16), &exif_cyclic()),
        bounded(10_000, 64),
    );
    {
        let mut flood = exif_blob(6, true);
        flood[8] = 0xFF;
        flood[9] = 0xFF;
        add(
            "jpeg-exif-65535-entries",
            jpeg_with_exif_blob(&jpeg_baseline(16, 16), &flood),
            bounded(10_000, 64),
        );
        let mut s = PngSpec::rgb8(8, 8);
        s.before_idat.push((*b"eXIf", exif_cyclic()));
        add("png-exif-cyclic-ifd", s.build(), bounded(10_000, 64));
        add(
            "webp-exif-cyclic-ifd",
            webp_extended(16, 16, Some(&exif_cyclic()), None),
            bounded(10_000, 64),
        );
    }

    // ---- Unsupported features: typed refusals ---------------------------------------------
    let j16 = jpeg_baseline(16, 16);
    add(
        "jpeg-12-bit",
        jpeg_patch_sof(&j16, Some(12), None, Some(0xC1)),
        Expect::Reject,
    );
    add(
        "jpeg-arithmetic-coded",
        jpeg_patch_sof(&j16, None, None, Some(0xC9)),
        Expect::Reject,
    );
    add("heic-header-only", heic_stub(), Expect::Reject);
    add("avif-header-only", avif_stub(), Expect::Reject);

    // ---- HEIF and AVIF (header walk always; libheif and dav1d with the `heif` feature) ------
    // Sizes the pixel cap must refuse from the header alone, before libheif sees the file.
    let spec = |w, h| HeifSpec::avif(w, h);
    add(
        "avif-ispe-60000x60000",
        spec(60_000, 60_000).build(),
        Expect::Reject,
    );
    add(
        "heic-ispe-65535x65535",
        HeifSpec::heic(65_535, 65_535).build(),
        Expect::Reject,
    );
    add(
        "avif-ispe-4g-x-4g",
        spec(u32::MAX, u32::MAX).build(),
        Expect::Reject,
    );
    add("avif-ispe-zero", spec(0, 0).build(), Expect::Reject);
    add(
        "avif-ispe-100mp-plus-one",
        spec(10_000, 10_001).build(),
        Expect::Reject,
    );
    add(
        "avif-clap-small-ispe-huge",
        {
            let mut s = spec(60_000, 60_000);
            s.props = vec![heif_clap(16, 16)];
            s.build()
        },
        Expect::Reject,
    );
    add(
        "avif-grid-canvas-60000x60000",
        {
            let mut s = spec(60_000, 60_000);
            s.grid_tiles = Some(4);
            s.build()
        },
        Expect::Reject,
    );
    // Item, entry and metadata floods.
    add(
        "avif-items-20000",
        {
            let mut s = spec(64, 48);
            s.extra_items = 20_000;
            s.build()
        },
        Expect::Reject,
    );
    add(
        "avif-iinf-claims-4g-entries",
        {
            let mut s = spec(64, 48);
            s.iinf_claim = Some(u32::MAX);
            s.build()
        },
        Expect::Reject,
    );
    add(
        "avif-exif-item-claims-4gb",
        {
            let mut s = spec(64, 48);
            s.exif_len = Some(u32::MAX);
            s.build()
        },
        Expect::Reject,
    );
    add(
        "avif-meta-box-size-lies",
        {
            let mut b = spec(64, 48).build();
            // ftyp is 24 bytes (one brand and two compatible ones); meta follows.
            b[24..28].copy_from_slice(&u32::MAX.to_be_bytes());
            b
        },
        Expect::Reject,
    );
    // Structure without coded data, and a grid of thousands of tiny tiles over a small canvas:
    // typed errors, no big allocation.
    add(
        "avif-structure-no-image-data",
        spec(64, 48).build(),
        bounded(10_000, 64),
    );
    add(
        "heic-structure-no-image-data",
        HeifSpec::heic(64, 48).build(),
        bounded(10_000, 64),
    );
    add(
        "avif-grid-4096-tiles",
        {
            let mut s = spec(1024, 1024);
            s.grid_tiles = Some(4096);
            s.build()
        },
        bounded(10_000, 64),
    );
    add(
        "heic-grid-4096-tiles",
        {
            let mut s = HeifSpec::heic(1024, 1024);
            s.grid_tiles = Some(4096);
            s.build()
        },
        bounded(10_000, 64),
    );
    add(
        "avif-sequence-structure",
        {
            let mut s = spec(64, 48);
            s.sequence = true;
            s.build()
        },
        bounded(10_000, 64),
    );
    {
        let real = heif_real_files();
        let get = |n: &str| {
            real.iter()
                .find(|(name, _)| *name == n)
                .map(|(_, b)| b.to_vec())
                .expect("fixture")
        };
        // The real files, truncated everywhere that matters.
        for (case, name) in [
            ("gradient", "gradient-444-48x32.avif"),
            ("depth10", "depth10-32x24.avif"),
            ("sequence", "seq-2frames-32x24.avif"),
        ] {
            let full = get(name);
            for (label, cut) in [
                ("3b", 3usize),
                ("20b", 20),
                ("10pct", full.len() / 10),
                ("50pct", full.len() / 2),
                ("90pct", full.len() * 9 / 10),
                ("99pct", full.len() - full.len() / 100 - 1),
                ("minus-1", full.len() - 1),
            ] {
                add(
                    &format!("avif-real-{case}-truncated-{label}"),
                    full[..cut.max(1)].to_vec(),
                    bounded(10_000, 64),
                );
            }
        }
    }
    add(
        "avif-magic-then-noise-4mb",
        noise(&ftyp(b"avif", &[b"mif1"]), 4 << 20, 0x2468_ACE0_1357_9BDF),
        bounded(10_000, 64),
    );
    add(
        "heic-magic-then-noise-4mb",
        noise(&ftyp(b"heic", &[b"mif1"]), 4 << 20, 0x1111_2222_3333_4444),
        bounded(10_000, 64),
    );

    // ---- Truncations and junk --------------------------------------------------------------
    for (name, full) in [
        ("jpeg", JpegSpec::new(128, 96).build()),
        ("jpeg-progressive", {
            let mut s = JpegSpec::new(128, 96);
            s.progressive = true;
            s.build()
        }),
        ("png", png_rgb(128, 96)),
        ("png-adam7", png_interlaced(128, 96)),
        (
            "tiff",
            tiff_rgb8(
                128,
                96,
                &TiffOpts {
                    comp: Some(TiffComp::Lzw),
                    ..TiffOpts::default()
                },
            ),
        ),
        ("webp", webp_lossless(128, 96)),
        ("webp-animated", webp_animated(32, 24, 3)),
    ] {
        for (label, cut) in [
            ("3b", 3usize),
            ("20b", 20),
            ("10pct", full.len() / 10),
            ("50pct", full.len() / 2),
            ("99pct", full.len() - full.len() / 100 - 1),
            ("minus-1", full.len() - 1),
        ] {
            add(
                &format!("{name}-truncated-{label}"),
                full[..cut.max(1)].to_vec(),
                bounded(10_000, 64),
            );
        }
    }
    add("empty-file", Vec::new(), Expect::Reject);
    add(
        "jpeg-magic-then-noise-4mb",
        noise(&[0xFF, 0xD8, 0xFF, 0xE0], 4 << 20, 0x9E37_79B9_7F4A_7C15),
        bounded(10_000, 64),
    );
    add(
        "png-magic-then-noise-4mb",
        noise(&PNG_SIG, 4 << 20, 0xDEAD_BEEF_1234_5678),
        bounded(10_000, 64),
    );
    add(
        "tiff-magic-then-noise-4mb",
        noise(b"II*\0\x08\0\0\0", 4 << 20, 0x1357_9BDF_2468_ACE0),
        bounded(10_000, 64),
    );
    add(
        "webp-magic-then-noise-4mb",
        noise(b"RIFF\0\0\x40\0WEBPVP8L", 4 << 20, 0x0F1E_2D3C_4B5A_6978),
        bounded(10_000, 64),
    );
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CodecError, DecodeLimits, decode_with};

    /// In-process pass over the cheap cases (the bombs run in subprocesses under `xtask`, where
    /// memory is measured): every outcome is a typed error or a decode, never a panic.
    #[test]
    fn every_cheap_case_is_a_typed_outcome_without_a_panic() {
        let mut n = 0;
        for h in corpus() {
            if h.bytes.len() > 8 << 20 || h.name.contains("100mp") || h.name.contains("bomb") {
                continue; // run by xtask with measurement
            }
            n += 1;
            let r = decode_with(&h.bytes, &DecodeLimits::default());
            if let Err(CodecError::InternalPanic(m)) = &r {
                panic!("{}: decoder panicked: {m}", h.name);
            }
            if h.expect == Expect::Reject {
                assert!(r.is_err(), "{} should be rejected", h.name);
            }
        }
        assert!(n > 40, "only {n} cases ran");
    }
}
