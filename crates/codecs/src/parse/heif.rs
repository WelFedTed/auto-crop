// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! HEIF, HEIC and AVIF header walk (PLAN 3.4, ROADMAP M6.18): the ISO base media file format
//! boxes that say what the primary image is, without a decoder and without allocating pixels.
//!
//! Read: `ftyp`; `meta` with `pitm` (primary item), `iinf` (items), `iref` (tiles, thumbnails,
//! auxiliary images), `iprp`/`ipco`/`ipma` (properties) and `iloc` (sizes of the Exif and XMP
//! items); and, for files that are sequences only, the first video track in `moov`. From those:
//! the stored size (`ispe` after `clap`), bit depth (`pixi`, `av1C` or `hvcC`), the orientation
//! that `irot` and `imir` add up to (as the equivalent EXIF value 1..=8), the ICC profile span
//! (`colr`), the `nclx` colour description, the codec of the primary item and the number of
//! top-level images.
//!
//! Hardening: every offset and size is checked before use (a box that overruns its parent is
//! `Corrupt`), the walk visits at most [`MAX_BOXES`] boxes, the item, reference and property
//! lists are capped (a file past `DecodeLimits::max_frames` items is `LimitExceeded`), and the
//! only allocations are those lists (a few hundred KiB at the caps, never proportional to the
//! declared image size).

use super::{Header, HeifCodec, HeifInfo, IccLoc, Nclx, be16, be32, check_metadata};
use crate::{CodecError, DecodeLimits, Format, Limit};

/// Boxes visited in one file; a legitimate file has well under a thousand.
const MAX_BOXES: usize = 100_000;
/// Properties in `ipco` and references in `iref` that this walk keeps.
const MAX_REFS: usize = 20_000;
/// Properties of one item that are applied (an item has a handful).
const MAX_ITEM_PROPS: usize = 64;

type Fourcc = [u8; 4];

#[derive(Debug, Clone, Copy)]
struct Bx {
    typ: Fourcc,
    /// First byte after the header.
    body: usize,
    end: usize,
}

struct Walk<'a> {
    b: &'a [u8],
    boxes: usize,
}

fn corrupt(msg: &str) -> CodecError {
    CodecError::corrupt(msg)
}

impl<'a> Walk<'a> {
    /// The box at `pos` inside `[pos, end)`; `None` when fewer than a header's worth of bytes are
    /// left (trailing padding is tolerated).
    fn next(&mut self, pos: usize, end: usize) -> Result<Option<Bx>, CodecError> {
        if end.saturating_sub(pos) < 8 {
            return Ok(None);
        }
        self.boxes += 1;
        if self.boxes > MAX_BOXES {
            return Err(corrupt("too many boxes in the HEIF file"));
        }
        let size = u64::from(be32(self.b, pos).ok_or_else(|| corrupt("truncated box"))?);
        let typ: Fourcc = self.b[pos + 4..pos + 8].try_into().expect("4 bytes");
        let (hdr, size) = match size {
            1 => {
                let hi = u64::from(be32(self.b, pos + 8).ok_or_else(|| corrupt("truncated box"))?);
                let lo = u64::from(be32(self.b, pos + 12).ok_or_else(|| corrupt("truncated box"))?);
                (16u64, hi << 32 | lo)
            }
            0 => (8, (end - pos) as u64),
            n => (8, n),
        };
        if size < hdr || size > (end - pos) as u64 {
            return Err(corrupt("a box overruns its parent (truncated HEIF file?)"));
        }
        Ok(Some(Bx {
            typ,
            body: pos + hdr as usize,
            end: pos + size as usize,
        }))
    }

    /// Calls `f` for every child box of `[start, end)`.
    fn each(
        &mut self,
        start: usize,
        end: usize,
        mut f: impl FnMut(&mut Self, Bx) -> Result<(), CodecError>,
    ) -> Result<(), CodecError> {
        let mut pos = start;
        while let Some(bx) = self.next(pos, end)? {
            pos = bx.end;
            f(self, bx)?;
        }
        Ok(())
    }

    fn u8(&self, o: usize) -> Option<u8> {
        self.b.get(o).copied()
    }
    fn u16(&self, o: usize) -> Option<u16> {
        be16(self.b, o)
    }
    fn u32(&self, o: usize) -> Option<u32> {
        be32(self.b, o)
    }
    /// A big-endian unsigned value of `n` (0..=8) bytes.
    fn uint(&self, o: usize, n: usize) -> Option<u64> {
        let s = self.b.get(o..o.checked_add(n)?)?;
        Some(s.iter().fold(0u64, |a, x| a << 8 | u64::from(*x)))
    }
}

#[derive(Debug, Clone, Copy)]
struct Item {
    id: u32,
    typ: Fourcc,
    hidden: bool,
}

const IMAGE_TYPES: [&Fourcc; 11] = [
    b"av01", b"hvc1", b"hev1", b"avc1", b"vvc1", b"jpeg", b"j2k1", b"grid", b"iovl", b"unci",
    b"tmap",
];

/// A transformative property of the primary item, in the order the file lists them.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Xform {
    /// `clap`: the clean aperture size (width, height) when it is well formed.
    Crop(Option<(u32, u32)>),
    /// `irot`: anticlockwise quarter turns.
    Rot(u8),
    /// `imir`: axis 0 flips top-bottom, axis 1 flips left-right. (ISO/IEC 23008-12 words this as
    /// "mirror about a vertical axis" for 0, which is ambiguous; libheif and libavif, whose files
    /// carry the meaning in practice, flip top-bottom for 0. The AVIF fixtures encode this.)
    Mirror(u8),
}

/// Orientation as `R^r o F^m` (flip left-right first, then `r` quarter turns clockwise).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Dihedral {
    r: u8,
    m: bool,
}

impl Dihedral {
    const ID: Self = Self { r: 0, m: false };

    /// `self` followed by `next`.
    fn then(self, next: Self) -> Self {
        // R^r2 F^m2 R^r1 F^m1, and F R^k = R^-k F.
        let r1 = i32::from(self.r);
        let signed = if next.m { -r1 } else { r1 };
        Self {
            r: (i32::from(next.r) + signed).rem_euclid(4) as u8,
            m: self.m ^ next.m,
        }
    }

    /// The EXIF orientation value that undoes exactly this transformation of the stored image.
    fn exif(self) -> u8 {
        match (self.r, self.m) {
            (0, false) => 1,
            (0, true) => 2,
            (2, false) => 3,
            (2, true) => 4,
            (3, true) => 5,
            (1, false) => 6,
            (1, true) => 7,
            _ => 8, // (3, false)
        }
    }
}

fn xform_op(x: Xform) -> Dihedral {
    match x {
        Xform::Crop(_) => Dihedral::ID,
        // irot counts anticlockwise turns; R counts clockwise ones.
        Xform::Rot(a) => Dihedral {
            r: (4 - a % 4) % 4,
            m: false,
        },
        // Axis 1: the left-right flip.
        Xform::Mirror(1) => Dihedral { r: 0, m: true },
        // Axis 0: the top-bottom flip, which is the left-right flip followed by a half turn.
        Xform::Mirror(_) => Dihedral { r: 2, m: true },
    }
}

/// The size after the transformations, and the EXIF-equivalent orientation they add up to.
fn fold(ispe: (u32, u32), xs: &[Xform]) -> ((u32, u32), Dihedral) {
    let (mut w, mut h) = ispe;
    let mut d = Dihedral::ID;
    for x in xs {
        match *x {
            Xform::Crop(Some((cw, ch))) => (w, h) = (cw, ch),
            Xform::Crop(None) => {}
            Xform::Rot(a) if a % 2 == 1 => std::mem::swap(&mut w, &mut h),
            _ => {}
        }
        d = d.then(xform_op(*x));
    }
    ((w, h), d)
}

/// What the properties of one item say.
#[derive(Default)]
struct Props {
    ispe: Option<(u32, u32)>,
    pixi: Option<(u8, u8)>,
    depth_cfg: Option<u8>,
    xforms: Vec<Xform>,
    icc: Option<std::ops::Range<usize>>,
    nclx: Option<Nclx>,
}

pub(crate) fn parse(b: &[u8], format: Format, limits: &DecodeLimits) -> Result<Header, CodecError> {
    let mut w = Walk { b, boxes: 0 };
    let (mut meta, mut moov, mut mini) = (None, None, false);
    let mut brands: Vec<Fourcc> = Vec::new();
    w.each(0, b.len(), |w, bx| {
        match &bx.typ {
            b"ftyp" if brands.is_empty() && bx.end - bx.body >= 8 => {
                brands.push(w.b[bx.body..bx.body + 4].try_into().expect("4 bytes"));
                let mut i = bx.body + 8;
                while i + 4 <= bx.end && brands.len() < 64 {
                    brands.push(w.b[i..i + 4].try_into().expect("4 bytes"));
                    i += 4;
                }
            }
            b"meta" if meta.is_none() => meta = Some(bx),
            b"moov" if moov.is_none() => moov = Some(bx),
            b"mini" => mini = true,
            _ => {}
        }
        Ok(())
    })?;
    if brands.is_empty() {
        return Err(corrupt("HEIF file without an ftyp box"));
    }
    if mini && meta.is_none() {
        return Err(CodecError::UnsupportedFeature(
            "HEIF files using the compact `mini` box".into(),
        ));
    }
    let brand_seq = brands.iter().any(|x| matches!(x, b"msf1" | b"avis"));

    let mut h = match meta {
        Some(m) => {
            let mut h = parse_meta(&mut w, m, format, limits, moov.is_some() || brand_seq)?;
            // A still image next to a sequence: the frame count is the sequence's.
            if let Some(mv) = moov {
                h.frames = h.frames.max(read_moov(&mut w, mv)?.1);
            }
            h
        }
        None => match moov {
            Some(m) => parse_sequence_only(&mut w, m, format)?,
            None => return Err(corrupt("HEIF file without a meta box")),
        },
    };
    if h.frames > limits.max_frames {
        return Err(CodecError::LimitExceeded {
            limit: Limit::Frames,
            actual: u64::from(h.frames),
            cap: u64::from(limits.max_frames),
        });
    }
    if let Some(icc) = h.icc_len {
        check_metadata(u64::from(icc), limits)?;
    }
    h.animated = h.heif.as_ref().is_some_and(|i| i.sequence) || h.frames > 1;
    Ok(h)
}

fn parse_meta(
    w: &mut Walk<'_>,
    meta: Bx,
    format: Format,
    limits: &DecodeLimits,
    has_sequence: bool,
) -> Result<Header, CodecError> {
    let start = meta.body + 4; // version and flags
    let (mut pitm, mut iinf, mut iref, mut iprp, mut iloc) = (None, None, None, None, None);
    w.each(start, meta.end, |_, bx| {
        let slot = match &bx.typ {
            b"pitm" => &mut pitm,
            b"iinf" => &mut iinf,
            b"iref" => &mut iref,
            b"iprp" => &mut iprp,
            b"iloc" => &mut iloc,
            _ => return Ok(()),
        };
        slot.get_or_insert(bx);
        Ok(())
    })?;
    let pitm = pitm.ok_or_else(|| corrupt("HEIF file without a primary item (pitm)"))?;
    let iinf = iinf.ok_or_else(|| corrupt("HEIF file without an item list (iinf)"))?;
    let iprp = iprp.ok_or_else(|| corrupt("HEIF file without item properties (iprp)"))?;

    let primary = match w.u8(pitm.body) {
        Some(0) => w.u16(pitm.body + 4).map(u32::from),
        Some(_) => w.u32(pitm.body + 4),
        None => None,
    }
    .ok_or_else(|| corrupt("truncated pitm box"))?;

    // --- items
    let items = parse_iinf(w, iinf, limits)?;
    let primary_item = items
        .iter()
        .find(|i| i.id == primary)
        .ok_or_else(|| corrupt("the primary item is not in the item list"))?;

    // --- references: which items are tiles, thumbnails or auxiliary images, and the first tile
    let (mut referenced, mut tile0) = (Vec::new(), None);
    if let Some(r) = iref {
        parse_iref(w, r, primary, &mut referenced, &mut tile0)?;
    }
    referenced.sort_unstable();
    let frames = items
        .iter()
        .filter(|i| {
            IMAGE_TYPES.iter().any(|t| **t == i.typ)
                && !i.hidden
                && referenced.binary_search(&i.id).is_err()
        })
        .count()
        .max(1) as u32;

    // --- the properties of the primary item (and of its first tile, which carries the codec
    // configuration of a grid)
    let wanted = [Some(primary), tile0];
    let assoc = parse_ipma(w, iprp, &wanted)?;
    let mut props = [Props::default(), Props::default()];
    read_props(w, iprp, &assoc, &mut props)?;
    let [mut p, t] = props;
    if let Some(r) = t.depth_cfg {
        p.depth_cfg.get_or_insert(r);
    }
    let ispe = p
        .ispe
        .ok_or_else(|| corrupt("the primary item has no ispe property"))?;
    let (disp, d) = fold(ispe, &p.xforms);
    // `Probe` and `Header` give the stored size, before the turn.
    let (width, height) = if d.r % 2 == 1 { (disp.1, disp.0) } else { disp };

    let codec = match primary_item.typ {
        t if &t == b"av01" => HeifCodec::Av1,
        t if &t == b"hvc1" || &t == b"hev1" => HeifCodec::Hevc,
        t if &t == b"grid" || &t == b"iovl" => match tile0
            .and_then(|id| items.iter().find(|i| i.id == id))
            .map(|i| i.typ)
        {
            Some(t) if &t == b"av01" => HeifCodec::Av1,
            Some(t) if &t == b"hvc1" || &t == b"hev1" => HeifCodec::Hevc,
            _ => HeifCodec::Other,
        },
        _ => HeifCodec::Other,
    };

    // Exif and XMP items are not decoded, but a huge one is refused like any metadata blob.
    if let Some(l) = iloc {
        let meta_ids: Vec<u32> = items
            .iter()
            .filter(|i| &i.typ == b"Exif" || &i.typ == b"mime")
            .map(|i| i.id)
            .collect();
        if !meta_ids.is_empty() {
            for len in iloc_lengths(w, l, &meta_ids)? {
                check_metadata(len, limits)?;
            }
        }
    }

    let mut h = Header::new(format, width, height);
    h.bit_depth = p.pixi.map(|(_, bits)| bits).or(p.depth_cfg).unwrap_or(8);
    h.channels = p.pixi.map_or(3, |(n, _)| n.clamp(1, 4));
    h.frames = frames;
    h.orientation = d.exif();
    h.icc = p.icc.clone().map_or(IccLoc::None, IccLoc::Range);
    h.icc_len = p.icc.map(|r| r.len().min(u32::MAX as usize) as u32);
    h.heif = Some(HeifInfo {
        codec,
        nclx: p.nclx,
        sequence: has_sequence,
        ispe,
    });
    Ok(h)
}

fn parse_iinf(w: &mut Walk<'_>, iinf: Bx, limits: &DecodeLimits) -> Result<Vec<Item>, CodecError> {
    let version = w
        .u8(iinf.body)
        .ok_or_else(|| corrupt("truncated iinf box"))?;
    let (count, first) = if version == 0 {
        (w.u16(iinf.body + 4).map(u32::from), iinf.body + 6)
    } else {
        (w.u32(iinf.body + 4), iinf.body + 8)
    };
    let count = count.ok_or_else(|| corrupt("truncated iinf box"))?;
    let cap = limits.max_frames;
    if count > cap {
        return Err(CodecError::LimitExceeded {
            limit: Limit::Frames,
            actual: u64::from(count),
            cap: u64::from(cap),
        });
    }
    let mut items = Vec::with_capacity(count as usize);
    w.each(first, iinf.end, |w, bx| {
        if &bx.typ != b"infe" {
            return Ok(());
        }
        if items.len() as u32 >= cap {
            return Err(CodecError::LimitExceeded {
                limit: Limit::Frames,
                actual: items.len() as u64 + 1,
                cap: u64::from(cap),
            });
        }
        let v = w.u8(bx.body).ok_or_else(|| corrupt("truncated infe box"))?;
        let hidden = w.uint(bx.body + 1, 3).is_some_and(|f| f & 1 == 1);
        let (id, typ_at) = match v {
            2 => (w.u16(bx.body + 4).map(u32::from), bx.body + 8),
            3 => (w.u32(bx.body + 4), bx.body + 10),
            // Versions 0 and 1 carry no item type; such items are not images we know.
            _ => (w.u16(bx.body + 4).map(u32::from), usize::MAX),
        };
        let id = id.ok_or_else(|| corrupt("truncated infe box"))?;
        let typ = if typ_at == usize::MAX {
            [0; 4]
        } else {
            let t =
                w.b.get(typ_at..typ_at.saturating_add(4))
                    .filter(|_| typ_at + 4 <= bx.end)
                    .ok_or_else(|| corrupt("truncated infe box"))?;
            t.try_into().expect("4 bytes")
        };
        items.push(Item { id, typ, hidden });
        Ok(())
    })?;
    Ok(items)
}

fn parse_iref(
    w: &mut Walk<'_>,
    iref: Bx,
    primary: u32,
    referenced: &mut Vec<u32>,
    tile0: &mut Option<u32>,
) -> Result<(), CodecError> {
    let wide = w
        .u8(iref.body)
        .ok_or_else(|| corrupt("truncated iref box"))?
        != 0;
    let idw = if wide { 4 } else { 2 };
    w.each(iref.body + 4, iref.end, |w, bx| {
        let kind = bx.typ;
        if !matches!(&kind, b"dimg" | b"thmb" | b"auxl") {
            return Ok(());
        }
        let from = w
            .uint(bx.body, idw)
            .ok_or_else(|| corrupt("truncated iref entry"))? as u32;
        let n = w
            .u16(bx.body + idw)
            .ok_or_else(|| corrupt("truncated iref entry"))?;
        let mut at = bx.body + idw + 2;
        // `thmb` and `auxl` point from the thumbnail or auxiliary item to the image; `dimg`
        // points from the derived image (a grid) to its tiles. In every case the image that is
        // *not* top-level is: the `from` item for thmb/auxl, the `to` items for dimg.
        for k in 0..n {
            let to = w
                .uint(at, idw)
                .ok_or_else(|| corrupt("truncated iref entry"))? as u32;
            at += idw;
            if referenced.len() >= MAX_REFS {
                return Err(corrupt("too many item references"));
            }
            if &kind == b"dimg" {
                referenced.push(to);
                if from == primary && k == 0 && tile0.is_none() {
                    *tile0 = Some(to);
                }
            } else if k == 0 {
                referenced.push(from);
            }
        }
        Ok(())
    })
}

/// The property indexes (1-based, in file order) associated with each wanted item.
fn parse_ipma(
    w: &mut Walk<'_>,
    iprp: Bx,
    wanted: &[Option<u32>; 2],
) -> Result<[Vec<u16>; 2], CodecError> {
    let mut out = [Vec::new(), Vec::new()];
    let mut found = false;
    w.each(iprp.body, iprp.end, |w, bx| {
        if &bx.typ != b"ipma" || found {
            return Ok(());
        }
        found = true;
        let version = w.u8(bx.body).ok_or_else(|| corrupt("truncated ipma box"))?;
        let wide = w.uint(bx.body + 1, 3).is_some_and(|f| f & 1 == 1);
        let entries = w
            .u32(bx.body + 4)
            .ok_or_else(|| corrupt("truncated ipma box"))?;
        let mut at = bx.body + 8;
        for _ in 0..entries {
            let id = if version < 1 {
                let v = w.u16(at).map(u32::from);
                at += 2;
                v
            } else {
                let v = w.u32(at);
                at += 4;
                v
            }
            .ok_or_else(|| corrupt("truncated ipma box"))?;
            let n = w.u8(at).ok_or_else(|| corrupt("truncated ipma box"))?;
            at += 1;
            let slot = wanted.iter().position(|x| *x == Some(id));
            for _ in 0..n {
                let idx = if wide {
                    let v = w.u16(at).map(|v| v & 0x7FFF);
                    at += 2;
                    v
                } else {
                    let v = w.u8(at).map(|v| u16::from(v & 0x7F));
                    at += 1;
                    v
                }
                .ok_or_else(|| corrupt("truncated ipma box"))?;
                if let Some(s) = slot
                    && out[s].len() < MAX_ITEM_PROPS
                {
                    out[s].push(idx);
                }
            }
            if at > bx.end {
                return Err(corrupt("truncated ipma box"));
            }
        }
        Ok(())
    })?;
    Ok(out)
}

fn read_props(
    w: &mut Walk<'_>,
    iprp: Bx,
    assoc: &[Vec<u16>; 2],
    out: &mut [Props; 2],
) -> Result<(), CodecError> {
    let mut ipco = None;
    w.each(iprp.body, iprp.end, |_, bx| {
        if &bx.typ == b"ipco" {
            ipco.get_or_insert(bx);
        }
        Ok(())
    })?;
    let ipco = ipco.ok_or_else(|| corrupt("HEIF file without a property container (ipco)"))?;
    // Walk the container once and keep only the boxes some item lists (at most a few dozen).
    let mut needed: Vec<(u16, Bx)> = Vec::new();
    let mut index = 0u32;
    w.each(ipco.body, ipco.end, |_, bx| {
        index += 1;
        let idx = index.min(u32::from(u16::MAX)) as u16;
        if assoc.iter().any(|l| l.contains(&idx)) && needed.len() < 2 * MAX_ITEM_PROPS {
            needed.push((idx, bx));
        }
        Ok(())
    })?;
    // Read each item's properties in the order the item lists them: that is the order its
    // transformations (clap, irot, imir) apply in.
    for (slot, list) in assoc.iter().enumerate() {
        for idx in list {
            if let Some((_, bx)) = needed.iter().find(|(i, _)| i == idx) {
                read_prop(w, *bx, &mut out[slot])?;
            }
        }
    }
    Ok(())
}

fn xform_of(w: &Walk<'_>, bx: Bx) -> Option<Xform> {
    match &bx.typ {
        b"clap" => {
            let n = |i: usize| w.u32(bx.body + 4 * i);
            let (wn, wd, hn, hd) = (n(0)?, n(1)?, n(2)?, n(3)?);
            // u64 so a hostile numerator close to u32::MAX cannot overflow the rounding term (found by the
            // `probe` fuzzer); a quotient that does not fit u32 is not a size.
            let size = |num: u32, den: u32| {
                (den != 0)
                    .then(|| (u64::from(num) + u64::from(den) / 2) / u64::from(den))
                    .and_then(|v| u32::try_from(v).ok())
            };
            Some(Xform::Crop(match (size(wn, wd), size(hn, hd)) {
                (Some(cw), Some(ch)) if cw > 0 && ch > 0 => Some((cw, ch)),
                _ => None,
            }))
        }
        b"irot" => Some(Xform::Rot(w.u8(bx.body)? & 3)),
        b"imir" => Some(Xform::Mirror(w.u8(bx.body)? & 1)),
        _ => None,
    }
}

fn read_prop(w: &Walk<'_>, bx: Bx, p: &mut Props) -> Result<(), CodecError> {
    if let Some(x) = xform_of(w, bx) {
        if p.xforms.len() < MAX_ITEM_PROPS {
            p.xforms.push(x);
        }
        return Ok(());
    }
    match &bx.typ {
        b"ispe" if p.ispe.is_none() => {
            p.ispe = Some((
                w.u32(bx.body + 4)
                    .ok_or_else(|| corrupt("truncated ispe box"))?,
                w.u32(bx.body + 8)
                    .ok_or_else(|| corrupt("truncated ispe box"))?,
            ));
        }
        b"pixi" if p.pixi.is_none() => {
            if let (Some(n), Some(bits)) = (w.u8(bx.body + 4), w.u8(bx.body + 5)) {
                p.pixi = Some((n, bits.clamp(1, 32)));
            }
        }
        b"av1C" if p.depth_cfg.is_none() => {
            // byte 2: tier(1) high_bitdepth(1) twelve_bit(1) monochrome(1) ...
            if let Some(f) = w.u8(bx.body + 2) {
                p.depth_cfg = Some(match (f & 0x40 != 0, f & 0x20 != 0) {
                    (true, true) => 12,
                    (true, false) => 10,
                    _ => 8,
                });
            }
        }
        b"hvcC" if p.depth_cfg.is_none() => {
            // HEVCDecoderConfigurationRecord: byte 0 version, 1 profile, 2-5 compatibility flags,
            // 6-11 constraint flags, 12 level, 13-14 segmentation, 15 parallelism, 16 chroma format,
            // 17 reserved(5) bitDepthLumaMinus8(3).
            if let Some(f) = w.u8(bx.body + 17) {
                p.depth_cfg = Some((f & 7) + 8);
            }
        }
        b"colr" => match w.b.get(bx.body..bx.body.saturating_add(4)) {
            Some(b"nclx") if p.nclx.is_none() => {
                if let (Some(cp), Some(tc), Some(mc), Some(fr)) = (
                    w.u16(bx.body + 4),
                    w.u16(bx.body + 6),
                    w.u16(bx.body + 8),
                    w.u8(bx.body + 10),
                ) {
                    p.nclx = Some(Nclx {
                        primaries: cp,
                        transfer: tc,
                        matrix: mc,
                        full_range: fr & 0x80 != 0,
                    });
                }
            }
            Some(b"prof" | b"rICC") if p.icc.is_none() && bx.end > bx.body + 4 => {
                p.icc = Some(bx.body + 4..bx.end);
            }
            _ => {}
        },
        _ => {}
    }
    Ok(())
}

/// Total extent length of each of `ids` in `iloc`.
fn iloc_lengths(w: &Walk<'_>, iloc: Bx, ids: &[u32]) -> Result<Vec<u64>, CodecError> {
    let bad = || corrupt("truncated iloc box");
    let version = w.u8(iloc.body).ok_or_else(bad)?;
    let sizes = w.u8(iloc.body + 4).ok_or_else(bad)?;
    let sizes2 = w.u8(iloc.body + 5).ok_or_else(bad)?;
    let (off_sz, len_sz) = (usize::from(sizes >> 4), usize::from(sizes & 15));
    let (base_sz, idx_sz) = (
        usize::from(sizes2 >> 4),
        if version >= 1 {
            usize::from(sizes2 & 15)
        } else {
            0
        },
    );
    if [off_sz, len_sz, base_sz, idx_sz].iter().any(|n| *n > 8) {
        return Err(corrupt("bad iloc field sizes"));
    }
    let (count, mut at) = if version < 2 {
        (
            w.u16(iloc.body + 6).map(u32::from).ok_or_else(bad)?,
            iloc.body + 8,
        )
    } else {
        (w.u32(iloc.body + 6).ok_or_else(bad)?, iloc.body + 10)
    };
    let mut out = Vec::new();
    for _ in 0..count {
        let id = if version < 2 {
            let v = w.u16(at).map(u32::from);
            at += 2;
            v
        } else {
            let v = w.u32(at);
            at += 4;
            v
        }
        .ok_or_else(bad)?;
        if version >= 1 {
            at += 2; // construction method
        }
        at += 2 + base_sz; // data reference index, base offset
        let extents = w.u16(at).ok_or_else(bad)?;
        at += 2;
        let mut total = 0u64;
        for _ in 0..extents {
            at += idx_sz + off_sz;
            total = total.saturating_add(w.uint(at, len_sz).ok_or_else(bad)?);
            at += len_sz;
        }
        if at > iloc.end {
            return Err(bad());
        }
        if ids.contains(&id) {
            out.push(total);
        }
    }
    Ok(out)
}

/// The size (`tkhd`, 16.16 fixed point) and sample count (`stsz`) of the first track of a `moov`.
fn read_moov(w: &mut Walk<'_>, moov: Bx) -> Result<(Option<(u32, u32)>, u32), CodecError> {
    let mut dims = None;
    let mut frames = 0u32;
    w.each(moov.body, moov.end, |w, trak| {
        if &trak.typ != b"trak" || dims.is_some() {
            return Ok(());
        }
        let (mut tk, mut stsz) = (None, None);
        // trak > tkhd, and trak > mdia > minf > stbl > stsz (sample count)
        w.each(trak.body, trak.end, |w, c| {
            match &c.typ {
                b"tkhd" => tk = Some(c),
                b"mdia" => {
                    w.each(c.body, c.end, |w, m| {
                        if &m.typ == b"minf" {
                            w.each(m.body, m.end, |w, s| {
                                if &s.typ == b"stbl" {
                                    w.each(s.body, s.end, |_, e| {
                                        if &e.typ == b"stsz" {
                                            stsz = Some(e);
                                        }
                                        Ok(())
                                    })?;
                                }
                                Ok(())
                            })?;
                        }
                        Ok(())
                    })?;
                }
                _ => {}
            }
            Ok(())
        })?;
        if let Some(t) = tk {
            let at = t.body + if w.u8(t.body) == Some(1) { 88 } else { 76 };
            if let (Some(tw), Some(th)) = (w.u32(at), w.u32(at + 4)) {
                let (tw, th) = (tw >> 16, th >> 16); // 16.16 fixed point
                if tw > 0 && th > 0 {
                    dims = Some((tw, th));
                }
            }
        }
        if let Some(s) = stsz {
            frames = w.u32(s.body + 8).unwrap_or(0);
        }
        Ok(())
    })?;
    Ok((dims, frames))
}

/// A file with a `moov` and no still image: the size comes from the first visual track.
fn parse_sequence_only(w: &mut Walk<'_>, moov: Bx, format: Format) -> Result<Header, CodecError> {
    let (dims, frames) = read_moov(w, moov)?;
    let (width, height) = dims.ok_or_else(|| corrupt("HEIF sequence without a visual track"))?;
    let mut h = Header::new(format, width, height);
    h.frames = frames.max(1);
    h.heif = Some(HeifInfo {
        codec: HeifCodec::Other,
        nclx: None,
        sequence: true,
        ispe: (width, height),
    });
    Ok(h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dihedral_composition_matches_the_exif_table() {
        let rot = |a| xform_op(Xform::Rot(a));
        let mir = |a| xform_op(Xform::Mirror(a));
        assert_eq!(Dihedral::ID.exif(), 1);
        // irot 1 is a quarter turn anticlockwise: EXIF 8; irot 2: 3; irot 3: 6.
        assert_eq!(rot(1).exif(), 8);
        assert_eq!(rot(2).exif(), 3);
        assert_eq!(rot(3).exif(), 6);
        // imir axis 1 is the left-right flip (EXIF 2), axis 0 the top-bottom flip (EXIF 4): the
        // meaning libavif wrote into the committed fixtures (orient2 has axis 1, orient4 axis 0).
        assert_eq!(mir(1).exif(), 2);
        assert_eq!(mir(0).exif(), 4);
        // Two quarter turns are a half turn; four are the identity; a mirror undoes itself.
        assert_eq!(rot(1).then(rot(1)), rot(2));
        assert_eq!(rot(1).then(rot(3)), Dihedral::ID);
        assert_eq!(mir(0).then(mir(0)), Dihedral::ID);
        assert_eq!(mir(1).then(mir(1)), Dihedral::ID);
        // The two flips make a half turn, in either order.
        assert_eq!(mir(0).then(mir(1)), rot(2));
        // Mirror then a quarter turn is not the same as a quarter turn then the mirror.
        assert_ne!(mir(0).then(rot(1)), rot(1).then(mir(0)));
        // All eight orientations are reachable and distinct.
        let mut seen = [false; 9];
        for a in 0..4 {
            for m in [None, Some(0), Some(1)] {
                let d = match m {
                    None => rot(a),
                    Some(ax) => rot(a).then(mir(ax)),
                };
                seen[usize::from(d.exif())] = true;
            }
        }
        assert!(seen[1..].iter().all(|s| *s));
    }

    /// The table `Dihedral::exif` is derived from, checked against the `image` crate's meaning of
    /// each EXIF value on an asymmetric picture: flip left-right first, then `r` clockwise turns.
    #[test]
    fn the_exif_table_is_what_the_image_crate_does() {
        use image::{ImageBuffer, Rgb, imageops, metadata::Orientation};
        let src: ImageBuffer<Rgb<u8>, Vec<u8>> =
            ImageBuffer::from_fn(3, 2, |x, y| Rgb([x as u8 * 40 + 1, y as u8 * 90 + 5, 7]));
        for r in 0u8..4 {
            for m in [false, true] {
                let mut img = image::DynamicImage::ImageRgb8(src.clone());
                if m {
                    img = image::DynamicImage::ImageRgb8(imageops::flip_horizontal(&img.to_rgb8()));
                }
                for _ in 0..r {
                    img = image::DynamicImage::ImageRgb8(imageops::rotate90(&img.to_rgb8()));
                }
                let o = Dihedral { r, m }.exif();
                let mut want = image::DynamicImage::ImageRgb8(src.clone());
                want.apply_orientation(Orientation::from_exif(o).unwrap());
                assert_eq!(img.to_rgb8(), want.to_rgb8(), "r={r} m={m} exif={o}");
            }
        }
    }

    fn prop_depth(typ: &[u8; 4], payload: &[u8]) -> Option<u8> {
        let mut b = ((8 + payload.len()) as u32).to_be_bytes().to_vec();
        b.extend_from_slice(typ);
        b.extend_from_slice(payload);
        let w = Walk { b: &b, boxes: 0 };
        let bx = Bx {
            typ: *typ,
            body: 8,
            end: b.len(),
        };
        let mut p = Props::default();
        read_prop(&w, bx, &mut p).unwrap();
        p.depth_cfg
    }

    #[test]
    fn the_codec_configuration_boxes_give_the_bit_depth() {
        // hvcC: byte 17 holds bitDepthLumaMinus8 in its low three bits.
        for (code, want) in [(0xF8u8, 8u8), (0xFA, 10), (0xFC, 12)] {
            let mut hvcc = vec![0u8; 23];
            hvcc[0] = 1;
            hvcc[17] = code;
            assert_eq!(prop_depth(b"hvcC", &hvcc), Some(want));
        }
        assert_eq!(prop_depth(b"hvcC", &[1; 10]), None, "too short to say");
        // av1C: byte 2 has high_bitdepth (0x40) and twelve_bit (0x20).
        for (flags, want) in [(0x00u8, 8u8), (0x40, 10), (0x60, 12)] {
            assert_eq!(prop_depth(b"av1C", &[0x81, 0x00, flags, 0x00]), Some(want));
        }
    }

    #[test]
    fn transformations_fold_in_property_order() {
        // 100x60, a quarter turn, then a clean aperture of 20x50 in the turned frame.
        let (size, d) = fold((100, 60), &[Xform::Rot(1), Xform::Crop(Some((20, 50)))]);
        assert_eq!(size, (20, 50));
        assert_eq!(d.exif(), 8);
        // No transformations.
        assert_eq!(fold((7, 9), &[]).0, (7, 9));
        // A malformed clap is ignored.
        assert_eq!(fold((7, 9), &[Xform::Crop(None)]).0, (7, 9));
    }
}
