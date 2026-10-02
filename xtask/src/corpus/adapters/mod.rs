// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Corpus adapters (ROADMAP M1.37, M1.38): turn an extracted public dataset into the files the
//! accuracy harness reads (`manifest.jsonl` in the format of `docs/testing/eval-harness.md`,
//! plus side files). Each adapter documents the layout it expects, and every one of them is
//! marked **UNVERIFIED against the real dataset**: they were written and tested against small
//! synthetic fixtures that mimic the documented layout, with no download.
//!
//! | Adapter | Dataset | Licence | Output |
//! |---|---|---|---|
//! | `smartdoc2015-ch1` | SmartDoc 2015 Challenge 1 | CC BY 4.0 | `manifest.jsonl`, quads |
//! | `cord` | CORD receipts | CC BY 4.0 | `manifest.jsonl` (ROI quads), `transcripts.jsonl` |
//! | `midv-500` | MIDV-500 | pending audit (refused) | `manifest.jsonl`, quads |
//! | `dibco` | DIBCO / H-DIBCO (Doxa BinBench) | CC0 | `binarisation.jsonl` pairs, fetch-only |
//! | `rawpixls-cc0` | raw.pixls.us | CC0 only | `cc0-samples.jsonl` |
//!
//! Shared rules: the manifest is validated with the harness's own checks before anything is
//! written (an invalid or empty manifest is an error and no file appears); the licence and
//! attribution of the lock entry are carried in every line; splits are by a hash of the
//! `scene_id`, so a clip or document never straddles dev and test.

pub mod common;
pub mod cord;
pub mod dibco;
pub mod midv;
pub mod pixls;
pub mod smartdoc;

use common::{CorpusInfo, IngestOpts, Report};
use std::path::Path;

pub const NAMES: &[&str] = &[
    "smartdoc2015-ch1",
    "cord",
    "midv-500",
    "dibco",
    "rawpixls-cc0",
];

/// Runs the adapter `name` over the extracted tree `src`, writing into `out` (the manifest's
/// directory: image paths in it are relative to `out`, so `out` must contain the images).
pub fn ingest(
    name: &str,
    src: &Path,
    out: &Path,
    info: &CorpusInfo,
    opts: &IngestOpts,
) -> Result<Report, String> {
    if opts.every == 0 {
        return Err("--every must be at least 1".to_owned());
    }
    match name {
        "smartdoc2015-ch1" => smartdoc::ingest(src, out, info, opts),
        "cord" => cord::ingest(src, out, info, opts),
        "midv-500" => midv::ingest(src, out, info, opts),
        "dibco" => dibco::ingest(src, out, info, opts),
        "rawpixls-cc0" => pixls::ingest(src, out, info),
        other => Err(format!(
            "unknown adapter `{other}` (known: {})",
            NAMES.join(", ")
        )),
    }
}
