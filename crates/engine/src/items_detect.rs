// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The seam to multi-item detection (ROADMAP M10.01-M10.16, built in `imgproc::items`). The
//! engine asks an [`ItemDetector`] for the items on a scan and never looks inside it, so the
//! detector can be swapped (a stub in tests, the classical detector in the app, a net later) and
//! the engine's behaviour is defined by what it does with the answer: two or more items make a
//! split; one or none is the ordinary single-item route (M10.13).

use auto_crop_core::{Confidence, Pt, SplitPolicy, SplitProfile};
use auto_crop_imgproc::Raster;

/// One item the detector accepted, with its own confidence (every hold code it raised is in
/// `confidence.reasons`).
#[derive(Debug, Clone)]
pub struct DetectedItem {
    /// TL, TR, BR, BL, normalised in the EXIF-oriented raster.
    pub quad: [Pt; 4],
    pub confidence: Confidence,
    /// Clockwise quarter turns that make it upright (Receipts profile only; 0 otherwise).
    pub quarter_turns: u8,
}

/// What the detector found on a scan.
#[derive(Debug, Clone, Default)]
pub struct SplitDetection {
    /// Accepted items, in any order (the engine puts them in reading order).
    pub items: Vec<DetectedItem>,
    /// Quads it considered and rejected (dust, lid edges): offered as "Add as item".
    pub rejected: Vec<[Pt; 4]>,
}

pub trait ItemDetector: Send + Sync {
    /// The items on `raster` under `policy` and `profile`, or `None` when this detector has
    /// nothing to say (the single-item route then runs). Never called for `SplitPolicy::Never`.
    fn detect(
        &self,
        raster: &Raster,
        policy: SplitPolicy,
        profile: SplitProfile,
    ) -> Option<SplitDetection>;

    /// The item at `at` (a tap on a missed item, M10.37), snapped to the edges if there are any.
    fn detect_at(&self, _raster: &Raster, _at: Pt) -> Option<[Pt; 4]> {
        None
    }
}

/// No multi-item detection: every scan takes the single-item route.
pub struct NoSplit;

impl ItemDetector for NoSplit {
    fn detect(&self, _: &Raster, _: SplitPolicy, _: SplitProfile) -> Option<SplitDetection> {
        None
    }
}

/// The scan-level confidence of a split: the worst item (lowest score, a forced band wins, every
/// reason kept once).
pub fn worst_confidence(items: &[DetectedItem]) -> Confidence {
    let mut out = Confidence {
        score: 1.0,
        forced: None,
        reasons: Vec::new(),
    };
    for it in items {
        let c = &it.confidence;
        out.score = out.score.min(c.score);
        out.forced = match (out.forced, c.forced) {
            (Some(auto_crop_core::Forced::Failed), _)
            | (_, Some(auto_crop_core::Forced::Failed)) => Some(auto_crop_core::Forced::Failed),
            (a, b) => a.or(b),
        };
        for r in &c.reasons {
            if !out.reasons.contains(r) {
                out.reasons.push(*r);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use auto_crop_core::{Forced, Reason, ReasonCode};

    fn item(score: f32, forced: Option<Forced>, codes: &[ReasonCode]) -> DetectedItem {
        DetectedItem {
            quad: [Pt::new(0.0, 0.0); 4],
            confidence: Confidence {
                score,
                forced,
                reasons: codes
                    .iter()
                    .map(|&code| Reason { code, side: None })
                    .collect(),
            },
            quarter_turns: 0,
        }
    }

    #[test]
    fn the_scan_confidence_is_the_worst_item() {
        let c = worst_confidence(&[
            item(0.97, None, &[]),
            item(0.7, Some(Forced::Check), &[ReasonCode::WeakEdge]),
            item(0.9, None, &[ReasonCode::WeakEdge, ReasonCode::PartialFrame]),
        ]);
        assert_eq!(c.score, 0.7);
        assert_eq!(c.forced, Some(Forced::Check));
        assert_eq!(c.reasons.len(), 2);
        let failed = worst_confidence(&[
            item(0.4, Some(Forced::Failed), &[]),
            item(0.7, Some(Forced::Check), &[]),
        ]);
        assert_eq!(failed.forced, Some(Forced::Failed));
    }
}
