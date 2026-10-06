// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! JPEG marker parser, quality estimator and metadata policy (ROADMAP M2.80, M2.22, M2.23, M2.25;
//! PLAN 3.6, 3.9).
//!
//! * [`read_meta`] walks the marker segments before the first scan (bounded, never indexes out of
//!   range) and returns the pieces an output may carry over: the EXIF APP1 blob, the XMP packet,
//!   the IPTC (Photoshop) APP13 segment, comments and the JFIF pixel density, plus the quality the
//!   quantisation tables say the file was saved at ([`estimate_quality`]).
//! * [`exif_patch`] edits an EXIF blob **in place** (no entry moves, no offset changes, so a
//!   MakerNote with absolute offsets stays valid): Orientation becomes 1, the pixel dimensions are
//!   updated, the thumbnail (a picture of the uncropped scan) is zeroed and unlinked, and with
//!   `strip_location` the GPS IFD is zeroed byte by byte and unlinked.
//! * [`rewrite_metadata`] rebuilds a JPEG's header: every metadata segment of the output is
//!   dropped (APP1 EXIF and XMP, MPF, Photoshop, comments) and the patched ones of the source are
//!   inserted after the JFIF segment; scan data is copied verbatim and anything after the end-of-
//!   image marker (an MPF secondary image, a gain map) is cut. ICC (APP2), Adobe (APP14) and the
//!   JFIF segment stay as the output has them.
//!
//! The EXIF Orientation is applied once: the decode turns the pixels (or the lossless transform
//! turns the coefficients) and this module writes Orientation = 1, never both.

use crate::CodecError;

const EXIF_TAG: &[u8] = b"Exif\0\0";
const XMP_TAG: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
const XMP_EXT_TAG: &[u8] = b"http://ns.adobe.com/xmp/extension/\0";
const MPF_TAG: &[u8] = b"MPF\0";
const PHOTOSHOP_TAG: &[u8] = b"Photoshop 3.0\0";

fn corrupt(why: &str) -> CodecError {
    CodecError::corrupt(why)
}

/// One marker segment before the scan data: its marker byte and the payload span (after the
/// two length bytes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub marker: u8,
    pub payload: std::ops::Range<usize>,
    /// The span of the whole segment including `FF xx` and the length.
    pub whole: std::ops::Range<usize>,
}

/// The marker segments up to (not including) the first `SOS`, in file order. A file that does not
/// start with SOI, or whose segments run past the end, is `Corrupt`.
pub fn header_segments(b: &[u8]) -> Result<Vec<Segment>, CodecError> {
    if b.get(..2) != Some(&[0xFF, 0xD8]) {
        return Err(corrupt("not a JPEG"));
    }
    let n = b.len();
    let mut i = 2;
    let mut out = Vec::new();
    while i < n {
        if b[i] != 0xFF {
            return Err(corrupt("expected a JPEG marker"));
        }
        let start = i;
        while i < n && b[i] == 0xFF {
            i += 1;
        }
        let Some(&m) = b.get(i) else { break };
        i += 1;
        match m {
            0x00 | 0xD8 => return Err(corrupt("misplaced JPEG marker")),
            0x01 | 0xD0..=0xD7 => continue,
            0xD9 | 0xDA => break,
            _ => {}
        }
        let len = match b.get(i..i + 2) {
            Some(l) => usize::from(u16::from_be_bytes([l[0], l[1]])),
            None => return Err(corrupt("truncated JPEG")),
        };
        if len < 2 || i + len > n {
            return Err(corrupt("truncated or malformed JPEG segment"));
        }
        out.push(Segment {
            marker: m,
            payload: i + 2..i + len,
            whole: start..i + len,
        });
        i += len;
    }
    Ok(out)
}

/// What a JPEG says about itself that an output may carry over.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JpegMeta {
    /// The EXIF TIFF blob (without the `Exif\0\0` prefix), the first one in the file.
    pub exif: Option<Vec<u8>>,
    /// The XMP packet (without the namespace prefix); extended XMP is not carried over.
    pub xmp: Option<Vec<u8>>,
    /// The Photoshop APP13 payload (IPTC lives in it).
    pub iptc: Option<Vec<u8>>,
    /// `COM` segments.
    pub comments: Vec<Vec<u8>>,
    /// Pixel density in dots per inch from the JFIF segment.
    pub dpi: Option<(u32, u32)>,
    /// What the luminance quantisation table says.
    pub quality: Option<QualityEstimate>,
}

/// The IJG quality a JPEG's quantisation tables correspond to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QualityEstimate {
    /// 1..=100.
    pub q: u8,
    /// The tables are the IJG tables scaled by `q` (within rounding). `false`: custom tables, `q`
    /// is only the closest fit and a caller should use its default instead (PLAN 3.9: q90).
    pub ijg: bool,
}

#[rustfmt::skip]
const STD_LUMA: [u16; 64] = [
    16, 11, 10, 16, 24, 40, 51, 61,
    12, 12, 14, 19, 26, 58, 60, 55,
    14, 13, 16, 24, 40, 57, 69, 56,
    14, 17, 22, 29, 51, 87, 80, 62,
    18, 22, 37, 56, 68, 109, 103, 77,
    24, 35, 55, 64, 81, 104, 113, 92,
    49, 64, 78, 87, 103, 121, 120, 101,
    72, 92, 95, 98, 112, 100, 103, 99,
];

#[rustfmt::skip]
const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// The luminance table (the first `DQT` entry of id 0), in natural (row-major) order.
fn luma_table(b: &[u8], segs: &[Segment]) -> Option<[u16; 64]> {
    for s in segs.iter().filter(|s| s.marker == 0xDB) {
        let mut p = &b[s.payload.clone()];
        while let Some((&pq_tq, rest)) = p.split_first() {
            let (precision, id) = (pq_tq >> 4, pq_tq & 15);
            let size = if precision == 0 { 64 } else { 128 };
            let (tbl, rest) = (rest.get(..size)?, rest.get(size..)?);
            if id == 0 {
                let mut out = [0u16; 64];
                for (k, &z) in ZIGZAG.iter().enumerate() {
                    out[z] = if precision == 0 {
                        u16::from(tbl[k])
                    } else {
                        u16::from_be_bytes([tbl[2 * k], tbl[2 * k + 1]])
                    };
                }
                return Some(out);
            }
            p = rest;
        }
    }
    None
}

/// The IJG scaling of the standard luminance table for quality `q` (libjpeg's `jpeg_set_quality`).
fn ijg_table(q: u32) -> [u16; 64] {
    let s = if q < 50 { 5000 / q } else { 200 - q * 2 };
    let mut t = [0u16; 64];
    for (o, &b) in t.iter_mut().zip(&STD_LUMA) {
        *o = ((u32::from(b) * s + 50) / 100).clamp(1, 255) as u16;
    }
    t
}

/// Estimates the IJG quality of a luminance table: the `q` whose scaled standard table is closest
/// in the log-ratio least-squares sense. `ijg` says whether the fit is within rounding.
pub fn estimate_quality(table: &[u16; 64]) -> Option<QualityEstimate> {
    if table.contains(&0) {
        return None;
    }
    let mut best: Option<(f64, u8)> = None;
    for q in 1..=100u32 {
        let t = ijg_table(q);
        let err: f64 = table
            .iter()
            .zip(&t)
            .map(|(&a, &b)| {
                let d = f64::from(a).ln() - f64::from(b).ln();
                d * d
            })
            .sum::<f64>()
            / 64.0;
        if best.is_none_or(|(e, _)| err < e) {
            best = Some((err, q as u8));
        }
    }
    let (err, q) = best?;
    // Exact IJG tables fit with zero error; rounding at a clamp of 1 or 255 can add a little.
    Some(QualityEstimate {
        q,
        ijg: err < 0.002,
    })
}

fn xmp_payload(seg: &[u8]) -> Option<&[u8]> {
    seg.strip_prefix(XMP_TAG)
}

/// Reads the carry-over metadata of a JPEG.
pub fn read_meta(b: &[u8]) -> Result<JpegMeta, CodecError> {
    let segs = header_segments(b)?;
    let mut m = JpegMeta {
        quality: luma_table(b, &segs).and_then(|t| estimate_quality(&t)),
        ..JpegMeta::default()
    };
    for s in &segs {
        let p = &b[s.payload.clone()];
        match s.marker {
            0xE0 if p.starts_with(b"JFIF\0") && p.len() >= 12 && m.dpi.is_none() => {
                let (units, x, y) = (
                    p[7],
                    u32::from(u16::from_be_bytes([p[8], p[9]])),
                    u32::from(u16::from_be_bytes([p[10], p[11]])),
                );
                m.dpi = match units {
                    1 if x > 0 && y > 0 => Some((x, y)),
                    // Dots per centimetre.
                    2 if x > 0 && y > 0 => Some((
                        (f64::from(x) * 2.54).round() as u32,
                        (f64::from(y) * 2.54).round() as u32,
                    )),
                    _ => None,
                };
            }
            0xE1 if p.starts_with(EXIF_TAG) && m.exif.is_none() => {
                m.exif = Some(p[EXIF_TAG.len()..].to_vec());
            }
            0xE1 if xmp_payload(p).is_some() && m.xmp.is_none() => {
                m.xmp = xmp_payload(p).map(<[u8]>::to_vec);
            }
            0xED if p.starts_with(PHOTOSHOP_TAG) && m.iptc.is_none() => {
                m.iptc = Some(p.to_vec());
            }
            0xFE => m.comments.push(p.to_vec()),
            _ => {}
        }
    }
    Ok(m)
}

// ---------------------------------------------------------------------------------------------
// EXIF: patch in place
// ---------------------------------------------------------------------------------------------

const TAG_IMAGE_WIDTH: u16 = 0x0100;
const TAG_IMAGE_LENGTH: u16 = 0x0101;
const TAG_STRIP_OFFSETS: u16 = 0x0111;
const TAG_ORIENTATION: u16 = 0x0112;
const TAG_STRIP_BYTES: u16 = 0x0117;
const TAG_EXIF_IFD: u16 = 0x8769;
const TAG_GPS_IFD: u16 = 0x8825;
const TAG_JPEG_OFFSET: u16 = 0x0201;
const TAG_JPEG_LENGTH: u16 = 0x0202;
const TAG_PIXEL_X: u16 = 0xA002;
const TAG_PIXEL_Y: u16 = 0xA003;

/// What [`exif_patch`] changes besides resetting the Orientation.
#[derive(Debug, Clone, Copy)]
pub struct ExifPatch {
    /// The size of the output picture: written to the dimension tags that exist.
    pub dims: (u32, u32),
    /// Remove the GPS IFD (every byte of it is zeroed).
    pub strip_location: bool,
}

struct Tiff<'a> {
    b: &'a mut Vec<u8>,
    le: bool,
    /// The spans zeroed so far (to trim a zeroed tail).
    zeroed: Vec<(usize, usize)>,
}

#[derive(Clone, Copy)]
struct Entry {
    tag: u16,
    ty: u16,
    count: u32,
    /// Offset of this entry's 12 bytes.
    at: usize,
}

fn type_size(ty: u16) -> usize {
    match ty {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 => 4,
        5 | 10 | 12 => 8,
        _ => 0,
    }
}

impl Tiff<'_> {
    fn u16_at(&self, o: usize) -> Option<u16> {
        let b: [u8; 2] = self.b.get(o..o.checked_add(2)?)?.try_into().ok()?;
        Some(if self.le {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    }

    fn u32_at(&self, o: usize) -> Option<u32> {
        let b: [u8; 4] = self.b.get(o..o.checked_add(4)?)?.try_into().ok()?;
        Some(if self.le {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    }

    fn put_u16(&mut self, o: usize, v: u16) -> Option<()> {
        let bytes = if self.le {
            v.to_le_bytes()
        } else {
            v.to_be_bytes()
        };
        self.b
            .get_mut(o..o.checked_add(2)?)?
            .copy_from_slice(&bytes);
        Some(())
    }

    fn put_u32(&mut self, o: usize, v: u32) -> Option<()> {
        let bytes = if self.le {
            v.to_le_bytes()
        } else {
            v.to_be_bytes()
        };
        self.b
            .get_mut(o..o.checked_add(4)?)?
            .copy_from_slice(&bytes);
        Some(())
    }

    /// The entries of the IFD at `ifd` (bounded by what fits in the blob) and the offset of its
    /// next-IFD pointer.
    fn ifd(&self, ifd: usize) -> Option<(Vec<Entry>, usize)> {
        let declared = usize::from(self.u16_at(ifd)?);
        let fits = self.b.len().checked_sub(ifd.checked_add(2)?)? / 12;
        let n = declared.min(fits);
        let mut v = Vec::with_capacity(n);
        for i in 0..n {
            let at = ifd + 2 + 12 * i;
            v.push(Entry {
                tag: self.u16_at(at)?,
                ty: self.u16_at(at + 2)?,
                count: self.u32_at(at + 4)?,
                at,
            });
        }
        let next = ifd + 2 + 12 * declared;
        Some((v, next))
    }

    fn zero(&mut self, from: usize, len: usize) {
        let end = from.saturating_add(len).min(self.b.len());
        if from >= end {
            return;
        }
        self.b[from..end].fill(0);
        self.zeroed.push((from, end));
    }

    /// Cuts the blob back over the run of zeroed spans that reaches its end (everything before
    /// keeps its offset). A span in the middle is left as zeros.
    fn trim_zeroed_tail(&mut self) {
        let mut end = self.b.len();
        while let Some(start) = self
            .zeroed
            .iter()
            .filter(|(a, z)| *a < end && *z >= end)
            .map(|(a, _)| *a)
            .min()
        {
            end = start;
        }
        self.b.truncate(end.max(8));
    }

    /// Sets a SHORT or LONG entry of count 1 to `v`.
    fn set_scalar(&mut self, e: Entry, v: u32) {
        if e.count != 1 {
            return;
        }
        match e.ty {
            3 => {
                if let Ok(v) = u16::try_from(v) {
                    let _ = self.put_u16(e.at + 8, v);
                }
            }
            4 => {
                let _ = self.put_u32(e.at + 8, v);
            }
            _ => {}
        }
    }

    /// Zeroes the out-of-line values of every entry of the IFD at `ifd`, and the IFD itself.
    fn wipe_ifd(&mut self, ifd: usize) {
        let Some((entries, next)) = self.ifd(ifd) else {
            return;
        };
        for e in &entries {
            let size = type_size(e.ty).saturating_mul(e.count as usize);
            if size > 4
                && let Some(off) = self.u32_at(e.at + 8)
            {
                self.zero(off as usize, size);
            }
        }
        self.zero(ifd, next + 4 - ifd);
    }

    /// Removes entry `e` from the IFD at `ifd` by shifting the later entries (and the next-IFD
    /// pointer) up one slot; no value offset changes.
    fn remove_entry(&mut self, ifd: usize, e: Entry) {
        let Some(count) = self.u16_at(ifd) else {
            return;
        };
        let (start, end) = (e.at + 12, ifd + 2 + 12 * usize::from(count) + 4);
        if end > self.b.len() || start > end {
            return;
        }
        self.b.copy_within(start..end, e.at);
        self.zero(end - 12, 12);
        let _ = self.put_u16(ifd, count - 1);
    }

    /// Drops the thumbnail: the picture's bytes, the IFD1 table and its values, and the pointer
    /// to it.
    fn drop_thumbnail(&mut self, next_ptr_at: usize) {
        let Some(ifd1) = self.u32_at(next_ptr_at).filter(|&o| o != 0) else {
            return;
        };
        let ifd1 = ifd1 as usize;
        if let Some((entries, _)) = self.ifd(ifd1) {
            let scalar = |t: &Self, e: &Entry| t.u32_at(e.at + 8);
            let find = |tag: u16| entries.iter().find(|e| e.tag == tag).copied();
            // JPEG thumbnail: offset and length.
            if let (Some(o), Some(l)) = (find(TAG_JPEG_OFFSET), find(TAG_JPEG_LENGTH))
                && let (Some(off), Some(len)) = (scalar(self, &o), scalar(self, &l))
            {
                self.zero(off as usize, len as usize);
            }
            // Uncompressed thumbnail: strips.
            if let (Some(o), Some(l)) = (find(TAG_STRIP_OFFSETS), find(TAG_STRIP_BYTES)) {
                let n = (o.count.min(l.count) as usize).min(4096);
                let per = |t: &Self, e: &Entry, i: usize| -> Option<u32> {
                    let sz = type_size(e.ty);
                    let base = if sz * e.count as usize > 4 {
                        t.u32_at(e.at + 8)? as usize
                    } else {
                        e.at + 8
                    };
                    match sz {
                        2 => t.u16_at(base + 2 * i).map(u32::from),
                        4 => t.u32_at(base + 4 * i),
                        _ => None,
                    }
                };
                let spans: Vec<(u32, u32)> = (0..n)
                    .filter_map(|i| Some((per(self, &o, i)?, per(self, &l, i)?)))
                    .collect();
                for (off, len) in spans {
                    self.zero(off as usize, len as usize);
                }
            }
        }
        self.wipe_ifd(ifd1);
        let _ = self.put_u32(next_ptr_at, 0);
    }
}

/// Patches the EXIF TIFF blob `exif` (see the module docs). `None` if it is not a TIFF structure
/// this function understands (the caller then drops EXIF rather than copying it unpatched: a stale
/// Orientation would turn the picture a second time).
pub fn exif_patch(exif: &[u8], p: &ExifPatch) -> Option<Vec<u8>> {
    let le = match exif.get(..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let mut blob = exif.to_vec();
    let mut t = Tiff {
        b: &mut blob,
        le,
        zeroed: Vec::new(),
    };
    if t.u16_at(2)? != 42 {
        return None;
    }
    let ifd0 = t.u32_at(4)? as usize;
    let (entries, next0) = t.ifd(ifd0)?;
    let mut exif_ifd = None;
    let mut gps: Option<Entry> = None;
    for e in &entries {
        match e.tag {
            TAG_ORIENTATION => {
                if e.ty == 3 && e.count == 1 {
                    t.put_u16(e.at + 8, 1)?;
                }
            }
            TAG_IMAGE_WIDTH => t.set_scalar(*e, p.dims.0),
            TAG_IMAGE_LENGTH => t.set_scalar(*e, p.dims.1),
            TAG_EXIF_IFD => exif_ifd = t.u32_at(e.at + 8),
            TAG_GPS_IFD => gps = Some(*e),
            _ => {}
        }
    }
    if let Some(off) = exif_ifd
        && let Some((sub, _)) = t.ifd(off as usize)
    {
        for e in sub {
            match e.tag {
                TAG_PIXEL_X => t.set_scalar(e, p.dims.0),
                TAG_PIXEL_Y => t.set_scalar(e, p.dims.1),
                _ => {}
            }
        }
    }
    // The thumbnail first (it sits behind IFD0's table), then the GPS entry (which shifts IFD0's
    // table, and with it the next-IFD pointer).
    t.drop_thumbnail(next0);
    if p.strip_location
        && let Some(g) = gps
    {
        if let Some(off) = t.u32_at(g.at + 8) {
            t.wipe_ifd(off as usize);
        }
        t.remove_entry(ifd0, g);
    }
    // A zeroed tail is dead weight: cut it.
    t.trim_zeroed_tail();
    Some(blob)
}

/// Rewrites `tiff:Orientation` in an XMP packet to 1 (same length, so nothing moves).
pub fn xmp_reset_orientation(xmp: &[u8]) -> Vec<u8> {
    let mut out = xmp.to_vec();
    let needle = b"tiff:Orientation";
    let mut from = 0;
    while let Some(p) = find(&out[from..], needle) {
        let at = from + p + needle.len();
        // The attribute form `tiff:Orientation="6"` and the element form `>6<`.
        let digit = if out.get(at) == Some(&b'=') && matches!(out.get(at + 1), Some(b'"' | b'\'')) {
            Some(at + 2)
        } else if out.get(at) == Some(&b'>') {
            Some(at + 1)
        } else {
            None
        };
        if let Some(d) = digit
            .and_then(|i| out.get_mut(i))
            .filter(|d| d.is_ascii_digit())
        {
            *d = b'1';
        }
        from = at;
    }
    out
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

// ---------------------------------------------------------------------------------------------
// Rebuilding the header
// ---------------------------------------------------------------------------------------------

/// The metadata policy of one output (PLAN 3.6, M2.23).
#[derive(Debug, Clone, Copy)]
pub struct MetaPolicy {
    /// The size of the output picture.
    pub dims: (u32, u32),
    /// Drop EXIF GPS, XMP and IPTC.
    pub strip_location: bool,
    /// Carry no metadata at all (`Metadata::Strip`).
    pub strip_all: bool,
}

fn segment(marker: u8, payload: &[u8]) -> Option<Vec<u8>> {
    let len = u16::try_from(payload.len() + 2).ok()?;
    let mut s = vec![0xFF, marker];
    s.extend_from_slice(&len.to_be_bytes());
    s.extend_from_slice(payload);
    Some(s)
}

/// The metadata segments the output carries, as whole segments, in the order they are written:
/// EXIF (patched), XMP (orientation reset), IPTC, comments.
pub fn carried_segments(meta: &JpegMeta, p: &MetaPolicy) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    if p.strip_all {
        return out;
    }
    if let Some(exif) = &meta.exif
        && let Some(patched) = exif_patch(
            exif,
            &ExifPatch {
                dims: p.dims,
                strip_location: p.strip_location,
            },
        )
    {
        let mut payload = EXIF_TAG.to_vec();
        payload.extend_from_slice(&patched);
        out.extend(segment(0xE1, &payload));
    }
    if !p.strip_location {
        if let Some(x) = &meta.xmp {
            let mut payload = XMP_TAG.to_vec();
            payload.extend_from_slice(&xmp_reset_orientation(x));
            out.extend(segment(0xE1, &payload));
        }
        if let Some(i) = &meta.iptc {
            out.extend(segment(0xED, i));
        }
    }
    for c in &meta.comments {
        // A comment can say anything, including where a picture was taken.
        if !p.strip_location {
            out.extend(segment(0xFE, c));
        }
    }
    out
}

/// True for the segments [`rewrite_metadata`] removes from the output before inserting the
/// carried ones.
fn is_metadata_segment(marker: u8, payload: &[u8]) -> bool {
    match marker {
        0xE1 => {
            payload.starts_with(EXIF_TAG)
                || payload.starts_with(XMP_TAG)
                || payload.starts_with(XMP_EXT_TAG)
        }
        0xE2 => payload.starts_with(MPF_TAG),
        0xED => payload.starts_with(PHOTOSHOP_TAG),
        0xFE => true,
        _ => false,
    }
}

/// Rebuilds the header of `jpeg` (see the module docs): its own metadata segments go, the carried
/// ones of `meta` come in after the JFIF segment, the scan data is copied verbatim up to the end-
/// of-image marker and nothing after it is kept.
pub fn rewrite_metadata(
    jpeg: &[u8],
    meta: &JpegMeta,
    p: &MetaPolicy,
) -> Result<Vec<u8>, CodecError> {
    let segs = header_segments(jpeg)?;
    let carried = carried_segments(meta, p);
    let mut out = Vec::with_capacity(jpeg.len() + carried.iter().map(Vec::len).sum::<usize>());
    out.extend_from_slice(&[0xFF, 0xD8]);
    let mut inserted = false;
    let mut insert = |out: &mut Vec<u8>| {
        if !inserted {
            for c in &carried {
                out.extend_from_slice(c);
            }
            inserted = true;
        }
    };
    for s in &segs {
        let payload = &jpeg[s.payload.clone()];
        if is_metadata_segment(s.marker, payload) {
            continue;
        }
        // JFIF (APP0) first, then our metadata, then the rest.
        if s.marker != 0xE0 {
            insert(&mut out);
        }
        out.extend_from_slice(&jpeg[s.whole.clone()]);
    }
    insert(&mut out);
    // The scan data: from the end of the last header segment to the first EOI.
    let scan_start = segs.last().map_or(2, |s| s.whole.end);
    let end = scan_end(jpeg, scan_start)?;
    out.extend_from_slice(&jpeg[scan_start..end]);
    Ok(out)
}

/// The offset just past the end-of-image marker that closes the file whose entropy-coded data
/// starts at `from`: every later scan and table is kept, anything after EOI is not.
fn scan_end(b: &[u8], from: usize) -> Result<usize, CodecError> {
    let n = b.len();
    let mut i = from;
    while i < n {
        // Inside a scan or between segments: find the next real marker.
        match b[i..].iter().position(|&x| x == 0xFF) {
            None => return Err(corrupt("no end-of-image marker")),
            Some(p) => i += p,
        }
        let Some(&m) = b.get(i + 1) else {
            return Err(corrupt("no end-of-image marker"));
        };
        match m {
            0x00 | 0xD0..=0xD7 | 0xFF => i += if m == 0xFF { 1 } else { 2 },
            0xD9 => return Ok(i + 2),
            0xD8 | 0x01 => i += 2,
            _ => {
                // A table or a scan header between scans: skip its segment.
                let len = match b.get(i + 2..i + 4) {
                    Some(l) => usize::from(u16::from_be_bytes([l[0], l[1]])),
                    None => return Err(corrupt("truncated JPEG")),
                };
                if len < 2 || i + 2 + len > n {
                    return Err(corrupt("truncated or malformed JPEG segment"));
                }
                i += 2 + len;
            }
        }
    }
    Err(corrupt("no end-of-image marker"))
}

#[cfg(test)]
mod tests;
