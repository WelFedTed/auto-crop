// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Throwaway GUI-stack spike (ROADMAP M0.57): Rust serves a synthetic display proxy through the
//! `acimg` custom scheme under a launch token (`no-store`, `nosniff`, size cap, strict parser,
//! never a filesystem path); the webview draws the view, handles and frame HUD.

#![cfg_attr(all(not(debug_assertions), windows), windows_subsystem = "windows")]

use std::sync::Arc;

use serde::Serialize;
use tauri::State;
use tauri::http::{Request, Response, StatusCode};

const SCENE_W: u32 = 1600;
const SCENE_H: u32 = 1200;
/// Tile responses above this size are refused (PLAN 8.6.4: 8 MB cap).
const MAX_BODY: usize = 8 * 1024 * 1024;

struct Launch {
    token: String,
    scene_png: Arc<Vec<u8>>,
}

#[derive(Serialize)]
struct LaunchInfo {
    token: String,
    width: u32,
    height: u32,
}

#[tauri::command]
fn launch_info(state: State<'_, Launch>) -> LaunchInfo {
    LaunchInfo { token: state.token.clone(), width: SCENE_W, height: SCENE_H }
}

fn random_token() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("OS random source");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Draws a desk with a slightly rotated receipt: enough structure to judge crop handles and a warp.
fn synthetic_scene() -> Vec<u8> {
    let (w, h) = (SCENE_W as usize, SCENE_H as usize);
    let mut rgb = vec![0u8; w * h * 3];
    let (cx, cy) = (w as f32 * 0.5, h as f32 * 0.5);
    let (half_w, half_h) = (230.0f32, 470.0f32);
    let (sin, cos) = (5.0f32.to_radians().sin(), 5.0f32.to_radians().cos());
    for y in 0..h {
        for x in 0..w {
            let t = (x + y) as f32 / (w + h) as f32;
            let mut px = [
                (138.0 - 44.0 * t) as u8,
                (121.0 - 40.0 * t) as u8,
                (104.0 - 36.0 * t) as u8,
            ];
            // Rotate into receipt space.
            let (dx, dy) = (x as f32 - cx, y as f32 - cy);
            let (u, v) = (dx * cos + dy * sin, -dx * sin + dy * cos);
            if u.abs() < half_w && v.abs() < half_h {
                px = [246, 244, 238];
                let row = ((v + half_h) as i32) % 22;
                let inside = u.abs() < half_w - 36.0 && v > -half_h + 40.0 && v < half_h - 60.0;
                if inside && (0..7).contains(&row) {
                    let len = 120.0 + ((((v + half_h) as i32) / 22 * 37) % 120) as f32;
                    if u > -half_w + 36.0 && u < -half_w + 36.0 + len {
                        px = [92, 98, 112];
                    }
                }
            }
            let i = (y * w + x) * 3;
            rgb[i..i + 3].copy_from_slice(&px);
        }
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, SCENE_W, SCENE_H);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let mut writer = enc.write_header().expect("png header");
        writer.write_image_data(&rgb).expect("png data");
    }
    out
}

fn reply(status: StatusCode, content_type: &str, body: Vec<u8>) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header("Content-Type", content_type)
        .header("Cache-Control", "no-store")
        .header("X-Content-Type-Options", "nosniff")
        .body(body)
        .expect("static response parts are valid")
}

/// `acimg://localhost/<launch-token>/<item-id>/<level>` (`http://acimg.localhost/...` on Windows).
/// Anything that does not match exactly is a 404; no URL is ever mapped to a filesystem path.
fn serve(launch: &Launch, request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let not_found = || reply(StatusCode::NOT_FOUND, "text/plain", b"not found".to_vec());
    let path = request.uri().path().trim_start_matches('/');
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() != 3 || parts[0] != launch.token {
        return not_found();
    }
    let numeric = |s: &str| !s.is_empty() && s.len() <= 4 && s.bytes().all(|b| b.is_ascii_digit());
    if parts[1] != "0" || !numeric(parts[2]) || parts[2] != "0" {
        return not_found();
    }
    if launch.scene_png.len() > MAX_BODY {
        return reply(StatusCode::PAYLOAD_TOO_LARGE, "text/plain", Vec::new());
    }
    reply(StatusCode::OK, "image/png", launch.scene_png.as_ref().clone())
}

fn main() {
    let launch = Arc::new(Launch { token: random_token(), scene_png: Arc::new(synthetic_scene()) });
    let for_scheme = Arc::clone(&launch);
    tauri::Builder::default()
        .manage(Launch { token: launch.token.clone(), scene_png: Arc::clone(&launch.scene_png) })
        .register_uri_scheme_protocol("acimg", move |_ctx, request| serve(&for_scheme, &request))
        .invoke_handler(tauri::generate_handler![launch_info])
        .run(tauri::generate_context!())
        .expect("error while running the GUI spike");
}
