// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! SmartDoc 2015 Challenge 1 adapter (ROADMAP M1.37). Licence CC BY 4.0 (the attribution text
//! of the lock entry is copied into every manifest line; the organisers ask to be cited and
//! emailed).
//!
//! **UNVERIFIED against the real dataset.** Nothing was downloaded when this was written. The
//! facts taken from the research notes: about 150 clips of about 24,000 frames, five
//! backgrounds, one page quad per frame, CC BY 4.0, Zenodo record 1230217. The file layout below
//! is a best understanding of the published release and must be confirmed at first fetch; the
//! adapter is deliberately tolerant and fails loudly (no manifest at all) when the data does not
//! look like this:
//!
//! * a ground-truth table `metadata.csv` (or `metadata.csv.gz`) anywhere under the root, with
//!   the header columns `bg_name, model_name, frame_index, tl_x, tl_y, tr_x, tr_y, br_x, br_y,
//!   bl_x, bl_y` (other columns are ignored; the corner coordinates are in frame pixels);
//! * frames as `<anything>/<bg_name>/<model_name>/<digits>.jpg|jpeg|png`, where the trailing
//!   digits of the file name are the `frame_index` (the images' own width and height come from
//!   their headers).
//!
//! A clip is one `(bg_name, model_name)` pair. Every Nth frame of a clip is kept (`--every`,
//! default 10): consecutive video frames are near duplicates, so the full 24,000 add nothing but
//! runtime. All frames of a clip share one `scene_id` (`smartdoc15-<bg>-<model>`), so the
//! dev/test split never separates two frames of the same clip. `--scene-by document` groups by
//! page model across all backgrounds instead, which is stricter and what the training splits of
//! M4.13 want.

use super::common::{
    Columns, CorpusInfo, IngestOpts, Report, SceneBy, every_nth, finish, is_image_ext, list_files,
    lower_ext, parse_csv, quad_item, read_text_maybe_gz,
};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

const REQUIRED: [&str; 11] = [
    "bg_name",
    "model_name",
    "frame_index",
    "tl_x",
    "tl_y",
    "tr_x",
    "tr_y",
    "br_x",
    "br_y",
    "bl_x",
    "bl_y",
];

fn trailing_number(stem: &str) -> Option<u64> {
    let digits: String = stem
        .chars()
        .rev()
        .take_while(char::is_ascii_digit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    digits.parse().ok()
}

fn doctype(model: &str) -> String {
    model
        .trim_end_matches(|c: char| c.is_ascii_digit() || c == '_' || c == '-')
        .to_owned()
}

pub fn ingest(
    src: &Path,
    out: &Path,
    info: &CorpusInfo,
    opts: &IngestOpts,
) -> Result<Report, String> {
    let mut report = Report::new("smartdoc2015-ch1");
    let files = list_files(src)?;
    let meta: PathBuf = files
        .iter()
        .find(|p| {
            let n = p
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            n == "metadata.csv" || n == "metadata.csv.gz"
        })
        .cloned()
        .ok_or("smartdoc2015-ch1: no metadata.csv or metadata.csv.gz under the data root")?;
    let rows = parse_csv(&read_text_maybe_gz(&meta)?);
    let (header, body) = rows
        .split_first()
        .ok_or_else(|| format!("{}: empty", meta.display()))?;
    let cols = Columns::new(header);
    let idx = cols
        .require(&REQUIRED)
        .map_err(|e| format!("{}: {e}", meta.display()))?;
    let (c_bg, c_model, c_frame) = (idx[0], idx[1], idx[2]);

    // (background, model, frame) -> image path
    let mut images: HashMap<(String, String, u64), PathBuf> = HashMap::new();
    for p in &files {
        if !is_image_ext(&lower_ext(p)) {
            continue;
        }
        let model = p.parent().and_then(Path::file_name);
        let bg = p.parent().and_then(Path::parent).and_then(Path::file_name);
        let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned());
        if let (Some(model), Some(bg), Some(stem)) = (model, bg, stem)
            && let Some(n) = trailing_number(&stem)
        {
            images.insert(
                (
                    bg.to_string_lossy().into_owned(),
                    model.to_string_lossy().into_owned(),
                    n,
                ),
                p.clone(),
            );
        }
    }

    type Clip = Vec<(u64, [[f64; 2]; 4])>;
    let mut clips: BTreeMap<(String, String), Clip> = BTreeMap::new();
    for row in body {
        let get = |i: usize| row.get(i).map(|s| s.trim()).unwrap_or("");
        let (bg, model) = (get(c_bg).to_owned(), get(c_model).to_owned());
        let Ok(frame) = get(c_frame).parse::<f64>().map(|f| f as u64) else {
            report.skip("bad-frame-index");
            continue;
        };
        let nums: Vec<&str> = idx[3..].iter().map(|&i| get(i)).collect();
        if nums.iter().all(|s| s.is_empty()) {
            report.skip("no-document-in-frame");
            continue;
        }
        let parsed: Vec<Option<f64>> = nums.iter().map(|s| s.parse::<f64>().ok()).collect();
        if parsed.iter().any(Option::is_none) {
            report.skip("bad-quad-numbers");
            continue;
        }
        let v: Vec<f64> = parsed.into_iter().flatten().collect();
        // Header order is tl, tr, br, bl (REQUIRED), each as x then y.
        let quad = [[v[0], v[1]], [v[2], v[3]], [v[4], v[5]], [v[6], v[7]]];
        clips.entry((bg, model)).or_default().push((frame, quad));
    }

    let mut items = Vec::new();
    for ((bg, model), mut frames) in clips {
        frames.sort_by_key(|f| f.0);
        frames.dedup_by_key(|f| f.0);
        for (frame, px) in every_nth(frames, opts.every) {
            let Some(path) = images.get(&(bg.clone(), model.clone(), frame)) else {
                report.skip("frame-image-not-found");
                continue;
            };
            let scene = match opts.scene_by.unwrap_or(SceneBy::Clip) {
                SceneBy::Clip => format!("smartdoc15-{bg}-{model}"),
                SceneBy::Document => format!("smartdoc15-{model}"),
            };
            let tags = BTreeMap::from([
                ("background".to_owned(), bg.clone()),
                ("doctype".to_owned(), doctype(&model)),
            ]);
            match quad_item(
                info,
                opts,
                out,
                path,
                format!("smartdoc15-{bg}-{model}-{frame:06}"),
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
