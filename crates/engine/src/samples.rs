// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! "Try sample images": synthetic receipts and documents with a deliberate spread of difficulty,
//! so every tier (Good, Check, Failed) appears. Generated on demand, no private data (B21).

use auto_crop_codecs::{Format, encode};
use auto_crop_imgproc::synth::{PaperKind, Rng, Scene, render_scene};
use std::fs;
use std::path::{Path, PathBuf};

const W: u32 = 1800;
const H: u32 = 1350;

struct Spec {
    file: &'static str,
    kind: PaperKind,
    corners: [(f64, f64); 4],
    background: [u8; 3],
    paper: [u8; 3],
    ink: [u8; 3],
    noise: f32,
    blur: u32,
    shadow: bool,
}

const DESK: [u8; 3] = [118, 98, 80];
const PAPER: [u8; 3] = [244, 242, 236];
const INK: [u8; 3] = [60, 64, 76];

fn specs() -> Vec<Spec> {
    let s = |file, kind, corners| Spec {
        file,
        kind,
        corners,
        background: DESK,
        paper: PAPER,
        ink: INK,
        noise: 4.0,
        blur: 1,
        shadow: true,
    };
    use PaperKind::{Document as D, Receipt as R};
    vec![
        s(
            "receipt_1_tilted.jpg",
            R,
            [(0.36, 0.07), (0.63, 0.11), (0.60, 0.94), (0.39, 0.90)],
        ),
        s(
            "receipt_2_straight.jpg",
            R,
            [(0.38, 0.06), (0.62, 0.06), (0.62, 0.94), (0.38, 0.94)],
        ),
        s(
            "receipt_3_leaning.jpg",
            R,
            [(0.30, 0.12), (0.52, 0.05), (0.70, 0.88), (0.46, 0.95)],
        ),
        s(
            "document_1_perspective.jpg",
            D,
            [(0.17, 0.12), (0.84, 0.08), (0.88, 0.90), (0.12, 0.93)],
        ),
        s(
            "document_2_slight_tilt.jpg",
            D,
            [(0.22, 0.10), (0.78, 0.13), (0.76, 0.92), (0.20, 0.89)],
        ),
        Spec {
            background: [214, 210, 200],
            paper: [70, 74, 82],
            ink: [200, 205, 210],
            ..s(
                "document_3_dark_page_light_desk.jpg",
                D,
                [(0.2, 0.15), (0.8, 0.12), (0.82, 0.88), (0.18, 0.9)],
            )
        },
        Spec {
            blur: 3,
            noise: 9.0,
            ..s(
                "receipt_4_blurry.jpg",
                R,
                [(0.34, 0.09), (0.62, 0.07), (0.64, 0.93), (0.37, 0.95)],
            )
        },
        // Hard cases: these should land in Check or Failed.
        s(
            "hard_page_cut_by_frame.jpg",
            R,
            [(0.30, -0.10), (0.70, -0.08), (0.72, 0.95), (0.28, 0.93)],
        ),
        Spec {
            background: [214, 210, 202],
            paper: [232, 229, 222],
            shadow: false,
            ..s(
                "hard_low_contrast.jpg",
                R,
                [(0.30, 0.10), (0.70, 0.12), (0.68, 0.90), (0.32, 0.88)],
            )
        },
        Spec {
            background: [236, 234, 229],
            paper: [244, 242, 236],
            shadow: false,
            noise: 2.0,
            ..s(
                "hard_white_on_white.jpg",
                D,
                [(0.2, 0.15), (0.8, 0.12), (0.82, 0.88), (0.18, 0.9)],
            )
        },
        s(
            "document_4_fills_frame.jpg",
            D,
            [(0.03, 0.03), (0.97, 0.02), (0.98, 0.97), (0.02, 0.98)],
        ),
        Spec {
            // A desk with no paper on it at all: the page sits far outside the frame.
            ..s(
                "hard_no_document.jpg",
                D,
                [(1.4, 1.4), (1.9, 1.4), (1.9, 1.9), (1.4, 1.9)],
            )
        },
    ]
}

/// One rectangle lying on a scanner bed (a photo or a receipt), in pixels.
struct BedItem {
    c: (f64, f64),
    w: f64,
    h: f64,
    angle_deg: f64,
    /// Picture colours (sky, ground), or a flat paper colour for a receipt.
    colours: ([u8; 3], [u8; 3]),
    receipt: bool,
}

const BED: [u8; 3] = [233, 233, 228];

/// A flatbed scan with several items on it, a soft shadow under each and a little sensor noise. The
/// multi-item samples ("Try sample images" shows how a scan with several photos is split).
fn bed_scene(bed: [u8; 3], items: &[BedItem], seed: u64) -> auto_crop_imgproc::Raster {
    use auto_crop_imgproc::Raster;
    let mut r = Raster::filled(W, H, bed);
    let mut rng = Rng::new(seed);
    for it in items {
        let a = it.angle_deg.to_radians();
        let (ca, sa) = (a.cos(), a.sin());
        for y in 0..H {
            for x in 0..W {
                let local = |dx: f64, dy: f64| {
                    let (px, py) = (f64::from(x) - dx - it.c.0, f64::from(y) - dy - it.c.1);
                    (px * ca + py * sa, -px * sa + py * ca)
                };
                let (lx, ly) = local(0.0, 0.0);
                if lx.abs() <= it.w / 2.0 && ly.abs() <= it.h / 2.0 {
                    let edge = (it.w / 2.0 - lx.abs()).min(it.h / 2.0 - ly.abs());
                    let (u, v) = ((lx + it.w / 2.0) / it.w, (ly + it.h / 2.0) / it.h);
                    let col = if edge < 8.0 {
                        [247, 246, 243]
                    } else if it.receipt {
                        // Text-like bars on cream paper.
                        let row = (v * it.h / 9.0).fract() < 0.38 && v > 0.08 && v < 0.92;
                        if row && u > 0.12 && u < 0.88 {
                            [60, 64, 76]
                        } else {
                            it.colours.0
                        }
                    } else if (u - 0.24).hypot((v - 0.26) * it.h / it.w) < 0.1 {
                        [255, 240, 170]
                    } else if v > 0.62 {
                        it.colours.1
                    } else {
                        it.colours.0
                    };
                    r.set_pixel(x, y, col);
                } else {
                    let (sx, sy) = local(3.0, 5.0);
                    let d = (it.w / 2.0 - sx.abs()).min(it.h / 2.0 - sy.abs());
                    if d > -7.0 {
                        let k = ((d + 7.0) / 14.0).clamp(0.0, 1.0) as f32 * 0.35;
                        let p = r.pixel(x, y);
                        r.set_pixel(x, y, p.map(|c| (f32::from(c) * (1.0 - k)) as u8));
                    }
                }
            }
        }
    }
    for y in 0..H {
        for x in 0..W {
            let n = (rng.unit() - 0.5) * 6.0;
            let p = r.pixel(x, y);
            r.set_pixel(x, y, p.map(|c| (f32::from(c) + n).clamp(0.0, 255.0) as u8));
        }
    }
    r
}

/// The multi-item samples: file name and the picture.
fn bed_samples() -> Vec<(&'static str, auto_crop_imgproc::Raster)> {
    let photo =
        |c: (f64, f64), w: f64, h: f64, angle: f64, sky: [u8; 3], ground: [u8; 3]| BedItem {
            c,
            w,
            h,
            angle_deg: angle,
            colours: (sky, ground),
            receipt: false,
        };
    let receipt = |c: (f64, f64), h: f64, angle: f64| BedItem {
        c,
        w: 330.0,
        h,
        angle_deg: angle,
        colours: ([247, 244, 236], [247, 244, 236]),
        receipt: true,
    };
    vec![
        (
            "scan_album_page.jpg",
            bed_scene(
                BED,
                &[
                    photo(
                        (470.0, 380.0),
                        640.0,
                        460.0,
                        3.0,
                        [140, 180, 225],
                        [60, 70, 140],
                    ),
                    photo(
                        (1330.0, 360.0),
                        660.0,
                        450.0,
                        -2.0,
                        [235, 190, 150],
                        [110, 120, 60],
                    ),
                    photo(
                        (450.0, 960.0),
                        440.0,
                        520.0,
                        1.5,
                        [150, 215, 150],
                        [40, 120, 90],
                    ),
                    photo(
                        (1000.0, 980.0),
                        420.0,
                        520.0,
                        -1.0,
                        [225, 150, 190],
                        [130, 60, 90],
                    ),
                    photo(
                        (1450.0, 960.0),
                        400.0,
                        500.0,
                        2.0,
                        [190, 160, 225],
                        [90, 70, 130],
                    ),
                ],
                11,
            ),
        ),
        (
            "scan_two_photos.jpg",
            bed_scene(
                BED,
                &[
                    photo(
                        (520.0, 680.0),
                        660.0,
                        880.0,
                        2.0,
                        [150, 205, 230],
                        [50, 90, 130],
                    ),
                    photo(
                        (1290.0, 670.0),
                        660.0,
                        880.0,
                        -2.0,
                        [235, 205, 140],
                        [100, 120, 60],
                    ),
                ],
                12,
            ),
        ),
        (
            "scan_three_receipts.jpg",
            // Receipts are pale, so they lie on a darker bed.
            bed_scene(
                [150, 156, 150],
                &[
                    receipt((360.0, 680.0), 1050.0, 2.0),
                    receipt((900.0, 680.0), 980.0, -1.5),
                    receipt((1440.0, 680.0), 1100.0, 1.0),
                ],
                13,
            ),
        ),
    ]
}

// ---------------------------------------------------------------- the curved page

/// File name of the curved sample (a crumpled receipt, edges bent by an analytic model).
pub const CURVED_SAMPLE: &str = "receipt_curved.jpg";

/// Size of the curved sample picture.
pub const CURVED_SIZE: (u32, u32) = (1800, 1350);

/// The curved page: a flat receipt `PAGE_W` by `PAGE_H` pixels laid out at (`PAGE_X`, `PAGE_Y`) and
/// bent by two analytic terms, so the four edges are known exactly. The page coordinates
/// `(s, t)` (0..1 across and down) land on the picture at
///
/// ```text
/// x = PAGE_X + s * PAGE_W + BULGE * 4t(1-t) * (2s-1)      (the sides bulge outwards)
/// y = PAGE_Y + t * PAGE_H + SAG   * 4s(1-s)               (top and bottom dip in the middle)
/// ```
///
/// The corners stay at the corners of the rectangle (both terms vanish there). The text rows run
/// along `t`, so they bend with the page and are straight again once the page is flattened.
const PAGE_X: f64 = 560.0;
const PAGE_Y: f64 = 150.0;
const PAGE_W: f64 = 680.0;
const PAGE_H: f64 = 1000.0;
const SAG: f64 = 44.0;
const BULGE: f64 = 22.0;

/// Where page coordinates `(s, t)` land on the picture, in pixels.
pub fn curved_page_point(s: f64, t: f64) -> (f64, f64) {
    (
        PAGE_X + s * PAGE_W + BULGE * 4.0 * t * (1.0 - t) * (2.0 * s - 1.0),
        PAGE_Y + t * PAGE_H + SAG * 4.0 * s * (1.0 - s),
    )
}

/// The page coordinates of a picture pixel (the inverse of [`curved_page_point`], by fixed-point
/// iteration: both terms are small against the page size, so it converges quickly).
fn curved_page_coords(x: f64, y: f64) -> (f64, f64) {
    let (mut s, mut t) = ((x - PAGE_X) / PAGE_W, (y - PAGE_Y) / PAGE_H);
    for _ in 0..14 {
        t = (y - PAGE_Y - SAG * 4.0 * s * (1.0 - s)) / PAGE_H;
        s = (x - PAGE_X - BULGE * 4.0 * t * (1.0 - t) * (2.0 * s - 1.0)) / PAGE_W;
    }
    (s, t)
}

/// The four edges of the curved sample as a person would mark them, `n` points each at equal
/// steps of the page parameter: top (TL to TR), right (TR to BR), bottom (BR to BL), left (BL to
/// TL), normalised to the picture. The first and last point of each are the page corners.
pub fn curved_sample_edges(n: usize) -> [Vec<(f64, f64)>; 4] {
    let (w, h) = (f64::from(CURVED_SIZE.0), f64::from(CURVED_SIZE.1));
    let at = |s: f64, t: f64| {
        let (x, y) = curved_page_point(s, t);
        (x / w, y / h)
    };
    let steps = |f: &dyn Fn(f64) -> (f64, f64)| -> Vec<(f64, f64)> {
        (0..n).map(|i| f(i as f64 / (n - 1) as f64)).collect()
    };
    [
        steps(&|u| at(u, 0.0)),
        steps(&|u| at(1.0, u)),
        steps(&|u| at(1.0 - u, 1.0)),
        steps(&|u| at(0.0, 1.0 - u)),
    ]
}

/// What is printed on the flat page at `(s, t)`: cream paper with rows of text and a barcode.
fn receipt_ink(s: f64, t: f64) -> [u8; 3] {
    const PAPER: [u8; 3] = [247, 244, 236];
    const INK: [u8; 3] = [60, 64, 76];
    // A pseudo-random length per row, stable between runs.
    let hash = |k: f64| (((k * 12.9898).sin() * 43758.5453).fract()).abs();
    let rows = 26.0;
    let r = (t * rows).floor();
    let along = (t * rows).fract();
    if t > 0.04 && t < 0.075 && (0.2..0.8).contains(&s) {
        return INK; // the shop name
    }
    if t > 0.1 && t < 0.9 && along < 0.46 && s > 0.1 && s < 0.1 + 0.8 * (0.45 + 0.5 * hash(r)) {
        return INK;
    }
    if t > 0.92 && t < 0.97 && (0.15..0.85).contains(&s) && ((s * 90.0).floor() as i64 % 3 != 1) {
        return INK; // the barcode
    }
    PAPER
}

/// The curved sample picture: the receipt on a desk, 2x2 supersampled so the edges are smooth.
fn curved_sample() -> auto_crop_imgproc::Raster {
    use auto_crop_imgproc::Raster;
    const DESK: [u8; 3] = [118, 98, 80];
    let (w, h) = CURVED_SIZE;
    let mut r = Raster::filled(w, h, DESK);
    let mut rng = Rng::new(21);
    let (x_lo, x_hi) = (
        (PAGE_X - BULGE - 6.0).max(0.0) as u32,
        ((PAGE_X + PAGE_W + BULGE + 6.0) as u32).min(w),
    );
    let (y_lo, y_hi) = (
        (PAGE_Y - 6.0).max(0.0) as u32,
        ((PAGE_Y + PAGE_H + SAG + 6.0) as u32).min(h),
    );
    for y in 0..h {
        for x in 0..w {
            let n = (rng.unit() - 0.5) * 8.0;
            let mut c = [f32::from(DESK[0]), f32::from(DESK[1]), f32::from(DESK[2])];
            if (x_lo..x_hi).contains(&x) && (y_lo..y_hi).contains(&y) {
                let mut acc = [0.0f32; 3];
                let mut paper = 0.0f32;
                for (dx, dy) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                    let (s, t) = curved_page_coords(f64::from(x) + dx, f64::from(y) + dy);
                    if (0.0..=1.0).contains(&s) && (0.0..=1.0).contains(&t) {
                        let ink = receipt_ink(s, t);
                        for k in 0..3 {
                            acc[k] += f32::from(ink[k]);
                        }
                        paper += 1.0;
                    }
                }
                if paper > 0.0 {
                    let share = paper / 4.0;
                    for k in 0..3 {
                        c[k] = c[k] * (1.0 - share) + acc[k] / paper * share;
                    }
                }
            }
            r.set_pixel(x, y, c.map(|v| (v + n).clamp(0.0, 255.0) as u8));
        }
    }
    r
}

/// Writes the sample set into `dir` (created if needed) and returns the files in order. Existing
/// files are reused, so "Try sample images" is instant the second time.
pub fn write_samples(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    fs::create_dir_all(dir)?;
    let mut out = Vec::new();
    // The curved page first: it is the one to try the curved-edges editor on.
    let curved = dir.join(CURVED_SAMPLE);
    if !curved.exists() {
        let bytes = encode(&curved_sample(), Format::Jpeg, 90, None)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        fs::write(&curved, bytes)?;
    }
    out.push(curved);
    for (file, raster) in bed_samples() {
        let path = dir.join(file);
        if !path.exists() {
            let bytes = encode(&raster, Format::Jpeg, 90, None)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            fs::write(&path, bytes)?;
        }
        out.push(path);
    }
    for (i, sp) in specs().iter().enumerate() {
        let path = dir.join(sp.file);
        if !path.exists() {
            let scene = Scene {
                width: W,
                height: H,
                background: sp.background,
                paper: sp.paper,
                ink: sp.ink,
                kind: sp.kind,
                corners: sp.corners,
                seed: 100 + i as u64,
                noise: sp.noise,
                blur_radius: sp.blur,
                shadow: sp.shadow,
            };
            let raster = render_scene(&scene);
            let bytes = encode(&raster, Format::Jpeg, 88, None)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            fs::write(&path, bytes)?;
        }
        out.push(path);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sample_set_is_written_once_and_reused() {
        let d = tempfile::tempdir().unwrap();
        let first = write_samples(d.path()).unwrap();
        assert_eq!(first.len(), specs().len() + bed_samples().len() + 1);
        assert!(
            first[0].ends_with(CURVED_SAMPLE),
            "the curved page comes first"
        );
        assert!(first.iter().all(|p| p.metadata().unwrap().len() > 10_000));
        let m = first[0].metadata().unwrap().modified().unwrap();
        let again = write_samples(d.path()).unwrap();
        assert_eq!(again[0].metadata().unwrap().modified().unwrap(), m);
    }
}
