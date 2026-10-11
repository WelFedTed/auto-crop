// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Synthetic receipt and document photos with known corners: the "Try sample images" set and the
//! detector's accuracy tests. Deterministic for a given seed. Contains no private data (B21).

use crate::Raster;
use crate::geometry::{apply, homography};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaperKind {
    Receipt,
    Document,
}

/// One synthetic photo. Corners are normalised (0..1 of the scene) and may lie outside the frame.
#[derive(Debug, Clone)]
pub struct Scene {
    pub width: u32,
    pub height: u32,
    pub background: [u8; 3],
    pub paper: [u8; 3],
    pub ink: [u8; 3],
    pub kind: PaperKind,
    /// TL, TR, BR, BL of the paper.
    pub corners: [(f64, f64); 4],
    pub seed: u64,
    /// Noise amplitude in grey levels (uniform sum, roughly gaussian).
    pub noise: f32,
    /// Box-blur radius in pixels (0 = sharp).
    pub blur_radius: u32,
    pub shadow: bool,
}

/// xorshift64*: small, deterministic, good enough for texture.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32) as u32
    }

    /// Uniform in `[0, 1)`.
    pub fn unit(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }

    pub fn below(&mut self, n: u32) -> u32 {
        self.next_u32() % n.max(1)
    }
}

fn fill_rect(r: &mut Raster, x0: i64, y0: i64, w: i64, h: i64, rgb: [u8; 3]) {
    for y in y0.max(0)..(y0 + h).min(i64::from(r.height)) {
        for x in x0.max(0)..(x0 + w).min(i64::from(r.width)) {
            r.set_pixel(x as u32, y as u32, rgb);
        }
    }
}

/// An upright paper with text-like content.
pub fn paper_texture(kind: PaperKind, paper: [u8; 3], ink: [u8; 3], seed: u64) -> Raster {
    let mut rng = Rng::new(seed);
    let (w, h) = match kind {
        PaperKind::Receipt => (420u32, 1300u32),
        PaperKind::Document => (1000, 1400),
    };
    let mut r = Raster::filled(w, h, paper);
    let (wi, hi) = (i64::from(w), i64::from(h));
    let margin = wi / 11;
    match kind {
        PaperKind::Receipt => {
            // Header, address lines, item rows with prices, a rule, a total and a barcode.
            fill_rect(&mut r, wi / 4, 60, wi / 2, 34, ink);
            for i in 0..3 {
                let len = wi / 3 + rng.below(80) as i64;
                fill_rect(&mut r, (wi - len) / 2, 120 + i * 22, len, 8, ink);
            }
            let mut y = 250;
            while y < hi - 330 {
                let name = 90 + rng.below(160) as i64;
                fill_rect(&mut r, margin, y, name, 10, ink);
                fill_rect(&mut r, wi - margin - 60, y, 60, 10, ink);
                y += 30;
            }
            fill_rect(&mut r, margin, y + 10, wi - 2 * margin, 3, ink);
            fill_rect(&mut r, margin, y + 40, 130, 16, ink);
            fill_rect(&mut r, wi - margin - 90, y + 40, 90, 16, ink);
            let mut bx = margin;
            let by = y + 110;
            while bx < wi - margin - 4 {
                let bw = 2 + rng.below(5) as i64;
                fill_rect(&mut r, bx, by, bw, 90, ink);
                bx += bw + 2 + rng.below(4) as i64;
            }
        }
        PaperKind::Document => {
            fill_rect(&mut r, margin, 90, wi / 2, 40, ink);
            fill_rect(&mut r, margin, 150, wi / 3, 10, ink);
            let mut y = 230;
            while y < hi - 180 {
                if (y / 20) % 17 == 0 {
                    // an image-like block
                    let bh = 160;
                    fill_rect(
                        &mut r,
                        margin,
                        y,
                        wi - 2 * margin,
                        bh,
                        [ink[0] / 2 + 120, ink[1] / 2 + 120, ink[2] / 2 + 120],
                    );
                    y += bh + 30;
                    continue;
                }
                let lines = 3 + rng.below(4) as i64;
                for l in 0..lines {
                    let full = wi - 2 * margin;
                    let len = if l == lines - 1 {
                        full / 3 + rng.below(200) as i64
                    } else {
                        full - rng.below(30) as i64
                    };
                    fill_rect(&mut r, margin, y, len.min(full), 9, ink);
                    y += 22;
                }
                y += 26;
            }
        }
    }
    r
}

fn bilinear(r: &Raster, x: f64, y: f64) -> [f32; 3] {
    let (w, h) = (r.width as i64, r.height as i64);
    let (x0, y0) = (x.floor() as i64, y.floor() as i64);
    let (fx, fy) = ((x - x0 as f64) as f32, (y - y0 as f64) as f32);
    let at = |xx: i64, yy: i64| {
        let p = r.pixel(xx.clamp(0, w - 1) as u32, yy.clamp(0, h - 1) as u32);
        [f32::from(p[0]), f32::from(p[1]), f32::from(p[2])]
    };
    let (a, b, c, d) = (
        at(x0, y0),
        at(x0 + 1, y0),
        at(x0, y0 + 1),
        at(x0 + 1, y0 + 1),
    );
    let mut out = [0.0; 3];
    for i in 0..3 {
        out[i] =
            (a[i] * (1.0 - fx) + b[i] * fx) * (1.0 - fy) + (c[i] * (1.0 - fx) + d[i] * fx) * fy;
    }
    out
}

fn box_blur(r: &Raster, radius: u32) -> Raster {
    if radius == 0 {
        return r.clone();
    }
    let (w, h, rad) = (r.width as usize, r.height as usize, radius as usize);
    let norm = (2 * rad + 1) as f32;
    let mut tmp = vec![0.0f32; w * h * 3];
    for y in 0..h {
        for c in 0..3 {
            let mut acc = 0.0f32;
            for k in 0..=2 * rad {
                let x = k.saturating_sub(rad).min(w - 1);
                acc += f32::from(r.data[(y * w + x) * 3 + c]);
            }
            for x in 0..w {
                tmp[(y * w + x) * 3 + c] = acc / norm;
                let add = (x + rad + 1).min(w - 1);
                let sub = x.saturating_sub(rad);
                acc += f32::from(r.data[(y * w + add) * 3 + c])
                    - f32::from(r.data[(y * w + sub) * 3 + c]);
            }
        }
    }
    let mut out = Raster::new(r.width, r.height);
    for x in 0..w {
        for c in 0..3 {
            let mut acc = 0.0f32;
            for k in 0..=2 * rad {
                let y = k.saturating_sub(rad).min(h - 1);
                acc += tmp[(y * w + x) * 3 + c];
            }
            for y in 0..h {
                out.data[(y * w + x) * 3 + c] = (acc / norm + 0.5).clamp(0.0, 255.0) as u8;
                let add = (y + rad + 1).min(h - 1);
                let sub = y.saturating_sub(rad);
                acc += tmp[(add * w + x) * 3 + c] - tmp[(sub * w + x) * 3 + c];
            }
        }
    }
    out
}

/// Renders the scene: a wood-ish desk with the paper laid on it through the given corners.
pub fn render_scene(s: &Scene) -> Raster {
    let paper = paper_texture(s.kind, s.paper, s.ink, s.seed);
    let (pw, ph) = (f64::from(paper.width), f64::from(paper.height));
    let (w, h) = (f64::from(s.width), f64::from(s.height));
    let quad: [(f64, f64); 4] = std::array::from_fn(|i| (s.corners[i].0 * w, s.corners[i].1 * h));
    let rect = [(0.0, 0.0), (pw, 0.0), (pw, ph), (0.0, ph)];
    let to_paper = homography(quad, rect).expect("non-degenerate scene quad");
    let mut rng = Rng::new(s.seed ^ 0xDE5C);
    let mut img = Raster::new(s.width, s.height);
    for y in 0..s.height {
        for x in 0..s.width {
            let (fx, fy) = (f64::from(x), f64::from(y));
            // Desk: gradient plus faint grain stripes.
            let t = ((fx + fy) / (w + h)) as f32;
            let grain =
                ((fy * 0.35).sin() as f32) * 4.0 + ((fx * 0.05 + fy * 0.02).sin() as f32) * 6.0;
            let mut px = [0.0f32; 3];
            for (p, bg) in px.iter_mut().zip(s.background) {
                *p = f32::from(bg) * (1.1 - 0.25 * t) + grain;
            }
            if let Some((u, v)) = apply(&to_paper, fx, fy) {
                let (du, dv) = ((-u).max(u - pw).max(0.0), (-v).max(v - ph).max(0.0));
                let outside = du.hypot(dv);
                if outside == 0.0 {
                    px = bilinear(&paper, u.clamp(0.0, pw - 1.0), v.clamp(0.0, ph - 1.0));
                } else if s.shadow && outside < 90.0 {
                    let k = 1.0 - 0.30 * (-outside / 28.0).exp() as f32;
                    for p in &mut px {
                        *p *= k;
                    }
                }
            }
            let i = (y as usize * s.width as usize + x as usize) * 3;
            for (c, p) in px.iter().enumerate() {
                let n = if s.noise > 0.0 {
                    (rng.unit() + rng.unit() + rng.unit() - 1.5) * s.noise
                } else {
                    0.0
                };
                img.data[i + c] = (p + n).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    box_blur(&img, s.blur_radius)
}

/// Normalised corners TL, TR, BR, BL.
pub type SceneCorners = [(f64, f64); 4];

/// A sheet of `kind` laid through the normalised `corners` on a desk given by `desk(x, y)` (grey
/// levels per channel, before noise), with a mild lighting gradient across the sheet and `noise`
/// grey levels of noise everywhere. Unlike [`render_scene`] the desk is any function, so textures
/// such as [`wood_desk`] can be used; the corners may lie outside the frame (a sheet that fills it).
pub fn compose_on_desk(
    (w, h): (u32, u32),
    corners: [(f64, f64); 4],
    kind: PaperKind,
    paper: [u8; 3],
    seed: u64,
    noise: f32,
    desk: &dyn Fn(u32, u32) -> [f32; 3],
) -> Raster {
    let sheet = paper_texture(kind, paper, [58, 62, 74], seed);
    let (pw, ph) = (f64::from(sheet.width), f64::from(sheet.height));
    let quad: [(f64, f64); 4] =
        std::array::from_fn(|i| (corners[i].0 * f64::from(w), corners[i].1 * f64::from(h)));
    let rect = [(0.0, 0.0), (pw, 0.0), (pw, ph), (0.0, ph)];
    let to_paper = homography(quad, rect).expect("a plain quad");
    let mut rng = Rng::new(seed ^ 0xFACE);
    let mut img = Raster::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let (u, v) =
                apply(&to_paper, f64::from(x) + 0.5, f64::from(y) + 0.5).unwrap_or((-1.0, -1.0));
            let mut px = if u >= 0.0 && v >= 0.0 && u < pw && v < ph {
                let t = 1.0 - 0.06 * ((f64::from(x) / f64::from(w)) as f32);
                bilinear(&sheet, u, v).map(|c| c * t)
            } else {
                desk(x, y)
            };
            for c in &mut px {
                *c += (rng.unit() + rng.unit() + rng.unit() - 1.5) * noise;
            }
            img.set_pixel(x, y, px.map(|c| c.round().clamp(0.0, 255.0) as u8));
        }
    }
    img
}

/// A wood-like desk for [`compose_on_desk`]: `base` colour with strong, long grain lines (a random
/// walk down the rows, so each line is a few pixels thick and runs across the picture, amplitude
/// about `grain` grey levels) and a slow blotchy variation.
pub fn wood_desk(h: u32, seed: u64, grain: f32, base: [f32; 3]) -> impl Fn(u32, u32) -> [f32; 3] {
    let mut rng = Rng::new(seed);
    let mut rows = vec![0.0f32; h as usize + 1];
    let mut g = 0.0f32;
    for r in &mut rows {
        g = 0.72 * g + (rng.unit() - 0.5) * grain * 1.6;
        *r = g;
    }
    let phase = f64::from(rng.unit()) * 6.0;
    move |x, y| {
        let slow = ((f64::from(x) * 0.011 + f64::from(y) * 0.004 + phase).sin() as f32) * 7.0;
        let k = 1.0 + (rows[y.min(h) as usize] + slow) / 130.0;
        base.map(|c| c * k)
    }
}

/// A black-and-white print: a black background, a white mat inset by `mat` (a share of the frame on
/// every side) and inside it a grey picture inset by `pic` with soft shapes and noise. Returns the
/// image and the normalised corners (TL, TR, BR, BL) of the mat and of the picture.
pub fn matted_print(
    (w, h): (u32, u32),
    mat: f64,
    pic: f64,
    seed: u64,
) -> (Raster, SceneCorners, SceneCorners) {
    let mut rng = Rng::new(seed);
    let (cx, cy) = (
        0.35 + 0.2 * f64::from(rng.unit()),
        0.35 + 0.2 * f64::from(rng.unit()),
    );
    let back = 185.0 + 30.0 * rng.unit();
    let mut img = Raster::filled(w, h, [14, 14, 16]);
    let (wf, hf) = (f64::from(w), f64::from(h));
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (f64::from(x) / wf, f64::from(y) / hf);
            let in_mat = fx >= mat && fx < 1.0 - mat && fy >= mat && fy < 1.0 - mat;
            let in_pic = fx >= pic && fx < 1.0 - pic && fy >= pic && fy < 1.0 - pic;
            let n = (rng.unit() - 0.5) * 6.0;
            let v = if in_pic {
                let d = ((fx - cx) / 0.2).hypot((fy - cy) / 0.26);
                let blob = (-d * d * 1.5).exp();
                let patch = (-(((fx - 0.78) / 0.08).powi(2) + ((fy - 0.30) / 0.1).powi(2))).exp();
                back - 150.0 * blob as f32 + 40.0 * patch as f32 + n * 2.0
            } else if in_mat {
                250.0 + n * 0.5
            } else {
                14.0 + n.abs()
            };
            let c = v.round().clamp(0.0, 255.0) as u8;
            img.set_pixel(x, y, [c, c, c]);
        }
    }
    let corners = |m: f64| [(m, m), (1.0 - m, m), (1.0 - m, 1.0 - m), (m, 1.0 - m)];
    (img, corners(mat), corners(pic))
}

/// The corners of the scene in normalised coordinates, for tests and ground truth.
pub fn corners_of(s: &Scene) -> [(f64, f64); 4] {
    s.corners
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene() -> Scene {
        Scene {
            width: 320,
            height: 240,
            background: [120, 100, 80],
            paper: [244, 242, 236],
            ink: [60, 64, 76],
            kind: PaperKind::Receipt,
            corners: [(0.38, 0.10), (0.62, 0.12), (0.60, 0.92), (0.40, 0.90)],
            seed: 7,
            noise: 3.0,
            blur_radius: 1,
            shadow: true,
        }
    }

    #[test]
    fn rendering_is_deterministic_and_paper_is_bright() {
        let (a, b) = (render_scene(&scene()), render_scene(&scene()));
        assert_eq!(a, b);
        let centre = a.pixel(160, 120);
        assert!(centre.iter().all(|c| *c > 150), "{centre:?}");
        let corner = a.pixel(3, 3);
        assert!(corner.iter().all(|c| *c < 150), "{corner:?}");
    }

    #[test]
    fn rng_is_reproducible() {
        let (mut a, mut b) = (Rng::new(42), Rng::new(42));
        for _ in 0..8 {
            assert_eq!(a.next_u32(), b.next_u32());
        }
        assert!(Rng::new(1).unit() < 1.0);
    }
}
