// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Per-item hand-off to the single-item pipeline (ROADMAP M10.14): an accepted item is cropped
//! out with a margin and the single-item detector (`detect::detect`) runs on the crop, so the
//! outline can be refined at the source's own resolution and cross-checked by an independent
//! method. The result is only offered when the two agree (IoU 0.9): a disagreement is a reason
//! to hold the item, never a reason to trust either.

use super::geom::convex_iou;
use super::{ItemCandidate, ItemKind};
use crate::Raster;
use crate::detect::detect;
use auto_crop_core::Pt;

/// The crop of `src` around `quad` (normalised) with `margin` (a share of the larger side of the
/// quad's bounding box) on every side; returns the crop and its offset and size in `src` pixels.
pub fn crop_for_item(src: &Raster, quad: &[Pt; 4], margin: f64) -> Option<(Raster, [u32; 4])> {
    let (w, h) = (f64::from(src.width), f64::from(src.height));
    let xs = quad.iter().map(|p| p.x * w);
    let ys = quad.iter().map(|p| p.y * h);
    let (x0, x1) = (
        xs.clone().fold(f64::MAX, f64::min),
        xs.fold(f64::MIN, f64::max),
    );
    let (y0, y1) = (
        ys.clone().fold(f64::MAX, f64::min),
        ys.fold(f64::MIN, f64::max),
    );
    let m = margin * (x1 - x0).max(y1 - y0);
    let (cx0, cy0) = ((x0 - m).floor().max(0.0), (y0 - m).floor().max(0.0));
    let (cx1, cy1) = ((x1 + m).ceil().min(w), (y1 + m).ceil().min(h));
    let (cw, ch) = ((cx1 - cx0) as u32, (cy1 - cy0) as u32);
    if cw < 16 || ch < 16 {
        return None;
    }
    let (ox, oy) = (cx0 as u32, cy0 as u32);
    let mut out = Raster::new(cw, ch);
    for y in 0..ch {
        let s = (((oy + y) * src.width + ox) * 3) as usize;
        let d = (y * cw * 3) as usize;
        out.data[d..d + (cw * 3) as usize].copy_from_slice(&src.data[s..s + (cw * 3) as usize]);
    }
    Some((out, [ox, oy, cw, ch]))
}

/// Runs the single-item detector on the item's crop (15% margin). Returns its quad, in `src`
/// coordinates, when it found one that agrees with the candidate (IoU at least 0.9); `None` for
/// a cluster, a missing crop, no quad or a disagreement.
pub fn refine_with_detect(src: &Raster, cand: &ItemCandidate) -> Option<[Pt; 4]> {
    if cand.kind != ItemKind::Rect {
        return None;
    }
    let (crop, [ox, oy, cw, ch]) = crop_for_item(src, &cand.quad, 0.15)?;
    let det = detect(&crop);
    let q = det.quad?;
    let (w, h) = (f64::from(src.width), f64::from(src.height));
    let mapped: [Pt; 4] = std::array::from_fn(|i| {
        Pt::new(
            (f64::from(ox) + q[i].x * f64::from(cw)) / w,
            (f64::from(oy) + q[i].y * f64::from(ch)) / h,
        )
    });
    let px = |q: &[Pt; 4]| -> Vec<(f64, f64)> { q.iter().map(|p| (p.x * w, p.y * h)).collect() };
    (convex_iou(&px(&mapped), &px(&cand.quad)) >= 0.9).then_some(mapped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::{ItemsOptions, detect_items};

    #[test]
    fn the_single_item_detector_agrees_with_a_clean_candidate() {
        let mut img = Raster::filled(480, 360, [30, 34, 30]);
        for y in 90..260u32 {
            for x in 120..380u32 {
                let v = 200 + ((x * 3 + y * 5) % 20) as u8;
                img.set_pixel(x, y, [v, v - 10, v - 30]);
            }
        }
        let det = detect_items(&img, &ItemsOptions::default());
        assert_eq!(det.items.len(), 1);
        let cand = &det.items[0];
        let (crop, [ox, oy, cw, ch]) = crop_for_item(&img, &cand.quad, 0.15).expect("crop");
        assert_eq!((crop.width, crop.height), (cw, ch));
        assert!(ox < 120 && oy < 90);
        if let Some(q) = refine_with_detect(&img, cand) {
            let c = q.iter().map(|p| p.x).sum::<f64>() / 4.0 * 480.0;
            assert!((c - 250.0).abs() < 6.0, "{c}");
        }
    }
}
