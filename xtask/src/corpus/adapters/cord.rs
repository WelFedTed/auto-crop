// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! CORD adapter (ROADMAP M1.37): receipt photographs with a receipt outline and the text on
//! them. Licence CC BY 4.0 (attribution is carried in every output line).
//!
//! **UNVERIFIED against the real dataset.** Nothing was downloaded when this was written, and
//! the research notes themselves say CORD's ROI field "needs verification". Assumed layout, the
//! original GitHub-style release:
//!
//! * `<split>/image/<name>.png|jpg|jpeg` and `<split>/json/<name>.json` (`split` is train, dev
//!   or test; it is kept as the `cord_split` tag, while the harness `split` is a scene hash like
//!   everywhere else);
//! * each JSON has `valid_line: [{words: [{text: ".."}, ..]}, ..]` (the transcript) and a
//!   receipt outline as `roi` (or `meta.roi`): either an object `{x1,y1,..,x4,y4}` or an array
//!   of four `[x, y]` / `{x, y}` points, in image pixels. The corner order is not assumed: the
//!   four points are put in clockwise order from the top-left corner. If `meta.image_size` is
//!   present it must equal the image header's size, else the receipt is skipped.
//!
//! Outputs: `manifest.jsonl` for the receipts that have an outline (one scene per receipt) and
//! `transcripts.jsonl` (`id`, `text`) for the CER checks of the enhancement stage (M1.66). The
//! HuggingFace parquet release is not read by this adapter. A tree with no outline at all
//! produces an error and no manifest, so a wrong layout assumption cannot pass silently.

use super::common::{
    CorpusInfo, IngestOpts, Report, finish, is_image_ext, list_files, lower_ext, order_clockwise,
    quad_item,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

fn point(v: &Value) -> Option<[f64; 2]> {
    match v {
        Value::Array(a) if a.len() == 2 => Some([a[0].as_f64()?, a[1].as_f64()?]),
        Value::Object(o) => Some([o.get("x")?.as_f64()?, o.get("y")?.as_f64()?]),
        _ => None,
    }
}

/// The receipt outline of a CORD JSON, in pixels, clockwise from the top-left.
pub fn roi_of(doc: &Value) -> Option<[[f64; 2]; 4]> {
    let roi = doc.get("roi").or_else(|| doc.get("meta")?.get("roi"))?;
    let pts: Vec<[f64; 2]> = match roi {
        Value::Array(a) => a.iter().filter_map(point).collect(),
        Value::Object(o) => (1..=4)
            .filter_map(|i| {
                Some([
                    o.get(&format!("x{i}"))?.as_f64()?,
                    o.get(&format!("y{i}"))?.as_f64()?,
                ])
            })
            .collect(),
        _ => return None,
    };
    let pts: [[f64; 2]; 4] = pts.try_into().ok()?;
    Some(order_clockwise(&pts))
}

/// All word texts of the receipt: words joined by a space, lines by a newline.
pub fn transcript_of(doc: &Value) -> String {
    let mut lines = Vec::new();
    for line in doc
        .get("valid_line")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let words: Vec<&str> = line
            .get("words")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|w| w.get("text")?.as_str())
            .filter(|t| !t.trim().is_empty())
            .collect();
        if !words.is_empty() {
            lines.push(words.join(" "));
        }
    }
    lines.join("\n")
}

fn declared_size(doc: &Value) -> Option<(u32, u32)> {
    let s = doc.get("meta")?.get("image_size")?;
    Some((
        u32::try_from(s.get("width")?.as_u64()?).ok()?,
        u32::try_from(s.get("height")?.as_u64()?).ok()?,
    ))
}

pub fn ingest(
    src: &Path,
    out: &Path,
    info: &CorpusInfo,
    opts: &IngestOpts,
) -> Result<Report, String> {
    let mut report = Report::new("cord");
    let files = list_files(src)?;
    let mut items = Vec::new();
    let mut transcripts = String::new();
    for json in files.iter().filter(|p| lower_ext(p) == "json") {
        let Some(json_dir) = json.parent() else {
            continue;
        };
        if json_dir
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            != Some("json".into())
        {
            continue;
        }
        let Some(split_dir) = json_dir.parent() else {
            continue;
        };
        let split_name = split_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let Some(stem) = json.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
            continue;
        };
        let image = ["png", "jpg", "jpeg"]
            .iter()
            .map(|e| split_dir.join("image").join(format!("{stem}.{e}")))
            .find(|p| p.is_file() && is_image_ext(&lower_ext(p)));
        let Some(image) = image else {
            report.skip("image-not-found");
            continue;
        };
        let doc: Value = match fs::read_to_string(json)
            .map_err(|e| e.to_string())
            .and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()))
        {
            Ok(v) => v,
            Err(_) => {
                report.skip("unparsable-json");
                continue;
            }
        };
        let id = format!("cord-{}-{stem}", split_name.to_lowercase());
        let text = transcript_of(&doc);
        if !text.is_empty() {
            transcripts.push_str(
                &serde_json::json!({
                    "v": 1, "id": id, "text": text, "licence": info.spdx,
                    "attribution": info.attribution, "source": info.name
                })
                .to_string(),
            );
            transcripts.push('\n');
        }
        let Some(px) = roi_of(&doc) else {
            report.skip("no-roi");
            continue;
        };
        if let Some(declared) = declared_size(&doc)
            && super::common::image_size(&image).is_ok_and(|real| real != declared)
        {
            report.skip("image-size-differs-from-meta");
            continue;
        }
        let tags = BTreeMap::from([("cord_split".to_owned(), split_name.to_lowercase())]);
        match quad_item(info, opts, out, &image, id.clone(), id, &px, tags) {
            Ok(it) => items.push(it),
            Err(reason) => report.skip(reason),
        }
    }
    if !transcripts.is_empty() {
        fs::create_dir_all(out).map_err(|e| e.to_string())?;
        fs::write(out.join("transcripts.jsonl"), transcripts).map_err(|e| e.to_string())?;
        report.outputs.push("transcripts.jsonl".to_owned());
    }
    finish(out, info, opts, items, &mut report)?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roi_accepts_objects_and_point_arrays_in_any_order() {
        let a: Value = serde_json::from_str(
            r#"{"roi":{"x1":10,"y1":10,"x2":10,"y2":90,"x3":50,"y3":90,"x4":50,"y4":10}}"#,
        )
        .unwrap();
        // given counter-clockwise, returned clockwise from the top-left
        assert_eq!(
            roi_of(&a).unwrap(),
            [[10.0, 10.0], [50.0, 10.0], [50.0, 90.0], [10.0, 90.0]]
        );
        let b: Value =
            serde_json::from_str(r#"{"meta":{"roi":[[50,90],[10,90],{"x":10,"y":10},[50,10]]}}"#)
                .unwrap();
        assert_eq!(roi_of(&b), roi_of(&a));
        let none: Value = serde_json::from_str(r#"{"roi":{"x1":1}}"#).unwrap();
        assert!(roi_of(&none).is_none());
    }

    #[test]
    fn transcripts_join_words_and_lines() {
        let d: Value = serde_json::from_str(
            r#"{"valid_line":[{"words":[{"text":"NASI"},{"text":"GORENG"}]},{"words":[{"text":" "}]},{"words":[{"text":"15.000"}]}]}"#,
        )
        .unwrap();
        assert_eq!(transcript_of(&d), "NASI GORENG\n15.000");
    }

    #[test]
    fn the_declared_size_is_read() {
        let d: Value =
            serde_json::from_str(r#"{"meta":{"image_size":{"width":864,"height":1296}}}"#).unwrap();
        assert_eq!(declared_size(&d), Some((864, 1296)));
        assert_eq!(declared_size(&Value::Null), None);
    }

    #[test]
    fn extension_helper_is_not_confused_by_case() {
        assert_eq!(lower_ext(Path::new("A.PNG")), "png");
    }
}
