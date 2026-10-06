// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `auto-crop process`: detect, crop and write (ROADMAP M2.39-M2.45). The batch driver: expand
//! the inputs, run a pool of workers over them, report, write the manifest, choose the exit code.
//!
//! Writing an original goes through the engine's save path only. Everything else (`--output`,
//! `--suffix`, `--copy`) writes new files with [`crate::writer`]. `--dry-run` runs the analysis
//! and the name planning and then stops: no temp file, backup, journal, purge, settings or notice
//! record is written (the manifest file is written if asked for, it is the answer).

use crate::args::{Global, OutputMode, ProcessArgs};
use crate::env::{Env, base_settings, cpu_floor, test_hook};
use crate::exit;
use crate::inputs::{self, Candidate, Problem};
use crate::manifest::{
    self, ItemRecord, Manifest, Options, OutputRec, Run, Status, Summary, Timing, code,
};
use crate::pipeline::{
    self, Decision, Prepared, Ready, Shared, analysed_record, decide, err_code, split_reasons,
};
use crate::report::Reporter;
use crate::writer::{self, PlanFailure, format_name, out_format};
use auto_crop_codecs::{Format, probe};
use auto_crop_core::{ErrKind, QuadWarp, ScanTriage};
use auto_crop_engine::fsplan::ReservedKeys;
use auto_crop_engine::memory::{MIB, MemoryBudget, system_cap};
use auto_crop_engine::store::{BackupState, Store};
use auto_crop_engine::util::{new_id, now_secs, rfc3339};
use auto_crop_engine::{Engine, Housekeeping, RunOptions, SaveOutcome, SaveTarget};
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Everything a worker needs beyond the shared pipeline state.
struct Work<'a> {
    args: &'a ProcessArgs,
    run_id: String,
    run_name: String,
    reserved: Mutex<ReservedKeys>,
}

pub fn default_jobs() -> usize {
    let cores = std::thread::available_parallelism().map_or(1, usize::from);
    (cores / 2).clamp(1, 4)
}

/// A memory budget of `mem_limit_mb`, or the machine's own cap.
pub fn budget(mem_limit_mb: Option<u64>) -> MemoryBudget {
    MemoryBudget::new(mem_limit_mb.map_or_else(system_cap, |mb| mb * MIB))
}

/// Hash of every output of an earlier run that has not been restored, with its backup id: the
/// "already processed" guard (PLAN 2.7), read once from the store (never written).
pub fn processed_outputs(store_dir: &Path) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for m in Store::new(store_dir.to_path_buf()).list() {
        if m.state == BackupState::Saved {
            for o in &m.outputs {
                map.insert(o.blake3.clone(), m.id.clone());
            }
        }
    }
    map
}

/// The backup store can take a write: the folder exists (or is made) and a probe file can be
/// created and removed. A run that would replace originals needs this before it touches one.
fn store_writable(dir: &Path) -> Result<(), String> {
    let probe = dir.join(format!(".write-test-{}", new_id()));
    fs::create_dir_all(dir)
        .and_then(|()| fs::write(&probe, b"x"))
        .and_then(|()| fs::remove_file(&probe))
        .map_err(|e| format!("the backup folder {} cannot be written: {e}", dir.display()))
}

/// The one-time notice. Returns whether it was shown.
fn first_write_notice(env: &Env, retention: Option<u32>) -> bool {
    if env.global.quiet || env.global.no_config {
        return false;
    }
    let marker = env.paths.config_dir.join("cli-notice-ack");
    if marker.exists() {
        return false;
    }
    let keep = retention.map_or_else(
        || "kept until you purge them".to_owned(),
        |d| format!("kept {d} days"),
    );
    eprintln!(
        "note: auto-crop replaces your originals, after saving a verified backup of each one.\n\
         \x20 backups:  {} ({keep})\n\
         \x20 undo:     auto-crop restore <file>   or   auto-crop restore --run <run id>\n\
         \x20 instead:  --output DIR, --suffix TEXT or --copy write new files and leave originals alone;\n\
         \x20           --dry-run shows what would happen. This note is shown once.",
        env.paths.backups_dir().display()
    );
    let _ = fs::create_dir_all(&env.paths.config_dir)
        .and_then(|()| fs::write(&marker, format!("shown {}\n", rfc3339(now_secs()))));
    true
}

fn quads_of(engine: &Engine, id: u32) -> Vec<QuadWarp> {
    engine
        .edit_state(id)
        .map(|s| {
            s.included()
                .filter_map(|i| i.geometry.quad().cloned())
                .collect()
        })
        .unwrap_or_default()
}

fn out_record(path: &Path, dims: Option<(u32, u32)>, format: Option<&'static str>) -> OutputRec {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    OutputRec {
        path: abs.to_string_lossy().into_owned(),
        bytes: fs::metadata(path).ok().map(|m| m.len()),
        width: dims.map(|d| d.0),
        height: dims.map(|d| d.1),
        format,
    }
}

/// An output the engine wrote, described from the file itself.
fn written_output(path: &Path) -> OutputRec {
    match fs::read(path).ok().and_then(|b| probe(&b).ok()) {
        Some(p) => out_record(path, Some((p.width, p.height)), Some(format_name(p.format))),
        None => out_record(path, None, None),
    }
}

fn planned_output(path: &Path, format: Format) -> OutputRec {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    OutputRec {
        path: abs.to_string_lossy().into_owned(),
        bytes: None,
        width: None,
        height: None,
        format: Some(format_name(format)),
    }
}

fn hold(rec: &mut ItemRecord, c: &'static str, reasons: Vec<String>) {
    rec.status = Status::Held;
    rec.code = Some(c.to_owned());
    rec.reasons = reasons;
}

/// In-place save of one image through the engine.
fn save_in_place(
    sh: &Shared,
    w: &Work<'_>,
    cand: &Candidate,
    r: &Ready,
    split: bool,
    rec: &mut ItemRecord,
) {
    let dry = w.args.dry_run;
    let parent = cand
        .path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    if dry {
        // The plan of the real run: a split scan is held unless it is accepted (the engine's rule),
        // otherwise the original's path keeps the result, or the set `<name>_01...` replaces it.
        if split {
            let approved = r
                .view
                .split
                .as_ref()
                .is_some_and(|s| s.triage == ScanTriage::Approved);
            if !(w.args.accept_splits && approved) {
                hold(rec, code::SPLIT_HELD, split_reasons(&r.view, sh.cutoff));
                return;
            }
            let ext = r.format.extension();
            let count = rec.crops.unwrap_or(2);
            match writer::plan_copies(
                &OutputMode::InPlace { explicit: false },
                None,
                crate::args::IfExists::KeepBoth,
                cand,
                count,
                ext,
                &w.reserved,
            ) {
                Ok(p) => {
                    rec.outputs = p
                        .paths
                        .iter()
                        .map(|p| planned_output(p, r.format))
                        .collect()
                }
                Err(_) => {
                    rec.status = Status::Failed;
                    rec.code = Some(err_code(ErrKind::PlanStale));
                    return;
                }
            }
        } else {
            rec.outputs = vec![planned_output(&cand.path, r.format)];
        }
        rec.detail = Some("would back up the original (copy), then replace it".to_owned());
        return;
    }
    let outcome: SaveOutcome =
        sh.engine
            .save_in_run(r.id, SaveTarget::Replace, &w.run_id, &w.run_name);
    if outcome.ok {
        if let Some(saved) = outcome.saved {
            let names = if saved.outputs.is_empty() {
                vec![saved.output.clone()]
            } else {
                saved.outputs.clone()
            };
            rec.outputs = names
                .iter()
                .map(|n| written_output(&parent.join(n)))
                .collect();
            rec.backup_id = saved.backup_id;
            rec.written = true;
        }
        rec.reasons.extend(outcome.notes.into_iter().map(err_code));
        rec.reasons.extend(outcome.notices);
        return;
    }
    let kind = outcome.error.unwrap_or(ErrKind::Internal);
    match kind {
        ErrKind::HeldForReview => hold(rec, code::SPLIT_HELD, split_reasons(&r.view, sh.cutoff)),
        ErrKind::NotReplaceable => {
            rec.status = Status::Skipped;
            rec.code = Some(code::NOT_REPLACEABLE.to_owned());
            rec.reasons = outcome.notices;
        }
        other => {
            rec.status = Status::Failed;
            rec.code = Some(err_code(other));
            rec.reasons = outcome.notices;
        }
    }
}

/// `--output`, `--suffix` and `--copy`: new files, nothing that exists is touched.
fn save_copies(
    sh: &Shared,
    w: &Work<'_>,
    cand: &Candidate,
    r: &Ready,
    crops: usize,
    rec: &mut ItemRecord,
) {
    let args = w.args;
    let format = out_format(r.format, args.format);
    let plan = match writer::plan_copies(
        &args.mode,
        args.name_template.as_deref(),
        args.if_exists,
        cand,
        crops,
        format.extension(),
        &w.reserved,
    ) {
        Ok(p) => p,
        Err(PlanFailure::Exists) => {
            rec.status = Status::Skipped;
            rec.code = Some(code::EXISTS.to_owned());
            return;
        }
        Err(PlanFailure::Error(k)) => {
            rec.status = Status::Failed;
            rec.code = Some(err_code(k));
            return;
        }
    };
    if args.dry_run {
        rec.outputs = plan
            .paths
            .iter()
            .map(|p| planned_output(p, format))
            .collect();
        return;
    }
    let quads = quads_of(&sh.engine, r.id);
    let result = (|| -> Result<Vec<writer::Rendered>, ErrKind> {
        let bytes = fs::read(&cand.path).map_err(|e| ErrKind::from_io(&e))?;
        writer::render_all(&bytes, &quads, format, &sh.engine.options())
    })();
    let files = match result {
        Ok(f) if f.len() == plan.paths.len() => f,
        Ok(_) => {
            rec.status = Status::Failed;
            rec.code = Some(err_code(ErrKind::Internal));
            return;
        }
        Err(k) => {
            rec.status = Status::Failed;
            rec.code = Some(err_code(k));
            return;
        }
    };
    if let Err(k) = writer::commit(&plan.paths, &files, r.mtime) {
        rec.status = Status::Failed;
        rec.code = Some(err_code(k));
        return;
    }
    rec.outputs = plan
        .paths
        .iter()
        .zip(&files)
        .map(|(p, f)| out_record(p, Some((f.width, f.height)), Some(format_name(f.format))))
        .collect();
    rec.written = true;
}

fn run_item(sh: &Shared, w: &Work<'_>, idx: usize, cand: &Candidate) -> ItemRecord {
    let t_total = Instant::now();
    let ready = match pipeline::prepare(sh, idx, cand) {
        Prepared::Done(rec) => return *rec,
        Prepared::Ready(r) => r,
    };
    let mut rec = analysed_record(idx, cand, &ready, sh.cutoff, Status::Saved);
    match decide(&ready.view, sh.cutoff) {
        Decision::Hold { code, reasons } => hold(&mut rec, code, reasons),
        Decision::Write { crops, split } => {
            let t_write = Instant::now();
            if w.args.mode.replaces_originals() {
                save_in_place(sh, w, cand, &ready, split, &mut rec);
            } else {
                save_copies(sh, w, cand, &ready, crops, &mut rec);
            }
            rec.ms = Some(Timing {
                write: pipeline::round_ms(t_write),
                ..Timing::default()
            });
        }
    }
    sh.engine.remove_items(&[ready.id]);
    let t = rec.ms.get_or_insert_with(Timing::default);
    t.read = ready.read_ms;
    t.analyse = ready.analyse_ms;
    t.total = pipeline::round_ms(t_total);
    rec
}

#[derive(Serialize)]
struct Event<'a, T: Serialize> {
    v: u32,
    t: &'static str,
    #[serde(flatten)]
    body: &'a T,
}

fn emit<T: Serialize>(t: &'static str, body: &T) {
    let line = serde_json::to_string(&Event {
        v: manifest::SCHEMA_VERSION,
        t,
        body,
    })
    .unwrap_or_default();
    let mut out = std::io::stdout().lock();
    // A closed pipe must not abort a batch half way: the work still finishes.
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

#[derive(Serialize)]
struct StartBody<'a> {
    tool: &'a manifest::Tool,
    run_id: &'a str,
    items: usize,
    dry_run: bool,
}

#[derive(Serialize)]
struct EndBody<'a> {
    summary: &'a Summary,
    exit_code: u8,
    exit_name: &'static str,
    cancelled: bool,
}

/// Writes `text` to `path` through a temp file in the same folder.
pub fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".autocrop-{}.tmp", new_id()));
    let go = || -> std::io::Result<()> {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        fs::rename(&tmp, path)
    };
    let r = go();
    if r.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    r
}

fn problem_record(idx: usize, typed: &str, p: Problem) -> ItemRecord {
    let status = match p {
        Problem::NotFound | Problem::NoMatch => Status::Failed,
        Problem::Unsupported | Problem::Link => Status::Skipped,
    };
    ItemRecord::new(idx, typed, status).with_code(p.code())
}

pub fn run(args: ProcessArgs, env: &Env) -> u8 {
    let g: &Global = &env.global;
    let t0 = Instant::now();
    let started = now_secs();
    let run_id = new_id();
    let replacing = args.mode.replaces_originals();

    // Preconditions: nothing is touched until they hold.
    if let Err(m) = cpu_floor() {
        eprintln!("error: {m}");
        return exit::PRECONDITION;
    }
    let store_dir = env.paths.backups_dir();
    if replacing
        && !args.dry_run
        && let Err(m) = store_writable(&store_dir)
    {
        eprintln!("error: {m}; nothing was changed");
        return exit::PRECONDITION;
    }
    if let Some(m) = &args.manifest {
        let dir = m
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        if let Err(e) = fs::create_dir_all(dir) {
            eprintln!("error: cannot write the manifest {}: {e}", m.display());
            return exit::PRECONDITION;
        }
        if m.is_dir() {
            eprintln!("error: the manifest path {} is a folder", m.display());
            return exit::PRECONDITION;
        }
    }

    let exp = inputs::expand(&args.input, Some(&store_dir));
    let mut records: Vec<ItemRecord> = Vec::new();
    for (typed, p) in &exp.problems {
        records.push(problem_record(records.len(), typed, *p));
    }
    let problem_count = records.len();
    let candidates = exp.candidates.clone();
    let total = records.len() + candidates.len();

    let mut settings = base_settings(&env.paths, g);
    settings.split_policy = args.detect.split;
    settings.split_profile = args.detect.profile;
    settings.auto_save_splits = args.accept_splits;
    let retention = settings.retention_days;
    let engine = Engine::with_settings(
        env.paths.clone(),
        settings,
        if args.dry_run {
            Housekeeping::None
        } else {
            Housekeeping::RecoverAndPurge
        },
    );
    engine.set_run_options(RunOptions {
        margin_pct: args.detect.margin,
    });
    engine.set_options(args.knobs.engine_options());
    let jobs = args.jobs.unwrap_or_else(default_jobs).max(1);
    let sh = Shared {
        engine,
        budget: budget(args.mem_limit_mb),
        cancel: env.cancel.clone(),
        cutoff: args.detect.cutoff,
        processed: if args.reprocess {
            HashMap::new()
        } else {
            processed_outputs(&store_dir)
        },
        reprocess: args.reprocess,
        replacing,
        delay: Duration::from_millis(test_hook("AUTO_CROP_TEST_DELAY_MS").unwrap_or(0)),
    };
    let run_name = format!("auto-crop {}", rfc3339(started));
    let work = Work {
        args: &args,
        run_id: run_id.clone(),
        run_name,
        reserved: Mutex::new(ReservedKeys::default()),
    };

    if replacing && !args.dry_run && !candidates.is_empty() {
        first_write_notice(env, retention);
    }
    let mut rep = Reporter::new(g, args.progress, total, args.dry_run);
    let mut warnings: Vec<String> = Vec::new();
    if exp.truncated {
        warnings.push(format!(
            "the walk stopped at --max-files ({}); raise it to take more",
            args.input.max_files
        ));
    }
    if exp.depth_limited > 0 {
        warnings.push(format!(
            "{} folders were not entered because of --max-depth {}",
            exp.depth_limited, args.input.max_depth
        ));
    }
    if exp.candidates.len() > 50_000 {
        warnings.push(format!(
            "{} files: this will take a while",
            exp.candidates.len()
        ));
    }
    for m in &warnings {
        rep.info(&format!("warning: {m}"));
    }
    if args.detect.triage == crate::args::Triage::Balanced {
        rep.info("note: --triage balanced is experimental until the accuracy gate is measured; strict is the default");
    }
    if g.ndjson {
        emit(
            "start",
            &StartBody {
                tool: &manifest::tool(),
                run_id: &run_id,
                items: total,
                dry_run: args.dry_run,
            },
        );
    }
    // The problem items are results already.
    for r in &records {
        if g.ndjson {
            emit("item", r);
        }
        rep.item(r);
    }

    // The pool.
    let cancel_after = test_hook("AUTO_CROP_TEST_CANCEL_AFTER");
    let mut results: Vec<Option<ItemRecord>> = vec![None; candidates.len()];
    let mut finished = 0u64;
    crate::pool::run(
        candidates.len(),
        jobs,
        &sh.cancel,
        |i| {
            let cand = &candidates[i];
            let idx = problem_count + i;
            auto_crop_engine::run_isolated(std::panic::AssertUnwindSafe(|| {
                run_item(&sh, &work, idx, cand)
            }))
            .unwrap_or_else(|_| {
                ItemRecord::new(idx, &cand.display, Status::Failed)
                    .with_code(err_code(ErrKind::InternalPanic))
            })
        },
        |i, rec| {
            finished += 1;
            if cancel_after.is_some_and(|n| finished >= n) {
                env.cancel.cancel();
            }
            if g.ndjson {
                emit("item", &rec);
            }
            rep.item(&rec);
            results[i] = Some(rec);
        },
    );
    let cancelled = env.cancel.is_cancelled();
    // Items no worker reached (a cancel) are skipped, with a record each.
    for (i, slot) in results.iter_mut().enumerate() {
        if slot.is_none() {
            let rec = ItemRecord::new(problem_count + i, &candidates[i].display, Status::Skipped)
                .with_code(code::CANCELLED);
            if g.ndjson {
                emit("item", &rec);
            }
            *slot = Some(rec);
        }
    }
    records.extend(results.into_iter().flatten());
    records.sort_by_key(|r| r.index);

    let mut summary = Summary::tally(&records);
    summary.ignored_non_image = exp.ignored_non_image;
    summary.links_skipped = exp.links_skipped;
    summary.hidden_skipped = exp.hidden_skipped;
    summary.filtered_out = exp.filtered_out;
    summary.truncated = exp.truncated;
    let exit_code = exit::batch_exit(exit::Outcome {
        failed: summary.failed,
        held: summary.held,
        candidates: candidates.len(),
        cancelled,
        hold_exit_zero: args.hold_exit_zero,
    });
    let manifest = Manifest {
        schema: manifest::SCHEMA_NAME,
        v: manifest::SCHEMA_VERSION,
        tool: manifest::tool(),
        run: Run {
            id: run_id,
            started: rfc3339(started),
            finished: rfc3339(now_secs()),
            dry_run: args.dry_run,
            cancelled,
            exit_code,
            exit_name: exit::name(exit_code),
            options: Options {
                mode: args.mode.name(),
                triage: args.detect.triage.name(),
                cutoff: args.detect.cutoff,
                split: match args.detect.split {
                    auto_crop_core::SplitPolicy::Auto => "auto",
                    auto_crop_core::SplitPolicy::Always => "always",
                    auto_crop_core::SplitPolicy::Never => "never",
                },
                profile: match args.detect.profile {
                    auto_crop_core::SplitProfile::Photos => "photos",
                    auto_crop_core::SplitProfile::Receipts => "receipts",
                },
                margin_percent: args.detect.margin,
                format: args.format.name(),
                quality: args.knobs.quality_name(),
                strip_location: args.knobs.strip_location,
                max_pixels: args.knobs.max_pixels,
                accept_splits: args.accept_splits,
                reprocess: args.reprocess,
                recursive: args.input.recursive,
                jobs,
            },
        },
        summary: summary.clone(),
        items: records,
        warnings,
    };
    rep.summary(&summary, t0.elapsed().as_secs_f64(), cancelled);
    if summary.held > 0 && !g.quiet {
        rep.line("held items were not written; `auto-crop analyze <file>` says why, and the app lets you fix them");
    }
    let mut final_code = exit_code;
    if let Some(path) = &args.manifest {
        let text = serde_json::to_string_pretty(&manifest).unwrap_or_default();
        if let Err(e) = write_atomic(path, &(text + "\n")) {
            eprintln!("error: cannot write the manifest {}: {e}", path.display());
            if final_code == exit::OK {
                final_code = exit::INTERNAL;
            }
        }
    }
    if g.json {
        let text = serde_json::to_string_pretty(&manifest).unwrap_or_default();
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{text}");
    }
    if g.ndjson {
        emit(
            "end",
            &EndBody {
                summary: &manifest.summary,
                exit_code: final_code,
                exit_name: exit::name(final_code),
                cancelled,
            },
        );
    }
    final_code
}
