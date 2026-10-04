// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `noise-floor`: how much two human annotators disagree (ROADMAP M1.43, harness half).
//!
//! No accuracy target may be tighter than the 95th percentile of annotator disagreement, because
//! below that the "ground truth" is itself noise. Inputs are two label files in JSON-lines form,
//! one `{"id":..,"width":..,"height":..,"quad":[[x,y]x4]}` per image (0..1 EXIF-oriented
//! coordinates); extra fields are ignored, so a manifest works as a label file.

use crate::geom::{self, Quad};
use crate::metrics;
use crate::stats::{mean, quantile};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Deserialize)]
pub struct LabelRow {
    pub id: String,
    pub width: u32,
    pub height: u32,
    pub quad: Quad,
}

pub fn parse_labels(text: &str) -> Result<BTreeMap<String, LabelRow>, String> {
    let mut out = BTreeMap::new();
    for (no, line) in text
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
    {
        let row: LabelRow =
            serde_json::from_str(line).map_err(|e| format!("line {}: {e}", no + 1))?;
        if !geom::quad_is_finite(&row.quad) || row.width == 0 || row.height == 0 {
            return Err(format!("line {}: non-finite quad or zero size", no + 1));
        }
        if out.insert(row.id.clone(), row).is_some() {
            return Err(format!("line {}: duplicate id", no + 1));
        }
    }
    Ok(out)
}

/// Label rows from either a JSON-lines file or a directory of golden label files
/// (`<image>.json`, [`crate::golden`]). From a directory only blank-quad labels of single-item
/// images are used (assisted labels, negatives and multi-item images are left out, because the
/// two annotators' item order is not comparable), and the second return value counts what was
/// left out. Label files that do not parse are an error, never silently skipped.
pub fn load_labels(path: &std::path::Path) -> Result<(BTreeMap<String, LabelRow>, usize), String> {
    if !path.is_dir() {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        return Ok((parse_labels(&text)?, 0));
    }
    let mut out = BTreeMap::new();
    let mut left_out = 0;
    for lf in crate::golden::read_label_dir(path)? {
        let l = lf
            .label
            .map_err(|e| format!("{}: {e}", lf.path.display()))?;
        let found = crate::golden::validate_label(&l, Some(&lf.stem));
        if !found.errors.is_empty() {
            return Err(format!(
                "{}: {}",
                lf.path.display(),
                found.errors.join("; ")
            ));
        }
        if l.assisted || l.items.len() != 1 {
            left_out += 1;
            continue;
        }
        out.insert(
            l.id.clone(),
            LabelRow {
                id: l.id,
                width: l.width,
                height: l.height,
                quad: l.items[0].quad,
            },
        );
    }
    Ok((out, left_out))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NoiseFloor {
    /// Images labelled by both annotators.
    pub n: usize,
    pub only_in_a: usize,
    pub only_in_b: usize,
    /// Quads that could not be scored against each other (counted as IoU 0).
    pub unscorable: usize,
    pub iou_mean: Option<f64>,
    pub iou_median: Option<f64>,
    /// The 5th percentile of IoU: the worst-disagreeing 5% of images.
    pub iou_p05: Option<f64>,
    pub corner_err_pct_median: Option<f64>,
    pub corner_err_pct_p95: Option<f64>,
    pub skew_deg_median: Option<f64>,
    pub skew_deg_p95: Option<f64>,
}

/// Disagreement between annotators A and B. IoU is averaged over both directions, since the
/// canonical warp is anchored on one quad; corner error and skew use A as the reference.
pub fn noise_floor(a: &BTreeMap<String, LabelRow>, b: &BTreeMap<String, LabelRow>) -> NoiseFloor {
    let (mut ious, mut corner, mut skew) = (Vec::new(), Vec::new(), Vec::new());
    let mut unscorable = 0;
    for (id, ra) in a {
        let Some(rb) = b.get(id) else { continue };
        let ab = metrics::score(&ra.quad, &rb.quad, ra.width, ra.height);
        let ba = metrics::canonical_iou(&rb.quad, &ra.quad);
        match (ab, ba) {
            (Ok(s), Ok(rev)) => {
                ious.push((s.iou + rev) / 2.0);
                corner.push(s.corner_err_pct);
                skew.push(s.skew_deg);
            }
            _ => {
                unscorable += 1;
                ious.push(0.0);
            }
        }
    }
    let some = |v: &[f64], f: &dyn Fn(&[f64]) -> f64| (!v.is_empty()).then(|| f(v));
    NoiseFloor {
        n: ious.len(),
        only_in_a: a.keys().filter(|k| !b.contains_key(*k)).count(),
        only_in_b: b.keys().filter(|k| !a.contains_key(*k)).count(),
        unscorable,
        iou_mean: some(&ious, &mean),
        iou_median: some(&ious, &|v| quantile(v, 0.5)),
        iou_p05: some(&ious, &|v| quantile(v, 0.05)),
        corner_err_pct_median: some(&corner, &|v| quantile(v, 0.5)),
        corner_err_pct_p95: some(&corner, &|v| quantile(v, 0.95)),
        skew_deg_median: some(&skew, &|v| quantile(v, 0.5)),
        skew_deg_p95: some(&skew, &|v| quantile(v, 0.95)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label(id: &str, quad: Quad) -> String {
        format!(
            "{{\"id\":\"{id}\",\"width\":1000,\"height\":1000,\"quad\":{}}}",
            serde_json::to_string(&quad).expect("serialises")
        )
    }

    #[test]
    fn identical_annotators_have_zero_disagreement() {
        let q: Quad = [[0.1, 0.1], [0.9, 0.12], [0.88, 0.9], [0.12, 0.88]];
        let text = (0..40)
            .map(|i| label(&format!("i{i}"), q))
            .collect::<Vec<_>>()
            .join("\n");
        let a = parse_labels(&text).expect("valid");
        let nf = noise_floor(&a, &a);
        assert_eq!(nf.n, 40);
        assert!((nf.iou_median.expect("n > 0") - 1.0).abs() < 1e-12);
        assert!(nf.corner_err_pct_p95.expect("n > 0") < 1e-12);
    }

    #[test]
    fn known_pixel_offsets_give_known_corner_error_and_p95() {
        // B is A shifted by k px right, k = 0..39 on a 1000 x 1000 image (diagonal 1414.2 px).
        let base: Quad = [[0.2, 0.2], [0.8, 0.2], [0.8, 0.8], [0.2, 0.8]];
        let (mut ta, mut tb) = (Vec::new(), Vec::new());
        for k in 0..40 {
            let id = format!("i{k:02}");
            ta.push(label(&id, base));
            let shifted: Quad =
                std::array::from_fn(|i| [base[i][0] + f64::from(k) / 1000.0, base[i][1]]);
            tb.push(label(&id, shifted));
        }
        let nf = noise_floor(
            &parse_labels(&ta.join("\n")).expect("valid"),
            &parse_labels(&tb.join("\n")).expect("valid"),
        );
        let diag = 1000.0f64.hypot(1000.0);
        // Offsets are 0, 1, ..., 39 px: median 19.5 px; p95 = 0 + 0.95 * 39 = 37.05 px.
        assert!((nf.corner_err_pct_median.expect("n > 0") - 100.0 * 19.5 / diag).abs() < 1e-9);
        assert!((nf.corner_err_pct_p95.expect("n > 0") - 100.0 * 37.05 / diag).abs() < 1e-9);
        assert!(nf.skew_deg_p95.expect("n > 0") < 1e-9);
        assert!(nf.iou_p05.expect("n > 0") < nf.iou_median.expect("n > 0"));
    }

    #[test]
    fn label_directories_load_blank_single_item_labels_only() {
        use crate::golden::{GoldenItem, GoldenLabel, label_to_json, new_label};
        let dir = tempfile::tempdir().expect("tempdir");
        let q: Quad = [[0.1, 0.1], [0.9, 0.1], [0.9, 0.9], [0.1, 0.9]];
        let write = |name: &str, f: &dyn Fn(&mut GoldenLabel)| {
            let mut l = new_label(name, &"b".repeat(64), 100, 80);
            l.slices = vec!["flatbed-single".to_owned()];
            l.items = vec![GoldenItem::new(q)];
            f(&mut l);
            std::fs::write(dir.path().join(format!("{name}.json")), label_to_json(&l))
                .expect("write");
        };
        write("one.jpg", &|_| {});
        write("assisted.jpg", &|l| l.assisted = true);
        write("multi.jpg", &|l| l.items.push(GoldenItem::new(q)));
        let (rows, left_out) = load_labels(dir.path()).expect("loads");
        assert_eq!(rows.keys().collect::<Vec<_>>(), ["one.jpg"]);
        assert_eq!(left_out, 2);
        // The same rows serve as annotator B: identical quads, no disagreement.
        let nf = noise_floor(&rows, &rows);
        assert_eq!(nf.n, 1);
        assert!((nf.iou_median.expect("n") - 1.0).abs() < 1e-12);
        // A file path still means JSON lines.
        let jl = dir.path().join("l.jsonl");
        std::fs::write(&jl, label("x", q)).expect("write");
        assert_eq!(load_labels(&jl).expect("loads").0.len(), 1);
        // A broken label is an error, not a silent skip.
        std::fs::write(dir.path().join("broken.json"), "{").expect("write");
        assert!(load_labels(dir.path()).is_err());
    }

    #[test]
    fn unmatched_and_bad_rows_are_reported() {
        let q: Quad = [[0.1, 0.1], [0.9, 0.1], [0.9, 0.9], [0.1, 0.9]];
        let a = parse_labels(&format!("{}\n{}", label("x", q), label("y", q))).expect("valid");
        let b = parse_labels(&format!("{}\n{}", label("y", q), label("z", q))).expect("valid");
        let nf = noise_floor(&a, &b);
        assert_eq!((nf.n, nf.only_in_a, nf.only_in_b), (1, 1, 1));
        assert!(parse_labels("{\"id\":\"a\"}").is_err());
        assert!(
            parse_labels(&format!("{}\n{}", label("x", q), label("x", q)))
                .expect_err("dup")
                .contains("duplicate")
        );
        let empty = noise_floor(&BTreeMap::new(), &BTreeMap::new());
        assert_eq!((empty.n, empty.iou_median), (0, None));
    }
}
