// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `auto-crop process`: detect, crop and write (ROADMAP M2.39-M2.45). The batch driver: expand
//! the inputs, run a pool of workers over them, report, write the manifest, choose the exit code.
//!
//! Writing an original goes through the engine's save path only. Everything else (`--output`,
//! `--suffix`, `--copy`) writes new files with [`crate::writer`]. `--dry-run` runs the analysis
//! and the name planning and then stops: no temp file, backup, journal, purge, settings or notice
//! record is written (the manifest file is written if asked for, it is the answer).

use crate::args::{EnhanceArg, Global, OutputMode, PdfArgs, PdfOrder, ProcessArgs};
use crate::env::{Env, base_settings, cpu_floor, test_hook};
use crate::exit;
use crate::inputs::{self, Candidate, Problem};
use crate::manifest::{
    self, ItemRecord, Manifest, Options, OutputRec, PdfRec, Run, RunOpts, Status, Summary, Timing,
    code,
};
use crate::pipeline::{
    self, Decision, Prepared, Ready, Shared, analysed_record, decide, err_code, split_reasons,
};
use crate::report::Reporter;
use crate::writer::{self, PlanFailure, format_name, out_format};
use auto_crop_codecs::{Format, probe};
use auto_crop_core::Enhance;
use auto_crop_core::{EnhanceMode, ErrKind, QuadWarp, ScanTriage};
use auto_crop_engine::enhance::quad_enhances;
use auto_crop_engine::fsplan::ReservedKeys;
use auto_crop_engine::memory::{MIB, MemoryBudget, system_cap};
use auto_crop_engine::run_pdf::{PdfRun, PdfRunOptions, PdfRunSaved, PdfSelect};
use auto_crop_engine::store::{BackupState, Store};
use auto_crop_engine::util::{new_id, now_secs, rfc3339};
use auto_crop_engine::{CancelToken, Engine, Housekeeping, RunOptions, SaveOutcome, SaveTarget};
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Everything a worker needs beyond the shared pipeline state.
struct Work<'a> {
    args: &'a ProcessArgs,
    run_id: String,
    run_name: String,
    reserved: Mutex<ReservedKeys>,
    /// `--pdf`: candidate index -> the engine item that keeps its crops until their page turn.
    kept: Mutex<HashMap<usize, u32>>,
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
pub(crate) fn store_writable(dir: &Path) -> Result<(), String> {
    let probe = dir.join(format!(".write-test-{}", new_id()));
    fs::create_dir_all(dir)
        .and_then(|()| fs::write(&probe, b"x"))
        .and_then(|()| fs::remove_file(&probe))
        .map_err(|e| format!("the backup folder {} cannot be written: {e}", dir.display()))
}

/// The one-time notice. Returns whether it was shown.
pub(crate) fn first_write_notice(env: &Env, retention: Option<u32>, instead: &str) -> bool {
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
         \x20 instead:  {instead} write new files and leave originals alone;\n\
         \x20           --dry-run shows what would happen. This note is shown once.",
        env.paths.backups_dir().display()
    );
    let _ = fs::create_dir_all(&env.paths.config_dir)
        .and_then(|()| fs::write(&marker, format!("shown {}\n", rfc3339(now_secs()))));
    true
}

/// The quads of the included crops with the enhancement each is written with (the engine's
/// effective value: the crop's override, else the image's default).
fn quads_of(engine: &Engine, id: u32) -> (Vec<QuadWarp>, Vec<Enhance>) {
    engine
        .edit_state(id)
        .map(|s| quad_enhances(&s).into_iter().unzip())
        .unwrap_or_default()
}

/// What `--enhance` does to one analysed image, before anything is written. Never forced: this
/// runs only when the run asked for it, a fixed mode sets the image's default enhancement and
/// `auto` applies, per crop, ONLY what `Engine::suggest_enhance` recommends (nothing for a page
/// it would leave alone). The record lists what each written crop got, in output order. A
/// suggestion that cannot be computed leaves the crop as it is.
fn apply_enhance(sh: &Shared, args: &ProcessArgs, r: &Ready, rec: &mut ItemRecord) {
    if args.enhance == EnhanceArg::Original {
        return;
    }
    let strength = f32::from(args.enhance_strength) / 100.0;
    let crops: Vec<u32> = r
        .view
        .crops
        .iter()
        .filter(|c| c.include && c.edit.is_some())
        .map(|c| c.id)
        .collect();
    let label = |m: EnhanceMode| format!("{}:{}", m.name(), args.enhance_strength);
    match args.enhance.mode() {
        Some(mode) => {
            if sh
                .engine
                .set_default_enhance(r.id, mode, strength, false, "", None)
                .is_ok()
            {
                rec.enhance = crops.iter().map(|_| label(mode)).collect();
            }
        }
        None => {
            for c in crops {
                let suggestion = sh.engine.suggest_enhance(r.id, c).ok().flatten();
                rec.enhance.push(match suggestion {
                    Some(s)
                        if sh
                            .engine
                            .set_enhance(r.id, c, s.mode, strength, false, "Enhance (auto)", None)
                            .is_ok() =>
                    {
                        label(s.mode)
                    }
                    _ => "original".to_owned(),
                });
            }
        }
    }
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
pub(crate) fn written_output(path: &Path) -> OutputRec {
    match fs::read(path).ok().and_then(|b| probe(&b).ok()) {
        Some(p) => out_record(path, Some((p.width, p.height)), Some(format_name(p.format))),
        None => out_record(path, None, None),
    }
}

pub(crate) fn planned_output(path: &Path, format: Format) -> OutputRec {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    OutputRec {
        path: abs.to_string_lossy().into_owned(),
        bytes: None,
        width: None,
        height: None,
        format: Some(format_name(format)),
    }
}

/// The manifest's record of the document: what was written, or in a dry run the planned name.
fn pdf_record(args: &ProcessArgs, saved: Option<&PdfRunSaved>) -> PdfRec {
    let pdf = args.pdf.as_ref();
    let file = saved.map_or_else(
        || pdf.map(|p| p.file.clone()).unwrap_or_default(),
        |s| s.path.clone(),
    );
    let abs = std::path::absolute(&file).unwrap_or(file);
    PdfRec {
        path: abs.to_string_lossy().into_owned(),
        pages: saved.map(|s| s.pages as usize),
        bytes: saved.map(|s| s.bytes),
        renamed: saved.is_some_and(|s| {
            s.notices
                .iter()
                .any(|n| n == auto_crop_engine::run_pdf::NOTICE_PDF_RENAMED)
        }),
        order: pdf.map_or("name", |p| p.order.name()),
        dpi: pdf.and_then(|p| p.dpi),
        pass_through_pages: saved.map_or(0, |s| s.pass_through_pages as usize),
        notices: saved.map(|s| s.notices.clone()).unwrap_or_default(),
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
    // `open_only` names the notice that makes the source not writable back as itself (a lossy WebP,
    // a 1-bit TIFF), which is also what picks PNG for its copies.
    let notices: Vec<&str> = r.view.open_only.iter().map(String::as_str).collect();
    let format = out_format(r.format, &notices, args.format);
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
    let (quads, enhances) = quads_of(&sh.engine, r.id);
    let result = (|| -> Result<Vec<writer::Rendered>, ErrKind> {
        let bytes = fs::read(&cand.path).map_err(|e| ErrKind::from_io(&e))?;
        writer::render_all(&bytes, &quads, &enhances, format, &sh.engine.options())
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

/// The document of a `--pdf` run, filled in page order while the workers analyse (the pages of
/// image `k` are added only when every image before it in the order has arrived or is known not to
/// come), so the PDF is written one image at a time in the order asked for.
struct Assembly<'e> {
    engine: &'e Engine,
    file: PathBuf,
    opts: PdfRunOptions,
    cancel: CancelToken,
    run: Option<PdfRun<'e>>,
    /// Why there is no document (`Cancelled` is not a failure of the pages).
    failed: Option<ErrKind>,
    /// Candidate indices in page order.
    order: Vec<usize>,
    next: usize,
    /// Candidate index -> the engine item that holds its crops (`None`: nothing to add).
    arrived: HashMap<usize, Option<u32>>,
    /// Candidates whose crops were offered to the document (all of them, written or not).
    kept: Vec<usize>,
    /// The candidate whose pages could not be added.
    broke: Option<usize>,
}

impl<'e> Assembly<'e> {
    fn new(
        engine: &'e Engine,
        pdf: &PdfArgs,
        opts: PdfRunOptions,
        cancel: CancelToken,
        order: Vec<usize>,
    ) -> Self {
        Self {
            engine,
            file: pdf.file.clone(),
            opts,
            cancel,
            run: None,
            failed: None,
            order,
            next: 0,
            arrived: HashMap::new(),
            kept: Vec::new(),
            broke: None,
        }
    }

    fn arrive(&mut self, i: usize, id: Option<u32>) {
        self.arrived.insert(i, id);
        self.drain(false);
    }

    /// Adds every image whose turn has come. `last`: nothing more will arrive, so the images that
    /// never did (a cancel) are passed over.
    fn drain(&mut self, last: bool) {
        while let Some(&k) = self.order.get(self.next) {
            match self.arrived.remove(&k) {
                Some(id) => {
                    self.consume(k, id);
                    self.next += 1;
                }
                None if last => self.next += 1,
                None => break,
            }
        }
    }

    fn consume(&mut self, k: usize, id: Option<u32>) {
        let Some(id) = id else { return };
        self.kept.push(k);
        if self.failed.is_none() {
            let r = (|| -> Result<(), ErrKind> {
                if self.run.is_none() {
                    self.run =
                        Some(
                            self.engine
                                .begin_run_pdf(&self.file, &self.opts, &self.cancel, 0)?,
                        );
                }
                let run = self.run.as_mut().ok_or(ErrKind::Internal)?;
                run.add(
                    PdfSelect {
                        item: id,
                        crop: None,
                    },
                    &|_| {},
                )?;
                Ok(())
            })();
            if let Err(e) = r {
                self.failed = Some(e);
                self.broke = Some(k);
                self.run = None; // drops the temp file
            }
        }
        self.engine.remove_items(&[id]);
    }

    /// Completes the document. `Ok(None)`: no image had a page.
    fn finish(&mut self) -> Result<Option<PdfRunSaved>, ErrKind> {
        if let Some(e) = self.failed {
            return Err(e);
        }
        match self.run.take() {
            None => Ok(None),
            Some(run) => run.finish(&|_| {}).map(Some),
        }
    }
}

/// File names compare with their digit runs as numbers (`scan2` before `scan10`), ignoring case.
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(p), Some(q)) if p.is_ascii_digit() && q.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars<'_>>| {
                    let mut s = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        s.push(c);
                        it.next();
                    }
                    s.trim_start_matches('0').to_owned()
                };
                let (m, n) = (take(&mut x), take(&mut y));
                let o = m.len().cmp(&n.len()).then_with(|| m.cmp(&n));
                if o != std::cmp::Ordering::Equal {
                    return o;
                }
            }
            (Some(p), Some(q)) => {
                let o = p.to_lowercase().cmp(q.to_lowercase());
                if o != std::cmp::Ordering::Equal {
                    return o;
                }
                x.next();
                y.next();
            }
        }
    }
}

/// The candidates in page order.
fn page_order(candidates: &[crate::inputs::Candidate], order: PdfOrder) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..candidates.len()).collect();
    if order == PdfOrder::Name {
        let name = |i: usize| {
            candidates[i]
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        idx.sort_by(|&a, &b| natural_cmp(&name(a), &name(b)).then(a.cmp(&b)));
    }
    idx
}

fn run_item(sh: &Shared, w: &Work<'_>, idx: usize, cand: &Candidate) -> ItemRecord {
    let t_total = Instant::now();
    let ready = match pipeline::prepare(sh, idx, cand) {
        Prepared::Done(rec) => return *rec,
        Prepared::Ready(r) => r,
    };
    let mut rec = analysed_record(idx, cand, &ready, sh.cutoff, Status::Saved);
    let mut keep = false;
    match decide(&ready.view, sh.cutoff) {
        Decision::Hold { code, reasons } => hold(&mut rec, code, reasons),
        Decision::Write { crops, split } => {
            let t_write = Instant::now();
            // Only an image that would be written is enhanced (a held one is not touched).
            apply_enhance(sh, w.args, &ready, &mut rec);
            if let OutputMode::Pdf(file) = &w.args.mode {
                if w.args.dry_run {
                    rec.outputs = vec![planned_output(file, Format::Pdf)];
                    rec.detail = Some("would be pages of the PDF".to_owned());
                } else {
                    // The crops stay in the engine until this image's turn in the page order.
                    w.kept
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(idx, ready.id);
                    keep = true;
                }
            } else if w.args.mode.replaces_originals() {
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
    if !keep {
        sh.engine.remove_items(&[ready.id]);
    }
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

pub(crate) fn emit<T: Serialize>(t: &'static str, body: &T) {
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
pub(crate) struct StartBody<'a> {
    pub(crate) tool: &'a manifest::Tool,
    pub(crate) run_id: &'a str,
    pub(crate) items: usize,
    pub(crate) dry_run: bool,
}

#[derive(Serialize)]
pub(crate) struct EndBody<'a> {
    pub(crate) summary: &'a Summary,
    pub(crate) exit_code: u8,
    pub(crate) exit_name: &'static str,
    pub(crate) cancelled: bool,
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

pub(crate) fn problem_record(idx: usize, typed: &str, p: Problem) -> ItemRecord {
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
        // The grey modes (and `auto`, which may pick one) add two planes of the output's size.
        enhance_bytes_per_px: match args.enhance {
            EnhanceArg::Original | EnhanceArg::Clean => 0,
            EnhanceArg::Document | EnhanceArg::Receipt | EnhanceArg::Auto => 2,
        },
    };
    let run_name = format!("auto-crop {}", rfc3339(started));
    let work = Work {
        args: &args,
        run_id: run_id.clone(),
        run_name,
        reserved: Mutex::new(ReservedKeys::default()),
        kept: Mutex::new(HashMap::new()),
    };
    let mut assembly: Option<Assembly<'_>> = match (&args.pdf, args.dry_run) {
        (Some(pdf), false) => Some(Assembly::new(
            &sh.engine,
            pdf,
            PdfRunOptions {
                dpi: pdf.dpi,
                one_bit: args.knobs.one_bit,
                quality: args.knobs.quality.unwrap_or_default(),
                ..PdfRunOptions::default()
            },
            env.cancel.clone(),
            page_order(&candidates, pdf.order),
        )),
        _ => None,
    };

    if replacing && !args.dry_run && !candidates.is_empty() {
        first_write_notice(env, retention, "--output DIR, --suffix TEXT or --copy");
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
            if let Some(asm) = assembly.as_mut() {
                let id = work
                    .kept
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&(problem_count + i));
                asm.arrive(i, id);
            }
        },
    );
    // The document: the pages of the images that are left, then the parse-back and the name.
    let mut pdf_rec: Option<PdfRec> = None;
    let mut pdf_written = false;
    if let Some(asm) = assembly.as_mut() {
        asm.drain(true);
        let kept = asm.kept.clone();
        let broke = asm.broke;
        let outcome = asm.finish();
        let slot = |k: usize| problem_count + k;
        match outcome {
            Ok(Some(saved)) => {
                pdf_written = true;
                for k in &kept {
                    if let Some(Some(rec)) = results.get_mut(*k) {
                        rec.outputs = vec![out_record(&saved.path, None, Some("pdf"))];
                        rec.written = true;
                        rec.reasons.extend(saved.notices.iter().cloned());
                        let _ = slot;
                    }
                }
                rep.info(&format!(
                    "wrote {} ({} pages)",
                    saved.path.display(),
                    saved.pages
                ));
                pdf_rec = Some(pdf_record(&args, Some(&saved)));
            }
            Ok(None) => rep.info("nothing was written: no image has a crop for the PDF"),
            Err(e) => {
                let cancelled_run = e == ErrKind::Cancelled;
                for k in &kept {
                    if let Some(Some(rec)) = results.get_mut(*k) {
                        if cancelled_run {
                            rec.status = Status::Skipped;
                            rec.code = Some(code::CANCELLED.to_owned());
                        } else {
                            rec.status = Status::Failed;
                            rec.code = Some(err_code(e));
                            if broke != Some(*k) {
                                rec.detail = Some("the PDF was not written".to_owned());
                            }
                        }
                    }
                }
                rep.info(&format!("no PDF was written: {}", err_code(e)));
            }
        }
    } else if args.pdf.is_some() && args.dry_run {
        pdf_rec = Some(pdf_record(&args, None));
    }
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
    if args.pdf.is_some() {
        // One document, however many images are its pages.
        summary.files_written = usize::from(pdf_written || (args.dry_run && summary.saved > 0));
    }
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
            command: "process",
            id: run_id,
            started: rfc3339(started),
            finished: rfc3339(now_secs()),
            dry_run: args.dry_run,
            cancelled,
            exit_code,
            exit_name: exit::name(exit_code),
            options: RunOpts::Process(Options {
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
                enhance: (args.enhance != EnhanceArg::Original)
                    .then(|| format!("{}:{}", args.enhance.name(), args.enhance_strength)),
                tiff_compression: args.knobs.tiff_compression_name(),
                one_bit: args.knobs.one_bit,
                avif_speed: args.knobs.avif_speed,
                avif_chroma: args.knobs.avif_chroma_name(),
            }),
        },
        summary: summary.clone(),
        items: records,
        warnings,
        pdf: pdf_rec,
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

#[cfg(test)]
mod tests {
    use super::natural_cmp;
    use std::cmp::Ordering::{Equal, Greater, Less};

    #[test]
    fn digit_runs_compare_as_numbers_and_case_does_not_matter() {
        assert_eq!(natural_cmp("scan2.jpg", "scan10.jpg"), Less);
        assert_eq!(natural_cmp("scan10.jpg", "scan2.jpg"), Greater);
        assert_eq!(natural_cmp("IMG_0009.jpg", "img_0010.jpg"), Less);
        assert_eq!(natural_cmp("a1b2", "a1b10"), Less);
        assert_eq!(natural_cmp("a01", "a1"), Less, "ties break on the raw text");
        assert_eq!(natural_cmp("x", "x"), Equal);
        assert_eq!(natural_cmp("a", "a1"), Less);
        // Numbers longer than any integer type are still compared by length then digits.
        assert_eq!(
            natural_cmp("p99999999999999999999999", "p100000000000000000000000"),
            Less
        );
    }
}
