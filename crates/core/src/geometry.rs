// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Geometry of one item (PLAN 2.3): points in EXIF-oriented source space, the perspective quad
//! and the dense-dewarp grid.
//!
//! All coordinates are normalised: `x` and `y` are fractions of the EXIF-oriented width and height.
//! EXIF orientation is applied once at decode (PLAN 2.1), so the engine never has to think about
//! the stored pixel layout again; [`ExifOrientation`] converts between the two spaces for the
//! decoders and for tests.

use crate::curve::CurveWarp;
use crate::error::ErrKind;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// A point in EXIF-oriented source space, normalised: `x` and `y` are 0..1 of width and height
/// (values outside that range are allowed: a crop may extend past the frame).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Pt {
    pub x: f64,
    pub y: f64,
}

impl Pt {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }

    /// Pixel position in an image of `w` x `h` pixels.
    pub fn to_px(self, w: u32, h: u32) -> (f64, f64) {
        (self.x * f64::from(w), self.y * f64::from(h))
    }

    /// Normalised point for a pixel position in an image of `w` x `h` pixels.
    pub fn from_px(x: f64, y: f64, w: u32, h: u32) -> Self {
        Self {
            x: x / f64::from(w),
            y: y / f64::from(h),
        }
    }
}

/// One side of a quadrilateral, in the order top, right, bottom, left (corner `i` to `i + 1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Top,
    Right,
    Bottom,
    Left,
}

impl Side {
    pub const ALL: [Side; 4] = [Side::Top, Side::Right, Side::Bottom, Side::Left];
}

/// The eight EXIF orientations (tag 0x0112). The tag value is kept on `SourceRef`; pixels are
/// turned once at decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub enum ExifOrientation {
    /// 1: as stored.
    #[default]
    Normal,
    /// 2: mirrored left to right.
    MirrorH,
    /// 3: rotated 180 degrees.
    Rotate180,
    /// 4: mirrored top to bottom.
    MirrorV,
    /// 5: transposed (mirrored along the main diagonal).
    Transpose,
    /// 6: rotated 90 degrees clockwise to display.
    Rotate90Cw,
    /// 7: transversed (mirrored along the anti-diagonal).
    Transverse,
    /// 8: rotated 90 degrees counter-clockwise to display.
    Rotate90Ccw,
}

impl ExifOrientation {
    pub const ALL: [ExifOrientation; 8] = [
        ExifOrientation::Normal,
        ExifOrientation::MirrorH,
        ExifOrientation::Rotate180,
        ExifOrientation::MirrorV,
        ExifOrientation::Transpose,
        ExifOrientation::Rotate90Cw,
        ExifOrientation::Transverse,
        ExifOrientation::Rotate90Ccw,
    ];

    /// The EXIF tag value 1..=8; `None` for anything else.
    pub const fn from_tag(tag: u32) -> Option<Self> {
        Some(match tag {
            1 => Self::Normal,
            2 => Self::MirrorH,
            3 => Self::Rotate180,
            4 => Self::MirrorV,
            5 => Self::Transpose,
            6 => Self::Rotate90Cw,
            7 => Self::Transverse,
            8 => Self::Rotate90Ccw,
            _ => return None,
        })
    }

    pub const fn tag(self) -> u8 {
        match self {
            Self::Normal => 1,
            Self::MirrorH => 2,
            Self::Rotate180 => 3,
            Self::MirrorV => 4,
            Self::Transpose => 5,
            Self::Rotate90Cw => 6,
            Self::Transverse => 7,
            Self::Rotate90Ccw => 8,
        }
    }

    /// True for the four orientations that swap width and height (5 to 8).
    pub const fn swaps_axes(self) -> bool {
        matches!(
            self,
            Self::Transpose | Self::Rotate90Cw | Self::Transverse | Self::Rotate90Ccw
        )
    }

    /// Dimensions of the EXIF-oriented image for stored dimensions `w` x `h`.
    pub const fn oriented_dims(self, w: u32, h: u32) -> (u32, u32) {
        if self.swaps_axes() { (h, w) } else { (w, h) }
    }

    /// Maps a normalised point of the stored (raw) image to EXIF-oriented space.
    pub fn to_oriented(self, p: Pt) -> Pt {
        let (u, v) = (p.x, p.y);
        let (x, y) = match self {
            Self::Normal => (u, v),
            Self::MirrorH => (1.0 - u, v),
            Self::Rotate180 => (1.0 - u, 1.0 - v),
            Self::MirrorV => (u, 1.0 - v),
            Self::Transpose => (v, u),
            Self::Rotate90Cw => (1.0 - v, u),
            Self::Transverse => (1.0 - v, 1.0 - u),
            Self::Rotate90Ccw => (v, 1.0 - u),
        };
        Pt::new(x, y)
    }

    /// Inverse of [`ExifOrientation::to_oriented`].
    pub fn from_oriented(self, p: Pt) -> Pt {
        let (x, y) = (p.x, p.y);
        let (u, v) = match self {
            Self::Normal => (x, y),
            Self::MirrorH => (1.0 - x, y),
            Self::Rotate180 => (1.0 - x, 1.0 - y),
            Self::MirrorV => (x, 1.0 - y),
            Self::Transpose => (y, x),
            Self::Rotate90Cw => (y, 1.0 - x),
            Self::Transverse => (1.0 - y, 1.0 - x),
            Self::Rotate90Ccw => (1.0 - y, x),
        };
        Pt::new(u, v)
    }

    /// A pixel position of the stored `w` x `h` image to the EXIF-oriented image's pixel position.
    pub fn px_to_oriented(self, x: f64, y: f64, w: u32, h: u32) -> (f64, f64) {
        let (ow, oh) = self.oriented_dims(w, h);
        self.to_oriented(Pt::from_px(x, y, w, h)).to_px(ow, oh)
    }

    /// Inverse of [`ExifOrientation::px_to_oriented`]: an oriented-image pixel position to the
    /// stored image's.
    pub fn px_from_oriented(self, x: f64, y: f64, w: u32, h: u32) -> (f64, f64) {
        let (ow, oh) = self.oriented_dims(w, h);
        self.from_oriented(Pt::from_px(x, y, ow, oh)).to_px(w, h)
    }
}

impl TryFrom<u8> for ExifOrientation {
    type Error = String;
    fn try_from(v: u8) -> Result<Self, Self::Error> {
        Self::from_tag(u32::from(v)).ok_or_else(|| format!("EXIF orientation {v} is not 1 to 8"))
    }
}

impl From<ExifOrientation> for u8 {
    fn from(o: ExifOrientation) -> u8 {
        o.tag()
    }
}

/// Perspective crop of one item (PLAN 2.3 `QuadWarp`, without the fields later milestones add:
/// aspect and output size).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuadWarp {
    /// TL, TR, BR, BL, normalised, in EXIF-oriented source space. May lie outside 0..1 (a crop
    /// that extends past the frame); [`QuadWarp::sanitised`] clamps for untrusted input.
    pub corners: [Pt; 4],
    /// Clockwise quarter turns applied to the rectified result, 0..=3.
    pub quarter_turns: u8,
    /// Mirror the rectified result left to right (applied before the turns).
    #[serde(default)]
    pub mirror: bool,
    /// Fine rotation in degrees, -45..=45.
    pub fine_deg: f32,
}

impl Default for QuadWarp {
    fn default() -> Self {
        Self::inset_frame(0.0)
    }
}

impl QuadWarp {
    pub fn new(corners: [Pt; 4]) -> Self {
        Self {
            corners,
            quarter_turns: 0,
            mirror: false,
            fine_deg: 0.0,
        }
    }

    /// A quad inset by `fraction` of each side from the frame, used by "Draw crop".
    pub fn inset_frame(fraction: f64) -> Self {
        let (a, b) = (fraction, 1.0 - fraction);
        Self::new([Pt::new(a, a), Pt::new(b, a), Pt::new(b, b), Pt::new(a, b)])
    }

    /// Clamps every corner into the frame and the fine angle into range.
    pub fn sanitised(mut self) -> Self {
        for c in &mut self.corners {
            c.x = if c.x.is_finite() {
                c.x.clamp(0.0, 1.0)
            } else {
                0.0
            };
            c.y = if c.y.is_finite() {
                c.y.clamp(0.0, 1.0)
            } else {
                0.0
            };
        }
        self.quarter_turns %= 4;
        self.fine_deg = if self.fine_deg.is_finite() {
            self.fine_deg.clamp(-45.0, 45.0)
        } else {
            0.0
        };
        self
    }

    /// The same crop with the fine rotation baked into the corners: they are turned about their
    /// centre by `fine_deg` (clockwise on screen) in the pixels of a `w` x `h` image, exactly as the
    /// renderer does, and `fine_deg` becomes 0. Used when a straight crop becomes a curved page.
    pub fn with_fine_baked(&self, w: u32, h: u32) -> QuadWarp {
        let fine = f64::from(self.fine_deg);
        let mut out = self.clone();
        out.fine_deg = 0.0;
        if fine == 0.0 || w == 0 || h == 0 {
            return out;
        }
        let px = self.corners_px(w, h);
        let cx = px.iter().map(|p| p.0).sum::<f64>() / 4.0;
        let cy = px.iter().map(|p| p.1).sum::<f64>() / 4.0;
        let (s, k) = fine.to_radians().sin_cos();
        for (c, p) in out.corners.iter_mut().zip(px) {
            let (dx, dy) = (p.0 - cx, p.1 - cy);
            *c = Pt::from_px(cx + dx * k - dy * s, cy + dx * s + dy * k, w, h);
        }
        out
    }

    /// The corners in pixels of an oriented image of `w` x `h`.
    pub fn corners_px(&self, w: u32, h: u32) -> [(f64, f64); 4] {
        self.corners.map(|c| c.to_px(w, h))
    }

    /// Builds a quad from pixel corners (TL, TR, BR, BL) of an oriented `w` x `h` image.
    pub fn from_corners_px(px: [(f64, f64); 4], w: u32, h: u32) -> Self {
        Self::new(px.map(|(x, y)| Pt::from_px(x, y, w, h)))
    }

    /// The same crop expressed in the stored (pre-orientation) space of an image whose EXIF
    /// orientation is `o`: handy for tools that work on raw pixels.
    pub fn corners_in_stored_space(&self, o: ExifOrientation) -> [Pt; 4] {
        self.corners.map(|c| o.from_oriented(c))
    }

    /// Signed area of the quad in normalised units (shoelace; positive for TL, TR, BR, BL on a
    /// y-down plane).
    pub fn signed_area(&self) -> f64 {
        let c = &self.corners;
        let mut s = 0.0;
        for i in 0..4 {
            let (a, b) = (c[i], c[(i + 1) % 4]);
            s += a.x * b.y - b.x * a.y;
        }
        s / 2.0
    }

    /// Rejects quads that cannot be warped: a non-finite corner, or an area (or a side) of
    /// effectively zero. Does not judge plausibility; that is the detector's job.
    pub fn check(&self) -> Result<(), ErrKind> {
        if self.corners.iter().any(|c| !c.is_finite()) || !self.fine_deg.is_finite() {
            return Err(ErrKind::Degenerate);
        }
        if self.signed_area().abs() < 1e-9 {
            return Err(ErrKind::Degenerate);
        }
        for i in 0..4 {
            let (a, b) = (self.corners[i], self.corners[(i + 1) % 4]);
            if (a.x - b.x).abs() < 1e-12 && (a.y - b.y).abs() < 1e-12 {
                return Err(ErrKind::Degenerate);
            }
        }
        Ok(())
    }
}

/// Dense dewarp grid (B10): the outline plus `cols` x `rows` source-space nodes. Its shape is
/// owned by M12.27; in v1 it is a plain serde payload that the model carries and the renderer does
/// not interpret yet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GridWarp {
    pub outline: [Pt; 4],
    pub cols: u16,
    pub rows: u16,
    /// Row-major `cols * rows` source-space nodes.
    pub nodes: Vec<Pt>,
    /// Identifier of the model that produced the grid (provenance; render ignores it).
    pub model: String,
    pub quarter_turns: u8,
    pub mirror: bool,
}

impl Default for GridWarp {
    fn default() -> Self {
        Self {
            outline: QuadWarp::default().corners,
            cols: 0,
            rows: 0,
            nodes: Vec::new(),
            model: String::new(),
            quarter_turns: 0,
            mirror: false,
        }
    }
}

/// How one item is cut out of the source (PLAN 2.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Geometry {
    /// Convert-only, or keep the whole image.
    #[default]
    Identity,
    /// Homography from four normalised corners in EXIF-oriented source space.
    Quad(QuadWarp),
    /// Dense dewarp; shared via `Arc` because a grid is tens of kilobytes.
    Grid(Arc<GridWarp>),
    /// A page with four editable boundary curves (`docs/dev/curved-pages.md`), flattened with a
    /// Coons patch. Never proposed by the detector: it exists only through the edit API, and it is
    /// held for review until the user accepts it. Edited through its curves only.
    Curved(CurveWarp),
}

impl Geometry {
    /// The plain quad, for `Quad` only. A curved page has no plain quad: use
    /// [`Geometry::outline_quad`] for its corners.
    pub fn quad(&self) -> Option<&QuadWarp> {
        match self {
            Geometry::Quad(q) => Some(q),
            _ => None,
        }
    }

    /// The curves of a `Curved` geometry.
    pub fn curves(&self) -> Option<&CurveWarp> {
        match self {
            Geometry::Curved(c) => Some(c),
            _ => None,
        }
    }

    pub fn is_curved(&self) -> bool {
        matches!(self, Geometry::Curved(_))
    }

    /// The straight quad of a page: the quad itself, or the corners (with the turns and mirror) of
    /// a curved page. What the views show as the page outline. `None` for identity and grids.
    pub fn outline_quad(&self) -> Option<QuadWarp> {
        match self {
            Geometry::Quad(q) => Some(q.clone()),
            Geometry::Curved(c) => Some(c.outline()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitising_clamps_corners_and_angle() {
        let q = QuadWarp {
            corners: [
                Pt::new(-1.0, 2.0),
                Pt::new(f64::NAN, 0.5),
                Pt::new(0.5, 0.5),
                Pt::new(1.5, 1.5),
            ],
            quarter_turns: 6,
            mirror: false,
            fine_deg: 90.0,
        }
        .sanitised();
        assert_eq!(q.corners[0], Pt::new(0.0, 1.0));
        assert_eq!(q.corners[1], Pt::new(0.0, 0.5));
        assert_eq!(q.corners[3], Pt::new(1.0, 1.0));
        assert_eq!(q.quarter_turns, 2);
        assert_eq!(q.fine_deg, 45.0);
    }

    #[test]
    fn corners_may_exceed_the_frame_without_sanitising() {
        let q = QuadWarp::new([
            Pt::new(-0.05, -0.02),
            Pt::new(1.04, 0.0),
            Pt::new(1.02, 1.03),
            Pt::new(0.0, 1.01),
        ]);
        assert_eq!(q.check(), Ok(()));
        assert!(q.corners[0].x < 0.0);
    }

    #[test]
    fn degenerate_quads_are_rejected() {
        let p = Pt::new(0.5, 0.5);
        assert_eq!(
            QuadWarp::new([p, p, p, p]).check(),
            Err(ErrKind::Degenerate)
        );
        // Collinear corners.
        let line = QuadWarp::new([
            Pt::new(0.1, 0.1),
            Pt::new(0.4, 0.4),
            Pt::new(0.7, 0.7),
            Pt::new(0.9, 0.9),
        ]);
        assert_eq!(line.check(), Err(ErrKind::Degenerate));
        let nan = QuadWarp::new([Pt::new(f64::NAN, 0.0); 4]);
        assert_eq!(nan.check(), Err(ErrKind::Degenerate));
        assert_eq!(QuadWarp::inset_frame(0.1).check(), Ok(()));
    }

    #[test]
    fn exif_tags_one_to_eight_map_the_stored_corners() {
        // Stored image 4 wide, 2 high; the stored top-left pixel (0,0) lands here when oriented.
        let tl = Pt::new(0.0, 0.0);
        let expect = [
            (1, Pt::new(0.0, 0.0)),
            (2, Pt::new(1.0, 0.0)),
            (3, Pt::new(1.0, 1.0)),
            (4, Pt::new(0.0, 1.0)),
            (5, Pt::new(0.0, 0.0)),
            (6, Pt::new(1.0, 0.0)),
            (7, Pt::new(1.0, 1.0)),
            (8, Pt::new(0.0, 1.0)),
        ];
        for (tag, want) in expect {
            let o = ExifOrientation::from_tag(tag).unwrap();
            assert_eq!(o.to_oriented(tl), want, "tag {tag}");
            assert_eq!(o.tag() as u32, tag);
            assert_eq!(
                o.oriented_dims(4, 2),
                if tag >= 5 { (2, 4) } else { (4, 2) }
            );
        }
        assert_eq!(ExifOrientation::from_tag(0), None);
        assert_eq!(ExifOrientation::from_tag(9), None);
        // Stored top-right (1,0) under a 90 degree clockwise display turn is the bottom-right.
        assert_eq!(
            ExifOrientation::Rotate90Cw.to_oriented(Pt::new(1.0, 0.0)),
            Pt::new(1.0, 1.0)
        );
    }

    #[test]
    fn pixel_round_trip_is_exact_to_1e_4_px_at_12k_for_every_orientation() {
        let (w, h) = (12_000u32, 9_000u32);
        let pts = [
            (0.0, 0.0),
            (11_999.5, 8_999.5),
            (1_234.567_891, 8_000.123_456),
            (6_000.25, 4_500.75),
            (-35.5, 9_040.25), // outside the frame
        ];
        for o in ExifOrientation::ALL {
            for (x, y) in pts {
                let (ox, oy) = o.px_to_oriented(x, y, w, h);
                let (bx, by) = o.px_from_oriented(ox, oy, w, h);
                assert!(
                    (bx - x).abs() < 1e-4 && (by - y).abs() < 1e-4,
                    "{o:?} {x},{y}"
                );
                // The oriented point lies in the oriented frame iff the stored one is in its own.
                let (ow, oh) = o.oriented_dims(w, h);
                if (0.0..=f64::from(w)).contains(&x) && (0.0..=f64::from(h)).contains(&y) {
                    assert!(
                        (0.0..=f64::from(ow)).contains(&ox) && (0.0..=f64::from(oh)).contains(&oy)
                    );
                }
            }
        }
    }

    #[test]
    fn quad_pixel_round_trip_at_12k() {
        let (w, h) = (12_000u32, 9_000u32);
        let px = [
            (301.7, 250.1),
            (11_650.3, 410.9),
            (11_702.2, 8_550.6),
            (250.0, 8_600.4),
        ];
        let q = QuadWarp::from_corners_px(px, w, h);
        for (a, b) in q.corners_px(w, h).iter().zip(px) {
            assert!((a.0 - b.0).abs() < 1e-4 && (a.1 - b.1).abs() < 1e-4);
        }
        // Through an orientation and back.
        for o in ExifOrientation::ALL {
            let stored = q.corners_in_stored_space(o);
            for (s, orig) in stored.iter().zip(q.corners) {
                let back = o.to_oriented(*s);
                assert!((back.x - orig.x).abs() < 1e-12 && (back.y - orig.y).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn geometry_serde_round_trips_every_variant() {
        let grid = GridWarp {
            cols: 2,
            rows: 2,
            nodes: vec![
                Pt::new(0.0, 0.0),
                Pt::new(1.0, 0.0),
                Pt::new(0.0, 1.0),
                Pt::new(1.0, 1.0),
            ],
            model: "test".into(),
            ..GridWarp::default()
        };
        let mut q = QuadWarp::inset_frame(0.05);
        q.mirror = true;
        q.quarter_turns = 3;
        q.fine_deg = -1.5;
        for g in [
            Geometry::Identity,
            Geometry::Quad(q),
            Geometry::Grid(Arc::new(grid)),
        ] {
            let json = serde_json::to_string(&g).unwrap();
            assert_eq!(
                serde_json::from_str::<Geometry>(&json).unwrap(),
                g,
                "{json}"
            );
        }
        let v = serde_json::to_value(Geometry::Quad(QuadWarp::inset_frame(0.1))).unwrap();
        assert_eq!(v["type"], "quad");
        assert!(v.get("quarterTurns").is_some() && v.get("mirror").is_some());
        // Old records without `mirror` still load.
        let old = r#"{"corners":[{"x":0,"y":0},{"x":1,"y":0},{"x":1,"y":1},{"x":0,"y":1}],"quarterTurns":1,"fineDeg":2.0}"#;
        let q: QuadWarp = serde_json::from_str(old).unwrap();
        assert!(!q.mirror && q.quarter_turns == 1);
    }

    #[test]
    fn orientation_serialises_as_its_tag() {
        assert_eq!(
            serde_json::to_string(&ExifOrientation::Rotate90Cw).unwrap(),
            "6"
        );
        assert!(serde_json::from_str::<ExifOrientation>("9").is_err());
        assert_eq!(
            serde_json::from_str::<ExifOrientation>("3").unwrap(),
            ExifOrientation::Rotate180
        );
    }
}
