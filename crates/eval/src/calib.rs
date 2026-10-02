// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Confidence calibration metrics (ROADMAP M1.48, PLAN 7.6): risk-coverage curve, reliability
//! diagram with 10 equal-mass bins, ECE, Brier score and AUROC of confidence against failure.
//! Reported, not gated until M4.
//!
//! Inputs are `(confidence, success)` pairs, where `success` means "acceptable" by the failure
//! definition (IoU at or above the failure line). A missing prediction enters as confidence 0 and
//! a failure, so the curve cannot be improved by crashing.

use crate::stats::wilson_95;
use serde::{Deserialize, Serialize};

pub const RELIABILITY_BINS: usize = 10;
/// The risk-coverage curve is thinned to at most this many points.
pub const MAX_CURVE_POINTS: usize = 101;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReliabilityBin {
    pub n: usize,
    pub confidence_mean: f64,
    /// Fraction of the bin that was acceptable.
    pub accuracy: f64,
    pub wilson_lo: f64,
    pub wilson_hi: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RiskCoveragePoint {
    /// Fraction of images kept (confidence at or above the threshold).
    pub coverage: f64,
    /// Failure rate among the kept images.
    pub risk: f64,
    pub threshold: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Calibration {
    pub n: usize,
    pub ece: f64,
    pub brier: f64,
    /// Probability that a random acceptable image outranks a random failure; `None` if either
    /// class is empty.
    pub auroc: Option<f64>,
    pub bins: Vec<ReliabilityBin>,
    pub risk_coverage: Vec<RiskCoveragePoint>,
}

/// Equal-mass reliability bins: items sorted by confidence (ties by input order) and cut into
/// `bins` contiguous chunks of as equal size as possible.
pub fn reliability_bins(data: &[(f64, bool)], bins: usize) -> Vec<ReliabilityBin> {
    let n = data.len();
    let bins = bins.min(n);
    if bins == 0 {
        return Vec::new();
    }
    let mut idx: Vec<usize> = (0..n).collect();
    idx.sort_by(|&a, &b| data[a].0.total_cmp(&data[b].0).then(a.cmp(&b)));
    (0..bins)
        .map(|b| {
            let chunk = &idx[b * n / bins..(b + 1) * n / bins];
            let m = chunk.len();
            let conf = chunk.iter().map(|&i| data[i].0).sum::<f64>() / m as f64;
            let ok = chunk.iter().filter(|&&i| data[i].1).count();
            let (lo, hi) = wilson_95(ok as u64, m as u64);
            ReliabilityBin {
                n: m,
                confidence_mean: conf,
                accuracy: ok as f64 / m as f64,
                wilson_lo: lo,
                wilson_hi: hi,
            }
        })
        .collect()
}

/// Expected calibration error: the bin-size-weighted mean of |accuracy - confidence|.
pub fn ece(bins: &[ReliabilityBin]) -> f64 {
    let n: usize = bins.iter().map(|b| b.n).sum();
    if n == 0 {
        return f64::NAN;
    }
    bins.iter()
        .map(|b| b.n as f64 * (b.accuracy - b.confidence_mean).abs())
        .sum::<f64>()
        / n as f64
}

/// Mean squared difference between confidence and the 0/1 outcome.
pub fn brier(data: &[(f64, bool)]) -> f64 {
    if data.is_empty() {
        return f64::NAN;
    }
    data.iter()
        .map(|&(c, ok)| {
            let y = if ok { 1.0 } else { 0.0 };
            (c - y) * (c - y)
        })
        .sum::<f64>()
        / data.len() as f64
}

/// AUROC of confidence as a score for "acceptable" (equivalently, of low confidence as a detector
/// of failures), with ties counting one half: the Mann-Whitney U statistic over average ranks.
pub fn auroc(data: &[(f64, bool)]) -> Option<f64> {
    let pos = data.iter().filter(|d| d.1).count();
    let neg = data.len() - pos;
    if pos == 0 || neg == 0 {
        return None;
    }
    let mut idx: Vec<usize> = (0..data.len()).collect();
    idx.sort_by(|&a, &b| data[a].0.total_cmp(&data[b].0));
    let mut rank_sum_pos = 0.0;
    let mut i = 0;
    while i < idx.len() {
        let mut j = i;
        while j + 1 < idx.len() && data[idx[j + 1]].0 == data[idx[i]].0 {
            j += 1;
        }
        // Average of the 1-based ranks i+1 ..= j+1.
        let avg = ((i + 1 + j + 1) as f64) / 2.0;
        for &k in &idx[i..=j] {
            if data[k].1 {
                rank_sum_pos += avg;
            }
        }
        i = j + 1;
    }
    let (p, q) = (pos as f64, neg as f64);
    Some((rank_sum_pos - p * (p + 1.0) / 2.0) / (p * q))
}

/// The selective-classification curve: for every distinct confidence threshold (highest first),
/// the coverage kept and the failure rate among the kept images. Thinned to at most
/// [`MAX_CURVE_POINTS`] evenly spaced points, always keeping the first and the last.
pub fn risk_coverage(data: &[(f64, bool)]) -> Vec<RiskCoveragePoint> {
    let n = data.len();
    if n == 0 {
        return Vec::new();
    }
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| data[b].0.total_cmp(&data[a].0));
    let mut points = Vec::new();
    let (mut kept, mut failed) = (0usize, 0usize);
    let mut i = 0;
    while i < n {
        let t = data[order[i]].0;
        while i < n && data[order[i]].0 == t {
            kept += 1;
            if !data[order[i]].1 {
                failed += 1;
            }
            i += 1;
        }
        points.push(RiskCoveragePoint {
            coverage: kept as f64 / n as f64,
            risk: failed as f64 / kept as f64,
            threshold: t,
        });
    }
    if points.len() <= MAX_CURVE_POINTS {
        return points;
    }
    let m = points.len();
    (0..MAX_CURVE_POINTS)
        .map(|j| points[j * (m - 1) / (MAX_CURVE_POINTS - 1)].clone())
        .collect()
}

pub fn calibration(data: &[(f64, bool)]) -> Calibration {
    let bins = reliability_bins(data, RELIABILITY_BINS);
    Calibration {
        n: data.len(),
        ece: ece(&bins),
        brier: brier(data),
        auroc: auroc(data),
        bins,
        risk_coverage: risk_coverage(data),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::SplitMix64;

    #[test]
    fn a_calibrated_predictor_has_ece_near_zero() {
        // Outcome drawn as Bernoulli(confidence): calibrated by construction.
        let mut r = SplitMix64(7);
        let data: Vec<(f64, bool)> = (0..60_000)
            .map(|_| {
                let c = r.unit();
                (c, r.unit() < c)
            })
            .collect();
        let cal = calibration(&data);
        assert!(cal.ece < 0.01, "ece {}", cal.ece);
        assert_eq!(cal.bins.len(), 10);
        assert!(cal.bins.iter().all(|b| b.n == 6000));
        // Brier of a calibrated uniform-confidence predictor is E[c(1-c)] = 1/6.
        assert!((cal.brier - 1.0 / 6.0).abs() < 0.005, "brier {}", cal.brier);
        // AUROC for this construction: positives have density 2c, negatives 2(1-c), so
        // P(c_pos > c_neg) = 4/3 - 1/2 = 5/6.
        let a = cal.auroc.expect("both classes");
        assert!((a - 5.0 / 6.0).abs() < 0.01, "auroc {a}");
    }

    #[test]
    fn an_overconfident_predictor_matches_the_analytic_ece_and_brier() {
        // Says 0.95 every time but is right 70% of the time.
        let data: Vec<(f64, bool)> = (0..1000).map(|i| (0.95, i % 10 < 7)).collect();
        let cal = calibration(&data);
        assert!((cal.ece - 0.25).abs() < 1e-12, "ece {}", cal.ece);
        let brier = 0.7 * 0.05f64.powi(2) + 0.3 * 0.95f64.powi(2);
        assert!((cal.brier - brier).abs() < 1e-12);
        // All confidences tie, so AUROC is exactly 0.5 (no information).
        assert_eq!(cal.auroc, Some(0.5));
        // Two-level overconfidence: half the images at 0.9 (right 60%), half at 0.6 (right 50%).
        let mut v = Vec::new();
        for i in 0..500 {
            v.push((0.9, i % 10 < 6));
            v.push((0.6, i % 10 < 5));
        }
        let cal = calibration(&v);
        let want = 0.5 * (0.9f64 - 0.6).abs() + 0.5 * (0.6f64 - 0.5).abs();
        assert!((cal.ece - want).abs() < 1e-12, "{} vs {want}", cal.ece);
    }

    #[test]
    fn perfect_ranking_has_auroc_one_and_a_zero_risk_curve_head() {
        let data = vec![(0.9, true), (0.8, true), (0.3, false), (0.2, false)];
        assert_eq!(auroc(&data), Some(1.0));
        let rc = risk_coverage(&data);
        assert_eq!(rc.len(), 4);
        assert_eq!(rc[0].risk, 0.0);
        assert_eq!(rc[1].risk, 0.0);
        assert!((rc[1].coverage - 0.5).abs() < 1e-15);
        assert!((rc[3].risk - 0.5).abs() < 1e-15 && rc[3].coverage == 1.0);
        let reversed: Vec<(f64, bool)> = data.iter().map(|&(c, ok)| (1.0 - c, ok)).collect();
        assert_eq!(auroc(&reversed), Some(0.0));
        assert_eq!(auroc(&[(0.5, true)]), None);
    }

    #[test]
    fn auroc_matches_a_brute_force_pair_count_with_ties() {
        let mut r = SplitMix64(3);
        let data: Vec<(f64, bool)> = (0..300)
            .map(|_| (f64::from((r.next_u64() % 12) as u32) / 11.0, r.unit() < 0.6))
            .collect();
        let (mut wins, mut pairs) = (0.0, 0.0);
        for a in &data {
            for b in &data {
                if a.1 && !b.1 {
                    pairs += 1.0;
                    wins += if a.0 > b.0 {
                        1.0
                    } else if a.0 == b.0 {
                        0.5
                    } else {
                        0.0
                    };
                }
            }
        }
        assert!((auroc(&data).expect("both classes") - wins / pairs).abs() < 1e-12);
    }

    #[test]
    fn risk_coverage_groups_ties_and_thins_long_curves() {
        // Ties are kept or dropped together.
        let data = vec![(1.0, true), (1.0, false), (0.5, true)];
        let rc = risk_coverage(&data);
        assert_eq!(rc.len(), 2);
        assert!((rc[0].coverage - 2.0 / 3.0).abs() < 1e-15 && (rc[0].risk - 0.5).abs() < 1e-15);
        // 1000 distinct confidences thin to 101 points, ending at full coverage.
        let many: Vec<(f64, bool)> = (0..1000).map(|i| (i as f64 / 1000.0, i % 7 != 0)).collect();
        let rc = risk_coverage(&many);
        assert_eq!(rc.len(), MAX_CURVE_POINTS);
        assert_eq!(rc.last().expect("non-empty").coverage, 1.0);
        assert!(rc.windows(2).all(|w| w[0].coverage < w[1].coverage));
    }

    #[test]
    fn bins_are_equal_mass_and_handle_small_inputs() {
        let data: Vec<(f64, bool)> = (0..25).map(|i| (i as f64 / 25.0, true)).collect();
        let bins = reliability_bins(&data, 10);
        assert_eq!(bins.iter().map(|b| b.n).sum::<usize>(), 25);
        assert!(bins.iter().all(|b| b.n == 2 || b.n == 3));
        assert!(reliability_bins(&[], 10).is_empty());
        assert_eq!(reliability_bins(&data[..3], 10).len(), 3);
        assert!(ece(&[]).is_nan());
    }
}
