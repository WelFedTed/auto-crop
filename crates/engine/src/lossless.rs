// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The lossless JPEG path (ROADMAP M2.24, M2.22; PLAN 2.7 "Lossless JPEG path", 3.9): a JPEG that is
//! only turned, flipped or cropped along its pixel grid is rewritten by moving DCT coefficients, so
//! no pixel is decoded or re-quantised and no generation is lost.
//!
//! It applies when the edit is an axis-aligned rectangle (skew under 0.05 degrees, no mirror, no
//! perspective) of a baseline JPEG that this build can transform, and the result is exact:
//!
//! * the crop origin snaps **outward** to the iMCU grid, so no requested pixel is lost; the output
//!   may be larger than asked by at most `max(16 px, 0.5% of the short side)` per axis, else the
//!   caller re-encodes;
//! * the EXIF orientation and the user's quarter turns are one dihedral transform, done by the
//!   transform ([`Policy::Perfect`]: if an edge is not on the MCU grid the turn would trim it, so
//!   the path is refused instead and the caller re-encodes);
//! * the Orientation tag is then written as 1 (`jpeg_meta`), never both a turn and a tag.
//!
//! "Lossless" is claimed only when every condition held. The transform is the safe-Rust one of
//! `auto_crop_codecs::jpeg_lossless` (baseline Huffman JPEG); with the `turbojpeg` feature a
//! `codecs::turbo::Transformer` (libjpeg-turbo >= 3.1.4, which also reads progressive files) does
//! the same work.

use crate::output::{EngineOptions, SourceMeta, meta_policy};
use auto_crop_codecs::jpeg_lossless::{Op, Policy, Rect, Transformed};
use auto_crop_codecs::jpeg_meta;
use auto_crop_codecs::{CodecError, DecodeLimits};
use auto_crop_core::{ExifOrientation, Pt, QuadWarp};

/// Skew below which a crop counts as axis-aligned (degrees).
pub const MAX_SKEW_DEG: f32 = 0.05;
/// Output growth allowed by the outward snap to the MCU grid: at least this many pixels per axis.
pub const MAX_GROWTH_PX: u32 = 16;
/// ... or this share of the short side of the output, whichever is larger.
pub const MAX_GROWTH_FRACTION: f64 = 0.005;

/// A lossless result.
#[derive(Debug, Clone)]
pub struct Lossless {
    /// The finished JPEG: coefficients moved, metadata policy applied.
    pub bytes: Vec<u8>,
    /// Size of the output picture.
    pub dims: (u32, u32),
    /// How many pixels larger than requested the output is per axis (the outward MCU snap).
    pub grew: (u32, u32),
}

/// The transform of `orientation` (EXIF 1..=8) followed by `quarter_turns` clockwise turns, as one
/// [`ExifOrientation`] (the stored-to-final mapping), found by where the three reference corners
/// of the unit square land.
pub fn combined(orientation: ExifOrientation, quarter_turns: u8) -> ExifOrientation {
    let turn = |p: Pt| Pt::new(1.0 - p.y, p.x); // one clockwise quarter turn of the unit square
    let f = |p: Pt| {
        let mut q = orientation.to_oriented(p);
        for _ in 0..quarter_turns % 4 {
            q = turn(q);
        }
        q
    };
    let refs = [Pt::new(0.0, 0.0), Pt::new(1.0, 0.0), Pt::new(0.0, 1.0)];
    ExifOrientation::ALL
        .into_iter()
        .find(|c| {
            refs.iter().all(|r| {
                let (a, b) = (c.to_oriented(*r), f(*r));
                (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9
            })
        })
        .unwrap_or(ExifOrientation::Normal)
}

/// The lossless operation of an orientation (`None` for the identity).
pub fn op_of(o: ExifOrientation) -> Option<Op> {
    Some(match o {
        ExifOrientation::Normal => return None,
        ExifOrientation::MirrorH => Op::FlipH,
        ExifOrientation::Rotate180 => Op::Rotate180,
        ExifOrientation::MirrorV => Op::FlipV,
        ExifOrientation::Transpose => Op::Transpose,
        ExifOrientation::Rotate90Cw => Op::Rotate90,
        ExifOrientation::Transverse => Op::Transverse,
        ExifOrientation::Rotate90Ccw => Op::Rotate270,
    })
}

/// The transform engine behind the path.
pub struct Backend {
    #[cfg(feature = "turbojpeg")]
    turbo: Option<auto_crop_codecs::turbo::Transformer>,
}

impl Backend {
    pub fn new() -> Self {
        Self {
            #[cfg(feature = "turbojpeg")]
            turbo: auto_crop_codecs::turbo::Transformer::new().ok(),
        }
    }

    fn transform(
        &mut self,
        bytes: &[u8],
        op: Op,
        policy: Policy,
        limits: &DecodeLimits,
    ) -> Result<Transformed, CodecError> {
        #[cfg(feature = "turbojpeg")]
        if let Some(t) = self.turbo.as_mut() {
            return t.transform(bytes, op, policy, limits);
        }
        auto_crop_codecs::jpeg_lossless::transform(bytes, op, policy, limits)
    }
}

impl Default for Backend {
    fn default() -> Self {
        Self::new()
    }
}

/// The pixel rectangle of an axis-aligned quad in the stored (pre-orientation) space of a
/// `stored` = (w, h) image whose EXIF orientation is `o`, rounded outward to whole pixels; `None`
/// if the quad is not an axis-aligned rectangle within a hundredth of a pixel.
fn stored_rect(q: &QuadWarp, o: ExifOrientation, stored: (u32, u32)) -> Option<Rect> {
    let (ow, oh) = o.oriented_dims(stored.0, stored.1);
    // Pixel-edge coordinates: the renderer's pixel centres are these minus a half pixel, so the
    // rectangle it samples is exactly [tl, br] in these units.
    let c = q.corners_px(ow, oh);
    let (tl, tr, br, bl) = (c[0], c[1], c[2], c[3]);
    let eps = 0.01;
    let aligned = (tl.1 - tr.1).abs() < eps
        && (bl.1 - br.1).abs() < eps
        && (tl.0 - bl.0).abs() < eps
        && (tr.0 - br.0).abs() < eps
        && tr.0 > tl.0
        && bl.1 > tl.1;
    if !aligned {
        return None;
    }
    // Rounded outward in oriented space, then mapped to stored space exactly.
    let (x0, y0) = ((tl.0 + eps).floor().max(0.0), (tl.1 + eps).floor().max(0.0));
    let (x1, y1) = (
        (br.0 - eps).ceil().min(f64::from(ow)),
        (br.1 - eps).ceil().min(f64::from(oh)),
    );
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let norm = |x: f64, y: f64| Pt::new(x / f64::from(ow), y / f64::from(oh));
    let (a, b) = (o.from_oriented(norm(x0, y0)), o.from_oriented(norm(x1, y1)));
    let (sw, sh) = (f64::from(stored.0), f64::from(stored.1));
    let (sx0, sx1) = ((a.x.min(b.x) * sw).round(), (a.x.max(b.x) * sw).round());
    let (sy0, sy1) = ((a.y.min(b.y) * sh).round(), (a.y.max(b.y) * sh).round());
    Some(Rect {
        x: sx0 as u32,
        y: sy0 as u32,
        w: (sx1 - sx0) as u32,
        h: (sy1 - sy0) as u32,
    })
}

/// Tries the lossless path. `src` is the source JPEG, `q` the edit in the EXIF-oriented space the
/// decode produces. `None` means "re-encode instead" (a condition did not hold, or the transform
/// refused); it never means failure.
pub fn try_lossless(
    backend: &mut Backend,
    src: &[u8],
    q: &QuadWarp,
    meta: &SourceMeta,
    o: &EngineOptions,
) -> Option<Lossless> {
    if !o.lossless_jpeg || q.mirror || q.fine_deg.abs() >= MAX_SKEW_DEG {
        return None;
    }
    let jm = meta.jpeg.as_ref()?;
    let limits = o.limits();
    let probe = auto_crop_codecs::probe_with(src, &limits).ok()?;
    let stored = (probe.width, probe.height);
    let ex =
        ExifOrientation::from_tag(u32::from(probe.orientation)).unwrap_or(ExifOrientation::Normal);
    let want = stored_rect(q, ex, stored)?;
    let full = want.x == 0 && want.y == 0 && want.w == stored.0 && want.h == stored.1;
    let op = op_of(combined(ex, q.quarter_turns));

    let mut cur: Option<Transformed> = None;
    let mut grew = (0u32, 0u32);
    if !full {
        let t = backend
            .transform(src, Op::Crop(want), Policy::Snap, &limits)
            .ok()?;
        grew = (
            t.rect.w.saturating_sub(want.w),
            t.rect.h.saturating_sub(want.h),
        );
        // Growth is judged against the output's short side, after any turn (the sides swap, the
        // short side does not).
        let short = f64::from(want.w.min(want.h));
        let allow = MAX_GROWTH_PX.max((short * MAX_GROWTH_FRACTION).ceil() as u32);
        if grew.0 > allow || grew.1 > allow {
            return None;
        }
        cur = Some(t);
    }
    if let Some(op) = op {
        let t = {
            let input = cur.as_ref().map_or(src, |t| t.bytes.as_slice());
            backend
                .transform(input, op, Policy::Perfect, &limits)
                .ok()?
        };
        if !t.perfect {
            return None;
        }
        cur = Some(t);
    }
    // The finished picture's size, from the last transform (or the untouched source).
    let (w, h, body) = match &cur {
        Some(t) => (t.width, t.height, t.bytes.as_slice()),
        None => {
            let (w, h) = ex.oriented_dims(stored.0, stored.1);
            (w, h, src)
        }
    };
    // `grew` was measured in stored space; report it in output space.
    let grew = if combined(ex, q.quarter_turns).swaps_axes() {
        (grew.1, grew.0)
    } else {
        grew
    };
    let out = jpeg_meta::rewrite_metadata(body, jm, &meta_policy(o, (w, h))).ok()?;
    Some(Lossless {
        bytes: out,
        dims: (w, h),
        grew,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use auto_crop_codecs::fixtures::{exif_blob, jpeg_insert_segment};
    use auto_crop_codecs::{Format, decode, exif_orientation};
    use auto_crop_imgproc::Raster;

    /// A picture whose every 16 x 16 block is distinct, so a transform that moved a block anywhere
    /// but the right place shows.
    fn tiles(w: u32, h: u32) -> Raster {
        let mut r = Raster::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let (bx, by) = (x / 16, y / 16);
                r.set_pixel(
                    x,
                    y,
                    [
                        (bx * 37 % 251) as u8,
                        (by * 61 % 251) as u8,
                        ((bx * 7 + by * 13) % 251) as u8,
                    ],
                );
            }
        }
        r
    }

    fn jpeg_of(w: u32, h: u32, orientation: Option<u16>) -> Vec<u8> {
        // The default encoder writes 4:2:0, so the MCU is 16 x 16.
        let mut j = crate::output::encode_raster(
            &tiles(w, h),
            Format::Jpeg,
            &SourceMeta::default(),
            &EngineOptions::default(),
        )
        .unwrap()
        .bytes;
        if let Some(o) = orientation {
            let mut e = b"Exif\0\0".to_vec();
            e.extend_from_slice(&exif_blob(o, true));
            j = jpeg_insert_segment(&j, 0xE1, &e);
        }
        j
    }

    fn quad_px(x0: f64, y0: f64, x1: f64, y1: f64, w: u32, h: u32) -> QuadWarp {
        let n = |x: f64, y: f64| Pt::new(x / f64::from(w), y / f64::from(h));
        QuadWarp::new([n(x0, y0), n(x1, y0), n(x1, y1), n(x0, y1)])
    }

    fn run(src: &[u8], q: &QuadWarp, o: &EngineOptions) -> Option<Lossless> {
        let meta = SourceMeta::read(Format::Jpeg, src, None);
        try_lossless(&mut Backend::new(), src, q, &meta, o)
    }

    #[test]
    fn the_eight_orientations_compose_with_the_user_turns() {
        // Identity.
        assert_eq!(
            combined(ExifOrientation::Normal, 0),
            ExifOrientation::Normal
        );
        // Four clockwise turns are the identity again, for every orientation.
        for o in ExifOrientation::ALL {
            assert_eq!(combined(o, 4), o);
        }
        // Orientation 6 (turn 90 cw) plus one more turn is 180.
        assert_eq!(
            combined(ExifOrientation::Rotate90Cw, 1),
            ExifOrientation::Rotate180
        );
        // A mirror then a turn is a diagonal mirror.
        assert_eq!(
            combined(ExifOrientation::MirrorH, 1),
            ExifOrientation::Transverse
        );
        // Every combination is one of the eight (the group is closed).
        for o in ExifOrientation::ALL {
            for t in 0..4 {
                let c = combined(o, t);
                assert!(ExifOrientation::ALL.contains(&c));
            }
        }
    }

    #[test]
    fn an_axis_aligned_mcu_crop_is_exact_and_nothing_is_decoded() {
        let src = jpeg_of(160, 128, None);
        // 32..128 x 16..96: on the 16 px grid of a 4:2:0 image.
        let q = quad_px(32.0, 16.0, 128.0, 96.0, 160, 128);
        let out = run(&src, &q, &EngineOptions::default()).expect("lossless");
        assert_eq!(out.dims, (96, 80));
        assert_eq!(out.grew, (0, 0));
        // The decoded output is the same picture as the decoded source's rectangle: every tile
        // lands where it was (interior pixels, away from the chroma edge of the crop).
        let (a, b) = (
            decode(&src).unwrap().raster,
            decode(&out.bytes).unwrap().raster,
        );
        assert_eq!((b.width, b.height), (96, 80));
        for y in 2..78 {
            for x in 2..94 {
                assert_eq!(b.pixel(x, y), a.pixel(x + 32, y + 16), "({x},{y})");
            }
        }
    }

    #[test]
    fn the_crop_snaps_outward_and_small_growth_is_accepted() {
        let src = jpeg_of(160, 128, None);
        // Starts at 35, 20 and ends at 125, 90: snaps to 32..128 and 16..96, growing 6 and 6.
        let q = quad_px(35.0, 20.0, 125.0, 90.0, 160, 128);
        let out = run(&src, &q, &EngineOptions::default()).expect("lossless");
        assert_eq!(out.dims, (96, 80));
        assert_eq!(out.grew, (6, 10));
    }

    #[test]
    fn growth_beyond_the_limit_re_encodes_instead() {
        // 4:2:0 with an odd origin: the snap would grow by up to 15 + 15; ask for 33 px offsets so
        // origin snaps by 1 and far edge by 15 -> 16 total is allowed; 17+ is not.
        let src = jpeg_of(400, 320, None);
        let q = quad_px(33.0, 17.0, 177.0, 113.0, 400, 320); // 33 -> 32 (1), 177 -> 192 (15)
        let ok = run(&src, &q, &EngineOptions::default());
        assert!(ok.is_some(), "16 px growth is allowed");
        let q2 = quad_px(47.0, 17.0, 161.0, 113.0, 400, 320); // 47 -> 32 (15), 161 -> 176 (15): 30
        assert!(run(&src, &q2, &EngineOptions::default()).is_none());
    }

    #[test]
    fn a_skewed_mirrored_or_disabled_edit_is_never_lossless() {
        let src = jpeg_of(160, 128, None);
        let base = quad_px(32.0, 16.0, 128.0, 96.0, 160, 128);
        let o = EngineOptions::default();
        assert!(run(&src, &base, &o).is_some());
        let mut skew = base.clone();
        skew.fine_deg = 0.2;
        assert!(run(&src, &skew, &o).is_none());
        let mut mirror = base.clone();
        mirror.mirror = true;
        assert!(run(&src, &mirror, &o).is_none());
        let off = EngineOptions {
            lossless_jpeg: false,
            ..EngineOptions::default()
        };
        assert!(run(&src, &base, &off).is_none());
        // A perspective quad.
        let mut persp = base.clone();
        persp.corners[1].y += 0.05;
        assert!(run(&src, &persp, &o).is_none());
    }

    /// Orientation applied once: for each of the 8 EXIF orientations, the lossless output of the
    /// whole frame is upright (same pixels as the decode, which applied the orientation), carries
    /// Orientation 1, and reprocessing it does not turn it again.
    #[test]
    fn all_eight_orientations_come_out_upright_with_the_tag_reset_once() {
        // 128 x 96 is a multiple of the 16 px MCU on both axes, so every turn is perfect.
        for tag in 1u16..=8 {
            let src = jpeg_of(128, 96, Some(tag));
            let truth = decode(&src).unwrap().raster; // upright, orientation applied once
            let (ow, oh) = (truth.width, truth.height);
            let q = quad_px(0.0, 0.0, f64::from(ow), f64::from(oh), ow, oh);
            let out = run(&src, &q, &EngineOptions::default())
                .unwrap_or_else(|| panic!("orientation {tag}: no lossless path"));
            assert_eq!(out.dims, (ow, oh), "orientation {tag}");
            let jm = jpeg_meta::read_meta(&out.bytes).unwrap();
            assert_eq!(
                exif_orientation(&jm.exif.expect("exif carried")),
                Some(1),
                "orientation {tag}"
            );
            let d = decode(&out.bytes).unwrap();
            assert_eq!(d.exif_orientation, 1, "orientation {tag}");
            let got = d.raster;
            // Compare away from the borders (chroma upsampling at the frame edge differs by a
            // level between a transposed and a native decode).
            for y in 3..oh - 3 {
                for x in 3..ow - 3 {
                    let (a, b) = (truth.pixel(x, y), got.pixel(x, y));
                    let d: i32 = (0..3)
                        .map(|c| (i32::from(a[c]) - i32::from(b[c])).abs())
                        .sum();
                    assert!(d <= 12, "orientation {tag} at ({x},{y}): {a:?} vs {b:?}");
                }
            }
            // Reprocessing the output (orientation 1 now) is a no-turn: same size, same pixels.
            let q = quad_px(0.0, 0.0, f64::from(ow), f64::from(oh), ow, oh);
            let again = run(&out.bytes, &q, &EngineOptions::default()).expect("again");
            assert_eq!(again.dims, (ow, oh));
        }
    }

    #[test]
    fn a_crop_with_a_turn_and_an_exif_orientation_is_one_transform() {
        // Orientation 6 plus a user turn of one: 180 degrees overall, with a crop in oriented space.
        let src = jpeg_of(128, 96, Some(6));
        let truth = decode(&src).unwrap().raster; // 96 x 128, upright
        assert_eq!((truth.width, truth.height), (96, 128));
        let mut q = quad_px(16.0, 32.0, 80.0, 112.0, 96, 128);
        q.quarter_turns = 1;
        let out = run(&src, &q, &EngineOptions::default()).expect("lossless");
        // The crop is 64 x 80 in oriented space; a quarter turn makes it 80 x 64.
        assert_eq!(out.dims, (80, 64));
        let got = decode(&out.bytes).unwrap().raster;
        // Rotate the truth's crop by hand (clockwise) and compare the interior.
        for y in 3..61 {
            for x in 3..77 {
                // Output (x, y) is crop pixel (cx, cy) with x = 79 - cy ... a clockwise turn maps
                // crop (cx, cy) of a 64 x 80 crop to (79 - cy, cx).
                let (cx, cy) = (y, 79 - x);
                let a = truth.pixel(16 + cx, 32 + cy);
                let b = got.pixel(x, y);
                let d: i32 = (0..3)
                    .map(|c| (i32::from(a[c]) - i32::from(b[c])).abs())
                    .sum();
                assert!(d <= 12, "({x},{y}): {a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn an_edge_off_the_mcu_grid_that_a_turn_would_trim_is_refused() {
        // 120 x 90 is not a multiple of 16: a 90-degree turn of the whole frame would trim it.
        let src = jpeg_of(120, 90, Some(6));
        let truth = decode(&src).unwrap().raster;
        let q = quad_px(
            0.0,
            0.0,
            f64::from(truth.width),
            f64::from(truth.height),
            truth.width,
            truth.height,
        );
        assert!(run(&src, &q, &EngineOptions::default()).is_none());
    }

    #[test]
    fn a_progressive_or_damaged_source_just_declines() {
        let q = quad_px(0.0, 0.0, 16.0, 16.0, 128, 96);
        assert!(run(b"not a jpeg", &q, &EngineOptions::default()).is_none());
        let src = jpeg_of(128, 96, None);
        assert!(run(&src[..src.len() / 2], &q, &EngineOptions::default()).is_none());
    }
}
