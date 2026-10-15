// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! A minimal PDF writer: one image per page, nothing else (ROADMAP M11.32 to M11.38; PLAN 3.3
//! "PDF"). Written by hand because the job is small and exact: a classic cross-reference table,
//! deterministic object numbers, no text layer (B19: no OCR), no fonts, no JavaScript, no
//! annotations, no JBIG2. Pages stream to the sink as they are added; the page tree, the
//! information dictionary, the cross-reference table and the trailer come last, so the writer holds
//! one page's bytes at a time.
//!
//! **Objects.** 1 = catalogue, 2 = page tree, 3 = information dictionary, then three per page
//! (image, content, page) and, for an image with an embedded colour profile, one more (the profile).
//! The cross-reference table is a classic `xref` section whose 20-byte entries `qpdf --check` and
//! every viewer accept.
//!
//! **Images** ([`PageImage`]):
//!
//! * `Jpeg`: the encoder's bytes embedded unchanged as `DCTDecode`, after the segments that can
//!   carry personal data (EXIF, XMP, IPTC, comments, the ICC `APP2`) are dropped; the profile moves
//!   to an `ICCBased` colour space (when it fits the component count). Baseline, extended
//!   sequential and progressive Huffman JPEGs of 1 or 3 components and 8 bits are accepted; CMYK,
//!   12-bit and arithmetic-coded streams are refused (a PDF reader cannot be relied on for them).
//! * `Flate`: 8-bit grey or RGB, PNG row filters (`/Predictor 15`, the technique of img2pdf) and
//!   zlib: lossless, and about as small as the PNG of the same pixels.
//! * `G4`: a 1-bit page as `CCITTFaxDecode` with `/K -1` (Group 4). The row coder's bytes mean "white
//!   run, black run, ..." and `/BlackIs1 false` (the default) says the decoded 0 bits are black, so
//!   a black mark on white stays black.
//!
//! **Page size** is the pixel size over the dpi times 72 points (300 dpi, flagged `dpi_assumed`,
//! when the dpi is unknown, as PLAN 5.9 sets), at most 14,400 points and at least 3 on a side
//! (scaled, and flagged `scaled`, beyond that).
//!
//! **Metadata.** `Producer` ("Auto Crop"), an optional `Title`, and an optional `CreationDate`
//! given by the caller (none by default, so equal inputs give equal bytes). No source path, no
//! location, no user name.
//!
//! **Verification.** [`read_pages`] parses back what this writer makes (not arbitrary PDFs):
//! header, every cross-reference offset, page count, stream lengths and image parameters. The engine
//! runs it, and [`PdfPageRead::to_rgb`] decodes the embedded image again, in the commit protocol's
//! verify step.

use super::bilevel::{BilevelImage, G4_MAX_WIDTH};
use super::{check, samples_u8};
use auto_crop_core::output::{Bits, OutFormat, Quality};
use auto_crop_core::ports::{EncodeMeta, Encoder, RasterView};
use auto_crop_core::{CancelToken, ErrKind, OutputSpec};
use std::io::{Seek, SeekFrom, Write};

/// The dpi assumed when the source says none (PLAN 5.9).
pub const NOMINAL_DPI: u32 = 300;
/// The largest page side, in points (Acrobat's limit).
pub const MAX_PAGE_PT: f64 = 14_400.0;
/// The smallest page side, in points.
pub const MIN_PAGE_PT: f64 = 3.0;
/// Pages one document may hold.
pub const MAX_PAGES: usize = 10_000;
pub const PRODUCER: &str = "Auto Crop";

/// What goes on a page.
pub enum PageImage<'a> {
    /// A JPEG file's bytes (any APPn and COM segment is dropped, the stream is otherwise unchanged).
    Jpeg {
        bytes: &'a [u8],
        /// The profile to embed (byte-exact); dropped when it does not match the component count.
        icc: Option<&'a [u8]>,
    },
    /// 8-bit samples, `channels` = 1 (grey) or 3 (RGB), rows contiguous.
    Flate {
        width: u32,
        height: u32,
        channels: u8,
        samples: &'a [u8],
        icc: Option<&'a [u8]>,
    },
    /// A thresholded page.
    G4(&'a BilevelImage),
}

/// What [`PdfWriter::add_page`] decided about a page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageInfo {
    pub width_px: u32,
    pub height_px: u32,
    pub width_pt: f64,
    pub height_pt: f64,
    /// The dpi was not known and 300 was used.
    pub dpi_assumed: bool,
    /// The page was scaled to stay within [`MIN_PAGE_PT`]..=[`MAX_PAGE_PT`].
    pub scaled: bool,
    /// The embedded profile did not fit the image and was left out.
    pub icc_dropped: bool,
}

/// A counting sink.
struct Sink<W: Write> {
    w: W,
    pos: u64,
}

impl<W: Write> Sink<W> {
    fn put(&mut self, b: &[u8]) -> Result<(), ErrKind> {
        self.w.write_all(b).map_err(|e| ErrKind::from_io(&e))?;
        self.pos += b.len() as u64;
        Ok(())
    }
}

pub struct PdfWriter<W: Write> {
    sink: Sink<W>,
    /// Byte offset of object `i + 1`.
    offsets: Vec<u64>,
    pages: Vec<u32>,
    title: Option<String>,
    created: Option<String>,
}

fn pdf_string(s: &str) -> String {
    // A literal string in PDFDocEncoding-compatible ASCII; anything else becomes UTF-16BE with a BOM
    // as a hex string.
    if s.chars().all(|c| (' '..='~').contains(&c)) {
        let mut o = String::from("(");
        for c in s.chars() {
            if matches!(c, '(' | ')' | '\\') {
                o.push('\\');
            }
            o.push(c);
        }
        o.push(')');
        o
    } else {
        let mut o = String::from("<FEFF");
        for u in s.encode_utf16() {
            o.push_str(&format!("{u:04X}"));
        }
        o.push('>');
        o
    }
}

fn pt(v: f64) -> String {
    // Two decimals, no exponent, trailing zeros trimmed.
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

fn icc_fits(icc: &[u8], channels: u8) -> bool {
    // Header: size (4), CMM (4), version (4), class (4), colour space (4) at 16..20, PCS (4),
    // ..., 'acsp' at 36. A grey profile has "GRAY", an RGB one "RGB ".
    icc.len() >= 128
        && &icc[36..40] == b"acsp"
        && match channels {
            1 => &icc[16..20] == b"GRAY",
            3 => &icc[16..20] == b"RGB ",
            _ => false,
        }
}

impl<W: Write> PdfWriter<W> {
    /// Starts a document: writes the header.
    pub fn new(w: W) -> Result<Self, ErrKind> {
        let mut sink = Sink { w, pos: 0 };
        sink.put(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n")?;
        Ok(Self {
            sink,
            // Objects 1 to 3 (catalogue, page tree, info) are written last; their offsets are
            // filled in then.
            offsets: vec![0, 0, 0],
            pages: Vec::new(),
            title: None,
            created: None,
        })
    }

    pub fn set_title(&mut self, title: &str) {
        let t: String = title
            .chars()
            .filter(|c| !c.is_control())
            .take(200)
            .collect();
        self.title = (!t.is_empty()).then_some(t);
    }

    /// `D:YYYYMMDDHHmmSSZ` (UTC); none by default.
    pub fn set_creation_date(&mut self, utc: (u16, u8, u8, u8, u8, u8)) {
        let (y, mo, d, h, mi, s) = utc;
        self.created = Some(format!("D:{y:04}{mo:02}{d:02}{h:02}{mi:02}{s:02}Z"));
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    fn begin_obj(&mut self) -> Result<u32, ErrKind> {
        self.offsets.push(self.sink.pos);
        let id = self.offsets.len() as u32;
        self.sink.put(format!("{id} 0 obj\n").as_bytes())?;
        Ok(id)
    }

    fn stream_obj(&mut self, dict: &str, data: &[u8]) -> Result<u32, ErrKind> {
        let id = self.begin_obj()?;
        self.sink
            .put(format!("<< {dict} /Length {} >>\nstream\n", data.len()).as_bytes())?;
        self.sink.put(data)?;
        self.sink.put(b"\nendstream\nendobj\n")?;
        Ok(id)
    }

    /// Adds one page whose image is `image`, sized from `dpi` (`None`: [`NOMINAL_DPI`]).
    pub fn add_page(
        &mut self,
        image: PageImage<'_>,
        dpi: Option<(u32, u32)>,
    ) -> Result<PageInfo, ErrKind> {
        if self.pages.len() >= MAX_PAGES {
            return Err(ErrKind::UnsupportedOutput);
        }
        // The image object (and the profile object, written first so the image can name it).
        let (w, h, dict, data, channels, icc_dropped, icc_obj) = match image {
            PageImage::Jpeg { bytes, icc } => {
                let info = jpeg_info(bytes)?;
                let stripped = strip_jpeg(bytes)?;
                let (icc_obj, dropped) = self.icc_object(icc, info.components)?;
                let cs = colour_space(info.components, icc_obj);
                let dict = format!(
                    "/Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace {cs} \
                     /BitsPerComponent 8 /Filter /DCTDecode",
                    info.width, info.height
                );
                (
                    info.width,
                    info.height,
                    dict,
                    stripped,
                    info.components,
                    dropped,
                    icc_obj,
                )
            }
            PageImage::Flate {
                width,
                height,
                channels,
                samples,
                icc,
            } => {
                if !(channels == 1 || channels == 3)
                    || width == 0
                    || height == 0
                    || samples.len() != width as usize * height as usize * channels as usize
                {
                    return Err(ErrKind::UnsupportedOutput);
                }
                let (icc_obj, dropped) = self.icc_object(icc, channels)?;
                let cs = colour_space(channels, icc_obj);
                let filtered =
                    png_filter_rows(samples, width as usize, height as usize, channels as usize);
                let data = zlib(&filtered)?;
                let dict = format!(
                    "/Type /XObject /Subtype /Image /Width {width} /Height {height} /ColorSpace {cs} \
                     /BitsPerComponent 8 /Filter /FlateDecode \
                     /DecodeParms << /Predictor 15 /Colors {channels} /BitsPerComponent 8 /Columns {width} >>"
                );
                (width, height, dict, data, channels, dropped, icc_obj)
            }
            PageImage::G4(img) => {
                if img.width == 0
                    || img.width > G4_MAX_WIDTH
                    || img.height == 0
                    || img.height > G4_MAX_WIDTH
                {
                    return Err(ErrKind::UnsupportedOutput);
                }
                let data = img.g4(0, img.height, &CancelToken::never())?;
                let dict = format!(
                    "/Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceGray \
                     /BitsPerComponent 1 /Filter /CCITTFaxDecode \
                     /DecodeParms << /K -1 /Columns {w} /Rows {h} >>",
                    w = img.width,
                    h = img.height
                );
                (img.width, img.height, dict, data, 1, false, None)
            }
        };
        let _ = (channels, icc_obj);
        let img_id = self.stream_obj(&dict, &data)?;
        drop(data);

        // Page size.
        let (dx, dy, assumed) = match dpi {
            Some((x, y)) if x > 0 && y > 0 => (f64::from(x), f64::from(y), false),
            _ => (f64::from(NOMINAL_DPI), f64::from(NOMINAL_DPI), true),
        };
        let (mut wp, mut hp) = (f64::from(w) * 72.0 / dx, f64::from(h) * 72.0 / dy);
        let mut scaled = false;
        let big = wp.max(hp);
        if big > MAX_PAGE_PT {
            let k = MAX_PAGE_PT / big;
            wp *= k;
            hp *= k;
            scaled = true;
        }
        let small = wp.min(hp);
        if small < MIN_PAGE_PT {
            let k = MIN_PAGE_PT / small;
            // Growing the small side may push the long one over the limit; the limit wins.
            let k = k.min(MAX_PAGE_PT / wp.max(hp));
            wp *= k;
            hp *= k;
            scaled = true;
        }
        let (wp, hp) = ((wp * 100.0).round() / 100.0, (hp * 100.0).round() / 100.0);

        let content = format!("q {} 0 0 {} 0 0 cm /Im0 Do Q", pt(wp), pt(hp));
        let content_id = self.stream_obj("", content.as_bytes())?;
        let page_id = self.begin_obj()?;
        self.sink.put(
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Resources << /XObject << /Im0 {img_id} 0 R >> >> /Contents {content_id} 0 R >>\nendobj\n",
                pt(wp),
                pt(hp)
            )
            .as_bytes(),
        )?;
        self.pages.push(page_id);
        Ok(PageInfo {
            width_px: w,
            height_px: h,
            width_pt: wp,
            height_pt: hp,
            dpi_assumed: assumed,
            scaled,
            icc_dropped,
        })
    }

    /// Writes the profile stream; returns its object number and whether it was left out.
    fn icc_object(
        &mut self,
        icc: Option<&[u8]>,
        channels: u8,
    ) -> Result<(Option<u32>, bool), ErrKind> {
        match icc.filter(|p| !p.is_empty()) {
            Some(p) if icc_fits(p, channels) => {
                let alt = if channels == 1 {
                    "/DeviceGray"
                } else {
                    "/DeviceRGB"
                };
                let data = zlib(p)?;
                let id = self.stream_obj(
                    &format!("/N {channels} /Alternate {alt} /Filter /FlateDecode"),
                    &data,
                )?;
                Ok((Some(id), false))
            }
            Some(_) => Ok((None, true)),
            None => Ok((None, false)),
        }
    }

    /// Writes the page tree, the information dictionary, the cross-reference table and the
    /// trailer; returns the sink.
    pub fn finish(mut self) -> Result<W, ErrKind> {
        if self.pages.is_empty() {
            return Err(ErrKind::UnsupportedOutput);
        }
        self.offsets[0] = self.sink.pos;
        self.sink
            .put(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n")?;
        self.offsets[1] = self.sink.pos;
        let kids: String = self
            .pages
            .iter()
            .map(|p| format!("{p} 0 R"))
            .collect::<Vec<_>>()
            .join(" ");
        self.sink.put(
            format!(
                "2 0 obj\n<< /Type /Pages /Kids [{kids}] /Count {} >>\nendobj\n",
                self.pages.len()
            )
            .as_bytes(),
        )?;
        self.offsets[2] = self.sink.pos;
        let mut info = format!("3 0 obj\n<< /Producer {}", pdf_string(PRODUCER));
        if let Some(t) = &self.title {
            info.push_str(&format!(" /Title {}", pdf_string(t)));
        }
        if let Some(d) = &self.created {
            info.push_str(&format!(" /CreationDate ({d})"));
        }
        info.push_str(" >>\nendobj\n");
        self.sink.put(info.as_bytes())?;

        let xref_pos = self.sink.pos;
        let n = self.offsets.len() + 1;
        let mut x = format!("xref\n0 {n}\n0000000000 65535 f \n");
        for off in &self.offsets {
            x.push_str(&format!("{off:010} 00000 n \n"));
        }
        x.push_str(&format!(
            "trailer\n<< /Size {n} /Root 1 0 R /Info 3 0 R >>\nstartxref\n{xref_pos}\n%%EOF\n"
        ));
        self.sink.put(x.as_bytes())?;
        self.sink.w.flush().map_err(|e| ErrKind::from_io(&e))?;
        Ok(self.sink.w)
    }
}

fn colour_space(channels: u8, icc: Option<u32>) -> String {
    match (icc, channels) {
        (Some(id), _) => format!("[/ICCBased {id} 0 R]"),
        (None, 1) => "/DeviceGray".to_owned(),
        _ => "/DeviceRGB".to_owned(),
    }
}

fn zlib(data: &[u8]) -> Result<Vec<u8>, ErrKind> {
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    z.write_all(data).map_err(|_| ErrKind::EncodeFailed)?;
    z.finish().map_err(|_| ErrKind::EncodeFailed)
}

/// PNG row filtering with the minimum-sum-of-absolute-differences heuristic: every row is
/// prefixed with its filter type, as `/Predictor 15` expects.
fn png_filter_rows(samples: &[u8], w: usize, h: usize, bpp: usize) -> Vec<u8> {
    let row_len = w * bpp;
    let mut out = Vec::with_capacity((row_len + 1) * h);
    let zero = vec![0u8; row_len];
    let mut cand: [Vec<u8>; 5] = std::array::from_fn(|_| vec![0u8; row_len]);
    for y in 0..h {
        let row = &samples[y * row_len..(y + 1) * row_len];
        let prev: &[u8] = if y == 0 {
            &zero
        } else {
            &samples[(y - 1) * row_len..y * row_len]
        };
        for i in 0..row_len {
            let a = if i >= bpp { row[i - bpp] } else { 0 };
            let b = prev[i];
            let c = if i >= bpp { prev[i - bpp] } else { 0 };
            cand[0][i] = row[i];
            cand[1][i] = row[i].wrapping_sub(a);
            cand[2][i] = row[i].wrapping_sub(b);
            cand[3][i] = row[i].wrapping_sub(((u16::from(a) + u16::from(b)) / 2) as u8);
            cand[4][i] = row[i].wrapping_sub(paeth(a, b, c));
        }
        let score = |v: &[u8]| {
            v.iter()
                .map(|&x| u32::from((x as i8).unsigned_abs()))
                .sum::<u32>()
        };
        let best = (0..5).min_by_key(|&k| score(&cand[k])).unwrap_or(0);
        out.push(best as u8);
        out.extend_from_slice(&cand[best]);
    }
    out
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let (ia, ib, ic) = (i32::from(a), i32::from(b), i32::from(c));
    let p = ia + ib - ic;
    let (pa, pb, pc) = ((p - ia).abs(), (p - ib).abs(), (p - ic).abs());
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

fn unfilter_rows(data: &[u8], w: usize, h: usize, bpp: usize) -> Option<Vec<u8>> {
    let row_len = w.checked_mul(bpp)?;
    if data.len() != (row_len + 1).checked_mul(h)? {
        return None;
    }
    let mut out = vec![0u8; row_len * h];
    for y in 0..h {
        let ft = data[y * (row_len + 1)];
        let src = &data[y * (row_len + 1) + 1..(y + 1) * (row_len + 1)];
        let (done, rest) = out.split_at_mut(y * row_len);
        let prev: &[u8] = if y == 0 {
            &[]
        } else {
            &done[(y - 1) * row_len..]
        };
        let row = &mut rest[..row_len];
        for i in 0..row_len {
            let a = if i >= bpp { row[i - bpp] } else { 0 };
            let b = prev.get(i).copied().unwrap_or(0);
            let c = if i >= bpp {
                prev.get(i - bpp).copied().unwrap_or(0)
            } else {
                0
            };
            row[i] = src[i].wrapping_add(match ft {
                0 => 0,
                1 => a,
                2 => b,
                3 => ((u16::from(a) + u16::from(b)) / 2) as u8,
                4 => paeth(a, b, c),
                _ => return None,
            });
        }
    }
    Some(out)
}

// ---------------------------------------------------------------------------------------------
// JPEG streams
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JpegInfo {
    pub width: u32,
    pub height: u32,
    pub components: u8,
}

fn segments(jpeg: &[u8]) -> Result<Vec<(u8, usize, usize)>, ErrKind> {
    // (marker, start of the segment including FF xx, end) up to and excluding the SOS data.
    if !jpeg.starts_with(&[0xFF, 0xD8]) {
        return Err(ErrKind::UnsupportedOutput);
    }
    let mut out = Vec::new();
    let mut i = 2;
    while i + 4 <= jpeg.len() {
        if jpeg[i] != 0xFF {
            return Err(ErrKind::UnsupportedOutput);
        }
        let m = jpeg[i + 1];
        if m == 0xFF {
            i += 1;
            continue;
        }
        let len = usize::from(u16::from_be_bytes([jpeg[i + 2], jpeg[i + 3]]));
        if len < 2 || i + 2 + len > jpeg.len() {
            return Err(ErrKind::UnsupportedOutput);
        }
        out.push((m, i, i + 2 + len));
        i += 2 + len;
        if m == 0xDA {
            return Ok(out);
        }
    }
    Err(ErrKind::UnsupportedOutput)
}

/// Dimensions and component count of a JPEG a PDF reader can be expected to draw.
pub fn jpeg_info(jpeg: &[u8]) -> Result<JpegInfo, ErrKind> {
    for (m, s, e) in segments(jpeg)? {
        match m {
            0xC0..=0xC2 => {
                let b = &jpeg[s + 4..e];
                if b.len() < 6 || b[0] != 8 {
                    return Err(ErrKind::UnsupportedOutput);
                }
                let (h, w) = (
                    u32::from(u16::from_be_bytes([b[1], b[2]])),
                    u32::from(u16::from_be_bytes([b[3], b[4]])),
                );
                let n = b[5];
                if w == 0 || h == 0 || !(n == 1 || n == 3) {
                    return Err(ErrKind::UnsupportedOutput);
                }
                return Ok(JpegInfo {
                    width: w,
                    height: h,
                    components: n,
                });
            }
            // Lossless, differential and arithmetic frames.
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => {
                return Err(ErrKind::UnsupportedOutput);
            }
            _ => {}
        }
    }
    Err(ErrKind::UnsupportedOutput)
}

/// The JPEG without APP1 to APP13, APP15 and COM (EXIF, XMP, IPTC, the ICC profile, comments); JFIF
/// (APP0) and Adobe (APP14, which says whether the colours are YCbCr) stay.
pub fn strip_jpeg(jpeg: &[u8]) -> Result<Vec<u8>, ErrKind> {
    let segs = segments(jpeg)?;
    let mut out = Vec::with_capacity(jpeg.len());
    out.extend_from_slice(&[0xFF, 0xD8]);
    let mut last_end = 2;
    for (m, s, e) in segs {
        let drop = matches!(m, 0xE1..=0xED | 0xEF | 0xFE);
        if !drop {
            out.extend_from_slice(&jpeg[s..e]);
        }
        last_end = e;
    }
    out.extend_from_slice(&jpeg[last_end..]);
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Reading back what this writer makes
// ---------------------------------------------------------------------------------------------

/// How the image of a page is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageCodec {
    Dct,
    Flate,
    G4,
}

/// One page as [`read_pages`] sees it.
#[derive(Debug, Clone)]
pub struct PdfPageRead {
    pub width_px: u32,
    pub height_px: u32,
    pub media_box: (f64, f64),
    pub codec: PageCodec,
    pub channels: u8,
    /// The embedded profile, inflated.
    pub icc: Option<Vec<u8>>,
    stream: Vec<u8>,
}

impl PdfPageRead {
    /// The image stream exactly as it is in the file (JPEG data, zlib data or Group 4 data).
    pub fn stream_bytes(&self) -> &[u8] {
        &self.stream
    }

    /// The embedded image as RGB8 (grey expanded, a bilevel page black 0 and white 255).
    pub fn to_rgb(&self) -> Option<(u32, u32, Vec<u8>)> {
        let (w, h) = (self.width_px, self.height_px);
        match self.codec {
            PageCodec::Dct => {
                let d = crate::decode(&self.stream).ok()?;
                Some((d.raster.width, d.raster.height, d.raster.data))
            }
            PageCodec::Flate => {
                let mut raw = Vec::new();
                flate2::read::ZlibDecoder::new(&self.stream[..])
                    .take(1 << 32)
                    .read_to_end(&mut raw)
                    .ok()?;
                let px = unfilter_rows(&raw, w as usize, h as usize, self.channels as usize)?;
                let rgb = if self.channels == 1 {
                    px.iter().flat_map(|v| [*v; 3]).collect()
                } else {
                    px
                };
                Some((w, h, rgb))
            }
            PageCodec::G4 => {
                let b = BilevelImage::from_g4(&self.stream, w, h)?;
                let mut rgb = Vec::with_capacity(w as usize * h as usize * 3);
                for y in 0..h {
                    for x in 0..w {
                        rgb.extend_from_slice(&[if b.is_black(x, y) { 0 } else { 255 }; 3]);
                    }
                }
                Some((w, h, rgb))
            }
        }
    }
}

use std::io::Read;

/// A tiny cursor over PDF syntax: just what this writer emits.
struct Cur<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Cur<'a> {
    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\n' | b'\r' | b'\t') {
            self.i += 1;
        }
    }

    fn lit(&mut self, s: &[u8]) -> bool {
        self.ws();
        if self.b[self.i..].starts_with(s) {
            self.i += s.len();
            true
        } else {
            false
        }
    }

    fn int(&mut self) -> Option<i64> {
        self.ws();
        let s = self.i;
        if self.b.get(self.i) == Some(&b'-') {
            self.i += 1;
        }
        while self.i < self.b.len() && self.b[self.i].is_ascii_digit() {
            self.i += 1;
        }
        std::str::from_utf8(&self.b[s..self.i]).ok()?.parse().ok()
    }
}

/// The dictionary text between `<<` and the matching `>>` starting at `i` (nested dictionaries
/// are kept whole).
fn dict_at(b: &[u8], i: usize) -> Option<(&str, usize)> {
    if !b[i..].starts_with(b"<<") {
        return None;
    }
    let mut depth = 0;
    let mut j = i;
    while j + 1 < b.len() {
        if b[j] == b'<' && b[j + 1] == b'<' {
            depth += 1;
            j += 2;
        } else if b[j] == b'>' && b[j + 1] == b'>' {
            depth -= 1;
            j += 2;
            if depth == 0 {
                return std::str::from_utf8(&b[i + 2..j - 2]).ok().map(|s| (s, j));
            }
        } else {
            j += 1;
        }
    }
    None
}

/// The integer after `/Key` in `d`.
fn key_int(d: &str, key: &str) -> Option<i64> {
    let at = d.find(&format!("/{key} "))? + key.len() + 2;
    let rest = d[at..].trim_start();
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '-'))
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn key_ref(d: &str, key: &str) -> Option<usize> {
    let at = d.find(&format!("/{key} "))? + key.len() + 2;
    let mut it = d[at..].split_whitespace();
    let n: usize = it.next()?.parse().ok()?;
    (it.next()? == "0" && it.next()?.starts_with('R')).then_some(n)
}

/// Checks the structure of a document this writer made and returns its pages. Any deviation
/// (a wrong offset, a stream whose `/Length` is off, a missing page, junk after `%%EOF`) is `None`.
pub fn read_pages(bytes: &[u8]) -> Option<Vec<PdfPageRead>> {
    if !bytes.starts_with(b"%PDF-1.") {
        return None;
    }
    let tail = bytes.get(bytes.len().checked_sub(64)?..)?;
    let tail_s = String::from_utf8_lossy(tail);
    let sx = tail_s.rfind("startxref")?;
    let after = &tail_s[sx + 9..];
    let mut lines = after.split_whitespace();
    let xref_pos: usize = lines.next()?.parse().ok()?;
    if lines.next()? != "%%EOF" || lines.next().is_some() {
        return None;
    }
    // The table.
    let mut c = Cur {
        b: bytes,
        i: xref_pos,
    };
    if !c.lit(b"xref") || c.int()? != 0 {
        return None;
    }
    let n = usize::try_from(c.int()?).ok()?;
    if !(5..=MAX_PAGES * 4 + 8).contains(&n) {
        return None;
    }
    c.ws();
    let mut offsets = Vec::with_capacity(n);
    for k in 0..n {
        let line = bytes.get(c.i..c.i + 20)?;
        if &line[10..] != b" 00000 n \n" && &line[10..] != b" 65535 f \n" {
            return None;
        }
        let off: usize = std::str::from_utf8(&line[..10]).ok()?.parse().ok()?;
        if k == 0 {
            if &line[10..] != b" 65535 f \n" {
                return None;
            }
        } else {
            if !bytes
                .get(off..)?
                .starts_with(format!("{k} 0 obj\n").as_bytes())
            {
                return None;
            }
            offsets.push(off);
        }
        c.i += 20;
    }
    if !c.lit(b"trailer") {
        return None;
    }
    c.ws();
    let (trailer, _) = dict_at(bytes, c.i)?;
    if key_int(trailer, "Size")? != n as i64 || key_ref(trailer, "Root")? != 1 {
        return None;
    }
    let obj = |id: usize| -> Option<usize> {
        let off = *offsets.get(id.checked_sub(1)?)?;
        let hdr = format!("{id} 0 obj\n").len();
        Some(off + hdr)
    };
    let (cat, _) = dict_at(bytes, obj(1)?)?;
    if !cat.contains("/Type /Catalog") || key_ref(cat, "Pages")? != 2 {
        return None;
    }
    let (tree, _) = dict_at(bytes, obj(2)?)?;
    let count = usize::try_from(key_int(tree, "Count")?).ok()?;
    let kids_at = tree.find("/Kids [")? + 7;
    let kids_end = tree[kids_at..].find(']')? + kids_at;
    let mut kid_ids = Vec::new();
    let mut toks = tree[kids_at..kids_end].split_whitespace();
    while let Some(t) = toks.next() {
        let id: usize = t.parse().ok()?;
        if toks.next()? != "0" || toks.next()? != "R" {
            return None;
        }
        kid_ids.push(id);
    }
    if kid_ids.len() != count || count == 0 {
        return None;
    }
    let mut pages = Vec::new();
    for pid in kid_ids {
        let (pd, _) = dict_at(bytes, obj(pid)?)?;
        if !pd.contains("/Type /Page ") || key_ref(pd, "Parent")? != 2 {
            return None;
        }
        let mb = pd.find("/MediaBox [")? + 11;
        let mb_end = pd[mb..].find(']')? + mb;
        let v: Vec<f64> = pd[mb..mb_end]
            .split_whitespace()
            .filter_map(|t| t.parse().ok())
            .collect();
        if v.len() != 4 || v[0] != 0.0 || v[1] != 0.0 {
            return None;
        }
        let xo = pd.find("/XObject << /Im0 ")? + 17;
        let img_id: usize = pd[xo..].split_whitespace().next()?.parse().ok()?;
        let content_id = key_ref(pd, "Contents")?;
        // The content stream: exactly the image draw, nothing else (no text, no fonts).
        let (cd, cend) = dict_at(bytes, obj(content_id)?)?;
        let clen = usize::try_from(key_int(cd, "Length")?).ok()?;
        let cdata = stream_data(bytes, cend, clen)?;
        if !cdata.ends_with(b"/Im0 Do Q")
            || cdata.contains(&b'T') && cdata.windows(2).any(|w| w == b"BT")
        {
            return None;
        }
        // The image.
        let (id_s, iend) = dict_at(bytes, obj(img_id)?)?;
        if !id_s.contains("/Subtype /Image") {
            return None;
        }
        let len = usize::try_from(key_int(id_s, "Length")?).ok()?;
        let stream = stream_data(bytes, iend, len)?.to_vec();
        let width_px = u32::try_from(key_int(id_s, "Width")?).ok()?;
        let height_px = u32::try_from(key_int(id_s, "Height")?).ok()?;
        let codec = if id_s.contains("/DCTDecode") {
            PageCodec::Dct
        } else if id_s.contains("/FlateDecode") {
            PageCodec::Flate
        } else if id_s.contains("/CCITTFaxDecode") {
            PageCodec::G4
        } else {
            return None;
        };
        let (channels, icc) = if id_s.contains("/DeviceGray") && !id_s.contains("/ICCBased") {
            (1, None)
        } else if id_s.contains("/DeviceRGB") && !id_s.contains("/ICCBased") {
            (3, None)
        } else {
            let at = id_s.find("/ICCBased ")?;
            let pid: usize = id_s[at + 10..].split_whitespace().next()?.parse().ok()?;
            let (pdict, pend) = dict_at(bytes, obj(pid)?)?;
            let n = u8::try_from(key_int(pdict, "N")?).ok()?;
            let plen = usize::try_from(key_int(pdict, "Length")?).ok()?;
            let raw = stream_data(bytes, pend, plen)?;
            let mut icc = Vec::new();
            flate2::read::ZlibDecoder::new(raw)
                .take(1 << 26)
                .read_to_end(&mut icc)
                .ok()?;
            (n, Some(icc))
        };
        if codec == PageCodec::G4 && (channels != 1 || !id_s.contains("/K -1")) {
            return None;
        }
        pages.push(PdfPageRead {
            width_px,
            height_px,
            media_box: (v[2], v[3]),
            codec,
            channels,
            icc,
            stream,
        });
    }
    Some(pages)
}

// ---------------------------------------------------------------------------------------------
// Reading back a document on disk, one page at a time
// ---------------------------------------------------------------------------------------------

/// A reader that can fetch a window at an offset (what the streaming check needs).
struct Windows<'a, R: Read + Seek> {
    r: &'a mut R,
    len: u64,
}

impl<R: Read + Seek> Windows<'_, R> {
    /// Up to `max` bytes at `off` (fewer at the end of the file); `None` past the end.
    fn at(&mut self, off: u64, max: usize) -> Option<Vec<u8>> {
        if off > self.len {
            return None;
        }
        self.r.seek(SeekFrom::Start(off)).ok()?;
        let mut v = Vec::new();
        (&mut *self.r).take(max as u64).read_to_end(&mut v).ok()?;
        Some(v)
    }

    /// The dictionary starting at `off` (text and the absolute offset just after it). The window
    /// grows from 8 KiB to 8 MiB: a page tree of 10,000 pages is about 100 KB.
    fn dict(&mut self, off: u64) -> Option<(String, u64)> {
        let mut size = 8 * 1024usize;
        loop {
            let w = self.at(off, size)?;
            if let Some((d, end)) = dict_at(&w, 0) {
                return Some((d.to_owned(), off + end as u64));
            }
            if w.len() < size || size >= 8 * 1024 * 1024 {
                return None;
            }
            size *= 4;
        }
    }

    /// The `len` bytes of the stream whose dictionary ended at `dict_end`, framing checked.
    fn stream(&mut self, dict_end: u64, len: usize) -> Option<Vec<u8>> {
        const PRE: &[u8] = b"\nstream\n";
        const POST: &[u8] = b"\nendstream\nendobj\n";
        let w = self.at(
            dict_end,
            PRE.len().checked_add(len)?.checked_add(POST.len())?,
        )?;
        if w.len() != PRE.len() + len + POST.len()
            || &w[..PRE.len()] != PRE
            || &w[PRE.len() + len..] != POST
        {
            return None;
        }
        Some(w[PRE.len()..PRE.len() + len].to_vec())
    }
}

/// [`read_pages`] for a document on disk: the same checks, but only one page's stream is in memory
/// at a time, so a document of hundreds of 12 MP pages is verified in constant memory. `each` gets
/// every page in order (index from 0) and says whether it is acceptable; the result is the page
/// count when the whole structure holds and `each` accepted every page.
pub fn read_pages_streaming<R: Read + Seek>(
    r: &mut R,
    each: &mut dyn FnMut(usize, PdfPageRead) -> bool,
) -> Option<usize> {
    let len = r.seek(SeekFrom::End(0)).ok()?;
    let mut w = Windows { r, len };
    if !w.at(0, 7)?.starts_with(b"%PDF-1.") {
        return None;
    }
    let tail = w.at(len.checked_sub(64)?, 64)?;
    let tail_s = String::from_utf8_lossy(&tail);
    let sx = tail_s.rfind("startxref")?;
    let mut lines = tail_s[sx + 9..].split_whitespace();
    let xref_pos: u64 = lines.next()?.parse().ok()?;
    if lines.next()? != "%%EOF" || lines.next().is_some() {
        return None;
    }
    // The table.
    let head = w.at(xref_pos, 64)?;
    let mut c = Cur { b: &head, i: 0 };
    if !c.lit(b"xref") || c.int()? != 0 {
        return None;
    }
    let n = usize::try_from(c.int()?).ok()?;
    if !(5..=MAX_PAGES * 4 + 8).contains(&n) {
        return None;
    }
    c.ws();
    let table_at = xref_pos + c.i as u64;
    let table = w.at(table_at, n * 20)?;
    if table.len() != n * 20 {
        return None;
    }
    let mut offsets: Vec<u64> = Vec::with_capacity(n);
    for (k, line) in table.as_chunks::<20>().0.iter().enumerate() {
        if &line[10..] != b" 00000 n \n" && &line[10..] != b" 65535 f \n" {
            return None;
        }
        let off: u64 = std::str::from_utf8(&line[..10]).ok()?.parse().ok()?;
        if k == 0 {
            if &line[10..] != b" 65535 f \n" {
                return None;
            }
        } else {
            let want = format!("{k} 0 obj\n");
            if w.at(off, want.len())? != want.as_bytes() {
                return None;
            }
            offsets.push(off);
        }
    }
    let trailer_head = w.at(table_at + (n * 20) as u64, 256)?;
    let mut c = Cur {
        b: &trailer_head,
        i: 0,
    };
    if !c.lit(b"trailer") {
        return None;
    }
    c.ws();
    let (trailer, _) = dict_at(&trailer_head, c.i)?;
    if key_int(trailer, "Size")? != n as i64 || key_ref(trailer, "Root")? != 1 {
        return None;
    }
    // The absolute offset of the dictionary of object `id`.
    let obj = |id: usize| -> Option<u64> {
        let off = *offsets.get(id.checked_sub(1)?)?;
        Some(off + format!("{id} 0 obj\n").len() as u64)
    };
    let (cat, _) = w.dict(obj(1)?)?;
    if !cat.contains("/Type /Catalog") || key_ref(&cat, "Pages")? != 2 {
        return None;
    }
    let (tree, _) = w.dict(obj(2)?)?;
    let count = usize::try_from(key_int(&tree, "Count")?).ok()?;
    let kids_at = tree.find("/Kids [")? + 7;
    let kids_end = tree[kids_at..].find(']')? + kids_at;
    let mut kid_ids = Vec::new();
    let mut toks = tree[kids_at..kids_end].split_whitespace();
    while let Some(t) = toks.next() {
        let id: usize = t.parse().ok()?;
        if toks.next()? != "0" || toks.next()? != "R" {
            return None;
        }
        kid_ids.push(id);
    }
    if kid_ids.len() != count || count == 0 {
        return None;
    }
    for (index, pid) in kid_ids.into_iter().enumerate() {
        let (pd, _) = w.dict(obj(pid)?)?;
        if !pd.contains("/Type /Page ") || key_ref(&pd, "Parent")? != 2 {
            return None;
        }
        let mb = pd.find("/MediaBox [")? + 11;
        let mb_end = pd[mb..].find(']')? + mb;
        let v: Vec<f64> = pd[mb..mb_end]
            .split_whitespace()
            .filter_map(|t| t.parse().ok())
            .collect();
        if v.len() != 4 || v[0] != 0.0 || v[1] != 0.0 {
            return None;
        }
        let xo = pd.find("/XObject << /Im0 ")? + 17;
        let img_id: usize = pd[xo..].split_whitespace().next()?.parse().ok()?;
        let content_id = key_ref(&pd, "Contents")?;
        let (cd, cend) = w.dict(obj(content_id)?)?;
        let clen = usize::try_from(key_int(&cd, "Length")?).ok()?;
        let cdata = w.stream(cend, clen)?;
        if !cdata.ends_with(b"/Im0 Do Q")
            || cdata.contains(&b'T') && cdata.windows(2).any(|x| x == b"BT")
        {
            return None;
        }
        let (id_s, iend) = w.dict(obj(img_id)?)?;
        if !id_s.contains("/Subtype /Image") {
            return None;
        }
        let ilen = usize::try_from(key_int(&id_s, "Length")?).ok()?;
        let stream = w.stream(iend, ilen)?;
        let width_px = u32::try_from(key_int(&id_s, "Width")?).ok()?;
        let height_px = u32::try_from(key_int(&id_s, "Height")?).ok()?;
        let codec = if id_s.contains("/DCTDecode") {
            PageCodec::Dct
        } else if id_s.contains("/FlateDecode") {
            PageCodec::Flate
        } else if id_s.contains("/CCITTFaxDecode") {
            PageCodec::G4
        } else {
            return None;
        };
        let (channels, icc) = if id_s.contains("/DeviceGray") && !id_s.contains("/ICCBased") {
            (1, None)
        } else if id_s.contains("/DeviceRGB") && !id_s.contains("/ICCBased") {
            (3, None)
        } else {
            let at = id_s.find("/ICCBased ")?;
            let prof: usize = id_s[at + 10..].split_whitespace().next()?.parse().ok()?;
            let (pdict, pend) = w.dict(obj(prof)?)?;
            let n = u8::try_from(key_int(&pdict, "N")?).ok()?;
            let plen = usize::try_from(key_int(&pdict, "Length")?).ok()?;
            let raw = w.stream(pend, plen)?;
            let mut icc = Vec::new();
            flate2::read::ZlibDecoder::new(&raw[..])
                .take(1 << 26)
                .read_to_end(&mut icc)
                .ok()?;
            (n, Some(icc))
        };
        if codec == PageCodec::G4 && (channels != 1 || !id_s.contains("/K -1")) {
            return None;
        }
        let page = PdfPageRead {
            width_px,
            height_px,
            media_box: (v[2], v[3]),
            codec,
            channels,
            icc,
            stream,
        };
        if !each(index, page) {
            return None;
        }
        // (the page's strings are dropped here: one page lives at a time)
    }
    Some(count)
}

/// The `len` bytes of the stream whose dictionary ended at `dict_end`, checking the framing.
fn stream_data(b: &[u8], dict_end: usize, len: usize) -> Option<&[u8]> {
    let start = dict_end + b"\nstream\n".len();
    if b.get(dict_end..start)? != b"\nstream\n" {
        return None;
    }
    let end = start.checked_add(len)?;
    (b.get(end..end + b"\nendstream\nendobj\n".len())? == b"\nendstream\nendobj\n")
        .then(|| &b[start..end])
}

// ---------------------------------------------------------------------------------------------
// The Encoder
// ---------------------------------------------------------------------------------------------

/// A one-page PDF from a raster, through the `Encoder` port.
///
/// * `Bits::One` gives a Group 4 page (the picture thresholded at 128);
/// * `Quality::Lossless` gives a Flate page (lossless, like a PNG);
/// * otherwise the picture is encoded as JPEG (libjpeg-turbo when the build has it) at the spec's
///   quality and embedded unchanged.
#[derive(Debug, Clone)]
pub struct PdfEncoder {
    /// The JPEG quality when the spec says `MatchSource`.
    pub jpeg_quality: u8,
    pub title: Option<String>,
}

impl Default for PdfEncoder {
    fn default() -> Self {
        Self {
            jpeg_quality: super::DEFAULT_JPEG_QUALITY,
            title: None,
        }
    }
}

impl Encoder for PdfEncoder {
    fn format(&self) -> OutFormat {
        OutFormat::Pdf
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
        let icc = meta.icc.as_deref();
        let mut w = PdfWriter::new(Vec::new())?;
        if let Some(t) = &self.title {
            w.set_title(t);
        }
        match (spec.bits, spec.quality) {
            (Bits::One { .. }, _) => {
                let img = BilevelImage::from_view(src, super::bilevel::DEFAULT_CUT, cancel)?;
                w.add_page(PageImage::G4(&img), meta.dpi)?;
            }
            (Bits::Eight, Quality::Lossless) => {
                let data = samples_u8(src);
                w.add_page(
                    PageImage::Flate {
                        width: src.width(),
                        height: src.height(),
                        channels: src.format().channels() as u8,
                        samples: &data,
                        icc,
                    },
                    meta.dpi,
                )?;
            }
            (Bits::Eight, q) => {
                let quality = match q {
                    Quality::Fixed { value } => value.clamp(1, 100),
                    _ => self.jpeg_quality,
                };
                let jspec = OutputSpec {
                    quality: Quality::Fixed { value: quality },
                    ..OutputSpec::default()
                };
                let jpeg = super::default_jpeg_encoder().encode(src, &jspec, meta, cancel)?;
                w.add_page(PageImage::Jpeg { bytes: &jpeg, icc }, meta.dpi)?;
            }
        }
        check(cancel)?;
        w.finish()
    }
}
