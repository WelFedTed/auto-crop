// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Flattens a curved page (`docs/dev/curved-pages.md`, ROADMAP M12.29 to M12.32): the four boundary
//! curves of a [`CurveWarp`] define a Coons patch in the page's **rectified space**, and every
//! output pixel samples the source at the patch point of its page position with the Lanczos3
//! kernel of the homography path (same band loop, same cancel polling, same bytes at any thread
//! count).
//!
//! The composition is the one of M12.29: the corner homography first, then the curve flow.
//!
//! 1. The four corners define a homography `H` from a rectangle `wq` x `hq` (the average lengths of
//!    the opposite corner distances, exactly the size a plain quad crop gets) onto the quad.
//! 2. The curves (their 64-samples-per-segment polylines, evaluated in the image as drawn) are
//!    moved into the rectangle with `H^-1`; arc length is measured there, in rectified pixels.
//! 3. The Coons patch of those rectified edges maps the flat page onto the rectangle, and `H` maps
//!    that to the source. With four straight edges the patch is the identity, so the result IS the
//!    homography crop, for any perspective quad, not only for a rectangle.
//!
//! The patch is evaluated exactly per pixel, never as a full-frame map: the four rectified edges
//! are tabulated once at one entry per output column or row (`O(width + height)` points), and a
//! 64-row band computes its positions from those tables. The extra heap is the output image plus a
//! few row-sized vectors per worker thread, like the homography warp.
//!
//! Conventions, all as the quad path:
//!
//! * pixel centres are at integer coordinates; the outer pixel EDGES of the flat page map to the
//!   page edges, so output pixel `(x, y)` of a `W` x `H` page samples the patch at `((x + 0.5) / W,
//!   (y + 0.5) / H)`;
//! * the output size is the longer of top and bottom by the longer of left and right, measured by
//!   arc length in rectified pixels, never enlarged, then capped by [`Limits`];
//! * quarter turns act on the flat page (a clockwise turn per step); the mirror flips the flat page
//!   left to right first;
//! * positions more than half a pixel outside the source give zero.

use crate::Raster;
use crate::cancel::{Cancel, NeverCancel};
use crate::geometry::{H, apply, dist, homography, invert};
use crate::pixels::{Image, ImageRef, Sample};
use crate::render::{Limits, RenderError, cap_size};
use crate::warp::warp_map_image;
use auto_crop_core::{ArcCurve, CurveWarp, Pt};

/// The page in its rectified space: the corner homography and the four edges as arc-length tables
/// of rectified points.
struct Rectified {
    /// Rectangle (continuous coordinates, `0..=wq` by `0..=hq`) to source pixel-centre coordinates.
    h: H,
    wq: f64,
    hq: f64,
    top: ArcCurve,
    right: ArcCurve,
    bottom: ArcCurve,
    left: ArcCurve,
}

impl Rectified {
    /// `None` when the corners cannot define a homography or a curve point maps to infinity.
    fn new(c: &CurveWarp, sw: u32, sh: u32) -> Option<Self> {
        let (w, h) = (f64::from(sw), f64::from(sh));
        let px = |p: Pt| (p.x * w - 0.5, p.y * h - 0.5);
        let [tl, tr, br, bl] = c.corners().map(px);
        let wq = (dist(tl, tr) + dist(bl, br)) / 2.0;
        let hq = (dist(tl, bl) + dist(tr, br)) / 2.0;
        if !(wq > 1e-9 && hq > 1e-9) {
            return None;
        }
        let fwd = homography(
            [(0.0, 0.0), (wq, 0.0), (wq, hq), (0.0, hq)],
            [tl, tr, br, bl],
        )?;
        let inv = invert(&fwd)?;
        let rectify = |curve: &auto_crop_core::Curve| -> Option<ArcCurve> {
            let pts: Option<Vec<Pt>> = curve
                .polyline()
                .into_iter()
                .map(|p| apply(&inv, px(p).0, px(p).1).map(|(x, y)| Pt::new(x, y)))
                .collect();
            ArcCurve::from_points(pts?, (1.0, 1.0))
        };
        Some(Self {
            h: fwd,
            wq,
            hq,
            top: rectify(&c.top)?,
            right: rectify(&c.right)?,
            bottom: rectify(&c.bottom)?,
            left: rectify(&c.left)?,
        })
    }

    /// The natural flat size in rectified pixels: longer of top and bottom by longer of left and
    /// right.
    fn flat_size(&self) -> (f64, f64) {
        (
            self.top.length().max(self.bottom.length()),
            self.left.length().max(self.right.length()),
        )
    }

    fn corners(&self) -> [(f64, f64); 4] {
        [
            (0.0, 0.0),
            (self.wq, 0.0),
            (self.wq, self.hq),
            (0.0, self.hq),
        ]
    }

    /// The patch point of page position `(u, v)` in rectified space.
    fn rect_point(&self, u: f64, v: f64) -> (f64, f64) {
        let t = self.top.at(u);
        let b = self.bottom.at(1.0 - u);
        let l = self.left.at(1.0 - v);
        let r = self.right.at(v);
        coons(
            self.corners(),
            u,
            v,
            (t.x, t.y),
            (b.x, b.y),
            (l.x, l.y),
            (r.x, r.y),
        )
    }

    /// Rectified point to source pixel-centre coordinates (NaN where the map goes to infinity).
    fn to_src(&self, p: (f64, f64)) -> (f64, f64) {
        project(&self.h, p)
    }
}

#[inline(always)]
fn project(h: &H, (x, y): (f64, f64)) -> (f64, f64) {
    // A reciprocal that is infinite or NaN (w = 0) makes both coordinates non-finite, and the
    // sampler treats a non-finite position as outside the source.
    let inv = 1.0 / (h[6] * x + h[7] * y + h[8]);
    (
        (h[0] * x + h[1] * y + h[2]) * inv,
        (h[3] * x + h[4] * y + h[5]) * inv,
    )
}

/// The Coons formula from looked-up edge points: `(1-v) T + v B + (1-u) L + u R` minus the
/// bilinear corner term.
#[inline(always)]
fn coons(
    [tl, tr, br, bl]: [(f64, f64); 4],
    u: f64,
    v: f64,
    t: (f64, f64),
    b: (f64, f64),
    l: (f64, f64),
    r: (f64, f64),
) -> (f64, f64) {
    let (w_tl, w_tr, w_bl, w_br) = ((1.0 - u) * (1.0 - v), u * (1.0 - v), (1.0 - u) * v, u * v);
    (
        (1.0 - v) * t.0 + v * b.0 + (1.0 - u) * l.0 + u * r.0
            - (w_tl * tl.0 + w_tr * tr.0 + w_bl * bl.0 + w_br * br.0),
        (1.0 - v) * t.1 + v * b.1 + (1.0 - u) * l.1 + u * r.1
            - (w_tl * tl.1 + w_tr * tr.1 + w_bl * bl.1 + w_br * br.1),
    )
}

/// Size of the flat page BEFORE the turns, in pixels, for a `src_w` x `src_h` source: arc lengths
/// of the rectified edges, capped by `limits`. `None` for a page that cannot be rectified.
pub fn flat_size(src_w: u32, src_h: u32, c: &CurveWarp, limits: Limits) -> Option<(u32, u32)> {
    let (w, h) = Rectified::new(c, src_w, src_h)?.flat_size();
    Some(cap_size(w, h, limits))
}

/// Size of the output after the quarter turns (width and height swap for an odd count).
pub fn output_size(src_w: u32, src_h: u32, c: &CurveWarp, limits: Limits) -> Option<(u32, u32)> {
    let (w, h) = flat_size(src_w, src_h, c, limits)?;
    Some(if c.quarter_turns % 2 == 1 {
        (h, w)
    } else {
        (w, h)
    })
}

/// The rectified page edges tabulated at one entry per flat-page column or row, plus the geometry
/// of the turns.
struct Tables {
    wf: usize,
    hf: usize,
    top: Vec<(f64, f64)>,
    bottom: Vec<(f64, f64)>,
    left: Vec<(f64, f64)>,
    right: Vec<(f64, f64)>,
    corners: [(f64, f64); 4],
    h: H,
    turns: u8,
    mirror: bool,
}

impl Tables {
    fn new(rect: &Rectified, c: &CurveWarp, wf: usize, hf: usize) -> Self {
        let col = |i: usize| (i as f64 + 0.5) / wf as f64;
        let row = |j: usize| (j as f64 + 0.5) / hf as f64;
        let xy = |p: Pt| (p.x, p.y);
        Self {
            wf,
            hf,
            top: (0..wf).map(|i| xy(rect.top.at(col(i)))).collect(),
            bottom: (0..wf).map(|i| xy(rect.bottom.at(1.0 - col(i)))).collect(),
            left: (0..hf).map(|j| xy(rect.left.at(1.0 - row(j)))).collect(),
            right: (0..hf).map(|j| xy(rect.right.at(row(j)))).collect(),
            corners: rect.corners(),
            h: rect.h,
            turns: c.quarter_turns % 4,
            mirror: c.mirror,
        }
    }

    /// Source positions of output row `y` (the output is the flat page after mirror and turns).
    fn row(&self, y: usize, xs: &mut [f64], ys: &mut [f64]) {
        let (wf, hf) = (self.wf, self.hf);
        for (x, (ox, oy)) in xs.iter_mut().zip(ys.iter_mut()).enumerate() {
            // Where this output pixel sits on the flat page (the inverse of the clockwise turn).
            let (mut xi, yi) = match self.turns {
                0 => (x, y),
                1 => (y, hf - 1 - x),
                2 => (wf - 1 - x, hf - 1 - y),
                _ => (wf - 1 - y, x),
            };
            if self.mirror {
                xi = wf - 1 - xi;
            }
            let (u, v) = ((xi as f64 + 0.5) / wf as f64, (yi as f64 + 0.5) / hf as f64);
            let p = coons(
                self.corners,
                u,
                v,
                self.top[xi],
                self.bottom[xi],
                self.left[yi],
                self.right[yi],
            );
            (*ox, *oy) = project(&self.h, p);
        }
    }
}

/// Flattens `c` out of `src` (any channel count and depth the warp supports). Invalid curves, a
/// page that cannot be rectified, a page smaller than 2 x 2 pixels or a cancelled token are
/// errors; nothing partial escapes.
pub fn render_curved_image<T: Sample>(
    src: ImageRef<'_, T>,
    c: &CurveWarp,
    limits: Limits,
    cancel: &dyn Cancel,
) -> Result<Image<T>, RenderError> {
    c.validate().map_err(|_| RenderError::DegenerateQuad)?;
    let rect = Rectified::new(c, src.width, src.height).ok_or(RenderError::DegenerateQuad)?;
    let (w, h) = rect.flat_size();
    let (wf, hf) = cap_size(w, h, limits);
    if wf < 2 || hf < 2 {
        return Err(RenderError::DegenerateQuad);
    }
    let (wo, ho) = if c.quarter_turns % 2 == 1 {
        (hf, wf)
    } else {
        (wf, hf)
    };
    let tables = Tables::new(&rect, c, wf as usize, hf as usize);
    let map = |y: usize, xs: &mut [f64], ys: &mut [f64]| tables.row(y, xs, ys);
    warp_map_image(src, wo, ho, cancel, &map).map_err(|_| RenderError::Cancelled)
}

/// [`render_curved_image`] for an 8-bit RGB raster.
pub fn render_curved_cancellable(
    src: &Raster,
    c: &CurveWarp,
    limits: Limits,
    cancel: &dyn Cancel,
) -> Result<Raster, RenderError> {
    let view = ImageRef {
        width: src.width,
        height: src.height,
        channels: 3,
        data: &src.data[..],
    };
    let out = render_curved_image(view, c, limits, cancel)?;
    Ok(Raster {
        width: out.width,
        height: out.height,
        data: out.data,
    })
}

/// [`render_curved_cancellable`] that cannot be cancelled.
pub fn render_curved(src: &Raster, c: &CurveWarp, limits: Limits) -> Result<Raster, RenderError> {
    render_curved_cancellable(src, c, limits, &NeverCancel)
}

/// The patch as a `cols` x `rows` grid of source pixel-centre coordinates, node `(i, j)` at page
/// position `(i / (cols - 1), j / (rows - 1))` (for the dense-grid warp,
/// `crate::warp::warp_grid_image`, and for overlays). Computed exactly as the renderer does. `None`
/// for a grid under 2 x 2 or a page that cannot be rectified.
pub fn coons_grid_px(
    src_w: u32,
    src_h: u32,
    c: &CurveWarp,
    cols: usize,
    rows: usize,
) -> Option<Vec<(f64, f64)>> {
    if cols < 2 || rows < 2 {
        return None;
    }
    let rect = Rectified::new(c, src_w, src_h)?;
    let mut nodes = Vec::with_capacity(cols * rows);
    for j in 0..rows {
        let v = j as f64 / (rows - 1) as f64;
        for i in 0..cols {
            let u = i as f64 / (cols - 1) as f64;
            nodes.push(rect.to_src(rect.rect_point(u, v)));
        }
    }
    Some(nodes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use auto_crop_core::{Curve, QuadWarp};

    fn curve(p: &[(f64, f64)]) -> Curve {
        Curve::new(p.iter().map(|&(x, y)| Pt::new(x, y)).collect()).unwrap()
    }

    fn halves() -> Raster {
        let mut r = Raster::filled(200, 150, [20, 20, 20]);
        for y in 40..100 {
            for x in 50..150 {
                r.set_pixel(x, y, if x < 100 { [255, 0, 0] } else { [0, 0, 255] });
            }
        }
        r
    }

    fn rect_curves() -> CurveWarp {
        let n = |x: f64, y: f64| Pt::new((x + 0.5) / 200.0, (y + 0.5) / 150.0);
        CurveWarp::from_quad(&QuadWarp::new([
            n(50.0, 40.0),
            n(150.0, 40.0),
            n(150.0, 100.0),
            n(50.0, 100.0),
        ]))
    }

    #[test]
    fn an_axis_aligned_straight_page_crops_exactly() {
        let out = render_curved(&halves(), &rect_curves(), Limits::pixels(u64::MAX)).unwrap();
        assert_eq!((out.width, out.height), (100, 60));
        assert_eq!(out.pixel(10, 30), [255, 0, 0]);
        assert_eq!(out.pixel(90, 30), [0, 0, 255]);
    }

    #[test]
    fn turns_and_mirror_act_on_the_flat_page() {
        let lim = Limits::pixels(u64::MAX);
        let mut c = rect_curves();
        c.quarter_turns = 1;
        let out = render_curved(&halves(), &c, lim).unwrap();
        assert_eq!((out.width, out.height), (60, 100));
        // Red was on the left; after a clockwise turn it is on top.
        assert_eq!(out.pixel(30, 10), [255, 0, 0]);
        assert_eq!(out.pixel(30, 90), [0, 0, 255]);
        // Mirror first: blue on the left; then the turn puts it on top.
        c.mirror = true;
        let out = render_curved(&halves(), &c, lim).unwrap();
        assert_eq!(out.pixel(30, 10), [0, 0, 255]);
        assert_eq!(out.pixel(30, 90), [255, 0, 0]);
        // Mirror alone swaps left and right.
        c.quarter_turns = 0;
        let out = render_curved(&halves(), &c, lim).unwrap();
        assert_eq!(out.pixel(10, 30), [0, 0, 255]);
        assert_eq!(out.pixel(90, 30), [255, 0, 0]);
    }

    #[test]
    fn limits_shrink_never_enlarge_and_bad_pages_are_errors() {
        let c = rect_curves();
        let small = render_curved(
            &halves(),
            &c,
            Limits {
                max_pixels: u64::MAX,
                max_edge: 50,
            },
        )
        .unwrap();
        assert_eq!((small.width, small.height), (50, 30));
        // A figure of eight is refused, not rendered.
        let mut bad = c.clone();
        bad.top = curve(&[(0.25, 0.27), (0.4, 0.9), (0.6, 0.9), (0.75, 0.27)]);
        assert_eq!(
            render_curved(&halves(), &bad, Limits::pixels(u64::MAX)),
            Err(RenderError::DegenerateQuad)
        );
        let stop = std::sync::atomic::AtomicBool::new(true);
        assert_eq!(
            render_curved_cancellable(&halves(), &c, Limits::pixels(u64::MAX), &stop),
            Err(RenderError::Cancelled)
        );
        assert_eq!(
            output_size(200, 150, &c, Limits::pixels(u64::MAX)),
            Some((100, 60))
        );
    }

    #[test]
    fn a_perspective_quad_with_straight_edges_is_exactly_the_homography_size() {
        // A keystone: the rectified size is the quad path's size (average opposite edges).
        let n = |x: f64, y: f64| Pt::new(x / 200.0, y / 150.0);
        let q = QuadWarp::new([
            n(40.0, 20.0),
            n(170.0, 25.0),
            n(150.0, 120.0),
            n(60.0, 125.0),
        ]);
        let want = crate::render::output_size(200, 150, &q, Limits::pixels(u64::MAX));
        let got = output_size(
            200,
            150,
            &CurveWarp::from_quad(&q),
            Limits::pixels(u64::MAX),
        );
        assert_eq!(got, Some(want));
    }
}
