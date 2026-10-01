// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! "Try sample images": synthetic receipts and documents with a deliberate spread of difficulty,
//! so every tier (Good, Check, Failed) appears. Generated on demand, no private data (B21).

use auto_crop_codecs::{Format, encode};
use auto_crop_imgproc::synth::{PaperKind, Scene, render_scene};
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

/// Writes the sample set into `dir` (created if needed) and returns the files in order. Existing
/// files are reused, so "Try sample images" is instant the second time.
pub fn write_samples(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    fs::create_dir_all(dir)?;
    let mut out = Vec::new();
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
        assert_eq!(first.len(), specs().len());
        assert!(first.iter().all(|p| p.metadata().unwrap().len() > 10_000));
        let m = first[0].metadata().unwrap().modified().unwrap();
        let again = write_samples(d.path()).unwrap();
        assert_eq!(again[0].metadata().unwrap().modified().unwrap(), m);
    }
}
