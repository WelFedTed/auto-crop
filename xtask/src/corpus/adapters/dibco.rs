// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! DIBCO / H-DIBCO adapter (ROADMAP M1.38): document images paired with their binary ground
//! truth, for the threshold tests of the enhancement stage (the metrics come with M7.51).
//!
//! **Fetch-only.** DIBCO has no page quads, so the output is not a quad manifest: it is
//! `binarisation.jsonl`, one pair per line:
//!
//! ```json
//! {"v":1,"id":"dibco-2016-1","image":"2016/img/1.bmp","gt":"2016/gt/1_GT.bmp","year":"2016",
//!  "scene_id":"dibco-2016-1","split":"test","licence":"CC0-1.0","attribution":"...","source":"dibco"}
//! ```
//!
//! **UNVERIFIED against the real dataset.** Nothing was downloaded when this was written; the
//! layout (Doxa BinBench bundling of the contest data, CC0 per ROADMAP M7.51) is not known to
//! this author, so the pairing rules are generic and documented here:
//!
//! * every image file (`png bmp tif tiff jpg jpeg`) under the root is either a ground truth or
//!   an original. It is a ground truth when a directory on its path is named `gt`, `groundtruth`,
//!   `ground_truth` or `ground-truth` (any case), or its file stem ends in `_gt`, `-gt` or ` gt`;
//! * the pairing key is the contest year (the first path component holding a 19xx or 20xx
//!   number, else none) plus the lower-cased stem without the ground-truth suffix;
//! * a key with two originals or two ground truths is ambiguous and left out (counted); an
//!   original without a ground truth, or the reverse, is counted too;
//! * where both headers can be read (the decoders have no BMP support) and the sizes differ,
//!   the pair is left out.
//!
//! One scene per image; splits by scene hash like every other adapter.

use super::common::{
    CorpusInfo, IngestOpts, Report, image_size, is_image_ext, list_files, lower_ext, rel_posix,
    split_for,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const GT_DIRS: [&str; 4] = ["gt", "groundtruth", "ground_truth", "ground-truth"];

fn strip_gt_suffix(stem: &str) -> Option<&str> {
    let lower = stem.to_lowercase();
    ["_gt", "-gt", " gt"]
        .iter()
        .find(|s| lower.ends_with(**s))
        .map(|s| &stem[..stem.len() - s.len()])
}

fn year_of(rel: &[String]) -> String {
    for comp in rel {
        let bytes: Vec<char> = comp.chars().collect();
        for w in bytes.windows(4) {
            let s: String = w.iter().collect();
            if s.chars().all(|c| c.is_ascii_digit()) && (s.starts_with("19") || s.starts_with("20"))
            {
                return s;
            }
        }
    }
    String::new()
}

#[derive(Default)]
struct Pair {
    originals: Vec<PathBuf>,
    truths: Vec<PathBuf>,
}

pub fn ingest(
    src: &Path,
    out: &Path,
    info: &CorpusInfo,
    opts: &IngestOpts,
) -> Result<Report, String> {
    let mut report = Report::new("dibco");
    let mut pairs: BTreeMap<(String, String), Pair> = BTreeMap::new();
    for p in list_files(src)? {
        if !is_image_ext(&lower_ext(&p)) {
            continue;
        }
        let rel: Vec<String> = p
            .strip_prefix(src)
            .map_err(|e| e.to_string())?
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        let Some(stem) = p.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
            continue;
        };
        let dir_gt = rel[..rel.len() - 1]
            .iter()
            .any(|c| GT_DIRS.contains(&c.to_lowercase().as_str()));
        let suffix = strip_gt_suffix(&stem);
        let is_gt = dir_gt || suffix.is_some();
        let base = suffix.unwrap_or(&stem).to_lowercase();
        let entry = pairs.entry((year_of(&rel), base)).or_default();
        if is_gt {
            entry.truths.push(p);
        } else {
            entry.originals.push(p);
        }
    }

    let mut lines: Vec<(String, String)> = Vec::new();
    for ((year, base), pair) in pairs {
        match (pair.originals.as_slice(), pair.truths.as_slice()) {
            ([image], [gt]) => {
                if let (Ok(a), Ok(b)) = (image_size(image), image_size(gt)) {
                    if a != b {
                        report.skip("image-and-ground-truth-sizes-differ");
                        continue;
                    }
                } else {
                    report.skip("size-not-checked");
                }
                let id = if year.is_empty() {
                    format!("dibco-{base}")
                } else {
                    format!("dibco-{year}-{base}")
                };
                let line = serde_json::json!({
                    "v": 1,
                    "id": id,
                    "image": rel_posix(out, image)?,
                    "gt": rel_posix(out, gt)?,
                    "year": year,
                    "scene_id": id,
                    "split": split_for(&id, opts.dev_percent),
                    "licence": info.spdx,
                    "attribution": info.attribution,
                    "source": info.name,
                });
                lines.push((id, line.to_string()));
            }
            ([], [_, ..]) => report.skip("ground-truth-without-original"),
            ([_, ..], []) => report.skip("original-without-ground-truth"),
            _ => report.skip("ambiguous-pair"),
        }
    }
    if lines.is_empty() {
        return Err(format!(
            "dibco: no image/ground-truth pairs were found (skipped: {:?}); the data does not \
             look like the layout this adapter expects (see its module docs)",
            report.skipped
        ));
    }
    lines.sort();
    let text: String = lines.iter().map(|(_, l)| format!("{l}\n")).collect();
    fs::create_dir_all(out).map_err(|e| e.to_string())?;
    fs::write(out.join("binarisation.jsonl"), text).map_err(|e| e.to_string())?;
    super::common::write_info(out, info)?;
    report.items = lines.len();
    report.outputs.push("binarisation.jsonl".to_owned());
    report.outputs.push("corpus-info.json".to_owned());
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ground_truth_suffixes_and_years() {
        assert_eq!(strip_gt_suffix("H01_GT"), Some("H01"));
        assert_eq!(strip_gt_suffix("7-gt"), Some("7"));
        assert_eq!(strip_gt_suffix("HW1"), None);
        assert_eq!(year_of(&["DIBCO2016".into(), "img".into()]), "2016");
        assert_eq!(year_of(&["H-DIBCO 2018".into()]), "2018");
        assert_eq!(year_of(&["12345".into(), "x".into()]), "");
    }
}
