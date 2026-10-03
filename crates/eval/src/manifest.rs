// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The eval manifest: one JSON object per line (`manifest.jsonl`), one image each.
//!
//! ```json
//! {"v":1,"id":"smoke-00007","image":"images/smoke-00007.jpg","scene_id":"smoke-s0003",
//!  "split":"test","width":480,"height":360,
//!  "quad":[[0.31,0.12],[0.66,0.10],[0.69,0.90],[0.28,0.88]],
//!  "tags":{"lighting":"dim","clutter":"light","tilt":"10-30","aspect":"receipt","format":"jpeg"}}
//! ```
//!
//! `quad` is the ground-truth page outline: TL, TR, BR, BL of the upright item, normalised to the
//! EXIF-oriented image (x by width, y by height), clockwise, inside the frame or not. `image` is
//! relative to the manifest directory; absolute paths and `..` are rejected so a hostile manifest
//! cannot point the evaluator at other files. Unknown fields are ignored (forward compatibility).

use crate::geom::{self, Quad};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

pub const MANIFEST_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestItem {
    #[serde(default = "default_version")]
    pub v: u32,
    pub id: String,
    pub image: String,
    #[serde(default)]
    pub scene_id: String,
    /// `dev` or `test` (or absent).
    #[serde(default)]
    pub split: Option<String>,
    pub width: u32,
    pub height: u32,
    pub quad: Quad,
    /// Multi-item scenes (ROADMAP M10.51): every item's ground-truth quad, in the same convention
    /// as `quad`. Absent or empty for single-item manifests, which are unchanged; when present
    /// `quad` is the first of them, so a single-item reader still gets a valid quad.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<Quad>,
    #[serde(default)]
    pub tags: BTreeMap<String, String>,
}

impl ManifestItem {
    /// The item quads of a multi-item scene; a single-item row is one item.
    pub fn item_quads(&self) -> Vec<Quad> {
        if self.items.is_empty() {
            vec![self.quad]
        } else {
            self.items.clone()
        }
    }
}

fn default_version() -> u32 {
    MANIFEST_VERSION
}

#[derive(Debug, Clone)]
pub struct Manifest {
    pub items: Vec<ManifestItem>,
    /// SHA-256 of the manifest bytes, so a result names exactly the data it was scored on.
    pub sha256: String,
    /// The directory `image` paths are relative to.
    pub base: PathBuf,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Everything wrong with one item, as human-readable strings (empty = valid).
pub fn validate_item(it: &ManifestItem) -> Vec<String> {
    let mut e = Vec::new();
    if it.id.is_empty() {
        e.push("empty id".to_owned());
    }
    if it.width == 0 || it.height == 0 {
        e.push("zero image size".to_owned());
    }
    if !geom::quad_is_finite(&it.quad) {
        e.push("non-finite quad".to_owned());
    } else {
        let unit: Quad = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        if geom::homography(&it.quad, &unit).is_none() {
            e.push("degenerate quad (duplicate or collinear corners)".to_owned());
        } else if geom::signed_area(&it.quad) <= 0.0 {
            e.push("quad is not clockwise from the top-left (or has no area)".to_owned());
        } else if !geom::quad_is_simple(&it.quad) {
            e.push("quad edges cross".to_owned());
        }
    }
    for (k, q) in it.items.iter().enumerate() {
        let unit: Quad = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        if !geom::quad_is_finite(q) || geom::homography(q, &unit).is_none() {
            e.push(format!("item {k}: non-finite or degenerate quad"));
        } else if geom::signed_area(q) <= 0.0 {
            e.push(format!("item {k}: quad is not clockwise from the top-left"));
        } else if !geom::quad_is_simple(q) {
            e.push(format!("item {k}: quad edges cross"));
        }
    }
    if let Some(first) = it.items.first()
        && *first != it.quad
    {
        e.push("`quad` must equal the first of `items`".to_owned());
    }
    let p = Path::new(&it.image);
    if it.image.is_empty()
        || it.image.contains(['\\', ':'])
        || p.is_absolute()
        || p.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::Prefix(_) | Component::RootDir
            )
        })
    {
        e.push(format!(
            "image path {:?} must be relative with no `..`",
            it.image
        ));
    }
    e
}

/// Whole-manifest checks: unique ids, valid items, and scene-disjoint splits (a `scene_id` that
/// appears in two splits would leak a scene between dev and test; M1.35's `check-splits` rule).
pub fn validate(items: &[ManifestItem]) -> Vec<String> {
    let mut errors = Vec::new();
    let mut ids = BTreeSet::new();
    let mut scene_split: BTreeMap<&str, &str> = BTreeMap::new();
    for it in items {
        if !ids.insert(it.id.as_str()) {
            errors.push(format!("duplicate id {}", it.id));
        }
        for e in validate_item(it) {
            errors.push(format!("{}: {e}", it.id));
        }
        if let (false, Some(split)) = (it.scene_id.is_empty(), it.split.as_deref()) {
            match scene_split.insert(it.scene_id.as_str(), split) {
                Some(prev) if prev != split => errors.push(format!(
                    "scene {} appears in splits {prev} and {split}",
                    it.scene_id
                )),
                _ => {}
            }
        }
    }
    errors
}

pub fn parse(text: &str, base: &Path) -> Result<Manifest, String> {
    let mut items = Vec::new();
    for (no, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let it: ManifestItem =
            serde_json::from_str(line).map_err(|e| format!("manifest line {}: {e}", no + 1))?;
        items.push(it);
    }
    let errors = validate(&items);
    if !errors.is_empty() {
        let shown: Vec<_> = errors.iter().take(10).cloned().collect();
        return Err(format!(
            "manifest invalid ({} problems): {}",
            errors.len(),
            shown.join("; ")
        ));
    }
    Ok(Manifest {
        items,
        sha256: sha256_hex(text.as_bytes()),
        base: base.to_owned(),
    })
}

pub fn load(path: &Path) -> Result<Manifest, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let text = String::from_utf8(bytes).map_err(|_| "manifest is not UTF-8".to_owned())?;
    let base = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut m = parse(&text, &base)?;
    m.sha256 = sha256_hex(text.as_bytes());
    Ok(m)
}

impl Manifest {
    /// Keeps only items in `split` (`all` keeps everything). The digest still names the full file.
    pub fn filter_split(mut self, split: &str) -> Self {
        if split != "all" {
            self.items.retain(|i| i.split.as_deref() == Some(split));
        }
        self
    }

    pub fn resolve(&self, it: &ManifestItem) -> PathBuf {
        self.base.join(&it.image)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str) -> ManifestItem {
        ManifestItem {
            v: 1,
            id: id.to_owned(),
            image: format!("images/{id}.jpg"),
            scene_id: format!("s-{id}"),
            split: Some("dev".to_owned()),
            width: 100,
            height: 80,
            quad: [[0.1, 0.1], [0.9, 0.1], [0.9, 0.9], [0.1, 0.9]],
            items: Vec::new(),
            tags: BTreeMap::new(),
        }
    }

    fn line(it: &ManifestItem) -> String {
        serde_json::to_string(it).expect("serialises")
    }

    #[test]
    fn a_valid_manifest_parses_and_hashes() {
        let text = format!("{}\n\n{}\n", line(&item("a")), line(&item("b")));
        let m = parse(&text, Path::new("/x")).expect("valid");
        assert_eq!(m.items.len(), 2);
        assert_eq!(m.sha256, sha256_hex(text.as_bytes()));
        assert_eq!(m.sha256.len(), 64);
        // SHA-256("abc") known answer.
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn bad_items_are_rejected_with_a_reason() {
        let mut nonfinite = item("a");
        nonfinite.quad[0][0] = f64::NAN;
        assert!(
            validate_item(&nonfinite)
                .iter()
                .any(|e| e.contains("non-finite"))
        );
        let mut ccw = item("a");
        ccw.quad.reverse();
        assert!(validate_item(&ccw).iter().any(|e| e.contains("clockwise")));
        let mut collinear = item("a");
        collinear.quad = [[0.0, 0.0], [0.3, 0.3], [0.6, 0.6], [0.9, 0.9]];
        assert!(!validate_item(&collinear).is_empty());
        let mut bow = item("a");
        bow.quad = [[0.1, 0.1], [0.9, 0.9], [0.9, 0.1], [0.1, 0.9]];
        assert!(!validate_item(&bow).is_empty());
        for bad in [
            "/etc/passwd",
            "../secret.jpg",
            "images/../../x.jpg",
            "C:/x.jpg",
            "",
        ] {
            let mut it = item("a");
            it.image = bad.to_owned();
            assert!(
                validate_item(&it).iter().any(|e| e.contains("relative")),
                "{bad} should be rejected"
            );
        }
        assert!(validate_item(&item("a")).is_empty());
    }

    #[test]
    fn duplicates_and_leaky_splits_are_rejected() {
        let a = item("a");
        let errs = validate(&[a.clone(), a.clone()]);
        assert!(errs.iter().any(|e| e.contains("duplicate id")));
        let (mut x, mut y) = (item("x"), item("y"));
        x.scene_id = "shared".to_owned();
        y.scene_id = "shared".to_owned();
        y.split = Some("test".to_owned());
        let errs = validate(&[x.clone(), y.clone()]);
        assert!(
            errs.iter()
                .any(|e| e.contains("scene shared appears in splits")),
            "{errs:?}"
        );
        y.split = Some("dev".to_owned());
        assert!(validate(&[x, y]).is_empty());
    }

    #[test]
    fn malformed_json_names_its_line() {
        let text = format!("{}\n{{not json\n", line(&item("a")));
        let e = parse(&text, Path::new(".")).expect_err("must fail");
        assert!(e.contains("line 2"), "{e}");
    }

    #[test]
    fn split_filtering_keeps_the_digest() {
        let mut b = item("b");
        b.split = Some("test".to_owned());
        let text = format!("{}\n{}\n", line(&item("a")), line(&b));
        let m = parse(&text, Path::new(".")).expect("valid");
        let digest = m.sha256.clone();
        let dev = m.clone().filter_split("dev");
        assert_eq!(dev.items.len(), 1);
        assert_eq!(dev.sha256, digest);
        assert_eq!(m.filter_split("all").items.len(), 2);
    }
}
