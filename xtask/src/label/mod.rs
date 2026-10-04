// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `cargo xtask label <image dir>`: the blank-quad labeller (ROADMAP M1.41, adapted).
//!
//! The first plan wanted Label Studio or a `tools/labeler/` with npm. This is neither: one
//! self-contained HTML page (`page.html`, vanilla JS, no CDN, no npm, no network) served by a tiny
//! std-only server that binds **127.0.0.1 only**, puts a random per-run token in the URL, sends
//! `Cache-Control: no-store` and `X-Content-Type-Options: nosniff` on every reply and rejects any
//! `Host` or `Origin` that is not this server. Images are decoded by the repository's own codecs
//! (EXIF orientation applied, so labels are in the same oriented space as the harness) and only
//! re-encoded, downscaled previews and tiles ever reach the browser; the server never serves a
//! file by path (requests name an image by its index in the list built at start-up).
//!
//! The labeller shows **no model output** unless the labeller explicitly asks for it (a separate,
//! off-by-default toggle); the server then records the fact on its own (`assisted: true` in the
//! label file and in `_assist-log.jsonl`), so a client bug cannot hide it. Labels are saved one
//! JSON file per image in the labels folder, together with a per-image labelling-time log
//! (`_labelling-log.jsonl`) and a list of skipped images (`_state.json`).

mod http;
mod images;

use crate::golden::common::{Args, ensure_private, utc_now, write_atomic};
use auto_crop_eval::golden::{self, GoldenItem, GoldenLabel};
use http::{Request, Response};
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeSet;
use std::io::Write as _;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const PAGE: &str = include_str!("page.html");
/// Largest time span one save may add to the labelling time (an hour; anything more is a forgotten tab).
const MAX_SECONDS_PER_SAVE: f64 = 3600.0;
const MAX_ITEMS: usize = 64;

#[derive(Debug, Clone)]
pub struct Config {
    pub images_dir: PathBuf,
    pub labels_dir: PathBuf,
    pub annotator: Option<String>,
    pub suggestions: bool,
    pub port: u16,
}

#[derive(Default)]
struct Persist {
    skipped: BTreeSet<String>,
    assisted: BTreeSet<String>,
}

pub struct Shared {
    token: String,
    port: u16,
    images: Vec<String>,
    /// Files with an image extension this build cannot decode (shown at start-up only).
    pub undecodable: Vec<String>,
    cfg: Config,
    cache: images::Cache,
    persist: Mutex<Persist>,
    /// Serialises writes of labels, logs and state.
    io: Mutex<()>,
}

fn random_hex(bytes: usize) -> Result<String, String> {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).map_err(|e| format!("no OS randomness: {e}"))?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        diff |= usize::from(a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(1));
    }
    diff == 0
}

#[derive(Deserialize)]
struct SavePayload {
    slices: Vec<String>,
    items: Vec<GoldenItem>,
    #[serde(default)]
    scene_id: Option<String>,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    seconds: Option<f64>,
}

#[derive(Deserialize)]
struct SkipPayload {
    skipped: bool,
    #[serde(default)]
    seconds: Option<f64>,
}

fn bad(status: u16, msg: &str) -> Response {
    Response::json(status, &json!({ "error": msg }))
}

impl Shared {
    pub fn new(cfg: Config) -> Result<Self, String> {
        let (mut images, mut undecodable) = (Vec::new(), Vec::new());
        let supported = auto_crop_codecs::supported_input_extensions();
        for name in golden::list_images(&cfg.images_dir)? {
            let ext = Path::new(&name)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            if supported.contains(&ext.as_str()) {
                images.push(name);
            } else {
                undecodable.push(name);
            }
        }
        ensure_private(&cfg.labels_dir)?;
        std::fs::create_dir_all(&cfg.labels_dir)
            .map_err(|e| format!("cannot create {}: {e}", cfg.labels_dir.display()))?;
        let mut persist = Persist::default();
        if let Ok(t) = std::fs::read_to_string(cfg.labels_dir.join("_state.json"))
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(&t)
        {
            for s in v["skipped"].as_array().into_iter().flatten() {
                if let Some(s) = s.as_str() {
                    persist.skipped.insert(s.to_owned());
                }
            }
        }
        if let Ok(t) = std::fs::read_to_string(cfg.labels_dir.join("_assist-log.jsonl")) {
            for line in t.lines() {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(line)
                    && let Some(id) = v["id"].as_str()
                {
                    persist.assisted.insert(id.to_owned());
                }
            }
        }
        Ok(Self {
            token: random_hex(16)?,
            port: cfg.port,
            images,
            undecodable,
            cfg,
            cache: images::Cache::default(),
            persist: Mutex::new(persist),
            io: Mutex::new(()),
        })
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    fn label_path(&self, name: &str) -> PathBuf {
        self.cfg.labels_dir.join(golden::label_file_name(name))
    }

    fn read_label(&self, name: &str) -> Option<GoldenLabel> {
        let t = std::fs::read_to_string(self.label_path(name)).ok()?;
        golden::parse_label(&t).ok()
    }

    fn append_log(&self, file: &str, v: &serde_json::Value) {
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.cfg.labels_dir.join(file))
        {
            let _ = writeln!(f, "{v}");
        }
    }

    fn host_ok(&self, host: Option<&str>) -> bool {
        host.is_some_and(|h| {
            h == format!("127.0.0.1:{}", self.port) || h == format!("localhost:{}", self.port)
        })
    }

    fn origin_ok(&self, origin: &str) -> bool {
        origin == format!("http://127.0.0.1:{}", self.port)
            || origin == format!("http://localhost:{}", self.port)
    }

    fn index(&self, seg: &str) -> Option<usize> {
        (!seg.is_empty() && seg.len() <= 7 && seg.bytes().all(|b| b.is_ascii_digit()))
            .then(|| seg.parse::<usize>().ok())
            .flatten()
            .filter(|i| *i < self.images.len())
    }

    /// Everything the server does, as a function of the request (the sockets only move bytes).
    pub fn handle(&self, req: &Request) -> Response {
        // 1. Only this server's own name is accepted as Host (DNS rebinding).
        if !self.host_ok(req.header("host")) {
            return Response::text(403, "bad host");
        }
        if req.path == "/favicon.ico" {
            return Response::new(204, "text/plain", Vec::new());
        }
        // 2. Cross-site requests are refused by their own headers.
        if let Some(o) = req.header("origin")
            && !self.origin_ok(o)
        {
            return Response::text(403, "bad origin");
        }
        if req.header("sec-fetch-site") == Some("cross-site") {
            return Response::text(403, "cross-site request");
        }
        // 3. The per-run token.
        if !req.param("t").is_some_and(|t| ct_eq(t, &self.token)) {
            return Response::text(403, "missing or wrong token");
        }
        match req.method.as_str() {
            "GET" => self.get(req),
            "POST" => {
                if !req
                    .header("content-type")
                    .is_some_and(|c| c.starts_with("application/json"))
                {
                    return bad(415, "JSON only");
                }
                if req.header("origin").is_none()
                    && !matches!(req.header("sec-fetch-site"), Some("same-origin" | "none"))
                {
                    return Response::text(403, "a POST needs an Origin header");
                }
                self.post(req)
            }
            _ => bad(405, "GET and POST only"),
        }
    }

    fn get(&self, req: &Request) -> Response {
        let segs: Vec<&str> = req.path.trim_matches('/').split('/').collect();
        match segs.as_slice() {
            [""] => self.page(),
            ["api", "list"] => self.list(),
            ["api", "meta", n] => self
                .index(n)
                .map_or_else(|| bad(404, "no such image"), |i| self.meta(i)),
            ["api", "preview", n] => self
                .index(n)
                .map_or_else(|| bad(404, "no such image"), |i| self.preview(i)),
            ["api", "tile", n] => self
                .index(n)
                .map_or_else(|| bad(404, "no such image"), |i| self.tile(i, req)),
            ["api", "suggest", n] => self
                .index(n)
                .map_or_else(|| bad(404, "no such image"), |i| self.suggest(i)),
            _ => bad(404, "not found"),
        }
    }

    fn post(&self, req: &Request) -> Response {
        let segs: Vec<&str> = req.path.trim_matches('/').split('/').collect();
        match segs.as_slice() {
            ["api", "save", n] => self
                .index(n)
                .map_or_else(|| bad(404, "no such image"), |i| self.save(i, &req.body)),
            ["api", "skip", n] => self
                .index(n)
                .map_or_else(|| bad(404, "no such image"), |i| self.skip(i, &req.body)),
            _ => bad(404, "not found"),
        }
    }

    fn page(&self) -> Response {
        let nonce = random_hex(12).unwrap_or_default();
        let html = PAGE.replace("{{NONCE}}", &nonce);
        Response::new(200, "text/html; charset=utf-8", html.into_bytes()).with(
            "Content-Security-Policy",
            format!(
                "default-src 'none'; script-src 'nonce-{nonce}'; style-src 'nonce-{nonce}'; img-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'"
            ),
        )
    }

    fn list(&self) -> Response {
        let persist = self
            .persist
            .lock()
            .map(|p| p.skipped.clone())
            .unwrap_or_default();
        let rows: Vec<_> = self
            .images
            .iter()
            .map(|name| {
                let label = self.read_label(name);
                let state = if label.is_some() {
                    "labelled"
                } else if persist.contains(name) {
                    "skipped"
                } else {
                    "todo"
                };
                json!({
                    "name": name,
                    "state": state,
                    "items": label.as_ref().map_or(0, |l| l.items.len()),
                    "assisted": label.as_ref().is_some_and(|l| l.assisted),
                })
            })
            .collect();
        Response::json(
            200,
            &json!({ "images": rows, "suggestions": self.cfg.suggestions }),
        )
    }

    fn assisted_flag(&self, name: &str) -> bool {
        self.persist.lock().is_ok_and(|p| p.assisted.contains(name))
            || self.read_label(name).is_some_and(|l| l.assisted)
    }

    fn meta(&self, i: usize) -> Response {
        let name = &self.images[i];
        let loaded = match self.cache.get(i, &self.cfg.images_dir.join(name)) {
            Ok(l) => l,
            Err(e) => return bad(422, &e),
        };
        let label = self.read_label(name);
        Response::json(
            200,
            &json!({
                "name": name,
                "width": loaded.raster.width,
                "height": loaded.raster.height,
                "format": loaded.format,
                "preview_w": loaded.preview_width(),
                "label": label,
                "skipped": self.persist.lock().is_ok_and(|p| p.skipped.contains(name)),
                "assisted": self.assisted_flag(name),
                "suggestions": self.cfg.suggestions,
            }),
        )
    }

    fn preview(&self, i: usize) -> Response {
        match self
            .cache
            .get(i, &self.cfg.images_dir.join(&self.images[i]))
            .and_then(|l| l.preview())
        {
            Ok((bytes, _)) => {
                let ct = if bytes.starts_with(&[0xff, 0xd8]) {
                    "image/jpeg"
                } else {
                    "image/png"
                };
                Response::new(200, ct, bytes.to_vec())
            }
            Err(e) => bad(422, &e),
        }
    }

    fn tile(&self, i: usize, req: &Request) -> Response {
        let num = |k: &str| req.param(k).and_then(|v| v.parse::<f64>().ok());
        let (Some(x), Some(y), Some(w), Some(h)) = (num("x"), num("y"), num("w"), num("h")) else {
            return bad(400, "x, y, w and h are fractions of the image");
        };
        let out = req
            .param("out")
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(1200);
        let loaded = match self
            .cache
            .get(i, &self.cfg.images_dir.join(&self.images[i]))
        {
            Ok(l) => l,
            Err(e) => return bad(422, &e),
        };
        let Some(t) = images::tile(&loaded.raster, x, y, w, h, out) else {
            return bad(400, "empty or invalid region");
        };
        match images::encode_preview(&t, images::TILE_EDGE_MAX) {
            Ok((bytes, ct)) => Response::new(200, ct, bytes),
            Err(e) => bad(500, &e),
        }
    }

    /// The optional detector suggestion. Asking for it marks the image `assisted` for good.
    fn suggest(&self, i: usize) -> Response {
        if !self.cfg.suggestions {
            return bad(404, "suggestions are disabled (--no-suggestions)");
        }
        let name = self.images[i].clone();
        let loaded = match self.cache.get(i, &self.cfg.images_dir.join(&name)) {
            Ok(l) => l,
            Err(e) => return bad(422, &e),
        };
        {
            let _io = self.io.lock();
            if let Ok(mut p) = self.persist.lock()
                && p.assisted.insert(name.clone())
            {
                self.append_log("_assist-log.jsonl", &json!({ "at": utc_now(), "id": name }));
            }
            // A label saved earlier is retroactively assisted: the owner has now seen the model.
            if let Some(mut l) = self.read_label(&name)
                && !l.assisted
            {
                l.assisted = true;
                let _ = write_atomic(
                    &self.label_path(&name),
                    golden::label_to_json(&l).as_bytes(),
                );
            }
        }
        let det = auto_crop_imgproc::detect::detect(&loaded.raster);
        let quad = det
            .quad
            .map(|q| q.iter().map(|p| [p.x, p.y]).collect::<Vec<_>>());
        Response::json(
            200,
            &json!({ "quad": quad, "confidence": f64::from(det.confidence.score) }),
        )
    }

    fn save(&self, i: usize, body: &[u8]) -> Response {
        let name = &self.images[i];
        let payload: SavePayload = match serde_json::from_slice(body) {
            Ok(p) => p,
            Err(e) => return bad(400, &format!("bad label payload: {e}")),
        };
        if payload.items.len() > MAX_ITEMS {
            return bad(400, "too many items");
        }
        let loaded = match self.cache.get(i, &self.cfg.images_dir.join(name)) {
            Ok(l) => l,
            Err(e) => return bad(422, &e),
        };
        let _io = self.io.lock();
        let prev = self.read_label(name);
        let mut l = golden::new_label(
            name,
            &loaded.sha256,
            loaded.raster.width,
            loaded.raster.height,
        );
        l.slices = payload.slices;
        l.items = payload.items;
        l.scene_id = payload
            .scene_id
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .or_else(|| prev.as_ref().map(|p| p.scene_id.clone()))
            .unwrap_or_else(|| golden::default_scene(name));
        l.notes = payload
            .notes
            .map(|n| n.trim().to_owned())
            .filter(|n| !n.is_empty());
        l.orientation_quarter_turns = l.items.first().map_or(0, |it| {
            golden::quarter_turns_for(&it.quad, l.width, l.height)
        });
        l.assisted = self.assisted_flag(name);
        l.annotator = self.cfg.annotator.clone();
        let add = payload
            .seconds
            .filter(|s| s.is_finite() && *s > 0.0)
            .map_or(0.0, |s| s.min(MAX_SECONDS_PER_SAVE));
        l.labelling_seconds = Some(
            prev.as_ref()
                .and_then(|p| p.labelling_seconds)
                .unwrap_or(0.0)
                + add,
        );
        l.labelled_at = Some(utc_now());
        l.noise_floor_double_labelled = prev.as_ref().and_then(|p| p.noise_floor_double_labelled);
        let found = golden::validate_label(&l, Some(name));
        if !found.errors.is_empty() {
            return Response::json(422, &json!({ "errors": found.errors }));
        }
        if let Err(e) = write_atomic(&self.label_path(name), golden::label_to_json(&l).as_bytes()) {
            return bad(500, &e);
        }
        if let Ok(mut p) = self.persist.lock() {
            p.skipped.remove(name);
        }
        self.append_log(
            "_labelling-log.jsonl",
            &json!({ "at": utc_now(), "id": name, "event": "save", "seconds": add,
                     "items": l.items.len(), "assisted": l.assisted }),
        );
        Response::json(
            200,
            &json!({ "ok": true, "label": l, "warnings": found.warnings }),
        )
    }

    fn skip(&self, i: usize, body: &[u8]) -> Response {
        let name = &self.images[i];
        let payload: SkipPayload = match serde_json::from_slice(body) {
            Ok(p) => p,
            Err(e) => return bad(400, &format!("bad payload: {e}")),
        };
        let _io = self.io.lock();
        let Ok(mut p) = self.persist.lock() else {
            return bad(500, "state lock poisoned");
        };
        if payload.skipped {
            p.skipped.insert(name.clone());
        } else {
            p.skipped.remove(name);
        }
        let state = json!({ "skipped": p.skipped.iter().collect::<Vec<_>>() });
        drop(p);
        if let Err(e) = write_atomic(
            &self.cfg.labels_dir.join("_state.json"),
            format!("{state:#}\n").as_bytes(),
        ) {
            return bad(500, &e);
        }
        let seconds = payload
            .seconds
            .filter(|s| s.is_finite() && *s > 0.0)
            .map_or(0.0, |s| s.min(MAX_SECONDS_PER_SAVE));
        self.append_log(
            "_labelling-log.jsonl",
            &json!({ "at": utc_now(), "id": name, "event": if payload.skipped { "skip" } else { "unskip" },
                     "seconds": seconds }),
        );
        Response::json(200, &json!({ "ok": true }))
    }
}

/// Binds 127.0.0.1 (never another interface) and returns the listener with the shared state; the
/// port in `cfg` may be 0 for "any free port", in which case the state learns the real one.
pub fn bind(mut cfg: Config) -> Result<(Arc<Shared>, TcpListener), String> {
    let listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), cfg.port))
        .map_err(|e| format!("cannot listen on 127.0.0.1:{}: {e}", cfg.port))?;
    cfg.port = listener.local_addr().map_err(|e| e.to_string())?.port();
    Ok((Arc::new(Shared::new(cfg)?), listener))
}

fn serve_one(shared: &Shared, mut stream: std::net::TcpStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(15)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
    if !stream.peer_addr().is_ok_and(|a| a.ip().is_loopback()) {
        return;
    }
    let resp = match http::read_request(&mut stream) {
        Ok(req) => shared.handle(&req),
        Err((status, msg)) => Response::text(status, msg),
    };
    let _ = http::write_response(&mut stream, &resp);
}

/// Accepts connections forever, one thread each.
pub fn serve(shared: Arc<Shared>, listener: TcpListener) {
    for stream in listener.incoming().flatten() {
        let sh = Arc::clone(&shared);
        std::thread::spawn(move || serve_one(&sh, stream));
    }
}

fn open_browser(url: &str) -> Result<(), String> {
    let status = if cfg!(target_os = "windows") {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .status()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).status()
    } else {
        std::process::Command::new("xdg-open").arg(url).status()
    };
    status
        .map_err(|e| format!("cannot open the browser: {e}"))
        .and_then(|s| {
            s.success()
                .then_some(())
                .ok_or_else(|| "the browser opener failed".to_owned())
        })
}

/// `cargo xtask label <image dir> [--labels DIR] [--open] [--port N] [--annotator NAME] [--no-suggestions]`.
pub fn run(args: &[String]) -> Result<(), String> {
    let a = Args::new(args);
    let opts = ["--labels", "--port", "--annotator"];
    a.reject_unknown(&["--open", "--no-suggestions"], &opts)?;
    let pos = a.positionals(&opts);
    let [dir] = pos.as_slice() else {
        return Err(
            "usage: label <image dir> [--labels DIR] [--open] [--port N] [--annotator NAME] [--no-suggestions]"
                .to_owned(),
        );
    };
    let images_dir = PathBuf::from(dir);
    if !images_dir.is_dir() {
        return Err(format!("{dir} is not a folder"));
    }
    let labels_dir = a.value("--labels")?.map_or_else(
        || {
            // Default: <image dir>/golden/labels when the images are the data folder itself.
            images_dir.join("golden").join("labels")
        },
        PathBuf::from,
    );
    let port = a
        .value("--port")?
        .map(|p| p.parse::<u16>().map_err(|_| format!("bad --port {p}")))
        .transpose()?
        .unwrap_or(0);
    let (shared, listener) = bind(Config {
        images_dir: images_dir.clone(),
        labels_dir: labels_dir.clone(),
        annotator: a.value("--annotator")?,
        suggestions: !a.flag("--no-suggestions"),
        port,
    })?;
    let url = format!(
        "http://127.0.0.1:{}/?t={}",
        listener.local_addr().map_err(|e| e.to_string())?.port(),
        shared.token()
    );
    println!(
        "labelling {} image(s) from {}, labels go to {}",
        shared.images.len(),
        images_dir.display(),
        labels_dir.display()
    );
    if !shared.undecodable.is_empty() {
        println!(
            "{} file(s) skipped because this build cannot decode them (HEIC/AVIF need `cargo run -p xtask --features heif -- label ...` after `cargo xtask build-native`): {}",
            shared.undecodable.len(),
            shared
                .undecodable
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    println!(
        "open this address in a browser on this machine (it contains this run's secret token):\n\n  {url}\n"
    );
    println!(
        "it listens on 127.0.0.1 only, loads nothing from the internet and sends nothing out; Ctrl+C stops it"
    );
    if a.flag("--open") {
        open_browser(&url)?;
    }
    serve(shared, listener);
    Ok(())
}

#[cfg(test)]
mod tests;
