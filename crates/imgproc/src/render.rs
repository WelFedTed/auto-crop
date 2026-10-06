// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Renders a [`QuadWarp`] from a source raster: perspective-correct the quad to an upright
//! rectangle, apply the quarter turns and the fine rotation. A pure function of its inputs.

use crate::Raster;
use crate::cancel::{Cancel, NeverCancel};
use crate::geometry::{dist, homography};
use crate::pixels::ImageRef;
use crate::warp::warp_perspective_image;
use auto_crop_core::QuadWarp;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderError {
    /// The four corners do not form a usable quadrilateral (also: curves that are not a valid
    /// page outline, or a flat page smaller than 2 x 2 pixels).
    DegenerateQuad,
    /// The cancel token fired between bands; no partial output escapes.
    Cancelled,
}

/// Limits for one render. `max_pixels` and `max_edge` shrink the output (a smaller output is
/// resampled directly from the source); both are upper bounds.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_pixels: u64,
    pub max_edge: u32,
}

impl Limits {
    pub const fn pixels(max_pixels: u64) -> Self {
        Self {
            max_pixels,
            max_edge: u32::MAX,
        }
    }
}

/// Corners in source pixel-centre coordinates after the fine rotation about the quad's centre.
fn corners_px(src_w: u32, src_h: u32, q: &QuadWarp) -> [(f64, f64); 4] {
    let mut c = [(0.0, 0.0); 4];
    for (i, p) in q.corners.iter().enumerate() {
        c[i] = (p.x * f64::from(src_w) - 0.5, p.y * f64::from(src_h) - 0.5);
    }
    let fine = f64::from(q.fine_deg);
    if fine != 0.0 {
        let cx = c.iter().map(|p| p.0).sum::<f64>() / 4.0;
        let cy = c.iter().map(|p| p.1).sum::<f64>() / 4.0;
        let (s, k) = fine.to_radians().sin_cos();
        for p in &mut c {
            let (dx, dy) = (p.0 - cx, p.1 - cy);
            // Clockwise in image space (y down) for a positive angle.
            *p = (cx + dx * k - dy * s, cy + dx * s + dy * k);
        }
    }
    c
}

/// Natural output size of the quad in source pixels before limits, after the quarter turns.
pub fn output_size(src_w: u32, src_h: u32, q: &QuadWarp, limits: Limits) -> (u32, u32) {
    let c = corners_px(src_w, src_h, q);
    let w = (dist(c[0], c[1]) + dist(c[3], c[2])) / 2.0;
    let h = (dist(c[0], c[3]) + dist(c[1], c[2])) / 2.0;
    let (w, h) = if q.quarter_turns % 2 == 1 {
        (h, w)
    } else {
        (w, h)
    };
    cap_size(w, h, limits)
}

/// A natural size in pixels shrunk (never enlarged) to the pixel cap and the long-edge cap of
/// `limits`, rounded, at least 1 x 1. Shared by the quad and the curved render.
pub(crate) fn cap_size(w: f64, h: f64, limits: Limits) -> (u32, u32) {
    let mut scale: f64 = 1.0;
    let pixels = w * h;
    if pixels > limits.max_pixels as f64 {
        scale = scale.min((limits.max_pixels as f64 / pixels).sqrt());
    }
    let long = w.max(h);
    if long > f64::from(limits.max_edge) {
        scale = scale.min(f64::from(limits.max_edge) / long);
    }
    let w = (w * scale).round().max(1.0);
    let h = (h * scale).round().max(1.0);
    (w as u32, h as u32)
}

pub fn render_quad(src: &Raster, q: &QuadWarp, limits: Limits) -> Result<Raster, RenderError> {
    render_quad_cancellable(src, q, limits, &NeverCancel)
}

/// [`render_quad`] that stops at the next 64-row band with [`RenderError::Cancelled`] once
/// `cancel` fires (ROADMAP M1.54: the skeleton's `rectify` stage honours the job token).
pub fn render_quad_cancellable(
    src: &Raster,
    q: &QuadWarp,
    limits: Limits,
    cancel: &dyn Cancel,
) -> Result<Raster, RenderError> {
    let c = corners_px(src.width, src.height, q);
    let (w, h) = output_size(src.width, src.height, q, limits);
    if w < 2 || h < 2 {
        return Err(RenderError::DegenerateQuad);
    }
    // Quarter turns: the result's corners take the quad's corners shifted by the turn count.
    let t = usize::from(q.quarter_turns % 4);
    let src_corners: [(f64, f64); 4] = std::array::from_fn(|i| c[(i + 4 - t) % 4]);
    let (wf, hf) = (f64::from(w), f64::from(h));
    let dst = [
        (-0.5, -0.5),
        (wf - 0.5, -0.5),
        (wf - 0.5, hf - 0.5),
        (-0.5, hf - 0.5),
    ];
    let m = homography(dst, src_corners).ok_or(RenderError::DegenerateQuad)?;
    let view = ImageRef {
        width: src.width,
        height: src.height,
        channels: 3,
        data: &src.data[..],
    };
    let out = warp_perspective_image(view, &m, w, h, cancel).map_err(|_| RenderError::Cancelled)?;
    Ok(Raster {
        width: out.width,
        height: out.height,
        data: out.data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use auto_crop_core::Pt;

    /// A source with a bright 100x60 rectangle at (50, 40) on a dark field, split into left and
    /// right halves so orientation is checkable.
    fn source() -> Raster {
        let mut r = Raster::filled(200, 150, [20, 20, 20]);
        for y in 40..100 {
            for x in 50..150 {
                r.set_pixel(x, y, if x < 100 { [255, 0, 0] } else { [0, 0, 255] });
            }
        }
        r
    }

    fn rect_quad() -> QuadWarp {
        let n = |x: f64, y: f64| Pt::new((x + 0.5) / 200.0, (y + 0.5) / 150.0);
        QuadWarp::new([
            n(50.0, 40.0),
            n(150.0, 40.0),
            n(150.0, 100.0),
            n(50.0, 100.0),
        ])
    }

    #[test]
    fn an_axis_aligned_quad_crops_exactly() {
        let out = render_quad(&source(), &rect_quad(), Limits::pixels(u64::MAX)).unwrap();
        assert_eq!((out.width, out.height), (100, 60));
        assert_eq!(out.pixel(10, 30), [255, 0, 0]);
        assert_eq!(out.pixel(90, 30), [0, 0, 255]);
    }

    #[test]
    fn a_quarter_turn_swaps_the_dimensions_and_rotates_clockwise() {
        let mut q = rect_quad();
        q.quarter_turns = 1;
        let out = render_quad(&source(), &q, Limits::pixels(u64::MAX)).unwrap();
        assert_eq!((out.width, out.height), (60, 100));
        // Red was on the left; after a clockwise turn it is on top.
        assert_eq!(out.pixel(30, 10), [255, 0, 0]);
        assert_eq!(out.pixel(30, 90), [0, 0, 255]);
    }

    #[test]
    fn limits_shrink_the_output_but_never_enlarge_it() {
        let q = rect_quad();
        let small = render_quad(
            &source(),
            &q,
            Limits {
                max_pixels: u64::MAX,
                max_edge: 50,
            },
        )
        .unwrap();
        assert_eq!((small.width, small.height), (50, 30));
        let capped = render_quad(&source(), &q, Limits::pixels(1500)).unwrap();
        assert!(u64::from(capped.width) * u64::from(capped.height) <= 1600);
    }

    #[test]
    fn a_cancelled_token_stops_the_render_without_output() {
        let stop = std::sync::atomic::AtomicBool::new(true);
        assert_eq!(
            render_quad_cancellable(&source(), &rect_quad(), Limits::pixels(u64::MAX), &stop),
            Err(RenderError::Cancelled)
        );
        let go = std::sync::atomic::AtomicBool::new(false);
        let out = render_quad_cancellable(&source(), &rect_quad(), Limits::pixels(u64::MAX), &go)
            .unwrap();
        assert_eq!(
            out,
            render_quad(&source(), &rect_quad(), Limits::pixels(u64::MAX)).unwrap()
        );
    }

    #[test]
    fn a_collapsed_quad_is_an_error_not_a_panic() {
        let p = Pt::new(0.5, 0.5);
        let q = QuadWarp::new([p, p, p, p]);
        assert_eq!(
            render_quad(&source(), &q, Limits::pixels(u64::MAX)),
            Err(RenderError::DegenerateQuad)
        );
    }
}
