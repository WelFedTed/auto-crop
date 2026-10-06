// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Shared plumbing of the corpus adapters: CSV, directory walks, quad handling, deterministic
//! scene-level splits, the manifest writer (which refuses to write an invalid manifest), the
//! `corpus-info.json` licence sidecar and the review contact sheet.

use auto_crop_eval::manifest::{self, ManifestItem, sha256_hex};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Quad corners, normalised: TL, TR, BR, BL (the harness manifest convention).
pub type Quad = [[f64; 2]; 4];

/// What the lock file says about the corpus being ingested; carried into every manifest line.
#[derive(Debug, Clone)]
pub struct CorpusInfo {
    pub name: String,
    pub adapter: String,
    pub spdx: String,
    pub attribution: String,
    pub licence_url: String,
}

/// How frames and documents are grouped into `scene_id`s (and therefore into splits).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneBy {
    /// One scene per video clip (the M1.37 rule): all frames of a clip share a split.
    Clip,
    /// One scene per underlying document or model, across every clip and background: stricter,
    /// because the same page seen on five backgrounds is not an independent test of anything.
    Document,
}

impl SceneBy {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "clip" => Ok(Self::Clip),
            "document" | "model" => Ok(Self::Document),
            other => Err(format!(
                "--scene-by must be clip or document, found `{other}`"
            )),
        }
    }
}

#[derive(Debug, Clone)]
pub struct IngestOpts {
    /// Keep every Nth frame of a clip (1 keeps all).
    pub every: usize,
    /// `None` = the adapter's own default (SmartDoc: clip, MIDV: document).
    pub scene_by: Option<SceneBy>,
    /// Share of scenes that go to `dev` (the rest is `test`).
    pub dev_percent: u32,
    /// Write a review contact sheet with this many quads (0 = none).
    pub contact_sheet: usize,
}

impl Default for IngestOpts {
    fn default() -> Self {
        Self {
            every: 10,
            scene_by: None,
            dev_percent: 30,
            contact_sheet: 20,
        }
    }
}

/// What an adapter produced, for the console and `ingest-report.json`.
#[derive(Debug, Default, Serialize)]
pub struct Report {
    pub adapter: String,
    pub items: usize,
    /// Why records were left out, by reason.
    pub skipped: BTreeMap<String, u64>,
    pub outputs: Vec<String>,
}

impl Report {
    pub fn new(adapter: &str) -> Self {
        Self {
            adapter: adapter.to_owned(),
            ..Self::default()
        }
    }

    pub fn skip(&mut self, reason: &str) {
        *self.skipped.entry(reason.to_owned()).or_insert(0) += 1;
    }

    pub fn summary(&self) -> String {
        let skipped: Vec<String> = self
            .skipped
            .iter()
            .map(|(k, v)| format!("{k}: {v}"))
            .collect();
        format!(
            "{}: {} item(s); skipped [{}]; wrote {}",
            self.adapter,
            self.items,
            skipped.join(", "),
            self.outputs.join(", ")
        )
    }
}

/// A manifest line: the harness item plus the licence it was published under.
#[derive(Serialize)]
struct Line<'a> {
    #[serde(flatten)]
    item: &'a ManifestItem,
    licence: &'a str,
    attribution: &'a str,
    source: &'a str,
}

/// Validates the items with the harness's own checks, then writes `manifest.jsonl` (sorted by
/// id, so the same input always gives the same bytes) and `corpus-info.json` into `out`.
/// An invalid manifest is never written.
pub fn write_manifest(
    out: &Path,
    info: &CorpusInfo,
    mut items: Vec<ManifestItem>,
    report: &mut Report,
) -> Result<Vec<ManifestItem>, String> {
    if items.is_empty() {
        return Err(format!(
            "{}: no usable records were found ({}); the data does not look like the layout this \
             adapter expects (see its module docs), so nothing was written",
            info.adapter,
            skipped_text(report)
        ));
    }
    items.sort_by(|a, b| a.id.cmp(&b.id));
    let errors = manifest::validate(&items);
    if !errors.is_empty() {
        let shown: Vec<_> = errors.iter().take(10).cloned().collect();
        return Err(format!(
            "{}: the manifest would be invalid ({} problems), nothing written: {}",
            info.adapter,
            errors.len(),
            shown.join("; ")
        ));
    }
    let mut text = String::new();
    for it in &items {
        let line = Line {
            item: it,
            licence: &info.spdx,
            attribution: &info.attribution,
            source: &info.name,
        };
        text.push_str(&serde_json::to_string(&line).map_err(|e| e.to_string())?);
        text.push('\n');
    }
    fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    fs::write(out.join("manifest.jsonl"), &text).map_err(|e| e.to_string())?;
    report.items = items.len();
    report.outputs.push("manifest.jsonl".to_owned());
    write_info(out, info)?;
    report.outputs.push("corpus-info.json".to_owned());
    Ok(items)
}

/// Writes the manifest and, when asked for, the review contact sheet.
pub fn finish(
    out: &Path,
    info: &CorpusInfo,
    opts: &IngestOpts,
    items: Vec<ManifestItem>,
    report: &mut Report,
) -> Result<(), String> {
    let items = write_manifest(out, info, items, report)?;
    if let Some(sheet) = contact_sheet(out, &items, opts.contact_sheet)? {
        report.outputs.push(sheet);
    }
    Ok(())
}

fn skipped_text(report: &Report) -> String {
    if report.skipped.is_empty() {
        "nothing matched".to_owned()
    } else {
        report
            .skipped
            .iter()
            .map(|(k, v)| format!("{k}: {v}"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// The licence and attribution of the data next to the files derived from it.
pub fn write_info(out: &Path, info: &CorpusInfo) -> Result<(), String> {
    #[derive(Serialize)]
    struct Info<'a> {
        v: u32,
        corpus: &'a str,
        adapter: &'a str,
        licence: &'a str,
        licence_url: &'a str,
        attribution: &'a str,
    }
    let text = serde_json::to_string_pretty(&Info {
        v: 1,
        corpus: &info.name,
        adapter: &info.adapter,
        licence: &info.spdx,
        licence_url: &info.licence_url,
        attribution: &info.attribution,
    })
    .map_err(|e| e.to_string())?;
    fs::create_dir_all(out).map_err(|e| e.to_string())?;
    fs::write(out.join("corpus-info.json"), text + "\n").map_err(|e| e.to_string())
}

/// All files below `dir`, sorted, without following symbolic links.
pub fn list_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let rd = fs::read_dir(&d).map_err(|e| format!("{}: {e}", d.display()))?;
        for entry in rd {
            let entry = entry.map_err(|e| e.to_string())?;
            let ty = entry.file_type().map_err(|e| e.to_string())?;
            if ty.is_dir() {
                stack.push(entry.path());
            } else if ty.is_file() {
                out.push(entry.path());
            }
        }
    }
    out.sort();
    Ok(out)
}

/// `path` relative to `base`, with forward slashes, or an error when it is not below it.
pub fn rel_posix(base: &Path, path: &Path) -> Result<String, String> {
    let rel = path
        .strip_prefix(base)
        .map_err(|_| format!("{} is not below {}", path.display(), base.display()))?;
    let parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    Ok(parts.join("/"))
}

pub fn lower_ext(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

pub fn is_image_ext(ext: &str) -> bool {
    matches!(
        ext,
        "jpg" | "jpeg" | "png" | "tif" | "tiff" | "bmp" | "webp"
    )
}

/// Tag value for the file format (`jpg` and `jpeg` are one format).
pub fn format_tag(path: &Path) -> String {
    match lower_ext(path).as_str() {
        "jpg" | "jpeg" => "jpeg".to_owned(),
        "tif" | "tiff" => "tiff".to_owned(),
        other => other.to_owned(),
    }
}

/// Stored pixel size of an image from its header. EXIF-rotated files are refused (reason
/// `exif-orientation-not-1`): ground truth in stored pixels would not match the oriented frame
/// the manifest promises.
pub fn image_size(path: &Path) -> Result<(u32, u32), &'static str> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|mut f| f.read_to_end(&mut bytes))
        .map_err(|_| "unreadable-image")?;
    let p = auto_crop_codecs::probe(&bytes).map_err(|_| "unreadable-image")?;
    if p.orientation != 1 {
        return Err("exif-orientation-not-1");
    }
    Ok((p.width, p.height))
}

/// Pixel quad to the manifest's normalised quad; `None` when a corner is not finite.
pub fn normalise(px: &[[f64; 2]; 4], w: u32, h: u32) -> Option<Quad> {
    let mut q = [[0.0; 2]; 4];
    for (o, p) in q.iter_mut().zip(px) {
        if !p[0].is_finite() || !p[1].is_finite() {
            return None;
        }
        *o = [p[0] / f64::from(w), p[1] / f64::from(h)];
    }
    Some(q)
}

/// Orders four unordered corners clockwise (y down) starting at the top-left-most one (smallest
/// x + y). Only for sources that do not promise an order; a dataset that does is taken as given.
pub fn order_clockwise(p: &[[f64; 2]; 4]) -> [[f64; 2]; 4] {
    let cx = p.iter().map(|c| c[0]).sum::<f64>() / 4.0;
    let cy = p.iter().map(|c| c[1]).sum::<f64>() / 4.0;
    let mut v = *p;
    v.sort_by(|a, b| {
        let aa = (a[1] - cy).atan2(a[0] - cx);
        let bb = (b[1] - cy).atan2(b[0] - cx);
        aa.total_cmp(&bb)
    });
    let start = (0..4)
        .min_by(|&i, &j| (v[i][0] + v[i][1]).total_cmp(&(v[j][0] + v[j][1])))
        .unwrap_or(0);
    [
        v[start],
        v[(start + 1) % 4],
        v[(start + 2) % 4],
        v[(start + 3) % 4],
    ]
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// `receipt` when the long side is at least 2.5 times the short one, else `document`.
pub fn aspect_tag(px: &[[f64; 2]; 4]) -> &'static str {
    let a = (dist(px[0], px[1]) + dist(px[3], px[2])) / 2.0;
    let b = (dist(px[1], px[2]) + dist(px[0], px[3])) / 2.0;
    let (long, short) = if a >= b { (a, b) } else { (b, a) };
    if short > 0.0 && long / short >= 2.5 {
        "receipt"
    } else {
        "document"
    }
}

/// `dev` for `dev_percent` percent of scenes and `test` for the rest, by a hash of the scene id
/// (stable across runs, machines and the order of the data).
pub fn split_for(scene_id: &str, dev_percent: u32) -> &'static str {
    let hex = sha256_hex(scene_id.as_bytes());
    let n = u32::from_str_radix(&hex[..8], 16).unwrap_or(0) % 100;
    if n < dev_percent { "dev" } else { "test" }
}

/// Parses CSV (RFC 4180 quoting, CRLF or LF) into rows of fields.
pub fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    let mut any = false;
    while let Some(c) = chars.next() {
        any = true;
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' => quoted = true,
            ',' => row.push(std::mem::take(&mut field)),
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
                any = false;
            }
            _ => field.push(c),
        }
    }
    if any {
        row.push(field);
        rows.push(row);
    }
    rows.retain(|r| !(r.len() == 1 && r[0].trim().is_empty()));
    rows
}

/// Column positions by header name (trimmed, case-insensitive).
pub struct Columns(Vec<String>);

impl Columns {
    pub fn new(header: &[String]) -> Self {
        Self(header.iter().map(|h| h.trim().to_lowercase()).collect())
    }

    pub fn find(&self, name: &str) -> Option<usize> {
        self.0.iter().position(|h| h == name)
    }

    /// Positions of all `names`, or an error naming what is missing and what the header has.
    pub fn require(&self, names: &[&str]) -> Result<Vec<usize>, String> {
        let missing: Vec<&str> = names
            .iter()
            .copied()
            .filter(|n| self.find(n).is_none())
            .collect();
        if missing.is_empty() {
            Ok(names.iter().filter_map(|n| self.find(n)).collect())
        } else {
            Err(format!(
                "missing column(s) {}; the header has: {}",
                missing.join(", "),
                self.0.join(", ")
            ))
        }
    }
}

/// Reads a text file, gunzipping it when it ends in `.gz`.
pub fn read_text_maybe_gz(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let bytes = if lower_ext(path) == "gz" {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(&bytes[..])
            .read_to_end(&mut out)
            .map_err(|e| format!("{}: not valid gzip: {e}", path.display()))?;
        out
    } else {
        bytes
    };
    String::from_utf8(bytes).map_err(|_| format!("{}: not UTF-8", path.display()))
}

/// Picks every `every`th element (the first, then the `every`th after it, ...).
pub fn every_nth<T>(items: Vec<T>, every: usize) -> Vec<T> {
    let every = every.max(1);
    items
        .into_iter()
        .enumerate()
        .filter(|(i, _)| i % every == 0)
        .map(|(_, t)| t)
        .collect()
}

/// An HTML page with `n` evenly spaced items drawn with their quads, to be eyeballed once per
/// adapter ("20 quads on a contact sheet", M1.37). Local review aid, never committed; browsers
/// do not show TIFF, so MIDV frames show the outline on an empty frame.
pub fn contact_sheet(
    out: &Path,
    items: &[ManifestItem],
    n: usize,
) -> Result<Option<String>, String> {
    if n == 0 || items.is_empty() {
        return Ok(None);
    }
    let step = (items.len() as f64 / n.min(items.len()) as f64).max(1.0);
    let mut html = String::from(
        "<!doctype html><meta charset=utf-8><title>quad contact sheet</title>\
         <style>body{font:12px sans-serif}figure{display:inline-block;margin:6px}\
         svg{width:320px;height:auto;background:#ccc}polygon{fill:rgba(255,0,0,.15);stroke:red;stroke-width:3}\
         circle{fill:#06f}</style><h1>Check every outline sits on the page edges</h1>",
    );
    let mut i = 0.0;
    let mut shown = 0;
    while (i as usize) < items.len() && shown < n {
        let it = &items[i as usize];
        let (w, h) = (f64::from(it.width), f64::from(it.height));
        let pts: Vec<String> = it
            .quad
            .iter()
            .map(|p| format!("{:.1},{:.1}", p[0] * w, p[1] * h))
            .collect();
        html.push_str(&format!(
            "<figure><svg viewBox=\"0 0 {w} {h}\"><image href=\"{img}\" width=\"{w}\" height=\"{h}\"/>\
             <polygon points=\"{pts}\"/><circle cx=\"{x:.1}\" cy=\"{y:.1}\" r=\"{r:.0}\"/></svg>\
             <figcaption>{id}</figcaption></figure>",
            img = html_escape(&it.image),
            pts = pts.join(" "),
            x = it.quad[0][0] * w,
            y = it.quad[0][1] * h,
            r = (w.max(h) / 100.0).max(4.0),
            id = html_escape(&it.id),
        ));
        shown += 1;
        i += step;
    }
    fs::write(out.join("contact-sheet.html"), html).map_err(|e| e.to_string())?;
    Ok(Some("contact-sheet.html".to_owned()))
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Builds an item with the shared defaults.
#[allow(clippy::too_many_arguments)]
pub fn item(
    id: String,
    image: String,
    scene_id: String,
    split: &str,
    size: (u32, u32),
    quad: Quad,
    tags: BTreeMap<String, String>,
) -> ManifestItem {
    ManifestItem {
        v: manifest::MANIFEST_VERSION,
        id,
        image,
        scene_id,
        split: Some(split.to_owned()),
        width: size.0,
        height: size.1,
        quad,
        items: Vec::new(),
        curves: None,
        items_curves: Vec::new(),
        tags,
    }
}

/// Everything a quad adapter needs to turn one ground-truth record into a manifest item.
#[allow(clippy::too_many_arguments)]
pub fn quad_item(
    info: &CorpusInfo,
    opts: &IngestOpts,
    out: &Path,
    image: &Path,
    id: String,
    scene_id: String,
    px: &[[f64; 2]; 4],
    mut tags: BTreeMap<String, String>,
) -> Result<ManifestItem, &'static str> {
    let size = image_size(image)?;
    let quad = normalise(px, size.0, size.1).ok_or("non-finite-quad")?;
    let rel = rel_posix(out, image).map_err(|_| "image-outside-output-dir")?;
    tags.insert("dataset".to_owned(), info.name.clone());
    tags.insert("format".to_owned(), format_tag(image));
    tags.insert(
        "aspect".to_owned(),
        aspect_tag(&[
            [px[0][0], px[0][1]],
            [px[1][0], px[1][1]],
            [px[2][0], px[2][1]],
            [px[3][0], px[3][1]],
        ])
        .to_owned(),
    );
    let split = split_for(&scene_id, opts.dev_percent);
    let it = item(id, rel, scene_id, split, size, quad, tags);
    if manifest::validate_item(&it).is_empty() {
        Ok(it)
    } else {
        // Typically a quad wound counter-clockwise, or one with crossing edges. Taken as given
        // from the dataset: never silently "fixed", always counted.
        Err("invalid-quad")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_handles_quotes_crlf_and_blank_lines() {
        let rows = parse_csv("a,b,c\r\n1,\"x,y\",\"q\"\"r\"\r\n\r\n4,5,6");
        assert_eq!(
            rows,
            vec![
                vec!["a", "b", "c"],
                vec!["1", "x,y", "q\"r"],
                vec!["4", "5", "6"]
            ]
        );
        assert!(parse_csv("").is_empty());
    }

    #[test]
    fn columns_are_found_by_name_and_missing_ones_are_reported() {
        let h: Vec<String> = ["Model_Name", " frame_index "].map(str::to_owned).to_vec();
        let c = Columns::new(&h);
        assert_eq!(
            c.require(&["frame_index", "model_name"]).unwrap(),
            vec![1, 0]
        );
        let e = c.require(&["tl_x"]).unwrap_err();
        assert!(e.contains("tl_x") && e.contains("model_name"), "{e}");
    }

    #[test]
    fn ordering_gives_clockwise_from_the_top_left_for_any_input_order() {
        let tl = [10.0, 10.0];
        let tr = [90.0, 12.0];
        let br = [88.0, 60.0];
        let bl = [8.0, 58.0];
        for perm in [
            [tl, tr, br, bl],
            [br, tl, bl, tr],
            [bl, br, tr, tl],
            [tr, bl, tl, br],
        ] {
            assert_eq!(order_clockwise(&perm), [tl, tr, br, bl]);
        }
    }

    #[test]
    fn splits_are_stable_and_close_to_the_requested_share() {
        assert_eq!(split_for("scene-1", 30), split_for("scene-1", 30));
        let dev = (0..2000)
            .filter(|i| split_for(&format!("scene-{i}"), 30) == "dev")
            .count();
        assert!((500..700).contains(&dev), "{dev}");
        assert_eq!(split_for("x", 0), "test");
        assert_eq!(split_for("x", 100), "dev");
    }

    #[test]
    fn aspect_tags_and_normalisation() {
        let doc = [[0.0, 0.0], [140.0, 0.0], [140.0, 100.0], [0.0, 100.0]];
        let strip = [[0.0, 0.0], [300.0, 0.0], [300.0, 100.0], [0.0, 100.0]];
        assert_eq!(aspect_tag(&doc), "document");
        assert_eq!(aspect_tag(&strip), "receipt");
        assert_eq!(normalise(&doc, 280, 200).unwrap()[2], [0.5, 0.5]);
        let mut bad = doc;
        bad[1][0] = f64::NAN;
        assert!(normalise(&bad, 10, 10).is_none());
    }

    #[test]
    fn every_nth_keeps_the_first_and_every_nth_after() {
        assert_eq!(every_nth((0..10).collect(), 4), vec![0, 4, 8]);
        assert_eq!(every_nth((0..3).collect(), 0), vec![0, 1, 2]);
    }

    #[test]
    fn an_empty_or_invalid_manifest_is_never_written() {
        let dir = std::env::temp_dir().join(format!("auto-crop-common-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let info = CorpusInfo {
            name: "n".into(),
            adapter: "a".into(),
            spdx: "CC0-1.0".into(),
            attribution: String::new(),
            licence_url: String::new(),
        };
        let mut r = Report::new("a");
        assert!(write_manifest(&dir, &info, vec![], &mut r).is_err());
        let ok = item(
            "a".into(),
            "i.png".into(),
            "s".into(),
            "dev",
            (10, 10),
            [[0.1, 0.1], [0.9, 0.1], [0.9, 0.9], [0.1, 0.9]],
            BTreeMap::new(),
        );
        let mut leaky = ok.clone();
        leaky.id = "b".into();
        leaky.split = Some("test".into());
        let e = write_manifest(&dir, &info, vec![ok.clone(), leaky], &mut r).unwrap_err();
        assert!(e.contains("appears in splits"), "{e}");
        assert!(!dir.join("manifest.jsonl").exists());
        write_manifest(&dir, &info, vec![ok], &mut r).unwrap();
        let text = fs::read_to_string(dir.join("manifest.jsonl")).unwrap();
        assert!(text.contains("\"licence\":\"CC0-1.0\""), "{text}");
        let _ = fs::remove_dir_all(&dir);
    }
}
