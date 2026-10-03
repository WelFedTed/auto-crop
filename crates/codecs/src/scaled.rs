// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Scaled decode (ROADMAP M1.19, PLAN 3.1): `Want::Scaled { min_edge }` picks the smallest
//! decoder-native reduction (JPEG DCT scaling by 1/8, 1/4, 1/2 or 1) that keeps the long edge at
//! least `min_edge`, never upscales, and applies the EXIF orientation after the scaling.
//!
//! With the `turbojpeg` feature the reduction happens inside libjpeg-turbo's inverse DCT, so a
//! 1/8 decode touches a sixty-fourth of the pixels. Without the feature (or for a CMYK JPEG, which
//! TurboJPEG cannot convert to RGB) the file is decoded in full and block-averaged to the same size
//! and on the same pixel grid, so callers get the same dimensions and geometry either way; only the
//! speed (and a few LSB of filter shape) differ. Other formats have no
//! native reduction and come back at full size with `denom == 1`.

use crate::decode::{decode_raw, orient, precheck};
use crate::{CodecError, DecodeLimits, Decoded, Format, guard_item};
use auto_crop_core::ports::Want;
use auto_crop_imgproc::Raster;

/// The reductions a JPEG decoder offers, smallest output first.
pub const DENOMS: [u32; 4] = [8, 4, 2, 1];

/// The denominator `want` selects for a `width` x `height` image: the largest of 8, 4, 2, 1 whose
/// output still has a long edge of at least `min_edge` (dimensions round up, as libjpeg's do);
/// 1 when even the full size is smaller than `min_edge` (no upscaling) or for [`Want::Full`].
pub fn pick_denom(width: u32, height: u32, want: Want) -> u32 {
    let Want::Scaled { min_edge } = want else {
        return 1;
    };
    let long = width.max(height);
    DENOMS
        .into_iter()
        .find(|d| long.div_ceil(*d) >= min_edge)
        .unwrap_or(1)
}

/// A scaled decode: the decoded image plus how it was reduced.
#[derive(Debug, Clone)]
pub struct ScaledDecoded {
    /// Pixels at `1/denom` size with the EXIF orientation applied; `exif_orientation`, `icc` and
    /// the notices are those of [`Decoded`].
    pub decoded: Decoded,
    /// The reduction applied: 1, 2, 4 or 8.
    pub denom: u32,
    /// Stored size of the file, before reduction and before the EXIF turn.
    pub source_width: u32,
    pub source_height: u32,
}

/// Decodes `bytes` as [`Want`] asks, under `limits`. Never panics: a panic inside a decoder becomes
/// [`CodecError::InternalPanic`].
pub fn decode_scaled(
    bytes: &[u8],
    want: Want,
    limits: &DecodeLimits,
) -> Result<ScaledDecoded, CodecError> {
    guard_item(|| decode_scaled_inner(bytes, want, limits))
}

fn decode_scaled_inner(
    bytes: &[u8],
    want: Want,
    limits: &DecodeLimits,
) -> Result<ScaledDecoded, CodecError> {
    let (format, h) = precheck(bytes, limits)?;
    let denom = if format == Format::Jpeg {
        pick_denom(h.width, h.height, want)
    } else {
        1
    };
    let source = (h.width, h.height);

    #[cfg(feature = "turbojpeg")]
    if format == Format::Jpeg && h.channels != 4 {
        return turbo_scaled(bytes, &h, denom, limits);
    }

    // Full decode of the stored pixels (zune-jpeg, `image`), the same reduction by a block average
    // (the geometry of DCT scaling: output pixel (x, y) covers the `denom` x `denom` block at
    // (x * denom, y * denom)), and only then the EXIF turn.
    let mut decoded = decode_raw(bytes, limits, false)?;
    if denom > 1 {
        decoded.raster = block_mean(&decoded.raster, denom);
    }
    // HEIC and AVIF come back upright from libheif (`irot` and `imir` are already applied), so
    // turning again by the orientation they report would rotate twice.
    if !matches!(format, Format::Heic | Format::Avif) {
        decoded.raster = orient(decoded.raster, decoded.exif_orientation)?;
    }
    Ok(ScaledDecoded {
        decoded,
        denom,
        source_width: source.0,
        source_height: source.1,
    })
}

/// Reduces `src` by `denom` with a block average: the output is `ceil(w / denom) x ceil(h / denom)`
/// and each pixel is the rounded mean of the in-bounds pixels of its block, which is what the
/// libjpeg DC-only (1/8) decode produces and what the other reductions approximate.
pub(crate) fn block_mean(src: &Raster, denom: u32) -> Raster {
    let d = denom as usize;
    let (sw, sh) = (src.width as usize, src.height as usize);
    let (ow, oh) = (sw.div_ceil(d), sh.div_ceil(d));
    let mut out = Raster::new(ow as u32, oh as u32);
    let mut acc = vec![0u32; ow * 3];
    for oy in 0..oh {
        acc.fill(0);
        let rows = (oy * d)..((oy + 1) * d).min(sh);
        for y in rows.clone() {
            let row = &src.data[y * sw * 3..(y + 1) * sw * 3];
            for (x, px) in row.as_chunks::<3>().0.iter().enumerate() {
                let o = (x / d) * 3;
                acc[o] += u32::from(px[0]);
                acc[o + 1] += u32::from(px[1]);
                acc[o + 2] += u32::from(px[2]);
            }
        }
        for ox in 0..ow {
            let cols = d.min(sw - ox * d) as u32;
            let n = cols * rows.len() as u32;
            for c in 0..3 {
                out.data[(oy * ow + ox) * 3 + c] = ((acc[ox * 3 + c] + n / 2) / n) as u8;
            }
        }
    }
    out
}

/// libjpeg-turbo: scale inside the inverse DCT, then turn by the EXIF orientation.
#[cfg(feature = "turbojpeg")]
fn turbo_scaled(
    bytes: &[u8],
    h: &crate::parse::Header,
    denom: u32,
    limits: &DecodeLimits,
) -> Result<ScaledDecoded, CodecError> {
    let raster = crate::turbo::decode_rgb(bytes, denom, h.width, h.height, limits)?;
    let raster = crate::decode::orient(raster, h.orientation)?;
    let mut notices = Vec::new();
    let icc = crate::decode::extract_icc(bytes, h, None, &mut notices);
    Ok(ScaledDecoded {
        decoded: Decoded {
            raster,
            format: Format::Jpeg,
            exif_orientation: h.orientation,
            icc,
            source_bit_depth: 8,
            frames: h.frames,
            notices,
        },
        denom,
        source_width: h.width,
        source_height: h.height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scaled(min_edge: u32) -> Want {
        Want::Scaled { min_edge }
    }

    #[test]
    fn the_smallest_reduction_that_keeps_the_long_edge_is_picked() {
        // 4000 x 3000: the long edge at 1/8, 1/4, 1/2 and 1 is 500, 1000, 2000 and 4000.
        let p = |m| pick_denom(4000, 3000, scaled(m));
        assert_eq!(p(0), 8);
        assert_eq!(p(1), 8);
        assert_eq!(p(500), 8);
        assert_eq!(p(501), 4);
        assert_eq!(p(1000), 4);
        assert_eq!(p(1001), 2);
        assert_eq!(p(2000), 2);
        assert_eq!(p(2001), 1);
        assert_eq!(p(4000), 1);
        // Never upscales: a request above the image gets the full size.
        assert_eq!(p(5000), 1);
        assert_eq!(p(u32::MAX), 1);
        assert_eq!(pick_denom(4000, 3000, Want::Full), 1);
    }

    #[test]
    fn the_long_edge_is_the_larger_side_and_odd_sizes_round_up() {
        // Portrait: 3001 x 4001 behaves like 4001 x 3001; ceil(4001 / 8) = 501.
        assert_eq!(pick_denom(3001, 4001, scaled(501)), 8);
        assert_eq!(pick_denom(3001, 4001, scaled(502)), 4);
        assert_eq!(pick_denom(4001, 3001, scaled(1001)), 4); // ceil(4001 / 4) = 1001
        assert_eq!(pick_denom(4001, 3001, scaled(1002)), 2);
        // Tiny images stay usable: 1 x 1 at 1/8 is still 1 x 1.
        assert_eq!(pick_denom(1, 1, scaled(1)), 8);
        assert_eq!(pick_denom(1, 1, scaled(2)), 1);
    }

    #[test]
    fn the_chosen_edge_never_falls_below_min_edge_unless_the_image_is_smaller() {
        for (w, h) in [
            (4001u32, 3001u32),
            (3000, 4000),
            (17, 9),
            (1, 700),
            (8000, 8),
        ] {
            for min_edge in [0, 1, 7, 64, 256, 1000, 1024, 3072, 5000, 9000] {
                let d = pick_denom(w, h, scaled(min_edge));
                assert!([1, 2, 4, 8].contains(&d));
                let edge = w.max(h).div_ceil(d);
                assert!(
                    edge >= min_edge || d == 1,
                    "{w}x{h} min {min_edge}: 1/{d} gives {edge}"
                );
                // and it is the smallest such reduction
                let pos = DENOMS.iter().position(|x| *x == d).unwrap();
                if pos > 0 {
                    // DENOMS[pos - 1] is the next smaller output; it must have been too small.
                    assert!(w.max(h).div_ceil(DENOMS[pos - 1]) < min_edge);
                }
            }
        }
    }
}
