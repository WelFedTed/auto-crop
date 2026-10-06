// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The Tauri side: commands, the `acimg` scheme, pickers and drag-and-drop.

use auto_crop_engine::{
    AppPaths, Cut, DerivedAction, Edit, Engine, ErrKind, ItemView, Notify, OpenSummary, Pt,
    RestoreMode, RestoreOutcome, RevertTo, SaveOutcome, SaveTarget, SessionStep, Settings,
    SplitPatch,
};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
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

fn image_response(status: StatusCode, mime: &str, body: Vec<u8>) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header("Content-Type", mime)
        .header("Cache-Control", "no-store")
        .header("X-Content-Type-Options", "nosniff")
        .body(body)
        .expect("static response parts are valid")
}

/// Answers one `acimg` request: the whole image or one crop of it, or a bare 404 for anything
/// that is not exactly one of those.
fn serve_image(shared: &Shared, path: &str) -> Response<Vec<u8>> {
    let empty = |status| image_response(status, "text/plain", Vec::new());
    let Some(image) = crate::parse_image_path(path, &shared.token) else {
        return empty(StatusCode::NOT_FOUND);
    };
    let result = match image {
        crate::ImagePath::Whole(id, kind) => shared.engine.image_bytes(id, kind),
        crate::ImagePath::Crop(id, crop, kind) => shared.engine.crop_image_bytes(id, crop, kind),
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
    };
    let for_scheme = shared.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(shared)
        .register_asynchronous_uri_scheme_protocol("acimg", move |_ctx, request, responder| {
            let shared = for_scheme.clone();
            let path = request.uri().path().to_owned();
            // Rendering can take tens of milliseconds; never block the webview's thread.
            std::thread::spawn(move || {
                let response = serve_image(&shared, &path);
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
        ])
        .on_window_event(|window, event| {
            // Drag-and-drop runs here, in Rust: paths are registered and the webview only ever
            // sees opaque ids (PLAN 8.6.4).
            if let WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. }) = event {
                let app = window.app_handle().clone();
                let paths = paths.clone();
                if let Some(shared) = app.try_state::<Shared>() {
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
