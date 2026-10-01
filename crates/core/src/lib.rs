// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! UI-agnostic core data model. No UI, codec or I/O dependencies.
//!
//! Edits are parametric (PLAN 2.3): history holds parameters, never pixels, so undo and redo are
//! cheap and rendering stays a pure function of (source, [`EditState`]).

use serde::{Deserialize, Serialize};

/// Current schema version of [`EditState`].
pub const EDIT_STATE_VERSION: u32 = 1;

/// A point in EXIF-oriented source space, normalised: `x` and `y` are 0..1 of width and height.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Pt {
    pub x: f64,
    pub y: f64,
}

impl Pt {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

/// One side of a quadrilateral, in the order top, right, bottom, left (corner `i` to `i + 1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Top,
    Right,
    Bottom,
    Left,
}

impl Side {
    pub const ALL: [Side; 4] = [Side::Top, Side::Right, Side::Bottom, Side::Left];
}

/// Perspective crop of one item (PLAN 2.3 `QuadWarp`, without the fields later milestones add).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuadWarp {
    /// TL, TR, BR, BL.
    pub corners: [Pt; 4],
    /// Clockwise quarter turns applied to the rectified result, 0..=3.
    pub quarter_turns: u8,
    /// Fine rotation in degrees, -45..=45.
    pub fine_deg: f32,
}

impl QuadWarp {
    pub fn new(corners: [Pt; 4]) -> Self {
        Self {
            corners,
            quarter_turns: 0,
            fine_deg: 0.0,
        }
    }

    /// A quad inset by `fraction` of each side from the frame, used by "Draw crop".
    pub fn inset_frame(fraction: f64) -> Self {
        let (a, b) = (fraction, 1.0 - fraction);
        Self::new([Pt::new(a, a), Pt::new(b, a), Pt::new(b, b), Pt::new(a, b)])
    }

    /// Clamps every corner into the frame and the fine angle into range.
    pub fn sanitised(mut self) -> Self {
        for c in &mut self.corners {
            c.x = if c.x.is_finite() {
                c.x.clamp(0.0, 1.0)
            } else {
                0.0
            };
            c.y = if c.y.is_finite() {
                c.y.clamp(0.0, 1.0)
            } else {
                0.0
            };
        }
        self.quarter_turns %= 4;
        self.fine_deg = if self.fine_deg.is_finite() {
            self.fine_deg.clamp(-45.0, 45.0)
        } else {
            0.0
        };
        self
    }
}

/// Versioned, serialisable edit parameters (PLAN 2.3). `geometry` is `None` for an item with no
/// crop (the original is left untouched).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditState {
    pub version: u32,
    pub geometry: Option<QuadWarp>,
}

impl Default for EditState {
    fn default() -> Self {
        Self {
            version: EDIT_STATE_VERSION,
            geometry: None,
        }
    }
}

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

/// Errors produced by the core crate.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CoreError {
    #[error("unsupported edit-state version {0}")]
    UnsupportedVersion(u32),
}

impl EditState {
    /// Rejects states written by a newer schema.
    pub fn validate(&self) -> Result<(), CoreError> {
        if self.version > EDIT_STATE_VERSION {
            Err(CoreError::UnsupportedVersion(self.version))
        } else {
            Ok(())
        }
    }
}

/// Linear undo and redo over complete states with a label per step (PLAN 2.6). One entry per
/// committed gesture; callers coalesce drags and nudge bursts before committing.
#[derive(Debug, Clone)]
pub struct History<T: Clone + PartialEq> {
    /// `entries[0]` is the initial state; each later entry carries the label of the commit that
    /// produced it.
    entries: Vec<(String, T)>,
    cursor: usize,
    cap: usize,
}

impl<T: Clone + PartialEq> History<T> {
    /// Entries kept per item (PLAN 6.6: 200).
    pub const DEFAULT_CAP: usize = 200;

    pub fn new(initial: T) -> Self {
        Self {
            entries: vec![(String::new(), initial)],
            cursor: 0,
            cap: Self::DEFAULT_CAP,
        }
    }

    pub fn current(&self) -> &T {
        &self.entries[self.cursor].1
    }

    /// Commits `state` as a new step. An unchanged state is dropped; the redo tail is cut.
    /// Returns whether a step was recorded.
    pub fn commit(&mut self, label: impl Into<String>, state: T) -> bool {
        if *self.current() == state {
            return false;
        }
        self.entries.truncate(self.cursor + 1);
        self.entries.push((label.into(), state));
        self.cursor += 1;
        if self.entries.len() > self.cap {
            self.entries.remove(0);
            self.cursor -= 1;
        }
        true
    }

    pub fn can_undo(&self) -> bool {
        self.cursor > 0
    }

    pub fn can_redo(&self) -> bool {
        self.cursor + 1 < self.entries.len()
    }

    /// Label of the step `undo` would take back.
    pub fn undo_label(&self) -> Option<&str> {
        self.can_undo()
            .then(|| self.entries[self.cursor].0.as_str())
    }

    /// Label of the step `redo` would reapply.
    pub fn redo_label(&self) -> Option<&str> {
        self.can_redo()
            .then(|| self.entries[self.cursor + 1].0.as_str())
    }

    pub fn undo(&mut self) -> Option<&T> {
        if self.can_undo() {
            self.cursor -= 1;
            Some(self.current())
        } else {
            None
        }
    }

    pub fn redo(&mut self) -> Option<&T> {
        if self.can_redo() {
            self.cursor += 1;
            Some(self.current())
        } else {
            None
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state_is_current_version_and_valid() {
        let s = EditState::default();
        assert_eq!(s.version, EDIT_STATE_VERSION);
        assert_eq!(s.validate(), Ok(()));
    }

    #[test]
    fn newer_version_is_rejected() {
        let s = EditState {
            version: EDIT_STATE_VERSION + 1,
            geometry: None,
        };
        assert_eq!(
            s.validate(),
            Err(CoreError::UnsupportedVersion(EDIT_STATE_VERSION + 1))
        );
    }

    #[test]
    fn sanitising_clamps_corners_and_angle() {
        let q = QuadWarp {
            corners: [
                Pt::new(-1.0, 2.0),
                Pt::new(f64::NAN, 0.5),
                Pt::new(0.5, 0.5),
                Pt::new(1.5, 1.5),
            ],
            quarter_turns: 6,
            fine_deg: 90.0,
        }
        .sanitised();
        assert_eq!(q.corners[0], Pt::new(0.0, 1.0));
        assert_eq!(q.corners[1], Pt::new(0.0, 0.5));
        assert_eq!(q.corners[3], Pt::new(1.0, 1.0));
        assert_eq!(q.quarter_turns, 2);
        assert_eq!(q.fine_deg, 45.0);
    }

    #[test]
    fn edit_state_survives_a_json_round_trip_and_missing_fields() {
        let s = EditState {
            version: 1,
            geometry: Some(QuadWarp::inset_frame(0.05)),
        };
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<EditState>(&json).unwrap(), s);
        // serde(default): an old or partial record still loads.
        assert_eq!(
            serde_json::from_str::<EditState>("{}").unwrap(),
            EditState::default()
        );
    }

    #[test]
    fn history_undo_redo_and_labels() {
        let mut h = History::new(0);
        assert!(!h.can_undo() && !h.can_redo());
        assert!(h.commit("Move corner", 1));
        assert!(h.commit("Rotate", 2));
        assert_eq!(h.undo_label(), Some("Rotate"));
        assert_eq!(h.undo(), Some(&1));
        assert_eq!(h.redo_label(), Some("Rotate"));
        assert_eq!(h.undo(), Some(&0));
        assert_eq!(h.undo(), None);
        assert_eq!(h.redo(), Some(&1));
        // Committing cuts the redo tail.
        assert!(h.commit("Move corner", 9));
        assert!(!h.can_redo());
        assert_eq!(*h.current(), 9);
    }

    #[test]
    fn history_drops_unchanged_states_and_caps() {
        let mut h = History::new(0);
        assert!(!h.commit("noop", 0));
        for i in 1..=(History::<i32>::DEFAULT_CAP as i32 + 50) {
            h.commit("step", i);
        }
        assert!(h.len() <= History::<i32>::DEFAULT_CAP);
        assert_eq!(*h.current(), History::<i32>::DEFAULT_CAP as i32 + 50);
        let mut undone = 0;
        while h.undo().is_some() {
            undone += 1;
        }
        assert_eq!(undone, History::<i32>::DEFAULT_CAP - 1);
    }
}
