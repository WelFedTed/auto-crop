// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Detection confidence and hold reasons (PLAN 2.8). Provenance: render ignores all of it.

use crate::geometry::Side;
use serde::{Deserialize, Serialize};

/// Reasons an item is held for review (PLAN 6.7); text lives in the UI, never here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReasonCode {
    NoQuad,
    WeakEdge,
    PartialFrame,
    OddAspect,
    LowContrastEdge,
    ImplausibleQuad,
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
