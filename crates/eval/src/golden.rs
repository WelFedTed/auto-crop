// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The golden-set label format (ROADMAP M1.40) and its bridge to the harness manifest.
//!
//! One JSON file per image, `<image file name>.json`, validated against
//! `docs/testing/golden-label.schema.json`. The schema file is the public contract; the checks
//! here are hand-written (no JSON-schema crate) and a test compares their constants with the
//! schema file so the two cannot drift apart.
//!
//! Coordinates are **normalised** to the EXIF-oriented image (x by `width`, y by `height`), like
//! the harness manifest, so a label can be scored without any conversion. A quad is TL, TR, BR, BL
//! of the *upright* item (the first corner is the top-left of the item as it should be read, so
//! the first edge is the top edge), clockwise on screen, and it may leave the frame.
//!
//! The golden set is private (B21) and lives under the owner's gitignored `_data/`; this module
//! only reads and checks files, it never copies or uploads anything.

use crate::curves::{self, Curves};
use crate::geom::{self, Quad};
use crate::manifest::{ManifestItem, sha256_hex};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};

/// `schema_version` written into every label.
pub const SCHEMA_VERSION: u32 = 1;

/// The ten strata of `docs/testing/golden-set.md`; one image may carry several.
pub const SLICES: [&str; 10] = [
    "receipt-long",
    "thermal-fade",
    "partial-frame",
    "touching-items",
    "phone-document",
    "flatbed-single",
    "flatbed-multi",
    "general-photo",
    "heic-device",
    "negative",
];

/// Per-item boolean flags.
pub const ITEM_FLAGS: [&str; 5] = ["partial_frame", "curved", "touching", "hand_held", "folded"];

/// Quota per slice for golden v0, v1 and v2 (`golden-set.md`, "Staging").
pub const QUOTAS: [(&str, usize); 3] = [("v0", 25), ("v1", 50), ("v2", 80)];

/// How far outside the frame a corner may lie (in image widths and heights) before the label is
/// taken for a typing error.
pub const MAX_OUTSIDE: f64 = 1.0;

/// Names a label directory may hold that are not labels.
pub const NON_LABEL_FILES: [&str; 1] = ["_state.json"];

/// File extensions treated as images when listing a folder (a superset of what one build decodes,
/// so an undecodable file is reported rather than silently ignored).
pub const IMAGE_EXTENSIONS: [&str; 14] = [
    "jpg", "jpeg", "png", "tif", "tiff", "webp", "heic", "heif", "avif", "jxl", "bmp", "gif",
    "jfif", "dng",
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoldenItem {
    /// Four corners, clockwise from the top-left of the upright item, normalised.
    pub quad: Quad,
    /// Optional curved edges (`docs/dev/curved-pages.md`): per edge the points of the boundary
    /// curve, end points equal to the quad corners. An absent edge is straight; an absent
    /// `curves` is a straight page. Having bent curves implies the `curved` flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub curves: Option<Curves>,
    #[serde(default)]
    pub partial_frame: bool,
    #[serde(default)]
    pub curved: bool,
    #[serde(default)]
    pub touching: bool,
    #[serde(default)]
    pub hand_held: bool,
    #[serde(default)]
    pub folded: bool,
}

impl GoldenItem {
    pub fn new(quad: Quad) -> Self {
        Self {
            quad,
            curves: None,
            partial_frame: false,
            curved: false,
            touching: false,
            hand_held: false,
            folded: false,
        }
    }

    /// True when the item is curved: the flag is set, or it carries bent curves (the flag is
    /// implied by the curves).
    pub fn is_curved(&self) -> bool {
        self.curved || self.curves.as_ref().is_some_and(Curves::any_bent)
    }

    /// The names of the flags that are set (`curved` also when bent curves imply it).
    pub fn flags(&self) -> Vec<&'static str> {
        let all = [
            self.partial_frame,
            self.is_curved(),
            self.touching,
            self.hand_held,
            self.folded,
        ];
        ITEM_FLAGS
            .iter()
            .zip(all)
            .filter_map(|(n, on)| on.then_some(*n))
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoldenLabel {
    pub schema_version: u32,
    /// The image's file name inside the image directory; also the label's file stem.
    pub id: String,
    /// The image file name (equal to `id`).
    pub image: String,
    pub image_sha256: String,
    /// Size of the EXIF-oriented image the quads are normalised to.
    pub width: u32,
    pub height: u32,
    /// Shared by images of one base document or background (scene-disjoint splits). Defaults to
    /// the file name, so every image is its own scene unless the owner says otherwise.
    pub scene_id: String,
    /// `golden` or `dev`; optional, because `golden lock` is the authority on the split.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
    pub slices: Vec<String>,
    /// Clockwise quarter turns needed to make the content upright.
    pub orientation_quarter_turns: u8,
    /// One entry per document or photo; empty for a no-document negative.
    pub items: Vec<GoldenItem>,
    /// True when the labeller showed the owner a detector suggestion for this image. Such labels
    /// are not independent of the model and are excluded from the golden evaluation.
    #[serde(default)]
    pub assisted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotator: Option<String>,
    /// Active labelling time in seconds (summed over sessions).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labelling_seconds: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labelled_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub noise_floor_double_labelled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// What is wrong (errors) and what looks odd (warnings) about one label.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Findings {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

/// Why a quad is not a valid outline, if it is not. The order of the checks decides the message
/// for a wrong corner order: a bow-tie is "edges cross", the right shape in the wrong winding is
/// "counter-clockwise".
pub fn quad_problem(q: &Quad) -> Option<String> {
    if !geom::quad_is_finite(q) {
        return Some("non-finite coordinate".to_owned());
    }
    if q.iter()
        .flatten()
        .any(|v| *v < -MAX_OUTSIDE || *v > 1.0 + MAX_OUTSIDE)
    {
        return Some(format!(
            "a corner lies more than {MAX_OUTSIDE} image sizes outside the frame (coordinates are normalised to 0..1)"
        ));
    }
    let unit: Quad = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    if geom::homography(q, &unit).is_none() {
        return Some("degenerate (duplicate or collinear corners)".to_owned());
    }
    if !geom::quad_is_simple(q) {
        return Some("edges cross: the corners are not in TL, TR, BR, BL order".to_owned());
    }
    if geom::signed_area(q) <= 0.0 {
        return Some(
            "counter-clockwise: corners must run clockwise from the top-left of the upright item"
                .to_owned(),
        );
    }
    for i in 0..4 {
        let (a, b, c) = (q[i], q[(i + 1) % 4], q[(i + 2) % 4]);
        let turn = (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]);
        if turn <= 0.0 {
            return Some("not convex (a page outline is a convex quadrilateral)".to_owned());
        }
    }
    None
}

/// Direction of the top edge (TL to TR) in degrees, clockwise on screen, in the image's pixel
/// space (so a non-square image is handled).
pub fn top_edge_degrees(q: &Quad, width: u32, height: u32) -> f64 {
    let (w, h) = (f64::from(width), f64::from(height));
    ((q[1][1] - q[0][1]) * h)
        .atan2((q[1][0] - q[0][0]) * w)
        .to_degrees()
}

/// Quarter turns (clockwise) that make an item with this top edge upright.
pub fn quarter_turns_for(q: &Quad, width: u32, height: u32) -> u8 {
    let deg = top_edge_degrees(q, width, height);
    // The item is rotated by `deg` clockwise; undoing that is -deg.
    let turns = (-deg / 90.0).round() as i64;
    turns.rem_euclid(4) as u8
}

fn is_hex64(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A plain file name: no separators, no `..`, no drive prefix, not empty.
pub fn is_plain_file_name(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && !s.contains(['/', '\\', ':', '\0'])
        && !s.starts_with('.')
}

/// Checks one label against the schema and the labelling rules. `file_stem` is the label file's
/// name without `.json`; it must equal `id`.
pub fn validate_label(l: &GoldenLabel, file_stem: Option<&str>) -> Findings {
    let mut f = Findings::default();
    let err = |f: &mut Findings, m: String| f.errors.push(m);
    if l.schema_version != SCHEMA_VERSION {
        err(
            &mut f,
            format!(
                "schema_version must be {SCHEMA_VERSION}, found {}",
                l.schema_version
            ),
        );
    }
    if !is_plain_file_name(&l.id) {
        err(&mut f, format!("id {:?} must be a plain file name", l.id));
    }
    if l.image != l.id {
        err(
            &mut f,
            format!("image {:?} must equal id {:?}", l.image, l.id),
        );
    }
    if let Some(stem) = file_stem
        && stem != l.id
    {
        err(
            &mut f,
            format!("the file is named {stem}.json but its id is {:?}", l.id),
        );
    }
    if !is_hex64(&l.image_sha256) {
        err(
            &mut f,
            "image_sha256 must be 64 lower-case hex digits".to_owned(),
        );
    }
    if l.scene_id.trim().is_empty() {
        err(&mut f, "scene_id is empty".to_owned());
    }
    if l.width == 0 || l.height == 0 {
        err(&mut f, "width and height must be positive".to_owned());
    }
    if let Some(t) = &l.tier
        && t != "golden"
        && t != "dev"
    {
        err(&mut f, format!("tier must be golden or dev, found {t:?}"));
    }
    if l.slices.is_empty() {
        err(
            &mut f,
            "no slice tag: tick at least one stratum (or `negative`)".to_owned(),
        );
    }
    let mut seen = BTreeSet::new();
    for s in &l.slices {
        if !SLICES.contains(&s.as_str()) {
            err(&mut f, format!("unknown slice tag {s:?}"));
        }
        if !seen.insert(s.as_str()) {
            err(&mut f, format!("duplicate slice tag {s:?}"));
        }
    }
    if l.orientation_quarter_turns > 3 {
        err(
            &mut f,
            format!(
                "orientation_quarter_turns must be 0..3, found {}",
                l.orientation_quarter_turns
            ),
        );
    }
    if let Some(s) = l.labelling_seconds
        && (!s.is_finite() || s < 0.0)
    {
        err(
            &mut f,
            "labelling_seconds must be a finite, non-negative number".to_owned(),
        );
    }
    let negative = l.slices.iter().any(|s| s == "negative");
    if negative && !l.items.is_empty() {
        err(
            &mut f,
            "tagged `negative` but has items: a no-document image has no quad".to_owned(),
        );
    }
    if !negative && l.items.is_empty() {
        err(
            &mut f,
            "no items and not tagged `negative` (label the quad, or tick `negative`)".to_owned(),
        );
    }
    for (k, it) in l.items.iter().enumerate() {
        if let Some(p) = quad_problem(&it.quad) {
            err(&mut f, format!("item {k}: {p}"));
            continue;
        }
        if let Some(c) = &it.curves {
            for p in curves::problems(&it.quad, c) {
                err(&mut f, format!("item {k}: {p}"));
            }
        }
        let outside = it
            .quad
            .iter()
            .any(|p| p[0] < 0.0 || p[0] > 1.0 || p[1] < 0.0 || p[1] > 1.0);
        if outside && !it.partial_frame {
            f.warnings.push(format!(
                "item {k}: a corner is outside the frame but partial_frame is not set"
            ));
        }
        if it.partial_frame && !outside {
            f.warnings.push(format!(
                "item {k}: partial_frame is set but all four corners are inside the frame"
            ));
        }
    }
    let any_partial = l.items.iter().any(|i| i.partial_frame);
    let has_partial_slice = l.slices.iter().any(|s| s == "partial-frame");
    if any_partial != has_partial_slice && !l.items.is_empty() {
        f.warnings
            .push("partial_frame flag and the partial-frame slice tag disagree".to_owned());
    }
    if (l
        .slices
        .iter()
        .any(|s| s == "touching-items" || s == "flatbed-multi"))
        && l.items.len() < 2
    {
        f.warnings.push(
            "tagged touching-items or flatbed-multi but fewer than two items are labelled"
                .to_owned(),
        );
    }
    if l.assisted {
        f.warnings.push(
            "assisted: a detector suggestion was shown, excluded from the golden evaluation"
                .to_owned(),
        );
    }
    f
}

/// A fresh label skeleton for an image (used by the labeller and by tests).
pub fn new_label(image: &str, sha256: &str, width: u32, height: u32) -> GoldenLabel {
    GoldenLabel {
        schema_version: SCHEMA_VERSION,
        id: image.to_owned(),
        image: image.to_owned(),
        image_sha256: sha256.to_owned(),
        width,
        height,
        scene_id: default_scene(image),
        tier: None,
        slices: Vec::new(),
        orientation_quarter_turns: 0,
        items: Vec::new(),
        assisted: false,
        annotator: None,
        labelling_seconds: None,
        labelled_at: None,
        noise_floor_double_labelled: None,
        notes: None,
    }
}

/// The default scene of an image: its own file name. Near-duplicates (two photos of one receipt)
/// must be given one shared `scene_id` by hand, or they can land in dev and locked at once.
pub fn default_scene(image: &str) -> String {
    image.to_owned()
}

pub fn label_file_name(image: &str) -> String {
    format!("{image}.json")
}

/// Streaming SHA-256 of a file.
pub fn sha256_file(path: &Path) -> Result<String, String> {
    let mut f =
        std::fs::File::open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f
            .read(&mut buf)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// True for a file name with an image extension.
pub fn is_image_name(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Image file names directly inside `dir` (not recursive), sorted. Dot files are skipped.
pub fn list_images(dir: &Path) -> Result<Vec<String>, String> {
    let rd = std::fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    let mut out: Vec<String> = rd
        .filter_map(Result::ok)
        .filter(|e| e.path().is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| is_image_name(n) && !n.starts_with('.'))
        .collect();
    out.sort();
    Ok(out)
}

/// One label file as found on disk.
#[derive(Debug, Clone)]
pub struct LabelFile {
    pub path: PathBuf,
    /// File name without `.json`.
    pub stem: String,
    pub label: Result<GoldenLabel, String>,
}

/// Reads every `*.json` label in `dir` (not recursive), sorted by name. A file that does not parse
/// is returned with its error, not skipped.
pub fn read_label_dir(dir: &Path) -> Result<Vec<LabelFile>, String> {
    let rd = std::fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    let mut paths: Vec<PathBuf> = rd
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension().is_some_and(|e| e == "json")
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| !NON_LABEL_FILES.contains(&n))
        })
        .collect();
    paths.sort();
    Ok(paths
        .into_iter()
        .map(|path| {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_owned();
            let label = std::fs::read_to_string(&path)
                .map_err(|e| format!("cannot read: {e}"))
                .and_then(|t| parse_label(&t));
            LabelFile { path, stem, label }
        })
        .collect())
}

pub fn parse_label(text: &str) -> Result<GoldenLabel, String> {
    serde_json::from_str(text).map_err(|e| format!("not a valid label: {e}"))
}

/// Serialises a label as pretty JSON with a trailing newline.
pub fn label_to_json(l: &GoldenLabel) -> String {
    let mut s = serde_json::to_string_pretty(l).expect("label serialises");
    s.push('\n');
    s
}

/// What `check-labels` found over a directory.
#[derive(Debug, Clone, Default)]
pub struct CheckReport {
    pub labels: usize,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    /// Images in the image directory that have no label (reported, an error only with `--strict`).
    pub images_without_label: Vec<String>,
    pub assisted: usize,
    /// Labels per slice tag (an image with several tags counts in each).
    pub slice_counts: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CheckOptions {
    /// Skip hashing the images (fast, but a changed image goes unnoticed).
    pub no_hash: bool,
}

/// Validates every label in `labels_dir` and cross-checks them with the images in `images_dir`:
/// unparseable files, rule violations, duplicate ids, two labels for one image, labels whose image
/// is missing or whose bytes changed, and (reported) images without a label.
pub fn check_dir(
    labels_dir: &Path,
    images_dir: &Path,
    opts: CheckOptions,
) -> Result<CheckReport, String> {
    let files = read_label_dir(labels_dir)?;
    let mut rep = CheckReport::default();
    let mut ids: BTreeMap<String, String> = BTreeMap::new();
    let mut by_image: BTreeMap<String, String> = BTreeMap::new();
    // sha -> (first id, scene)
    let mut by_sha: BTreeMap<String, (String, String)> = BTreeMap::new();
    for lf in &files {
        let shown = format!("{}.json", lf.stem);
        let l = match &lf.label {
            Ok(l) => l,
            Err(e) => {
                rep.errors.push(format!("{shown}: {e}"));
                continue;
            }
        };
        rep.labels += 1;
        let found = validate_label(l, Some(&lf.stem));
        rep.errors
            .extend(found.errors.into_iter().map(|e| format!("{shown}: {e}")));
        rep.warnings
            .extend(found.warnings.into_iter().map(|e| format!("{shown}: {e}")));
        if l.assisted {
            rep.assisted += 1;
        }
        for s in &l.slices {
            *rep.slice_counts.entry(s.clone()).or_default() += 1;
        }
        if let Some(prev) = ids.insert(l.id.clone(), shown.clone()) {
            rep.errors
                .push(format!("{shown}: duplicate id {:?} (also in {prev})", l.id));
        }
        if let Some(prev) = by_image.insert(l.image.clone(), shown.clone())
            && prev != shown
        {
            rep.errors.push(format!(
                "{shown}: image {:?} is already labelled by {prev}",
                l.image
            ));
        }
        match by_sha.get(&l.image_sha256) {
            Some((first, scene)) if *scene != l.scene_id => rep.errors.push(format!(
                "{shown}: same image bytes as {first} but a different scene_id; give duplicates one scene_id (or remove one)"
            )),
            Some(_) => {}
            None => {
                by_sha.insert(l.image_sha256.clone(), (shown.clone(), l.scene_id.clone()));
            }
        }
        if !is_plain_file_name(&l.image) {
            continue;
        }
        let img = images_dir.join(&l.image);
        if !img.is_file() {
            rep.errors.push(format!(
                "{shown}: label without an image ({} not found)",
                l.image
            ));
        } else if !opts.no_hash {
            match sha256_file(&img) {
                Ok(h) if h != l.image_sha256 => rep.errors.push(format!(
                    "{shown}: the image {} changed since it was labelled (sha256 differs)",
                    l.image
                )),
                Ok(_) => {}
                Err(e) => rep.errors.push(format!("{shown}: {e}")),
            }
        }
    }
    let labelled: BTreeSet<&str> = by_image.keys().map(String::as_str).collect();
    for name in list_images(images_dir)? {
        if !labelled.contains(name.as_str()) {
            rep.images_without_label.push(name);
        }
    }
    Ok(rep)
}

/// Builds harness manifest rows from labels. `split_of` gives the split (`dev` or `locked`) of a
/// label, or `None` to leave the label out. Negatives (no item) and assisted labels are never
/// included. With `single_only`, images with several items are left out too (the single-quad
/// predictors cannot be scored on them); otherwise `quad` is the first item and `items` all of
/// them. Returns the rows and the number of labels left out for being negative, assisted or
/// (single-only) multi-item.
pub fn manifest_items(
    labels: &[GoldenLabel],
    split_of: &dyn Fn(&GoldenLabel) -> Option<String>,
    single_only: bool,
) -> (Vec<ManifestItem>, LeftOut) {
    let mut rows = Vec::new();
    let mut left = LeftOut::default();
    for l in labels {
        let Some(split) = split_of(l) else { continue };
        if l.assisted {
            left.assisted += 1;
            continue;
        }
        if l.items.is_empty() {
            left.negatives += 1;
            continue;
        }
        if single_only && l.items.len() > 1 {
            left.multi_item += 1;
            continue;
        }
        let mut tags = BTreeMap::new();
        for s in &l.slices {
            tags.insert(format!("slice-{s}"), "yes".to_owned());
        }
        for flag in ITEM_FLAGS {
            if l.items.iter().any(|i| i.flags().contains(&flag)) {
                tags.insert(format!("flag-{flag}"), "yes".to_owned());
            }
        }
        tags.insert(
            "items".to_owned(),
            if l.items.len() == 1 { "1" } else { "2+" }.to_owned(),
        );
        let quads: Vec<Quad> = l.items.iter().map(|i| i.quad).collect();
        let item_curves: Vec<Option<Curves>> = l.items.iter().map(|i| i.curves.clone()).collect();
        let multi = quads.len() > 1;
        rows.push(ManifestItem {
            v: 1,
            id: l.id.clone(),
            image: l.image.clone(),
            scene_id: l.scene_id.clone(),
            split: Some(split),
            width: l.width,
            height: l.height,
            quad: quads[0],
            curves: item_curves[0].clone(),
            items: if multi { quads } else { Vec::new() },
            items_curves: if multi && item_curves.iter().any(Option::is_some) {
                item_curves
            } else {
                Vec::new()
            },
            tags,
        });
    }
    (rows, left)
}

/// Labels not scored, by reason.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LeftOut {
    pub negatives: usize,
    pub assisted: usize,
    pub multi_item: usize,
}

/// The manifest text of `items` (one JSON object per line), so a run names the exact data it
/// scored via the SHA-256 of this text.
pub fn manifest_text(items: &[ManifestItem]) -> String {
    let mut s = String::new();
    for it in items {
        s.push_str(&serde_json::to_string(it).expect("row serialises"));
        s.push('\n');
    }
    s
}

pub fn manifest_sha256(items: &[ManifestItem]) -> String {
    sha256_hex(manifest_text(items).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: Quad = [[0.1, 0.1], [0.9, 0.1], [0.9, 0.9], [0.1, 0.9]];

    fn sha() -> String {
        "a".repeat(64)
    }

    fn label(name: &str) -> GoldenLabel {
        let mut l = new_label(name, &sha(), 800, 600);
        l.slices = vec!["flatbed-single".to_owned()];
        l.items = vec![GoldenItem::new(GOOD)];
        l
    }

    fn errors(l: &GoldenLabel) -> Vec<String> {
        validate_label(l, None).errors
    }

    #[test]
    fn a_good_label_is_clean_and_round_trips() {
        let l = label("a.jpg");
        let f = validate_label(&l, Some("a.jpg"));
        assert!(f.errors.is_empty(), "{:?}", f.errors);
        assert!(f.warnings.is_empty(), "{:?}", f.warnings);
        let back = parse_label(&label_to_json(&l)).expect("parses");
        assert_eq!(back, l);
    }

    #[test]
    fn quads_with_a_wrong_corner_order_or_shape_are_rejected_with_a_reason() {
        let mut ccw = GOOD;
        ccw.reverse();
        assert!(
            quad_problem(&ccw)
                .expect("bad")
                .contains("counter-clockwise")
        );
        // TL, BL, BR, TR is the same shape walked the other way.
        let ccw2 = [GOOD[0], GOOD[3], GOOD[2], GOOD[1]];
        assert!(
            quad_problem(&ccw2)
                .expect("bad")
                .contains("counter-clockwise")
        );
        // Bow-tie: TL, BR, TR, BL.
        let bow = [GOOD[0], GOOD[2], GOOD[1], GOOD[3]];
        assert!(quad_problem(&bow).expect("bad").contains("cross"));
        let mut nan = GOOD;
        nan[2][1] = f64::NAN;
        assert!(quad_problem(&nan).expect("bad").contains("non-finite"));
        let mut inf = GOOD;
        inf[0][0] = f64::INFINITY;
        assert!(quad_problem(&inf).expect("bad").contains("non-finite"));
        let line = [[0.1, 0.1], [0.3, 0.3], [0.6, 0.6], [0.9, 0.9]];
        assert!(quad_problem(&line).is_some());
        let dup = [GOOD[0], GOOD[0], GOOD[2], GOOD[3]];
        assert!(quad_problem(&dup).is_some());
        // A dart: simple, clockwise, but concave.
        let dart = [[0.1, 0.1], [0.9, 0.1], [0.5, 0.4], [0.1, 0.9]];
        assert!(quad_problem(&dart).expect("bad").contains("convex"));
        let far = [[-3.0, 0.1], [0.9, 0.1], [0.9, 0.9], [0.1, 0.9]];
        assert!(quad_problem(&far).expect("bad").contains("outside"));
        // Leaving the frame a little is fine (partial frames).
        let partial = [[-0.05, -0.02], [1.04, 0.0], [1.02, 1.1], [-0.01, 1.05]];
        assert!(quad_problem(&partial).is_none());
    }

    #[test]
    fn missing_tags_and_inconsistent_negatives_are_errors() {
        let mut l = label("a.jpg");
        l.slices.clear();
        assert!(errors(&l).iter().any(|e| e.contains("no slice tag")));
        let mut l = label("a.jpg");
        l.slices = vec!["bogus".to_owned()];
        assert!(errors(&l).iter().any(|e| e.contains("unknown slice tag")));
        let mut l = label("a.jpg");
        l.slices = vec!["flatbed-single".to_owned(), "flatbed-single".to_owned()];
        assert!(errors(&l).iter().any(|e| e.contains("duplicate slice")));
        let mut l = label("a.jpg");
        l.items.clear();
        assert!(
            errors(&l)
                .iter()
                .any(|e| e.contains("not tagged `negative`"))
        );
        let mut l = label("a.jpg");
        l.slices = vec!["negative".to_owned()];
        assert!(errors(&l).iter().any(|e| e.contains("has items")));
        l.items.clear();
        assert!(errors(&l).is_empty());
        let mut l = label("a.jpg");
        l.items[0].quad[1][0] = f64::NAN;
        assert!(errors(&l).iter().any(|e| e.contains("item 0")));
        let mut l = label("a.jpg");
        l.image_sha256 = "xyz".to_owned();
        assert!(errors(&l).iter().any(|e| e.contains("image_sha256")));
        let mut l = label("a.jpg");
        l.orientation_quarter_turns = 4;
        assert!(errors(&l).iter().any(|e| e.contains("orientation")));
        let mut l = label("../x.jpg");
        l.image = l.id.clone();
        assert!(errors(&l).iter().any(|e| e.contains("plain file name")));
        let mut l = label("a.jpg");
        l.scene_id = " ".to_owned();
        assert!(errors(&l).iter().any(|e| e.contains("scene_id")));
        let l = label("a.jpg");
        assert!(
            validate_label(&l, Some("other.jpg"))
                .errors
                .iter()
                .any(|e| e.contains("named"))
        );
    }

    #[test]
    fn partial_frame_warnings() {
        let mut l = label("a.jpg");
        l.items[0].quad = [[-0.05, 0.1], [0.9, 0.1], [0.9, 0.9], [-0.05, 0.9]];
        assert!(
            validate_label(&l, None)
                .warnings
                .iter()
                .any(|w| w.contains("outside"))
        );
        l.items[0].partial_frame = true;
        l.slices.push("partial-frame".to_owned());
        assert!(validate_label(&l, None).warnings.is_empty());
    }

    #[test]
    fn unknown_fields_and_bad_types_do_not_parse() {
        let good = label_to_json(&label("a.jpg"));
        let mut v: serde_json::Value = serde_json::from_str(&good).expect("json");
        v["surprise"] = serde_json::json!(1);
        assert!(parse_label(&v.to_string()).is_err());
        let mut v: serde_json::Value = serde_json::from_str(&good).expect("json");
        v["items"][0]["quad"] = serde_json::json!([[0.1, 0.1], [0.9, 0.1], [0.9, 0.9]]);
        assert!(parse_label(&v.to_string()).is_err());
        // JSON has no NaN and serde_json refuses out-of-range numbers, so a non-finite quad cannot
        // even be read from a file.
        assert!(parse_label(&good.replace("0.9", "NaN")).is_err());
        assert!(parse_label(&good.replace("0.9", "1e999")).is_err());
    }

    #[test]
    fn top_edge_decides_the_quarter_turns() {
        let upright = GOOD;
        assert_eq!(quarter_turns_for(&upright, 800, 800), 0);
        // The item lies rotated 90 degrees clockwise: its top edge now points down the screen.
        let cw90 = [[0.9, 0.1], [0.9, 0.9], [0.1, 0.9], [0.1, 0.1]];
        assert_eq!(quarter_turns_for(&cw90, 800, 800), 3);
        let upside_down = [[0.9, 0.9], [0.1, 0.9], [0.1, 0.1], [0.9, 0.1]];
        assert_eq!(quarter_turns_for(&upside_down, 800, 800), 2);
        let ccw90 = [[0.1, 0.9], [0.1, 0.1], [0.9, 0.1], [0.9, 0.9]];
        assert_eq!(quarter_turns_for(&ccw90, 800, 800), 1);
    }

    #[test]
    fn the_checks_agree_with_the_schema_file() {
        let schema: serde_json::Value = serde_json::from_str(include_str!(
            "../../../docs/testing/golden-label.schema.json"
        ))
        .expect("schema is JSON");
        let props = &schema["properties"];
        let slices: Vec<&str> = props["slices"]["items"]["enum"]
            .as_array()
            .expect("slice enum")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(slices, SLICES);
        let item_props = &props["items"]["items"]["properties"];
        for flag in ITEM_FLAGS {
            assert!(item_props.get(flag).is_some(), "schema lacks flag {flag}");
        }
        // The flags, `quad` and `curves`.
        assert_eq!(
            item_props.as_object().expect("props").len(),
            ITEM_FLAGS.len() + 2
        );
        assert!(item_props.get("quad").is_some() && item_props.get("curves").is_some());
        let curve_props = &schema["$defs"]["curves"]["properties"];
        for name in curves::EDGE_NAMES {
            assert!(curve_props.get(name).is_some(), "schema lacks curve {name}");
        }
        assert_eq!(curve_props.as_object().expect("curves").len(), 4);
        let curve = &schema["$defs"]["curve"];
        assert_eq!(curve["minItems"], serde_json::json!(curves::MIN_POINTS));
        assert_eq!(curve["maxItems"], serde_json::json!(curves::MAX_POINTS));
        // Every key a label serialises is declared by the schema (additionalProperties is false),
        // and every required key is one the labeller writes.
        let json = serde_json::to_value(label("a.jpg")).expect("json");
        for k in json.as_object().expect("object").keys() {
            assert!(props.get(k).is_some(), "schema does not declare {k}");
        }
        let mut full = label("a.jpg");
        full.tier = Some("dev".to_owned());
        full.annotator = Some("x".to_owned());
        full.labelling_seconds = Some(1.0);
        full.labelled_at = Some("t".to_owned());
        full.noise_floor_double_labelled = Some(true);
        full.notes = Some("n".to_owned());
        for k in serde_json::to_value(&full)
            .expect("json")
            .as_object()
            .expect("object")
            .keys()
        {
            assert!(props.get(k).is_some(), "schema does not declare {k}");
        }
        for k in schema["required"].as_array().expect("required") {
            let k = k.as_str().expect("string");
            assert!(json.get(k).is_some(), "required key {k} is not written");
        }
        assert_eq!(schema["additionalProperties"], serde_json::json!(false));
        assert_eq!(
            props["schema_version"]["const"],
            serde_json::json!(SCHEMA_VERSION)
        );
    }

    fn write(dir: &Path, name: &str, bytes: &[u8]) {
        std::fs::write(dir.join(name), bytes).expect("write");
    }

    fn labelled(dir: &Path, name: &str, bytes: &[u8]) -> GoldenLabel {
        write(dir, name, bytes);
        let mut l = label(name);
        l.image_sha256 = sha256_hex(bytes);
        l
    }

    #[test]
    fn check_dir_reports_every_kind_of_problem() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (img, lab) = (dir.path().join("img"), dir.path().join("lab"));
        std::fs::create_dir_all(&img).expect("mkdir");
        std::fs::create_dir_all(&lab).expect("mkdir");
        // ok.jpg: fine.
        let ok = labelled(&img, "ok.jpg", b"ok-bytes");
        write(&lab, "ok.jpg.json", label_to_json(&ok).as_bytes());
        // changed.jpg: the image was edited after labelling.
        let ch = labelled(&img, "changed.jpg", b"before");
        write(&img, "changed.jpg", b"after");
        write(&lab, "changed.jpg.json", label_to_json(&ch).as_bytes());
        // orphan label: no image.
        let orphan = label("gone.jpg");
        write(&lab, "gone.jpg.json", label_to_json(&orphan).as_bytes());
        // unlabelled image.
        write(&img, "unlabelled.png", b"u");
        write(&img, "notes.txt", b"not an image");
        // wrong corner order.
        let mut bad = labelled(&img, "bad.jpg", b"bad-bytes");
        bad.items[0].quad.reverse();
        write(&lab, "bad.jpg.json", label_to_json(&bad).as_bytes());
        // missing tags.
        let mut notag = labelled(&img, "notag.jpg", b"notag-bytes");
        notag.slices.clear();
        write(&lab, "notag.jpg.json", label_to_json(&notag).as_bytes());
        // file named differently from its id (a duplicate id once copied).
        write(&lab, "copy-of-ok.json", label_to_json(&ok).as_bytes());
        // not JSON at all, and a non-label file that must be ignored.
        write(&lab, "broken.jpg.json", b"{ nope");
        write(&lab, "_state.json", b"{}");
        // same bytes, different scene.
        let dup = labelled(&img, "dup.jpg", b"ok-bytes");
        write(&lab, "dup.jpg.json", label_to_json(&dup).as_bytes());

        let r = check_dir(&lab, &img, CheckOptions::default()).expect("runs");
        let all = r.errors.join("\n");
        for needle in [
            "changed.jpg.json: the image changed.jpg changed since",
            "gone.jpg.json: label without an image",
            "bad.jpg.json: item 0: counter-clockwise",
            "notag.jpg.json: no slice tag",
            "copy-of-ok.json: the file is named copy-of-ok.json but its id is",
            "ok.jpg.json: duplicate id",
            "broken.jpg.json: not a valid label",
            "dup.jpg.json: same image bytes as",
        ] {
            assert!(all.contains(needle), "missing `{needle}` in:\n{all}");
        }
        assert!(!all.contains("_state"), "{all}");
        assert_eq!(r.images_without_label, ["unlabelled.png"]);
        assert_eq!(r.slice_counts.get("flatbed-single"), Some(&6));
    }

    #[test]
    fn a_clean_directory_passes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let l = labelled(dir.path(), "a.png", b"aaa");
        write(dir.path(), "a.png.json", label_to_json(&l).as_bytes());
        let r = check_dir(dir.path(), dir.path(), CheckOptions::default()).expect("runs");
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!((r.labels, r.images_without_label.len()), (1, 0));
    }

    #[test]
    fn manifest_rows_carry_slices_flags_and_skip_what_cannot_be_scored() {
        let mut one = label("one.jpg");
        one.slices.push("receipt-long".to_owned());
        one.items[0].hand_held = true;
        let mut two = label("two.jpg");
        two.items.push(GoldenItem::new([
            [0.2, 0.2],
            [0.4, 0.2],
            [0.4, 0.4],
            [0.2, 0.4],
        ]));
        let mut neg = label("neg.jpg");
        neg.slices = vec!["negative".to_owned()];
        neg.items.clear();
        let mut assisted = label("assisted.jpg");
        assisted.assisted = true;
        let all = [one, two, neg, assisted];
        let to_dev = |_: &GoldenLabel| Some("dev".to_owned());
        let (rows, left) = manifest_items(&all, &to_dev, false);
        assert_eq!(rows.len(), 2);
        assert_eq!(
            left,
            LeftOut {
                negatives: 1,
                assisted: 1,
                multi_item: 0
            }
        );
        assert_eq!(
            rows[0].tags.get("slice-receipt-long").map(String::as_str),
            Some("yes")
        );
        assert_eq!(
            rows[0].tags.get("flag-hand_held").map(String::as_str),
            Some("yes")
        );
        assert_eq!(rows[1].items.len(), 2);
        assert_eq!(rows[1].quad, rows[1].items[0]);
        let (rows, left) = manifest_items(&all, &to_dev, true);
        assert_eq!(rows.len(), 1);
        assert_eq!(left.multi_item, 1);
        // The rows pass the harness's own validation.
        assert!(crate::manifest::validate(&rows).is_empty());
        let none = |_: &GoldenLabel| None;
        assert!(manifest_items(&all, &none, false).0.is_empty());
        assert_eq!(manifest_sha256(&rows).len(), 64);
    }

    fn curved_label(name: &str) -> GoldenLabel {
        let mut l = label(name);
        l.items[0].curves = Some(Curves {
            top: Some(vec![GOOD[0], [0.5, 0.06], GOOD[1]]),
            right: Some(vec![GOOD[1], [0.94, 0.5], GOOD[2]]),
            bottom: None,
            left: Some(vec![GOOD[3], [0.06, 0.5], GOOD[0]]),
        });
        l
    }

    #[test]
    fn curved_labels_validate_round_trip_and_older_labels_still_load() {
        let l = curved_label("c.jpg");
        let f = validate_label(&l, Some("c.jpg"));
        assert!(f.errors.is_empty(), "{:?}", f.errors);
        assert!(f.warnings.is_empty(), "{:?}", f.warnings);
        let text = label_to_json(&l);
        assert!(text.contains("\"curves\"") && !text.contains("\"bottom\""));
        assert_eq!(parse_label(&text).expect("parses"), l);
        // The curved flag is implied by bent curves, and a label with no curves has none.
        assert!(l.items[0].is_curved() && l.items[0].flags().contains(&"curved"));
        assert!(!label("a.jpg").items[0].is_curved());
        assert!(!label_to_json(&label("a.jpg")).contains("curves"));
        // A label written before `curves` existed (no key) loads unchanged.
        let old = r#"{"schema_version":1,"id":"o.jpg","image":"o.jpg","image_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","width":8,"height":6,"scene_id":"o.jpg","slices":["flatbed-single"],"orientation_quarter_turns":0,"items":[{"quad":[[0.1,0.1],[0.9,0.1],[0.9,0.9],[0.1,0.9]],"partial_frame":false,"curved":true,"touching":false,"hand_held":false,"folded":false}]}"#;
        let o = parse_label(old).expect("an old label parses");
        assert!(o.items[0].curves.is_none() && o.items[0].curved);
        assert!(validate_label(&o, Some("o.jpg")).errors.is_empty());
    }

    #[test]
    fn invalid_curves_are_errors_naming_the_item_and_the_curve() {
        let mut l = curved_label("c.jpg");
        l.items[0].curves.as_mut().expect("curves").top =
            Some(vec![[0.2, 0.1], [0.5, 0.06], GOOD[1]]);
        assert!(
            errors(&l).iter().any(
                |e| e.contains("item 0: curves.top: the first point") && e.contains("top-left")
            ),
            "{:?}",
            errors(&l)
        );
        let mut l = curved_label("c.jpg");
        l.items[0].curves.as_mut().expect("curves").left = Some(vec![GOOD[3]]);
        assert!(
            errors(&l)
                .iter()
                .any(|e| e.contains("curves.left: 1 point(s)"))
        );
        let mut l = curved_label("c.jpg");
        l.items[0].curves.as_mut().expect("curves").top = Some(vec![GOOD[0], [0.5, 1.3], GOOD[1]]);
        assert!(
            errors(&l)
                .iter()
                .any(|e| e.contains("item 0: curves.top crosses curves.bottom")),
            "{:?}",
            errors(&l)
        );
        // Two items: the error says which one.
        let mut l = curved_label("c.jpg");
        let mut bad = GoldenItem::new([[0.2, 0.2], [0.4, 0.2], [0.4, 0.4], [0.2, 0.4]]);
        bad.curves = Some(Curves {
            top: Some(vec![[0.2, 0.2], [0.3, 0.1]]),
            ..Curves::default()
        });
        l.items.push(bad);
        assert!(
            errors(&l)
                .iter()
                .any(|e| e.starts_with("item 1: curves.top"))
        );
        // Unknown curve names do not parse.
        let text = label_to_json(&curved_label("c.jpg")).replace("\"top\"", "\"middle\"");
        assert!(parse_label(&text).is_err());
    }

    #[test]
    fn the_manifest_bridge_carries_curves_and_marks_the_flag() {
        let mut multi = label("m.jpg");
        multi.items = vec![
            curved_label("x").items.remove(0),
            GoldenItem::new([[0.2, 0.2], [0.4, 0.2], [0.4, 0.4], [0.2, 0.4]]),
        ];
        let single = curved_label("s.jpg");
        let plain = label("p.jpg");
        let to_dev = |_: &GoldenLabel| Some("dev".to_owned());
        let (rows, _) = manifest_items(&[single.clone(), multi, plain], &to_dev, false);
        assert_eq!(rows[0].curves, single.items[0].curves);
        assert!(rows[0].items_curves.is_empty());
        assert_eq!(
            rows[0].tags.get("flag-curved").map(String::as_str),
            Some("yes")
        );
        assert_eq!(rows[1].items.len(), 2);
        assert_eq!(rows[1].items_curves.len(), 2);
        assert!(rows[1].items_curves[0].is_some() && rows[1].items_curves[1].is_none());
        assert!(rows[1].curves.is_some());
        assert!(rows[2].curves.is_none() && rows[2].items_curves.is_empty());
        assert!(!rows[2].tags.contains_key("flag-curved"));
        // Round trip through the manifest text, and the harness validation still passes (and
        // rejects bad curves).
        let text = manifest_text(&rows);
        let back = crate::manifest::parse(&text, Path::new(".")).expect("valid manifest");
        assert_eq!(back.items[0].curves, rows[0].curves);
        assert_eq!(back.items[1].items_curves, rows[1].items_curves);
        assert!(!text.lines().last().expect("line").contains("curves"));
        let mut broken = rows[0].clone();
        broken.curves.as_mut().expect("curves").top = Some(vec![[0.9, 0.9], [0.5, 0.5]]);
        assert!(
            crate::manifest::validate_item(&broken)
                .iter()
                .any(|e| e.contains("curves.top"))
        );
    }

    #[test]
    fn check_dir_reports_bad_curves_clearly() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut ok = labelled(dir.path(), "ok.png", b"ok");
        ok.items[0].curves = curved_label("x").items[0].curves.clone();
        write(dir.path(), "ok.png.json", label_to_json(&ok).as_bytes());
        let mut bad = labelled(dir.path(), "bad.png", b"bad");
        bad.items[0].curves = Some(Curves {
            right: Some(vec![GOOD[1], [0.5, 0.5], [0.8, 0.8]]),
            ..Curves::default()
        });
        write(dir.path(), "bad.png.json", label_to_json(&bad).as_bytes());
        let r = check_dir(dir.path(), dir.path(), CheckOptions::default()).expect("runs");
        assert_eq!(r.errors.len(), 1, "{:?}", r.errors);
        assert!(
            r.errors[0].starts_with("bad.png.json: item 0: curves.right: the last point"),
            "{}",
            r.errors[0]
        );
    }
}
