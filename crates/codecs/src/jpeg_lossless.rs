// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Lossless JPEG transforms in safe Rust (ROADMAP M1.20, PLAN 3.9): rotate, flip, transpose and
//! crop by rearranging the DCT coefficients, never touching a pixel.
//!
//! The file is entropy-decoded to coefficient blocks, the blocks are moved (and, for odd rows or
//! columns of a block, negated, or transposed), and the result is written back with Huffman tables
//! optimised for the new data. Quantisation tables are copied, transposed when the op transposes.
//! Every other marker segment (JFIF, EXIF, XMP, ICC, Adobe, comments) is copied byte for byte, so
//! the EXIF Orientation tag is left exactly as it was: patching it is the caller's job (PLAN 3.6).
//!
//! Supported input: 8-bit sequential Huffman JPEG (baseline and extended), any sampling factors,
//! restart intervals, one interleaved scan covering every component. Progressive, arithmetic-coded,
//! lossless and 12-bit files are refused with `UnsupportedFeature`; the caller re-encodes those.
//! Output is a baseline JPEG with a single interleaved scan and no restart markers.
//!
//! Geometry follows jpegtran. Blocks cannot be mirrored across a partial MCU, so an op that mirrors
//! an axis needs that image dimension to be a multiple of the MCU size: `Policy::Perfect` refuses
//! otherwise, `Policy::Snap` trims the partial edge MCUs (and reports `perfect: false`). A crop
//! always snaps its origin down and its far edge up to the MCU grid and reports the realised
//! rectangle.

use crate::parse::{self, check_metadata};
use crate::{CodecError, DecodeLimits, Format, Limit, guard_item};

/// An axis-aligned rectangle in pixels of the (unrotated) source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// The lossless operations. Rotations are clockwise as displayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Rotate90,
    Rotate180,
    Rotate270,
    FlipH,
    FlipV,
    /// Mirror across the main diagonal (EXIF orientation 5).
    Transpose,
    /// Mirror across the anti-diagonal (EXIF orientation 7).
    Transverse,
    Crop(Rect),
}

/// What to do when the op cannot be exact at the image edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// Refuse (`UnsupportedFeature`) unless the result is exactly what was asked for.
    Perfect,
    /// Trim partial edge MCUs (rotate, flip) or grow the crop to the MCU grid, and report it.
    Snap,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transformed {
    pub bytes: Vec<u8>,
    /// Size of the output image.
    pub width: u32,
    pub height: u32,
    /// The part of the source that the output shows, in source pixels (the crop rectangle as
    /// realised on the MCU grid, or the source minus any trimmed edge).
    pub rect: Rect,
    /// True when the output is exactly the requested operation with nothing trimmed or grown.
    pub perfect: bool,
}

fn unsupported(why: &str) -> CodecError {
    CodecError::UnsupportedFeature(why.to_owned())
}

fn corrupt(why: &str) -> CodecError {
    CodecError::corrupt(why)
}

#[rustfmt::skip]
const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

// ---------------------------------------------------------------------------------------------
// Parsing and entropy decoding
// ---------------------------------------------------------------------------------------------

struct Component {
    id: u8,
    h: usize,
    v: usize,
    tq: usize,
    /// Padded block grid: MCUs across x sampling factor, MCUs down x sampling factor.
    bw: usize,
    bh: usize,
    /// `bw * bh` blocks of 64 coefficients, natural (row-major) order.
    coef: Vec<i16>,
}

struct Huff {
    maxcode: [i32; 18],
    valptr: [i32; 17],
    mincode: [i32; 17],
    vals: Vec<u8>,
}

impl Huff {
    fn new(bits: &[u8; 16], vals: &[u8]) -> Result<Self, CodecError> {
        let total: usize = bits.iter().map(|&b| usize::from(b)).sum();
        if total > 256 || total != vals.len() {
            return Err(corrupt("bad Huffman table"));
        }
        let mut h = Huff {
            maxcode: [-1; 18],
            valptr: [0; 17],
            mincode: [0; 17],
            vals: vals.to_vec(),
        };
        let (mut code, mut k) = (0i32, 0i32);
        for l in 1..=16usize {
            let n = i32::from(bits[l - 1]);
            h.valptr[l] = k;
            h.mincode[l] = code;
            if n > 0 {
                code += n;
                k += n;
                h.maxcode[l] = code - 1;
                if h.maxcode[l] >= (1 << l) {
                    return Err(corrupt("over-subscribed Huffman table"));
                }
            }
            code <<= 1;
        }
        Ok(h)
    }
}

struct Bits<'a> {
    d: &'a [u8],
    pos: usize,
    acc: u32,
    n: u32,
    /// A marker byte was reached; zeros are fed until the caller resynchronises.
    hit_marker: bool,
    /// Zero bytes fed after the data ran out (a few are normal padding, many mean truncation).
    fake: u32,
}

impl<'a> Bits<'a> {
    fn new(d: &'a [u8], pos: usize) -> Self {
        Bits {
            d,
            pos,
            acc: 0,
            n: 0,
            hit_marker: false,
            fake: 0,
        }
    }

    fn fill(&mut self) {
        while self.n <= 24 {
            let byte = if self.hit_marker || self.pos >= self.d.len() {
                self.hit_marker = true;
                self.fake += 1;
                0
            } else if self.d[self.pos] == 0xFF {
                match self.d.get(self.pos + 1) {
                    Some(0x00) => {
                        self.pos += 2;
                        0xFF
                    }
                    _ => {
                        self.hit_marker = true;
                        self.fake += 1;
                        0
                    }
                }
            } else {
                self.pos += 1;
                self.d[self.pos - 1]
            };
            self.acc |= u32::from(byte) << (24 - self.n);
            self.n += 8;
        }
    }

    fn bit(&mut self) -> u32 {
        if self.n == 0 {
            self.fill();
        }
        let b = self.acc >> 31;
        self.acc <<= 1;
        self.n -= 1;
        b
    }

    fn get(&mut self, nbits: u32) -> u32 {
        let mut v = 0;
        for _ in 0..nbits {
            v = (v << 1) | self.bit();
        }
        v
    }

    /// Byte-aligns and consumes the `RSTn` marker that must follow a restart interval.
    fn restart(&mut self, want: u8) -> Result<(), CodecError> {
        // Bytes already pulled into the accumulator but unused are padding; a marker, if the
        // reader has already seen it, is at `pos`.
        self.acc = 0;
        self.n = 0;
        while self.d.get(self.pos) == Some(&0xFF) && self.d.get(self.pos + 1) == Some(&0xFF) {
            self.pos += 1;
        }
        if self.d.get(self.pos) == Some(&0xFF) && self.d.get(self.pos + 1) == Some(&(0xD0 + want)) {
            self.pos += 2;
            self.hit_marker = false;
            self.fake = 0;
            Ok(())
        } else {
            Err(corrupt("missing restart marker"))
        }
    }

    fn huff(&mut self, t: &Huff) -> Result<u8, CodecError> {
        let mut code = 0i32;
        for l in 1..=16usize {
            code = (code << 1) | self.bit() as i32;
            if t.maxcode[l] >= 0 && code <= t.maxcode[l] {
                let i = t.valptr[l] + code - t.mincode[l];
                return t
                    .vals
                    .get(i as usize)
                    .copied()
                    .ok_or_else(|| corrupt("bad Huffman code"));
            }
        }
        Err(corrupt("bad Huffman code"))
    }
}

fn extend(v: u32, t: u32) -> i32 {
    if t == 0 {
        0
    } else if v < (1 << (t - 1)) {
        v as i32 - (1 << t) + 1
    } else {
        v as i32
    }
}

struct Frame {
    width: usize,
    height: usize,
    comps: Vec<Component>,
    hmax: usize,
    vmax: usize,
    mcux: usize,
    mcuy: usize,
}

/// Table specification, in file (zigzag) order.
struct Dqt {
    wide: bool,
    q: [u16; 64],
}

struct Parsed<'a> {
    /// Verbatim segments (marker byte, then the whole segment including its length).
    copy: Vec<(u8, &'a [u8])>,
    dqt: [Option<Dqt>; 4],
    frame: Frame,
}

fn be16(d: &[u8], o: usize) -> Result<usize, CodecError> {
    d.get(o..o + 2)
        .map(|b| usize::from(u16::from_be_bytes([b[0], b[1]])))
        .ok_or_else(|| corrupt("truncated JPEG"))
}

fn parse_all<'a>(d: &'a [u8], limits: &DecodeLimits) -> Result<Parsed<'a>, CodecError> {
    let mut copy: Vec<(u8, &[u8])> = Vec::new();
    let mut dqt: [Option<Dqt>; 4] = [None, None, None, None];
    let mut dc: [Option<Huff>; 4] = [None, None, None, None];
    let mut ac: [Option<Huff>; 4] = [None, None, None, None];
    let mut restart = 0usize;
    let mut frame: Option<Frame> = None;
    let mut scans = 0u32;
    let mut i = 2usize;

    while i + 1 < d.len() {
        if d[i] != 0xFF {
            return Err(corrupt("expected a JPEG marker"));
        }
        while d.get(i + 1) == Some(&0xFF) {
            i += 1;
        }
        let m = *d.get(i + 1).ok_or_else(|| corrupt("truncated JPEG"))?;
        i += 2;
        match m {
            0x00 | 0xD8 => return Err(corrupt("misplaced JPEG marker")),
            0x01 | 0xD0..=0xD7 => continue,
            0xD9 => break,
            _ => {}
        }
        let len = be16(d, i)?;
        if len < 2 || i + len > d.len() {
            return Err(corrupt("truncated JPEG segment"));
        }
        let seg = &d[i + 2..i + len];
        let whole = &d[i - 2..i + len];
        i += len;
        match m {
            0xE0..=0xEF | 0xFE => {
                check_metadata(seg.len() as u64, limits)?;
                copy.push((m, whole));
            }
            0xDB => {
                let mut p = 0;
                while p < seg.len() {
                    let (pq, tq) = (seg[p] >> 4, usize::from(seg[p] & 15));
                    p += 1;
                    let n = if pq == 0 { 64 } else { 128 };
                    if tq > 3 || p + n > seg.len() {
                        return Err(corrupt("bad DQT"));
                    }
                    let mut q = [0u16; 64];
                    for (k, qv) in q.iter_mut().enumerate() {
                        *qv = if pq == 0 {
                            u16::from(seg[p + k])
                        } else {
                            u16::from_be_bytes([seg[p + 2 * k], seg[p + 2 * k + 1]])
                        };
                    }
                    p += n;
                    dqt[tq] = Some(Dqt { wide: pq != 0, q });
                }
            }
            0xC4 => {
                let mut p = 0;
                while p < seg.len() {
                    let (tc, th) = (seg[p] >> 4, usize::from(seg[p] & 15));
                    if p + 17 > seg.len() || th > 3 || tc > 1 {
                        return Err(corrupt("bad DHT"));
                    }
                    let mut bits = [0u8; 16];
                    bits.copy_from_slice(&seg[p + 1..p + 17]);
                    let n: usize = bits.iter().map(|&b| usize::from(b)).sum();
                    if p + 17 + n > seg.len() {
                        return Err(corrupt("bad DHT"));
                    }
                    let h = Huff::new(&bits, &seg[p + 17..p + 17 + n])?;
                    if tc == 0 {
                        dc[th] = Some(h);
                    } else {
                        ac[th] = Some(h);
                    }
                    p += 17 + n;
                }
            }
            0xDD => {
                restart = be16(seg, 0)?;
            }
            0xC0 | 0xC1 => {
                if frame.is_some() {
                    return Err(corrupt("two frame headers"));
                }
                frame = Some(parse_frame(seg, limits)?);
            }
            0xC2 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF | 0xC3 => {
                return Err(unsupported(
                    "lossless transforms need sequential Huffman JPEG (not progressive, lossless or arithmetic-coded)",
                ));
            }
            0xDA => {
                let f = frame.as_mut().ok_or_else(|| corrupt("scan before frame"))?;
                scans += 1;
                if scans > limits.max_scans {
                    return Err(CodecError::LimitExceeded {
                        limit: Limit::Scans,
                        actual: u64::from(scans),
                        cap: u64::from(limits.max_scans),
                    });
                }
                i = decode_scan(d, i, seg, f, &dc, &ac, restart)?;
            }
            _ => {}
        }
    }
    let frame = frame.ok_or_else(|| corrupt("no frame header"))?;
    if scans == 0 {
        return Err(corrupt("no scan"));
    }
    Ok(Parsed { copy, dqt, frame })
}

fn parse_frame(seg: &[u8], limits: &DecodeLimits) -> Result<Frame, CodecError> {
    if seg.len() < 6 {
        return Err(corrupt("short frame header"));
    }
    if seg[0] != 8 {
        return Err(unsupported("only 8-bit JPEG can be transformed losslessly"));
    }
    let height = be16(seg, 1)?;
    let width = be16(seg, 3)?;
    let n = usize::from(seg[5]);
    if width == 0 || height == 0 {
        return Err(corrupt("zero dimension"));
    }
    if n == 0 || n > 4 || seg.len() < 6 + 3 * n {
        return Err(corrupt("bad component count"));
    }
    let pixels = (width * height) as u64;
    if pixels > limits.max_pixels {
        return Err(CodecError::TooLarge(pixels));
    }
    let mut comps = Vec::new();
    for c in 0..n {
        let b = &seg[6 + 3 * c..9 + 3 * c];
        let (h, v) = (usize::from(b[1] >> 4), usize::from(b[1] & 15));
        if !(1..=4).contains(&h) || !(1..=4).contains(&v) || b[2] > 3 {
            return Err(corrupt("bad sampling factors"));
        }
        let (h, v) = if n == 1 { (1, 1) } else { (h, v) };
        comps.push(Component {
            id: b[0],
            h,
            v,
            tq: usize::from(b[2]),
            bw: 0,
            bh: 0,
            coef: Vec::new(),
        });
    }
    let hmax = comps.iter().map(|c| c.h).max().unwrap_or(1);
    let vmax = comps.iter().map(|c| c.v).max().unwrap_or(1);
    let mcux = width.div_ceil(8 * hmax);
    let mcuy = height.div_ceil(8 * vmax);
    // Coefficients are 2 bytes per sample; keep to the pixel cap's memory estimate.
    let mut est = 0u64;
    for c in &mut comps {
        c.bw = mcux * c.h;
        c.bh = mcuy * c.v;
        est += (c.bw * c.bh * 64 * 2) as u64;
    }
    if est > limits.max_est_bytes {
        return Err(CodecError::LimitExceeded {
            limit: Limit::EstBytes,
            actual: est,
            cap: limits.max_est_bytes,
        });
    }
    for c in &mut comps {
        c.coef = vec![0i16; c.bw * c.bh * 64];
    }
    Ok(Frame {
        width,
        height,
        comps,
        hmax,
        vmax,
        mcux,
        mcuy,
    })
}

/// Decodes one scan into `f` and returns the offset of the first byte after its entropy data.
fn decode_scan(
    d: &[u8],
    start: usize,
    hdr: &[u8],
    f: &mut Frame,
    dc: &[Option<Huff>; 4],
    ac: &[Option<Huff>; 4],
    restart: usize,
) -> Result<usize, CodecError> {
    let ns = usize::from(*hdr.first().ok_or_else(|| corrupt("short scan header"))?);
    if ns == 0 || ns > 4 || hdr.len() < 1 + 2 * ns + 3 {
        return Err(corrupt("bad scan header"));
    }
    let (ss, se, a) = (hdr[1 + 2 * ns], hdr[2 + 2 * ns], hdr[3 + 2 * ns]);
    if ss != 0 || se != 63 || a != 0 {
        return Err(unsupported("progressive scan"));
    }
    // (component index, DC table, AC table)
    let mut sc: Vec<(usize, usize, usize)> = Vec::new();
    for k in 0..ns {
        let cid = hdr[1 + 2 * k];
        let t = hdr[2 + 2 * k];
        let ci = f
            .comps
            .iter()
            .position(|c| c.id == cid)
            .ok_or_else(|| corrupt("scan names an unknown component"))?;
        if sc.iter().any(|s| s.0 == ci) {
            return Err(corrupt("component repeated in a scan"));
        }
        sc.push((ci, usize::from(t >> 4).min(3), usize::from(t & 15).min(3)));
    }
    if ns != f.comps.len() {
        return Err(unsupported(
            "non-interleaved JPEG (a scan per component) is not transformed losslessly",
        ));
    }
    let mut bits = Bits::new(d, start);
    let mut pred = [0i32; 4];
    let mut since_restart = 0usize;
    let mut rst = 0u8;

    let (units_x, units_y) = (f.mcux, f.mcuy);

    for uy in 0..units_y {
        for ux in 0..units_x {
            if restart > 0 && since_restart == restart {
                bits.restart(rst)?;
                rst = (rst + 1) & 7;
                pred = [0; 4];
                since_restart = 0;
            }
            since_restart += 1;
            for &(ci, td, ta) in &sc {
                let dct = dc[td].as_ref().ok_or_else(|| corrupt("missing DC table"))?;
                let act = ac[ta].as_ref().ok_or_else(|| corrupt("missing AC table"))?;
                let (h, v, bw) = {
                    let c = &f.comps[ci];
                    (c.h, c.v, c.bw)
                };
                for by in 0..v {
                    for bx in 0..h {
                        let (x, y) = (ux * h + bx, uy * v + by);
                        let at = (y * bw + x) * 64;
                        let block = f.comps[ci]
                            .coef
                            .get_mut(at..at + 64)
                            .ok_or_else(|| corrupt("block outside the frame"))?;
                        decode_block(&mut bits, dct, act, &mut pred[ci], block)?;
                    }
                }
            }
            if bits.fake > 8 {
                return Err(corrupt("scan data ends early"));
            }
        }
    }
    // Skip to the next marker.
    let mut p = bits.pos;
    while p + 1 < d.len()
        && !(d[p] == 0xFF && d[p + 1] != 0x00 && !(0xD0..=0xD7).contains(&d[p + 1]))
    {
        p += 1;
    }
    Ok(p)
}

fn decode_block(
    bits: &mut Bits,
    dc: &Huff,
    ac: &Huff,
    pred: &mut i32,
    out: &mut [i16],
) -> Result<(), CodecError> {
    let t = u32::from(bits.huff(dc)?);
    if t > 16 {
        return Err(corrupt("bad DC category"));
    }
    let diff = if t == 0 { 0 } else { extend(bits.get(t), t) };
    *pred = pred.wrapping_add(diff);
    out[0] = i16::try_from(*pred).map_err(|_| corrupt("DC out of range"))?;
    let mut k = 1usize;
    while k < 64 {
        let rs = bits.huff(ac)?;
        let (r, s) = (usize::from(rs >> 4), u32::from(rs & 15));
        if s == 0 {
            if r == 15 {
                k += 16;
                continue;
            }
            break;
        }
        k += r;
        if k > 63 {
            return Err(corrupt("AC run past the block"));
        }
        let v = extend(bits.get(s), s);
        out[ZIGZAG[k]] = i16::try_from(v).map_err(|_| corrupt("AC out of range"))?;
        k += 1;
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Coefficient-domain transforms
// ---------------------------------------------------------------------------------------------

/// Keeps MCUs `[mx0, mx1) x [my0, my1)` of a padded component plane.
fn crop_plane(c: &mut Component, mx0: usize, mx1: usize, my0: usize, my1: usize) {
    let (x0, x1, y0, y1) = (mx0 * c.h, mx1 * c.h, my0 * c.v, my1 * c.v);
    let nbw = x1 - x0;
    let mut out = Vec::with_capacity(nbw * (y1 - y0) * 64);
    for by in y0..y1 {
        let s = (by * c.bw + x0) * 64;
        out.extend_from_slice(&c.coef[s..s + nbw * 64]);
    }
    c.coef = out;
    c.bw = nbw;
    c.bh = y1 - y0;
}

fn flip_h(c: &mut Component) {
    let (bw, bh) = (c.bw, c.bh);
    for by in 0..bh {
        for bx in 0..bw / 2 {
            let (a, b) = ((by * bw + bx) * 64, (by * bw + bw - 1 - bx) * 64);
            for k in 0..64 {
                c.coef.swap(a + k, b + k);
            }
        }
    }
    for blk in c.coef.as_chunks_mut::<64>().0 {
        for (k, v) in blk.iter_mut().enumerate() {
            if k & 1 == 1 {
                *v = v.wrapping_neg();
            }
        }
    }
}

fn flip_v(c: &mut Component) {
    let (bw, bh) = (c.bw, c.bh);
    for by in 0..bh / 2 {
        let (a, b) = (by * bw * 64, (bh - 1 - by) * bw * 64);
        for k in 0..bw * 64 {
            c.coef.swap(a + k, b + k);
        }
    }
    for blk in c.coef.as_chunks_mut::<64>().0 {
        for (k, v) in blk.iter_mut().enumerate() {
            if (k >> 3) & 1 == 1 {
                *v = v.wrapping_neg();
            }
        }
    }
}

fn transpose(c: &mut Component) {
    let (bw, bh) = (c.bw, c.bh);
    let mut out = vec![0i16; c.coef.len()];
    for by in 0..bh {
        for bx in 0..bw {
            let s = (by * bw + bx) * 64;
            let d = (bx * bh + by) * 64;
            for v in 0..8 {
                for u in 0..8 {
                    out[d + u * 8 + v] = c.coef[s + v * 8 + u];
                }
            }
        }
    }
    c.coef = out;
    c.bw = bh;
    c.bh = bw;
    std::mem::swap(&mut c.h, &mut c.v);
}

// ---------------------------------------------------------------------------------------------
// Entropy encoding with optimised tables (ITU-T T.81 Annex K.2)
// ---------------------------------------------------------------------------------------------

/// Huffman code lengths and values for the symbol frequencies (index 256 is the reserved
/// all-ones guard).
fn optimal_table(mut freq: [u64; 257]) -> ([u8; 17], Vec<u8>) {
    freq[256] = 1;
    let mut size = [0usize; 257];
    let mut others = [-1i32; 257];
    loop {
        let mut c1 = -1i32;
        let mut v = u64::MAX;
        for (i, &f) in freq.iter().enumerate() {
            if f != 0 && f <= v {
                v = f;
                c1 = i as i32;
            }
        }
        let mut c2 = -1i32;
        v = u64::MAX;
        for (i, &f) in freq.iter().enumerate() {
            if f != 0 && f <= v && i as i32 != c1 {
                v = f;
                c2 = i as i32;
            }
        }
        if c2 < 0 {
            break;
        }
        let (mut a, mut b) = (c1 as usize, c2 as usize);
        freq[a] += freq[b];
        freq[b] = 0;
        size[a] += 1;
        while others[a] >= 0 {
            a = others[a] as usize;
            size[a] += 1;
        }
        others[a] = b as i32;
        size[b] += 1;
        while others[b] >= 0 {
            b = others[b] as usize;
            size[b] += 1;
        }
    }
    let mut bits = [0u32; 258];
    for &s in &size {
        if s > 0 {
            bits[s.min(257)] += 1;
        }
    }
    for i in (17..258).rev() {
        while bits[i] > 0 {
            let mut j = i - 2;
            while bits[j] == 0 {
                j -= 1;
            }
            bits[i] -= 2;
            bits[i - 1] += 1;
            bits[j + 1] += 2;
            bits[j] -= 1;
        }
    }
    let mut i = 16;
    while bits[i] == 0 {
        i -= 1;
    }
    bits[i] -= 1; // drop the reserved code point
    let mut out_bits = [0u8; 17];
    for (o, b) in out_bits[1..].iter_mut().zip(&bits[1..=16]) {
        *o = *b as u8;
    }
    // Symbols by code length, then by value (anything deeper than 32 sorts last).
    let mut order: Vec<usize> = (0..256).filter(|&s| size[s] > 0).collect();
    order.sort_by_key(|&s| (size[s], s));
    let vals: Vec<u8> = order.into_iter().map(|s| s as u8).collect();
    (out_bits, vals)
}

struct EncTable {
    code: [u32; 256],
    len: [u8; 256],
}

impl EncTable {
    fn new(bits: &[u8; 17], vals: &[u8]) -> Self {
        let mut t = EncTable {
            code: [0; 256],
            len: [0; 256],
        };
        let (mut code, mut k) = (0u32, 0usize);
        for (l, &n) in bits.iter().enumerate().skip(1) {
            for _ in 0..n {
                if let Some(&sym) = vals.get(k) {
                    t.code[usize::from(sym)] = code;
                    t.len[usize::from(sym)] = l as u8;
                }
                code += 1;
                k += 1;
            }
            code <<= 1;
        }
        t
    }
}

struct BitWriter {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl BitWriter {
    fn put(&mut self, code: u32, len: u32) {
        if len == 0 {
            return;
        }
        self.acc = (self.acc << len) | u64::from(code & ((1u32 << len) - 1));
        self.n += len;
        while self.n >= 8 {
            let b = (self.acc >> (self.n - 8)) as u8;
            self.out.push(b);
            if b == 0xFF {
                self.out.push(0);
            }
            self.n -= 8;
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.n > 0 {
            let pad = 8 - self.n;
            self.put((1 << pad) - 1, pad);
        }
        self.out
    }
}

fn category(v: i32) -> u32 {
    32 - v.unsigned_abs().leading_zeros()
}

/// One block's symbols, as the entropy coder will emit them: `visit(is_dc, symbol, extra_bits,
/// extra_len)` in order.
fn block_symbols(blk: &[i16], pred: &mut i32, mut visit: impl FnMut(bool, u8, u32, u32)) {
    let dc = i32::from(blk[0]);
    let diff = dc - *pred;
    *pred = dc;
    let s = category(diff);
    let bits = if diff < 0 {
        (diff - 1) as u32
    } else {
        diff as u32
    };
    visit(true, s as u8, bits, s);
    let mut run = 0u32;
    for &zz in &ZIGZAG[1..] {
        let v = i32::from(blk[zz]);
        if v == 0 {
            run += 1;
            continue;
        }
        while run > 15 {
            visit(false, 0xF0, 0, 0);
            run -= 16;
        }
        let s = category(v);
        let bits = if v < 0 { (v - 1) as u32 } else { v as u32 };
        visit(false, ((run << 4) | s) as u8, bits, s);
        run = 0;
    }
    if run > 0 {
        visit(false, 0x00, 0, 0);
    }
}

/// Huffman code lengths (index 1..=16) and symbol values of one table.
type Table = ([u8; 17], Vec<u8>);

fn encode_scan(f: &Frame) -> (Vec<u8>, Vec<Table>) {
    // Table class per component: luma-like (0) or chroma-like (1).
    let class = |ci: usize| usize::from(ci != 0);
    let mut freq = [[[0u64; 257]; 2]; 2]; // [dc|ac][class]
    let walk = |emit: &mut dyn FnMut(usize, bool, u8, u32, u32)| {
        let mut pred = vec![0i32; f.comps.len()];
        for my in 0..f.mcuy {
            for mx in 0..f.mcux {
                for (ci, c) in f.comps.iter().enumerate() {
                    for by in 0..c.v {
                        for bx in 0..c.h {
                            let at = ((my * c.v + by) * c.bw + mx * c.h + bx) * 64;
                            block_symbols(
                                &c.coef[at..at + 64],
                                &mut pred[ci],
                                |dc, sym, eb, el| emit(ci, dc, sym, eb, el),
                            );
                        }
                    }
                }
            }
        }
    };
    walk(&mut |ci, dc, sym, _, _| {
        freq[usize::from(!dc)][class(ci)][usize::from(sym)] += 1;
    });
    let nclass = if f.comps.len() > 1 { 2 } else { 1 };
    let mut tables: Vec<Table> = Vec::new(); // dc0, dc1, ac0, ac1
    for per_kind in &freq {
        for per_class in per_kind.iter().take(nclass) {
            tables.push(optimal_table(*per_class));
        }
    }
    let enc: Vec<EncTable> = tables.iter().map(|(b, v)| EncTable::new(b, v)).collect();
    let mut w = BitWriter {
        out: Vec::new(),
        acc: 0,
        n: 0,
    };
    walk(&mut |ci, dc, sym, eb, el| {
        let t = &enc[usize::from(!dc) * nclass + class(ci)];
        w.put(t.code[usize::from(sym)], u32::from(t.len[usize::from(sym)]));
        w.put(eb, el);
    });
    (w.finish(), tables)
}

fn segment(out: &mut Vec<u8>, marker: u8, payload: &[u8]) {
    out.extend_from_slice(&[0xFF, marker]);
    out.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(payload);
}

fn write_jpeg(
    p: &Parsed,
    f: &Frame,
    transposed: bool,
    width: usize,
    height: usize,
) -> Result<Vec<u8>, CodecError> {
    let mut out = vec![0xFF, 0xD8];
    for (_, whole) in &p.copy {
        out.extend_from_slice(whole);
    }
    for (tq, t) in p.dqt.iter().enumerate() {
        let Some(t) = t else { continue };
        let mut q = t.q;
        if transposed {
            // The coefficient at (u, v) moved to (v, u): so did its quantiser.
            let mut nat = [0u16; 64];
            for k in 0..64 {
                nat[ZIGZAG[k]] = t.q[k];
            }
            for k in 0..64 {
                let n = ZIGZAG[k];
                q[k] = nat[(n % 8) * 8 + n / 8];
            }
        }
        let mut pl = vec![(u8::from(t.wide) << 4) | tq as u8];
        for v in q {
            if t.wide {
                pl.extend_from_slice(&v.to_be_bytes());
            } else {
                pl.push(v as u8);
            }
        }
        segment(&mut out, 0xDB, &pl);
    }
    let mut sof = vec![8];
    sof.extend_from_slice(&(height as u16).to_be_bytes());
    sof.extend_from_slice(&(width as u16).to_be_bytes());
    sof.push(f.comps.len() as u8);
    for c in &f.comps {
        sof.extend_from_slice(&[c.id, ((c.h as u8) << 4) | c.v as u8, c.tq as u8]);
    }
    segment(&mut out, 0xC0, &sof);

    let (data, tables) = encode_scan(f);
    let nclass = tables.len() / 2;
    for (i, (bits, vals)) in tables.iter().enumerate() {
        let (tc, th) = if i < nclass {
            (0u8, i as u8)
        } else {
            (1u8, (i - nclass) as u8)
        };
        let mut pl = vec![(tc << 4) | th];
        pl.extend_from_slice(&bits[1..]);
        pl.extend_from_slice(vals);
        segment(&mut out, 0xC4, &pl);
    }
    let mut sos = vec![f.comps.len() as u8];
    for (ci, c) in f.comps.iter().enumerate() {
        let t = u8::from(ci != 0 && nclass > 1);
        sos.extend_from_slice(&[c.id, (t << 4) | t]);
    }
    sos.extend_from_slice(&[0, 63, 0]);
    segment(&mut out, 0xDA, &sos);
    out.extend(data);
    out.extend_from_slice(&[0xFF, 0xD9]);
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------------------------

/// Applies `op` to the JPEG `bytes` without decoding any pixel. See the module docs for what is
/// supported and what `policy` means.
pub fn transform(
    bytes: &[u8],
    op: Op,
    policy: Policy,
    limits: &DecodeLimits,
) -> Result<Transformed, CodecError> {
    guard_item(|| transform_inner(bytes, op, policy, limits))
}

fn transform_inner(
    bytes: &[u8],
    op: Op,
    policy: Policy,
    limits: &DecodeLimits,
) -> Result<Transformed, CodecError> {
    if crate::sniff(bytes) != Some(Format::Jpeg) {
        return Err(CodecError::Unsupported);
    }
    let cap = limits.file_cap(Format::Jpeg);
    if bytes.len() as u64 > cap {
        return Err(CodecError::LimitExceeded {
            limit: Limit::FileBytes,
            actual: bytes.len() as u64,
            cap,
        });
    }
    // The shared header walk enforces the metadata and scan caps and the truncation rule.
    let h = parse::parse(bytes, Format::Jpeg, limits)?;
    if let Some(why) = &h.unsupported {
        return Err(CodecError::UnsupportedFeature(why.clone()));
    }
    if h.truncated {
        return Err(corrupt("truncated JPEG (no end-of-image marker)"));
    }
    parse::jpeg::check_plausible(&h, bytes.len())?;
    let mut p = parse_all(bytes, limits)?;
    let (w, ht) = (p.frame.width, p.frame.height);
    let (mcu_w, mcu_h) = (8 * p.frame.hmax, 8 * p.frame.vmax);

    // Which axes the op mirrors (and so must be MCU-aligned), and whether it transposes.
    let (need_w, need_h, transposes) = match op {
        Op::Rotate90 => (false, true, true),
        Op::Rotate270 => (true, false, true),
        Op::Rotate180 | Op::Transverse => (true, true, matches!(op, Op::Transverse)),
        Op::FlipH => (true, false, false),
        Op::FlipV => (false, true, false),
        Op::Transpose => (false, false, true),
        Op::Crop(_) => (false, false, false),
    };

    // The part of the source the output will show.
    let (x0, y0, x1, y1) = match op {
        Op::Crop(r) => {
            if r.w == 0 || r.h == 0 || r.x as usize >= w || r.y as usize >= ht {
                return Err(CodecError::corrupt(
                    "crop rectangle is empty or outside the image",
                ));
            }
            let rx1 = (r.x as usize + r.w as usize).min(w);
            let ry1 = (r.y as usize + r.h as usize).min(ht);
            (
                r.x as usize / mcu_w * mcu_w,
                r.y as usize / mcu_h * mcu_h,
                (rx1.div_ceil(mcu_w) * mcu_w).min(w),
                (ry1.div_ceil(mcu_h) * mcu_h).min(ht),
            )
        }
        _ => (
            0,
            0,
            if need_w { w / mcu_w * mcu_w } else { w },
            if need_h { ht / mcu_h * mcu_h } else { ht },
        ),
    };
    if x1 <= x0 || y1 <= y0 {
        return Err(unsupported(
            "the image is smaller than one MCU on an axis the transform mirrors",
        ));
    }
    let rect = Rect {
        x: x0 as u32,
        y: y0 as u32,
        w: (x1 - x0) as u32,
        h: (y1 - y0) as u32,
    };
    let perfect = match op {
        Op::Crop(r) => rect == r,
        _ => (x1 - x0, y1 - y0) == (w, ht),
    };
    if !perfect && policy == Policy::Perfect {
        return Err(unsupported(
            "the transform is not perfect for this image size (partial MCUs at the edge)",
        ));
    }

    // Keep only the MCUs of the shown part, then move blocks.
    let f = &mut p.frame;
    if (x0, y0, x1, y1) != (0, 0, w, ht) {
        let (mx0, mx1) = (x0 / mcu_w, x1.div_ceil(mcu_w));
        let (my0, my1) = (y0 / mcu_h, y1.div_ceil(mcu_h));
        for c in &mut f.comps {
            crop_plane(c, mx0, mx1, my0, my1);
        }
        f.mcux = mx1 - mx0;
        f.mcuy = my1 - my0;
    }
    let (mut ow, mut oh) = (x1 - x0, y1 - y0);
    match op {
        Op::Crop(_) => {}
        Op::FlipH => f.comps.iter_mut().for_each(flip_h),
        Op::FlipV => f.comps.iter_mut().for_each(flip_v),
        Op::Rotate180 => f.comps.iter_mut().for_each(|c| {
            flip_h(c);
            flip_v(c);
        }),
        Op::Transpose => f.comps.iter_mut().for_each(transpose),
        Op::Rotate90 => f.comps.iter_mut().for_each(|c| {
            transpose(c);
            flip_h(c);
        }),
        Op::Rotate270 => f.comps.iter_mut().for_each(|c| {
            transpose(c);
            flip_v(c);
        }),
        Op::Transverse => f.comps.iter_mut().for_each(|c| {
            transpose(c);
            flip_h(c);
            flip_v(c);
        }),
    }
    if transposes {
        std::mem::swap(&mut f.mcux, &mut f.mcuy);
        std::mem::swap(&mut f.hmax, &mut f.vmax);
        std::mem::swap(&mut ow, &mut oh);
    }
    let out = write_jpeg(&p, &p.frame, transposes, ow, oh)?;
    Ok(Transformed {
        bytes: out,
        width: ow as u32,
        height: oh as u32,
        rect,
        perfect,
    })
}
