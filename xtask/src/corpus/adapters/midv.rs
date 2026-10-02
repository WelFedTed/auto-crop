// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! MIDV-500 adapter (ROADMAP M1.38): identity-document frames with a quad per frame.
//!
//! **Licence.** The provenance log has MIDV-500 as `pending` (NOASSERTION): the source documents
//! are public domain or openly licensed per the paper, to be checked per release. `corpus.lock.toml`
//! therefore carries `NOASSERTION`, and `fetch-corpus` and `corpus-ingest` both refuse an
//! uncleared licence. Recording the checked licence in the lock entry is what unlocks this
//! adapter.
//!
//! **UNVERIFIED against the real dataset.** Nothing was downloaded when this was written. The
//! layout is the published one as remembered: one directory per document class, each with
//!
//! * `<doc>/images/<COND>/<CLIP>/<frame>.tif` (`COND` is the capture condition such as `TS`,
//!   `TA`, `HA`, `KA`, `PA`, `PS`, `MA`, `CA`, `CS`; `CLIP` such as `TS01`), and
//! * `<doc>/ground_truth/<COND>/<CLIP>/<frame>.json` holding `{"quad": [[x, y], ...]}` with the
//!   four document corners in frame pixels, taken as listed in the order top-left, top-right,
//!   bottom-right, bottom-left. A quad that is not clockwise in that order is counted
//!   (`invalid-quad`) and left out, never reordered, so a wrong assumption shows up as a loud
//!   skip count instead of silently wrong labels.
//!
//! Other JSON files (per-document field annotations) are ignored. Every Nth frame per clip is
//! kept (`--every`, default 10 as everywhere; pass `--every 1` for all). The default scene is the
//! **document class** (`--scene-by document`), because the same document template appears in
//! every clip and condition; `--scene-by clip` gives the looser per-clip scenes of SmartDoc.

use super::common::{
    CorpusInfo, IngestOpts, Report, SceneBy, every_nth, finish, is_image_ext, list_files,
    lower_ext, quad_item,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

fn quad_of(v: &Value) -> Option<[[f64; 2]; 4]> {
    let a = v.get("quad")?.as_array()?;
    let pts: Vec<[f64; 2]> = a
        .iter()
        .filter_map(|p| {
            let p = p.as_array()?;
            Some([p.first()?.as_f64()?, p.get(1)?.as_f64()?])
        })
        .collect();
    pts.try_into().ok()
}

fn image_for(images_dir: &Path, stem: &str) -> Option<PathBuf> {
    ["tif", "tiff", "jpg", "jpeg", "png"]
        .iter()
        .map(|e| images_dir.join(format!("{stem}.{e}")))
        .find(|p| p.is_file() && is_image_ext(&lower_ext(p)))
}

pub fn ingest(
    src: &Path,
    out: &Path,
    info: &CorpusInfo,
    opts: &IngestOpts,
) -> Result<Report, String> {
    let mut report = Report::new("midv-500");
    let scene_by = opts.scene_by.unwrap_or(SceneBy::Document);
    // (doc, cond, clip) -> frames (stem, quad)
    type Key = (String, String, String);
    type Frames = Vec<(String, [[f64; 2]; 4])>;
    let mut clips: BTreeMap<Key, Frames> = BTreeMap::new();
    for p in list_files(src)? {
        if lower_ext(&p) != "json" {
            continue;
        }
        let rel: Vec<String> = p
            .strip_prefix(src)
            .map_err(|e| e.to_string())?
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        if rel.len() != 5 || rel[1] != "ground_truth" {
            continue;
        }
        let Some(stem) = p.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
            continue;
        };
        let doc: Value = match fs::read_to_string(&p)
            .map_err(|e| e.to_string())
            .and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()))
        {
            Ok(v) => v,
            Err(_) => {
                report.skip("unparsable-json");
                continue;
            }
        };
        let Some(quad) = quad_of(&doc) else {
            report.skip("no-quad");
            continue;
        };
        clips
            .entry((rel[0].clone(), rel[2].clone(), rel[3].clone()))
            .or_default()
            .push((stem, quad));
    }

    let mut items = Vec::new();
    for ((doc, cond, clip), mut frames) in clips {
        frames.sort_by(|a, b| a.0.cmp(&b.0));
        for (stem, px) in every_nth(frames, opts.every) {
            let images_dir = src.join(&doc).join("images").join(&cond).join(&clip);
            let Some(image) = image_for(&images_dir, &stem) else {
                report.skip("frame-image-not-found");
                continue;
            };
            let scene = match scene_by {
                SceneBy::Document => format!("midv500-{doc}"),
                SceneBy::Clip => format!("midv500-{doc}-{clip}"),
            };
            let tags = BTreeMap::from([
                ("doctype".to_owned(), doc.clone()),
                ("condition".to_owned(), cond.clone()),
            ]);
            match quad_item(
                info,
                opts,
                out,
                &image,
                format!("midv500-{doc}-{stem}"),
                scene,
                &px,
                tags,
            ) {
                Ok(it) => items.push(it),
                Err(reason) => report.skip(reason),
            }
        }
    }
    finish(out, info, opts, items, &mut report)?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quads_are_read_and_anything_else_is_not_a_quad() {
        let v: Value =
            serde_json::from_str(r#"{"quad":[[1,2],[3,2],[3,4],[1,4]],"field01":{}}"#).unwrap();
        assert_eq!(quad_of(&v).unwrap()[2], [3.0, 4.0]);
        for bad in [
            r#"{"field01":{}}"#,
            r#"{"quad":[[1,2],[3,2],[3,4]]}"#,
            r#"{"quad":"x"}"#,
        ] {
            let v: Value = serde_json::from_str(bad).unwrap();
            assert!(quad_of(&v).is_none(), "{bad}");
        }
    }
}
