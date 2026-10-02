// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Shared fixtures of the corpus tests (ROADMAP M1.36-M1.38): a scratch directory, a tar and a
//! zip writer, a loopback HTTP server that can serve bad downloads, and small SYNTHETIC trees
//! that mimic the documented layout of each dataset. No real dataset and no real host is ever
//! involved: the fixtures are generated here, byte for byte, from `auto-crop-codecs` fixtures.
#![allow(dead_code)]

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A scratch directory removed on drop.
pub struct Tmp(pub PathBuf);

impl Tmp {
    pub fn new(tag: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let d =
            std::env::temp_dir().join(format!("auto-crop-corpus-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Self(d)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn join(&self, p: &str) -> PathBuf {
        self.0.join(p)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// Runs `xtask` in `cwd` (a scratch directory, so the repo-root guard sees no repository) with
/// the corpus cache in `cache`.
pub fn xtask(cwd: &Path, cache: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
        .current_dir(cwd)
        .env("AUTOCROP_CORPUS_CACHE", cache)
        .args(args)
        .output()
        .expect("run xtask")
}

pub fn text(o: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

// ---------------------------------------------------------------------------------- archives

/// A ustar archive of `(name, bytes)` members.
pub fn tar(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, data) in entries {
        assert!(name.len() <= 100, "fixture name too long: {name}");
        let mut h = [0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        h[100..108].copy_from_slice(b"0000644\0");
        h[108..116].copy_from_slice(b"0000000\0");
        h[116..124].copy_from_slice(b"0000000\0");
        h[124..136].copy_from_slice(format!("{:011o}\0", data.len()).as_bytes());
        h[136..148].copy_from_slice(b"00000000000\0");
        h[148..156].copy_from_slice(b"        ");
        h[156] = b'0';
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        let sum: u32 = h.iter().map(|&b| u32::from(b)).sum();
        h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        out.extend_from_slice(&h);
        out.extend_from_slice(data);
        out.resize(out.len().div_ceil(512) * 512, 0);
    }
    out.extend_from_slice(&[0u8; 1024]);
    out
}

/// A zip archive with stored (uncompressed) members.
pub fn zip(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    fn le32(v: &mut Vec<u8>, x: u32) {
        v.extend_from_slice(&x.to_le_bytes());
    }
    fn le16(v: &mut Vec<u8>, x: u16) {
        v.extend_from_slice(&x.to_le_bytes());
    }
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let crc = auto_crop_codecs::fixtures::crc32(data);
        let offset = out.len() as u32;
        le32(&mut out, 0x0403_4b50);
        le16(&mut out, 20);
        le16(&mut out, 0);
        le16(&mut out, 0);
        le16(&mut out, 0);
        le16(&mut out, 0x21);
        le32(&mut out, crc);
        le32(&mut out, data.len() as u32);
        le32(&mut out, data.len() as u32);
        le16(&mut out, name.len() as u16);
        le16(&mut out, 0);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);

        le32(&mut central, 0x0201_4b50);
        le16(&mut central, 20);
        le16(&mut central, 20);
        le16(&mut central, 0);
        le16(&mut central, 0);
        le16(&mut central, 0);
        le16(&mut central, 0x21);
        le32(&mut central, crc);
        le32(&mut central, data.len() as u32);
        le32(&mut central, data.len() as u32);
        le16(&mut central, name.len() as u16);
        le16(&mut central, 0);
        le16(&mut central, 0);
        le16(&mut central, 0);
        le16(&mut central, 0);
        le32(&mut central, 0);
        le32(&mut central, offset);
        central.extend_from_slice(name.as_bytes());
    }
    let cd_offset = out.len() as u32;
    out.extend_from_slice(&central);
    le32(&mut out, 0x0605_4b50);
    le16(&mut out, 0);
    le16(&mut out, 0);
    le16(&mut out, entries.len() as u16);
    le16(&mut out, entries.len() as u16);
    le32(&mut out, central.len() as u32);
    le32(&mut out, cd_offset);
    le16(&mut out, 0);
    out
}

/// True when this machine can extract zip archives the way `xtask` does.
pub fn can_unzip() -> bool {
    if cfg!(windows) || cfg!(target_os = "macos") {
        return true;
    }
    Command::new("unzip").arg("-v").output().is_ok()
}

// ---------------------------------------------------------------------------- HTTP fixture

pub enum Route {
    /// 200 with the bytes.
    Bytes(Vec<u8>),
    /// Promises `claimed` bytes, sends `body`, closes the connection.
    Truncated { claimed: usize, body: Vec<u8> },
    /// 302 to the URL.
    Redirect(String),
    /// A bare status.
    Status(u16),
}

/// A loopback HTTP server on an ephemeral port. Records every request path.
pub struct Server {
    pub port: u16,
    hits: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Server {
    pub fn start(routes: Vec<(&str, Route)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let routes: Arc<HashMap<String, Route>> =
            Arc::new(routes.into_iter().map(|(p, r)| (p.to_owned(), r)).collect());
        let hits = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (h2, s2) = (hits.clone(), stop.clone());
        let handle = std::thread::spawn(move || {
            while !s2.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((conn, _)) => {
                        let (routes, hits) = (routes.clone(), h2.clone());
                        std::thread::spawn(move || serve(conn, &routes, &hits));
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        Self {
            port,
            hits,
            stop,
            handle: Some(handle),
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    pub fn hits(&self) -> Vec<String> {
        self.hits.lock().unwrap().clone()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn serve(mut conn: TcpStream, routes: &HashMap<String, Route>, hits: &Mutex<Vec<String>>) {
    let _ = conn.set_nonblocking(false);
    let _ = conn.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        match conn.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    let head = String::from_utf8_lossy(&buf).into_owned();
    let path = head.split_whitespace().nth(1).unwrap_or("/").to_owned();
    hits.lock().unwrap().push(path.clone());
    let write_head = |c: &mut TcpStream, status: &str, extra: &str, len: usize| {
        let _ = c.write_all(
            format!(
                "HTTP/1.1 {status}\r\nContent-Length: {len}\r\nConnection: close\r\n{extra}\r\n"
            )
            .as_bytes(),
        );
    };
    match routes.get(&path) {
        Some(Route::Bytes(b)) => {
            write_head(
                &mut conn,
                "200 OK",
                "Content-Type: application/octet-stream\r\n",
                b.len(),
            );
            let _ = conn.write_all(b);
        }
        Some(Route::Truncated { claimed, body }) => {
            write_head(&mut conn, "200 OK", "", *claimed);
            let _ = conn.write_all(body);
        }
        Some(Route::Redirect(to)) => {
            write_head(&mut conn, "302 Found", &format!("Location: {to}\r\n"), 0);
        }
        Some(Route::Status(s)) => write_head(&mut conn, &format!("{s} Err"), "", 0),
        None => write_head(&mut conn, "404 Not Found", "", 0),
    }
    let _ = conn.flush();
    let _ = conn.shutdown(Shutdown::Both);
}

// ------------------------------------------------------------------------------ lock files

/// One `[[corpus]]` entry with a single file.
pub struct LockSpec<'a> {
    pub name: &'a str,
    pub adapter: &'a str,
    pub spdx: &'a str,
    pub variant: &'a str,
    pub url: &'a str,
    pub size: String,
    pub sha256: String,
    pub extract: &'a str,
}

pub fn lock_text(specs: &[LockSpec]) -> String {
    let mut t = String::from("version = 1\n");
    for s in specs {
        let size = if s.size.chars().all(|c| c.is_ascii_digit()) {
            s.size.clone()
        } else {
            format!("\"{}\"", s.size)
        };
        t.push_str(&format!(
            "\n[[corpus]]\nname = \"{}\"\nadapter = \"{}\"\nspdx = \"{}\"\nlicence_url = \"https://example.org/licence\"\n\
             attribution = \"Fixture attribution: cite the fixture paper\"\n\
             [[corpus.file]]\nvariant = \"{}\"\nurl = \"{}\"\nsize = {size}\nsha256 = \"{}\"\nextract = \"{}\"\n",
            s.name, s.adapter, s.spdx, s.variant, s.url, s.sha256, s.extract
        ));
    }
    t
}

/// A lock with one corpus per adapter, all structurally valid, for ingest-only tests.
pub fn ingest_lock(midv_spdx: &str) -> String {
    let mk = |name: &'static str, spdx: &'static str| LockSpec {
        name,
        adapter: name,
        spdx,
        variant: "full",
        url: "https://example.org/never-fetched.tar",
        size: "TODO-first-fetch".to_owned(),
        sha256: "TODO-first-fetch".to_owned(),
        extract: "tar",
    };
    let mut midv = mk("midv-500", "CC-BY-4.0");
    midv.spdx = Box::leak(midv_spdx.to_owned().into_boxed_str());
    lock_text(&[
        mk("smartdoc2015-ch1", "CC-BY-4.0"),
        mk("cord", "CC-BY-4.0"),
        midv,
        mk("dibco", "CC0-1.0"),
        mk("rawpixls-cc0", "CC0-1.0"),
    ])
}

// ----------------------------------------------------------------- synthetic dataset trees

pub fn jpeg(w: u32, h: u32) -> Vec<u8> {
    auto_crop_codecs::fixtures::jpeg_baseline(w, h)
}

pub fn png(w: u32, h: u32) -> Vec<u8> {
    auto_crop_codecs::fixtures::png_rgb(w, h)
}

pub fn tiff(w: u32, h: u32) -> Vec<u8> {
    auto_crop_codecs::fixtures::tiff_rgb8(w, h, &auto_crop_codecs::fixtures::TiffOpts::default())
}

pub const FRAME_W: u32 = 64;
pub const FRAME_H: u32 = 48;

/// Mimics the assumed SmartDoc 2015 Ch.1 layout: `metadata.csv` and
/// `frames/<background>/<model>/<frame>.jpg`. Two backgrounds times two page models, 25 frames
/// per clip. Includes three bad rows: a frame with no image, a counter-clockwise quad and an
/// empty quad. Returns `(entries, expected_clips)`.
pub fn smartdoc_entries() -> (Vec<(String, Vec<u8>)>, usize) {
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    let mut csv = String::from(
        "bg_name,bg_id,model_name,model_id,frame_index,tl_x,tl_y,tr_x,tr_y,br_x,br_y,bl_x,bl_y\n",
    );
    let mut clips = 0;
    for (bi, bg) in ["background01", "background02"].iter().enumerate() {
        for (mi, model) in ["datasheet001", "letter001"].iter().enumerate() {
            clips += 1;
            for f in 0..25u32 {
                let j = f64::from(f) * 0.1;
                let (x0, y0) = (8.0 + j + bi as f64, 6.0 + j);
                let (x1, y1) = (
                    FRAME_W as f64 - 8.0 - j,
                    FRAME_H as f64 - 6.0 - j - mi as f64,
                );
                csv.push_str(&format!(
                    "{bg},{bi},{model},{mi},{f},{x0},{y0},{x1},{y0},{x1},{y1},{x0},{y1}\n"
                ));
                entries.push((
                    format!("frames/{bg}/{model}/{f:06}.jpg"),
                    jpeg(FRAME_W, FRAME_H),
                ));
            }
        }
    }
    // frame 30 of the first clip has a row but no image
    csv.push_str("background01,0,datasheet001,0,30,8,6,56,6,56,42,8,42\n");
    // frame 40 is wound counter-clockwise
    csv.push_str("background01,0,datasheet001,0,40,8,6,8,42,56,42,56,6\n");
    entries.push((
        "frames/background01/datasheet001/000040.jpg".to_owned(),
        jpeg(FRAME_W, FRAME_H),
    ));
    // frame 50 has no document in view
    csv.push_str("background01,0,datasheet001,0,50,,,,,,,,\n");
    entries.push((
        "frames/background01/datasheet001/000050.jpg".to_owned(),
        jpeg(FRAME_W, FRAME_H),
    ));
    entries.push(("metadata.csv".to_owned(), csv.into_bytes()));
    (entries, clips)
}

pub fn write_tree(root: &Path, entries: &[(String, Vec<u8>)]) {
    for (name, bytes) in entries {
        write(&root.join(name), bytes);
    }
}

/// CORD-like tree: `<split>/image/*.png` and `<split>/json/*.json`.
pub fn cord_entries() -> Vec<(String, Vec<u8>)> {
    let mut e: Vec<(String, Vec<u8>)> = Vec::new();
    let roi = |x1: u32, y1: u32, x2: u32, y2: u32| {
        // counter-clockwise on purpose: the adapter must not rely on the order
        format!(
            "\"roi\":{{\"x1\":{x1},\"y1\":{y1},\"x2\":{x1},\"y2\":{y2},\"x3\":{x2},\"y3\":{y2},\"x4\":{x2},\"y4\":{y1}}}"
        )
    };
    for (split, n) in [("train", 3), ("test", 2)] {
        for i in 0..n {
            let stem = format!("receipt_{i:05}");
            e.push((format!("{split}/image/{stem}.png"), png(60, 90)));
            let words = "\"valid_line\":[{\"words\":[{\"text\":\"NASI\"},{\"text\":\"GORENG\"}]},{\"words\":[{\"text\":\"15.000\"}]}]";
            let meta = "\"meta\":{\"image_size\":{\"width\":60,\"height\":90}}";
            let json = format!("{{{words},{meta},{}}}", roi(6, 8, 52, 80));
            e.push((format!("{split}/json/{stem}.json"), json.into_bytes()));
        }
    }
    // no outline: transcript only
    e.push(("train/image/receipt_00009.png".into(), png(60, 90)));
    e.push((
        "train/json/receipt_00009.json".into(),
        br#"{"valid_line":[{"words":[{"text":"TOTAL"}]}],"meta":{"image_size":{"width":60,"height":90}}}"#
            .to_vec(),
    ));
    // meta says another size than the image
    e.push(("train/image/receipt_00008.png".into(), png(60, 90)));
    e.push((
        "train/json/receipt_00008.json".into(),
        format!(
            "{{\"valid_line\":[],\"meta\":{{\"image_size\":{{\"width\":600,\"height\":900}}}},{}}}",
            roi(6, 8, 52, 80)
        )
        .into_bytes(),
    ));
    // json without image
    e.push(("train/json/receipt_00007.json".into(), b"{}".to_vec()));
    e
}

/// MIDV-500-like tree: `<doc>/images/<cond>/<clip>/<frame>.tif` plus
/// `<doc>/ground_truth/<cond>/<clip>/<frame>.json`. Two documents times two conditions times
/// two clips of 12 frames, plus a counter-clockwise quad, a json without a quad and a frame
/// with no image.
pub fn midv_entries() -> Vec<(String, Vec<u8>)> {
    let mut e: Vec<(String, Vec<u8>)> = Vec::new();
    for doc in ["01_alb_id", "02_aut_drvlic_new"] {
        for cond in ["TS", "HA"] {
            for clip in 1..=2 {
                let clip_name = format!("{cond}{clip:02}");
                for f in 1..=12 {
                    let stem = format!("{clip_name}_{f:02}");
                    e.push((
                        format!("{doc}/images/{cond}/{clip_name}/{stem}.tif"),
                        tiff(FRAME_W, FRAME_H),
                    ));
                    let j = f64::from(f) * 0.2;
                    e.push((
                        format!("{doc}/ground_truth/{cond}/{clip_name}/{stem}.json"),
                        format!(
                            "{{\"quad\":[[{a},{b}],[{c},{b}],[{c},{d}],[{a},{d}]],\"field01\":{{\"quad\":[[1,1],[2,1],[2,2],[1,2]]}}}}",
                            a = 6.0 + j,
                            b = 5.0 + j,
                            c = 58.0 - j,
                            d = 43.0 - j
                        )
                        .into_bytes(),
                    ));
                }
            }
        }
    }
    let doc = "01_alb_id";
    e.push((
        format!("{doc}/images/TS/TS01/TS01_90.tif"),
        tiff(FRAME_W, FRAME_H),
    ));
    e.push((
        format!("{doc}/ground_truth/TS/TS01/TS01_90.json"),
        br#"{"quad":[[6,5],[6,43],[58,43],[58,5]]}"#.to_vec(),
    ));
    e.push((
        format!("{doc}/ground_truth/TS/TS01/TS01_91.json"),
        br#"{"field01":{}}"#.to_vec(),
    ));
    e.push((
        format!("{doc}/ground_truth/TS/TS01/TS01_92.json"),
        br#"{"quad":[[6,5],[58,5],[58,43],[6,43]]}"#.to_vec(),
    ));
    // a per-document annotation file at another depth is not a frame
    e.push((
        format!("{doc}/ground_truth/{doc}.json"),
        br#"{"quad":[[0,0]]}"#.to_vec(),
    ));
    e
}

/// DIBCO-like tree: originals and `_GT` or `gt/` ground truths grouped by year.
pub fn dibco_entries() -> Vec<(String, Vec<u8>)> {
    vec![
        ("2016/img/1.png".into(), png(40, 30)),
        ("2016/gt/1_GT.png".into(), png(40, 30)),
        ("2016/img/2.png".into(), png(40, 30)),
        ("2016/gt/2_GT.png".into(), png(40, 30)),
        ("2016/gt/9_GT.png".into(), png(40, 30)),
        ("2017/HW1.png".into(), png(50, 20)),
        ("2017/HW1_GT.png".into(), png(50, 20)),
        ("2017/orphan.png".into(), png(50, 20)),
        ("2018/a.png".into(), png(30, 30)),
        ("2018/a_GT.png".into(), png(31, 30)),
        ("2015/x.bmp".into(), b"BM not decodable here".to_vec()),
        ("2015/x_GT.bmp".into(), b"BM not decodable here".to_vec()),
        ("readme.txt".into(), b"hello".to_vec()),
    ]
}

pub fn sha(i: u32) -> String {
    sha256_hex(format!("sample-{i}").as_bytes())
}

/// A mixed-licence raw.pixls.us-like index: only the entries named `cc0-*` may survive.
pub fn pixls_index_lines() -> Vec<String> {
    let l = |path: &str, licence: Option<&str>, sha: Option<String>| {
        let mut o = serde_json::json!({"path": path, "size": 1000});
        if let Some(l) = licence {
            o["licence"] = l.into();
        }
        if let Some(s) = sha {
            o["sha256"] = s.into();
        }
        o.to_string()
    };
    vec![
        l("Canon/EOS_5D/cc0-a.CR2", Some("CC0-1.0"), Some(sha(1))),
        l("Nikon/D70/cc0-b.NEF", Some("CC0"), Some(sha(2))),
        l("Sony/A7/cc0-c.ARW", Some(" cc0-1.0 "), Some(sha(3))),
        l(
            "Old/rawsamples/not-cc0-sa.RAW",
            Some("CC-BY-SA-4.0"),
            Some(sha(4)),
        ),
        l(
            "Old/rawsamples/not-cc0-pd.RAW",
            Some("Public Domain"),
            Some(sha(5)),
        ),
        l("Old/rawsamples/not-cc0-missing.RAW", None, Some(sha(6))),
        l("Old/rawsamples/not-cc0-empty.RAW", Some(""), Some(sha(7))),
        l(
            "Old/rawsamples/not-cc0-combined.RAW",
            Some("CC0-1.0 OR CC-BY-SA-4.0"),
            Some(sha(8)),
        ),
        l(
            "Old/rawsamples/not-cc0-nc.RAW",
            Some("CC-BY-NC-4.0"),
            Some(sha(9)),
        ),
        l("Fuji/X100/cc0-but-no-hash.RAF", Some("CC0-1.0"), None),
        l(
            "../escape/cc0-traversal.CR2",
            Some("CC0-1.0"),
            Some(sha(10)),
        ),
        l("/abs/cc0-absolute.CR2", Some("CC0-1.0"), Some(sha(11))),
    ]
}
