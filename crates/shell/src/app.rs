// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The Tauri side: commands, the `acimg` scheme, pickers and drag-and-drop.

use auto_crop_engine::convert::ConvertOptions;
use auto_crop_engine::convert_batch::{
    ConvertAdded, ConvertFileView, ConvertPreview, ConvertProgress, ConvertSummary,
};
use auto_crop_engine::curved::CurveSuggestion;
use auto_crop_engine::{
    AppPaths, CurveWarp, Cut, DerivedAction, Edit, Engine, EnhanceBatch, EnhanceMode,
    EnhanceSuggestion, ErrKind, ItemView, Notify, OpenSummary, Pt, RestoreMode, RestoreOutcome,
    RevertTo, SaveOutcome, SaveTarget, SessionStep, Settings, SplitPatch, Superseded, TrimProposal,
};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tauri::http::{Response, StatusCode};
use tauri::{AppHandle, DragDropEvent, Emitter, Manager, State, WindowEvent};
use tauri_plugin_dialog::DialogExt;

/// Tile responses above this size are refused (PLAN 8.6.4).
const MAX_BODY: usize = 8 * 1024 * 1024;

#[derive(Clone)]
struct Shared {
    engine: Engine,
    /// 128-bit random launch token. It is not a credential: it keeps the image URLs unguessable
    /// by anything but this process's own webview, and it changes every launch.
    token: String,
    /// Counters of the curve previews: a request that starts while an older one of the same kind
    /// is still rendering cancels the older one (a drag makes many, only the last matters).
    live_preview: Arc<AtomicU64>,
    full_preview: Arc<AtomicU64>,
    /// The Convert screen is showing: files dropped on the window go to its list, not to the crop
    /// flow. Set by the webview (`convert_mode`), read by the drop handler.
    convert_mode: Arc<AtomicBool>,
}

type Cmd<T> = Result<T, ErrKind>;

fn random_token() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the OS random source is available");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn notifier(app: &AppHandle) -> Notify {
    let app = app.clone();
    Arc::new(move |v: ItemView| {
        let _ = app.emit("item-updated", v);
    })
}

fn opened(shared: &Shared, app: &AppHandle, summary: OpenSummary) -> OpenSummary {
    shared
        .engine
        .spawn_analysis(summary.ids.clone(), notifier(app));
    summary
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LaunchInfo {
    token: String,
    version: &'static str,
    platform: &'static str,
    backups_location: String,
    /// The file extensions this build opens (lower case, no dot), so the UI lists the formats
    /// truthfully.
    input_extensions: Vec<String>,
    /// HEIC, HEIF and AVIF input is compiled in.
    heif: bool,
    /// AVIF output is compiled in (Convert and Save as offer AVIF).
    avif: bool,
}

#[tauri::command]
fn launch_info(shared: State<'_, Shared>) -> LaunchInfo {
    LaunchInfo {
        token: shared.token.clone(),
        version: env!("CARGO_PKG_VERSION"),
        platform: if cfg!(windows) {
            "windows"
        } else if cfg!(target_os = "macos") {
            "macos"
        } else {
            "linux"
        },
        backups_location: shared
            .engine
            .paths()
            .backups_dir()
            .to_string_lossy()
            .into_owned(),
        input_extensions: auto_crop_engine::enumerate::input_extensions()
            .iter()
            .map(|e| (*e).to_owned())
            .collect(),
        heif: cfg!(feature = "heif"),
        avif: cfg!(feature = "avif"),
    }
}

#[tauri::command]
async fn pick_files(app: AppHandle, shared: State<'_, Shared>) -> Cmd<OpenSummary> {
    // The picker offers exactly the formats the engine opens in this build.
    let picked = app
        .dialog()
        .file()
        .add_filter("Images", auto_crop_engine::enumerate::input_extensions())
        .blocking_pick_files();
    let paths: Vec<PathBuf> = picked
        .unwrap_or_default()
        .into_iter()
        .filter_map(|p| p.into_path().ok())
        .collect();
    let summary = shared.engine.open_paths(&paths, false);
    Ok(opened(&shared, &app, summary))
}

#[tauri::command]
async fn pick_folder(
    app: AppHandle,
    shared: State<'_, Shared>,
    include_subfolders: bool,
) -> Cmd<OpenSummary> {
    let paths: Vec<PathBuf> = app
        .dialog()
        .file()
        .blocking_pick_folder()
        .and_then(|p| p.into_path().ok())
        .into_iter()
        .collect();
    let summary = shared.engine.open_paths(&paths, include_subfolders);
    Ok(opened(&shared, &app, summary))
}

#[tauri::command]
async fn add_samples(app: AppHandle, shared: State<'_, Shared>) -> Cmd<OpenSummary> {
    let engine = shared.engine.clone();
    let summary = tauri::async_runtime::spawn_blocking(move || engine.add_samples())
        .await
        .map_err(|_| ErrKind::Internal)??;
    Ok(opened(&shared, &app, summary))
}

#[tauri::command]
fn list_items(shared: State<'_, Shared>) -> Vec<ItemView> {
    shared.engine.list_items()
}

#[tauri::command]
fn set_edit(
    shared: State<'_, Shared>,
    id: u32,
    edit: Edit,
    phase: String,
    label: String,
) -> Cmd<ItemView> {
    shared.engine.set_edit(id, &edit, phase == "live", &label)
}

#[tauri::command]
fn undo(shared: State<'_, Shared>, id: u32) -> Cmd<ItemView> {
    shared.engine.undo(id)
}

#[tauri::command]
fn redo(shared: State<'_, Shared>, id: u32) -> Cmd<ItemView> {
    shared.engine.redo(id)
}

#[tauri::command]
fn reset_to_auto(shared: State<'_, Shared>, id: u32) -> Cmd<ItemView> {
    shared.engine.reset_to_auto(id)
}

#[tauri::command]
fn draw_crop(shared: State<'_, Shared>, id: u32) -> Cmd<ItemView> {
    shared.engine.draw_crop(id)
}

/// Runs engine work that may decode or detect off the main thread.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Cmd<T> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|_| ErrKind::Internal)
}

// ---------------------------------------------------------------- crops (ids only, M10)

#[tauri::command]
fn set_crop_edit(
    shared: State<'_, Shared>,
    id: u32,
    crop: u32,
    edit: Edit,
    phase: String,
    label: String,
    gesture: Option<u64>,
) -> Cmd<ItemView> {
    shared
        .engine
        .set_crop_edit(id, crop, &edit, phase == "live", &label, gesture)
}

/// May run the detector (`at`), so it leaves the main thread.
#[tauri::command]
async fn add_crop(
    shared: State<'_, Shared>,
    id: u32,
    quad: Option<[Pt; 4]>,
    at: Option<Pt>,
) -> Cmd<ItemView> {
    let engine = shared.engine.clone();
    blocking(move || engine.add_crop(id, quad, at)).await?
}

// ---------------------------------------------------------------- curved pages (ids only)

/// A quad crop becomes a curved page with four straight edges (idempotent on a curved crop).
#[tauri::command]
fn curve_from_quad(shared: State<'_, Shared>, id: u32, crop: u32) -> Cmd<ItemView> {
    shared.engine.curve_from_quad(id, crop)
}

/// `phase: "live"` validates and returns the view without recording anything (a drag in progress);
/// anything else is one undo step, and the same `gesture` coalesces a burst of nudges into one.
#[tauri::command]
fn set_curves(
    shared: State<'_, Shared>,
    id: u32,
    crop: u32,
    curves: CurveWarp,
    phase: String,
    label: String,
    gesture: Option<u64>,
) -> Cmd<ItemView> {
    shared
        .engine
        .set_curves(id, crop, &curves, phase == "live", &label, gesture)
}

/// The engine's PROPOSAL of four curves for one crop (`Engine::suggest_curves`). It records and applies
/// nothing: the UI shows it as a preview and, on the person's click, passes `curves` whole to
/// `set_curves`. `None` while the image is not ready. The fit takes 5 to 25 ms of one thread, so it
/// runs on the blocking pool like the other engine calls that compute, never on the main thread.
#[tauri::command]
async fn suggest_curves(
    shared: State<'_, Shared>,
    id: u32,
    crop: u32,
) -> Cmd<Option<CurveSuggestion>> {
    let engine = shared.engine.clone();
    blocking(move || engine.suggest_curves(id, crop)).await?
}

// ---------------------------------------------------------------- trim to content (ids only)

/// The engine's PROPOSAL of the picture inside the page of one crop (`Engine::trim_to_content`): a print
/// on a wide white mat has two outlines, and the detector keeps the paper edge. It records and applies
/// nothing; `None` when there is no picture inside to trim to (and while the image is not ready). A curved
/// crop answers `ITEM_OP`. It takes a few milliseconds to a few tens of one thread, so it runs on the
/// blocking pool, never on the main thread.
#[tauri::command]
async fn trim_to_content(
    shared: State<'_, Shared>,
    id: u32,
    crop: u32,
) -> Cmd<Option<TrimProposal>> {
    let engine = shared.engine.clone();
    blocking(move || engine.trim_to_content(id, crop)).await?
}

/// Applies the trim of one crop as ONE undo step ("Trim to content"). The engine recomputes the
/// proposal on the crop as it is now (no number from the webview is trusted) and refuses with
/// `ITEM_OP` when there is nothing to trim to, the crop is curved or it changed meanwhile.
#[tauri::command]
async fn apply_trim(shared: State<'_, Shared>, id: u32, crop: u32) -> Cmd<ItemView> {
    let engine = shared.engine.clone();
    blocking(move || engine.apply_trim(id, crop)).await?
}

// ---------------------------------------------------------------- enhancement (ids and numbers only)

/// Sets one crop's enhancement: `mode` is `original`, `clean`, `document` or `receipt`, `strength`
/// 0 to 1. `phase: "live"` validates and returns the view without recording anything (a slider in
/// motion; the picture comes from the `enhance-result` route of the `acimg` scheme); anything else
/// is one undo step, and the same `gesture` coalesces a drag into one. Never applied by anything
/// but the person's choice.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
fn set_enhance(
    shared: State<'_, Shared>,
    id: u32,
    crop: u32,
    mode: EnhanceMode,
    strength: f32,
    phase: String,
    label: String,
    gesture: Option<u64>,
) -> Cmd<ItemView> {
    shared
        .engine
        .set_enhance(id, crop, mode, strength, phase == "live", &label, gesture)
}

/// The crop follows the image's default enhancement again.
#[tauri::command]
fn clear_enhance(shared: State<'_, Shared>, id: u32, crop: u32) -> Cmd<ItemView> {
    shared.engine.clear_enhance(id, crop)
}

/// Sets the default enhancement of the image (crops without their own override get it).
#[tauri::command]
fn set_default_enhance(
    shared: State<'_, Shared>,
    id: u32,
    mode: EnhanceMode,
    strength: f32,
    phase: String,
    label: String,
    gesture: Option<u64>,
) -> Cmd<ItemView> {
    shared
        .engine
        .set_default_enhance(id, mode, strength, phase == "live", &label, gesture)
}

/// The "All items in this scan" scope: the default enhancement of the image AND no crop keeps its
/// own override, as ONE undo step (`Engine::set_scan_enhance`). Arguments as `set_default_enhance`.
#[tauri::command]
fn set_scan_enhance(
    shared: State<'_, Shared>,
    id: u32,
    mode: EnhanceMode,
    strength: f32,
    phase: String,
    label: String,
    gesture: Option<u64>,
) -> Cmd<ItemView> {
    shared
        .engine
        .set_scan_enhance(id, mode, strength, phase == "live", &label, gesture)
}

/// The engine's RECOMMENDATION for one crop (`Engine::suggest_enhance`): the mode, why, the
/// black-and-white look that fits and what it writes. `None` when the crop should be left alone
/// (a photograph, a clean scan) and while the image is not ready. It records and applies nothing:
/// the UI shows a chip and, on the person's tap, calls `set_enhance`. A few tens of milliseconds
/// the first time (cached after), so it runs on the blocking pool.
#[tauri::command]
async fn suggest_enhance(
    shared: State<'_, Shared>,
    id: u32,
    crop: u32,
) -> Cmd<Option<EnhanceSuggestion>> {
    let engine = shared.engine.clone();
    blocking(move || engine.suggest_enhance(id, crop)).await?
}

/// "Enhance all suggested": applies the engine's recommendation to every crop of `ids` that has
/// one and no enhancement yet, as ONE step of the session history (`session_undo` reverts it).
/// Only ever called by a click on that action; the engine never applies a suggestion by itself.
/// Every changed image is announced with `item-updated`. On the blocking pool (a few tens of
/// milliseconds per crop the first time).
#[tauri::command]
async fn enhance_suggested(
    app: AppHandle,
    shared: State<'_, Shared>,
    ids: Vec<u32>,
) -> Cmd<EnhanceBatch> {
    let engine = shared.engine.clone();
    let batch = blocking(move || engine.enhance_suggested(&ids)).await?;
    let notify = notifier(&app);
    for v in &batch.views {
        notify(v.clone());
    }
    Ok(batch)
}

/// A curved crop becomes the straight quad through its corners.
#[tauri::command]
fn clear_curves(shared: State<'_, Shared>, id: u32, crop: u32) -> Cmd<ItemView> {
    shared.engine.clear_curves(id, crop)
}

#[tauri::command]
fn remove_crop(shared: State<'_, Shared>, id: u32, crop: u32) -> Cmd<ItemView> {
    shared.engine.remove_crop(id, crop)
}

#[tauri::command]
fn restore_crop(shared: State<'_, Shared>, id: u32, crop: u32) -> Cmd<ItemView> {
    shared.engine.restore_crop(id, crop)
}

#[tauri::command]
fn merge_crops(shared: State<'_, Shared>, id: u32, crops: Vec<u32>) -> Cmd<ItemView> {
    shared.engine.merge_crops(id, &crops)
}

#[tauri::command]
fn cut_crop(shared: State<'_, Shared>, id: u32, crop: u32, cut: Cut) -> Cmd<ItemView> {
    shared.engine.cut_crop(id, crop, cut)
}

#[tauri::command]
fn move_crop(shared: State<'_, Shared>, id: u32, crop: u32, to_index: usize) -> Cmd<ItemView> {
    shared.engine.move_crop(id, crop, to_index)
}

#[tauri::command]
fn use_reading_order(shared: State<'_, Shared>, id: u32) -> Cmd<ItemView> {
    shared.engine.use_reading_order(id)
}

#[tauri::command]
fn turn_crop(shared: State<'_, Shared>, id: u32, crop: u32, clockwise: bool) -> Cmd<ItemView> {
    shared.engine.turn_crop(id, crop, clockwise)
}

#[tauri::command]
fn set_crop_angle(
    shared: State<'_, Shared>,
    id: u32,
    crop: u32,
    deg: f32,
    gesture: Option<u64>,
) -> Cmd<ItemView> {
    shared.engine.set_crop_angle(id, crop, deg, gesture)
}

#[tauri::command]
fn flip_crop(shared: State<'_, Shared>, id: u32, crop: u32) -> Cmd<ItemView> {
    shared.engine.flip_crop(id, crop)
}

#[tauri::command]
fn revert_crop(shared: State<'_, Shared>, id: u32, crop: u32, to: RevertTo) -> Cmd<ItemView> {
    shared.engine.revert_crop(id, crop, to)
}

/// Runs the detector again, so it leaves the main thread.
#[tauri::command]
async fn redetect(shared: State<'_, Shared>, id: u32, patch: SplitPatch) -> Cmd<ItemView> {
    let engine = shared.engine.clone();
    blocking(move || engine.redetect(id, patch)).await?
}

/// The outcome for one image of [`redetect_many`].
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RedetectResult {
    id: u32,
    view: Option<ItemView>,
    error: Option<ErrKind>,
}

/// One session undo step for all of them; every changed image is also announced as
/// `item-updated`.
#[tauri::command]
async fn redetect_many(
    app: AppHandle,
    shared: State<'_, Shared>,
    ids: Vec<u32>,
    patch: SplitPatch,
) -> Cmd<Vec<RedetectResult>> {
    let engine = shared.engine.clone();
    let results = blocking(move || engine.redetect_many(&ids, patch)).await?;
    let notify = notifier(&app);
    Ok(results
        .into_iter()
        .map(|(id, r)| match r {
            Ok(view) => {
                notify(view.clone());
                RedetectResult {
                    id,
                    view: Some(view),
                    error: None,
                }
            }
            Err(e) => RedetectResult {
                id,
                view: None,
                error: Some(e),
            },
        })
        .collect())
}

fn session_step(app: &AppHandle, step: Option<(String, Vec<ItemView>)>) -> Option<SessionStep> {
    let (label, items) = step?;
    let notify = notifier(app);
    for v in &items {
        notify(v.clone());
    }
    Some(SessionStep { label, items })
}

#[tauri::command]
fn session_undo(app: AppHandle, shared: State<'_, Shared>) -> Option<SessionStep> {
    session_step(&app, shared.engine.session_undo())
}

#[tauri::command]
fn session_redo(app: AppHandle, shared: State<'_, Shared>) -> Option<SessionStep> {
    session_step(&app, shared.engine.session_redo())
}

#[tauri::command]
fn accept_scan(shared: State<'_, Shared>, id: u32) -> Cmd<ItemView> {
    shared.engine.accept_scan(id)
}

#[tauri::command]
fn unaccept_scan(shared: State<'_, Shared>, id: u32) -> Cmd<ItemView> {
    shared.engine.unaccept_scan(id)
}

#[tauri::command]
fn remove_items(shared: State<'_, Shared>, ids: Vec<u32>) {
    shared.engine.remove_items(&ids);
}

#[tauri::command]
async fn save_items(
    app: AppHandle,
    shared: State<'_, Shared>,
    ids: Vec<u32>,
    target: SaveTarget,
    run_name: String,
) -> Cmd<Vec<SaveOutcome>> {
    let engine = shared.engine.clone();
    tauri::async_runtime::spawn_blocking(move || {
        engine.save_items(&ids, target, &run_name, &|v| {
            let _ = app.emit("item-updated", v);
        })
    })
    .await
    .map_err(|_| ErrKind::Internal)
}

#[tauri::command]
fn get_settings(shared: State<'_, Shared>) -> Settings {
    shared.engine.settings()
}

/// The parameter is named `s` because the webview's `Api.setSettings(s)` sends `{ s }`.
#[tauri::command]
fn set_settings(shared: State<'_, Shared>, s: Settings) -> Settings {
    shared.engine.set_settings(s)
}

#[tauri::command]
async fn list_backups(shared: State<'_, Shared>) -> Cmd<auto_crop_engine::BackupsView> {
    let engine = shared.engine.clone();
    tauri::async_runtime::spawn_blocking(move || engine.list_backups())
        .await
        .map_err(|_| ErrKind::Internal)
}

#[tauri::command]
async fn restore_file(
    app: AppHandle,
    shared: State<'_, Shared>,
    file_id: String,
    mode: RestoreMode,
) -> Cmd<RestoreOutcome> {
    let engine = shared.engine.clone();
    tauri::async_runtime::spawn_blocking(move || {
        engine.restore_file(&file_id, mode, &|v| {
            let _ = app.emit("item-updated", v);
        })
    })
    .await
    .map_err(|_| ErrKind::Internal)
}

#[tauri::command]
async fn restore_run(
    app: AppHandle,
    shared: State<'_, Shared>,
    run_id: String,
) -> Cmd<Vec<RestoreOutcome>> {
    let engine = shared.engine.clone();
    tauri::async_runtime::spawn_blocking(move || {
        engine.restore_run(&run_id, &|v| {
            let _ = app.emit("item-updated", v);
        })
    })
    .await
    .map_err(|_| ErrKind::Internal)
}

#[tauri::command]
async fn restore_file_derived(
    app: AppHandle,
    shared: State<'_, Shared>,
    file_id: String,
    mode: RestoreMode,
    derived: DerivedAction,
) -> Cmd<RestoreOutcome> {
    let engine = shared.engine.clone();
    blocking(move || {
        engine.restore_file_derived(&file_id, mode, derived, &|v| {
            let _ = app.emit("item-updated", v);
        })
    })
    .await
}

#[tauri::command]
async fn restore_run_derived(
    app: AppHandle,
    shared: State<'_, Shared>,
    run_id: String,
    derived: DerivedAction,
) -> Cmd<Vec<RestoreOutcome>> {
    let engine = shared.engine.clone();
    blocking(move || {
        engine.restore_run_derived(&run_id, derived, &|v| {
            let _ = app.emit("item-updated", v);
        })
    })
    .await
}

#[tauri::command]
fn pin_run(shared: State<'_, Shared>, run_id: String, pinned: bool) {
    shared.engine.pin_run(&run_id, pinned);
}

#[tauri::command]
fn purge_now(shared: State<'_, Shared>) -> usize {
    shared.engine.purge_now()
}

/// Opens the backup store in the file manager. The path is ours, never the webview's.
#[tauri::command]
fn open_backups_folder(shared: State<'_, Shared>) -> Cmd<()> {
    let dir = shared.engine.paths().backups_dir();
    std::fs::create_dir_all(&dir).map_err(|e| ErrKind::from_io(&e))?;
    let opener = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener)
        .arg(&dir)
        .spawn()
        .map(|_| ())
        .map_err(|_| ErrKind::Unreadable)
}

// ---------------------------------------------------------------- Convert (ids and counts only)

/// The webview tells the shell whether the Convert screen is showing, so a drop goes to its list.
#[tauri::command]
fn convert_mode(shared: State<'_, Shared>, on: bool) {
    shared.convert_mode.store(on, Ordering::SeqCst);
}

#[tauri::command]
async fn convert_pick_files(app: AppHandle, shared: State<'_, Shared>) -> Cmd<ConvertAdded> {
    let picked = app
        .dialog()
        .file()
        .add_filter("Images", &auto_crop_engine::enumerate::convert_extensions())
        .blocking_pick_files();
    let paths: Vec<PathBuf> = picked
        .unwrap_or_default()
        .into_iter()
        .filter_map(|p| p.into_path().ok())
        .collect();
    let engine = shared.engine.clone();
    blocking(move || engine.convert_add(&paths, false)).await
}

#[tauri::command]
async fn convert_pick_folder(
    app: AppHandle,
    shared: State<'_, Shared>,
    include_subfolders: bool,
) -> Cmd<ConvertAdded> {
    let paths: Vec<PathBuf> = app
        .dialog()
        .file()
        .blocking_pick_folder()
        .and_then(|p| p.into_path().ok())
        .into_iter()
        .collect();
    let engine = shared.engine.clone();
    blocking(move || engine.convert_add(&paths, include_subfolders)).await
}

/// The folder of "Save to a folder": picked here, the webview only ever sees its name.
#[tauri::command]
async fn convert_pick_output_folder(
    app: AppHandle,
    shared: State<'_, Shared>,
) -> Cmd<Option<String>> {
    let picked = app
        .dialog()
        .file()
        .blocking_pick_folder()
        .and_then(|p| p.into_path().ok());
    Ok(match picked {
        Some(p) => shared.engine.convert_set_folder(Some(p)),
        None => shared.engine.convert_folder_name(),
    })
}

#[tauri::command]
fn convert_list(shared: State<'_, Shared>) -> Vec<ConvertFileView> {
    shared.engine.convert_list()
}

#[tauri::command]
fn convert_remove(shared: State<'_, Shared>, ids: Vec<u32>) {
    shared.engine.convert_remove(&ids);
}

#[tauri::command]
fn convert_clear(shared: State<'_, Shared>) {
    shared.engine.convert_clear();
}

#[tauri::command]
fn convert_preview(shared: State<'_, Shared>, options: ConvertOptions) -> ConvertPreview {
    shared.engine.convert_preview(&options)
}

/// Converts the list. Progress is emitted as `convert-progress` events (one per finished file).
#[tauri::command]
async fn convert_run(
    app: AppHandle,
    shared: State<'_, Shared>,
    options: ConvertOptions,
    run_name: String,
) -> Cmd<ConvertSummary> {
    let engine = shared.engine.clone();
    let jobs = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .div_euclid(2)
        .clamp(1, 4);
    blocking(move || {
        engine.convert_run(&options, &run_name, jobs, &|p: ConvertProgress| {
            let _ = app.emit("convert-progress", p);
        })
    })
    .await
}

#[tauri::command]
fn convert_cancel(shared: State<'_, Shared>) {
    shared.engine.convert_cancel();
}

/// Opens the folder a converted file went to in the file manager. The path is ours, never the
/// webview's.
#[tauri::command]
fn convert_open_folder(shared: State<'_, Shared>, id: Option<u32>) -> Cmd<()> {
    let dir = shared
        .engine
        .convert_output_dir(id)
        .ok_or(ErrKind::Unreadable)?;
    let opener = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener)
        .arg(&dir)
        .spawn()
        .map(|_| ())
        .map_err(|_| ErrKind::Unreadable)
}

fn image_response(status: StatusCode, mime: &str, body: Vec<u8>) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header("Content-Type", mime)
        .header("Cache-Control", "no-store")
        .header("X-Content-Type-Options", "nosniff")
        .body(body)
        .expect("static response parts are valid")
}

/// Answers one `acimg` request: the whole image, one crop of it, or the preview of a candidate
/// curve set (the curves are in `query`), or a bare 404 for anything that is not exactly one of
/// those.
fn serve_image(shared: &Shared, path: &str, query: Option<&str>) -> Response<Vec<u8>> {
    let empty = |status| image_response(status, "text/plain", Vec::new());
    let Some(image) = crate::parse_image_path(path, &shared.token) else {
        return empty(StatusCode::NOT_FOUND);
    };
    let result = match image {
        crate::ImagePath::Whole(id, kind) => shared.engine.image_bytes(id, kind),
        crate::ImagePath::Crop(id, crop, kind) => shared.engine.crop_image_bytes(id, crop, kind),
        crate::ImagePath::CurvePreview(id, crop, kind) => {
            let Some(curves) = query.and_then(crate::parse_curve_query) else {
                return empty(StatusCode::BAD_REQUEST);
            };
            // A newer preview of the same kind supersedes this one (see `Shared`).
            let counter = if kind == auto_crop_engine::CropImage::Thumb {
                &shared.live_preview
            } else {
                &shared.full_preview
            };
            let token = Superseded::begin(counter);
            match shared
                .engine
                .preview_curves(id, crop, &curves, kind, &token)
            {
                // A superseded request is answered with an empty 204; the webview has moved on.
                Err(ErrKind::Cancelled) => return empty(StatusCode::NO_CONTENT),
                // The page the person is drawing is not one the renderer accepts (a crossing
                // outline, a point out of range): nothing to show.
                Err(ErrKind::Degenerate) => return empty(StatusCode::UNPROCESSABLE_ENTITY),
                other => other,
            }
        }
        crate::ImagePath::EnhancePreview(id, crop, kind) => {
            let Some(candidate) = query.and_then(crate::parse_enhance_query) else {
                return empty(StatusCode::BAD_REQUEST);
            };
            // The same supersession rule as the curve previews: a slider drag makes many requests,
            // only the last one matters.
            let counter = if kind == auto_crop_engine::CropImage::Thumb {
                &shared.live_preview
            } else {
                &shared.full_preview
            };
            let token = Superseded::begin(counter);
            match shared
                .engine
                .preview_enhance(id, crop, candidate, kind, &token)
            {
                Err(ErrKind::Cancelled) => return empty(StatusCode::NO_CONTENT),
                other => other,
            }
        }
    };
    match result {
        Ok((bytes, mime)) if bytes.len() <= MAX_BODY => {
            image_response(StatusCode::OK, mime, bytes.as_ref().clone())
        }
        Ok(_) => empty(StatusCode::PAYLOAD_TOO_LARGE),
        Err(ErrKind::NoCrop) => empty(StatusCode::NOT_FOUND),
        Err(_) => empty(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub fn run() {
    // Packaged builds keep the HEIC plugin folder beside the executable; set before any decode.
    auto_crop_engine::packaged::configure_heif_from_exe();
    auto_crop_engine::packaged::configure_decode_from_exe();
    // `AUTO_CROP_HOME` keeps all data (settings, backups, samples) under one folder: for tests and
    // for trying the app without touching the real per-user store.
    let paths = std::env::var_os("AUTO_CROP_HOME")
        .filter(|v| !v.is_empty())
        .map(|home| AppPaths::under(&PathBuf::from(home)))
        .or_else(AppPaths::for_current_user)
        .unwrap_or_else(|| {
            // No home directory: keep everything in the temp dir rather than refuse to start.
            AppPaths::under(&std::env::temp_dir().join("AutoCrop"))
        });
    let shared = Shared {
        engine: Engine::new(paths),
        token: random_token(),
        live_preview: Arc::new(AtomicU64::new(0)),
        full_preview: Arc::new(AtomicU64::new(0)),
        convert_mode: Arc::new(AtomicBool::new(false)),
    };
    let for_scheme = shared.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(shared)
        .register_asynchronous_uri_scheme_protocol("acimg", move |_ctx, request, responder| {
            let shared = for_scheme.clone();
            let path = request.uri().path().to_owned();
            let query = request.uri().query().map(str::to_owned);
            // Rendering can take tens of milliseconds; never block the webview's thread.
            std::thread::spawn(move || {
                let response = serve_image(&shared, &path, query.as_deref());
                responder.respond(response);
            });
        })
        .invoke_handler(tauri::generate_handler![
            launch_info,
            pick_files,
            pick_folder,
            add_samples,
            list_items,
            set_edit,
            undo,
            redo,
            reset_to_auto,
            draw_crop,
            remove_items,
            save_items,
            get_settings,
            set_settings,
            list_backups,
            restore_file,
            restore_run,
            set_crop_edit,
            curve_from_quad,
            set_curves,
            set_enhance,
            clear_enhance,
            set_default_enhance,
            set_scan_enhance,
            suggest_enhance,
            enhance_suggested,
            suggest_curves,
            trim_to_content,
            apply_trim,
            clear_curves,
            add_crop,
            remove_crop,
            restore_crop,
            merge_crops,
            cut_crop,
            move_crop,
            use_reading_order,
            turn_crop,
            set_crop_angle,
            flip_crop,
            revert_crop,
            redetect,
            redetect_many,
            session_undo,
            session_redo,
            accept_scan,
            unaccept_scan,
            restore_file_derived,
            restore_run_derived,
            pin_run,
            purge_now,
            open_backups_folder,
            convert_mode,
            convert_pick_files,
            convert_pick_folder,
            convert_pick_output_folder,
            convert_list,
            convert_remove,
            convert_clear,
            convert_preview,
            convert_run,
            convert_cancel,
            convert_open_folder,
        ])
        .setup(|app| {
            // "Open with" and `AutoCrop.exe file-or-folder ...`: paths on the command line are opened like a
            // drop. They are opened here, in Rust, before the webview asks for its list of items.
            let mut args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
            // `AutoCrop.exe --convert file-or-folder ...` (a "Convert with Auto Crop" shortcut)
            // puts the paths on the Convert list instead of opening them for cropping.
            let convert = args.first().is_some_and(|a| a == "--convert");
            if convert {
                args.remove(0);
            }
            let paths: Vec<PathBuf> = args
                .into_iter()
                .map(PathBuf::from)
                .filter(|p| p.exists())
                .collect();
            if convert && !paths.is_empty() {
                if let Some(shared) = app.try_state::<Shared>() {
                    shared.engine.convert_add(&paths, true);
                }
            } else if !paths.is_empty()
                && let Some(shared) = app.try_state::<Shared>()
            {
                let summary = shared.engine.open_paths(&paths, true);
                shared
                    .engine
                    .spawn_analysis(summary.ids, notifier(app.handle()));
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Drag-and-drop runs here, in Rust: paths are registered and the webview only ever
            // sees opaque ids (PLAN 8.6.4).
            if let WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. }) = event {
                let app = window.app_handle().clone();
                let paths = paths.clone();
                if let Some(shared) = app.try_state::<Shared>() {
                    if shared.convert_mode.load(Ordering::SeqCst) {
                        // The Convert screen is showing: the drop goes to its list.
                        let engine = shared.engine.clone();
                        std::thread::spawn(move || {
                            let added = engine.convert_add(&paths, true);
                            let _ = app.emit("convert-added", added);
                        });
                        return;
                    }
                    let summary = shared.engine.open_paths(&paths, true);
                    shared
                        .engine
                        .spawn_analysis(summary.ids.clone(), notifier(&app));
                    let _ = app.emit("items-added", summary);
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Auto Crop");
}
