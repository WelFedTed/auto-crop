// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Synthetic test pages: the repo's `synth::paper_texture` (text-like lines and an image-like
//! block) plus a barcode, a black band and a dense QR-like block, lit by a known smooth
//! illumination field and a little noise. The unlit texture is the ground truth.

use crate::flatten::Gray;
use auto_crop_imgproc::synth::{PaperKind, Rng, paper_texture};

pub const PAPER: u8 = 235;
const INK: u8 = 30;

/// Where the hard objects are (x0, y0, x1, y1), for the ghost measurement.
#[derive(Clone, Copy, Debug)]
pub struct Rect(pub usize, pub usize, pub usize, pub usize);

pub struct Page {
    pub truth: Gray,
    pub objects: Vec<(&'static str, Rect)>,
}

fn fill(g: &mut Gray, r: Rect, v: u8) {
    for y in r.1..r.3.min(g.h) {
        for x in r.0..r.2.min(g.w) {
            g.data[y * g.w + x] = v;
        }
    }
}

/// A 1000 x 1400 (1.4 MP, the analysis proxy size) page with its hard objects.
pub fn truth_page(seed: u64) -> Page {
    truth_page_with(seed, false)
}

/// `grey_block` keeps the texture's image-like mid-grey block (the hazard case); otherwise it is
/// whitened, because luma alone cannot tell a large uniform grey area from shaded paper.
pub fn truth_page_with(seed: u64, grey_block: bool) -> Page {
    let t = paper_texture(PaperKind::Document, [PAPER; 3], [INK; 3], seed);
    let (w, h) = (t.width as usize, t.height as usize);
    let block = INK / 2 + 120;
    let mut g = Gray {
        w,
        h,
        data: t
            .data
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| {
                if !grey_block && p[0] == block {
                    PAPER
                } else {
                    p[0]
                }
            })
            .collect(),
    };
    let mut rng = Rng::new(seed ^ 0xBA2C_0DE5);
    let mut objects = Vec::new();

    // Barcode as seen on a ~1.5 MP proxy of a phone photo: bars 1-3 px, gaps 1-3 px (about 50%
    // ink), 110 px tall. The blur at the end turns it into mid-grey, so no pixel in it reaches the
    // paper level (the failure the ink-coverage rule exists for).
    let bar = Rect(180, 860, 560, 970);
    fill(&mut g, bar, PAPER);
    let mut x = bar.0;
    while x < bar.2 - 4 {
        let bw = 1 + rng.below(3) as usize;
        fill(&mut g, Rect(x, bar.1, x + bw, bar.3), INK);
        x += bw + 1 + rng.below(3) as usize;
    }
    objects.push(("barcode", bar));

    // Black band (reverse-video header) with a bright text line inside.
    let band = Rect(120, 1080, 880, 1150);
    fill(&mut g, band, 12);
    fill(&mut g, Rect(160, 1108, 520, 1118), 220);
    objects.push(("black band", band));

    // QR-like dense block: 3 px modules, about half black.
    let qr = Rect(740, 180, 860, 300);
    for my in 0..40 {
        for mx in 0..40 {
            let v = if rng.below(100) < 52 { INK } else { PAPER };
            fill(
                &mut g,
                Rect(
                    qr.0 + mx * 3,
                    qr.1 + my * 3,
                    qr.0 + mx * 3 + 3,
                    qr.1 + my * 3 + 3,
                ),
                v,
            );
        }
    }
    objects.push(("qr block", qr));
    Page {
        truth: box_blur3(&g),
        objects,
    }
}

/// A smooth illumination factor in `(0, 1]`.
#[derive(Clone, Copy, Debug)]
pub enum Light {
    Flat,
    /// Linear fall-off, 1.0 at the top-left to `1 - dx` / `1 - dy` at the bottom-right edges.
    Tilt {
        dx: f32,
        dy: f32,
    },
    /// Radial vignette: `1 - k (r / r_max)^2`.
    Vignette {
        k: f32,
    },
    /// A soft shadow edge across the page (smoothstep over `width` px), darkest factor `1 - depth`.
    Shadow {
        depth: f32,
        width: f32,
    },
}

impl Light {
    pub fn at(self, x: usize, y: usize, w: usize, h: usize) -> f32 {
        let (xf, yf) = (x as f32 / w as f32, y as f32 / h as f32);
        match self {
            Light::Flat => 1.0,
            Light::Tilt { dx, dy } => 1.0 - dx * xf - dy * yf,
            Light::Vignette { k } => {
                let (cx, cy) = (xf - 0.5, yf - 0.5);
                let r2 = (cx * cx + cy * cy) / 0.5;
                1.0 - k * r2
            }
            Light::Shadow { depth, width } => {
                // Distance along a diagonal direction, centred on the page.
                let d = ((x as f32 - w as f32 * 0.55) * 0.8 + (y as f32 - h as f32 * 0.45) * 0.6)
                    / width;
                let t = (d * 0.5 + 0.5).clamp(0.0, 1.0);
                let s = t * t * (3.0 - 2.0 * t);
                1.0 - depth * s
            }
        }
    }
}

/// 3 x 3 box blur (the camera's modulation transfer at proxy scale, crudely).
fn box_blur3(g: &Gray) -> Gray {
    let mut out = g.clone();
    for y in 0..g.h {
        for x in 0..g.w {
            let (mut s, mut n) = (0u32, 0u32);
            for yy in y.saturating_sub(1)..(y + 2).min(g.h) {
                for xx in x.saturating_sub(1)..(x + 2).min(g.w) {
                    s += u32::from(g.data[yy * g.w + xx]);
                    n += 1;
                }
            }
            out.data[y * g.w + x] = ((s + n / 2) / n) as u8;
        }
    }
    out
}

/// The photographed page: `truth x light + noise` (uniform-sum noise of the given sigma).
pub fn light_page(truth: &Gray, light: Light, noise: f32, seed: u64) -> Gray {
    let mut rng = Rng::new(seed);
    let mut out = truth.clone();
    for y in 0..truth.h {
        for x in 0..truth.w {
            let i = y * truth.w + x;
            let n = (rng.unit() + rng.unit() + rng.unit() - 1.5) * 2.0 * noise;
            let v = f32::from(truth.data[i]) * light.at(x, y, truth.w, truth.h) + n;
            out.data[i] = (v + 0.5).clamp(0.0, 255.0) as u8;
        }
    }
    out
}
