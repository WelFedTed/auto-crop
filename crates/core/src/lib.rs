// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! UI-agnostic core data model. No UI, codec or I/O dependencies.

use serde::{Deserialize, Serialize};

/// Current schema version of [`EditState`].
pub const EDIT_STATE_VERSION: u32 = 1;

/// Placeholder for the versioned, serialisable edit parameters (see PLAN 2.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditState {
    pub version: u32,
}

impl Default for EditState {
    fn default() -> Self {
        Self {
            version: EDIT_STATE_VERSION,
        }
    }
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
        };
        assert_eq!(
            s.validate(),
            Err(CoreError::UnsupportedVersion(EDIT_STATE_VERSION + 1))
        );
    }
}
