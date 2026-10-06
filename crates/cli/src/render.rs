// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `auto-crop render`: write the crop of one image to a chosen file, without touching the source
//! and without the backup store (ROADMAP M2.81). The crop is the detector's, or the edit state of
//! `--edit FILE` (what `analyze --emit-edit` wrote, possibly changed by hand). It uses the same
//! decode, render and encode as `process`, so for the same quality the bytes are the same.
//!
//! `render` is the explicit tool: it writes what it is asked to even when `process` would hold
//! the result (it says so on stderr), and refuses only what cannot be done: no crop, an output
//! that is the source, an output that exists (unless `--force`).

use crate::args::{FormatArg, RenderArgs};
use crate::env::{Env, base_settings, cpu_floor};
use crate::exit;
use crate::inputs::Candidate;
use crate::manifest::{ConfidenceRec, OutputRec};
use crate::pipeline::{self, Prepared, Shared, confidence_rec, err_code};
use crate::writer::{self, format_name, out_format};
use auto_crop_codecs::Format;
use auto_crop_core::{ErrKind, Geometry, QuadWarp, STRICT_CUTOFF};
use auto_crop_engine::fsplan::expand_name;
use auto_crop_engine::{Engine, Housekeeping, RunOptions};
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Serialize)]
struct Doc {
    schema: &'static str,
    v: u32,
    input: String,
    outputs: Vec<OutputRec>,
    /// `detected` or `edit`.
    source: &'static str,
    confidence: Option<ConfidenceRec>,
    /// What `process` would have done at the Strict cut-off, for a detected crop.
    would_hold: Option<bool>,
}

fn fail(msg: &str, code: u8) -> u8 {
    eprintln!("error: {msg}");
    code
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// The format of the output file: `--format`, else the output's own extension when it is a
/// format this build writes, else the source's.
fn format_for(args: &RenderArgs, source: Format) -> Format {
    if args.format != FormatArg::Keep {
        return out_format(source, args.format);
    }
    match args
        .output
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .as_deref()
    {
        Some("jpg" | "jpeg") => Format::Jpeg,
        Some("png") => Format::Png,
        _ => out_format(source, FormatArg::Keep),
    }
}

/// The paths of `count` outputs: the given path for one, `<stem>_01.<ext>`... for several.
fn output_paths(out: &Path, count: usize, ext: &str) -> Result<Vec<PathBuf>, ErrKind> {
    if count == 1 {
        return Ok(vec![out.to_path_buf()]);
    }
    let dir = out.parent().map(Path::to_path_buf).unwrap_or_default();
    let stem = out
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "image".to_owned());
    (1..=count)
        .map(|n| {
            expand_name("{name}_{n}", &stem, Some((n, count)), ext)
                .map(|name| dir.join(name))
                .map_err(|_| ErrKind::UnsupportedOutput)
        })
        .collect()
}

pub fn run(args: RenderArgs, env: &Env) -> u8 {
    let g = &env.global;
    if let Err(m) = cpu_floor() {
        return fail(&m, exit::PRECONDITION);
    }
    let input = PathBuf::from(&args.input);
    let abs = std::path::absolute(&input).unwrap_or_else(|_| input.clone());
    if !abs.is_file() {
        return fail(&format!("{} is not a file", args.input), exit::FAILED);
    }
    if same_file(&abs, &args.output) || abs == std::path::absolute(&args.output).unwrap_or_default()
    {
        return fail(
            "the output is the source: render never touches the source",
            exit::USAGE,
        );
    }
    let cand = Candidate {
        path: abs.clone(),
        display: args.input.clone(),
        root: abs.parent().map(Path::to_path_buf).unwrap_or_default(),
    };

    // The crops: from the edit file, or detected.
    let mut confidence: Option<ConfidenceRec> = None;
    let mut would_hold = None;
    let (quads, source_format, source): (Vec<QuadWarp>, Format, &'static str) = if let Some(edit) =
        &args.edit
    {
        let text = match fs::read_to_string(edit) {
            Ok(t) => t,
            Err(e) => {
                return fail(
                    &format!("cannot read {}: {e}", edit.display()),
                    exit::FAILED,
                );
            }
        };
        let state = match auto_crop_engine::migrate::migrate_str(&text) {
            Ok(s) => s,
            Err(k) => {
                return fail(
                    &format!(
                        "{} is not a usable edit state ({})",
                        edit.display(),
                        err_code(k)
                    ),
                    exit::FAILED,
                );
            }
        };
        let quads: Vec<QuadWarp> = state
            .included()
            .filter_map(|i| match &i.geometry {
                Geometry::Quad(q) => Some(q.clone().sanitised()),
                _ => None,
            })
            .filter(|q| q.check().is_ok())
            .collect();
        let fmt = fs::read(&abs)
            .ok()
            .and_then(|b| auto_crop_codecs::probe(&b).ok())
            .map(|p| p.format)
            .unwrap_or(Format::Jpeg);
        (quads, fmt, "edit")
    } else {
        let mut settings = base_settings(&env.paths, g);
        settings.split_policy = args.split;
        settings.split_profile = args.profile;
        let engine = Engine::with_settings(env.paths.clone(), settings, Housekeeping::None);
        engine.set_run_options(RunOptions {
            margin_pct: args.margin,
        });
        engine.set_options(args.knobs.engine_options());
        let sh = Shared {
            engine,
            budget: crate::process::budget(None),
            cancel: env.cancel.clone(),
            cutoff: STRICT_CUTOFF,
            processed: HashMap::new(),
            reprocess: true,
            replacing: false,
            delay: Duration::ZERO,
        };
        let ready = match pipeline::prepare(&sh, 0, &cand) {
            Prepared::Done(rec) => {
                return fail(
                    &format!(
                        "{}: {}",
                        args.input,
                        rec.code.as_deref().unwrap_or("not processed")
                    ),
                    exit::FAILED,
                );
            }
            Prepared::Ready(r) => r,
        };
        confidence = ready
            .view
            .confidence
            .as_ref()
            .map(|c| confidence_rec(c, STRICT_CUTOFF));
        would_hold = Some(matches!(
            pipeline::decide(&ready.view, STRICT_CUTOFF),
            pipeline::Decision::Hold { .. }
        ));
        let quads: Vec<QuadWarp> = sh
            .engine
            .edit_state(ready.id)
            .map(|s| {
                s.included()
                    .filter_map(|i| i.geometry.quad().cloned())
                    .collect()
            })
            .unwrap_or_default();
        let fmt = ready.format;
        sh.engine.remove_items(&[ready.id]);
        if would_hold == Some(true) && !g.quiet {
            eprintln!(
                "note: `process` would hold this result at the strict cut-off ({}); written because you asked for it",
                confidence
                    .as_ref()
                    .map_or("no confidence".to_owned(), |c| format!(
                        "score {}, {}",
                        c.score, c.band
                    ))
            );
        }
        (quads, fmt, "detected")
    };
    if quads.is_empty() {
        return fail(
            "there is no crop to render (the detector found none; use `--edit`)",
            exit::FAILED,
        );
    }

    let format = format_for(&args, source_format);
    let paths = match output_paths(&args.output, quads.len(), format.extension()) {
        Ok(p) => p,
        Err(k) => {
            return fail(
                &format!("cannot name the outputs ({})", err_code(k)),
                exit::USAGE,
            );
        }
    };
    if !args.force
        && let Some(p) = paths.iter().find(|p| fs::symlink_metadata(p).is_ok())
    {
        return fail(
            &format!("{} exists; use --force to replace it", p.display()),
            exit::FAILED,
        );
    }
    if paths.iter().any(|p| same_file(p, &abs)) {
        return fail(
            "an output is the source: render never touches the source",
            exit::USAGE,
        );
    }
    let rendered = (|| -> Result<Vec<writer::Rendered>, ErrKind> {
        let bytes = fs::read(&abs).map_err(|e| ErrKind::from_io(&e))?;
        writer::render_all(&bytes, &quads, format, &args.knobs.engine_options())
    })();
    let files = match rendered {
        Ok(f) => f,
        Err(k) => return fail(&format!("{}: {}", args.input, err_code(k)), exit::FAILED),
    };
    let mtime = fs::metadata(&abs).and_then(|m| m.modified()).ok();
    if let Err(k) = writer::commit_with(&paths, &files, mtime, args.force) {
        return fail(
            &format!("could not write the output ({})", err_code(k)),
            exit::FAILED,
        );
    }
    let outputs: Vec<OutputRec> = paths
        .iter()
        .zip(&files)
        .map(|(p, f)| OutputRec {
            path: std::path::absolute(p)
                .unwrap_or_else(|_| p.clone())
                .to_string_lossy()
                .into_owned(),
            bytes: fs::metadata(p).ok().map(|m| m.len()),
            width: Some(f.width),
            height: Some(f.height),
            format: Some(format_name(f.format)),
        })
        .collect();
    if g.json {
        let doc = Doc {
            schema: "auto-crop/render",
            v: crate::manifest::SCHEMA_VERSION,
            input: args.input.clone(),
            outputs,
            source,
            confidence,
            would_hold,
        };
        let _ = writeln!(
            std::io::stdout().lock(),
            "{}",
            serde_json::to_string_pretty(&doc).unwrap_or_default()
        );
    } else {
        let mut out = std::io::stdout().lock();
        for o in &outputs {
            let _ = writeln!(out, "{}", o.path);
        }
    }
    exit::OK
}
