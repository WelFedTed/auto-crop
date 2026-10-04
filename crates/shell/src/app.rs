// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The Tauri side: commands, the `acimg` scheme, pickers and drag-and-drop.

use auto_crop_engine::{
    AppPaths, Edit, Engine, ErrKind, ItemView, Notify, OpenSummary, RestoreMode, RestoreOutcome,
    SaveOutcome, SaveTarget, Settings,
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
    }
}

#[tauri::command]
async fn pick_files(app: AppHandle, shared: State<'_, Shared>) -> Cmd<OpenSummary> {
    let picked = app
        .dialog()
        .file()
        .add_filter("Images", &["jpg", "jpeg", "png"])
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
                let response = match crate::parse_image_path(&path, &shared.token) {
                    None => image_response(StatusCode::NOT_FOUND, "text/plain", Vec::new()),
                    Some((id, kind)) => match shared.engine.image_bytes(id, kind) {
                        Ok((bytes, mime)) if bytes.len() <= MAX_BODY => {
                            image_response(StatusCode::OK, mime, bytes.as_ref().clone())
                        }
                        Ok(_) => {
                            image_response(StatusCode::PAYLOAD_TOO_LARGE, "text/plain", Vec::new())
                        }
                        Err(ErrKind::NoCrop) => {
                            image_response(StatusCode::NOT_FOUND, "text/plain", Vec::new())
                        }
                        Err(_) => image_response(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "text/plain",
                            Vec::new(),
                        ),
                    },
                };
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
