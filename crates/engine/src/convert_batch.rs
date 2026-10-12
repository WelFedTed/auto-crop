// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The Convert flow as a front end sees it (ROADMAP M6.49; PLAN 6): files and folders are added to a
//! list (opaque ids and display names only, the webview never holds a path), a preview says what
//! would happen to each format, and a run converts the list on a small pool with progress, a
//! cancel, and a per-file result. Every file goes through [`Engine::convert_file`], so the safety
//! guarantees (verified backup, no-clobber commit, journal, restore) are the CLI's.
//!
//! The list is the engine's own state (one per engine), separate from the images opened for
//! cropping: converting needs no analysis, no thumbnails and no detection.

use crate::colour::ColourMode;
use crate::convert::{ConvertError, ConvertOptions, ConvertRequest, Converted, Placement, Skip};
use crate::engine::{Engine, lock};
use crate::enumerate;
use crate::error::ErrKind;
use crate::memory::MemoryBudget;
use crate::util::{display_name, new_id};
use auto_crop_codecs::{Format, sniff};
use auto_crop_core::CancelToken;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Most files one list holds (the walk's own cap).
pub const MAX_LIST: usize = enumerate::MAX_FILES;
/// Files named in the preview's "cannot be converted" list (the counts are always complete).
const MAX_CANNOT_LISTED: usize = 50;

#[derive(Debug, Clone)]
struct Entry {
    id: u32,
    path: PathBuf,
    name: String,
    format: Option<Format>,
    bytes: u64,
    /// Where the converted file went (after a successful run).
    output: Option<PathBuf>,
    /// Converted by a run: not converted again.
    done: bool,
}

/// The Convert list and the state of a run. Held by the engine.
#[derive(Default)]
pub(crate) struct Session {
    next_id: u32,
    files: Vec<Entry>,
    folder: Option<PathBuf>,
    cancel: Option<CancelToken>,
    running: bool,
}

/// The stable lower-case name of a format, as the UI and the manifest write it.
pub fn format_name(f: Option<Format>) -> &'static str {
    match f {
        Some(Format::Jpeg) => "jpeg",
        Some(Format::Png) => "png",
        Some(Format::Tiff) => "tiff",
        Some(Format::Webp) => "webp",
        Some(Format::Heic) => "heic",
        Some(Format::Avif) => "avif",
        Some(Format::Gif) => "gif",
        Some(Format::Bmp) => "bmp",
        Some(Format::Jxl) => "jxl",
        None => "unknown",
    }
}

/// One file of the list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConvertFileView {
    pub id: u32,
    pub name: String,
    pub format: String,
    pub bytes: u64,
}

/// What adding paths to the list found.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConvertAdded {
    pub added: usize,
    /// Already in the list.
    pub duplicates: usize,
    /// Links and junctions that were not followed.
    pub links_skipped: usize,
    /// The walk stopped at the file cap.
    pub truncated: bool,
    pub ids: Vec<u32>,
}

/// How many files of one detected format are in the list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormatCount {
    pub format: String,
    pub count: usize,
}

/// A file the run will not convert, and the code that says why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CannotConvert {
    pub id: u32,
    pub name: String,
    pub format: String,
    /// `HEIF_UNAVAILABLE` or `UNSUPPORTED_FORMAT`.
    pub code: String,
}

/// What a run with these options would do, from the files' magic bytes (nothing is decoded).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConvertPreview {
    pub total: usize,
    /// Files that will be converted.
    pub will_convert: usize,
    /// Already in the target format: left alone unless re-encoding is on.
    pub same_format: usize,
    /// Files that cannot be converted in this build.
    pub cannot: usize,
    pub by_format: Vec<FormatCount>,
    pub cannot_files: Vec<CannotConvert>,
    /// HEIC and AVIF can be decoded in this build.
    pub heif_available: bool,
    /// Files that are HEIC, HEIF or AVIF and cannot be converted because the build has no decoder.
    pub heif_unavailable: usize,
    /// Total size of the files that will be converted.
    pub bytes_to_convert: u64,
    /// The placement, echoed.
    pub placement: Placement,
    /// For [`Placement::Folder`]: the folder's name (never its path); `None` until one is chosen.
    pub folder: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConvertStatus {
    Converted,
    Skipped,
    Failed,
}

/// The result for one file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConvertFileResult {
    pub id: u32,
    pub name: String,
    pub status: ConvertStatus,
    /// For a skip or a failure: one registry code (`SAME_FORMAT`, `NOT_REPLACEABLE`, `CORRUPT`...).
    pub code: Option<String>,
    /// Notice codes: `alpha.flattened`, `colour.converted_to_srgb`, `heic.multi_image`...
    pub notices: Vec<String>,
    /// The file name the converted image was written as.
    pub output: Option<String>,
    pub from: String,
    pub to: Option<String>,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// The backup that holds the original (replace mode): `Restore` and the Backups view use it.
    pub backup_id: Option<String>,
}

/// One step of a run, for the progress bar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConvertProgress {
    pub done: usize,
    pub total: usize,
    pub result: ConvertFileResult,
}

/// The end of a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConvertSummary {
    /// Also the id of the backup run in the Backups view.
    pub run_id: String,
    pub converted: usize,
    pub skipped: usize,
    pub failed: usize,
    pub cancelled: bool,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub results: Vec<ConvertFileResult>,
}

/// A file's result and, if it was converted, where its output went.
type FileDone = (ConvertFileResult, Option<PathBuf>);

fn err_code(e: ErrKind) -> String {
    serde_json::to_value(e)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "INTERNAL".to_owned())
}

/// The first bytes of a file, enough for the format sniffer.
fn head(path: &Path) -> Option<Vec<u8>> {
    let mut buf = vec![0u8; 512];
    let n = std::fs::File::open(path).ok()?.read(&mut buf).ok()?;
    buf.truncate(n);
    Some(buf)
}

fn result_of(entry: &Entry, r: Result<Converted, ConvertError>) -> FileDone {
    let from = format_name(entry.format).to_owned();
    let base = |status, code: Option<String>, notices: Vec<String>| ConvertFileResult {
        id: entry.id,
        name: entry.name.clone(),
        status,
        code,
        notices,
        output: None,
        from: from.clone(),
        to: None,
        bytes_in: entry.bytes,
        bytes_out: 0,
        width: None,
        height: None,
        backup_id: None,
    };
    match r {
        Ok(c) => {
            let mut res = base(ConvertStatus::Converted, None, c.notices.clone());
            res.output = c
                .output
                .file_name()
                .map(|n| n.to_string_lossy().into_owned());
            res.to = Some(format_name(Some(c.to)).to_owned());
            res.bytes_in = c.bytes_in;
            res.bytes_out = c.bytes_out;
            res.width = Some(c.dims.0);
            res.height = Some(c.dims.1);
            res.backup_id = c.backup_id.clone();
            for n in &c.notes {
                res.notices.push(err_code(*n));
            }
            (res, Some(c.output))
        }
        Err(ConvertError::Skipped(s, mut notices)) => {
            if let Some(n) = s.notice()
                && !notices.iter().any(|x| x == n)
            {
                notices.push(n.to_owned());
            }
            (
                base(ConvertStatus::Skipped, Some(s.code().to_owned()), notices),
                None,
            )
        }
        Err(ConvertError::Failed(k, notices)) => (
            base(ConvertStatus::Failed, Some(err_code(k)), notices),
            None,
        ),
    }
}

impl Engine {
    /// Adds files and folders to the Convert list. HEIC, HEIF and AVIF files are listed in every
    /// build (a build without the decoder says so in the preview instead of ignoring them).
    pub fn convert_add(&self, roots: &[PathBuf], include_subfolders: bool) -> ConvertAdded {
        let found =
            enumerate::collect_with(roots, include_subfolders, enumerate::is_convert_candidate);
        // Orphan temp files of an interrupted conversion are swept from the folders being added,
        // like for the crop flow (PLAN 2.7).
        let mut dirs: std::collections::BTreeSet<PathBuf> = found
            .files
            .iter()
            .filter_map(|f| f.parent().map(Path::to_path_buf))
            .collect();
        dirs.extend(roots.iter().filter(|r| r.is_dir()).cloned());
        for d in &dirs {
            self.sweep_temps(d);
        }
        let mut s = lock(&self.inner.convert);
        let mut out = ConvertAdded {
            links_skipped: found.links_skipped,
            truncated: found.truncated,
            ..ConvertAdded::default()
        };
        let known: std::collections::HashSet<PathBuf> =
            s.files.iter().map(|e| e.path.clone()).collect();
        for path in found.files {
            if known.contains(&path) {
                out.duplicates += 1;
                continue;
            }
            if s.files.len() >= MAX_LIST {
                out.truncated = true;
                break;
            }
            s.next_id += 1;
            let id = s.next_id;
            let bytes = std::fs::metadata(&path).map_or(0, |m| m.len());
            let format = head(&path).and_then(|h| sniff(&h));
            s.files.push(Entry {
                id,
                name: display_name(&path),
                path,
                format,
                bytes,
                output: None,
                done: false,
            });
            out.ids.push(id);
        }
        out.added = out.ids.len();
        out
    }

    /// The files of the list, in the order they were added.
    pub fn convert_list(&self) -> Vec<ConvertFileView> {
        lock(&self.inner.convert)
            .files
            .iter()
            .map(|e| ConvertFileView {
                id: e.id,
                name: e.name.clone(),
                format: format_name(e.format).to_owned(),
                bytes: e.bytes,
            })
            .collect()
    }

    /// Takes files out of the list.
    pub fn convert_remove(&self, ids: &[u32]) {
        lock(&self.inner.convert)
            .files
            .retain(|e| !ids.contains(&e.id));
    }

    /// Empties the list (a run in progress is not stopped: see [`Engine::convert_cancel`]).
    pub fn convert_clear(&self) {
        lock(&self.inner.convert).files.clear();
    }

    /// Chooses the folder of [`Placement::Folder`]; returns its name for display. `None` forgets it.
    pub fn convert_set_folder(&self, folder: Option<PathBuf>) -> Option<String> {
        let mut s = lock(&self.inner.convert);
        s.folder = folder;
        s.folder.as_deref().map(display_name)
    }

    /// The folder's name, if one is chosen.
    pub fn convert_folder_name(&self) -> Option<String> {
        lock(&self.inner.convert)
            .folder
            .as_deref()
            .map(display_name)
    }

    /// What a run with `opts` would do to the list, from the files' magic bytes.
    pub fn convert_preview(&self, opts: &ConvertOptions) -> ConvertPreview {
        let heif = Format::Heic.is_decodable();
        let s = lock(&self.inner.convert);
        let target = opts.target.format();
        let mut by_format: BTreeMap<&'static str, usize> = BTreeMap::new();
        let mut p = ConvertPreview {
            total: 0,
            will_convert: 0,
            same_format: 0,
            cannot: 0,
            by_format: Vec::new(),
            cannot_files: Vec::new(),
            heif_available: heif,
            heif_unavailable: 0,
            bytes_to_convert: 0,
            placement: opts.placement,
            folder: s.folder.as_deref().map(display_name),
        };
        for e in s.files.iter().filter(|e| !e.done) {
            p.total += 1;
            *by_format.entry(format_name(e.format)).or_default() += 1;
            let cannot = match e.format {
                Some(f) if f.is_decodable() => None,
                Some(Format::Heic | Format::Avif) => {
                    p.heif_unavailable += 1;
                    Some(Skip::HeifUnavailable)
                }
                _ => Some(Skip::Unsupported),
            };
            if let Some(why) = cannot {
                p.cannot += 1;
                if p.cannot_files.len() < MAX_CANNOT_LISTED {
                    p.cannot_files.push(CannotConvert {
                        id: e.id,
                        name: e.name.clone(),
                        format: format_name(e.format).to_owned(),
                        code: why.code().to_owned(),
                    });
                }
            } else if e.format == Some(target) && !opts.reencode {
                p.same_format += 1;
            } else {
                p.will_convert += 1;
                p.bytes_to_convert += e.bytes;
            }
        }
        p.by_format = by_format
            .into_iter()
            .map(|(format, count)| FormatCount {
                format: format.to_owned(),
                count,
            })
            .collect();
        p
    }

    /// Stops the run in progress after the files being converted (each one finishes, none is
    /// abandoned half way).
    pub fn convert_cancel(&self) {
        if let Some(c) = &lock(&self.inner.convert).cancel {
            c.cancel();
        }
    }

    /// A run is in progress.
    pub fn convert_running(&self) -> bool {
        lock(&self.inner.convert).running
    }

    /// The folder a converted file went to (by list id), or the folder of the first converted file.
    pub fn convert_output_dir(&self, id: Option<u32>) -> Option<PathBuf> {
        let s = lock(&self.inner.convert);
        s.files
            .iter()
            .filter(|e| id.is_none_or(|i| e.id == i))
            .find_map(|e| e.output.as_deref())
            .and_then(|p| p.parent().map(Path::to_path_buf))
    }

    /// Converts the files of the list that are not converted yet. `jobs` files run at once (at
    /// least 1). `notify` hears every finished file, from the worker that finished it. One run is
    /// one backup run in the Backups view, named `run_name`.
    pub fn convert_run(
        &self,
        opts: &ConvertOptions,
        run_name: &str,
        jobs: usize,
        notify: &(dyn Fn(ConvertProgress) + Sync),
    ) -> ConvertSummary {
        let run_id = new_id();
        let run_name: String = run_name
            .chars()
            .filter(|c| !c.is_control())
            .take(80)
            .collect();
        let (entries, folder, cancel) = {
            let mut s = lock(&self.inner.convert);
            let cancel = CancelToken::new_batch();
            s.cancel = Some(cancel.clone());
            s.running = true;
            (
                s.files
                    .iter()
                    .filter(|e| !e.done)
                    .cloned()
                    .collect::<Vec<_>>(),
                s.folder.clone(),
                cancel,
            )
        };
        let total = entries.len();
        let budget = MemoryBudget::from_system();
        let next = AtomicUsize::new(0);
        let finished = AtomicUsize::new(0);
        let slots: std::sync::Mutex<Vec<Option<FileDone>>> =
            std::sync::Mutex::new(vec![None; total]);
        let one = |entry: &Entry| -> FileDone {
            if cancel.is_cancelled() {
                return result_of(
                    entry,
                    Err(ConvertError::Skipped(Skip::Unsupported, Vec::new())),
                )
                .into_cancelled();
            }
            let mut req = ConvertRequest::new(&entry.path, *opts, &run_id, &run_name);
            req.folder = folder.as_deref();
            req.budget = Some(&budget);
            req.cancel = Some(&cancel);
            let r = self.convert_file(&req);
            result_of(entry, r)
        };
        let workers = jobs.clamp(1, 16).min(total.max(1));
        std::thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, Ordering::SeqCst);
                        let Some(entry) = entries.get(i) else { break };
                        let (result, output) = one(entry);
                        let done = finished.fetch_add(1, Ordering::SeqCst) + 1;
                        slots.lock().unwrap_or_else(|e| e.into_inner())[i] =
                            Some((result.clone(), output));
                        notify(ConvertProgress {
                            done,
                            total,
                            result,
                        });
                    }
                });
            }
        });
        let slots = slots.into_inner().unwrap_or_else(|e| e.into_inner());
        let mut summary = ConvertSummary {
            run_id,
            converted: 0,
            skipped: 0,
            failed: 0,
            cancelled: cancel.is_cancelled(),
            bytes_in: 0,
            bytes_out: 0,
            results: Vec::with_capacity(total),
        };
        {
            let mut s = lock(&self.inner.convert);
            s.running = false;
            s.cancel = None;
            for (entry, slot) in entries.iter().zip(slots) {
                let Some((result, output)) = slot else {
                    continue;
                };
                match result.status {
                    ConvertStatus::Converted => {
                        summary.converted += 1;
                        summary.bytes_in += result.bytes_in;
                        summary.bytes_out += result.bytes_out;
                        if let Some(e) = s.files.iter_mut().find(|e| e.id == entry.id) {
                            e.done = true;
                            e.output = output;
                        }
                    }
                    ConvertStatus::Skipped => summary.skipped += 1,
                    ConvertStatus::Failed => summary.failed += 1,
                }
                summary.results.push(result);
            }
        }
        summary
    }
}

/// A file the run never reached because it was cancelled.
trait IntoCancelled {
    fn into_cancelled(self) -> Self;
}

impl IntoCancelled for FileDone {
    fn into_cancelled(mut self) -> Self {
        self.0.status = ConvertStatus::Skipped;
        self.0.code = Some("CANCELLED".to_owned());
        self.0.notices.clear();
        self
    }
}

/// `true` when `opts` would write next to or into somewhere other than the source's place.
pub fn keeps_originals(opts: &ConvertOptions) -> bool {
    opts.placement != Placement::Replace
}

/// The colour mode as the CLI writes it.
pub fn colour_name(c: ColourMode) -> &'static str {
    match c {
        ColourMode::Srgb => "srgb",
        ColourMode::Preserve => "preserve",
    }
}
