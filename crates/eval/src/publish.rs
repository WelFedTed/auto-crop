// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The publishing guard (ROADMAP M1.52, type-level part): the only shape of result that may leave
//! the machine. Golden-set policy (B21): aggregates only, slices with n >= 30 only, no per-image
//! rows, paths, ids, quads or thumbnails.
//!
//! The guard is the type: [`PublishableMetrics`] has no field that can hold an image, and the only
//! way to build one is [`PublishableMetrics::from_results`], which drops suppressed slices. The
//! workflow that publishes (M1.51/M1.52) must serialise this type and nothing else.

use crate::calib::Calibration;
use crate::report::{Host, Results, SliceStatus, Summary, Worst, WorstSlice};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const METRICS_SCHEMA: &str = "auto-crop-metrics/1";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishableSlice {
    pub n: usize,
    /// `advisory` (30 <= n < 80) or `gated`; never `suppressed`.
    pub status: SliceStatus,
    pub summary: Summary,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishableMetrics {
    pub schema: String,
    pub commit: String,
    pub suite: String,
    pub split: String,
    pub predictor: String,
    pub host: Host,
    pub n: usize,
    pub summary: Summary,
    /// Keyed by `axis=value`; contains no slice with n < 30.
    pub slices: BTreeMap<String, PublishableSlice>,
    pub worst_slice: Worst,
    pub calibration: Option<Calibration>,
    /// How many slices were withheld for being too small (a count, never their names).
    pub suppressed_slice_count: usize,
}

impl PublishableMetrics {
    pub fn from_results(r: &Results) -> Self {
        let mut slices = BTreeMap::new();
        let mut suppressed = 0;
        for s in &r.slices {
            if s.status == SliceStatus::Suppressed {
                suppressed += 1;
            } else {
                slices.insert(
                    s.key.clone(),
                    PublishableSlice {
                        n: s.n,
                        status: s.status,
                        summary: s.summary.clone(),
                    },
                );
            }
        }
        // `worst_slice` is computed over reportable slices only, but recheck so a hand-edited
        // results file cannot smuggle a suppressed slice's name through it.
        let keep = |w: &Option<WorstSlice>| {
            w.clone()
                .filter(|w| w.n >= crate::stats::MIN_PUBLIC_N && slices.contains_key(&w.key))
        };
        let (worst_iou, worst_fail) = (
            keep(&r.worst_slice.by_mean_iou),
            keep(&r.worst_slice.by_failure_rate),
        );
        Self {
            schema: METRICS_SCHEMA.to_owned(),
            commit: r.header.commit.clone(),
            suite: r.header.suite.clone(),
            split: r.header.split.clone(),
            predictor: r.header.predictor.clone(),
            host: r.header.host.clone(),
            n: r.summary.n,
            summary: r.summary.clone(),
            slices,
            worst_slice: Worst {
                by_mean_iou: worst_iou,
                by_failure_rate: worst_fail,
            },
            calibration: r.calibration.clone(),
            suppressed_slice_count: suppressed,
        }
    }

    pub fn to_json(&self) -> String {
        let mut s = serde_json::to_string_pretty(self).expect("metrics serialise");
        s.push('\n');
        s
    }
}

/// JSON keys that name or locate one image or carry per-image data. None may appear anywhere in a
/// publishable artefact.
pub const FORBIDDEN_KEYS: &[&str] = &[
    "id",
    "image",
    "images",
    "path",
    "paths",
    "quad",
    "rows",
    "per_image",
    "tags",
    "note",
    "iou",
    "thumbnail",
    "filename",
    "file",
];

/// Every object key in a JSON value, recursively.
pub fn all_keys(v: &serde_json::Value, out: &mut Vec<String>) {
    match v {
        serde_json::Value::Object(m) => {
            for (k, x) in m {
                out.push(k.clone());
                all_keys(x, out);
            }
        }
        serde_json::Value::Array(a) => a.iter().for_each(|x| all_keys(x, out)),
        _ => {}
    }
}

/// The leak check the publisher runs on the serialised artefact: no forbidden key, and no
/// per-image id from `results` appears anywhere in the text.
pub fn check_no_leak(json: &str, results: &Results) -> Result<(), String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let mut keys = Vec::new();
    all_keys(&v, &mut keys);
    if let Some(k) = keys.iter().find(|k| FORBIDDEN_KEYS.contains(&k.as_str())) {
        return Err(format!("forbidden key `{k}` in publishable metrics"));
    }
    if let Some(i) = results
        .images
        .iter()
        .find(|i| json.contains(&format!("\"{}\"", i.id)))
    {
        return Err(format!(
            "an image id appears in publishable metrics: {}",
            i.id
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::predictor::{FullFrame, Oracle};
    use crate::run::testing::parallelogram_manifest;
    use crate::run::{RunConfig, run};

    fn results() -> Results {
        // 300 images: tilt thirds (100) and lighting halves (150) are reportable.
        let m = parallelogram_manifest(300, 21);
        run(&m, &FullFrame, &RunConfig::default()).expect("runs")
    }

    #[test]
    fn publishable_metrics_have_no_per_image_data_and_no_ids() {
        let r = results();
        let p = PublishableMetrics::from_results(&r);
        let json = p.to_json();
        check_no_leak(&json, &r).expect("clean");
        assert_eq!(p.n, 300);
        assert_eq!(p.schema, METRICS_SCHEMA);
        assert!(p.slices.contains_key("lighting=dim"));
        // It also round-trips through its own strict reader.
        let back: PublishableMetrics = serde_json::from_str(&json).expect("strict parse");
        assert_eq!(back, p);
    }

    #[test]
    fn leak_negative_tests_the_checker_catches_planted_leaks() {
        let r = results();
        let good = PublishableMetrics::from_results(&r).to_json();
        // A forbidden key anywhere, however deep.
        let mut v: serde_json::Value = serde_json::from_str(&good).expect("json");
        v["slices"]["lighting=dim"]["summary"]["images"] = serde_json::json!([1, 2]);
        assert!(
            check_no_leak(&v.to_string(), &r)
                .expect_err("must flag")
                .contains("images")
        );
        // An id smuggled in as a value.
        let mut v: serde_json::Value = serde_json::from_str(&good).expect("json");
        v["note_text"] = serde_json::json!(r.images[3].id);
        assert!(
            check_no_leak(&v.to_string(), &r)
                .expect_err("must flag")
                .contains("image id")
        );
        // Per-image results themselves are flagged wholesale.
        let full = serde_json::to_string(&r).expect("serialises");
        assert!(check_no_leak(&full, &r).is_err());
        // The strict reader refuses unknown fields, so a widened payload cannot parse as the type.
        let mut v: serde_json::Value = serde_json::from_str(&good).expect("json");
        v["images"] = serde_json::json!([]);
        assert!(serde_json::from_value::<PublishableMetrics>(v).is_err());
    }

    #[test]
    fn small_slices_are_withheld_by_construction() {
        // 60 images: tilt thirds have 20 each (suppressed), lighting halves 30 (advisory).
        let m = parallelogram_manifest(60, 22);
        let r = run(&m, &Oracle::from_manifest(&m), &RunConfig::default()).expect("runs");
        assert!(r.slices.iter().any(|s| s.status == SliceStatus::Suppressed));
        let p = PublishableMetrics::from_results(&r);
        assert!(
            p.slices.keys().all(|k| !k.starts_with("tilt=")),
            "{:?}",
            p.slices.keys()
        );
        assert_eq!(p.slices.len(), 2);
        assert!(p.slices.values().all(|s| s.n >= 30));
        assert_eq!(p.suppressed_slice_count, 3);
        // Neither the names of withheld slices nor a worst-slice pointing at one survive.
        let json = p.to_json();
        assert!(!json.contains("tilt=0-10"));
        // Tamper: a results file whose worst_slice names a suppressed slice is cleaned.
        let mut tampered = r.clone();
        tampered.worst_slice.by_mean_iou = Some(WorstSlice {
            key: "tilt=0-10".to_owned(),
            n: 20,
            status: SliceStatus::Suppressed,
            value: 0.0,
        });
        let p = PublishableMetrics::from_results(&tampered);
        assert!(p.worst_slice.by_mean_iou.is_none());
        assert!(!p.to_json().contains("tilt=0-10"));
    }

    #[test]
    fn the_type_has_no_per_image_field() {
        // Guards against someone adding a per-image field to the type: every top-level key of the
        // serialised value must be on this allow-list.
        let json: serde_json::Value =
            serde_json::to_value(PublishableMetrics::from_results(&results())).expect("json");
        let mut keys: Vec<&str> = json
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "calibration",
                "commit",
                "host",
                "n",
                "predictor",
                "schema",
                "slices",
                "split",
                "suite",
                "summary",
                "suppressed_slice_count",
                "worst_slice"
            ]
        );
    }
}
