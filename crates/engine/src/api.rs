// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The data the engine hands to a front end. Serialised as camelCase JSON, mirroring
//! `ui/src/lib/types.ts` one to one. Items are opaque numeric ids plus a sanitised display name;
//! no field lets the UI act on a file path.

use crate::error::ErrKind;
use crate::store::BackupKind;
use auto_crop_core::{
    Band, Confidence, EditState, OrderMode, Pt, QuadWarp, ScanTriage, SplitPolicy, SplitProfile,
};
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
        EditState::single(
            QuadWarp {
                corners: self.quad,
                quarter_turns: self.quarter_turns,
                mirror: false,
                fine_deg: self.fine_deg,
            }
            .sanitised(),
        )
    }
}

pub fn edit_of(state: &EditState) -> Option<Edit> {
    state.quad().map(Edit::from)
}

impl Edit {
    /// Applies this edit to `quad` (validated, clamped) and keeps what `Edit` does not carry
    /// (the mirror).
    pub fn apply_to(&self, quad: &QuadWarp) -> QuadWarp {
        QuadWarp {
            corners: self.quad,
            quarter_turns: self.quarter_turns,
            mirror: quad.mirror,
            fine_deg: self.fine_deg,
        }
        .sanitised()
    }
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
    /// The output file name; for a split scan, the first of `outputs`.
    pub output: String,
    pub copy: bool,
    /// Every output file name of a split scan, in output order (empty for one-to-one saves).
    #[serde(default)]
    pub outputs: Vec<String>,
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
    /// The crops of this image, in output order (M10.34). One for an ordinary image, none for an
    /// image with no crop. `edit` above is the first included one.
    #[serde(default)]
    pub crops: Vec<CropView>,
    /// Split policy, scan-level triage and approval (M10.29); `None` until analysed.
    #[serde(default)]
    pub split: Option<SplitView>,
    /// Stable position in the undo history, for `revert_crop(Step)`.
    #[serde(default)]
    pub history_position: usize,
    /// The notice code of the reason this source is never replaced in place (`tiff.multi_page`
    /// or `format.write_unavailable`); `None` for a source that can be replaced, and while the
    /// image is still being analysed. Saving such a source writes copies only.
    #[serde(default)]
    pub open_only: Option<String>,
}

/// One step of the session history (a multi-image command): its label and the new view of every
/// image it touched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStep {
    pub label: String,
    pub items: Vec<ItemView>,
}

/// Where a crop came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CropOrigin {
    Auto,
    Manual,
    AutoThenEdited,
}

/// One crop (one output file) of an image (M10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CropView {
    /// Stable across edits and undo; never reused.
    pub id: u32,
    /// The 1-based output rank (the `{n}` of the file name); 0 while excluded.
    pub order: u32,
    pub include: bool,
    pub edit: Option<Edit>,
    /// The detector's proposal for this crop, if it has one (a crop added by hand has none).
    pub auto_edit: Option<Edit>,
    pub mirror: bool,
    pub origin: CropOrigin,
    pub confidence: Option<Confidence>,
    /// Band at the Strict cutoff (icon plus word in the UI); a crop the user placed or edited
    /// counts as reviewed (Good).
    pub band: Option<Band>,
    /// Differs from the detector's proposal (or was added by hand).
    pub edited: bool,
    /// The file name this crop will be saved as, when the image is saved as a split.
    pub output_name: Option<String>,
    /// Cache key for this crop's pixels: part of its image URL (M10.28).
    pub render_key: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitView {
    pub policy: SplitPolicy,
    pub profile: SplitProfile,
    pub order_mode: OrderMode,
    /// What the engine may do with this scan at the Strict cutoff.
    pub triage: ScanTriage,
    /// The user accepted the current state, so a save of a held scan is allowed.
    pub accepted: bool,
    /// The scan would be saved as several files.
    pub is_split: bool,
    /// Number of included crops with a quad.
    pub included: usize,
}

/// Which baseline a crop is reverted to (M10.19).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum RevertTo {
    /// What the detector proposed.
    Auto,
    /// What the crop was at a history position taken from `historyPosition` earlier.
    Step { position: usize },
}

/// A change of the split settings of one image (M10.40): either may be left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SplitPatch {
    pub policy: Option<SplitPolicy>,
    pub profile: Option<SplitProfile>,
}

/// What happens to the derived files when a split scan is restored (M10.26).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DerivedAction {
    /// Keep the derived files where they are (the safe default).
    Keep,
    /// Move the unchanged ones into the backup store (reversible, never a delete); files that
    /// changed since they were saved are kept.
    Remove,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DerivedState {
    /// As it was saved.
    Unchanged,
    /// Edited or replaced since it was saved.
    Changed,
    /// No longer where it was saved.
    Missing,
    /// Moved into the backup store by a restore.
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DerivedFile {
    pub name: String,
    pub bytes: u64,
    pub state: DerivedState,
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
    /// Things that did not stop the save: `SavedSourceInUse` (the set is complete but the scan
    /// could not be removed), `SourceChanged` (the scan was changed meanwhile and left alone).
    #[serde(default)]
    pub notes: Vec<ErrKind>,
    /// A one-line notice code for the UI, e.g. `tiff.multi_page` or `derived.user_edited`.
    #[serde(default)]
    pub notices: Vec<String>,
}

impl SaveOutcome {
    pub fn failed(id: u32, e: ErrKind) -> Self {
        Self {
            id,
            ok: false,
            error: Some(e),
            saved: None,
            notes: Vec::new(),
            notices: Vec::new(),
        }
    }
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
    /// `OneToN` for a split scan (M10.44): then `derived` lists its files.
    #[serde(default)]
    pub kind: BackupKind,
    #[serde(default)]
    pub derived: Vec<DerivedFile>,
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
    /// For a split scan: what became of each derived file.
    #[serde(default)]
    pub derived: Vec<DerivedFile>,
}

impl RestoreOutcome {
    pub fn failed(e: ErrKind) -> Self {
        Self {
            ok: false,
            needs_choice: false,
            error: Some(e),
            restored: None,
            derived: Vec::new(),
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
    fn the_multi_item_json_is_additive_and_camel_case() {
        let v = serde_json::to_value(SavedInfo {
            backup_id: None,
            output: "a_01.jpg".into(),
            copy: false,
            outputs: vec!["a_01.jpg".into(), "a_02.jpg".into()],
        })
        .unwrap();
        assert_eq!(v["outputs"].as_array().unwrap().len(), 2);
        // A record from before M10 (no `outputs`) still parses.
        let old: SavedInfo =
            serde_json::from_str(r#"{"backupId":null,"output":"a.jpg","copy":true}"#).unwrap();
        assert!(old.outputs.is_empty());
        assert_eq!(
            serde_json::to_value(RevertTo::Step { position: 3 }).unwrap(),
            serde_json::json!({"kind": "step", "position": 3})
        );
        assert_eq!(
            serde_json::to_value(DerivedAction::Remove).unwrap(),
            "remove"
        );
        let patch: SplitPatch = serde_json::from_str(r#"{"policy":"never"}"#).unwrap();
        assert_eq!(patch.policy, Some(SplitPolicy::Never));
        assert_eq!(patch.profile, None);

        let step = serde_json::to_value(SessionStep {
            label: "Split into items (2 images)".into(),
            items: Vec::new(),
        })
        .unwrap();
        assert_eq!(step["label"], "Split into items (2 images)");
        assert!(step["items"].as_array().unwrap().is_empty());
    }

    #[test]
    fn open_only_is_a_camel_case_optional_field_that_old_json_lacks() {
        let view = ItemView {
            id: 1,
            name: "a.tif".into(),
            width: 10,
            height: 10,
            status: ItemStatus::Ready,
            error: None,
            edit: None,
            auto_edit: None,
            confidence: None,
            generation: 1,
            edited: false,
            saved: None,
            dirty_since_save: false,
            can_undo: false,
            can_redo: false,
            undo_label: None,
            redo_label: None,
            crops: Vec::new(),
            split: None,
            history_position: 0,
            open_only: Some("tiff.multi_page".into()),
        };
        let mut v = serde_json::to_value(&view).unwrap();
        assert_eq!(v["openOnly"], "tiff.multi_page");
        // A view from before this field existed still parses, as `None`.
        v.as_object_mut().unwrap().remove("openOnly");
        let old: ItemView = serde_json::from_value(v).unwrap();
        assert_eq!(old.open_only, None);
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
        let g = s.quad().unwrap().clone();
        assert_eq!(g.corners[0], Pt::new(0.0, 1.0));
        assert_eq!(g.quarter_turns, 1);
        assert_eq!(g.fine_deg, 45.0);
    }
}
