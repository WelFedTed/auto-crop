// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The publishing guard for multi-item results (ROADMAP M1.81, extends M1.52): the only shape of a
//! `run --multi` result that may leave the machine. Same policy as [`crate::publish`]: aggregates
//! only, slices with n >= 30 only, no per-scan row, id, path, quad, tag or note.
//!
//! [`PublishableMultiMetrics`] has no field that can hold a scan, and the only way to build one is
//! [`PublishableMultiMetrics::from_results`], which drops suppressed slices.

use crate::multi::{MultiResults, MultiSummary, Routing};
use crate::publish::{FORBIDDEN_KEYS, all_keys};
use crate::report::SliceStatus;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MULTI_METRICS_SCHEMA: &str = "auto-crop-metrics-multi/1";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishableMultiSlice {
    pub n: usize,
    /// `advisory` (30 <= n < 80) or `gated`; never `suppressed`.
    pub status: SliceStatus,
    pub summary: MultiSummary,
    pub held_rate: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishableMultiMetrics {
    pub schema: String,
    pub commit: String,
    pub suite: String,
    pub split: String,
    pub predictor: String,
    pub os: String,
    pub arch: String,
    /// Number of scans.
    pub n: usize,
    pub summary: MultiSummary,
    pub routing: Routing,
    /// Keyed by `axis=value`; contains no slice with n < 30.
    pub slices: BTreeMap<String, PublishableMultiSlice>,
    /// How many slices were withheld for being too small (a count, never their names).
    pub suppressed_slice_count: usize,
}

impl PublishableMultiMetrics {
    pub fn from_results(r: &MultiResults) -> Self {
        let mut slices = BTreeMap::new();
        let mut suppressed = 0;
        for s in &r.slices {
            if s.status == SliceStatus::Suppressed {
                suppressed += 1;
            } else {
                slices.insert(
                    s.key.clone(),
                    PublishableMultiSlice {
                        n: s.n,
                        status: s.status,
                        summary: s.summary.clone(),
                        held_rate: s.held_rate,
                    },
                );
            }
        }
        Self {
            schema: MULTI_METRICS_SCHEMA.to_owned(),
            commit: r.header.commit.clone(),
            suite: r.header.suite.clone(),
            split: r.header.split.clone(),
            predictor: r.header.predictor.clone(),
            os: r.header.os.clone(),
            arch: r.header.arch.clone(),
            n: r.summary.scans,
            summary: r.summary.clone(),
            routing: r.routing.clone(),
            slices,
            suppressed_slice_count: suppressed,
        }
    }

    pub fn to_json(&self) -> String {
        let mut s = serde_json::to_string_pretty(self).expect("metrics serialise");
        s.push('\n');
        s
    }
}

/// The leak check the publisher runs on the serialised artefact: no forbidden key, and no scan id
/// from `results` appears anywhere in the text.
pub fn check_no_leak(json: &str, results: &MultiResults) -> Result<(), String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let mut keys = Vec::new();
    all_keys(&v, &mut keys);
    if let Some(k) = keys.iter().find(|k| FORBIDDEN_KEYS.contains(&k.as_str())) {
        return Err(format!("forbidden key `{k}` in publishable metrics"));
    }
    if let Some(s) = results
        .scans
        .iter()
        .find(|s| json.contains(&format!("\"{}\"", s.id)))
    {
        return Err(format!(
            "a scan id appears in publishable metrics: {}",
            s.id
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multi::{MATCH_IOU, MULTI_SCHEMA};
    use crate::multi::{MultiHeader, MultiSlice, ScanResult, ScanStatus};

    fn scan(i: usize, sep: &str) -> ScanResult {
        ScanResult {
            id: format!("scan-secret-{i:04}"),
            status: ScanStatus::Ok,
            n_gt: 2,
            n_pred: 2,
            matched: 2,
            exact_count: true,
            accepted: i % 3 != 0,
            silent_wrong: false,
            mean_matched_iou: Some(0.97),
            gt_best_iou: vec![0.97, 0.97],
            confidence: Some(0.9),
            reasons: vec![],
            tags: BTreeMap::from([("separation".to_owned(), sep.to_owned())]),
            note: Some("a note".to_owned()),
        }
    }

    fn results() -> MultiResults {
        // 40 separated scans (reportable) and 10 touching ones (suppressed).
        let scans: Vec<ScanResult> = (0..50)
            .map(|i| scan(i, if i < 40 { "separated" } else { "touching" }))
            .collect();
        let refs: Vec<&ScanResult> = scans.iter().collect();
        MultiResults {
            header: MultiHeader {
                schema: MULTI_SCHEMA.to_owned(),
                eval_version: "0".to_owned(),
                commit: "abc".to_owned(),
                os: "linux".to_owned(),
                arch: "x86_64".to_owned(),
                suite: "multi-test".to_owned(),
                split: "all".to_owned(),
                predictor: "items".to_owned(),
                manifest_sha256: "00".to_owned(),
                match_iou: MATCH_IOU,
            },
            summary: crate::multi::summarise(&refs),
            routing: crate::multi::routing(&scans),
            slices: crate::multi::slices(&scans)
                .into_iter()
                .collect::<Vec<MultiSlice>>(),
            scans,
        }
    }

    #[test]
    fn publishable_multi_metrics_have_no_scan_data_and_withhold_small_slices() {
        let r = results();
        let p = PublishableMultiMetrics::from_results(&r);
        let json = p.to_json();
        check_no_leak(&json, &r).expect("clean");
        assert_eq!(p.n, 50);
        assert!(p.slices.contains_key("separation=separated"));
        assert!(!p.slices.contains_key("separation=touching"));
        assert_eq!(p.suppressed_slice_count, 1);
        assert!(!json.contains("touching"), "names of withheld slices leak");
        assert!(!json.contains("scan-secret"));
        assert!(!json.contains("a note"));
        let back: PublishableMultiMetrics = serde_json::from_str(&json).expect("strict parse");
        assert_eq!(back, p);
    }

    #[test]
    fn the_leak_checker_catches_planted_leaks() {
        let r = results();
        let good = PublishableMultiMetrics::from_results(&r).to_json();
        let mut v: serde_json::Value = serde_json::from_str(&good).expect("json");
        v["slices"]["separation=separated"]["summary"]["scans_list"] = serde_json::json!([1]);
        v["slices"]["separation=separated"]["summary"]["rows"] = serde_json::json!([1]);
        assert!(
            check_no_leak(&v.to_string(), &r)
                .expect_err("must flag")
                .contains("rows")
        );
        let mut v: serde_json::Value = serde_json::from_str(&good).expect("json");
        v["extra"] = serde_json::json!(r.scans[3].id);
        assert!(
            check_no_leak(&v.to_string(), &r)
                .expect_err("must flag")
                .contains("scan id")
        );
        // The full local results are flagged wholesale, and the strict reader refuses extras.
        assert!(check_no_leak(&serde_json::to_string(&r).expect("json"), &r).is_err());
        let mut v: serde_json::Value = serde_json::from_str(&good).expect("json");
        v["scans"] = serde_json::json!([]);
        assert!(serde_json::from_value::<PublishableMultiMetrics>(v).is_err());
    }

    #[test]
    fn the_type_has_no_per_scan_field() {
        let json =
            serde_json::to_value(PublishableMultiMetrics::from_results(&results())).expect("json");
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
                "arch",
                "commit",
                "n",
                "os",
                "predictor",
                "routing",
                "schema",
                "slices",
                "split",
                "suite",
                "summary",
                "suppressed_slice_count"
            ]
        );
    }
}
