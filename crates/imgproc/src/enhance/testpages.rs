// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Synthetic pages for the enhancement oracles: real glyphs (a 5 x 7 bitmap font at an integer
//! scale, then blurred like a camera's transfer function), price columns with decimal points, a
//! barcode, a black band with a bright line, a coloured stamp, all with exact ground truth, lit by
//! a known illumination field with sensor noise. Public so the engine and CLI tests use the same
//! pages. Contains no private data (B21); the pages are a pure function of their seed.

use crate::Raster;
use crate::synth::Rng;

/// 5 x 7 glyphs, one row per `u8` (bit 4 = leftmost pixel).
const FONT: &[(char, [u8; 7])] = &[
    ('A', [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11]),
    ('B', [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E]),
    ('C', [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E]),
    ('D', [0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E]),
    ('E', [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F]),
    ('F', [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10]),
    ('G', [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F]),
    ('H', [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11]),
    ('I', [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E]),
    ('J', [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C]),
    ('K', [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11]),
    ('L', [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F]),
    ('M', [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11]),
    ('N', [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11]),
    ('O', [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E]),
    ('P', [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10]),
    ('Q', [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D]),
    ('R', [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11]),
    ('S', [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E]),
    ('T', [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04]),
    ('U', [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E]),
    ('V', [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04]),
    ('W', [0x11, 0x11, 0x11, 0x15, 0x15, 0x1B, 0x11]),
    ('X', [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11]),
    ('Y', [0x11, 0x11, 0x0A, 0x04, 0x04, 0x04, 0x04]),
    ('Z', [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F]),
    ('0', [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E]),
    ('1', [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E]),
    ('2', [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F]),
    ('3', [0x1E, 0x01, 0x01, 0x0E, 0x01, 0x01, 0x1E]),
    ('4', [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02]),
    ('5', [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E]),
    ('6', [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E]),
    ('7', [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08]),
    ('8', [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E]),
    ('9', [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C]),
    ('.', [0x00, 0x00, 0x00, 0x00, 0x00, 0x0C, 0x0C]),
    (',', [0x00, 0x00, 0x00, 0x00, 0x0C, 0x04, 0x08]),
    (':', [0x00, 0x0C, 0x0C, 0x00, 0x0C, 0x0C, 0x00]),
    ('-', [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00]),
    ('/', [0x01, 0x02, 0x02, 0x04, 0x08, 0x08, 0x10]),
];

/// The glyph of `c` (blank for a space or anything unknown).
fn glyph(c: char) -> [u8; 7] {
    FONT.iter()
        .find(|(k, _)| *k == c)
        .map_or([0; 7], |(_, g)| *g)
}

/// A rectangle, `x1` and `y1` exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

impl Rect {
    fn new(x0: u32, y0: u32, x1: u32, y1: u32) -> Self {
        Self { x0, y0, x1, y1 }
    }

    pub fn contains(&self, x: u32, y: u32) -> bool {
        (self.x0..self.x1).contains(&x) && (self.y0..self.y1).contains(&y)
    }

    pub fn grow(&self, by: u32, w: u32, h: u32) -> Rect {
        Rect {
            x0: self.x0.saturating_sub(by),
            y0: self.y0.saturating_sub(by),
            x1: (self.x1 + by).min(w),
            y1: (self.y1 + by).min(h),
        }
    }
}

/// What to draw.
#[derive(Debug, Clone, Copy)]
pub struct PageSpec {
    pub width: u32,
    pub height: u32,
    /// Pixels per font pixel: glyphs are `7 * scale` tall.
    pub scale: u32,
    pub seed: u64,
    /// A receipt (item names on the left, prices on the right) or a justified paragraph page.
    pub receipt: bool,
    pub barcode: bool,
    pub band: bool,
    pub stamp: bool,
    pub paper: [u8; 3],
    pub ink: [u8; 3],
}

impl PageSpec {
    /// A 1000 x 1400 document page (1.4 MP) with text 21 px tall.
    pub fn document(seed: u64) -> Self {
        Self {
            width: 1000,
            height: 1400,
            scale: 3,
            seed,
            receipt: false,
            barcode: false,
            band: false,
            stamp: false,
            paper: [236, 236, 236],
            ink: [28, 28, 28],
        }
    }

    /// A 600 x 1800 receipt with price columns, 21 px glyphs.
    pub fn receipt(seed: u64) -> Self {
        Self {
            width: 600,
            height: 1800,
            receipt: true,
            ..Self::document(seed)
        }
    }

    pub fn with_objects(self) -> Self {
        Self {
            barcode: true,
            band: true,
            stamp: true,
            ..self
        }
    }

    pub fn sized(self, width: u32, height: u32, scale: u32) -> Self {
        Self {
            width,
            height,
            scale,
            ..self
        }
    }
}

/// A page with its ground truth.
#[derive(Debug, Clone)]
pub struct TestPage {
    /// The unlit page: paper, ink, blurred like a camera.
    pub truth: Raster,
    /// Pixels that are ink in the sharp page (text glyphs, bars; not the band or the stamp).
    pub ink: Vec<bool>,
    /// The hard objects, for the ghost measurement.
    pub objects: Vec<(&'static str, Rect)>,
    /// Decimal points of the price columns (centre pixels), for the small-mark test.
    pub points: Vec<(u32, u32)>,
}

fn fill(r: &mut Raster, rect: Rect, rgb: [u8; 3]) {
    for y in rect.y0..rect.y1.min(r.height) {
        for x in rect.x0..rect.x1.min(r.width) {
            r.set_pixel(x, y, rgb);
        }
    }
}

struct Painter<'a> {
    img: &'a mut Raster,
    ink: &'a mut Vec<bool>,
    inkc: [u8; 3],
    scale: u32,
}

impl Painter<'_> {
    fn text(&mut self, x: u32, y: u32, s: &str) -> Vec<(u32, u32)> {
        let sc = self.scale;
        let mut dots = Vec::new();
        for (i, c) in s.chars().enumerate() {
            let g = glyph(c);
            let ox = x + i as u32 * 6 * sc;
            for (ry, row) in g.iter().enumerate() {
                for rx in 0..5u32 {
                    if row >> (4 - rx) & 1 == 1 {
                        for dy in 0..sc {
                            for dx in 0..sc {
                                let (px, py) = (ox + rx * sc + dx, y + ry as u32 * sc + dy);
                                if px < self.img.width && py < self.img.height {
                                    self.img.set_pixel(px, py, self.inkc);
                                    self.ink[(py * self.img.width + px) as usize] = true;
                                }
                            }
                        }
                    }
                }
            }
            if c == '.' {
                dots.push((ox + 3 * sc, y + 5 * sc + sc / 2));
            }
        }
        dots
    }
}

/// A word of `lo` to `lo + span - 1` letters.
fn word(rng: &mut Rng, lo: u32, span: u32) -> String {
    const LETTERS: &[u8] = b"ABCDEFGHIKLMNOPRSTUVWY";
    let len = lo + rng.below(span.max(1));
    (0..len)
        .map(|_| LETTERS[rng.below(LETTERS.len() as u32) as usize] as char)
        .collect()
}

fn price(rng: &mut Rng) -> String {
    format!("{}.{:02}", 1 + rng.below(98), rng.below(100))
}

/// Draws the page for `spec`.
pub fn page(spec: &PageSpec) -> TestPage {
    let (w, h, sc) = (spec.width, spec.height, spec.scale.max(1));
    let mut img = Raster::filled(w, h, spec.paper);
    let mut ink = vec![false; (w * h) as usize];
    let mut rng = Rng::new(spec.seed);
    let mut objects: Vec<(&'static str, Rect)> = Vec::new();
    let mut points = Vec::new();
    let pitch = 6 * sc;
    let line = 11 * sc;
    let margin = 6 * sc;
    let cols = (w - 2 * margin) / pitch;
    {
        let mut p = Painter {
            img: &mut img,
            ink: &mut ink,
            inkc: spec.ink,
            scale: sc,
        };
        let mut y = margin;
        // Title.
        let title = word(&mut rng, 8, 1);
        p.text(margin, y, &title);
        y += 2 * line;
        // Reserve room at the bottom for the objects.
        let reserve = if spec.barcode { 24 * sc } else { 0 } + if spec.band { 12 * sc } else { 0 };
        let object_top = h.saturating_sub(margin + reserve);
        while y + 7 * sc < object_top.min(h - margin) {
            if spec.receipt {
                let name = format!("{} {}", word(&mut rng, 3, 6), word(&mut rng, 2, 4));
                let pr = price(&mut rng);
                p.text(margin, y, &name[..name.len().min(cols as usize - 8)]);
                let px = margin + (cols - pr.len() as u32) * pitch;
                points.extend(p.text(px, y, &pr));
            } else {
                // A paragraph line: words separated by spaces, about 85% of the width; a number now
                // and then.
                let mut s = String::new();
                while (s.len() as u32) < cols * 85 / 100 {
                    if rng.below(9) == 0 {
                        let n = format!("{}.{}", rng.below(2000), rng.below(100));
                        s.push_str(&n);
                    } else {
                        s.push_str(&word(&mut rng, 2, 7));
                    }
                    s.push(' ');
                }
                s.truncate(cols as usize);
                points.extend(p.text(margin, y, &s));
            }
            y += line;
            // A blank line between paragraphs now and then.
            if !spec.receipt && rng.below(8) == 0 {
                y += line;
            }
        }
    }
    let mut y = h.saturating_sub(
        margin + if spec.barcode { 24 * sc } else { 0 } + if spec.band { 12 * sc } else { 0 },
    );
    if spec.band {
        let band = Rect::new(margin, y, w - margin, y + 8 * sc);
        fill(&mut img, band, [14, 14, 14]);
        let bright = Rect::new(
            margin + 6 * sc,
            y + 3 * sc,
            margin + 6 * sc + w / 3,
            y + 5 * sc,
        );
        fill(&mut img, bright, [225, 225, 225]);
        objects.push(("band", band));
        y += 12 * sc;
    }
    if spec.barcode {
        let bar = Rect::new(margin, y, margin + (w - 2 * margin) * 2 / 3, y + 20 * sc);
        let unit = (sc / 2).max(1);
        let mut x = bar.x0;
        while x + 4 * unit < bar.x1 {
            let bw = (1 + rng.below(3)) * unit;
            fill(&mut img, Rect::new(x, bar.y0, x + bw, bar.y1), spec.ink);
            for yy in bar.y0..bar.y1 {
                for xx in x..x + bw {
                    ink[(yy * w + xx) as usize] = true;
                }
            }
            x += bw + (1 + rng.below(3)) * unit;
        }
        objects.push(("barcode", bar));
    }
    if spec.stamp {
        // A blue ring stamp in the upper right corner of the text area.
        let r = 14 * sc;
        let (cx, cy) = (w - margin - r - 4 * sc, margin + r + 3 * line);
        let stamp = Rect::new(cx - r, cy - r, cx + r, cy + r);
        for yy in stamp.y0..stamp.y1 {
            for xx in stamp.x0..stamp.x1 {
                let d = ((xx as f32 - cx as f32).powi(2) + (yy as f32 - cy as f32).powi(2)).sqrt();
                if (r as f32 - 3.0 * sc as f32..=r as f32).contains(&d) {
                    img.set_pixel(xx, yy, [40, 70, 190]);
                }
            }
        }
        objects.push(("stamp", stamp));
    }
    TestPage {
        truth: blur(&img),
        ink,
        objects,
        points,
    }
}

/// Two passes of a 3 x 3 binomial blur: the camera's transfer function at proxy scale.
fn blur(src: &Raster) -> Raster {
    let (w, h) = (src.width as usize, src.height as usize);
    let mut cur = src.data.clone();
    for _ in 0..2 {
        let mut tmp = cur.clone();
        for y in 0..h {
            for x in 0..w {
                let (a, b) = (x.saturating_sub(1), (x + 1).min(w - 1));
                for c in 0..3 {
                    let v = u32::from(cur[(y * w + a) * 3 + c])
                        + 2 * u32::from(cur[(y * w + x) * 3 + c])
                        + u32::from(cur[(y * w + b) * 3 + c]);
                    tmp[(y * w + x) * 3 + c] = ((v + 2) / 4) as u8;
                }
            }
        }
        for y in 0..h {
            let (a, b) = (y.saturating_sub(1), (y + 1).min(h - 1));
            for x in 0..w {
                for c in 0..3 {
                    let v = u32::from(tmp[(a * w + x) * 3 + c])
                        + 2 * u32::from(tmp[(y * w + x) * 3 + c])
                        + u32::from(tmp[(b * w + x) * 3 + c]);
                    cur[(y * w + x) * 3 + c] = ((v + 2) / 4) as u8;
                }
            }
        }
    }
    Raster {
        width: src.width,
        height: src.height,
        data: cur,
    }
}

/// A smooth illumination factor in `(0, 1]`.
#[derive(Debug, Clone, Copy)]
pub enum Light {
    Flat,
    /// Linear fall-off to `1 - dx` at the right and `1 - dy` at the bottom.
    Tilt {
        dx: f32,
        dy: f32,
    },
    /// Radial vignette `1 - k (r / r_max)^2`.
    Vignette {
        k: f32,
    },
    /// A soft shadow across the page: smoothstep over `width` px, darkest factor `1 - depth`.
    Shadow {
        depth: f32,
        width: f32,
    },
}

impl Light {
    pub fn at(self, x: u32, y: u32, w: u32, h: u32) -> f32 {
        let (xf, yf) = (x as f32 / w as f32, y as f32 / h as f32);
        match self {
            Light::Flat => 1.0,
            Light::Tilt { dx, dy } => 1.0 - dx * xf - dy * yf,
            Light::Vignette { k } => {
                let (cx, cy) = (xf - 0.5, yf - 0.5);
                1.0 - k * (cx * cx + cy * cy) / 0.5
            }
            Light::Shadow { depth, width } => {
                let d = ((x as f32 - w as f32 * 0.55) * 0.8 + (y as f32 - h as f32 * 0.45) * 0.6)
                    / width;
                let t = (d * 0.5 + 0.5).clamp(0.0, 1.0);
                1.0 - depth * t * t * (3.0 - 2.0 * t)
            }
        }
    }
}

/// The photographed page: `truth * light * tint + noise` (a sum of three uniforms, sigma `noise`
/// levels). `tint` multiplies the channels (a warm lamp is `[1.0, 0.93, 0.82]`).
pub fn lit(truth: &Raster, light: Light, tint: [f32; 3], noise: f32, seed: u64) -> Raster {
    let mut rng = Rng::new(seed);
    let mut out = truth.clone();
    let (w, h) = (truth.width, truth.height);
    for y in 0..h {
        for x in 0..w {
            let l = light.at(x, y, w, h);
            let i = ((y * w + x) * 3) as usize;
            for (c, t) in tint.iter().enumerate() {
                let n = (rng.unit() + rng.unit() + rng.unit() - 1.5) * 2.0 * noise;
                let v = f32::from(truth.data[i + c]) * l * t + n;
                out.data[i + c] = (v + 0.5).clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

/// Thermal fade: the ink contrast to `keep` (0 to 1) of what it was, `v' = paper - keep (paper - v)`
/// on every channel, with the paper level taken from `spec`.
pub fn fade(truth: &Raster, paper: [u8; 3], keep: f32) -> Raster {
    let mut out = truth.clone();
    for (i, v) in out.data.iter_mut().enumerate() {
        let p = f32::from(paper[i % 3]);
        *v = (p - keep * (p - f32::from(*v)) + 0.5).clamp(0.0, 255.0) as u8;
    }
    out
}

// ------------------------------------------------------------------------------ photographs

/// Kinds of synthetic photograph, the false-positive suite of the suggestion (`measures`): scenes
/// that are NOT receipts or documents, with continuous tones, texture at every scale, regular
/// patterns (bricks, tiles) and large bright areas. They are procedural stand-ins, not
/// photographs: a clean result here is evidence against crude false positives, not a measurement
/// on real photos (which stays with the private slice, B21).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhotoKind {
    Landscape,
    Foliage,
    Interior,
    BrickWall,
    PosterWall,
    NightCity,
    Portrait,
    Tiles,
}

impl PhotoKind {
    pub const ALL: [PhotoKind; 8] = [
        PhotoKind::Landscape,
        PhotoKind::Foliage,
        PhotoKind::Interior,
        PhotoKind::BrickWall,
        PhotoKind::PosterWall,
        PhotoKind::NightCity,
        PhotoKind::Portrait,
        PhotoKind::Tiles,
    ];
}

fn lattice(ix: i32, iy: i32, seed: u64) -> f32 {
    let mut h = (ix as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (iy as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ seed.wrapping_mul(0x1656_67B1_9E37_79F9);
    h ^= h >> 29;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 32;
    (h >> 40) as f32 / (1u64 << 24) as f32
}

/// Smooth value noise in 0..1.
fn value_noise(x: f32, y: f32, seed: u64) -> f32 {
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let (ix, iy) = (x0 as i32, y0 as i32);
    let a = lattice(ix, iy, seed) * (1.0 - sx) + lattice(ix + 1, iy, seed) * sx;
    let b = lattice(ix, iy + 1, seed) * (1.0 - sx) + lattice(ix + 1, iy + 1, seed) * sx;
    a * (1.0 - sy) + b * sy
}

/// Fractal noise in 0..1: `octaves` octaves from the base wavelength `base` pixels.
fn fbm(x: f32, y: f32, base: f32, octaves: u32, seed: u64) -> f32 {
    let (mut sum, mut amp, mut norm, mut f) = (0.0, 1.0, 0.0, 1.0 / base);
    for o in 0..octaves {
        sum += amp * value_noise(x * f, y * f, seed.wrapping_add(u64::from(o) * 977));
        norm += amp;
        amp *= 0.5;
        f *= 2.0;
    }
    sum / norm
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// A synthetic photograph of `kind`, `w` x `h`, a pure function of the seed. Lit like a photo (a
/// smooth gradient) with sensor noise of 2 levels.
pub fn photo(kind: PhotoKind, seed: u64, w: u32, h: u32) -> Raster {
    let mut img = Raster::new(w, h);
    let mut rng = Rng::new(seed ^ 0x5EED);
    let wf = w as f32;
    let hf = h as f32;
    // Per-brick or per-object random numbers must not depend on pixel order.
    let rect_hash = |a: i32, b: i32| lattice(a, b, seed ^ 0xB41C);
    for y in 0..h {
        for x in 0..w {
            let (xf, yf) = (x as f32, y as f32);
            let (u, v) = (xf / wf, yf / hf);
            let rgb: [f32; 3] = match kind {
                PhotoKind::Landscape => {
                    let horizon = 0.45 + 0.08 * fbm(xf, 0.0, wf / 3.0, 3, seed);
                    if v < horizon {
                        let t = v / horizon;
                        let sun = (-(((u - 0.7).powi(2) + (v - 0.2).powi(2)) / 0.02)).exp();
                        let cloud = fbm(xf, yf, 90.0, 5, seed ^ 1);
                        let sky = mix([70.0, 120.0, 200.0], [190.0, 215.0, 240.0], t);
                        mix(
                            sky,
                            [255.0, 250.0, 235.0],
                            (0.55 * sun + 0.5 * cloud * cloud).min(1.0),
                        )
                    } else {
                        let g = fbm(xf, yf, 40.0, 6, seed ^ 2);
                        mix([40.0, 70.0, 30.0], [150.0, 140.0, 80.0], g)
                    }
                }
                PhotoKind::Foliage => {
                    let g = fbm(xf, yf, 14.0, 6, seed);
                    let l = fbm(xf, yf, 5.0, 3, seed ^ 3);
                    mix([20.0, 50.0, 15.0], [150.0, 190.0, 70.0], g * 0.7 + l * 0.5)
                }
                PhotoKind::Interior => {
                    let wall = [200.0, 190.0, 170.0];
                    let floor = [110.0, 80.0, 55.0];
                    let base = if v < 0.62 { wall } else { floor };
                    let mut c = mix(base, [base[0] * 0.8, base[1] * 0.8, base[2] * 0.8], u);
                    // A few pieces of furniture: random rectangles with their own colour.
                    for k in 0..5 {
                        let (x0, y0) = (rect_hash(k, 1) * 0.7, 0.3 + rect_hash(k, 2) * 0.5);
                        let (x1, y1) = (
                            x0 + 0.1 + rect_hash(k, 3) * 0.2,
                            y0 + 0.1 + rect_hash(k, 4) * 0.2,
                        );
                        if (x0..x1).contains(&u) && (y0..y1).contains(&v) {
                            c = [
                                rect_hash(k, 5) * 200.0 + 30.0,
                                rect_hash(k, 6) * 200.0 + 30.0,
                                rect_hash(k, 7) * 200.0 + 30.0,
                            ];
                        }
                    }
                    let n = fbm(xf, yf, 60.0, 3, seed ^ 4);
                    mix(c, [c[0] * 0.7, c[1] * 0.7, c[2] * 0.7], n)
                }
                PhotoKind::BrickWall => {
                    let (bw, bh) = (wf / 12.0, hf / 22.0);
                    let row = (yf / bh).floor();
                    let off = if row as i32 % 2 == 0 { 0.0 } else { bw / 2.0 };
                    let col = ((xf + off) / bw).floor();
                    let (fx, fy) = ((xf + off) / bw - col, yf / bh - row);
                    let mortar = fx < 0.04 || fy < 0.08;
                    let tint = rect_hash(col as i32, row as i32);
                    let n = fbm(xf, yf, 6.0, 3, seed ^ 5);
                    if mortar {
                        [190.0 - 30.0 * n, 185.0 - 30.0 * n, 175.0 - 30.0 * n]
                    } else {
                        [
                            150.0 + 60.0 * tint - 40.0 * n,
                            70.0 + 30.0 * tint - 30.0 * n,
                            50.0 + 20.0 * tint - 25.0 * n,
                        ]
                    }
                }
                PhotoKind::PosterWall => {
                    let mut c = [228.0, 226.0, 220.0];
                    for k in 0..3 {
                        let x0 = 0.08 + 0.3 * k as f32;
                        if (x0..x0 + 0.25).contains(&u) && (0.15..0.8).contains(&v) {
                            let t = fbm(xf, yf, 30.0, 4, seed ^ (10 + k as u64));
                            let pal = [
                                [200.0, 60.0, 50.0],
                                [50.0, 90.0, 190.0],
                                [60.0, 160.0, 90.0],
                            ][k];
                            c = mix([30.0, 30.0, 30.0], pal, t);
                            // A headline bar near the top of each poster.
                            if (0.17..0.22).contains(&v) {
                                c = [235.0, 235.0, 235.0];
                            }
                        }
                    }
                    c
                }
                PhotoKind::NightCity => {
                    let g = fbm(xf, yf, 120.0, 3, seed) * 40.0 + 8.0;
                    let lamp = lattice(x as i32 / 7, y as i32 / 7, seed ^ 6);
                    let on = if lamp > 0.93 && (x % 7 < 3 && y % 7 < 3) {
                        230.0
                    } else {
                        0.0
                    };
                    [g + on, g + on * 0.9, g + on * 0.6]
                }
                PhotoKind::Portrait => {
                    let bg = mix([90.0, 110.0, 140.0], [200.0, 200.0, 210.0], v);
                    let (cx, cy) = (0.5, 0.45);
                    let d = ((u - cx) / 0.22).powi(2) + ((v - cy) / 0.3).powi(2);
                    if d < 1.0 {
                        let shade = 0.75 + 0.25 * (1.0 - d);
                        let skin = fbm(xf, yf, 25.0, 3, seed ^ 7);
                        [
                            (205.0 + 20.0 * skin) * shade,
                            (155.0 + 20.0 * skin) * shade,
                            (125.0 + 15.0 * skin) * shade,
                        ]
                    } else {
                        bg
                    }
                }
                PhotoKind::Tiles => {
                    let n = 9.0;
                    let (fx, fy) = ((u * n).fract(), (v * n * hf / wf * wf / hf).fract());
                    let grout = fx < 0.025 || fy < 0.025;
                    let stone = fbm(xf, yf, 9.0, 4, seed ^ 8);
                    if grout {
                        [120.0, 120.0, 115.0]
                    } else {
                        let t = rect_hash((u * n) as i32, (v * n) as i32) * 0.2;
                        [
                            205.0 - 40.0 * stone - 20.0 * t,
                            200.0 - 40.0 * stone - 20.0 * t,
                            190.0 - 45.0 * stone - 20.0 * t,
                        ]
                    }
                }
            };
            // A smooth photographic light and sensor noise.
            let light = 0.85 + 0.15 * (1.0 - ((u - 0.4).powi(2) + (v - 0.4).powi(2)));
            for (c, v) in rgb.iter().enumerate() {
                let n = (rng.unit() + rng.unit() + rng.unit() - 1.5) * 4.0;
                img.data[((y * w + x) * 3) as usize + c] = (v * light + n).clamp(0.0, 255.0) as u8;
            }
        }
    }
    img
}

/// Rec. 709 luma of an RGB raster.
pub fn luma_of(r: &Raster) -> Vec<u8> {
    r.data
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| super::apply::luma709(*p))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_has_text_barcode_band_and_stamp_and_is_deterministic() {
        let spec = PageSpec::document(7).with_objects();
        let a = page(&spec);
        let b = page(&spec);
        assert_eq!(a.truth, b.truth);
        let ink = a.ink.iter().filter(|v| **v).count();
        let frac = ink as f32 / a.ink.len() as f32;
        assert!((0.04..0.30).contains(&frac), "ink share {frac}");
        let names: Vec<_> = a.objects.iter().map(|o| o.0).collect();
        assert_eq!(names, ["band", "barcode", "stamp"]);
        assert!(!a.points.is_empty(), "decimal points were placed");
        // The sharp ink is darker than the paper after the blur.
        let lu = luma_of(&a.truth);
        let darker = a
            .ink
            .iter()
            .zip(&lu)
            .filter(|(i, l)| **i && **l < 150)
            .count();
        assert!(darker as f32 > 0.5 * ink as f32);
    }

    #[test]
    fn a_receipt_has_price_columns() {
        let p = page(&PageSpec::receipt(3));
        assert!(p.points.len() > 30);
        // Prices sit in the right half.
        assert!(p.points.iter().all(|(x, _)| *x > 300));
    }

    #[test]
    fn light_is_a_factor_at_most_one_and_fade_keeps_the_paper() {
        let p = page(&PageSpec::document(1));
        let lit = lit(&p.truth, Light::Tilt { dx: 0.4, dy: 0.1 }, [1.0; 3], 0.0, 1);
        assert!(lit.data.iter().zip(&p.truth.data).all(|(a, b)| a <= b));
        let f = fade(&p.truth, [236; 3], 0.3);
        let paper = p.truth.pixel(2, 2)[0];
        assert_eq!(f.pixel(2, 2)[0], paper);
        let min_before = luma_of(&p.truth).into_iter().min().unwrap();
        let min_after = luma_of(&f).into_iter().min().unwrap();
        assert!(min_after > min_before + 50);
    }
}
