// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Adapter that runs the real classical page detector from `auto-crop-imgproc` on the images, so
//! the harness measures the code the app ships. Decoding goes through `auto-crop-codecs`, which
//! applies the EXIF orientation once, matching the "EXIF-oriented" ground truth.

use crate::predictor::{PredictError, PredictInput, Prediction, Predictor, Verdict};
use auto_crop_core::Forced;
use auto_crop_imgproc::detect::detect;

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
