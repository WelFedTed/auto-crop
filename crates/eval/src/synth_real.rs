// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The `real-cases` suite of the Rust stand-in writer: three families of scenes modelled on failures
//! that real photographs showed and the earlier suites could not (see
//! `docs/perf/detector-baseline.md`, "Update 2026-10-12"). Additive: the `smoke` and `full` suites
//! are untouched and byte-identical.
//!
//! * `fills-frame`: a white receipt (text rows, a barcode) that fills the whole frame; the truth is
//!   the frame.
//! * `wood-desk`: a receipt or page on a strongly grained wood desk, grain lines running as long and
//!   as straight as the page edges; the truth is the paper.
//! * `matted-print`: a black-and-white print on a wide white mat on a black background. The truth
//!   is the PAPER edge (the outer mat), the detector's present policy; whether the user wants the
//!   picture inside the mat instead is an open question, so this family is tagged
//!   `policy = paper-edge` and an IoU against the picture would be about 0.6 to 0.7.
//!
//! Like everything in `synth` this shares a lineage with the detector's unit tests (the scenes come
//! from `auto_crop_imgproc::synth`); it detects regressions, it never measures real accuracy.

use crate::geom::Quad;
use crate::manifest::ManifestItem;
use crate::stats::SplitMix64;
use crate::synth::{Generated, SuiteSpec};
use auto_crop_codecs::{Format, encode};
use auto_crop_imgproc::Raster;
use auto_crop_imgproc::synth::{PaperKind, compose_on_desk, matted_print, wood_desk};
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::path::Path;

pub const FAMILIES: [&str; 3] = ["fills-frame", "wood-desk", "matted-print"];

fn mix(a: u64, b: u64) -> u64 {
    SplitMix64(a ^ b.wrapping_mul(0x9E37_79B9_7F4A_7C15)).next_u64()
}

fn quad_of(c: [(f64, f64); 4]) -> Quad {
    std::array::from_fn(|i| [c[i].0, c[i].1])
}

/// One scene of the suite: the image, the truth and the tags.
pub fn scene(spec: &SuiteSpec, i: usize) -> (Raster, Quad, BTreeMap<String, String>) {
    let (img, truth, tags, _) = scene_with_picture(spec, i);
    (img, truth, tags)
}

/// [`scene`] and, for a matted print, the corners of the picture inside the mat (what a "trim to
/// content" should find); `None` for the other families.
pub fn scene_with_picture(
    spec: &SuiteSpec,
    i: usize,
) -> (Raster, Quad, BTreeMap<String, String>, Option<Quad>) {
    let family = i % FAMILIES.len();
    let mut r = SplitMix64(mix(spec.seed, 0xCA5E ^ i as u64));
    let seed = r.next_u64() >> 8;
    let e = spec.max_edge;
    let mut tags: BTreeMap<String, String> = BTreeMap::new();
    tags.insert("family".to_owned(), FAMILIES[family].to_owned());
    let mut picture: Option<Quad> = None;
    let (img, truth) = match family {
        0 => {
            // Receipt photographs and screenshots: 2:1 to 3.4:1, white to cream, a little noise.
            let aspect = 2.0 + 1.4 * r.unit();
            let (w, h) = ((f64::from(e) / aspect) as u32, e);
            let paper = if r.below(2) == 0 {
                [255, 255, 255]
            } else {
                [
                    (244.0 + 8.0 * r.unit()) as u8,
                    (240.0 + 8.0 * r.unit()) as u8,
                    228,
                ]
            };
            let desk = |_: u32, _: u32| [0.0f32; 3];
            let full = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
            (
                compose_on_desk(
                    (w, h),
                    full,
                    PaperKind::Receipt,
                    paper,
                    seed,
                    (3.0 * r.unit()) as f32,
                    &desk,
                ),
                quad_of(full),
            )
        }
        1 => {
            let receipt = r.below(2) == 0;
            let (w, h) = if receipt {
                (e * 2 / 3, e)
            } else if r.below(2) == 0 {
                (e, e * 3 / 4)
            } else {
                (e * 3 / 4, e)
            };
            let (mx, my) = (0.07 + 0.08 * r.unit(), 0.07 + 0.07 * r.unit());
            let j = |r: &mut SplitMix64| 0.02 * (r.unit() - 0.5);
            let corners = [
                (mx + j(&mut r), my + j(&mut r)),
                (1.0 - mx + j(&mut r), my + j(&mut r)),
                (1.0 - mx + j(&mut r), 1.0 - my + j(&mut r)),
                (mx + j(&mut r), 1.0 - my + j(&mut r)),
            ];
            let grain = (15.0 + 40.0 * r.unit()) as f32;
            let base = [
                (105.0 + 50.0 * r.unit()) as f32,
                (65.0 + 30.0 * r.unit()) as f32,
                (30.0 + 25.0 * r.unit()) as f32,
            ];
            let kind = if receipt {
                PaperKind::Receipt
            } else {
                PaperKind::Document
            };
            let paper = [
                (205.0 + 35.0 * r.unit()) as u8,
                (203.0 + 35.0 * r.unit()) as u8,
                (195.0 + 35.0 * r.unit()) as u8,
            ];
            let desk = wood_desk(h, seed, grain, base);
            tags.insert(
                "aspect".to_owned(),
                if receipt { "receipt" } else { "document" }.to_owned(),
            );
            (
                compose_on_desk(
                    (w, h),
                    corners,
                    kind,
                    paper,
                    seed,
                    (4.0 * r.unit()) as f32,
                    &desk,
                ),
                quad_of(corners),
            )
        }
        _ => {
            let (w, h) = if r.below(2) == 0 {
                (e, e)
            } else {
                (e * 4 / 5, e)
            };
            let mat = 0.03 + 0.03 * r.unit();
            let pic = mat + 0.06 + 0.03 * r.unit();
            let (img, mat_corners, pic_corners) = matted_print((w, h), mat, pic, seed);
            picture = Some(quad_of(pic_corners));
            tags.insert("policy".to_owned(), "paper-edge".to_owned());
            (img, quad_of(mat_corners))
        }
    };
    tags.insert(
        "format".to_owned(),
        if i % 4 < 2 { "png" } else { "jpeg" }.to_owned(),
    );
    (img, truth, tags, picture)
}

/// Writes `images/*` and `manifest.jsonl` under `out`.
pub fn generate(spec: &SuiteSpec, out: &Path) -> Result<Generated, String> {
    let images = out.join("images");
    std::fs::create_dir_all(&images)
        .map_err(|e| format!("cannot create {}: {e}", images.display()))?;
    let rows: Vec<Result<(ManifestItem, u64), String>> = (0..spec.count)
        .into_par_iter()
        .map(|i| {
            let (img, quad, tags) = scene(spec, i);
            let png = tags.get("format").is_some_and(|f| f == "png");
            let (fmt, ext) = if png {
                (Format::Png, "png")
            } else {
                (Format::Jpeg, "jpg")
            };
            let id = format!("{}-{i:05}", spec.name);
            let bytes = encode(&img, fmt, 90, None).map_err(|e| format!("{id}: {e}"))?;
            let rel = format!("images/{id}.{ext}");
            std::fs::write(out.join(&rel), &bytes).map_err(|e| format!("{rel}: {e}"))?;
            let scene_no = i / 2;
            Ok((
                ManifestItem {
                    v: 1,
                    id,
                    image: rel,
                    scene_id: format!("{}-s{scene_no:04}", spec.name),
                    split: Some(
                        if mix(spec.seed, 0x5C11 ^ scene_no as u64) % 10 < 3 {
                            "dev"
                        } else {
                            "test"
                        }
                        .to_owned(),
                    ),
                    width: img.width,
                    height: img.height,
                    quad,
                    items: Vec::new(),
                    curves: None,
                    items_curves: Vec::new(),
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

    fn spec(count: usize) -> SuiteSpec {
        SuiteSpec {
            name: "real-cases".to_owned(),
            count,
            max_edge: 240,
            seed: 0x5EED_A070_C20B_0002,
        }
    }

    #[test]
    fn families_alternate_and_are_deterministic() {
        let s = spec(6);
        for i in 0..6 {
            let (a, qa, ta) = scene(&s, i);
            let (b, qb, tb) = scene(&s, i);
            assert_eq!(a, b);
            assert_eq!(qa, qb);
            assert_eq!(ta, tb);
            assert_eq!(ta["family"], FAMILIES[i % 3]);
        }
    }

    #[test]
    fn the_truth_quads_are_clockwise_and_inside_the_frame() {
        let s = spec(12);
        for i in 0..12 {
            let (img, q, _) = scene(&s, i);
            assert!(crate::geom::signed_area(&q) > 0.0, "scene {i}: {q:?}");
            assert!(img.width > 0 && img.height > 0);
            for p in q {
                assert!(
                    (-0.001..=1.001).contains(&p[0]) && (-0.001..=1.001).contains(&p[1]),
                    "{p:?}"
                );
            }
        }
    }
}
