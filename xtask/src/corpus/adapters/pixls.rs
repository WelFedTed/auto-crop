// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! raw.pixls.us adapter (ROADMAP M1.38): a list of camera RAW samples **filtered to CC0 only**.
//!
//! The site is not uniformly CC0: some older samples (from rawsamples.ch) are not. The filter is
//! therefore strict and fails closed: an index entry survives only when its licence field is
//! exactly one of the CC0 spellings ([`is_cc0`]) **and** it has a path that stays inside the
//! sample directory **and** a pinned SHA-256 (so a later fetch can verify it). A missing,
//! empty, differently worded or combined licence (`CC-BY-SA`, `Public Domain`,
//! `CC0-1.0 OR CC-BY-SA-4.0`, ...) is excluded and counted by reason.
//!
//! **UNVERIFIED against the real site.** Nothing was contacted when this was written. The input
//! is an index file named `index.jsonl` in the extracted root, either JSON lines or one JSON
//! array, one object per sample with: `path` (or `name`), `licence` (or `license`), `sha256` and
//! optionally `size`, `url`, `make`, `model`. The real index format must be confirmed at first
//! fetch; the filter itself does not depend on it.
//!
//! Output: `cc0-samples.jsonl` with the surviving entries (sorted by path), `corpus-info.json`
//! and the exclusion counts in the report. Fetching the samples themselves (each with its own
//! verified pin) is not built yet.

use super::common::{CorpusInfo, Report, write_info};
use crate::corpus::fetch::is_plain_relative;
use serde_json::Value;
use std::fs;
use std::path::Path;

/// Exactly CC0, in the spellings the site and SPDX use; everything else is not CC0.
pub fn is_cc0(licence: &str) -> bool {
    matches!(
        licence.trim().to_lowercase().as_str(),
        "cc0" | "cc0-1.0" | "cc0 1.0"
    )
}

fn text<'a>(o: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| o.get(*k)?.as_str())
}

fn read_index(path: &Path) -> Result<Vec<Value>, String> {
    let t = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if t.trim_start().starts_with('[') {
        return serde_json::from_str(&t).map_err(|e| format!("{}: {e}", path.display()));
    }
    t.lines()
        .filter(|l| !l.trim().is_empty())
        .enumerate()
        .map(|(i, l)| {
            serde_json::from_str(l).map_err(|e| format!("{} line {}: {e}", path.display(), i + 1))
        })
        .collect()
}

pub fn ingest(src: &Path, out: &Path, info: &CorpusInfo) -> Result<Report, String> {
    let mut report = Report::new("rawpixls-cc0");
    let index = src.join("index.jsonl");
    if !index.is_file() {
        return Err(format!(
            "rawpixls-cc0: no index.jsonl under {} (see the adapter docs)",
            src.display()
        ));
    }
    let mut kept: Vec<(String, String)> = Vec::new();
    for entry in read_index(&index)? {
        let Some(path) = text(&entry, &["path", "name"]) else {
            report.skip("no-path");
            continue;
        };
        if !is_plain_relative(path) {
            report.skip("unsafe-path");
            continue;
        }
        let licence = text(&entry, &["licence", "license"]).unwrap_or("");
        if !is_cc0(licence) {
            let shown = if licence.trim().is_empty() {
                "missing".to_owned()
            } else {
                licence.trim().to_owned()
            };
            report.skip(&format!("not-cc0: {shown}"));
            continue;
        }
        let sha = text(&entry, &["sha256"]).unwrap_or("");
        if sha.len() != 64 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
            report.skip("cc0-but-no-pinned-sha256");
            continue;
        }
        let mut line = serde_json::json!({
            "v": 1,
            "id": format!("rawpixls-{path}"),
            "path": path,
            "sha256": sha.to_lowercase(),
            "licence": "CC0-1.0",
            "attribution": info.attribution,
            "source": info.name,
        });
        for key in ["size", "url", "make", "model"] {
            if let Some(v) = entry.get(key) {
                line[key] = v.clone();
            }
        }
        kept.push((path.to_owned(), line.to_string()));
    }
    if kept.is_empty() {
        return Err(format!(
            "rawpixls-cc0: no CC0 sample with a pinned SHA-256 in the index (excluded: {:?})",
            report.skipped
        ));
    }
    kept.sort();
    kept.dedup_by(|a, b| a.0 == b.0);
    let body: String = kept.iter().map(|(_, l)| format!("{l}\n")).collect();
    fs::create_dir_all(out).map_err(|e| e.to_string())?;
    fs::write(out.join("cc0-samples.jsonl"), body).map_err(|e| e.to_string())?;
    write_info(out, info)?;
    report.items = kept.len();
    report.outputs.push("cc0-samples.jsonl".to_owned());
    report.outputs.push("corpus-info.json".to_owned());
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_exact_cc0_spellings_pass() {
        for ok in ["CC0", "cc0-1.0", " CC0-1.0 ", "CC0 1.0"] {
            assert!(is_cc0(ok), "{ok}");
        }
        for bad in [
            "",
            "CC-BY-SA-4.0",
            "Public Domain",
            "CC0-1.0 OR CC-BY-SA-4.0",
            "CC0-ish",
            "not CC0",
            "CC-BY-4.0",
            "unknown",
        ] {
            assert!(!is_cc0(bad), "{bad}");
        }
    }
}
