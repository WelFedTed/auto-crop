// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The ISOBMFF/HEIF container of one still AVIF picture, written and read here with no dependency
//! (ROADMAP M11.23). It is compiled in every build (the verify step reads files with it); the
//! encoder that feeds it is behind the `avif` feature.

/// The payload of the `Exif` item: the 4-byte big-endian offset to the TIFF header (0: it starts
/// at once) and the blob. `None` when the blob is not a TIFF structure (a leading `Exif\0\0` is
/// removed first).
pub fn exif_item_payload(blob: &[u8]) -> Option<Vec<u8>> {
    let blob = blob.strip_prefix(b"Exif\0\0").unwrap_or(blob);
    if !(blob.starts_with(b"II*\0") || blob.starts_with(b"MM\0*")) {
        return None;
    }
    let mut out = Vec::with_capacity(blob.len() + 4);
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(blob);
    Some(out)
}

/// What goes into the file.
pub struct Picture<'a> {
    pub width: u32,
    pub height: u32,
    pub monochrome: bool,
    pub subsampled: bool,
    /// The `av1C` record (4 bytes, then the sequence header OBU).
    pub av1c: &'a [u8],
    /// The AV1 temporal unit.
    pub av1_data: &'a [u8],
    pub icc: Option<&'a [u8]>,
    /// The `nclx` box claims no primaries or transfer (it accompanies a profile).
    pub nclx_unspecified: bool,
    /// The `Exif` item payload (offset prefix included).
    pub exif: Option<&'a [u8]>,
}

fn put_box(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&((body.len() + 8) as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
}

fn full(version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
    let mut v = vec![version];
    v.extend_from_slice(&flags.to_be_bytes()[1..]);
    v.extend_from_slice(body);
    v
}

fn boxed(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(body.len() + 8);
    put_box(&mut v, kind, body);
    v
}

/// The whole file: `ftyp`, `meta`, `mdat`.
pub fn write(p: &Picture<'_>) -> Vec<u8> {
    let mut ftyp = Vec::new();
    ftyp.extend_from_slice(b"avif");
    ftyp.extend_from_slice(&0u32.to_be_bytes());
    for b in [b"avif", b"mif1", b"miaf"] {
        ftyp.extend_from_slice(b);
    }
    let ftyp = boxed(b"ftyp", &ftyp);

    // Properties, 1-based in `ipco` order.
    let mut ipco = Vec::new();
    let mut assoc: Vec<u8> = Vec::new();
    let mut index = 0u8;
    let mut add = |ipco: &mut Vec<u8>, b: Vec<u8>, essential: bool| {
        ipco.extend_from_slice(&b);
        index += 1;
        assoc.push(index | if essential { 0x80 } else { 0 });
    };
    let mut ispe = p.width.to_be_bytes().to_vec();
    ispe.extend_from_slice(&p.height.to_be_bytes());
    add(&mut ipco, boxed(b"ispe", &full(0, 0, &ispe)), false);
    let channels: u8 = if p.monochrome { 1 } else { 3 };
    let mut pixi = vec![channels];
    pixi.extend(std::iter::repeat_n(8u8, usize::from(channels)));
    add(&mut ipco, boxed(b"pixi", &full(0, 0, &pixi)), false);
    add(&mut ipco, boxed(b"av1C", p.av1c), true);
    let mut nclx = b"nclx".to_vec();
    let (cp, tc): (u16, u16) = if p.nclx_unspecified { (2, 2) } else { (1, 13) };
    nclx.extend_from_slice(&cp.to_be_bytes());
    nclx.extend_from_slice(&tc.to_be_bytes());
    nclx.extend_from_slice(&6u16.to_be_bytes());
    nclx.push(0x80); // full range
    add(&mut ipco, boxed(b"colr", &nclx), false);
    if let Some(icc) = p.icc {
        let mut prof = b"prof".to_vec();
        prof.extend_from_slice(icc);
        add(&mut ipco, boxed(b"colr", &prof), false);
    }
    let ipco = boxed(b"ipco", &ipco);
    let mut ipma = 1u32.to_be_bytes().to_vec();
    ipma.extend_from_slice(&1u16.to_be_bytes());
    ipma.push(assoc.len() as u8);
    ipma.extend_from_slice(&assoc);
    let mut iprp = ipco;
    iprp.extend_from_slice(&boxed(b"ipma", &full(0, 0, &ipma)));
    let iprp = boxed(b"iprp", &iprp);

    let mut hdlr = 0u32.to_be_bytes().to_vec();
    hdlr.extend_from_slice(b"pict");
    hdlr.extend_from_slice(&[0; 12]);
    hdlr.push(0);
    let hdlr = boxed(b"hdlr", &full(0, 0, &hdlr));
    let pitm = boxed(b"pitm", &full(0, 0, &1u16.to_be_bytes()));

    let infe = |id: u16, kind: &[u8; 4]| {
        let mut b = id.to_be_bytes().to_vec();
        b.extend_from_slice(&0u16.to_be_bytes());
        b.extend_from_slice(kind);
        b.push(0);
        boxed(b"infe", &full(2, 0, &b))
    };
    let n_items: u16 = 1 + u16::from(p.exif.is_some());
    let mut iinf = n_items.to_be_bytes().to_vec();
    iinf.extend_from_slice(&infe(1, b"av01"));
    if p.exif.is_some() {
        iinf.extend_from_slice(&infe(2, b"Exif"));
    }
    let iinf = boxed(b"iinf", &full(0, 0, &iinf));
    let iref = p.exif.map(|_| {
        let mut cdsc = 2u16.to_be_bytes().to_vec();
        cdsc.extend_from_slice(&1u16.to_be_bytes());
        cdsc.extend_from_slice(&1u16.to_be_bytes());
        boxed(b"iref", &full(0, 0, &boxed(b"cdsc", &cdsc)))
    });

    // `iloc` has a fixed size per item count, so the offsets can be written once.
    let iloc_len = 8 + 4 + 2 + 2 + usize::from(n_items) * (2 + 2 + 2 + 4 + 4);
    let meta_body_len = 4
        + hdlr.len()
        + pitm.len()
        + iloc_len
        + iinf.len()
        + iref.as_ref().map_or(0, Vec::len)
        + iprp.len();
    let mdat_start = (ftyp.len() + 8 + meta_body_len + 8) as u32;
    let mut extents = vec![(mdat_start, p.av1_data.len() as u32)];
    if let Some(e) = p.exif {
        extents.push((mdat_start + p.av1_data.len() as u32, e.len() as u32));
    }
    let mut iloc = vec![0x44, 0x00];
    iloc.extend_from_slice(&n_items.to_be_bytes());
    for (i, (off, len)) in extents.iter().enumerate() {
        iloc.extend_from_slice(&(i as u16 + 1).to_be_bytes());
        iloc.extend_from_slice(&0u16.to_be_bytes());
        iloc.extend_from_slice(&1u16.to_be_bytes());
        iloc.extend_from_slice(&off.to_be_bytes());
        iloc.extend_from_slice(&len.to_be_bytes());
    }
    let iloc = boxed(b"iloc", &full(0, 0, &iloc));
    debug_assert_eq!(iloc.len(), iloc_len);

    let mut meta = vec![0, 0, 0, 0];
    for part in [&hdlr, &pitm, &iloc, &iinf] {
        meta.extend_from_slice(part);
    }
    if let Some(r) = &iref {
        meta.extend_from_slice(r);
    }
    meta.extend_from_slice(&iprp);
    debug_assert_eq!(meta.len(), meta_body_len);
    let mut out = ftyp;
    put_box(&mut out, b"meta", &meta);
    let mut mdat = p.av1_data.to_vec();
    if let Some(e) = p.exif {
        mdat.extend_from_slice(e);
    }
    debug_assert_eq!(out.len() + 8, mdat_start as usize);
    put_box(&mut out, b"mdat", &mdat);
    out
}

/// The `nclx` colour description.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Nclx {
    pub primaries: u16,
    pub transfer: u16,
    pub matrix: u16,
    pub full_range: bool,
}

/// What [`read_info`] finds about the primary picture of a file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AvifInfo {
    pub width: u32,
    pub height: u32,
    /// Channel depths of `pixi`.
    pub bits: Vec<u8>,
    pub monochrome: bool,
    pub subsampled_x: bool,
    pub subsampled_y: bool,
    pub nclx: Option<Nclx>,
    pub icc: Option<Vec<u8>>,
    /// The Exif item's TIFF blob (offset prefix removed).
    pub exif: Option<Vec<u8>>,
    /// A rotation or mirror property is associated (a turn the viewer would apply again).
    pub has_irot_or_imir: bool,
    /// Length of the AV1 data of the primary item.
    pub av1_len: usize,
    /// The AV1 data starts with a well-formed OBU header.
    pub av1_header_ok: bool,
}

fn be16(b: &[u8], o: usize) -> Option<usize> {
    Some(usize::from(u16::from_be_bytes(
        b.get(o..o + 2)?.try_into().ok()?,
    )))
}
fn be32(b: &[u8], o: usize) -> Option<usize> {
    Some(u32::from_be_bytes(b.get(o..o + 4)?.try_into().ok()?) as usize)
}

/// The boxes of `b` as (type, body) pairs; `None` on a size that leaves the buffer.
fn boxes(b: &[u8]) -> Option<Vec<([u8; 4], &[u8])>> {
    let mut out = Vec::new();
    let mut o = 0;
    while o < b.len() {
        let size = be32(b, o)?;
        let kind: [u8; 4] = b.get(o + 4..o + 8)?.try_into().ok()?;
        let end = if size == 0 {
            b.len()
        } else {
            o.checked_add(size)?
        };
        if size == 1 || end > b.len() || end < o + 8 {
            return None;
        }
        out.push((kind, &b[o + 8..end]));
        o = end;
    }
    Some(out)
}

fn find<'a>(list: &[([u8; 4], &'a [u8])], kind: &[u8; 4]) -> Option<&'a [u8]> {
    list.iter().find(|(k, _)| k == kind).map(|(_, b)| *b)
}

/// Reads the primary picture's description out of an AVIF/HEIF file this writer makes (or any
/// still AVIF with `construction_method` 0). `None` when the structure does not hold together:
/// a box leaves the file, an offset points outside it, a property index does not exist.
pub fn read_info(bytes: &[u8]) -> Option<AvifInfo> {
    let top = boxes(bytes)?;
    let ftyp = find(&top, b"ftyp")?;
    let brands_ok = ftyp.len() >= 8
        && (&ftyp[..4] == b"avif" || ftyp[8..].as_chunks::<4>().0.iter().any(|b| b == b"avif"));
    if !brands_ok {
        return None;
    }
    let meta = find(&top, b"meta")?;
    let meta = boxes(meta.get(4..)?)?;
    let primary = be16(find(&meta, b"pitm")?, 4)?;
    // iloc: item -> (offset, length)
    let iloc = find(&meta, b"iloc")?;
    let version = *iloc.first()?;
    if version > 1 {
        return None;
    }
    let (osz, lsz) = (usize::from(iloc[4] >> 4), usize::from(iloc[4] & 15));
    let (bsz, isz) = (
        usize::from(iloc[5] >> 4),
        if version == 1 {
            usize::from(iloc[5] & 15)
        } else {
            0
        },
    );
    let read = |o: usize, n: usize| -> Option<usize> {
        match n {
            0 => Some(0),
            4 => be32(iloc, o),
            8 => Some(u64::from_be_bytes(iloc.get(o..o + 8)?.try_into().ok()?) as usize),
            _ => None,
        }
    };
    let count = be16(iloc, 6)?;
    let mut o = 8;
    let mut items: Vec<(usize, usize, usize)> = Vec::new();
    for _ in 0..count {
        let id = be16(iloc, o)?;
        o += 2;
        if version == 1 {
            if be16(iloc, o)? & 15 != 0 {
                return None; // construction method other than file offset
            }
            o += 2;
        }
        o += 2; // data_reference_index
        let base = read(o, bsz)?;
        o += bsz;
        let ext = be16(iloc, o)?;
        o += 2;
        if ext != 1 {
            return None;
        }
        if isz > 0 {
            o += isz;
        }
        let off = read(o, osz)?;
        o += osz;
        let len = read(o, lsz)?;
        o += lsz;
        items.push((id, base.checked_add(off)?, len));
    }
    let data_of = |id: usize| -> Option<&[u8]> {
        let (_, off, len) = items.iter().find(|(i, _, _)| *i == id)?;
        bytes.get(*off..off.checked_add(*len)?)
    };
    let av1 = data_of(primary)?;
    // iprp
    let iprp = boxes(find(&meta, b"iprp")?)?;
    let ipco = boxes(find(&iprp, b"ipco")?)?;
    let ipma = find(&iprp, b"ipma")?;
    let (ver, flags) = (ipma[0], u32::from_be_bytes([0, ipma[1], ipma[2], ipma[3]]));
    let entries = be32(ipma, 4)?;
    let mut o = 8;
    let mut props: Vec<usize> = Vec::new();
    for _ in 0..entries {
        let id = if ver == 0 {
            let v = be16(ipma, o)?;
            o += 2;
            v
        } else {
            let v = be32(ipma, o)?;
            o += 4;
            v
        };
        let n = usize::from(*ipma.get(o)?);
        o += 1;
        for _ in 0..n {
            let idx = if flags & 1 == 1 {
                let v = be16(ipma, o)? & 0x7fff;
                o += 2;
                v
            } else {
                let v = usize::from(*ipma.get(o)? & 0x7f);
                o += 1;
                v
            };
            if id == primary {
                props.push(idx);
            }
        }
    }
    let mut info = AvifInfo {
        av1_len: av1.len(),
        av1_header_ok: av1.first().is_some_and(|b| b & 0x80 == 0),
        ..AvifInfo::default()
    };
    for idx in props {
        let (kind, body) = ipco.get(idx.checked_sub(1)?)?;
        match kind {
            b"ispe" => {
                info.width = be32(body, 4)? as u32;
                info.height = be32(body, 8)? as u32;
            }
            b"pixi" => {
                let n = usize::from(*body.get(4)?);
                info.bits = body.get(5..5 + n)?.to_vec();
            }
            b"av1C" => {
                let (b2, b3) = (*body.get(2)?, *body.get(3)?);
                let _ = b3;
                info.monochrome = b2 & 0x10 != 0;
                info.subsampled_x = b2 & 0x08 != 0;
                info.subsampled_y = b2 & 0x04 != 0;
            }
            b"colr" => match body.get(..4)? {
                b"prof" | b"rICC" => info.icc = Some(body[4..].to_vec()),
                b"nclx" => {
                    info.nclx = Some(Nclx {
                        primaries: be16(body, 4)? as u16,
                        transfer: be16(body, 6)? as u16,
                        matrix: be16(body, 8)? as u16,
                        full_range: body.get(10)? & 0x80 != 0,
                    });
                }
                _ => {}
            },
            b"irot" | b"imir" => info.has_irot_or_imir = true,
            _ => {}
        }
    }
    // The Exif item: the infe whose type is `Exif`.
    if let Some(iinf) = find(&meta, b"iinf") {
        let n = be16(iinf, 4)?;
        let inner = boxes(iinf.get(6..)?)?;
        for (k, b) in inner.iter().take(n) {
            if k == b"infe" && b.first() == Some(&2) && b.get(8..12) == Some(b"Exif") {
                let id = be16(b, 4)?;
                let d = data_of(id)?;
                let skip = be32(d, 0)?;
                let blob = d.get(4usize.checked_add(skip)?..)?;
                if blob.starts_with(b"II*\0") || blob.starts_with(b"MM\0*") {
                    info.exif = Some(blob.to_vec());
                }
            }
        }
    }
    Some(info)
}
