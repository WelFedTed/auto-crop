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

/// Writes the sample set into `dir` (created if needed) and returns the files in order. Existing
/// files are reused, so "Try sample images" is instant the second time.
pub fn write_samples(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    fs::create_dir_all(dir)?;
    let mut out = Vec::new();
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
        assert_eq!(first.len(), specs().len() + bed_samples().len());
        assert!(first.iter().all(|p| p.metadata().unwrap().len() > 10_000));
        let m = first[0].metadata().unwrap().modified().unwrap();
        let again = write_samples(d.path()).unwrap();
        assert_eq!(again[0].metadata().unwrap().modified().unwrap(), m);
    }
}
