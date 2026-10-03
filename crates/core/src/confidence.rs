// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Detection confidence and hold reasons (PLAN 2.8). Provenance: render ignores all of it.

use crate::geometry::Side;
use serde::{Deserialize, Serialize};

/// Reasons an item is held for review (PLAN 6.7); text lives in the UI, never here. The serde
/// names are the registry codes of PLAN 4.9 (`SCREAMING_SNAKE_CASE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReasonCode {
    NoQuad,
    WeakEdge,
    PartialFrame,
    OddAspect,
    LowContrastEdge,
    ImplausibleQuad,
    // Multi-item splitting (PLAN 4.7, ROADMAP M10.13 and M10.15). Additive: older readers see an
    // unknown code and the UI falls back to its generic hold copy.
    /// Items touch (an unsupported cut): kept as one flagged cluster, never written.
    TouchingItems,
    /// Items overlap: kept as one flagged cluster, never written.
    OverlappingItems,
    /// The clear gap between two items is under 1.5% of the shorter side.
    ItemsTooClose,
    /// The item count or an outline changed under a perturbed threshold.
    SplitUnstable,
    /// More than 32 items: the largest are kept.
    TooManyItems,
    /// A forced split on an image that does not look like a scanner bed.
    BedUncertain,
    /// The analysis time or component cap was hit.
    AnalysisLimit,
    /// A bed-like scan with no item at all, or no document on a non-bed image.
    NoDocument,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reason {
    pub code: ReasonCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub side: Option<Side>,
}

/// A band the detector can force regardless of the score.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Forced {
    Failed,
    Check,
}

/// Detection confidence. `score` is an UNCALIBRATED heuristic in 0..1: no calibration exists yet
/// (ROADMAP M4). The UI maps it to Good, Check or Failed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Confidence {
    pub score: f32,
    pub forced: Option<Forced>,
    pub reasons: Vec<Reason>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multi_item_codes_use_the_registry_names() {
        let names = [
            (ReasonCode::TouchingItems, "TOUCHING_ITEMS"),
            (ReasonCode::OverlappingItems, "OVERLAPPING_ITEMS"),
            (ReasonCode::ItemsTooClose, "ITEMS_TOO_CLOSE"),
            (ReasonCode::SplitUnstable, "SPLIT_UNSTABLE"),
            (ReasonCode::TooManyItems, "TOO_MANY_ITEMS"),
            (ReasonCode::BedUncertain, "BED_UNCERTAIN"),
            (ReasonCode::AnalysisLimit, "ANALYSIS_LIMIT"),
            (ReasonCode::NoDocument, "NO_DOCUMENT"),
            // The pre-existing codes keep their names.
            (ReasonCode::LowContrastEdge, "LOW_CONTRAST_EDGE"),
            (ReasonCode::PartialFrame, "PARTIAL_FRAME"),
        ];
        for (code, name) in names {
            let json = serde_json::to_string(&code).expect("serialises");
            assert_eq!(json, format!("\"{name}\""));
            let back: ReasonCode = serde_json::from_str(&json).expect("parses");
            assert_eq!(back, code);
        }
    }
}
