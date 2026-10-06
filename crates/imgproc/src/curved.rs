// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Flattens a curved page (`docs/dev/curved-pages.md`, ROADMAP M12.30 to M12.32): the four
//! boundary curves of a [`CurveWarp`] define a Coons patch, and every output pixel samples the
//! source at the patch point of its page position with the Lanczos3 kernel of the homography
//! path (same band loop, same cancel polling, same bytes at any thread count).
//!
//! The patch is evaluated exactly per pixel, never as a full-frame map: the four edges are
//! tabulated once at one entry per output column or row (`O(width + height)` points), and a
//! 64-row band computes its positions from those tables. The extra heap is the output image plus a
//! few row-sized vectors per worker thread, like the homography warp.
//!
//! Conventions, all as the quad path:
//!
//! * pixel centres are at integer coordinates; the outer pixel EDGES of the flat page map to the
//!   page edges, so output pixel `(x, y)` of a `W` x `H` page samples `S((x + 0.5) / W, (y + 0.5) /
//!   H)` and a straight rectangle equals the homography result;
//! * the output size is the longer of top and bottom by the longer of left and right, measured by
//!   arc length in source pixels, never enlarged, then capped by [`Limits`];
//! * quarter turns act on the flat page (a clockwise turn per step); the mirror flips the flat page
//!   left to right first;
//! * positions more than half a pixel outside the source give zero.

use crate::Raster;
use crate::cancel::{Cancel, NeverCancel};
use crate::pixels::{Image, ImageRef, Sample};
use crate::render::{Limits, RenderError, cap_size};
use crate::warp::warp_map_image;
use auto_crop_core::{CoonsSampler, CurveWarp, Pt};

/// Size of the flat page BEFORE the turns, in pixels, for a `src_w` x `src_h` source: arc lengths
/// of the edges in source pixels, capped by `limits`.
pub fn flat_size(src_w: u32, src_h: u32, c: &CurveWarp, limits: Limits) -> (u32, u32) {
    let (w, h) = c.flat_size((f64::from(src_w), f64::from(src_h)));
    cap_size(w, h, limits)
}

/// Size of the output after the quarter turns (width and height swap for an odd count).
pub fn output_size(src_w: u32, src_h: u32, c: &CurveWarp, limits: Limits) -> (u32, u32) {
    let (w, h) = flat_size(src_w, src_h, c, limits);
    if c.quarter_turns % 2 == 1 {
        (h, w)
    } else {
        (w, h)
    }
}

/// The page edges tabulated at one entry per flat-page column or row, in source pixel-centre
/// coordinates, plus the geometry of the turns.
struct Tables {
    wf: usize,
    hf: usize,
    top: Vec<(f64, f64)>,
    bottom: Vec<(f64, f64)>,
    left: Vec<(f64, f64)>,
    right: Vec<(f64, f64)>,
    corners: [(f64, f64); 4],
    turns: u8,
    mirror: bool,
}

impl Tables {
    fn new(c: &CurveWarp, src_w: u32, src_h: u32, wf: usize, hf: usize) -> Self {
        let scale = (f64::from(src_w), f64::from(src_h));
        let s: CoonsSampler = c.sampler(scale);
        let px = |p: Pt| (p.x * scale.0 - 0.5, p.y * scale.1 - 0.5);
        let col = |i: usize| (i as f64 + 0.5) / wf as f64;
        let row = |j: usize| (j as f64 + 0.5) / hf as f64;
        Self {
            wf,
            hf,
            top: (0..wf).map(|i| px(s.top_at(col(i)))).collect(),
            bottom: (0..wf).map(|i| px(s.bottom_at(col(i)))).collect(),
            left: (0..hf).map(|j| px(s.left_at(row(j)))).collect(),
            right: (0..hf).map(|j| px(s.right_at(row(j)))).collect(),
            corners: s.corners().map(px),
            turns: c.quarter_turns % 4,
            mirror: c.mirror,
        }
    }

    /// Source positions of output row `y` (the output is the flat page after mirror and turns).
    fn row(&self, y: usize, xs: &mut [f64], ys: &mut [f64]) {
        let (wf, hf) = (self.wf, self.hf);
        let [tl, tr, br, bl] = self.corners;
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
            let (t, b, l, r) = (self.top[xi], self.bottom[xi], self.left[yi], self.right[yi]);
            let (w_tl, w_tr, w_bl, w_br) =
                ((1.0 - u) * (1.0 - v), u * (1.0 - v), (1.0 - u) * v, u * v);
            *ox = (1.0 - v) * t.0 + v * b.0 + (1.0 - u) * l.0 + u * r.0
                - (w_tl * tl.0 + w_tr * tr.0 + w_bl * bl.0 + w_br * br.0);
            *oy = (1.0 - v) * t.1 + v * b.1 + (1.0 - u) * l.1 + u * r.1
                - (w_tl * tl.1 + w_tr * tr.1 + w_bl * bl.1 + w_br * br.1);
        }
    }
}

/// Flattens `c` out of `src` (any channel count and depth the warp supports). Invalid curves, a
/// page smaller than 2 x 2 pixels or a cancelled token are errors; nothing partial escapes.
pub fn render_curved_image<T: Sample>(
    src: ImageRef<'_, T>,
    c: &CurveWarp,
    limits: Limits,
    cancel: &dyn Cancel,
) -> Result<Image<T>, RenderError> {
    c.validate().map_err(|_| RenderError::DegenerateQuad)?;
    let (wf, hf) = flat_size(src.width, src.height, c, limits);
    if wf < 2 || hf < 2 {
        return Err(RenderError::DegenerateQuad);
    }
    let (wo, ho) = output_size(src.width, src.height, c, limits);
    let tables = Tables::new(c, src.width, src.height, wf as usize, hf as usize);
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

/// The Coons patch as a `cols` x `rows` grid of source pixel-centre coordinates (for the dense-grid
/// warp, `crate::warp::warp_grid_image`, and for overlays). Edges are walked by arc length in
/// source pixels, exactly as the renderer does.
pub fn coons_grid_px(
    src_w: u32,
    src_h: u32,
    c: &CurveWarp,
    cols: usize,
    rows: usize,
) -> Vec<(f64, f64)> {
    let scale = (f64::from(src_w), f64::from(src_h));
    c.to_grid_scaled(cols, rows, scale)
        .nodes
        .iter()
        .map(|p| (p.x * scale.0 - 0.5, p.y * scale.1 - 0.5))
        .collect()
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
    }
}
