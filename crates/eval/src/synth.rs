// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `synth`: a STAND-IN synthetic suite writer (ROADMAP M1.35, minimal).
//!
//! **This is not M1.30.** The planned generator is a Python/Augraphy tool with known-text pages,
//! CC0 backgrounds and real degradations (M1.30-M1.34). This stand-in exists so the accuracy
//! harness has data before that lands. It is built on the Rust scene renderer in
//! `auto-crop-imgproc::synth` (flat desk, two paper kinds, box blur, noise, a soft shadow), adds a
//! pinhole camera for the ground truth and some distractor clutter, and writes JPEG/PNG files plus
//! `manifest.jsonl`. Consequences, all deliberate:
//!
//! * The detector under test and this generator share a lineage (the detector's unit tests use the
//!   same renderer), so numbers measured here say nothing about real photographs. They detect
//!   regressions between two builds; they never back a real-world accuracy claim (B6, PLAN 7.4).
//! * Only JPEG and PNG, EXIF orientation 1 only, one item per image, paper fully inside the frame,
//!   one receipt aspect (about 3.1:1, not the plan's > 4:1).
//! * Output is deterministic for a given suite and seed on one platform (it uses f64
//!   trigonometry, so bit-exactness across operating systems is not promised).
//!
//! Output is never committed: write it under `target/` or another ignored directory.

use crate::geom::{self, P, Quad};
use crate::manifest::ManifestItem;
use crate::stats::SplitMix64;
use auto_crop_codecs::{Format, encode};
use auto_crop_imgproc::Raster;
use auto_crop_imgproc::synth::{PaperKind, Scene, render_scene};
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::path::Path;

pub const GENERATOR: &str = "stand-in-rust/1";

#[derive(Debug, Clone)]
pub struct SuiteSpec {
    pub name: String,
    pub count: usize,
    /// Long edge of every image in pixels.
    pub max_edge: u32,
    pub seed: u64,
}

impl SuiteSpec {
    /// `smoke`: 200 images for the per-PR gate. `full`: 5,184 images (48 x 108, so every tag value
    /// is exactly balanced) for the nightly suite.
    pub fn named(name: &str) -> Option<Self> {
        let (count, max_edge) = match name {
            "smoke" => (200, 480),
            "full" => (5184, 480),
            _ => return None,
        };
        Some(Self {
            name: name.to_owned(),
            count,
            max_edge,
            seed: 0x5EED_A070_C20B_0001,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lighting {
    Normal,
    Dim,
    LowContrast,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Clutter {
    None,
    Light,
    Heavy,
}

/// Everything needed to render one image, derived purely from `(spec, index)`.
#[derive(Debug, Clone)]
pub struct ImagePlan {
    pub index: usize,
    pub id: String,
    pub scene_id: String,
    pub split: &'static str,
    kind: PaperKind,
    lighting: Lighting,
    clutter: Clutter,
    pub tilt_bucket: usize,
    pub png: bool,
    pub jpeg_quality: u8,
    scene_seed: u64,
    image_seed: u64,
    pub width: u32,
    pub height: u32,
    roll_deg: f64,
    tilt_deg: f64,
    azimuth: f64,
    fill: f64,
    offset: (f64, f64),
}

const TILT_BUCKETS: [(&str, f64, f64); 3] = [
    ("0-10", 0.0, 10.0),
    ("10-30", 10.0, 30.0),
    ("30-45", 30.0, 45.0),
];

fn mix(a: u64, b: u64) -> u64 {
    SplitMix64(a ^ b.wrapping_mul(0x9E37_79B9_7F4A_7C15)).next_u64()
}

/// Splits are by scene hash, 30% dev and 70% test, so a scene never spans both.
fn split_of(scene_index: usize, seed: u64) -> &'static str {
    if mix(seed, 0x5C11 ^ scene_index as u64) % 10 < 3 {
        "dev"
    } else {
        "test"
    }
}

pub fn plan(spec: &SuiteSpec, i: usize) -> ImagePlan {
    let scene = i / 2;
    let mut r = SplitMix64(mix(spec.seed, i as u64));
    let lighting = [Lighting::Normal, Lighting::Dim, Lighting::LowContrast][i % 3];
    let clutter = [Clutter::None, Clutter::Light, Clutter::Heavy][(i / 3) % 3];
    let tilt_bucket = (i / 9) % 3;
    let png = (i / 27) % 2 == 1;
    let kind = if scene.is_multiple_of(2) {
        PaperKind::Document
    } else {
        PaperKind::Receipt
    };
    let (bw, bh) = {
        let e = spec.max_edge;
        let short = e * 3 / 4;
        match kind {
            PaperKind::Receipt => (short, e),
            PaperKind::Document => {
                if r.next_u64() & 1 == 0 {
                    (e, short)
                } else {
                    (short, e)
                }
            }
        }
    };
    let (_, lo, hi) = TILT_BUCKETS[tilt_bucket];
    let dominant = lo + (hi - lo) * r.unit();
    let other = dominant * r.unit();
    let sign = if r.next_u64() & 1 == 0 { 1.0 } else { -1.0 };
    let (roll, tilt) = if r.next_u64() & 1 == 0 {
        (sign * dominant, other)
    } else {
        (sign * other, dominant)
    };
    ImagePlan {
        index: i,
        id: format!("{}-{i:05}", spec.name),
        scene_id: format!("{}-s{scene:04}", spec.name),
        split: split_of(scene, spec.seed),
        kind,
        lighting,
        clutter,
        tilt_bucket,
        png,
        jpeg_quality: 40 + (r.below(56)) as u8,
        scene_seed: mix(spec.seed, 0x5CE4E ^ scene as u64),
        image_seed: mix(spec.seed, 0x1A6E ^ i as u64),
        width: bw,
        height: bh,
        roll_deg: roll,
        tilt_deg: tilt,
        azimuth: std::f64::consts::TAU * r.unit(),
        fill: 0.72 + 0.28 * r.unit(),
        offset: (2.0 * r.unit() - 1.0, 2.0 * r.unit() - 1.0),
    }
}

fn matmul(a: [[f64; 3]; 3], b: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut o = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            o[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    o
}

/// The four page corners (TL, TR, BR, BL), normalised to the canvas, from a pinhole camera looking
/// at a paper of aspect `paper_w : paper_h` tilted by `tilt_deg` about an in-plane axis at
/// `azimuth` and rolled by `roll_deg`, then scaled and shifted to sit inside the frame.
fn project(p: &ImagePlan, paper_w: f64, paper_h: f64) -> Quad {
    let (a, t) = (p.azimuth, p.tilt_deg.to_radians());
    let (ax, ay) = (a.cos(), a.sin());
    // Rodrigues for the axis (ax, ay, 0).
    let k = [[0.0, 0.0, ay], [0.0, 0.0, -ax], [-ay, ax, 0.0]];
    let k2 = matmul(k, k);
    let mut tilt = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            tilt[i][j] =
                if i == j { 1.0 } else { 0.0 } + t.sin() * k[i][j] + (1.0 - t.cos()) * k2[i][j];
        }
    }
    let (s, c) = p.roll_deg.to_radians().sin_cos();
    let roll = [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]];
    let rot = matmul(roll, tilt);
    let depth = 2.2 * paper_w.max(paper_h);
    let corners = [
        [-paper_w / 2.0, -paper_h / 2.0],
        [paper_w / 2.0, -paper_h / 2.0],
        [paper_w / 2.0, paper_h / 2.0],
        [-paper_w / 2.0, paper_h / 2.0],
    ];
    let proj: Vec<P> = corners
        .iter()
        .map(|c| {
            let q = [
                rot[0][0] * c[0] + rot[0][1] * c[1],
                rot[1][0] * c[0] + rot[1][1] * c[1],
                rot[2][0] * c[0] + rot[2][1] * c[1],
            ];
            let z = q[2] + depth;
            [q[0] / z, q[1] / z]
        })
        .collect();
    let (xmin, xmax) = proj
        .iter()
        .fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p[0]), a.1.max(p[0])));
    let (ymin, ymax) = proj
        .iter()
        .fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p[1]), a.1.max(p[1])));
    let (w, h) = (f64::from(p.width), f64::from(p.height));
    let margin = 0.06;
    let (avail_w, avail_h) = (w * (1.0 - 2.0 * margin), h * (1.0 - 2.0 * margin));
    let scale = p.fill * (avail_w / (xmax - xmin)).min(avail_h / (ymax - ymin));
    let slack = (
        (avail_w - scale * (xmax - xmin)) / 2.0,
        (avail_h - scale * (ymax - ymin)) / 2.0,
    );
    let centre = (
        w / 2.0 + p.offset.0 * slack.0,
        h / 2.0 + p.offset.1 * slack.1,
    );
    let (xm, ym) = ((xmin + xmax) / 2.0, (ymin + ymax) / 2.0);
    std::array::from_fn(|i| {
        [
            (centre.0 + scale * (proj[i][0] - xm)) / w,
            (centre.1 + scale * (proj[i][1] - ym)) / h,
        ]
    })
}

fn rgb(r: &mut SplitMix64, lo: u8, hi: u8) -> [u8; 3] {
    std::array::from_fn(|_| lo + (r.unit() * f64::from(hi - lo)) as u8)
}

fn scaled(c: [u8; 3], k: f64) -> [u8; 3] {
    std::array::from_fn(|i| (f64::from(c[i]) * k).round().clamp(0.0, 255.0) as u8)
}

/// Distance in pixels from `p` to the outside of the convex quad (0 inside).
fn dist_outside(q: &[P; 4], p: P) -> f64 {
    if geom::point_in_convex(q, p) {
        return 0.0;
    }
    (0..4)
        .map(|i| {
            let (a, b) = (q[i], q[(i + 1) % 4]);
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let t =
                (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
            (p[0] - (a[0] + t * dx)).hypot(p[1] - (a[1] + t * dy))
        })
        .fold(f64::MAX, f64::min)
}

/// Draws distractor rectangles and stripes on the desk only. Pixels within 8 px of the page are
/// left alone, so clutter makes the page harder to find without hiding or moving its true edge.
fn add_clutter(img: &mut Raster, quad_px: &[P; 4], level: Clutter, light: f64, r: &mut SplitMix64) {
    let count = match level {
        Clutter::None => 0,
        Clutter::Light => 3,
        Clutter::Heavy => 9,
    };
    let (w, h) = (i64::from(img.width), i64::from(img.height));
    for _ in 0..count {
        let (rw, rh) = (
            (w as f64 * (0.05 + 0.25 * r.unit())) as i64,
            (h as f64 * (0.04 + 0.2 * r.unit())) as i64,
        );
        let (x0, y0) = (
            (r.unit() * (w - rw) as f64) as i64,
            (r.unit() * (h - rh) as f64) as i64,
        );
        // About a third of the distractors are paper-white, the case that confuses a threshold.
        let colour = if r.unit() < 0.33 {
            scaled(rgb(r, 225, 252), light)
        } else {
            scaled(rgb(r, 20, 220), light)
        };
        let stripe = r.unit() < 0.4;
        for y in y0.max(0)..(y0 + rh).min(h) {
            for x in x0.max(0)..(x0 + rw).min(w) {
                if stripe && (x / 4 + y / 4) % 2 == 0 {
                    continue;
                }
                if dist_outside(quad_px, [x as f64, y as f64]) > 8.0 {
                    img.set_pixel(x as u32, y as u32, colour);
                }
            }
        }
    }
}

/// Renders one image and returns it with its ground-truth quad (normalised, TL TR BR BL).
pub fn render(p: &ImagePlan) -> (Raster, Quad) {
    let mut sr = SplitMix64(p.scene_seed);
    let bg_base = rgb(&mut sr, 70, 170);
    let paper_base = rgb(&mut sr, 236, 250);
    let ink_base = rgb(&mut sr, 25, 80);
    let mut ir = SplitMix64(p.image_seed);
    let (background, paper, ink, noise, light) = match p.lighting {
        Lighting::Normal => (
            bg_base,
            paper_base,
            ink_base,
            2.0 + 2.0 * ir.unit() as f32,
            1.0,
        ),
        Lighting::Dim => {
            let k = 0.40 + 0.15 * ir.unit();
            (
                scaled(bg_base, k),
                scaled(paper_base, k),
                scaled(ink_base, k),
                5.0 + 3.0 * ir.unit() as f32,
                k,
            )
        }
        Lighting::LowContrast => {
            let d = 16.0 + 14.0 * ir.unit();
            let bg =
                std::array::from_fn(|i| (f64::from(paper_base[i]) - d).clamp(0.0, 255.0) as u8);
            (bg, paper_base, ink_base, 2.0 + 2.0 * ir.unit() as f32, 1.0)
        }
    };
    let (pw, ph) = match p.kind {
        PaperKind::Receipt => (420.0, 1300.0),
        PaperKind::Document => (1000.0, 1400.0),
    };
    let quad = project(p, pw, ph);
    let scene = Scene {
        width: p.width,
        height: p.height,
        background,
        paper,
        ink,
        kind: p.kind,
        corners: std::array::from_fn(|i| (quad[i][0], quad[i][1])),
        seed: p.scene_seed,
        noise,
        blur_radius: ir.below(3) as u32,
        shadow: ir.unit() < 0.5,
    };
    let mut img = render_scene(&scene);
    let (w, h) = (f64::from(p.width), f64::from(p.height));
    let quad_px: [P; 4] = std::array::from_fn(|i| [quad[i][0] * w, quad[i][1] * h]);
    add_clutter(&mut img, &quad_px, p.clutter, light, &mut ir);
    (img, quad)
}

fn tags_of(p: &ImagePlan) -> BTreeMap<String, String> {
    let lighting = match p.lighting {
        Lighting::Normal => "normal",
        Lighting::Dim => "dim",
        Lighting::LowContrast => "low-contrast",
    };
    let clutter = match p.clutter {
        Clutter::None => "none",
        Clutter::Light => "light",
        Clutter::Heavy => "heavy",
    };
    let aspect = match p.kind {
        PaperKind::Receipt => "receipt",
        PaperKind::Document => "document",
    };
    [
        ("lighting", lighting),
        ("clutter", clutter),
        ("tilt", TILT_BUCKETS[p.tilt_bucket].0),
        ("aspect", aspect),
        ("format", if p.png { "png" } else { "jpeg" }),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect()
}

#[derive(Debug, Clone)]
pub struct Generated {
    pub n: usize,
    pub image_bytes: u64,
    pub manifest_bytes: u64,
}

/// Writes `images/*` and `manifest.jsonl` under `out`.
pub fn generate(spec: &SuiteSpec, out: &Path) -> Result<Generated, String> {
    let images = out.join("images");
    std::fs::create_dir_all(&images)
        .map_err(|e| format!("cannot create {}: {e}", images.display()))?;
    let rows: Vec<Result<(ManifestItem, u64), String>> = (0..spec.count)
        .into_par_iter()
        .map(|i| {
            let p = plan(spec, i);
            let (img, quad) = render(&p);
            let (fmt, ext) = if p.png {
                (Format::Png, "png")
            } else {
                (Format::Jpeg, "jpg")
            };
            let bytes =
                encode(&img, fmt, p.jpeg_quality, None).map_err(|e| format!("{}: {e}", p.id))?;
            let rel = format!("images/{}.{ext}", p.id);
            std::fs::write(out.join(&rel), &bytes).map_err(|e| format!("{rel}: {e}"))?;
            let tags = tags_of(&p);
            Ok((
                ManifestItem {
                    v: 1,
                    id: p.id.clone(),
                    image: rel,
                    scene_id: p.scene_id.clone(),
                    split: Some(p.split.to_owned()),
                    width: p.width,
                    height: p.height,
                    quad,
                    items: Vec::new(),
                    tags,
                },
                bytes.len() as u64,
            ))
        })
        .collect();
    let mut text = String::new();
    let mut image_bytes = 0;
    for r in rows {
        let (item, len) = r?;
        image_bytes += len;
        text.push_str(&serde_json::to_string(&item).map_err(|e| e.to_string())?);
        text.push('\n');
    }
    std::fs::write(out.join("manifest.jsonl"), &text).map_err(|e| format!("manifest: {e}"))?;
    Ok(Generated {
        n: spec.count,
        image_bytes,
        manifest_bytes: text.len() as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest;

    fn tiny(count: usize) -> SuiteSpec {
        SuiteSpec {
            name: "t".to_owned(),
            count,
            max_edge: 160,
            seed: 7,
        }
    }

    #[test]
    fn suite_names_and_sizes() {
        assert_eq!(SuiteSpec::named("smoke").expect("smoke").count, 200);
        assert!(SuiteSpec::named("full").expect("full").count >= 5000);
        assert!(SuiteSpec::named("huge").is_none());
    }

    #[test]
    fn plans_are_deterministic_and_balanced_on_every_axis() {
        let spec = SuiteSpec::named("full").expect("full");
        let a: Vec<_> = (0..spec.count).map(|i| tags_of(&plan(&spec, i))).collect();
        let b: Vec<_> = (0..spec.count).map(|i| tags_of(&plan(&spec, i))).collect();
        assert_eq!(a, b);
        let mut counts: BTreeMap<(String, String), usize> = BTreeMap::new();
        for t in &a {
            for axis in ["lighting", "clutter", "tilt", "aspect", "format"] {
                *counts
                    .entry((axis.to_owned(), t[axis].clone()))
                    .or_default() += 1;
            }
        }
        // M1.35 asks for every slice cell >= 200 on the full suite.
        assert!(counts.values().all(|&n| n >= 200), "{counts:?}");
        assert_eq!(counts[&("lighting".into(), "dim".into())], 5184 / 3);
        assert_eq!(counts[&("format".into(), "png".into())], 5184 / 2);
    }

    #[test]
    fn scenes_never_span_splits_and_split_ratio_is_about_30_70() {
        let spec = SuiteSpec::named("full").expect("full");
        let plans: Vec<_> = (0..spec.count).map(|i| plan(&spec, i)).collect();
        let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
        for p in &plans {
            assert_eq!(*seen.entry(&p.scene_id).or_insert(p.split), p.split);
        }
        let dev = plans.iter().filter(|p| p.split == "dev").count() as f64 / plans.len() as f64;
        assert!((dev - 0.3).abs() < 0.04, "dev share {dev}");
    }

    #[test]
    fn the_ground_truth_quad_is_inside_the_frame_clockwise_and_in_its_tilt_bucket() {
        let spec = SuiteSpec::named("full").expect("full");
        for i in (0..spec.count).step_by(7) {
            let p = plan(&spec, i);
            let (pw, ph) = match p.kind {
                PaperKind::Receipt => (420.0, 1300.0),
                PaperKind::Document => (1000.0, 1400.0),
            };
            let q = project(&p, pw, ph);
            assert!(
                q.iter()
                    .all(|c| (0.05..=0.95).contains(&c[0]) && (0.05..=0.95).contains(&c[1])),
                "{i}: {q:?}"
            );
            assert!(
                geom::signed_area(&q) > 0.0 && geom::quad_is_simple(&q),
                "{i}"
            );
            let tilt = p.roll_deg.abs().max(p.tilt_deg);
            let (_, lo, hi) = TILT_BUCKETS[p.tilt_bucket];
            assert!(
                tilt >= lo && tilt <= hi,
                "{i}: tilt {tilt} not in [{lo}, {hi}]"
            );
        }
    }

    #[test]
    fn a_flat_camera_gives_a_plain_rectangle() {
        let mut p = plan(&tiny(10), 0);
        p.roll_deg = 0.0;
        p.tilt_deg = 0.0;
        let q = project(&p, 1000.0, 1400.0);
        assert!((q[0][1] - q[1][1]).abs() < 1e-12 && (q[0][0] - q[3][0]).abs() < 1e-12);
        let ratio = ((q[1][0] - q[0][0]) * f64::from(p.width))
            / ((q[3][1] - q[0][1]) * f64::from(p.height));
        assert!((ratio - 1000.0 / 1400.0).abs() < 1e-9, "{ratio}");
    }

    #[test]
    fn rendering_is_deterministic_and_clutter_stays_off_the_page() {
        let mut p = plan(&tiny(10), 5);
        p.clutter = Clutter::Heavy;
        let (a, qa) = render(&p);
        let (b, qb) = render(&p);
        assert_eq!(a, b);
        assert_eq!(qa, qb);
        // Pixels well inside the page are never clutter: compare with the clutter-free render.
        let mut clean = p.clone();
        clean.clutter = Clutter::None;
        let (c, _) = render(&clean);
        let (w, h) = (f64::from(p.width), f64::from(p.height));
        let px: [P; 4] = std::array::from_fn(|i| [qa[i][0] * w, qa[i][1] * h]);
        let mut changed_outside = 0;
        for y in 0..p.height {
            for x in 0..p.width {
                if a.pixel(x, y) != c.pixel(x, y) {
                    assert!(
                        dist_outside(&px, [f64::from(x), f64::from(y)]) > 7.0,
                        "clutter touched the page at {x},{y}"
                    );
                    changed_outside += 1;
                }
            }
        }
        assert!(
            changed_outside > 100,
            "heavy clutter should change many pixels"
        );
    }

    #[test]
    fn a_generated_suite_validates_decodes_and_is_reproducible() {
        let dir = std::env::temp_dir().join(format!("ac-synth-{}", std::process::id()));
        let (d1, d2) = (dir.join("a"), dir.join("b"));
        let spec = tiny(12);
        let g = generate(&spec, &d1).expect("generates");
        generate(&spec, &d2).expect("generates");
        assert_eq!(g.n, 12);
        let m1 = std::fs::read(d1.join("manifest.jsonl")).expect("manifest");
        assert_eq!(
            m1,
            std::fs::read(d2.join("manifest.jsonl")).expect("manifest")
        );
        let m = manifest::load(&d1.join("manifest.jsonl")).expect("valid manifest");
        assert_eq!(m.items.len(), 12);
        for it in &m.items {
            let bytes = std::fs::read(m.resolve(it)).expect("image file");
            assert_eq!(
                bytes,
                std::fs::read(d2.join(&it.image)).expect("second copy")
            );
            let d = auto_crop_codecs::decode(&bytes).expect("decodes");
            assert_eq!((d.raster.width, d.raster.height), (it.width, it.height));
            assert_eq!(it.tags.len(), 5);
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
