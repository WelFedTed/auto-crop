// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `check-splits` (ROADMAP M1.35, extended by M4.13): a scene, a seed or a document must never
//! reach two splits, in one manifest or across several.
//!
//! The harness's own [`crate::manifest::validate`] already refuses a `scene_id` in two splits
//! inside one manifest. This checker works on the raw JSON lines, so it also sees the fields the
//! generators add beyond manifest v1, and it can compare several manifests at once (for example a
//! training manifest against an evaluation one):
//!
//! * the **group keys** [`GROUP_KEYS`] (`scene_id`, `scene_seed`, `group_id`, `document_id`,
//!   `background_seed`) each name a unit that must live in exactly one split;
//! * an item's split label is its `split` field, or, when it has none, `file:<manifest file name>`,
//!   so two manifests without split fields still count as two different splits;
//! * the same group value in two labels is a violation, whichever manifests the lines came from.
//!
//! A manifest that mixes items with and without `split` is checked on the labels it has.

use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

/// Fields that identify a unit that must not straddle splits.
pub const GROUP_KEYS: &[&str] = &[
    "scene_id",
    "scene_seed",
    "group_id",
    "document_id",
    "background_seed",
];

/// Every violation found over `(manifest name, manifest text)` pairs; empty when the splits are
/// disjoint.
pub fn check_texts(manifests: &[(String, String)]) -> Result<Vec<String>, String> {
    // (key, value) -> label -> first place seen
    let mut seen: BTreeMap<(&'static str, String), BTreeMap<String, String>> = BTreeMap::new();
    for (name, text) in manifests {
        for (no, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let v: Value =
                serde_json::from_str(line).map_err(|e| format!("{name} line {}: {e}", no + 1))?;
            let id = v.get("id").and_then(Value::as_str).unwrap_or("?");
            let label = v
                .get("split")
                .and_then(Value::as_str)
                .map_or_else(|| format!("file:{name}"), str::to_owned);
            for key in GROUP_KEYS {
                let Some(val) = v.get(*key) else { continue };
                let val = match val {
                    Value::String(s) => s.clone(),
                    Value::Number(n) => n.to_string(),
                    _ => continue,
                };
                seen.entry((*key, val))
                    .or_default()
                    .entry(label.clone())
                    .or_insert_with(|| format!("{name}:{id}"));
            }
        }
    }
    let mut out = Vec::new();
    for ((key, val), labels) in &seen {
        if labels.len() > 1 {
            let places: Vec<String> = labels
                .iter()
                .map(|(l, at)| format!("{l} (first {at})"))
                .collect();
            out.push(format!(
                "{key} {val} appears in {} splits: {}",
                labels.len(),
                places.join(", ")
            ));
        }
    }
    Ok(out)
}

/// [`check_texts`] over manifest files.
pub fn check_files(paths: &[&Path]) -> Result<Vec<String>, String> {
    let mut manifests = Vec::new();
    for p in paths {
        let text =
            std::fs::read_to_string(p).map_err(|e| format!("cannot read {}: {e}", p.display()))?;
        // The directory name tells two `manifest.jsonl` files apart.
        let name = p
            .parent()
            .and_then(|d| d.file_name())
            .map_or_else(String::new, |d| format!("{}/", d.to_string_lossy()))
            + &p.file_name()
                .map_or_else(String::new, |f| f.to_string_lossy().into_owned());
        manifests.push((name, text));
    }
    check_texts(&manifests)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(id: &str, scene: &str, split: &str, seed: u64) -> String {
        format!(
            "{{\"v\":1,\"id\":\"{id}\",\"scene_id\":\"{scene}\",\"split\":\"{split}\",\"scene_seed\":{seed}}}\n"
        )
    }

    fn one(text: String) -> Vec<(String, String)> {
        vec![("m.jsonl".to_owned(), text)]
    }

    #[test]
    fn disjoint_scenes_pass() {
        let t =
            line("a", "s1", "dev", 1) + &line("b", "s1", "dev", 1) + &line("c", "s2", "test", 2);
        assert!(check_texts(&one(t)).expect("parses").is_empty());
    }

    #[test]
    fn a_scene_in_two_splits_fails_and_names_both() {
        let t = line("a", "s1", "dev", 1) + &line("b", "s1", "test", 2);
        let v = check_texts(&one(t)).expect("parses");
        assert!(
            v.iter()
                .any(|m| m.starts_with("scene_id s1 appears in 2 splits")
                    && m.contains("dev")
                    && m.contains("test")),
            "{v:?}"
        );
    }

    #[test]
    fn a_seed_in_two_splits_fails_even_when_the_scene_ids_differ() {
        let t = line("a", "s1", "dev", 7) + &line("b", "s2", "test", 7);
        let v = check_texts(&one(t)).expect("parses");
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(
            v[0].starts_with("scene_seed 7 appears in 2 splits"),
            "{v:?}"
        );
    }

    #[test]
    fn two_manifests_are_compared_and_unlabelled_ones_count_by_file() {
        let a = (
            "train/manifest.jsonl".to_owned(),
            "{\"id\":\"a\",\"scene_id\":\"s1\"}\n".to_owned(),
        );
        let b = (
            "eval/manifest.jsonl".to_owned(),
            "{\"id\":\"b\",\"scene_id\":\"s1\"}\n".to_owned(),
        );
        let v = check_texts(&[a.clone(), b]).expect("parses");
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(
            v[0].contains("file:train/manifest.jsonl") && v[0].contains("file:eval/manifest.jsonl")
        );
        let c = (
            "eval/manifest.jsonl".to_owned(),
            "{\"id\":\"b\",\"scene_id\":\"s9\"}\n".to_owned(),
        );
        assert!(check_texts(&[a, c]).expect("parses").is_empty());
    }

    #[test]
    fn items_without_group_keys_and_blank_lines_are_ignored_and_bad_json_is_an_error() {
        let ok = one(
            "{\"id\":\"a\",\"split\":\"dev\"}\n\n{\"id\":\"b\",\"split\":\"test\"}\n".to_owned(),
        );
        assert!(check_texts(&ok).expect("parses").is_empty());
        let bad = one("{\"id\":\"a\"}\n{nope\n".to_owned());
        assert!(check_texts(&bad).expect_err("bad json").contains("line 2"));
    }

    #[test]
    fn the_python_generators_planned_splits_pass_on_a_real_looking_manifest() {
        // two images of one scene share every group key and the split
        let t = line("a", "smoke-s00001", "dev", 99)
            + &line("b", "smoke-s00001", "dev", 99)
            + &line("c", "smoke-s00002", "test", 100);
        assert!(check_texts(&one(t)).expect("parses").is_empty());
    }
}
