// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The run manifest: what one `process` run did to every input, as JSON (schema version 1,
//! documented in `docs/cli.md` and `docs/schema/run-manifest.v1.schema.json`). The same record
//! is one `item` event of `--ndjson`.
//!
//! Stability: within version 1 fields are only ever added. Codes (`code`, `reasons`) are the
//! registry names of the engine (`ErrKind`, hold reasons) plus the CLI's own (`CODES` below);
//! they are never English text. `detail` is free text for people and may change.

use serde::Serialize;

pub const SCHEMA_VERSION: u32 = 1;
pub const SCHEMA_NAME: &str = "auto-crop/run-manifest";

/// Item outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Written (or, in a dry run, would be written).
    Saved,
    /// Not written: it needs a look (low confidence, a failed detection, an unaccepted split).
    Held,
    /// Could not be processed; the original is untouched.
    Failed,
    /// Left alone on purpose (already processed, never replaced, unsupported, cancelled).
    Skipped,
}

impl Status {
    pub fn word(self) -> &'static str {
        match self {
            Status::Saved => "saved",
            Status::Held => "held",
            Status::Failed => "failed",
            Status::Skipped => "skipped",
        }
    }
}

/// The CLI's own codes (the engine's `ErrKind` names are the rest).
pub mod code {
    pub const LOW_CONFIDENCE: &str = "LOW_CONFIDENCE";
    pub const DETECTION_FAILED: &str = "DETECTION_FAILED";
    pub const SPLIT_HELD: &str = "SPLIT_HELD";
    pub const ALREADY_PROCESSED: &str = "ALREADY_PROCESSED";
    pub const NOT_REPLACEABLE: &str = "NOT_REPLACEABLE";
    pub const UNSUPPORTED_FORMAT: &str = "UNSUPPORTED_FORMAT";
    pub const CANCELLED: &str = "CANCELLED";
    pub const EXISTS: &str = "EXISTS";
    pub const NOT_FOUND: &str = "NOT_FOUND";
    pub const NO_MATCH: &str = "NO_MATCH";
    pub const LINK: &str = "LINK";
    /// restore: the file changed since it was saved and `--if-modified fail` stopped.
    pub const MODIFIED_SINCE_SAVE: &str = "MODIFIED_SINCE_SAVE";
    /// restore: no backup is recorded for that file or id.
    pub const NO_BACKUP: &str = "NO_BACKUP";
    /// restore: that backup was already restored.
    pub const ALREADY_RESTORED: &str = "ALREADY_RESTORED";
}

/// Every code this CLI itself emits (the docs test checks `docs/cli.md` against it).
#[cfg(test)]
pub const CODES: [&str; 14] = [
    code::LOW_CONFIDENCE,
    code::DETECTION_FAILED,
    code::SPLIT_HELD,
    code::ALREADY_PROCESSED,
    code::NOT_REPLACEABLE,
    code::UNSUPPORTED_FORMAT,
    code::CANCELLED,
    code::EXISTS,
    code::NOT_FOUND,
    code::NO_MATCH,
    code::LINK,
    code::MODIFIED_SINCE_SAVE,
    code::NO_BACKUP,
    code::ALREADY_RESTORED,
];

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ConfidenceRec {
    /// The uncalibrated v0 score, 0 to 1.
    pub score: f32,
    /// `good`, `check` or `failed` at the run's cut-off.
    pub band: &'static str,
    /// Hold reason codes the detector raised.
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct OutputRec {
    pub path: String,
    pub bytes: Option<u64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub format: Option<&'static str>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Timing {
    pub read: f64,
    pub analyse: f64,
    pub write: f64,
    pub total: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ItemRecord {
    /// Position of the input in the run (0-based), stable across `--jobs`.
    pub index: usize,
    pub input: String,
    pub status: Status,
    /// Why, for held, failed and skipped items: one registry code.
    pub code: Option<String>,
    /// Further codes (the detector's hold reasons, notices such as `tiff.multi_page`).
    pub reasons: Vec<String>,
    /// Free text for people; not stable.
    pub detail: Option<String>,
    pub confidence: Option<ConfidenceRec>,
    /// How many files this image becomes (1, or N for a split scan).
    pub crops: Option<usize>,
    /// The image was found to hold several items.
    pub split: bool,
    /// Files were actually written (false in a dry run, and for held, failed and skipped items).
    pub written: bool,
    pub outputs: Vec<OutputRec>,
    /// The backup that holds the original, for an in-place save.
    pub backup_id: Option<String>,
    pub ms: Option<Timing>,
}

impl ItemRecord {
    pub fn new(index: usize, input: impl Into<String>, status: Status) -> Self {
        Self {
            index,
            input: input.into(),
            status,
            code: None,
            reasons: Vec::new(),
            detail: None,
            confidence: None,
            crops: None,
            split: false,
            written: false,
            outputs: Vec::new(),
            backup_id: None,
            ms: None,
        }
    }

    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Summary {
    /// Items in the manifest.
    pub items: usize,
    pub saved: usize,
    pub held: usize,
    pub failed: usize,
    pub skipped: usize,
    /// Files written (or planned, in a dry run).
    pub files_written: usize,
    /// Non-image files met while walking folders (not items).
    pub ignored_non_image: usize,
    /// Links and junctions not followed.
    pub links_skipped: usize,
    /// Hidden, system, temp, store and `AutoCrop` entries left out of a walk.
    pub hidden_skipped: usize,
    /// Left out by `--include` / `--exclude`.
    pub filtered_out: usize,
    /// The walk stopped at `--max-files`.
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Tool {
    pub name: &'static str,
    pub version: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct Options {
    pub mode: &'static str,
    pub triage: &'static str,
    /// The effective cut-off on the uncalibrated score.
    pub cutoff: f32,
    pub split: &'static str,
    pub profile: &'static str,
    pub margin_percent: f32,
    pub format: &'static str,
    pub quality: Option<u8>,
    pub accept_splits: bool,
    pub reprocess: bool,
    pub recursive: bool,
    pub jobs: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Run {
    /// Also the id of the backup run (`restore --run`).
    pub id: String,
    pub started: String,
    pub finished: String,
    pub dry_run: bool,
    pub cancelled: bool,
    pub exit_code: u8,
    pub exit_name: &'static str,
    pub options: Options,
}

#[derive(Debug, Clone, Serialize)]
pub struct Manifest {
    pub schema: &'static str,
    pub v: u32,
    pub tool: Tool,
    pub run: Run,
    pub summary: Summary,
    pub items: Vec<ItemRecord>,
    /// Notes about the run itself (the walk stopped early, a link was skipped...), codes first.
    pub warnings: Vec<String>,
}

pub fn tool() -> Tool {
    Tool {
        name: "auto-crop",
        version: env!("CARGO_PKG_VERSION"),
    }
}

impl Summary {
    pub fn tally(items: &[ItemRecord]) -> Self {
        let mut s = Summary {
            items: items.len(),
            ..Summary::default()
        };
        for i in items {
            match i.status {
                Status::Saved => s.saved += 1,
                Status::Held => s.held += 1,
                Status::Failed => s.failed += 1,
                Status::Skipped => s.skipped += 1,
            }
            s.files_written += i.outputs.len();
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_serialise_lowercase_and_tally() {
        assert_eq!(serde_json::to_value(Status::Held).unwrap(), "held");
        let mut a = ItemRecord::new(0, "a.jpg", Status::Saved);
        a.outputs.push(OutputRec {
            path: "a.jpg".into(),
            bytes: Some(1),
            width: Some(2),
            height: Some(3),
            format: Some("jpeg"),
        });
        let b = ItemRecord::new(1, "b.jpg", Status::Held).with_code(code::LOW_CONFIDENCE);
        let s = Summary::tally(&[a, b]);
        assert_eq!((s.items, s.saved, s.held, s.files_written), (2, 1, 1, 1));
    }
}
