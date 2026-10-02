// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! UI-agnostic core data model and ports (ROADMAP M1.01). Model and traits only: the only
//! dependencies are `serde` and `thiserror`, there is no file or network I/O, no thread
//! spawning, no `rayon` and no OS API, and `unsafe` is forbidden. `cargo xtask check-deps`
//! enforces the dependency half of that.
//!
//! Edits are parametric (PLAN 2.3): history holds parameters, never pixels, so undo and redo are
//! cheap and rendering stays a pure function of (source, [`EditState`]).

#![forbid(unsafe_code)]

pub mod cancel;
pub mod confidence;
pub mod edit;
pub mod error;
pub mod geometry;
pub mod history;
pub mod output;
pub mod ports;
pub mod source;

pub use cancel::{BAND_ROWS, CancelToken, GenerationCounter, Interrupt, Level};
pub use confidence::{Confidence, Forced, Reason, ReasonCode};
pub use edit::{
    EDIT_STATE_VERSION, EditState, Enhance, Item, ItemId, MarginPolicy, Orient, Origin,
};
pub use error::{CoreError, ErrKind};
pub use geometry::{ExifOrientation, Geometry, GridWarp, Pt, QuadWarp, Side};
pub use history::{GestureId, History, SessionCmd, SessionHistory};
pub use output::{OutputSpec, WriteRequirements};
pub use source::{SourceId, SourceRef};
