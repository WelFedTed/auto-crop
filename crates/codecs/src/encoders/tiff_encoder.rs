// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! TIFF writer (ROADMAP M11.29, M11.30 for one page, M7.29; PLAN 3.3 "TIFF" and 5.9).
//!
//! One writer covers every layout we produce, so a 1-bit Group 4 page and a colour page come out
//! of the same code and the same tag policy:
//!
//! * **Classic TIFF, little endian**, one image directory, strips written first and the directory
//!   last (the layout libtiff itself uses), so nothing is buffered twice. A file that would pass
//!   4 GiB is refused (`TooLarge`); BigTIFF is not written.
//! * **Samples**: 8 or 16 bit, grey (BlackIsZero) or RGB (chunky). No alpha: the engine's rasters
//!   carry none, and a writer that cannot hold content exactly says `UnsupportedOutput`.
//! * **Compression** ([`TiffCompression`]): none, LZW (`weezl`, TIFF flavour, early change), Deflate
//!   (zlib, tag 8), PackBits (per row, as the specification asks) and CCITT Group 4 for 1-bit output.
//!   LZW and Deflate use the horizontal predictor (tag 317 = 2) unless switched off, on 8 and on
//!   16-bit samples.
//! * **Strips**: about 256 KiB of raw samples each; a Group 4 page is one strip (best ratio) unless
//!   `rows_per_strip` says otherwise.
//! * **Tags**: Orientation is always 1 (the pixels were turned once, PLAN 3.6), the ICC profile is
//!   embedded byte-exact (tag 34675), the resolution comes from the source (inch units) and, when
//!   unknown, is `1/1` with unit "none": a dpi is never invented. `Software` says "Auto Crop"
//!   without a version (documented in `docs/dev/output-formats.md`). No EXIF, XMP, GPS or thumbnail
//!   is written, so nothing about the source's location can leak through a TIFF.
//! * **1-bit** ([`Bits::One`]): the picture is thresholded (`DEFAULT_CUT`, no dithering) and coded
//!   as Group 4 with PhotometricInterpretation WhiteIsZero and the Group4Options tag.

use super::bilevel::{BilevelImage, DEFAULT_CUT, G4_MAX_WIDTH};
use super::check;
use crate::RawTag;
use auto_crop_core::output::{Bilevel, Bits, OutFormat};
use auto_crop_core::ports::{EncodeMeta, Encoder, RasterView};
use auto_crop_core::{CancelToken, ErrKind, OutputSpec};
use std::io::Write;

/// The text of the `Software` tag.
pub const SOFTWARE: &str = "Auto Crop";
/// Raw sample bytes per strip, about (PLAN: ~256 KB).
const STRIP_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TiffCompression {
    /// Compression 1.
    None,
    /// Compression 5, with the horizontal predictor.
    #[default]
    Lzw,
    /// Compression 8 (zlib), with the horizontal predictor.
    Deflate,
    /// Compression 32773, per row.
    PackBits,
    /// Compression 4, 1-bit only (chosen by `Bits::One`, or here to force it).
    G4,
}

impl TiffCompression {
    fn tag(self) -> u16 {
        match self {
            TiffCompression::None => 1,
            TiffCompression::Lzw => 5,
            TiffCompression::Deflate => 8,
            TiffCompression::PackBits => 32773,
            TiffCompression::G4 => 4,
        }
    }
}

/// TIFF through the writer of this module.
#[derive(Debug, Clone)]
pub struct TiffEncoder {
    pub compression: TiffCompression,
    /// The horizontal predictor for LZW and Deflate.
    pub predictor: bool,
    /// Rows per strip; `None`: about 256 KiB of samples (a whole page for Group 4).
    pub rows_per_strip: Option<u32>,
    /// The cut of the 1-bit threshold: luma below it is black.
    pub bilevel_cut: u8,
    /// Descriptive tags of the source carried over (text, XMP, IPTC), little-endian values; see
    /// the policy functions of the engine for which ones a save keeps.
    pub extra: Vec<RawTag>,
}

impl Default for TiffEncoder {
    fn default() -> Self {
        Self {
            compression: TiffCompression::Lzw,
            predictor: true,
            rows_per_strip: None,
            bilevel_cut: DEFAULT_CUT,
            extra: Vec::new(),
        }
    }
}

// TIFF field types.
const ASCII: u16 = 2;
const SHORT: u16 = 3;
const LONG: u16 = 4;
const RATIONAL: u16 = 5;
const UNDEFINED: u16 = 7;

struct Entry {
    tag: u16,
    typ: u16,
    count: u32,
    data: Vec<u8>,
}

fn short(tag: u16, v: u16) -> Entry {
    Entry {
        tag,
        typ: SHORT,
        count: 1,
        data: v.to_le_bytes().to_vec(),
    }
}

fn long(tag: u16, v: u32) -> Entry {
    Entry {
        tag,
        typ: LONG,
        count: 1,
        data: v.to_le_bytes().to_vec(),
    }
}

fn rational(tag: u16, num: u32, den: u32) -> Entry {
    let mut data = num.to_le_bytes().to_vec();
    data.extend_from_slice(&den.to_le_bytes());
    Entry {
        tag,
        typ: RATIONAL,
        count: 1,
        data,
    }
}

fn longs(tag: u16, v: &[u32]) -> Entry {
    Entry {
        tag,
        typ: LONG,
        count: v.len() as u32,
        data: v.iter().flat_map(|x| x.to_le_bytes()).collect(),
    }
}

/// PackBits of one row (TIFF 6.0 section 9): literal runs of up to 128 bytes, repeat runs of 2 to
/// 128 equal bytes.
pub(crate) fn packbits_row(row: &[u8], out: &mut Vec<u8>) {
    let n = row.len();
    let mut i = 0;
    while i < n {
        // A run of equal bytes starting here?
        let mut run = 1;
        while i + run < n && row[i + run] == row[i] && run < 128 {
            run += 1;
        }
        if run >= 2 {
            out.push((257 - run) as u8); // -(run-1) as i8
            out.push(row[i]);
            i += run;
            continue;
        }
        // A literal stretch up to the next run of 3 (a run of 2 inside a literal is cheaper to
        // keep in the literal), at most 128 bytes.
        let start = i;
        i += 1;
        while i < n && i - start < 128 {
            if i + 2 < n && row[i] == row[i + 1] && row[i + 1] == row[i + 2] {
                break;
            }
            i += 1;
        }
        out.push((i - start - 1) as u8);
        out.extend_from_slice(&row[start..i]);
    }
}

/// The horizontal predictor on one row of `spp` interleaved samples, in place.
fn predict_row(row: &mut [u8], spp: usize, sixteen: bool) {
    if sixteen {
        let n = row.len() / 2;
        for i in (spp..n).rev() {
            let cur = u16::from_le_bytes([row[2 * i], row[2 * i + 1]]);
            let prev = u16::from_le_bytes([row[2 * (i - spp)], row[2 * (i - spp) + 1]]);
            row[2 * i..2 * i + 2].copy_from_slice(&cur.wrapping_sub(prev).to_le_bytes());
        }
    } else {
        for i in (spp..row.len()).rev() {
            row[i] = row[i].wrapping_sub(row[i - spp]);
        }
    }
}

fn lzw(data: &[u8]) -> Result<Vec<u8>, ErrKind> {
    weezl::encode::Encoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8)
        .encode(data)
        .map_err(|_| ErrKind::EncodeFailed)
}

fn deflate(data: &[u8]) -> Result<Vec<u8>, ErrKind> {
    let mut z = flate2::write::ZlibEncoder::new(
        Vec::with_capacity(data.len() / 2 + 64),
        flate2::Compression::default(),
    );
    z.write_all(data).map_err(|_| ErrKind::EncodeFailed)?;
    z.finish().map_err(|_| ErrKind::EncodeFailed)
}

/// What the directory of one image says.
struct Layout {
    width: u32,
    height: u32,
    photometric: u16,
    samples_per_pixel: u16,
    bits: u16,
    rows_per_strip: u32,
    compression: TiffCompression,
    predictor: bool,
}

impl TiffEncoder {
    fn write_file(
        &self,
        l: &Layout,
        strips: Vec<Vec<u8>>,
        meta: &EncodeMeta,
    ) -> Result<Vec<u8>, ErrKind> {
        let (w, h, photometric, spp, bits) = (
            l.width,
            l.height,
            l.photometric,
            l.samples_per_pixel,
            l.bits,
        );
        let (rows_per_strip, compression, predictor) =
            (l.rows_per_strip, l.compression, l.predictor);
        let total: usize = strips.iter().map(Vec::len).sum();
        let mut out: Vec<u8> = Vec::with_capacity(total + 4096);
        out.extend_from_slice(b"II*\0\0\0\0\0");
        let mut offsets = Vec::with_capacity(strips.len());
        let mut counts = Vec::with_capacity(strips.len());
        for s in &strips {
            // Strips start on a word boundary.
            if out.len() % 2 == 1 {
                out.push(0);
            }
            offsets.push(u32::try_from(out.len()).map_err(|_| ErrKind::TooLarge)?);
            counts.push(u32::try_from(s.len()).map_err(|_| ErrKind::TooLarge)?);
            out.extend_from_slice(s);
        }
        drop(strips);

        let mut e: Vec<Entry> = vec![
            long(256, w),
            long(257, h),
            Entry {
                tag: 258,
                typ: SHORT,
                count: u32::from(spp),
                data: (0..spp).flat_map(|_| bits.to_le_bytes()).collect(),
            },
            short(259, compression.tag()),
            short(262, photometric),
            longs(273, &offsets),
            short(274, 1),
            short(277, spp),
            long(278, rows_per_strip),
            longs(279, &counts),
        ];
        match meta.dpi {
            Some((x, y)) if x > 0 && y > 0 => {
                e.push(rational(282, x, 1));
                e.push(rational(283, y, 1));
                e.push(short(296, 2));
            }
            _ => {
                e.push(rational(282, 1, 1));
                e.push(rational(283, 1, 1));
                e.push(short(296, 1));
            }
        }
        e.push(short(284, 1));
        let mut sw = SOFTWARE.as_bytes().to_vec();
        sw.push(0);
        e.push(Entry {
            tag: 305,
            typ: ASCII,
            count: sw.len() as u32,
            data: sw,
        });
        if predictor {
            e.push(short(317, 2));
        }
        if compression == TiffCompression::G4 {
            e.push(long(293, 0)); // T6Options: no uncompressed mode
        }
        for x in &self.extra {
            if !e.iter().any(|en| en.tag == x.tag) && !x.data.is_empty() {
                e.push(Entry {
                    tag: x.tag,
                    typ: x.ty,
                    count: x.count,
                    data: x.data.clone(),
                });
            }
        }
        if let Some(icc) = meta.icc.as_deref().filter(|p| !p.is_empty()) {
            e.push(Entry {
                tag: 34675,
                typ: UNDEFINED,
                count: u32::try_from(icc.len()).map_err(|_| ErrKind::TooLarge)?,
                data: icc.to_vec(),
            });
        }
        e.sort_by_key(|x| x.tag);

        // Values that do not fit the 4-byte slot go after the strips, word aligned.
        let mut slots: Vec<[u8; 4]> = Vec::with_capacity(e.len());
        for en in &e {
            if en.data.len() <= 4 {
                let mut s = [0u8; 4];
                s[..en.data.len()].copy_from_slice(&en.data);
                slots.push(s);
            } else {
                if out.len() % 2 == 1 {
                    out.push(0);
                }
                let off = u32::try_from(out.len()).map_err(|_| ErrKind::TooLarge)?;
                out.extend_from_slice(&en.data);
                slots.push(off.to_le_bytes());
            }
        }
        if out.len() % 2 == 1 {
            out.push(0);
        }
        let ifd = u32::try_from(out.len()).map_err(|_| ErrKind::TooLarge)?;
        out.extend_from_slice(&(e.len() as u16).to_le_bytes());
        for (en, slot) in e.iter().zip(&slots) {
            out.extend_from_slice(&en.tag.to_le_bytes());
            out.extend_from_slice(&en.typ.to_le_bytes());
            out.extend_from_slice(&en.count.to_le_bytes());
            out.extend_from_slice(slot);
        }
        out.extend_from_slice(&0u32.to_le_bytes());
        if u32::try_from(out.len()).is_err() {
            return Err(ErrKind::TooLarge);
        }
        out[4..8].copy_from_slice(&ifd.to_le_bytes());
        Ok(out)
    }

    fn encode_g4(
        &self,
        src: &RasterView<'_>,
        meta: &EncodeMeta,
        cancel: &CancelToken,
    ) -> Result<Vec<u8>, ErrKind> {
        if src.width() > G4_MAX_WIDTH || src.height() == 0 || src.width() == 0 {
            return Err(ErrKind::UnsupportedOutput);
        }
        let img = BilevelImage::from_view(src, self.bilevel_cut, cancel)?;
        let h = img.height;
        let rps = self.rows_per_strip.unwrap_or(h).clamp(1, h);
        let mut strips = Vec::new();
        let mut y = 0;
        while y < h {
            check(cancel)?;
            let y1 = (y + rps).min(h);
            strips.push(img.g4(y, y1, cancel)?);
            y = y1;
        }
        // WhiteIsZero (0): the bytes code white and black runs, the decoder maps them to bits.
        let layout = Layout {
            width: img.width,
            height: h,
            photometric: 0,
            samples_per_pixel: 1,
            bits: 1,
            rows_per_strip: rps,
            compression: TiffCompression::G4,
            predictor: false,
        };
        self.write_file(&layout, strips, meta)
    }
}

impl Encoder for TiffEncoder {
    fn format(&self) -> OutFormat {
        OutFormat::Tiff
    }

    fn encode(
        &self,
        src: &RasterView<'_>,
        spec: &OutputSpec,
        meta: &EncodeMeta,
        cancel: &CancelToken,
    ) -> Result<Vec<u8>, ErrKind> {
        check(cancel)?;
        if src.width() == 0 || src.height() == 0 {
            return Err(ErrKind::UnsupportedOutput);
        }
        match spec.bits {
            Bits::One {
                bilevel: Bilevel::TiffG4,
            } => return self.encode_g4(src, meta, cancel),
            Bits::One { .. } => return Err(ErrKind::UnsupportedOutput),
            Bits::Eight => {}
        }
        if self.compression == TiffCompression::G4 {
            return self.encode_g4(src, meta, cancel);
        }
        let fmt = src.format();
        let (spp, sixteen) = (fmt.channels(), fmt.bytes_per_sample() == 2);
        let photometric = if spp == 1 { 1 } else { 2 };
        let row_bytes = src.width() as usize * fmt.bytes_per_pixel();
        let rps = self
            .rows_per_strip
            .unwrap_or_else(|| (STRIP_BYTES / row_bytes.max(1)).max(1) as u32)
            .clamp(1, src.height());
        let predictor = self.predictor
            && matches!(
                self.compression,
                TiffCompression::Lzw | TiffCompression::Deflate
            );

        let mut strips = Vec::with_capacity(src.height().div_ceil(rps) as usize);
        let mut raw = Vec::with_capacity(row_bytes * rps as usize);
        let mut y = 0;
        while y < src.height() {
            check(cancel)?;
            let y1 = (y + rps).min(src.height());
            raw.clear();
            for r in y..y1 {
                let start = raw.len();
                if let Some(row) = src.row_u8(r) {
                    raw.extend_from_slice(row);
                } else if let Some(row) = src.row_u16(r) {
                    for v in row {
                        raw.extend_from_slice(&v.to_le_bytes());
                    }
                } else {
                    return Err(ErrKind::Internal);
                }
                if predictor {
                    predict_row(&mut raw[start..], spp, sixteen);
                }
            }
            strips.push(match self.compression {
                TiffCompression::None | TiffCompression::G4 => raw.clone(),
                TiffCompression::Lzw => lzw(&raw)?,
                TiffCompression::Deflate => deflate(&raw)?,
                TiffCompression::PackBits => {
                    let mut p = Vec::with_capacity(raw.len() + raw.len() / 64 + 16);
                    for row in raw.chunks(row_bytes) {
                        packbits_row(row, &mut p);
                    }
                    p
                }
            });
            y = y1;
        }
        let bits: u16 = if sixteen { 16 } else { 8 };
        let layout = Layout {
            width: src.width(),
            height: src.height(),
            photometric,
            samples_per_pixel: spp as u16,
            bits,
            rows_per_strip: rps,
            compression: self.compression,
            predictor,
        };
        self.write_file(&layout, strips, meta)
    }
}
