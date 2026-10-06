// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `auto-crop analyze`: detection and the confidence report, no images written (ROADMAP M2.81).
//! It runs exactly the analysis `process` runs and reports, per image, the crops, the confidence
//! at the chosen cut-off and what `process` would do with the result. `--emit-edit DIR` also
//! writes the edit state of each image (`<name>.edit.json`), which `render --edit` applies.

use crate::args::AnalyzeArgs;
use crate::env::{Env, base_settings, cpu_floor, test_hook};
use crate::exit;
use crate::inputs::{self, Candidate};
use crate::manifest::{ConfidenceRec, Timing, code};
use crate::pipeline::{
    self, Decision, Prepared, Shared, confidence_rec, decide, err_code, split_reasons,
};
use crate::process::{budget, default_jobs, write_atomic};
use crate::report::Reporter;
use auto_crop_core::ErrKind;
use auto_crop_engine::{Engine, Housekeeping, RunOptions};
use serde::Serialize;
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

pub const SCHEMA_NAME: &str = "auto-crop/analysis";

#[derive(Debug, Clone, Serialize)]
pub struct CropOut {
    pub id: u32,
    /// 1-based output rank; 0 when excluded.
    pub order: u32,
    /// Corners TL, TR, BR, BL as fractions of the (EXIF-oriented) image, `[x, y]`.
    pub quad: [[f64; 2]; 4],
    pub quarter_turns: u8,
    pub fine_deg: f32,
    pub mirror: bool,
    pub confidence: Option<ConfidenceRec>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AnalysisItem {
    pub index: usize,
    pub input: String,
    /// `analysed`, `failed` or `skipped`.
    pub status: &'static str,
    pub code: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// What `process` would do at this cut-off: `write` or `held` (absent when not analysed).
    pub decision: Option<&'static str>,
    /// Why it would be held.
    pub hold_code: Option<String>,
    pub reasons: Vec<String>,
    pub confidence: Option<ConfidenceRec>,
    pub split: bool,
    pub crops: Vec<CropOut>,
    pub edit_file: Option<String>,
    pub ms: Option<Timing>,
}

impl AnalysisItem {
    fn bare(index: usize, input: &str, status: &'static str, c: Option<String>) -> Self {
        Self {
            index,
            input: input.to_owned(),
            status,
            code: c,
            width: None,
            height: None,
            decision: None,
            hold_code: None,
            reasons: Vec::new(),
            confidence: None,
            split: false,
            crops: Vec::new(),
            edit_file: None,
            ms: None,
        }
    }
}

#[derive(Debug, Serialize)]
struct Doc<'a> {
    schema: &'static str,
    v: u32,
    tool: crate::manifest::Tool,
    cutoff: f32,
    triage: &'static str,
    summary: DocSummary,
    items: &'a [AnalysisItem],
}

#[derive(Debug, Default, Clone, Copy, Serialize)]
struct DocSummary {
    items: usize,
    write: usize,
    held: usize,
    failed: usize,
    skipped: usize,
}

fn stem(path: &Path) -> String {
    auto_crop_engine::fsplan::sanitise_component(
        &path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
    )
}

fn analyse_one(
    sh: &Shared,
    args: &AnalyzeArgs,
    idx: usize,
    cand: &Candidate,
    emit_dir: Option<&Path>,
    taken: &std::sync::Mutex<std::collections::HashSet<String>>,
) -> AnalysisItem {
    let t = Instant::now();
    let ready = match pipeline::prepare(sh, idx, cand) {
        Prepared::Done(rec) => {
            let status = match rec.status {
                crate::manifest::Status::Failed => "failed",
                _ => "skipped",
            };
            let mut it = AnalysisItem::bare(idx, &cand.display, status, rec.code.clone());
            it.reasons = rec.reasons.clone();
            return it;
        }
        Prepared::Ready(r) => r,
    };
    let mut it = AnalysisItem::bare(idx, &cand.display, "analysed", None);
    it.width = Some(ready.view.width);
    it.height = Some(ready.view.height);
    it.confidence = ready
        .view
        .confidence
        .as_ref()
        .map(|c| confidence_rec(c, sh.cutoff));
    it.split = ready.view.split.as_ref().is_some_and(|s| s.is_split);
    it.crops = ready
        .view
        .crops
        .iter()
        .filter_map(|c| {
            let e = c.edit.as_ref()?;
            Some(CropOut {
                id: c.id,
                order: c.order,
                quad: e.quad.map(|p| [p.x, p.y]),
                quarter_turns: e.quarter_turns,
                fine_deg: e.fine_deg,
                mirror: c.mirror,
                confidence: c.confidence.as_ref().map(|c| confidence_rec(c, sh.cutoff)),
            })
        })
        .collect();
    match decide(&ready.view, sh.cutoff) {
        Decision::Write { split, .. } => {
            // A scan with several items is written as copies, and held in place unless accepted.
            it.decision = Some("write");
            if split {
                it.reasons = split_reasons(&ready.view, sh.cutoff);
            }
        }
        Decision::Hold { code, reasons } => {
            it.decision = Some("held");
            it.hold_code = Some(code.to_owned());
            it.reasons = reasons;
        }
    }
    if let (Some(dir), Some(state)) = (emit_dir, sh.engine.edit_state(ready.id)) {
        let mut name = format!("{}.edit.json", stem(&cand.path));
        {
            // Two inputs with one stem (a/x.jpg and b/x.jpg) get distinct files.
            let mut set = taken.lock().unwrap_or_else(|e| e.into_inner());
            let mut n = 1;
            while !set.insert(name.to_lowercase()) {
                n += 1;
                name = format!("{} ({n}).edit.json", stem(&cand.path));
            }
        }
        let path = dir.join(&name);
        match serde_json::to_string_pretty(&state)
            .map_err(|e| e.to_string())
            .and_then(|t| write_atomic(&path, &(t + "\n")).map_err(|e| e.to_string()))
        {
            Ok(()) => it.edit_file = Some(path.to_string_lossy().into_owned()),
            Err(e) => {
                it.status = "failed";
                it.code = Some(err_code(ErrKind::Unreadable));
                it.reasons.push(format!("edit file: {e}"));
            }
        }
    }
    sh.engine.remove_items(&[ready.id]);
    if args.timings {
        it.ms = Some(Timing {
            read: ready.read_ms,
            analyse: ready.analyse_ms,
            write: 0.0,
            total: pipeline::round_ms(t),
        });
    }
    it
}

fn text_line(it: &AnalysisItem) -> String {
    match it.status {
        "analysed" => {
            let dims = match (it.width, it.height) {
                (Some(w), Some(h)) => format!("{w}x{h}"),
                _ => String::new(),
            };
            let conf = it
                .confidence
                .as_ref()
                .map_or_else(String::new, |c| format!("score {:.2} {}", c.score, c.band));
            let what = match (&it.hold_code, it.split) {
                (Some(c), _) => format!("held {c}"),
                (None, true) => format!("write {} files (split)", it.crops.len()),
                (None, false) => "write".to_owned(),
            };
            let mut line = format!("{}  {dims}  {what}  {conf}", it.input);
            if !it.reasons.is_empty() {
                line.push_str(&format!(" [{}]", it.reasons.join(", ")));
            }
            for c in &it.crops {
                let q = c
                    .quad
                    .iter()
                    .map(|p| format!("({:.3},{:.3})", p[0], p[1]))
                    .collect::<Vec<_>>()
                    .join(" ");
                line.push_str(&format!("\n    crop {}: {q}", c.order.max(1)));
                if c.quarter_turns != 0 || c.fine_deg.abs() > 0.0 {
                    line.push_str(&format!(
                        " turn {}x90 + {:.2} deg",
                        c.quarter_turns, c.fine_deg
                    ));
                }
            }
            if let Some(m) = &it.ms {
                line.push_str(&format!(
                    "\n    ms: read {} analyse {} total {}",
                    m.read, m.analyse, m.total
                ));
            }
            if let Some(f) = &it.edit_file {
                line.push_str(&format!("\n    edit: {f}"));
            }
            line
        }
        other => format!("{}  {other} {}", it.input, it.code.as_deref().unwrap_or("")),
    }
}

pub fn run(args: AnalyzeArgs, env: &Env) -> u8 {
    let g = &env.global;
    if let Err(m) = cpu_floor() {
        eprintln!("error: {m}");
        return exit::PRECONDITION;
    }
    let store_dir = env.paths.backups_dir();
    if let Some(d) = &args.emit_edit
        && let Err(e) = std::fs::create_dir_all(d)
    {
        eprintln!("error: cannot create {}: {e}", d.display());
        return exit::PRECONDITION;
    }
    let exp = inputs::expand(&args.input, Some(&store_dir));
    let mut items: Vec<AnalysisItem> = Vec::new();
    for (typed, p) in &exp.problems {
        let status = match p {
            inputs::Problem::NotFound | inputs::Problem::NoMatch => "failed",
            _ => "skipped",
        };
        items.push(AnalysisItem::bare(
            items.len(),
            typed,
            status,
            Some(p.code().to_owned()),
        ));
    }
    let base = items.len();
    let mut settings = base_settings(&env.paths, g);
    settings.split_policy = args.detect.split;
    settings.split_profile = args.detect.profile;
    // Analysis writes nothing at all: no recovery sweep, no purge.
    let engine = Engine::with_settings(env.paths.clone(), settings, Housekeeping::None);
    engine.set_run_options(RunOptions {
        margin_pct: args.detect.margin,
    });
    engine.set_options(args.knobs.engine_options());
    let sh = Shared {
        engine,
        budget: budget(args.mem_limit_mb),
        cancel: env.cancel.clone(),
        cutoff: args.detect.cutoff,
        processed: HashMap::new(),
        reprocess: true,
        replacing: false,
        delay: Duration::from_millis(test_hook("AUTO_CROP_TEST_DELAY_MS").unwrap_or(0)),
    };
    let taken = std::sync::Mutex::new(std::collections::HashSet::new());
    let mut results: Vec<Option<AnalysisItem>> = vec![None; exp.candidates.len()];
    let jobs = args.jobs.unwrap_or_else(default_jobs);
    let mut rep = Reporter::new(g, crate::args::Progress::Never, exp.candidates.len(), true);
    let ndjson = g.ndjson;
    let emit_dir = args.emit_edit.as_deref();
    crate::pool::run(
        exp.candidates.len(),
        jobs,
        &sh.cancel,
        |i| analyse_one(&sh, &args, base + i, &exp.candidates[i], emit_dir, &taken),
        |i, it| {
            // Events stream as they finish (each carries its index); the text report is printed in
            // input order at the end, so its lines do not depend on --jobs.
            if ndjson {
                print_json_line("item", &it);
            }
            results[i] = Some(it);
        },
    );
    let cancelled = env.cancel.is_cancelled();
    for (i, slot) in results.into_iter().enumerate() {
        items.push(slot.unwrap_or_else(|| {
            AnalysisItem::bare(
                base + i,
                &exp.candidates[i].display,
                "skipped",
                Some(code::CANCELLED.to_owned()),
            )
        }));
    }
    items.sort_by_key(|i| i.index);
    if !g.json && !ndjson {
        for it in &items {
            println!("{}", text_line(it));
        }
    }
    let mut sum = DocSummary {
        items: items.len(),
        ..DocSummary::default()
    };
    for it in &items {
        match (it.status, it.decision) {
            ("analysed", Some("write")) => sum.write += 1,
            ("analysed", _) => sum.held += 1,
            ("failed", _) => sum.failed += 1,
            _ => sum.skipped += 1,
        }
    }
    let code = exit::batch_exit(exit::Outcome {
        failed: sum.failed,
        // Analysis reports holds; it does not fail on them.
        held: 0,
        candidates: exp.candidates.len(),
        cancelled,
        hold_exit_zero: false,
    });
    if g.json {
        let doc = Doc {
            schema: SCHEMA_NAME,
            v: crate::manifest::SCHEMA_VERSION,
            tool: crate::manifest::tool(),
            cutoff: args.detect.cutoff,
            triage: args.detect.triage.name(),
            summary: sum,
            items: &items,
        };
        let text = serde_json::to_string_pretty(&doc).unwrap_or_default();
        let _ = writeln!(std::io::stdout().lock(), "{text}");
    } else if ndjson {
        print_json_line(
            "end",
            &serde_json::json!({"summary": {"items": sum.items, "write": sum.write, "held": sum.held, "failed": sum.failed, "skipped": sum.skipped}, "exit_code": code, "cancelled": cancelled}),
        );
    }
    if !g.quiet {
        rep.line(&format!(
            "analysed {} images: {} would be written, {} held, {} failed, {} skipped",
            sum.items, sum.write, sum.held, sum.failed, sum.skipped
        ));
    }
    code
}

fn print_json_line<T: Serialize>(t: &'static str, body: &T) {
    #[derive(Serialize)]
    struct E<'a, T: Serialize> {
        v: u32,
        t: &'static str,
        #[serde(flatten)]
        body: &'a T,
    }
    let line = serde_json::to_string(&E {
        v: crate::manifest::SCHEMA_VERSION,
        t,
        body,
    })
    .unwrap_or_default();
    let _ = writeln!(std::io::stdout().lock(), "{line}");
}
