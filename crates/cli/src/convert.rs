// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `auto-crop convert`: change the format of images (ROADMAP M6.56; PLAN 2.12, 3.5): HEIC to JPG,
//! TIFF, WebP and BMP to JPG or PNG, no crop and no detection. The batch driver is `process`'s:
//! expand the inputs, run a pool of workers, report, write the manifest, choose the exit code. Each
//! file goes through the engine's `convert_file`, which owns every write: a verified backup of the
//! original, a no-clobber commit, the journal and the restore (`auto-crop restore`).
//!
//! * default: each original is replaced by the converted file after a verified backup;
//! * `--copy`: the converted file is written beside the original, which is kept;
//! * `--output DIR`: the converted files go to DIR (the folder tree is mirrored), originals kept;
//! * `--dry-run`: headers are read and the plan is printed; nothing is decoded or written.
//!
//! A file already in the target format is skipped (`SAME_FORMAT`) unless `--reencode`; a source with
//! several frames is never replaced (`NOT_REPLACEABLE`); a HEIC in a build without the decoder is
//! `HEIF_UNAVAILABLE`. The exit codes and the manifest are `process`'s (docs/cli.md).

use crate::args::{ConvertArgs, ConvertDest, Global};
use crate::env::{Env, base_settings, cpu_floor, test_hook};
use crate::exit;
use crate::inputs::{self, Candidate};
use crate::manifest::{
    self, ConvertOpts, ConvertRec, ItemRecord, Manifest, Run, RunOpts, Status, Summary, Timing,
    code,
};
use crate::pipeline::err_code;
use crate::process::{
    EndBody, StartBody, budget, default_jobs, emit, first_write_notice, planned_output,
    problem_record, store_writable, write_atomic,
};
use crate::report::Reporter;
use crate::writer::format_name;
use auto_crop_engine::colour::ColourMode;
use auto_crop_engine::convert::{
    ConvertError, ConvertOptions, ConvertRequest, ConvertTarget, Converted, Placement, Skip,
};
use auto_crop_engine::enumerate::is_convert_candidate;
use auto_crop_engine::memory::MemoryBudget;
use auto_crop_engine::util::{new_id, now_secs, rfc3339};
use auto_crop_engine::{Engine, ErrKind, Housekeeping};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

struct Work<'a> {
    args: &'a ConvertArgs,
    engine: &'a Engine,
    options: ConvertOptions,
    run_id: String,
    run_name: String,
    budget: MemoryBudget,
    cancel: &'a auto_crop_core::CancelToken,
    delay: Duration,
}

fn absolute(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

/// The folder a file goes to under `--output DIR`: DIR plus the file's folder below its root.
fn mirrored(out: &Path, cand: &Candidate) -> PathBuf {
    let parent = cand
        .path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let rel = parent
        .strip_prefix(&cand.root)
        .map(Path::to_path_buf)
        .unwrap_or_default();
    absolute(&out.join(rel))
}

fn to_name(t: ConvertTarget) -> &'static str {
    t.name()
}

fn colour_name(c: ColourMode) -> &'static str {
    auto_crop_engine::convert_batch::colour_name(c)
}

/// The registry code of a skip (the engine's strings are these, checked by a test).
fn skip_code(s: &Skip) -> &'static str {
    match s {
        Skip::SameFormat => code::SAME_FORMAT,
        Skip::HeifUnavailable => code::HEIF_UNAVAILABLE,
        Skip::NotReplaceable(_) => code::NOT_REPLACEABLE,
        Skip::Unsupported => code::UNSUPPORTED_FORMAT,
    }
}

fn written_record(c: &Converted) -> manifest::OutputRec {
    let abs = absolute(&c.output);
    manifest::OutputRec {
        path: abs.to_string_lossy().into_owned(),
        bytes: (!c.dry_run).then_some(c.bytes_out),
        width: Some(c.dims.0),
        height: Some(c.dims.1),
        format: Some(format_name(c.to)),
    }
}

fn record_of(
    idx: usize,
    cand: &Candidate,
    r: Result<Converted, ConvertError>,
    dry_run: bool,
    replacing: bool,
) -> ItemRecord {
    match r {
        Ok(c) => {
            let mut rec = ItemRecord::new(idx, &cand.display, Status::Saved);
            if c.dry_run {
                let mut o = planned_output(&c.output, c.to);
                o.width = Some(c.dims.0);
                o.height = Some(c.dims.1);
                rec.outputs = vec![o];
                if replacing {
                    rec.detail = Some("would back up the original (copy), then replace it".into());
                }
            } else {
                rec.outputs = vec![written_record(&c)];
                rec.written = true;
                rec.backup_id = c.backup_id.clone();
            }
            rec.crops = Some(1);
            rec.reasons = c.notices.clone();
            rec.reasons.extend(c.notes.iter().map(|k| err_code(*k)));
            rec.convert = Some(ConvertRec {
                from: format_name(c.from),
                to: format_name(c.to),
                bytes_in: c.bytes_in,
                bytes_out: if dry_run { 0 } else { c.bytes_out },
                icc: c.icc.name(),
            });
            rec
        }
        Err(ConvertError::Skipped(s, notices)) => {
            let mut rec =
                ItemRecord::new(idx, &cand.display, Status::Skipped).with_code(skip_code(&s));
            rec.reasons = notices;
            if let Some(n) = s.notice()
                && !rec.reasons.iter().any(|x| x == n)
            {
                rec.reasons.push(n.to_owned());
            }
            rec
        }
        Err(ConvertError::Failed(k, notices)) => {
            let status = if matches!(
                k,
                ErrKind::UnsupportedFormat
                    | ErrKind::UnsupportedFeature
                    | ErrKind::HevcDecoderMissing
            ) {
                Status::Skipped
            } else {
                Status::Failed
            };
            let mut rec = ItemRecord::new(idx, &cand.display, status).with_code(err_code(k));
            rec.reasons = notices;
            rec
        }
    }
}

fn run_item(w: &Work<'_>, idx: usize, cand: &Candidate) -> ItemRecord {
    let t0 = Instant::now();
    if !w.delay.is_zero() {
        std::thread::sleep(w.delay);
    }
    if w.cancel.is_cancelled() {
        return ItemRecord::new(idx, &cand.display, Status::Skipped).with_code(code::CANCELLED);
    }
    let meta = match fs::symlink_metadata(&cand.path) {
        Ok(m) => m,
        Err(e) => {
            return ItemRecord::new(idx, &cand.display, Status::Failed)
                .with_code(err_code(ErrKind::from_io(&e)));
        }
    };
    if inputs::is_link(&meta) {
        return ItemRecord::new(idx, &cand.display, Status::Skipped).with_code(code::LINK);
    }
    let folder = match &w.args.dest {
        ConvertDest::Dir(out) => Some(mirrored(out, cand)),
        _ => None,
    };
    let mut req = ConvertRequest::new(&cand.path, w.options, &w.run_id, &w.run_name);
    req.folder = folder.as_deref();
    req.dry_run = w.args.dry_run;
    req.budget = Some(&w.budget);
    req.cancel = Some(w.cancel);
    let r = w.engine.convert_file(&req);
    let mut rec = record_of(
        idx,
        cand,
        r,
        w.args.dry_run,
        w.options.placement == Placement::Replace,
    );
    let total = (t0.elapsed().as_secs_f64() * 1000.0 * 10.0).round() / 10.0;
    rec.ms = Some(Timing {
        write: total,
        total,
        ..Timing::default()
    });
    rec
}

pub fn run(args: ConvertArgs, env: &Env) -> u8 {
    let g: &Global = &env.global;
    let t0 = Instant::now();
    let started = now_secs();
    let run_id = new_id();
    let replacing = matches!(args.dest, ConvertDest::InPlace { .. });

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
    if let ConvertDest::Dir(d) = &args.dest
        && d.is_file()
    {
        eprintln!("error: --output {} is a file, not a folder", d.display());
        return exit::USAGE;
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

    let exp = inputs::expand_with(&args.input, Some(&store_dir), is_convert_candidate);
    let mut records: Vec<ItemRecord> = Vec::new();
    for (typed, p) in &exp.problems {
        records.push(problem_record(records.len(), typed, *p));
    }
    let problem_count = records.len();
    let candidates = exp.candidates.clone();
    let total = records.len() + candidates.len();

    let settings = base_settings(&env.paths, g);
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
    engine.set_options(args.knobs.engine_options());
    let jobs = args.jobs.unwrap_or_else(default_jobs).max(1);
    let options = ConvertOptions {
        target: args.to,
        colour: args.colour,
        quality: args.knobs.quality.unwrap_or_default(),
        strip_location: args.knobs.strip_location,
        placement: match args.dest {
            ConvertDest::InPlace { .. } => Placement::Replace,
            ConvertDest::Copy => Placement::Keep,
            ConvertDest::Dir(_) => Placement::Folder,
        },
        reencode: args.reencode,
    };
    let work = Work {
        args: &args,
        engine: &engine,
        options,
        run_id: run_id.clone(),
        run_name: format!("auto-crop convert {}", rfc3339(started)),
        budget: budget(args.mem_limit_mb),
        cancel: &env.cancel,
        delay: Duration::from_millis(test_hook("AUTO_CROP_TEST_DELAY_MS").unwrap_or(0)),
    };

    if replacing && !args.dry_run && !candidates.is_empty() {
        first_write_notice(env, retention, "--output DIR or --copy");
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
    if !auto_crop_codecs::Format::Heic.is_decodable() {
        let heif = candidates.iter().any(|c| {
            c.path
                .extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase())
                .is_some_and(|e| matches!(e.as_str(), "heic" | "heif" | "avif"))
        });
        if heif {
            warnings.push(
                "HEIC, HEIF and AVIF files cannot be converted by this build (HEIF_UNAVAILABLE); \
                 the HEIC package has the decoder"
                    .to_owned(),
            );
        }
    }
    for m in &warnings {
        rep.info(&format!("warning: {m}"));
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
    for r in &records {
        if g.ndjson {
            emit("item", r);
        }
        rep.item(r);
    }

    let cancel_after = test_hook("AUTO_CROP_TEST_CANCEL_AFTER");
    let mut results: Vec<Option<ItemRecord>> = vec![None; candidates.len()];
    let mut finished = 0u64;
    crate::pool::run(
        candidates.len(),
        jobs,
        &env.cancel,
        |i| {
            let cand = &candidates[i];
            let idx = problem_count + i;
            auto_crop_engine::run_isolated(std::panic::AssertUnwindSafe(|| {
                run_item(&work, idx, cand)
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
        held: 0,
        candidates: candidates.len(),
        cancelled,
        hold_exit_zero: false,
    });
    let manifest = Manifest {
        schema: manifest::SCHEMA_NAME,
        v: manifest::SCHEMA_VERSION,
        tool: manifest::tool(),
        run: Run {
            command: "convert",
            id: run_id,
            started: rfc3339(started),
            finished: rfc3339(now_secs()),
            dry_run: args.dry_run,
            cancelled,
            exit_code,
            exit_name: exit::name(exit_code),
            options: RunOpts::Convert(ConvertOpts {
                mode: args.dest.name(),
                to: to_name(args.to),
                colour: colour_name(args.colour),
                quality: args.knobs.quality_name(),
                strip_location: args.knobs.strip_location,
                reencode: args.reencode,
                max_pixels: args.knobs.max_pixels,
                recursive: args.input.recursive,
                jobs,
                tiff_compression: args.knobs.tiff_compression_name(),
                one_bit: args.knobs.one_bit,
                avif_speed: args.knobs.avif_speed,
                avif_chroma: args.knobs.avif_chroma_name(),
            }),
        },
        summary: summary.clone(),
        items: records,
        warnings,
    };
    rep.summary(&summary, t0.elapsed().as_secs_f64(), cancelled);
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
