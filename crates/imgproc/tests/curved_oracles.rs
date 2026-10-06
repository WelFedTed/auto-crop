// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Oracles of the curved-page resampler (`auto_crop_imgproc::curved`, docs/dev/curved-pages.md):
//!
//! (a) four straight curves on a rectangle equal the homography result to within 1 LSB;
//! (b) a synthetic page is rendered with a KNOWN edge distortion (a forward generator written here
//!     from analytic boundary curves, independent of the library's spline), flattened back and
//!     compared with the flat page: SSIM and straightness of the printed rules on the interior;
//! (c) the limits of a boundary-only model are measured, not hidden (a bend the edges do not show,
//!     perspective foreshortening);
//! plus: the dense-grid warp agrees with the exact one, any channel layout works, 1 and 8 threads
//! give the same bytes, and cancellation stops at a band boundary.

use auto_crop_core::{Curve, CurveWarp, Pt, QuadWarp};
use auto_crop_imgproc::Raster;
use auto_crop_imgproc::cancel::NeverCancel;
use auto_crop_imgproc::curved::{
    coons_grid_px, output_size, render_curved, render_curved_cancellable, render_curved_image,
};
use auto_crop_imgproc::pixels::{Image, ImageRef};
use auto_crop_imgproc::render::{Limits, RenderError, render_quad};
use auto_crop_imgproc::warp::{BAND_ROWS, SrcGrid, warp_grid_image};
use std::f64::consts::PI;

const UNLIMITED: Limits = Limits {
    max_pixels: u64::MAX,
    max_edge: u32::MAX,
};

fn pool(n: usize) -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new()
        .num_threads(n)
        .build()
        .unwrap()
}

fn noise(w: u32, h: u32) -> Raster {
    let mut r = Raster::new(w, h);
    let mut s = 0x9E37_79B9u32;
    for v in &mut r.data {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        *v = (s % 251) as u8;
    }
    r
}

/// A smooth gradient with a little structure, so interpolation errors show but noise does not
/// dominate.
fn textured(w: u32, h: u32) -> Raster {
    let mut r = Raster::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (f64::from(x), f64::from(y));
            let v = 128.0
                + 70.0 * (fx * 0.11).sin() * (fy * 0.07).cos()
                + 30.0 * (fx * 0.013 + fy * 0.02).sin();
            r.set_pixel(x, y, [v as u8, (v * 0.8) as u8, (255.0 - v) as u8]);
        }
    }
    r
}

fn max_diff(a: &Raster, b: &Raster) -> u8 {
    assert_eq!((a.width, a.height), (b.width, b.height));
    a.data
        .iter()
        .zip(&b.data)
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap_or(0)
}

// ------------------------------------------------------------------------------------ (a)

#[test]
fn a_straight_quad_equals_the_homography_within_one_lsb_for_any_perspective() {
    let src = noise(321, 240);
    // An axis-aligned rectangle with fractional corners, and the same rotated by 12 degrees.
    let rects: Vec<[(f64, f64); 4]> = {
        let axis = [(30.3, 22.7), (250.9, 22.7), (250.9, 190.1), (30.3, 190.1)];
        let (cx, cy) = (140.6, 106.4);
        let (s, k) = 12f64.to_radians().sin_cos();
        let rot = axis.map(|(x, y)| {
            (
                cx + (x - cx) * k - (y - cy) * s,
                cy + (x - cx) * s + (y - cy) * k,
            )
        });
        // A perspective quad: the Coons path composes with the corner homography, so it is the
        // homography crop too, not only for a rectangle.
        let persp = [(40.0, 25.0), (270.0, 20.0), (250.0, 200.0), (60.0, 190.0)];
        vec![axis, rot, persp]
    };
    let mut worst = 0u8;
    for corners in rects {
        for turns in 0..4u8 {
            let mut q = QuadWarp::from_corners_px(corners, src.width, src.height);
            q.quarter_turns = turns;
            let want = render_quad(&src, &q, UNLIMITED).unwrap();
            let got = render_curved(&src, &CurveWarp::from_quad(&q), UNLIMITED).unwrap();
            assert_eq!(
                (got.width, got.height),
                (want.width, want.height),
                "turns {turns}"
            );
            let d = max_diff(&got, &want);
            worst = worst.max(d);
            assert!(d <= 1, "turns {turns}: max difference {d} LSB");
        }
    }
    println!(
        "oracle (a): straight quads (rectangle, rotated, perspective) vs homography, worst difference {worst} LSB"
    );
}

// ------------------------------------------------------------------------------------ (b)

/// The flat page: paper with printed rules (raised-cosine, 4 px wide) and text-like blobs.
/// `s` runs along the page width, `v` down its height, both in page pixels.
fn page(s: f64, v: f64) -> f64 {
    page_with(s, v, true)
}

/// The flat page, with or without the text-like blobs (the rules alone are what the straightness
/// measurement looks at; the blobs add texture for the SSIM).
fn page_with(s: f64, v: f64, blobs: bool) -> f64 {
    let lattice = |x: f64, off: f64, step: f64| x - off - step * ((x - off) / step).round();
    let rule = |d: f64| {
        if d.abs() < 2.0 {
            0.5 * (1.0 + (PI * d / 2.0).cos())
        } else {
            0.0
        }
    };
    let mut val = 238.0;
    val -= 170.0 * rule(lattice(v, 20.0, 40.0));
    val -= 170.0 * rule(lattice(s, 30.0, 60.0));
    // Glyph-like blobs on a lattice, a hashed subset of the cells.
    let (ci, cj) = (((s - 7.0) / 14.0).round(), ((v - 30.0) / 40.0).round());
    let h =
        ((ci as i64).wrapping_mul(73_856_093) ^ (cj as i64).wrapping_mul(19_349_663)).rem_euclid(7);
    if blobs && h < 4 {
        let (dx, dy) = (s - (14.0 * ci + 7.0), v - (40.0 * cj + 30.0));
        val -= 90.0 * (-(dx * dx + dy * dy) / (2.0 * 2.0 * 2.0)).exp();
    }
    val.clamp(0.0, 255.0)
}

/// A page edge: the analytic top curve `y(p)` over `x(p) = x0 + p * (x1 - x0)`, a constant height
/// `h` (the bottom edge is the top edge shifted down), and a rotation about the page centre.
struct Generator {
    x0: f64,
    x1: f64,
    h: f64,
    y: fn(f64) -> f64,
    theta: f64,
    /// Cumulative arc length of the top edge at `N + 1` values of `p`.
    arc: Vec<f64>,
}

const DENSE: usize = 40_000;

impl Generator {
    fn new(x0: f64, x1: f64, h: f64, y: fn(f64) -> f64, theta_deg: f64) -> Self {
        let mut arc = Vec::with_capacity(DENSE + 1);
        let mut total = 0.0;
        let mut prev = (x0, y(0.0));
        arc.push(0.0);
        for i in 1..=DENSE {
            let p = i as f64 / DENSE as f64;
            let cur = (x0 + p * (x1 - x0), y(p));
            total += (cur.0 - prev.0).hypot(cur.1 - prev.1);
            arc.push(total);
            prev = cur;
        }
        Self {
            x0,
            x1,
            h,
            y,
            theta: theta_deg.to_radians(),
            arc,
        }
    }

    fn top_length(&self) -> f64 {
        self.arc[DENSE]
    }

    /// Arc length (page `s`) at parameter `p`.
    fn s_of_p(&self, p: f64) -> f64 {
        let f = (p * DENSE as f64).clamp(0.0, DENSE as f64);
        let i = (f.floor() as usize).min(DENSE - 1);
        self.arc[i] + (f - i as f64) * (self.arc[i + 1] - self.arc[i])
    }

    /// Parameter `p` at page position `s` (bisection on the monotone table).
    fn p_of_s(&self, s: f64) -> f64 {
        let (mut lo, mut hi) = (0usize, DENSE);
        while hi - lo > 1 {
            let mid = (lo + hi) / 2;
            if self.arc[mid] <= s {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let span = self.arc[hi] - self.arc[lo];
        let f = if span > 0.0 {
            (s - self.arc[lo]) / span
        } else {
            0.0
        };
        (lo as f64 + f) / DENSE as f64
    }

    fn centre(&self) -> (f64, f64) {
        ((self.x0 + self.x1) / 2.0, ((self.y)(0.5) + self.h / 2.0))
    }

    /// Local (unrotated) point of the top edge at `p`, shifted down by `v`.
    fn local(&self, p: f64, v: f64) -> (f64, f64) {
        (self.x0 + p * (self.x1 - self.x0), (self.y)(p) + v)
    }

    fn rotate(&self, (x, y): (f64, f64)) -> (f64, f64) {
        let (cx, cy) = self.centre();
        let (s, k) = self.theta.sin_cos();
        (
            cx + (x - cx) * k - (y - cy) * s,
            cy + (x - cx) * s + (y - cy) * k,
        )
    }

    fn unrotate(&self, (x, y): (f64, f64)) -> (f64, f64) {
        let (cx, cy) = self.centre();
        let (s, k) = self.theta.sin_cos();
        (
            cx + (x - cx) * k + (y - cy) * s,
            cy - (x - cx) * s + (y - cy) * k,
        )
    }

    /// Page position `(s, v)` of an image point, if it lies on the page.
    fn inverse(&self, pt: (f64, f64)) -> Option<(f64, f64)> {
        let (xl, yl) = self.unrotate(pt);
        let p = (xl - self.x0) / (self.x1 - self.x0);
        if !(0.0..=1.0).contains(&p) {
            return None;
        }
        let v = yl - (self.y)(p);
        (0.0..=self.h).contains(&v).then(|| (self.s_of_p(p), v))
    }

    /// The photographed page: 2 x 2 supersampling, the surround is dark grey.
    fn photograph(&self, w: u32, h: u32, blobs: bool) -> Raster {
        self.photograph_through(w, h, blobs, Some)
    }

    /// [`Generator::photograph`] seen through a camera: `to_ortho` maps an image point back to the
    /// point of the orthographic picture of the bent page (`None` where it sees nothing).
    fn photograph_through(
        &self,
        w: u32,
        h: u32,
        blobs: bool,
        to_ortho: impl Fn((f64, f64)) -> Option<(f64, f64)>,
    ) -> Raster {
        let mut r = Raster::new(w, h);
        for iy in 0..h {
            for ix in 0..w {
                let mut acc = 0.0;
                for (a, b) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                    let pt = (f64::from(ix) + a, f64::from(iy) + b);
                    acc += to_ortho(pt)
                        .and_then(|o| self.inverse(o))
                        .map_or(70.0, |(s, v)| page_with(s, v, blobs));
                }
                let g = (acc / 4.0).round() as u8;
                r.set_pixel(ix, iy, [g, g, g]);
            }
        }
        r
    }

    /// The boundary curves a person (or a detector) would mark: `n` points along the top and
    /// bottom edges at equal arc length, straight sides, normalised to a `w` x `h` image.
    fn curves(&self, n: usize, w: u32, h: u32) -> CurveWarp {
        let norm = |(x, y): (f64, f64)| Pt::new(x / f64::from(w), y / f64::from(h));
        let top: Vec<Pt> = (0..n)
            .map(|i| {
                let s = self.top_length() * i as f64 / (n - 1) as f64;
                norm(self.rotate(self.local(self.p_of_s(s), 0.0)))
            })
            .collect();
        let bottom: Vec<Pt> = (0..n)
            .rev()
            .map(|i| {
                let s = self.top_length() * i as f64 / (n - 1) as f64;
                norm(self.rotate(self.local(self.p_of_s(s), self.h)))
            })
            .collect();
        let (tl, tr, br, bl) = (top[0], top[n - 1], bottom[0], bottom[n - 1]);
        CurveWarp {
            top: Curve::new(top).unwrap(),
            right: Curve::new(vec![tr, br]).unwrap(),
            bottom: Curve::new(bottom).unwrap(),
            left: Curve::new(vec![bl, tl]).unwrap(),
            quarter_turns: 0,
            mirror: false,
        }
    }
}

fn gray(r: &Raster) -> Vec<f64> {
    r.data.chunks(3).map(|p| f64::from(p[1])).collect()
}

/// Mean SSIM over 8 x 8 windows (stride 4) of the interior, `margin` pixels in from the edges.
fn ssim(a: &[f64], b: &[f64], w: usize, h: usize, margin: usize) -> f64 {
    let (c1, c2) = ((0.01f64 * 255.0).powi(2), (0.03f64 * 255.0).powi(2));
    let (mut sum, mut n) = (0.0, 0usize);
    let mut y = margin;
    while y + 8 <= h - margin {
        let mut x = margin;
        while x + 8 <= w - margin {
            let (mut ma, mut mb) = (0.0, 0.0);
            for j in 0..8 {
                for i in 0..8 {
                    ma += a[(y + j) * w + x + i];
                    mb += b[(y + j) * w + x + i];
                }
            }
            ma /= 64.0;
            mb /= 64.0;
            let (mut va, mut vb, mut cov) = (0.0, 0.0, 0.0);
            for j in 0..8 {
                for i in 0..8 {
                    let (da, db) = (a[(y + j) * w + x + i] - ma, b[(y + j) * w + x + i] - mb);
                    va += da * da;
                    vb += db * db;
                    cov += da * db;
                }
            }
            va /= 63.0;
            vb /= 63.0;
            cov /= 63.0;
            sum += ((2.0 * ma * mb + c1) * (2.0 * cov + c2))
                / ((ma * ma + mb * mb + c1) * (va + vb + c2));
            n += 1;
            x += 4;
        }
        y += 4;
    }
    sum / n as f64
}

/// Is `x` within `tol` of a lattice line `off + step * k`?
fn near_lattice(x: f64, off: f64, step: f64, tol: f64) -> bool {
    (x - off - step * ((x - off) / step).round()).abs() < tol
}

/// Position of the printed rules in the flattened output. For every horizontal rule, the centre row
/// of the dark line per column (weighted centroid in a +-7 px window); for every vertical rule, the
/// centre column per row. Returns `(straightness, position error)` in output pixels: the largest
/// deviation of a rule from its own best-fit line, and the largest deviation from where the flat
/// page puts it.
fn rule_residuals(out: &Raster, page_w: f64, page_h: f64, margin: usize) -> (f64, f64) {
    rule_residuals_in(out, page_w, page_h, margin, 7)
}

/// [`rule_residuals`] looking `half` pixels either side of where each rule should be.
fn rule_residuals_in(
    out: &Raster,
    page_w: f64,
    page_h: f64,
    margin: usize,
    half: usize,
) -> (f64, f64) {
    let half_f = half as f64;
    let (w, h) = (out.width as usize, out.height as usize);
    let g = gray(out);
    let (sx, sy) = (w as f64 / page_w, h as f64 / page_h);
    let mut straight: f64 = 0.0;
    let mut position: f64 = 0.0;
    let centroid = |samples: &mut dyn Iterator<Item = (f64, f64)>| -> Option<f64> {
        let (mut num, mut den) = (0.0, 0.0);
        for (pos, v) in samples {
            let wgt = (238.0 - v).max(0.0);
            num += wgt * pos;
            den += wgt;
        }
        (den > 40.0).then(|| num / den)
    };
    let fit = |pts: &[(f64, f64)]| -> (f64, f64) {
        let n = pts.len() as f64;
        let (sx, sy) = pts.iter().fold((0.0, 0.0), |a, p| (a.0 + p.0, a.1 + p.1));
        let (mx, my) = (sx / n, sy / n);
        let (mut sxx, mut sxy) = (0.0, 0.0);
        for p in pts {
            sxx += (p.0 - mx) * (p.0 - mx);
            sxy += (p.0 - mx) * (p.1 - my);
        }
        let k = sxy / sxx;
        (k, my - k * mx)
    };
    // Horizontal rules at page v = 20 + 40 k (skip those within the margin of the border).
    let mut k = 0;
    loop {
        let vc = 20.0 + 40.0 * f64::from(k);
        k += 1;
        let yc = vc * sy; // output row coordinate (edge based); pixel centres at +0.5
        if yc + half_f + 1.0 > (h - margin) as f64 {
            break;
        }
        if yc - half_f - 1.0 < margin as f64 {
            continue;
        }
        let mut pts = Vec::new();
        for x in margin..w - margin {
            // Not where a vertical rule crosses (it darkens the whole window).
            if near_lattice((x as f64 + 0.5) / sx, 30.0, 60.0, 6.0 / sx) {
                continue;
            }
            let lo = (yc - half_f).floor() as usize;
            let mut it = (lo..lo + 2 * half + 1).map(|y| (y as f64 + 0.5, g[y * w + x]));
            if let Some(c) = centroid(&mut it) {
                pts.push((x as f64 + 0.5, c));
            }
        }
        assert!(pts.len() > 20, "rule at v={vc} was not found");
        let (slope, icpt) = fit(&pts);
        for (x, y) in &pts {
            straight = straight.max((y - (slope * x + icpt)).abs());
            position = position.max((y - yc).abs());
        }
    }
    // Vertical rules at page s = 30 + 60 k.
    let mut k = 0;
    loop {
        let sc = 30.0 + 60.0 * f64::from(k);
        k += 1;
        let xc = sc * sx;
        if xc + half_f + 1.0 > (w - margin) as f64 {
            break;
        }
        if xc - half_f - 1.0 < margin as f64 {
            continue;
        }
        let mut pts = Vec::new();
        for y in margin..h - margin {
            if near_lattice((y as f64 + 0.5) / sy, 20.0, 40.0, 6.0 / sy) {
                continue;
            }
            let lo = (xc - half_f).floor() as usize;
            let mut it = (lo..lo + 2 * half + 1).map(|x| (x as f64 + 0.5, g[y * w + x]));
            if let Some(c) = centroid(&mut it) {
                pts.push((y as f64 + 0.5, c));
            }
        }
        assert!(pts.len() > 20, "rule at s={sc} was not found");
        let (slope, icpt) = fit(&pts);
        for (y, x) in &pts {
            straight = straight.max((x - (slope * y + icpt)).abs());
            position = position.max((x - xc).abs());
        }
    }
    (straight, position)
}

struct Outcome {
    ssim: f64,
    straight: f64,
    position: f64,
    out_size: (u32, u32),
}

/// Photographs the page, flattens it from the boundary curves and measures against the flat page.
fn flatten_and_measure(g: &Generator, points: usize) -> Outcome {
    let (iw, ih) = (1000u32, 720u32);
    let photo = g.photograph(iw, ih, true);
    let rules_photo = g.photograph(iw, ih, false);
    let curves = g.curves(points, iw, ih);
    assert_eq!(curves.validate(), Ok(()));
    let out = render_curved(&photo, &curves, UNLIMITED).unwrap();
    let rules_out = render_curved(&rules_photo, &curves, UNLIMITED).unwrap();
    let (w, h) = (out.width as usize, out.height as usize);
    // The ideal flat page at the output's own resolution.
    let (pw, ph) = (g.top_length(), g.h);
    let mut ideal = Raster::new(out.width, out.height);
    for y in 0..h {
        for x in 0..w {
            let v = page(
                (x as f64 + 0.5) / w as f64 * pw,
                (y as f64 + 0.5) / h as f64 * ph,
            );
            let b = v.round() as u8;
            ideal.set_pixel(x as u32, y as u32, [b, b, b]);
        }
    }
    let margin = 10;
    let s = ssim(&gray(&out), &gray(&ideal), w, h, margin);
    let (straight, position) = rule_residuals(&rules_out, pw, ph, margin);
    Outcome {
        ssim: s,
        straight,
        position,
        out_size: (out.width, out.height),
    }
}

fn cylinder_y(p: f64) -> f64 {
    // A page bowed like a cylinder seen with its axis horizontal: a parabolic sag of 44 px.
    120.0 + 44.0 * 4.0 * p * (1.0 - p)
}

fn wave_y(p: f64) -> f64 {
    140.0 + 24.0 * (2.0 * PI * 1.5 * p + 0.4).sin()
}

#[test]
fn a_cylindrically_bowed_page_is_flattened_back_to_the_flat_page() {
    let g = Generator::new(120.0, 880.0, 470.0, cylinder_y, 0.0);
    let o = flatten_and_measure(&g, 9);
    println!(
        "oracle (b) cylinder: out {:?}, SSIM {:.4}, straightness residual {:.2} px, position error {:.2} px",
        o.out_size, o.ssim, o.straight, o.position
    );
    assert!(o.ssim >= 0.97, "SSIM {}", o.ssim);
    assert!(o.straight < 1.5, "straightness {}", o.straight);
}

#[test]
fn a_waving_tilted_page_is_flattened_back_to_the_flat_page() {
    let g = Generator::new(130.0, 870.0, 440.0, wave_y, 6.0);
    let o = flatten_and_measure(&g, 13);
    println!(
        "oracle (b) wave+tilt: out {:?}, SSIM {:.4}, straightness residual {:.2} px, position error {:.2} px",
        o.out_size, o.ssim, o.straight, o.position
    );
    assert!(o.ssim >= 0.97, "SSIM {}", o.ssim);
    assert!(o.straight < 1.5, "straightness {}", o.straight);
}

#[test]
fn a_bowed_page_photographed_at_an_angle_is_flattened_through_the_corner_homography() {
    use auto_crop_imgproc::geometry::{apply, invert};
    use auto_crop_imgproc::homography::Homography;
    let g = Generator::new(120.0, 880.0, 470.0, cylinder_y, 0.0);
    // The camera: the orthographic picture (1000 x 720) seen as a keystone quad in a 1100 x 800
    // image (about 7% narrower at the bottom, tilted sideways a little).
    let (ow, oh) = (1000.0, 720.0);
    let (iw, ih) = (1100u32, 800u32);
    let quad = [(70.0, 40.0), (1040.0, 85.0), (985.0, 760.0), (115.0, 715.0)];
    let cam = Homography::from_quads([(0.0, 0.0), (ow, 0.0), (ow, oh), (0.0, oh)], quad).unwrap();
    let inv = invert(&cam.0).unwrap();
    let to_ortho = |p: (f64, f64)| apply(&inv, p.0, p.1);
    let photo = g.photograph_through(iw, ih, true, to_ortho);
    let rules = g.photograph_through(iw, ih, false, to_ortho);
    // The edges as a person marks them in the photo: the orthographic curves seen by the camera.
    let ortho = g.curves(11, 1000, 720);
    let norm = |c: &Curve| {
        Curve::new(
            c.points()
                .iter()
                .map(|p| {
                    let (x, y) = apply(&cam.0, p.x * ow, p.y * oh).unwrap();
                    Pt::new(x / f64::from(iw), y / f64::from(ih))
                })
                .collect(),
        )
        .unwrap()
    };
    let curves = CurveWarp {
        top: norm(&ortho.top),
        right: norm(&ortho.right),
        bottom: norm(&ortho.bottom),
        left: norm(&ortho.left),
        quarter_turns: 0,
        mirror: false,
    };
    assert_eq!(curves.validate(), Ok(()));
    let out = render_curved(&photo, &curves, UNLIMITED).unwrap();
    let rules_out = render_curved(&rules, &curves, UNLIMITED).unwrap();
    let (w, h) = (out.width as usize, out.height as usize);
    let (pw, ph) = (g.top_length(), g.h);
    let mut ideal = Raster::new(out.width, out.height);
    for y in 0..h {
        for x in 0..w {
            let b = page(
                (x as f64 + 0.5) / w as f64 * pw,
                (y as f64 + 0.5) / h as f64 * ph,
            )
            .round() as u8;
            ideal.set_pixel(x as u32, y as u32, [b, b, b]);
        }
    }
    let s = ssim(&gray(&out), &gray(&ideal), w, h, 10);
    let (straight, position) = rule_residuals(&rules_out, pw, ph, 10);
    println!(
        "oracle (d) bowed page + perspective: out {w} x {h}, SSIM {s:.4}, straightness {straight:.2} px, position error {position:.2} px"
    );
    assert!(s >= 0.97, "SSIM {s}");
    assert!(straight < 1.5, "straightness {straight}");
}

#[test]
fn fewer_control_points_cost_accuracy_in_a_measured_way() {
    // The boundary model is only as good as the points: 3 points cannot follow 1.5 sine periods.
    let g = Generator::new(130.0, 870.0, 440.0, wave_y, 0.0);
    let good = flatten_and_measure(&g, 13);
    let coarse = flatten_and_measure(&g, 3);
    println!(
        "oracle (b) wave: 13 points SSIM {:.4} straightness {:.2} px; 3 points SSIM {:.4} straightness {:.2} px",
        good.ssim, good.straight, coarse.ssim, coarse.straight
    );
    assert!(coarse.ssim < good.ssim && coarse.straight > good.straight);
}

// ------------------------------------------------------------------------------------ (c) limits

#[test]
fn a_bend_the_edges_do_not_show_is_not_corrected_and_perspective_alone_is_the_homography() {
    // (1) A page bent about a vertical axis (a book page open at the spine), photographed square on:
    // image x = R sin(s / R). Its edges are straight, so the boundary model has nothing to say and
    // the flat result keeps the compression: the vertical rules are NOT equally spaced.
    let (iw, ih) = (1000u32, 720u32);
    let (pw, ph) = (760.0f64, 470.0f64);
    let radius = pw / 2.2; // a bend of ~126 degrees across the page
    let x_of_s = |s: f64| 500.0 + radius * ((s - pw / 2.0) / radius).sin();
    let span = x_of_s(pw) - x_of_s(0.0);
    let bent = |blobs: bool| {
        let mut photo = Raster::new(iw, ih);
        for iy in 0..ih {
            for ix in 0..iw {
                let (x, y) = (f64::from(ix) + 0.5, f64::from(iy) + 0.5);
                // Invert x(s) by bisection.
                let v = y - 125.0;
                let val = if (0.0..=ph).contains(&v) && x >= x_of_s(0.0) && x <= x_of_s(pw) {
                    let (mut lo, mut hi) = (0.0, pw);
                    for _ in 0..40 {
                        let mid = (lo + hi) / 2.0;
                        if x_of_s(mid) < x {
                            lo = mid;
                        } else {
                            hi = mid;
                        }
                    }
                    page_with((lo + hi) / 2.0, v, blobs)
                } else {
                    70.0
                };
                let b = val.round() as u8;
                photo.set_pixel(ix, iy, [b, b, b]);
            }
        }
        photo
    };
    let (photo, rules_photo) = (bent(true), bent(false));
    let (x0, x1) = (x_of_s(0.0) / f64::from(iw), x_of_s(pw) / f64::from(iw));
    let (y0, y1) = (125.0 / f64::from(ih), (125.0 + ph) / f64::from(ih));
    let straight = CurveWarp::from_quad(&QuadWarp::new([
        Pt::new(x0, y0),
        Pt::new(x1, y0),
        Pt::new(x1, y1),
        Pt::new(x0, y1),
    ]));
    let out = render_curved(&photo, &straight, UNLIMITED).unwrap();
    let rules_out = render_curved(&rules_photo, &straight, UNLIMITED).unwrap();
    let (w, h) = (out.width as usize, out.height as usize);
    let mut ideal = Raster::new(out.width, out.height);
    for y in 0..h {
        for x in 0..w {
            let b = page(
                (x as f64 + 0.5) / w as f64 * pw,
                (y as f64 + 0.5) / h as f64 * ph,
            )
            .round() as u8;
            ideal.set_pixel(x as u32, y as u32, [b, b, b]);
        }
    }
    let s = ssim(&gray(&out), &gray(&ideal), w, h, 10);
    let (_, position) = rule_residuals_in(&rules_out, pw, ph, 10, 26);
    println!(
        "limit: bend about a vertical axis (edges straight, span {span:.0} px for a {pw:.0} px page): SSIM {s:.3}, rule position error {position:.1} px"
    );
    assert!(
        s < 0.97,
        "the model cannot fix this, so the test must not pass silently: {s}"
    );
    assert!(position > 8.0, "{position}");

    // (2) Perspective alone: a flat page photographed at an angle. The curved path is composed with
    // the corner homography, so four straight edges give the homography crop (a bare bilinear
    // patch would not: it scored SSIM 0.57 at a 5% keystone, see docs/dev/curved-pages.md).
    let (pw, ph) = (700.0f64, 480.0f64);
    // Forward homography: page (s, v) to image, a keystone: the bottom edge is `k` shorter than the
    // top one (5% is a casual hand-held tilt, 20% a steep one).
    let keystone = |k: f64| -> (f64, f64, u8) {
        let inset = 350.0 * k;
        let corners = [
            (150.0, 110.0),
            (850.0, 110.0),
            (850.0 - inset, 640.0),
            (150.0 + inset, 640.0),
        ];
        let hm = {
            use auto_crop_imgproc::homography::Homography;
            Homography::from_quads([(0.0, 0.0), (pw, 0.0), (pw, ph), (0.0, ph)], corners).unwrap()
        };
        let inv = auto_crop_imgproc::geometry::invert(&hm.0).unwrap();
        let mut photo = Raster::new(iw, ih);
        for iy in 0..ih {
            for ix in 0..iw {
                let (s, v) = auto_crop_imgproc::geometry::apply(
                    &inv,
                    f64::from(ix) + 0.5,
                    f64::from(iy) + 0.5,
                )
                .unwrap();
                let val = if (0.0..=pw).contains(&s) && (0.0..=ph).contains(&v) {
                    page(s, v)
                } else {
                    70.0
                };
                let b = val.round() as u8;
                photo.set_pixel(ix, iy, [b, b, b]);
            }
        }
        let norm = |(x, y): (f64, f64)| Pt::new(x / f64::from(iw), y / f64::from(ih));
        let q = QuadWarp::new(corners.map(norm));
        let against_flat = |out: &Raster| {
            let (w, h) = (out.width as usize, out.height as usize);
            let mut ideal = Raster::new(out.width, out.height);
            for y in 0..h {
                for x in 0..w {
                    let b = page(
                        (x as f64 + 0.5) / w as f64 * pw,
                        (y as f64 + 0.5) / h as f64 * ph,
                    )
                    .round() as u8;
                    ideal.set_pixel(x as u32, y as u32, [b, b, b]);
                }
            }
            ssim(&gray(out), &gray(&ideal), w, h, 10)
        };
        let coons = render_curved(&photo, &CurveWarp::from_quad(&q), UNLIMITED).unwrap();
        let hom = render_quad(&photo, &q, UNLIMITED).unwrap();
        (
            against_flat(&coons),
            against_flat(&hom),
            max_diff(&coons, &hom),
        )
    };
    for k in [0.05, 0.10, 0.20] {
        let (s_coons, s_hom, diff) = keystone(k);
        println!(
            "perspective keystone {:.0}%, SSIM against the flat page: homography {s_hom:.3}, Coons path {s_coons:.3}, pixel difference {diff} LSB",
            k * 100.0
        );
        assert!(s_hom > 0.9, "the homography path is the reference: {s_hom}");
        // Composed with the corner homography, four straight edges ARE the homography crop.
        assert!(diff <= 1, "{diff} LSB at {k}");
    }
}

// ------------------------------------------------------------------------------------ plumbing

fn bulging_page() -> CurveWarp {
    let c = |p: &[(f64, f64)]| Curve::new(p.iter().map(|&(x, y)| Pt::new(x, y)).collect()).unwrap();
    CurveWarp {
        top: c(&[(0.10, 0.12), (0.35, 0.07), (0.65, 0.09), (0.90, 0.13)]),
        right: c(&[(0.90, 0.13), (0.93, 0.5), (0.89, 0.88)]),
        bottom: c(&[(0.89, 0.88), (0.6, 0.95), (0.3, 0.93), (0.11, 0.90)]),
        left: c(&[(0.11, 0.90), (0.07, 0.5), (0.10, 0.12)]),
        quarter_turns: 0,
        mirror: false,
    }
}

#[test]
fn the_dense_grid_warp_agrees_with_the_exact_one() {
    let src = textured(640, 480);
    let page = bulging_page();
    let exact = render_curved(&src, &page, UNLIMITED).unwrap();
    let (w, h) = (exact.width, exact.height);
    let nodes = coons_grid_px(src.width, src.height, &page, 41, 33).unwrap();
    let view = ImageRef {
        width: src.width,
        height: src.height,
        channels: 3,
        data: &src.data[..],
    };
    let grid = warp_grid_image(
        view,
        SrcGrid {
            cols: 41,
            rows: 33,
            nodes: &nodes,
        },
        w,
        h,
        &NeverCancel,
    )
    .expect("a valid grid")
    .unwrap();
    let d = exact
        .data
        .iter()
        .zip(&grid.data)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap();
    let mean = exact
        .data
        .iter()
        .zip(&grid.data)
        .map(|(a, b)| f64::from(a.abs_diff(*b)))
        .sum::<f64>()
        / exact.data.len() as f64;
    println!("dense grid 41 x 33 vs exact: max {d} LSB, mean {mean:.3} LSB");
    assert!(d <= 12 && mean < 0.5, "max {d}, mean {mean}");
    // A grid that is too small is refused, not rendered.
    let tiny = [(0.0, 0.0)];
    assert!(
        warp_grid_image(
            view,
            SrcGrid {
                cols: 1,
                rows: 1,
                nodes: &tiny
            },
            4,
            4,
            &NeverCancel
        )
        .is_none()
    );
}

#[test]
fn bytes_are_the_same_at_one_and_eight_threads() {
    let src = textured(500, 400);
    let mut page = bulging_page();
    page.quarter_turns = 3;
    page.mirror = true;
    let run = |n: usize| {
        pool(n)
            .install(|| render_curved(&src, &page, UNLIMITED))
            .unwrap()
    };
    let (a, b) = (run(1), run(8));
    assert_eq!(a, b);
    assert!(a.width > 100 && a.height > 100);
}

#[test]
fn other_channel_layouts_and_depths_render() {
    let page = bulging_page();
    let mut g16 = Image::<u16>::new(300, 220, 1);
    for (i, v) in g16.data.iter_mut().enumerate() {
        *v = ((i * 7919) % 65521) as u16;
    }
    let out = render_curved_image(g16.as_ref(), &page, UNLIMITED, &NeverCancel).unwrap();
    assert_eq!(out.channels, 1);
    assert!(out.data.iter().any(|v| *v > 1000));
    let rgba = Image::<u8>::new(120, 100, 4);
    let out = render_curved_image(rgba.as_ref(), &page, UNLIMITED, &NeverCancel).unwrap();
    assert_eq!(out.channels, 4);
    // An empty source is an all-zero page, not a panic.
    let empty = Image::<u8>::new(0, 0, 3);
    let _ = render_curved_image(empty.as_ref(), &page, UNLIMITED, &NeverCancel);
}

#[test]
fn outside_the_source_is_zero_like_the_quad_path() {
    let src = Raster::filled(100, 100, [200, 200, 200]);
    // A page that hangs off the right and bottom of the frame.
    let c = |p: &[(f64, f64)]| Curve::new(p.iter().map(|&(x, y)| Pt::new(x, y)).collect()).unwrap();
    let page = CurveWarp {
        top: c(&[(0.5, 0.5), (1.5, 0.5)]),
        right: c(&[(1.5, 0.5), (1.5, 1.5)]),
        bottom: c(&[(1.5, 1.5), (0.5, 1.5)]),
        left: c(&[(0.5, 1.5), (0.5, 0.5)]),
        quarter_turns: 0,
        mirror: false,
    };
    let out = render_curved(&src, &page, UNLIMITED).unwrap();
    assert_eq!((out.width, out.height), (100, 100));
    assert_eq!(out.pixel(10, 10), [200, 200, 200]);
    assert_eq!(out.pixel(90, 90), [0, 0, 0]);
}

#[test]
fn cancellation_polls_once_per_band_and_returns_no_image() {
    use auto_crop_imgproc::cancel::Cancel;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct After {
        limit: usize,
        polls: AtomicUsize,
    }
    impl Cancel for After {
        fn is_cancelled(&self) -> bool {
            self.polls.fetch_add(1, Ordering::SeqCst) >= self.limit
        }
    }
    let src = noise(200, 4000);
    let c = |p: &[(f64, f64)]| Curve::new(p.iter().map(|&(x, y)| Pt::new(x, y)).collect()).unwrap();
    let page = CurveWarp {
        top: c(&[(0.05, 0.02), (0.5, 0.01), (0.95, 0.02)]),
        right: c(&[(0.95, 0.02), (0.95, 0.98)]),
        bottom: c(&[(0.95, 0.98), (0.5, 0.99), (0.05, 0.98)]),
        left: c(&[(0.05, 0.98), (0.05, 0.02)]),
        quarter_turns: 0,
        mirror: false,
    };
    let (_, h) = output_size(src.width, src.height, &page, UNLIMITED).unwrap();
    assert!(h as usize > 20 * BAND_ROWS, "{h} rows");
    let token = After {
        limit: 3,
        polls: AtomicUsize::new(0),
    };
    let view = ImageRef {
        width: src.width,
        height: src.height,
        channels: 3,
        data: &src.data[..],
    };
    let r = pool(1).install(|| render_curved_image(view, &page, UNLIMITED, &token));
    assert_eq!(r.unwrap_err(), RenderError::Cancelled);
    assert!(
        token.polls.load(Ordering::SeqCst) <= 5,
        "stopped within a band of the flip"
    );
    let stop = std::sync::atomic::AtomicBool::new(true);
    assert_eq!(
        render_curved_cancellable(&src, &page, UNLIMITED, &stop),
        Err(RenderError::Cancelled)
    );
}

#[test]
fn the_output_size_follows_the_rectified_arc_lengths_and_the_caps() {
    // Straight edges: the size a plain quad crop gets, perspective or not.
    let n = |x: f64, y: f64| Pt::new(x / 1000.0, y / 800.0);
    let q = QuadWarp::new([
        n(120.0, 90.0),
        n(880.0, 70.0),
        n(840.0, 700.0),
        n(160.0, 740.0),
    ]);
    let want = auto_crop_imgproc::render::output_size(1000, 800, &q, UNLIMITED);
    assert_eq!(
        output_size(1000, 800, &CurveWarp::from_quad(&q), UNLIMITED),
        Some(want)
    );
    // A bulging page is longer than the straight quad through its corners, both ways.
    let page = bulging_page();
    let (w, h) = output_size(1000, 800, &page, UNLIMITED).unwrap();
    let straight =
        output_size(1000, 800, &CurveWarp::from_quad(&page.outline()), UNLIMITED).unwrap();
    assert!(w > straight.0 && h > straight.1, "{w}x{h} vs {straight:?}");
    // The cap only shrinks.
    let capped = output_size(
        1000,
        800,
        &page,
        Limits {
            max_pixels: 100_000,
            max_edge: u32::MAX,
        },
    )
    .unwrap();
    assert!(u64::from(capped.0) * u64::from(capped.1) <= 101_000);
    assert!(capped.0 < w);
    // A quarter turn swaps the sides.
    let mut turned = page.clone();
    turned.quarter_turns = 1;
    assert_eq!(output_size(1000, 800, &turned, UNLIMITED), Some((h, w)));
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(128))]

    /// Whatever pages the validator lets through (corners anywhere near the frame, any bow, any
    /// turns and mirror, pages that run past the frame), rendering never panics, respects the
    /// pixel cap and its own size, or says why not.
    #[test]
    fn any_validated_page_renders_within_its_limits_or_errors_cleanly(
        corners in proptest::collection::vec((-0.4f64..1.4, -0.4f64..1.4), 4),
        bows in proptest::collection::vec(-0.12f64..0.12, 12),
        extra in proptest::collection::vec(0usize..3, 4),
        turns in 0u8..8,
        mirror in proptest::bool::ANY,
    ) {
        let c: Vec<Pt> = corners.iter().map(|&(x, y)| Pt::new(x, y)).collect();
        let edge = |a: Pt, b: Pt, k: usize| -> Curve {
            let mut pts = vec![a];
            for i in 0..extra[k] {
                let t = (i + 1) as f64 / (extra[k] + 1) as f64;
                let (dx, dy) = (b.x - a.x, b.y - a.y);
                let off = bows[k * 3 + i];
                pts.push(Pt::new(a.x + dx * t - dy * off, a.y + dy * t + dx * off));
            }
            pts.push(b);
            Curve::new(pts).unwrap()
        };
        let page = CurveWarp {
            top: edge(c[0], c[1], 0),
            right: edge(c[1], c[2], 1),
            bottom: edge(c[2], c[3], 2),
            left: edge(c[3], c[0], 3),
            quarter_turns: turns,
            mirror,
        };
        let src = textured(80, 60);
        let cap = 200_000u64;
        let limits = Limits::pixels(cap);
        let r = render_curved(&src, &page, limits);
        if page.validate().is_err() {
            proptest::prop_assert_eq!(r, Err(RenderError::DegenerateQuad));
        } else {
            match r {
                Ok(out) => {
                    proptest::prop_assert_eq!(
                        Some((out.width, out.height)),
                        output_size(80, 60, &page, limits)
                    );
                    // Each side rounds to the nearest pixel, so a sliver can overshoot the cap by a little.
                    proptest::prop_assert!(u64::from(out.width) * u64::from(out.height) <= cap * 2);
                    proptest::prop_assert_eq!(out.data.len(), out.width as usize * out.height as usize * 3);
                }
                Err(e) => proptest::prop_assert_eq!(e, RenderError::DegenerateQuad),
            }
        }
    }
}
