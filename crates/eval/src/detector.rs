// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Adapter that runs the real classical page detector from `auto-crop-imgproc` on the images, so
//! the harness measures the code the app ships. Decoding goes through `auto-crop-codecs`, which
//! applies the EXIF orientation once, matching the "EXIF-oriented" ground truth.

use crate::multi::{MultiPrediction, MultiPredictor};
use crate::predictor::{PredictError, PredictInput, Prediction, Predictor, Verdict};
use auto_crop_core::Forced;
use auto_crop_imgproc::detect::detect;
use auto_crop_imgproc::items::{ItemsOptions, SplitProfile, detect_items};

/// The interim Balanced cutoff (PLAN 7.4: 0.90 until `calibration.json` exists). The detector's
/// score is an uncalibrated heuristic, so this only decides what counts as auto-accepted here.
pub const DEFAULT_GOOD_THRESHOLD: f64 = 0.90;
/// Below this the app shows the item as Failed in every mode (PLAN 6.2.4).
pub const FAILED_BELOW: f64 = 0.60;

pub struct DetectorPredictor {
    pub good_threshold: f64,
}

impl Default for DetectorPredictor {
    fn default() -> Self {
        Self {
            good_threshold: DEFAULT_GOOD_THRESHOLD,
        }
    }
}

impl Predictor for DetectorPredictor {
    fn name(&self) -> String {
        format!("classical-detector(good>={})", self.good_threshold)
    }

    fn predict(&self, input: &PredictInput) -> Result<Prediction, PredictError> {
        let bytes =
            std::fs::read(&input.image).map_err(|e| PredictError::Failed(format!("read: {e}")))?;
        let decoded = auto_crop_codecs::decode(&bytes)
            .map_err(|e| PredictError::Failed(format!("decode: {e}")))?;
        let det = detect(&decoded.raster);
        let score = f64::from(det.confidence.score);
        let quad = det.quad.map(|q| std::array::from_fn(|i| [q[i].x, q[i].y]));
        let verdict = match (quad.is_some(), det.confidence.forced) {
            (false, _) | (_, Some(Forced::Failed)) => Verdict::Failed,
            (_, Some(Forced::Check)) => Verdict::Check,
            _ if score >= self.good_threshold => Verdict::Good,
            _ if score >= FAILED_BELOW => Verdict::Check,
            _ => Verdict::Failed,
        };
        Ok(Prediction {
            quad,
            confidence: Some(score.clamp(0.0, 1.0)),
            verdict: Some(verdict),
        })
    }
}

/// Runs `imgproc::items::detect_items` (the multi-item detector) and reports every item it found.
/// A scan counts as auto-accepted only under the preview rule (ROADMAP M10.29): every item Good
/// at the strict cutoff and no scan-level hold.
pub struct ItemsDetectorPredictor {
    pub opts: ItemsOptions,
}

impl Default for ItemsDetectorPredictor {
    /// The mixed synthetic scenes hold photos, receipts and cards, so the plausible-aspect limit
    /// is the receipts profile's (12:1), as a user who picked "Documents and receipts" would have.
    fn default() -> Self {
        Self {
            opts: ItemsOptions {
                profile: SplitProfile::Receipts,
                ..ItemsOptions::default()
            },
        }
    }
}

impl MultiPredictor for ItemsDetectorPredictor {
    fn name(&self) -> String {
        format!("items-detector(good>={})", self.opts.good_cutoff)
    }

    fn predict(&self, input: &PredictInput) -> Result<MultiPrediction, PredictError> {
        let bytes =
            std::fs::read(&input.image).map_err(|e| PredictError::Failed(format!("read: {e}")))?;
        let decoded = auto_crop_codecs::decode(&bytes)
            .map_err(|e| PredictError::Failed(format!("decode: {e}")))?;
        let det = detect_items(&decoded.raster, &self.opts);
        let conf = det.scan_confidence();
        let mut reasons: Vec<String> = conf
            .reasons
            .iter()
            .map(|r| {
                serde_json::to_string(&r.code).map_or_else(
                    |_| format!("{:?}", r.code),
                    |s| s.trim_matches('"').to_owned(),
                )
            })
            .collect();
        reasons.sort();
        reasons.dedup();
        Ok(MultiPrediction {
            items: det
                .items
                .iter()
                .map(|i| std::array::from_fn(|k| [i.quad[k].x, i.quad[k].y]))
                .collect(),
            accepted: det.auto_accept(self.opts.good_cutoff),
            reasons,
            confidence: Some(f64::from(conf.score).clamp(0.0, 1.0)),
        })
    }
}
