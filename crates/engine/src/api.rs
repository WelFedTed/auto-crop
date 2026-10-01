// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The data the engine hands to a front end. Serialised as camelCase JSON, mirroring
//! `ui/src/lib/types.ts` one to one. Items are opaque numeric ids plus a sanitised display name;
//! no field lets the UI act on a file path.

use crate::error::ErrKind;
use auto_crop_core::{Confidence, EditState, Pt, QuadWarp};
use serde::{Deserialize, Serialize};

/// What the UI edits: the quad plus rotations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Edit {
    pub quad: [Pt; 4],
    pub quarter_turns: u8,
    pub fine_deg: f32,
}

impl From<&QuadWarp> for Edit {
    fn from(q: &QuadWarp) -> Self {
        Self {
            quad: q.corners,
            quarter_turns: q.quarter_turns,
            fine_deg: q.fine_deg,
        }
    }
}

impl Edit {
    /// Validated, clamped geometry; the engine never trusts the webview's numbers.
    pub fn to_state(&self) -> EditState {
        EditState {
            geometry: Some(
                QuadWarp {
                    corners: self.quad,
                    quarter_turns: self.quarter_turns,
                    fine_deg: self.fine_deg,
                }
                .sanitised(),
            ),
            ..EditState::default()
        }
    }
}

pub fn edit_of(state: &EditState) -> Option<Edit> {
    state.geometry.as_ref().map(Edit::from)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemStatus {
    Analysing,
    Ready,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedInfo {
    pub backup_id: Option<String>,
    pub output: String,
    pub copy: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemView {
    pub id: u32,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub status: ItemStatus,
    pub error: Option<ErrKind>,
    pub edit: Option<Edit>,
    pub auto_edit: Option<Edit>,
    pub confidence: Option<Confidence>,
    #[serde(rename = "gen")]
    pub generation: u64,
    pub edited: bool,
    pub saved: Option<SavedInfo>,
    pub dirty_since_save: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    pub undo_label: Option<String>,
    pub redo_label: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenSummary {
    pub added: usize,
    pub skipped: usize,
    pub ids: Vec<u32>,
    pub skipped_reasons: Vec<ErrKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SaveTarget {
    Replace,
    Copy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveOutcome {
    pub id: u32,
    pub ok: bool,
    pub error: Option<ErrKind>,
    pub saved: Option<SavedInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupFile {
    pub id: String,
    pub name: String,
    pub display_path: String,
    pub original_bytes: u64,
    pub output_bytes: Option<u64>,
    pub changed_since_saved: bool,
    pub restored: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupRun {
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub expires_at: Option<String>,
    pub file_count: usize,
    pub total_bytes: u64,
    pub pinned: bool,
    pub files: Vec<BackupFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupsView {
    pub location: String,
    pub used_bytes: u64,
    pub free_bytes: Option<u64>,
    pub runs: Vec<BackupRun>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestoreMode {
    Auto,
    AsCopy,
    ReplaceAnyway,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreOutcome {
    pub ok: bool,
    pub needs_choice: bool,
    pub error: Option<ErrKind>,
    pub restored: Option<String>,
}

impl RestoreOutcome {
    pub fn failed(e: ErrKind) -> Self {
        Self {
            ok: false,
            needs_choice: false,
            error: Some(e),
            restored: None,
        }
    }
}

/// Which image the UI wants from the `acimg` scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageKind {
    Thumb,
    Src,
    Result,
}

impl ImageKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "thumb" => Some(Self::Thumb),
            "src" => Some(Self::Src),
            "result" => Some(Self::Result),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_shapes_match_the_ui_contract() {
        let edit = Edit::from(&QuadWarp::inset_frame(0.1));
        let v = serde_json::to_value(&edit).unwrap();
        assert!(v.get("quarterTurns").is_some() && v.get("fineDeg").is_some());
        assert_eq!(v["quad"].as_array().unwrap().len(), 4);

        let out = RestoreOutcome::failed(ErrKind::OriginalExpired);
        let v = serde_json::to_value(&out).unwrap();
        assert_eq!(v["needsChoice"], false);
        assert_eq!(v["error"], "ORIGINAL_EXPIRED");

        assert_eq!(
            serde_json::to_value(RestoreMode::AsCopy).unwrap(),
            "as_copy"
        );
        assert_eq!(
            serde_json::to_value(ItemStatus::Analysing).unwrap(),
            "analysing"
        );
        assert_eq!(
            serde_json::to_value(SaveTarget::Replace).unwrap(),
            "replace"
        );
    }

    #[test]
    fn webview_numbers_are_clamped_not_trusted() {
        let edit = Edit {
            quad: [
                Pt::new(-5.0, 9.0),
                Pt::new(0.5, 0.5),
                Pt::new(0.5, 0.5),
                Pt::new(f64::NAN, 0.2),
            ],
            quarter_turns: 9,
            fine_deg: 500.0,
        };
        let s = edit.to_state();
        let g = s.geometry.unwrap();
        assert_eq!(g.corners[0], Pt::new(0.0, 1.0));
        assert_eq!(g.quarter_turns, 1);
        assert_eq!(g.fine_deg, 45.0);
    }
}
