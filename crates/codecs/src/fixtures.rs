// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Generators for test images and hostile files (ROADMAP M1.12 to M1.17, M1.69).
//!
//! Everything is built from code: no third-party or large binary files are committed (PLAN 3.11,
//! B21). The encoders used here (`jpeg-encoder`, the `tiff` writer, `fax`, `flate2`) are different
//! implementations from the decoders under test, which is what makes a round trip meaningful.
//! Compiled for the crate's own tests and for `xtask` through the `fixtures` feature; never part of
//! a shipped build.

use std::io::{Cursor, Write};

/// Deterministic asymmetric RGB8 test pattern: red grows with x, green with y, and a bright white
/// 3x3 marker sits in the top-left corner, so every flip and turn changes the picture.
pub fn pattern(w: u32, h: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity(w as usize * h as usize * 3);
    for y in 0..h {
        for x in 0..w {
            let r = (x * 255 / w.max(2).saturating_sub(1)) as u8;
            let g = (y * 255 / h.max(2).saturating_sub(1)) as u8;
            let b = ((x + y) * 3 % 256) as u8;
            if x < 3 && y < 3 {
                v.extend_from_slice(&[255, 255, 255]);
            } else {
                v.extend_from_slice(&[r, g, b]);
            }
        }
    }
    v
}

/// Smooth variant of [`pattern`] (no marker, no modular wrap) for lossy codecs.
pub fn smooth(w: u32, h: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity(w as usize * h as usize * 3);
    for y in 0..h {
        for x in 0..w {
            v.push((x * 255 / w.max(2).saturating_sub(1)) as u8);
            v.push((y * 255 / h.max(2).saturating_sub(1)) as u8);
            v.push(((x + y) * 255 / (w + h).max(2).saturating_sub(2).max(1)) as u8);
        }
    }
    v
}

/// Deterministic photo-like RGB8: a gradient with soft colour variation, dark rectangles (text
/// and rules) and light noise.
pub fn photo(w: u32, h: u32, seed: u64) -> Vec<u8> {
    let mut s = seed | 1;
    let mut next = move || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        (s >> 33) as u32
    };
    let (wu, hu) = (w as usize, h as usize);
    let mut v = vec![0u8; wu * hu * 3];
    for y in 0..hu {
        for x in 0..wu {
            let base = 150.0
                + 50.0 * (x as f64 / 37.0).sin()
                + 30.0 * (y as f64 / 23.0).cos()
                + 40.0 * x as f64 / wu as f64;
            let i = (y * wu + x) * 3;
            v[i] = base.clamp(0.0, 255.0) as u8;
            v[i + 1] = (base * 0.93 + 8.0).clamp(0.0, 255.0) as u8;
            v[i + 2] = (base * 0.80 + 12.0).clamp(0.0, 255.0) as u8;
        }
    }
    for _ in 0..(wu * hu / 3000).max(6) {
        let rw = 3 + next() as usize % 40;
        let rh = 2 + next() as usize % 9;
        let x0 = next() as usize % wu.saturating_sub(rw).max(1);
        let y0 = next() as usize % hu.saturating_sub(rh).max(1);
        let ink = (20 + next() % 70) as u8;
        for y in y0..(y0 + rh).min(hu) {
            for x in x0..(x0 + rw).min(wu) {
                let i = (y * wu + x) * 3;
                v[i..i + 3].fill(ink);
            }
        }
    }
    for b in &mut v {
        let n = (next() % 7) as i32 - 3;
        *b = (i32::from(*b) + n).clamp(0, 255) as u8;
    }
    v
}

/// PSNR in dB between two equally long 8-bit buffers (infinite when identical).
pub fn psnr(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len());
    let sse: f64 = a
        .iter()
        .zip(b)
        .map(|(x, y)| {
            let d = f64::from(*x) - f64::from(*y);
            d * d
        })
        .sum();
    if sse == 0.0 {
        return f64::INFINITY;
    }
    10.0 * (255.0f64 * 255.0 / (sse / a.len() as f64)).log10()
}

/// The grey (luma) channel of [`pattern`], for single-channel fixtures.
pub fn pattern_gray(w: u32, h: u32) -> Vec<u8> {
    pattern(w, h)
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| {
            ((u32::from(p[0]) * 299 + u32::from(p[1]) * 587 + u32::from(p[2]) * 114) / 1000) as u8
        })
        .collect()
}

/// Reference for what EXIF orientation `o` (1..=8) means, written straight from the EXIF
/// definition and independent of any decoder: returns the displayed pixels and their size.
pub fn orient_reference(rgb: &[u8], w: u32, h: u32, o: u8) -> (Vec<u8>, u32, u32) {
    let (ow, oh) = if (5..=8).contains(&o) { (h, w) } else { (w, h) };
    let mut out = vec![0u8; rgb.len()];
    for y in 0..oh {
        for x in 0..ow {
            let (sx, sy) = match o {
                1 => (x, y),
                2 => (w - 1 - x, y),
                3 => (w - 1 - x, h - 1 - y),
                4 => (x, h - 1 - y),
                5 => (y, x),
                6 => (y, h - 1 - x),
                7 => (w - 1 - y, h - 1 - x),
                8 => (w - 1 - y, x),
                _ => panic!("orientation must be 1..=8"),
            };
            let s = (sy as usize * w as usize + sx as usize) * 3;
            let d = (y as usize * ow as usize + x as usize) * 3;
            out[d..d + 3].copy_from_slice(&rgb[s..s + 3]);
        }
    }
    (out, ow, oh)
}

/// A TIFF-structured EXIF blob (no `Exif\0\0` prefix) holding just the Orientation tag.
pub fn exif_blob(orientation: u16, le: bool) -> Vec<u8> {
    exif_blob_with(orientation, le, None)
}

/// [`exif_blob`] with an optional extra `ExifIFD` pointer tag (34665) set to `exif_ifd`, which a
/// hostile caller points back at IFD0 to make the structure cyclic.
pub fn exif_blob_with(orientation: u16, le: bool, exif_ifd: Option<u32>) -> Vec<u8> {
    let p16 = |v: &mut Vec<u8>, x: u16| {
        v.extend_from_slice(&if le { x.to_le_bytes() } else { x.to_be_bytes() })
    };
    let p32 = |v: &mut Vec<u8>, x: u32| {
        v.extend_from_slice(&if le { x.to_le_bytes() } else { x.to_be_bytes() })
    };
    let mut v = Vec::new();
    v.extend_from_slice(if le { b"II" } else { b"MM" });
    p16(&mut v, 42);
    p32(&mut v, 8);
    p16(&mut v, 1 + u16::from(exif_ifd.is_some()));
    p16(&mut v, 0x0112);
    p16(&mut v, 3);
    p32(&mut v, 1);
    p16(&mut v, orientation);
    p16(&mut v, 0);
    if let Some(off) = exif_ifd {
        p16(&mut v, 0x8769);
        p16(&mut v, 4);
        p32(&mut v, 1);
        p32(&mut v, off);
    }
    // Next-IFD pointer: 0 (callers patch it to make a loop).
    p32(&mut v, 0);
    v
}

/// A pseudo ICC profile: not a valid profile, but a deterministic byte string with the `acsp`
/// signature at its place, which is all that "kept byte-exact" needs.
pub fn fake_icc(len: usize) -> Vec<u8> {
    let mut v: Vec<u8> = (0..len)
        .map(|i| (i.wrapping_mul(2654435761) >> 13) as u8)
        .collect();
    if len >= 40 {
        v[..4].copy_from_slice(&(len as u32).to_be_bytes());
        v[36..40].copy_from_slice(b"acsp");
    }
    v
}

// ---------------------------------------------------------------------------------------------
// JPEG
// ---------------------------------------------------------------------------------------------

/// Which JPEG to build.
#[derive(Clone)]
pub struct JpegSpec {
    pub w: u32,
    pub h: u32,
    pub quality: u8,
    pub progressive: bool,
    pub scans: Option<u8>,
    /// Horizontal and vertical sampling factors of the luma component (1,1 = 4:4:4; 2,2 = 4:2:0).
    pub sampling: (u8, u8),
    pub gray: bool,
    pub restart: Option<u16>,
    pub optimize: bool,
    pub icc: Option<Vec<u8>>,
    pub exif_orientation: Option<u16>,
}

impl JpegSpec {
    pub fn new(w: u32, h: u32) -> Self {
        Self {
            w,
            h,
            quality: 92,
            progressive: false,
            scans: None,
            sampling: (1, 1),
            gray: false,
            restart: None,
            optimize: false,
            icc: None,
            exif_orientation: None,
        }
    }

    /// Encodes the [`smooth`] pattern (or its luma for `gray`).
    pub fn build(&self) -> Vec<u8> {
        let (data, ct) = if self.gray {
            (
                smooth(self.w, self.h)
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .map(|p| p[0] / 3 + p[1] / 3 + p[2] / 3)
                    .collect::<Vec<u8>>(),
                jpeg_encoder::ColorType::Luma,
            )
        } else {
            (smooth(self.w, self.h), jpeg_encoder::ColorType::Rgb)
        };
        self.encode(&data, ct)
    }

    /// Encodes caller-provided samples (`ct` decides the component count).
    pub fn encode(&self, data: &[u8], ct: jpeg_encoder::ColorType) -> Vec<u8> {
        let mut out = Vec::new();
        let mut e = jpeg_encoder::Encoder::new(&mut out, self.quality);
        if let Some(f) =
            jpeg_encoder::SamplingFactor::from_factors(self.sampling.0, self.sampling.1)
        {
            e.set_sampling_factor(f);
        }
        match (self.progressive, self.scans) {
            (true, Some(s)) => e.set_progressive_scans(s),
            (true, None) => e.set_progressive(true),
            _ => {}
        }
        if let Some(r) = self.restart {
            e.set_restart_interval(r);
        }
        e.set_optimized_huffman_tables(self.optimize);
        if let Some(icc) = &self.icc {
            e.add_icc_profile(icc).expect("icc fits");
        }
        if let Some(o) = self.exif_orientation {
            e.add_exif_metadata(&exif_blob(o, false))
                .expect("exif fits");
        }
        e.encode(data, self.w as u16, self.h as u16, ct)
            .expect("jpeg encode");
        out
    }
}

pub fn jpeg_baseline(w: u32, h: u32) -> Vec<u8> {
    JpegSpec::new(w, h).build()
}

/// CMYK (`ycck = false`) or YCCK JPEG with an Adobe marker, from a smooth 4-channel pattern.
pub fn jpeg_cmyk(w: u32, h: u32, ycck: bool) -> Vec<u8> {
    let s = smooth(w, h);
    let mut data = Vec::new();
    for p in s.as_chunks::<3>().0 {
        data.extend_from_slice(&[p[0], p[1], p[2], 20]);
    }
    let ct = if ycck {
        jpeg_encoder::ColorType::Ycck
    } else {
        jpeg_encoder::ColorType::Cmyk
    };
    JpegSpec::new(w, h).encode(&data, ct)
}

/// Rewrites the frame header of a JPEG: `(precision, height, width)`, and optionally the SOF marker
/// (e.g. 0xC1 for extended sequential, 0xC9 for arithmetic).
pub fn jpeg_patch_sof(
    jpeg: &[u8],
    precision: Option<u8>,
    dims: Option<(u16, u16)>,
    marker: Option<u8>,
) -> Vec<u8> {
    let mut v = jpeg.to_vec();
    let mut i = 2;
    while i + 4 <= v.len() {
        if v[i] != 0xFF {
            break;
        }
        let m = v[i + 1];
        let len = usize::from(u16::from_be_bytes([v[i + 2], v[i + 3]]));
        if matches!(m, 0xC0..=0xC2) {
            if let Some(p) = precision {
                v[i + 4] = p;
            }
            if let Some((w, h)) = dims {
                v[i + 5..i + 7].copy_from_slice(&h.to_be_bytes());
                v[i + 7..i + 9].copy_from_slice(&w.to_be_bytes());
            }
            if let Some(mk) = marker {
                v[i + 1] = mk;
            }
            return v;
        }
        i += 2 + len;
    }
    panic!("no SOF in fixture");
}

/// Inserts a segment (marker byte and payload, length added) right after SOI.
pub fn jpeg_insert_segment(jpeg: &[u8], marker: u8, payload: &[u8]) -> Vec<u8> {
    let mut v = jpeg[..2].to_vec();
    v.extend_from_slice(&[0xFF, marker]);
    v.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
    v.extend_from_slice(payload);
    v.extend_from_slice(&jpeg[2..]);
    v
}

/// Adds an `Exif\0\0` APP1 segment holding `blob` right after SOI.
pub fn jpeg_with_exif_blob(jpeg: &[u8], blob: &[u8]) -> Vec<u8> {
    let mut p = b"Exif\0\0".to_vec();
    p.extend_from_slice(blob);
    jpeg_insert_segment(jpeg, 0xE1, &p)
}

/// Repeats the first scan (SOS header and entropy data) `times` extra times before EOI: a scan bomb.
pub fn jpeg_repeat_scan(jpeg: &[u8], times: usize) -> Vec<u8> {
    let mut i = 2;
    let sos = loop {
        assert_eq!(jpeg[i], 0xFF);
        let m = jpeg[i + 1];
        if m == 0xDA {
            break i;
        }
        i += 2 + usize::from(u16::from_be_bytes([jpeg[i + 2], jpeg[i + 3]]));
    };
    let eoi = jpeg.len() - 2;
    let scan = &jpeg[sos..eoi];
    let mut v = jpeg[..eoi].to_vec();
    for _ in 0..times {
        v.extend_from_slice(scan);
    }
    v.extend_from_slice(&[0xFF, 0xD9]);
    v
}

// ---------------------------------------------------------------------------------------------
// PNG
// ---------------------------------------------------------------------------------------------

pub fn crc32(data: &[u8]) -> u32 {
    let mut c = flate2::Crc::new();
    c.update(data);
    c.sum()
}

pub fn zlib(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

/// One PNG chunk: length, type, data, CRC.
pub fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(data.len() + 12);
    v.extend_from_slice(&(data.len() as u32).to_be_bytes());
    v.extend_from_slice(kind);
    v.extend_from_slice(data);
    let mut crc = flate2::Crc::new();
    crc.update(kind);
    crc.update(data);
    v.extend_from_slice(&crc.sum().to_be_bytes());
    v
}

pub const PNG_SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// What PNG to write. `data` holds unpacked samples per pixel (bytes per pixel = channels x
/// depth/8; 16-bit samples big-endian), rows top to bottom.
#[derive(Clone)]
pub struct PngSpec {
    pub w: u32,
    pub h: u32,
    pub depth: u8,
    /// PNG colour type: 0 grey, 2 RGB, 3 palette, 4 grey+alpha, 6 RGBA.
    pub colour: u8,
    pub interlace: bool,
    pub data: Vec<u8>,
    pub palette: Option<Vec<u8>>,
    /// Chunks placed between IHDR and IDAT.
    pub before_idat: Vec<([u8; 4], Vec<u8>)>,
    /// Chunks placed after the last IDAT, before IEND.
    pub after_idat: Vec<([u8; 4], Vec<u8>)>,
}

impl PngSpec {
    pub fn rgb8(w: u32, h: u32) -> Self {
        Self {
            w,
            h,
            depth: 8,
            colour: 2,
            interlace: false,
            data: pattern(w, h),
            palette: None,
            before_idat: vec![],
            after_idat: vec![],
        }
    }

    fn bytes_per_pixel(&self) -> usize {
        let ch = match self.colour {
            0 | 3 => 1,
            4 => 2,
            2 => 3,
            _ => 4,
        };
        ch * usize::from(self.depth / 8)
    }

    /// Filtered scanlines (filter type 0 on every row), Adam7 passes when interlaced.
    pub fn raw_scanlines(&self) -> Vec<u8> {
        let bpp = self.bytes_per_pixel();
        let (w, h) = (self.w as usize, self.h as usize);
        let mut out = Vec::new();
        let passes: &[(usize, usize, usize, usize)] = if self.interlace {
            &[
                (0, 0, 8, 8),
                (4, 0, 8, 8),
                (0, 4, 4, 8),
                (2, 0, 4, 4),
                (0, 2, 2, 4),
                (1, 0, 2, 2),
                (0, 1, 1, 2),
            ]
        } else {
            &[(0, 0, 1, 1)]
        };
        for &(x0, y0, dx, dy) in passes {
            let mut y = y0;
            while y < h {
                if x0 < w {
                    out.push(0);
                    let mut x = x0;
                    while x < w {
                        let s = (y * w + x) * bpp;
                        out.extend_from_slice(&self.data[s..s + bpp]);
                        x += dx;
                    }
                }
                y += dy;
            }
        }
        out
    }

    pub fn build(&self) -> Vec<u8> {
        let mut v = PNG_SIG.to_vec();
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&self.w.to_be_bytes());
        ihdr.extend_from_slice(&self.h.to_be_bytes());
        ihdr.extend_from_slice(&[self.depth, self.colour, 0, 0, u8::from(self.interlace)]);
        v.extend(png_chunk(b"IHDR", &ihdr));
        if let Some(p) = &self.palette {
            v.extend(png_chunk(b"PLTE", p));
        }
        for (k, d) in &self.before_idat {
            v.extend(png_chunk(k, d));
        }
        v.extend(png_chunk(b"IDAT", &zlib(&self.raw_scanlines())));
        for (k, d) in &self.after_idat {
            v.extend(png_chunk(k, d));
        }
        v.extend(png_chunk(b"IEND", &[]));
        v
    }
}

pub fn png_rgb(w: u32, h: u32) -> Vec<u8> {
    PngSpec::rgb8(w, h).build()
}

pub fn png_rgba(w: u32, h: u32) -> Vec<u8> {
    let mut s = PngSpec::rgb8(w, h);
    s.colour = 6;
    s.data = pattern(w, h)
        .as_chunks::<3>()
        .0
        .iter()
        .enumerate()
        .flat_map(|(i, p)| [p[0], p[1], p[2], if i % 5 == 0 { 128 } else { 255 }])
        .collect();
    s.build()
}

pub fn png_gray(w: u32, h: u32) -> Vec<u8> {
    let mut s = PngSpec::rgb8(w, h);
    s.colour = 0;
    s.data = pattern_gray(w, h);
    s.build()
}

pub fn png_rgb16(w: u32, h: u32) -> Vec<u8> {
    let mut s = PngSpec::rgb8(w, h);
    s.depth = 16;
    // 16-bit samples whose high byte is the 8-bit pattern value and whose low byte differs.
    s.data = pattern(w, h)
        .iter()
        .flat_map(|&b| [b, b.wrapping_add(7)])
        .collect();
    s.build()
}

pub fn png_palette(w: u32, h: u32) -> Vec<u8> {
    let mut s = PngSpec::rgb8(w, h);
    s.colour = 3;
    let pal: Vec<u8> = (0..=255u8).flat_map(|i| [i, 255 - i, i / 2]).collect();
    s.palette = Some(pal);
    s.data = (0..w * h).map(|i| (i % 256) as u8).collect();
    s.build()
}

pub fn png_interlaced(w: u32, h: u32) -> Vec<u8> {
    let mut s = PngSpec::rgb8(w, h);
    s.interlace = true;
    s.build()
}

/// Two-frame APNG: IDAT is the default image and frame 0; a second frame follows as `fdAT`.
pub fn png_apng(w: u32, h: u32) -> Vec<u8> {
    let fctl = |seq: u32| -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&seq.to_be_bytes());
        d.extend_from_slice(&w.to_be_bytes());
        d.extend_from_slice(&h.to_be_bytes());
        d.extend_from_slice(&[0; 8]); // x, y offsets
        d.extend_from_slice(&[0, 1, 0, 10, 0, 0]); // delay 1/10, dispose none, blend source
        d
    };
    let mut s = PngSpec::rgb8(w, h);
    let mut actl = Vec::new();
    actl.extend_from_slice(&2u32.to_be_bytes());
    actl.extend_from_slice(&0u32.to_be_bytes());
    s.before_idat = vec![(*b"acTL", actl), (*b"fcTL", fctl(0))];
    let mut frame2 = 2u32.to_be_bytes().to_vec();
    let second = PngSpec::rgb8(w, h);
    let mut flipped = second.clone();
    flipped.data = flipped.data.iter().map(|b| 255 - b).collect();
    frame2.extend(zlib(&flipped.raw_scanlines()));
    s.after_idat = vec![(*b"fcTL", fctl(1)), (*b"fdAT", frame2)];
    s.build()
}

pub fn png_with_exif(w: u32, h: u32, orientation: u16) -> Vec<u8> {
    let mut s = PngSpec::rgb8(w, h);
    s.before_idat
        .push((*b"eXIf", exif_blob(orientation, false)));
    s.build()
}

/// A PNG whose `iCCP` chunk holds `profile` (name "t", zlib method 0).
pub fn png_with_icc(w: u32, h: u32, profile: &[u8]) -> Vec<u8> {
    let mut s = PngSpec::rgb8(w, h);
    let mut d = b"t\0\0".to_vec();
    d.extend(zlib(profile));
    s.before_idat.push((*b"iCCP", d));
    s.build()
}

// ---------------------------------------------------------------------------------------------
// TIFF
// ---------------------------------------------------------------------------------------------

/// Compression for [`tiff_rgb8`] and friends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TiffComp {
    None,
    Lzw,
    Deflate,
    PackBits,
}

fn tiff_comp(c: TiffComp) -> tiff::encoder::Compression {
    use tiff::encoder::{Compression, DeflateLevel};
    match c {
        TiffComp::None => Compression::Uncompressed,
        TiffComp::Lzw => Compression::Lzw,
        TiffComp::Deflate => Compression::Deflate(DeflateLevel::Balanced),
        TiffComp::PackBits => Compression::Packbits,
    }
}

/// Options shared by the valid TIFF builders.
#[derive(Clone, Default)]
pub struct TiffOpts {
    pub orientation: Option<u16>,
    pub icc: Option<Vec<u8>>,
    /// Extra identical pages after the first.
    pub extra_pages: u32,
    pub big: bool,
    pub comp: Option<TiffComp>,
}

fn tiff_pages<K, C>(
    mut enc: tiff::encoder::TiffEncoder<&mut Cursor<Vec<u8>>, K>,
    w: u32,
    h: u32,
    data: &[C::Inner],
    o: &TiffOpts,
) where
    K: tiff::encoder::TiffKind,
    C: tiff::encoder::colortype::ColorType,
    [C::Inner]: tiff::encoder::TiffValue,
{
    for page in 0..=o.extra_pages {
        let mut img = enc.new_image::<C>(w, h).unwrap();
        if page == 0 {
            if let Some(or) = o.orientation {
                img.encoder()
                    .write_tag(tiff::tags::Tag::Orientation, or)
                    .unwrap();
            }
            if let Some(icc) = &o.icc {
                img.encoder()
                    .write_tag(tiff::tags::Tag::IccProfile, &icc[..])
                    .unwrap();
            }
        }
        img.write_data(data).unwrap();
    }
}

fn tiff_write<C: tiff::encoder::colortype::ColorType>(
    w: u32,
    h: u32,
    data: &[C::Inner],
    o: &TiffOpts,
) -> Vec<u8>
where
    [C::Inner]: tiff::encoder::TiffValue,
{
    let mut out = Cursor::new(Vec::new());
    let comp = tiff_comp(o.comp.unwrap_or(TiffComp::None));
    if o.big {
        let enc = tiff::encoder::TiffEncoder::new_big(&mut out)
            .unwrap()
            .with_compression(comp);
        tiff_pages::<_, C>(enc, w, h, data, o);
    } else {
        let enc = tiff::encoder::TiffEncoder::new(&mut out)
            .unwrap()
            .with_compression(comp);
        tiff_pages::<_, C>(enc, w, h, data, o);
    }
    out.into_inner()
}

pub fn tiff_rgb8(w: u32, h: u32, o: &TiffOpts) -> Vec<u8> {
    tiff_write::<tiff::encoder::colortype::RGB8>(w, h, &pattern(w, h), o)
}

pub fn tiff_gray8(w: u32, h: u32, o: &TiffOpts) -> Vec<u8> {
    tiff_write::<tiff::encoder::colortype::Gray8>(w, h, &pattern_gray(w, h), o)
}

/// 16-bit RGB whose 8-bit reduction (high byte) is [`pattern`].
pub fn tiff_rgb16(w: u32, h: u32, o: &TiffOpts) -> Vec<u8> {
    let d: Vec<u16> = pattern(w, h)
        .iter()
        .map(|&b| u16::from(b) << 8 | 0x37)
        .collect();
    tiff_write::<tiff::encoder::colortype::RGB16>(w, h, &d, o)
}

pub fn tiff_gray16(w: u32, h: u32, o: &TiffOpts) -> Vec<u8> {
    let d: Vec<u16> = pattern_gray(w, h)
        .iter()
        .map(|&b| u16::from(b) << 8 | 0x37)
        .collect();
    tiff_write::<tiff::encoder::colortype::Gray16>(w, h, &d, o)
}

/// The bilevel pattern used by the 1-bit fixtures: true = black.
pub fn bilevel(w: u32, h: u32) -> Vec<bool> {
    (0..h)
        .flat_map(|y| (0..w).map(move |x| (x / 3 + y / 2) % 2 == 0 || x == y))
        .collect()
}

/// A 1-bit Group 4 (CCITT T.6) TIFF built with the `fax` encoder; PhotometricInterpretation is
/// WhiteIsZero, so `true` pixels of [`bilevel`] are black.
pub fn tiff_g4(w: u32, h: u32) -> Vec<u8> {
    use fax::{Color, VecWriter, encoder::Encoder};
    let bits = bilevel(w, h);
    let mut enc = Encoder::new(VecWriter::new());
    for row in bits.chunks(w as usize) {
        enc.encode_line(
            row.iter()
                .map(|&b| if b { Color::Black } else { Color::White }),
            w as u16,
        )
        .unwrap();
    }
    let data = enc.finish().unwrap().finish();
    fax::tiff::wrap(&data, w, h)
}

/// An uncompressed 1-bit TIFF; `white_is_zero` picks the photometric interpretation.
pub fn tiff_bilevel_raw(w: u32, h: u32, white_is_zero: bool) -> Vec<u8> {
    let bits = bilevel(w, h);
    let stride = (w as usize).div_ceil(8);
    let mut data = vec![0u8; stride * h as usize];
    for y in 0..h as usize {
        for x in 0..w as usize {
            // `true` = black. WhiteIsZero stores black as 1; BlackIsZero stores black as 0.
            let bit = bits[y * w as usize + x] == white_is_zero;
            if bit {
                data[y * stride + x / 8] |= 0x80 >> (x % 8);
            }
        }
    }
    let photometric = if white_is_zero { 0 } else { 1 };
    tiff_classic(
        true,
        &[
            (256, 4, 1, w),
            (257, 4, 1, h),
            (258, 3, 1, 1),
            (259, 3, 1, 1),
            (262, 3, 1, photometric),
            (273, 4, 1, 0), // patched to the data offset below
            (277, 3, 1, 1),
            (278, 4, 1, h),
            (279, 4, 1, data.len() as u32),
        ],
        0,
        &data,
        Some(5),
    )
}

/// Builds a classic TIFF with one IFD whose entries are all inline `(tag, type, count, value)`.
/// `tail` is appended after the IFD; `strip_entry` is the index of a StripOffsets entry whose value
/// is rewritten to point at `tail`. `next` is the next-IFD pointer.
pub fn tiff_classic(
    le: bool,
    entries: &[(u16, u16, u32, u32)],
    next: u32,
    tail: &[u8],
    strip_entry: Option<usize>,
) -> Vec<u8> {
    let p16 = |v: &mut Vec<u8>, x: u16| {
        v.extend_from_slice(&if le { x.to_le_bytes() } else { x.to_be_bytes() })
    };
    let p32 = |v: &mut Vec<u8>, x: u32| {
        v.extend_from_slice(&if le { x.to_le_bytes() } else { x.to_be_bytes() })
    };
    let mut v = Vec::new();
    v.extend_from_slice(if le { b"II" } else { b"MM" });
    p16(&mut v, 42);
    p32(&mut v, 8);
    p16(&mut v, entries.len() as u16);
    let data_at = 8 + 2 + entries.len() as u32 * 12 + 4;
    for (i, &(tag, ty, count, val)) in entries.iter().enumerate() {
        p16(&mut v, tag);
        p16(&mut v, ty);
        p32(&mut v, count);
        let val = if Some(i) == strip_entry { data_at } else { val };
        // SHORT values live in the first two bytes of the field.
        if ty == 3 && count == 1 {
            p16(&mut v, val as u16);
            p16(&mut v, 0);
        } else {
            p32(&mut v, val);
        }
    }
    p32(&mut v, next);
    v.extend_from_slice(tail);
    v
}

// ---------------------------------------------------------------------------------------------
// WebP
// ---------------------------------------------------------------------------------------------

fn riff_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut v = kind.to_vec();
    v.extend_from_slice(&(data.len() as u32).to_le_bytes());
    v.extend_from_slice(data);
    if data.len() % 2 == 1 {
        v.push(0);
    }
    v
}

fn riff(chunks: &[Vec<u8>]) -> Vec<u8> {
    let body: Vec<u8> = chunks.concat();
    let mut v = b"RIFF".to_vec();
    v.extend_from_slice(&((body.len() + 4) as u32).to_le_bytes());
    v.extend_from_slice(b"WEBP");
    v.extend(body);
    v
}

/// A lossless VP8L WebP of [`pattern`], from the `image` crate's encoder.
pub fn webp_lossless(w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    image::codecs::webp::WebPEncoder::new_lossless(&mut out)
        .encode(&pattern(w, h), w, h, image::ExtendedColorType::Rgb8)
        .unwrap();
    out
}

/// The payload of the first `VP8L` chunk of a simple lossless file.
fn vp8l_payload(webp: &[u8]) -> Vec<u8> {
    assert_eq!(&webp[12..16], b"VP8L");
    let len = u32::from_le_bytes(webp[16..20].try_into().unwrap()) as usize;
    webp[20..20 + len].to_vec()
}

fn vp8x(flags: u8, w: u32, h: u32) -> Vec<u8> {
    let mut d = vec![flags, 0, 0, 0];
    d.extend_from_slice(&(w - 1).to_le_bytes()[..3]);
    d.extend_from_slice(&(h - 1).to_le_bytes()[..3]);
    riff_chunk(b"VP8X", &d)
}

/// Extended-format WebP (VP8X) around the lossless image with optional metadata chunks.
pub fn webp_extended(w: u32, h: u32, exif: Option<&[u8]>, icc: Option<&[u8]>) -> Vec<u8> {
    let payload = vp8l_payload(&webp_lossless(w, h));
    let mut flags = 0u8;
    let mut chunks = vec![];
    if icc.is_some() {
        flags |= 0x20;
    }
    if exif.is_some() {
        flags |= 0x08;
    }
    chunks.push(vp8x(flags, w, h));
    if let Some(i) = icc {
        chunks.push(riff_chunk(b"ICCP", i));
    }
    chunks.push(riff_chunk(b"VP8L", &payload));
    if let Some(e) = exif {
        chunks.push(riff_chunk(b"EXIF", e));
    }
    riff(&chunks)
}

pub fn webp_with_exif(w: u32, h: u32, orientation: u16) -> Vec<u8> {
    webp_extended(w, h, Some(&exif_blob(orientation, true)), None)
}

/// An animated WebP with `frames` copies of the lossless image.
pub fn webp_animated(w: u32, h: u32, frames: u32) -> Vec<u8> {
    let payload = vp8l_payload(&webp_lossless(w, h));
    let mut chunks = vec![vp8x(0x02, w, h)];
    chunks.push(riff_chunk(b"ANIM", &[0, 0, 0, 0, 0, 0]));
    for _ in 0..frames {
        let mut d = vec![0u8; 6]; // x, y offsets
        d.extend_from_slice(&(w - 1).to_le_bytes()[..3]);
        d.extend_from_slice(&(h - 1).to_le_bytes()[..3]);
        d.extend_from_slice(&[100, 0, 0]); // duration
        d.push(0); // flags
        d.extend(riff_chunk(b"VP8L", &payload));
        chunks.push(riff_chunk(b"ANMF", &d));
    }
    riff(&chunks)
}

/// A 1x1 lossy (VP8) WebP: 44 bytes, so the lossy decode path runs in tests. These bytes are the
/// widely circulated minimal-WebP feature-detection image (a trivial 1x1 picture, no creative
/// content).
pub fn webp_lossy_1x1() -> Vec<u8> {
    const HEX: &str = "52494646220000005745425056503820160000003001009d012a01000100\
0ec0fe25a400037000000000";
    (0..HEX.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&HEX[i..i + 2], 16).unwrap())
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Recognised-only formats (header stubs for the sniffer)
// ---------------------------------------------------------------------------------------------

pub fn heic_stub() -> Vec<u8> {
    ftyp(b"heic", &[b"mif1", b"heic"])
}

pub fn avif_stub() -> Vec<u8> {
    ftyp(b"avif", &[b"mif1", b"miaf"])
}

pub fn ftyp(major: &[u8; 4], compat: &[&[u8; 4]]) -> Vec<u8> {
    let mut v = ((16 + 4 * compat.len()) as u32).to_be_bytes().to_vec();
    v.extend_from_slice(b"ftyp");
    v.extend_from_slice(major);
    v.extend_from_slice(&[0; 4]);
    for c in compat {
        v.extend_from_slice(*c);
    }
    v
}

// ---------------------------------------------------------------------------------------------
// HEIF and AVIF: generated header-only files (no coded image data) and the committed real files
// ---------------------------------------------------------------------------------------------

/// The committed AVIF fixtures of `tests/fixtures/heif/` (made by `make_heif_fixtures.py` with
/// Pillow, see docs/provenance.md): name and bytes.
pub fn heif_real_files() -> Vec<(&'static str, &'static [u8])> {
    macro_rules! f {
        ($n:literal) => {
            (
                $n,
                &include_bytes!(concat!("../tests/fixtures/heif/", $n))[..],
            )
        };
    }
    vec![
        f!("gradient-444-48x32.avif"),
        f!("orient2-444-48x32.avif"),
        f!("orient3-444-48x32.avif"),
        f!("orient4-444-48x32.avif"),
        f!("orient5-444-48x32.avif"),
        f!("orient6-444-48x32.avif"),
        f!("orient7-444-48x32.avif"),
        f!("orient8-444-48x32.avif"),
        f!("gradient-420-64x48.avif"),
        f!("gray-32x24.avif"),
        f!("alpha-32x24.avif"),
        f!("depth10-32x24.avif"),
        f!("icc-srgb-32x24.avif"),
        f!("seq-2frames-32x24.avif"),
        f!("exif-orient6-noirot-48x32.avif"),
    ]
}

fn bx(typ: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = ((8 + body.len()) as u32).to_be_bytes().to_vec();
    v.extend_from_slice(typ);
    v.extend_from_slice(body);
    v
}

fn full_box(typ: &[u8; 4], version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
    let mut b = vec![version];
    b.extend_from_slice(&flags.to_be_bytes()[1..]);
    b.extend_from_slice(body);
    bx(typ, &b)
}

/// An `irot` property: `quarter_turns` anticlockwise.
pub fn heif_irot(quarter_turns: u8) -> Vec<u8> {
    bx(b"irot", &[quarter_turns & 3])
}

/// An `imir` property: axis 0 flips top-bottom, 1 left-right (as libavif and libheif read it).
pub fn heif_imir(axis: u8) -> Vec<u8> {
    bx(b"imir", &[axis & 1])
}

/// A `clap` property with an integer clean-aperture size and no offset.
pub fn heif_clap(w: u32, h: u32) -> Vec<u8> {
    let mut b = Vec::new();
    for (n, d) in [(w, 1u32), (h, 1), (0, 1), (0, 1)] {
        b.extend_from_slice(&n.to_be_bytes());
        b.extend_from_slice(&d.to_be_bytes());
    }
    bx(b"clap", &b)
}

/// A HEIF or AVIF container made of boxes only: the structure of a real file (primary item,
/// properties, optional grid, extra items, Exif item) but no coded image, so it probes and fails
/// to decode with a typed error. Used for the hostile corpus and the header-walk tests.
#[derive(Debug, Clone)]
pub struct HeifSpec {
    /// Major brand: `avif` or `heic`.
    pub brand: [u8; 4],
    /// Item type of the image (and of grid tiles): `av01` or `hvc1`.
    pub codec: [u8; 4],
    pub width: u32,
    pub height: u32,
    /// Bits per channel in `pixi`.
    pub depth: u8,
    /// Properties after `ispe` and `pixi`, in the order the primary item applies them.
    pub props: Vec<Vec<u8>>,
    /// An ICC profile in a `colr` box.
    pub icc: Option<Vec<u8>>,
    /// Further top-level image items.
    pub extra_items: u32,
    /// `iinf` claims this many entries instead of the real count.
    pub iinf_claim: Option<u32>,
    /// A grid primary item (`grid`) over this many hidden tile items.
    pub grid_tiles: Option<u32>,
    /// An `Exif` item whose `iloc` extent claims this many bytes.
    pub exif_len: Option<u32>,
    /// Add a `moov` box (an image sequence).
    pub sequence: bool,
}

impl HeifSpec {
    pub fn avif(width: u32, height: u32) -> Self {
        Self {
            brand: *b"avif",
            codec: *b"av01",
            width,
            height,
            depth: 8,
            props: Vec::new(),
            icc: None,
            extra_items: 0,
            iinf_claim: None,
            grid_tiles: None,
            exif_len: None,
            sequence: false,
        }
    }

    pub fn heic(width: u32, height: u32) -> Self {
        Self {
            brand: *b"heic",
            codec: *b"hvc1",
            ..Self::avif(width, height)
        }
    }

    pub fn build(&self) -> Vec<u8> {
        let infe = |id: u32, typ: &[u8; 4], hidden: bool| {
            let mut b = (id as u16).to_be_bytes().to_vec();
            b.extend_from_slice(&[0, 0]);
            b.extend_from_slice(typ);
            b.push(0);
            full_box(b"infe", 2, u32::from(hidden), &b)
        };
        let primary_type = if self.grid_tiles.is_some() {
            *b"grid"
        } else {
            self.codec
        };
        let mut infes = infe(1, &primary_type, false);
        let mut next = 2u32;
        let tiles: Vec<u32> = (0..self.grid_tiles.unwrap_or(0))
            .map(|_| {
                let id = next;
                next += 1;
                infes.extend(infe(id, &self.codec, true));
                id
            })
            .collect();
        for _ in 0..self.extra_items {
            infes.extend(infe(next, &self.codec, false));
            next += 1;
        }
        let exif_id = self.exif_len.map(|_| {
            let id = next;
            next += 1;
            infes.extend(infe(id, b"Exif", false));
            id
        });
        let count = self.iinf_claim.unwrap_or(next - 1);
        let iinf = if count > u32::from(u16::MAX) {
            let mut b = count.to_be_bytes().to_vec();
            b.extend(&infes);
            full_box(b"iinf", 1, 0, &b)
        } else {
            let mut b = (count as u16).to_be_bytes().to_vec();
            b.extend(&infes);
            full_box(b"iinf", 0, 0, &b)
        };

        // properties: ispe, pixi, the caller's, colr
        let mut ipco = Vec::new();
        let mut ispe = self.width.to_be_bytes().to_vec();
        ispe.extend_from_slice(&self.height.to_be_bytes());
        ipco.extend(full_box(b"ispe", 0, 0, &ispe));
        ipco.extend(full_box(
            b"pixi",
            0,
            0,
            &[3, self.depth, self.depth, self.depth],
        ));
        let mut n_props = 2u8;
        for p in &self.props {
            ipco.extend(p);
            n_props += 1;
        }
        if let Some(icc) = &self.icc {
            let mut c = b"prof".to_vec();
            c.extend(icc);
            ipco.extend(bx(b"colr", &c));
            n_props += 1;
        }
        let mut ipma = 1u32.to_be_bytes().to_vec();
        ipma.extend_from_slice(&1u16.to_be_bytes());
        ipma.push(n_props);
        ipma.extend(1..=n_props);
        let iprp = bx(
            b"iprp",
            &[bx(b"ipco", &ipco), full_box(b"ipma", 0, 0, &ipma)].concat(),
        );

        let mut hdlr = vec![0u8; 4];
        hdlr.extend_from_slice(b"pict");
        hdlr.extend_from_slice(&[0; 13]); // reserved, empty name
        let mut meta = full_box(b"hdlr", 0, 0, &hdlr);
        meta.extend(full_box(b"pitm", 0, 0, &1u16.to_be_bytes()));
        meta.extend(iinf);
        if !tiles.is_empty() {
            let mut r = 1u16.to_be_bytes().to_vec();
            r.extend_from_slice(&(tiles.len() as u16).to_be_bytes());
            for t in &tiles {
                r.extend_from_slice(&(*t as u16).to_be_bytes());
            }
            meta.extend(full_box(b"iref", 0, 0, &bx(b"dimg", &r)));
        }
        meta.extend(iprp);
        if let (Some(id), Some(len)) = (exif_id, self.exif_len) {
            // iloc v0: offset_size 4, length_size 4, base_offset_size 0; one item
            let mut l = vec![0x44, 0x00];
            l.extend_from_slice(&1u16.to_be_bytes());
            l.extend_from_slice(&(id as u16).to_be_bytes());
            l.extend_from_slice(&[0, 0]); // data reference index
            l.extend_from_slice(&1u16.to_be_bytes());
            l.extend_from_slice(&0u32.to_be_bytes());
            l.extend_from_slice(&len.to_be_bytes());
            meta.extend(full_box(b"iloc", 0, 0, &l));
        }
        let mut out = ftyp(&self.brand, &[b"mif1", b"miaf"]);
        out.extend(full_box(b"meta", 0, 0, &meta));
        if self.sequence {
            out.extend(bx(b"moov", &[]));
        }
        out.extend(bx(b"mdat", &[0xAB; 32]));
        out
    }
}

pub fn gif_stub() -> Vec<u8> {
    let mut v = b"GIF89a".to_vec();
    v.extend_from_slice(&[1, 0, 1, 0, 0, 0, 0, 0x3B]);
    v
}

pub fn bmp_stub() -> Vec<u8> {
    let mut v = b"BM".to_vec();
    v.extend_from_slice(&[0; 12]);
    v.extend_from_slice(&40u32.to_le_bytes());
    v.extend_from_slice(&[0; 40]);
    v
}

pub fn jxl_stub() -> Vec<u8> {
    vec![0xFF, 0x0A, 0, 0, 0, 0]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orientation_reference_inverts_with_the_opposite_turn() {
        let (w, h) = (7, 5);
        let p = pattern(w, h);
        for (o, inv) in [(2, 2), (3, 3), (4, 4), (5, 5), (6, 8), (7, 7), (8, 6)] {
            let (d, dw, dh) = orient_reference(&p, w, h, o);
            let (back, bw, bh) = orient_reference(&d, dw, dh, inv);
            assert_eq!((bw, bh, &back), (w, h, &p), "orientation {o}");
        }
    }

    #[test]
    fn generators_produce_well_formed_files() {
        assert_eq!(
            crate::sniff(&jpeg_baseline(16, 16)),
            Some(crate::Format::Jpeg)
        );
        assert_eq!(crate::sniff(&png_rgb(8, 8)), Some(crate::Format::Png));
        assert_eq!(
            crate::sniff(&tiff_rgb8(8, 8, &TiffOpts::default())),
            Some(crate::Format::Tiff)
        );
        assert_eq!(
            crate::sniff(&webp_lossless(8, 8)),
            Some(crate::Format::Webp)
        );
        assert_eq!(crate::sniff(&heic_stub()), Some(crate::Format::Heic));
        assert_eq!(crate::sniff(&avif_stub()), Some(crate::Format::Avif));
        assert_eq!(crate::sniff(&gif_stub()), Some(crate::Format::Gif));
        assert_eq!(crate::sniff(&bmp_stub()), Some(crate::Format::Bmp));
        assert_eq!(crate::sniff(&jxl_stub()), Some(crate::Format::Jxl));
    }
}
