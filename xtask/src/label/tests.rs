// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Tests of the labeller server. All images are synthetic; nothing here reads `_data`.

use super::*;
use auto_crop_codecs::{Format, encode};
use auto_crop_imgproc::Raster;
use std::io::{Read, Write};
use std::net::TcpStream;

/// A bright page on a dark background; `variant` makes every file's bytes different.
fn synthetic_png(w: u32, h: u32, variant: u8) -> Vec<u8> {
    let mut r = Raster::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let inside = x > w / 5 && x < w * 4 / 5 && y > h / 5 && y < h * 4 / 5;
            let v = if inside { 235 } else { 40 };
            let i = ((y * w + x) * 3) as usize;
            r.data[i..i + 3].copy_from_slice(&[v, v, v]);
        }
    }
    r.data[0] = variant;
    encode(&r, Format::Png, 90, None).expect("encodes")
}

struct Fixture {
    _dir: tempfile::TempDir,
    images: PathBuf,
    labels: PathBuf,
    shared: Arc<Shared>,
    listener: Option<TcpListener>,
}

fn fixture_with(suggestions: bool) -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let images = dir.path().join("images");
    let labels = dir.path().join("labels");
    std::fs::create_dir_all(&images).expect("mkdir");
    for i in 0..3u8 {
        std::fs::write(
            images.join(format!("img{i}.png")),
            synthetic_png(160, 120, i),
        )
        .expect("write");
    }
    std::fs::write(images.join("notes.txt"), "not an image").expect("write");
    std::fs::write(images.join("x.heic"), "fake").expect("write");
    let (shared, listener) = bind(Config {
        images_dir: images.clone(),
        labels_dir: labels.clone(),
        annotator: Some("tester".to_owned()),
        suggestions,
        port: 0,
    })
    .expect("binds");
    Fixture {
        _dir: dir,
        images,
        labels,
        shared,
        listener: Some(listener),
    }
}

fn fixture() -> Fixture {
    fixture_with(true)
}

impl Fixture {
    fn req(&self, method: &str, target: &str, body: Option<&str>) -> Request {
        let port = self.shared.port;
        let sep = if target.contains('?') { '&' } else { '?' };
        let target = format!("{target}{sep}t={}", self.shared.token());
        let mut r = http::parse_head(&format!(
            "{method} {target} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{port}\r\nContent-Type: application/json"
        ))
        .expect("parses");
        r.body = body.unwrap_or_default().as_bytes().to_vec();
        r
    }

    fn get(&self, target: &str) -> Response {
        self.shared.handle(&self.req("GET", target, None))
    }

    fn post(&self, target: &str, body: &str) -> Response {
        self.shared.handle(&self.req("POST", target, Some(body)))
    }
}

const GOOD_QUAD: &str = "[[0.2,0.2],[0.8,0.2],[0.8,0.8],[0.2,0.8]]";

fn payload(quad: &str, slices: &str) -> String {
    format!(
        "{{\"slices\":{slices},\"items\":[{{\"quad\":{quad},\"partial_frame\":false,\"curved\":false,\"touching\":false,\"hand_held\":true,\"folded\":false}}],\"seconds\":12.5}}"
    )
}

fn json_of(r: &Response) -> serde_json::Value {
    serde_json::from_slice(&r.body).expect("json body")
}

#[test]
fn token_host_origin_and_method_rules() {
    let f = fixture();
    assert_eq!(f.get("/api/list").status, 200);
    // No token, a wrong token, a token of the wrong length.
    let port = f.shared.port;
    let head = |extra: &str| {
        http::parse_head(&format!(
            "GET /api/list{extra} HTTP/1.1\r\nHost: 127.0.0.1:{port}"
        ))
        .expect("parses")
    };
    assert_eq!(f.shared.handle(&head("")).status, 403);
    assert_eq!(f.shared.handle(&head("?t=wrong")).status, 403);
    assert_eq!(f.shared.handle(&head("?t=")).status, 403);
    // Host must be this server (DNS rebinding).
    for host in ["evil.example", "127.0.0.1:1", "localhost", "127.0.0.1"] {
        let mut r = f.req("GET", "/api/list", None);
        r.headers.insert("host".to_owned(), host.to_owned());
        assert_eq!(f.shared.handle(&r).status, 403, "host {host}");
    }
    let mut r = f.req("GET", "/api/list", None);
    r.headers.remove("host");
    assert_eq!(f.shared.handle(&r).status, 403);
    let mut r = f.req("GET", "/api/list", None);
    r.headers
        .insert("host".to_owned(), format!("localhost:{port}"));
    assert_eq!(f.shared.handle(&r).status, 200);
    // Origin and fetch metadata.
    let mut r = f.req("GET", "/api/list", None);
    r.headers
        .insert("origin".to_owned(), "http://evil.example".to_owned());
    assert_eq!(f.shared.handle(&r).status, 403);
    let mut r = f.req("GET", "/api/list", None);
    r.headers
        .insert("sec-fetch-site".to_owned(), "cross-site".to_owned());
    assert_eq!(f.shared.handle(&r).status, 403);
    // POST needs JSON and an origin.
    let mut r = f.req("POST", "/api/skip/0", Some("{\"skipped\":true}"));
    r.headers
        .insert("content-type".to_owned(), "text/plain".to_owned());
    assert_eq!(f.shared.handle(&r).status, 415);
    let mut r = f.req("POST", "/api/skip/0", Some("{\"skipped\":true}"));
    r.headers.remove("origin");
    assert_eq!(f.shared.handle(&r).status, 403);
    // Other methods.
    for m in ["PUT", "DELETE", "OPTIONS", "HEAD"] {
        assert_eq!(
            f.shared.handle(&f.req(m, "/api/list", None)).status,
            405,
            "{m}"
        );
    }
}

#[test]
fn path_traversal_and_bad_indices_are_rejected_and_names_never_become_paths() {
    let f = fixture();
    for t in [
        "/api/meta/9",
        "/api/meta/abc",
        "/api/meta/-1",
        "/api/meta/1.5",
        "/api/meta/99999999",
        "/api/preview/img0.png",
        "/api/preview/0/extra",
        "/images/img0.png",
        "/api/tile/7?x=0&y=0&w=1&h=1",
        "/etc/passwd",
    ] {
        assert_eq!(f.get(t).status, 404, "{t}");
    }
    assert_eq!(f.get("/api/tile/0?x=nan&y=0&w=1&h=1").status, 400);
    assert_eq!(f.get("/api/tile/0?x=0&y=0&w=0&h=1").status, 400);
    assert_eq!(
        f.post("/api/save/12", &payload(GOOD_QUAD, "[\"flatbed-single\"]"))
            .status,
        404
    );
    // The raw parser refuses dot segments, encoded dots and backslashes before routing sees them.
    for raw in [
        "GET /../etc/passwd HTTP/1.1\r\nHost: x",
        "GET /%2e%2e/etc/passwd HTTP/1.1\r\nHost: x",
        "GET /api/..%2f..%2fsecret HTTP/1.1\r\nHost: x",
        "GET /api\\..\\x HTTP/1.1\r\nHost: x",
    ] {
        assert!(http::parse_head(raw).is_err(), "{raw}");
    }
    // The list names files, never paths, and leaves out what this build cannot decode.
    let list = json_of(&f.get("/api/list"));
    let text = list.to_string();
    assert!(!text.contains(&f.images.display().to_string()), "{text}");
    assert!(!text.contains('\\') && !text.contains("notes.txt") && !text.contains("x.heic"));
    assert_eq!(list["images"].as_array().expect("array").len(), 3);
    assert!(f.shared.undecodable.contains(&"x.heic".to_owned()));
}

#[test]
fn it_listens_on_loopback_only_and_every_reply_has_the_security_headers() {
    let mut f = fixture();
    let listener = f.listener.take().expect("listener");
    let addr = listener.local_addr().expect("addr");
    assert_eq!(addr.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
    assert_ne!(addr.port(), 0);
    let shared = Arc::clone(&f.shared);
    std::thread::spawn(move || serve(shared, listener));
    let roundtrip = |raw: String| -> String {
        let mut s = TcpStream::connect(addr).expect("connects");
        s.set_read_timeout(Some(Duration::from_secs(10)))
            .expect("timeout");
        s.write_all(raw.as_bytes()).expect("writes");
        let mut out = Vec::new();
        let _ = s.read_to_end(&mut out);
        String::from_utf8_lossy(&out).into_owned()
    };
    let host = format!("127.0.0.1:{}", addr.port());
    let page = roundtrip(format!(
        "GET /?t={} HTTP/1.1\r\nHost: {host}\r\n\r\n",
        f.shared.token()
    ));
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    for h in [
        "Cache-Control: no-store",
        "X-Content-Type-Options: nosniff",
        "Referrer-Policy: no-referrer",
        "Content-Security-Policy: default-src 'none'",
    ] {
        assert!(
            page.contains(h),
            "{h} missing:\n{}",
            &page[..page.len().min(900)]
        );
    }
    // The nonce in the policy is the one in the page, and it changes per load.
    let nonce = page
        .split("script-src 'nonce-")
        .nth(1)
        .and_then(|s| s.split('\'').next())
        .expect("nonce in the policy")
        .to_owned();
    assert!(page.contains(&format!("<script nonce=\"{nonce}\">")));
    assert!(!page.contains("{{NONCE}}"));
    let again = roundtrip(format!(
        "GET /?t={} HTTP/1.1\r\nHost: {host}\r\n\r\n",
        f.shared.token()
    ));
    assert!(!again.contains(&nonce));
    // Rejections carry the headers too.
    let denied = roundtrip(format!("GET /api/list HTTP/1.1\r\nHost: {host}\r\n\r\n"));
    assert!(denied.starts_with("HTTP/1.1 403"), "{denied}");
    assert!(denied.contains("Cache-Control: no-store") && denied.contains("nosniff"));
    let rebind = roundtrip(format!(
        "GET /api/list?t={} HTTP/1.1\r\nHost: attacker.example:{}\r\n\r\n",
        f.shared.token(),
        addr.port()
    ));
    assert!(rebind.starts_with("HTTP/1.1 403"), "{rebind}");
    let dots = roundtrip(format!(
        "GET /../secret?t={} HTTP/1.1\r\nHost: {host}\r\n\r\n",
        f.shared.token()
    ));
    assert!(dots.starts_with("HTTP/1.1 400"), "{dots}");
    // A real preview comes back as an image, not the file.
    let img = roundtrip(format!(
        "GET /api/preview/0?t={} HTTP/1.1\r\nHost: {host}\r\n\r\n",
        f.shared.token()
    ));
    assert!(
        img.starts_with("HTTP/1.1 200") && img.contains("Content-Type: image/"),
        "{}",
        &img[..img.len().min(300)]
    );
}

#[test]
fn a_label_round_trips_through_the_server_and_passes_check_labels() {
    let f = fixture();
    let meta = json_of(&f.get("/api/meta/1"));
    assert_eq!(
        (meta["width"].as_u64(), meta["height"].as_u64()),
        (Some(160), Some(120))
    );
    assert!(meta["label"].is_null());
    assert_eq!(meta["assisted"], false);
    // Valid save.
    let r = f.post(
        "/api/save/1",
        &payload(GOOD_QUAD, "[\"flatbed-single\",\"phone-document\"]"),
    );
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let path = f.labels.join("img1.png.json");
    let on_disk =
        golden::parse_label(&std::fs::read_to_string(&path).expect("label file")).expect("parses");
    assert_eq!(on_disk.id, "img1.png");
    assert_eq!(
        on_disk.image_sha256,
        golden::sha256_file(&f.images.join("img1.png")).expect("hash")
    );
    assert_eq!((on_disk.width, on_disk.height), (160, 120));
    assert_eq!(on_disk.items.len(), 1);
    assert!(on_disk.items[0].hand_held);
    assert!(!on_disk.assisted);
    assert_eq!(on_disk.annotator.as_deref(), Some("tester"));
    assert_eq!(on_disk.labelling_seconds, Some(12.5));
    assert_eq!(on_disk.orientation_quarter_turns, 0);
    assert_eq!(on_disk.scene_id, "img1.png");
    // Read back through the API; the labelling time accumulates over saves.
    let meta = json_of(&f.get("/api/meta/1"));
    assert_eq!(
        meta["label"]["items"][0]["quad"],
        serde_json::from_str::<serde_json::Value>(GOOD_QUAD).expect("json")
    );
    assert_eq!(
        f.post("/api/save/1", &payload(GOOD_QUAD, "[\"flatbed-single\"]"))
            .status,
        200
    );
    let again =
        golden::parse_label(&std::fs::read_to_string(&path).expect("file")).expect("parses");
    assert_eq!(again.labelling_seconds, Some(25.0));
    // A top edge pointing down means the content needs rotating: reported through the quarter turns.
    let rotated = "[[0.8,0.2],[0.8,0.8],[0.2,0.8],[0.2,0.2]]";
    assert_eq!(
        f.post("/api/save/2", &payload(rotated, "[\"general-photo\"]"))
            .status,
        200
    );
    let l2 = golden::parse_label(
        &std::fs::read_to_string(f.labels.join("img2.png.json")).expect("file"),
    )
    .expect("parses");
    assert_eq!(l2.orientation_quarter_turns, 3);
    // Invalid labels are refused with reasons and write nothing.
    let ccw = "[[0.2,0.8],[0.8,0.8],[0.8,0.2],[0.2,0.2]]";
    let r = f.post("/api/save/0", &payload(ccw, "[\"flatbed-single\"]"));
    assert_eq!(r.status, 422);
    assert!(
        json_of(&r)["errors"]
            .to_string()
            .contains("counter-clockwise")
    );
    assert_eq!(f.post("/api/save/0", &payload(GOOD_QUAD, "[]")).status, 422);
    assert_eq!(f.post("/api/save/0", "{not json").status, 400);
    assert_eq!(
        f.post(
            "/api/save/0",
            &payload("[[0.1,0.1],[0.9,0.1],[0.9,0.9]]", "[\"flatbed-single\"]")
        )
        .status,
        400
    );
    assert!(!f.labels.join("img0.png.json").exists());
    // A negative has no items.
    let neg = "{\"slices\":[\"negative\"],\"items\":[],\"seconds\":3}";
    assert_eq!(f.post("/api/save/0", neg).status, 200);
    // Skip: persisted, shown in the list, cleared by a later save.
    let list = json_of(&f.get("/api/list"));
    assert_eq!(list["images"][0]["state"], "labelled");
    std::fs::remove_file(f.labels.join("img0.png.json")).expect("remove");
    assert_eq!(
        f.post("/api/skip/0", "{\"skipped\":true,\"seconds\":4}")
            .status,
        200
    );
    assert_eq!(
        json_of(&f.get("/api/list"))["images"][0]["state"],
        "skipped"
    );
    let state = std::fs::read_to_string(f.labels.join("_state.json")).expect("state");
    assert!(state.contains("img0.png"));
    // Everything it wrote passes the validator (hashes included) apart from the unlabelled image.
    let rep =
        golden::check_dir(&f.labels, &f.images, golden::CheckOptions::default()).expect("checks");
    assert!(rep.errors.is_empty(), "{:?}", rep.errors);
    assert_eq!(rep.images_without_label, ["img0.png", "x.heic"]);
    // The labelling-time log has a line per save and skip.
    let log = std::fs::read_to_string(f.labels.join("_labelling-log.jsonl")).expect("log");
    let lines: Vec<serde_json::Value> = log
        .lines()
        .map(|l| serde_json::from_str(l).expect("json line"))
        .collect();
    assert_eq!(lines.len(), 5);
    assert!(
        lines
            .iter()
            .any(|l| l["event"] == "skip" && l["seconds"] == 4.0)
    );
    assert!(
        lines
            .iter()
            .filter(|l| l["event"] == "save")
            .all(|l| l["id"].is_string())
    );
    // A restarted server finds the skip and the labels again.
    let (again, _l) = bind(Config {
        images_dir: f.images.clone(),
        labels_dir: f.labels.clone(),
        annotator: None,
        suggestions: true,
        port: 0,
    })
    .expect("binds");
    let list = again.persist.lock().expect("lock").skipped.clone();
    assert!(list.contains("img0.png"));
}

#[test]
fn a_suggestion_marks_the_label_assisted_for_good_and_can_be_disabled() {
    let f = fixture();
    // Nothing about a suggestion is in the plain responses.
    let meta = json_of(&f.get("/api/meta/0")).to_string();
    assert!(!meta.contains("\"quad\""));
    assert_eq!(
        f.post("/api/save/0", &payload(GOOD_QUAD, "[\"flatbed-single\"]"))
            .status,
        200
    );
    let path = f.labels.join("img0.png.json");
    assert!(
        !golden::parse_label(&std::fs::read_to_string(&path).expect("f"))
            .expect("p")
            .assisted
    );
    // Asking for the suggestion flags the existing label at once ...
    let s = f.get("/api/suggest/0");
    assert_eq!(s.status, 200);
    assert!(json_of(&s).get("quad").is_some());
    assert!(
        golden::parse_label(&std::fs::read_to_string(&path).expect("f"))
            .expect("p")
            .assisted
    );
    // ... and every later save of that image, even from a fresh server.
    let (again, _l) = bind(Config {
        images_dir: f.images.clone(),
        labels_dir: f.labels.clone(),
        annotator: None,
        suggestions: true,
        port: 0,
    })
    .expect("binds");
    assert!(again.assisted_flag("img0.png"));
    assert!(!again.assisted_flag("img1.png"));
    // A first label made after looking is assisted from its first save.
    assert_eq!(f.get("/api/suggest/1").status, 200);
    assert_eq!(
        f.post("/api/save/1", &payload(GOOD_QUAD, "[\"flatbed-single\"]"))
            .status,
        200
    );
    let l1 =
        golden::parse_label(&std::fs::read_to_string(f.labels.join("img1.png.json")).expect("f"))
            .expect("p");
    assert!(l1.assisted);
    let log = std::fs::read_to_string(f.labels.join("_assist-log.jsonl")).expect("assist log");
    assert_eq!(log.lines().count(), 2);
    // check-labels counts them, golden lock would leave them out.
    let rep =
        golden::check_dir(&f.labels, &f.images, golden::CheckOptions::default()).expect("checks");
    assert_eq!(rep.assisted, 2);
    // --no-suggestions removes the endpoint altogether.
    let off = fixture_with(false);
    assert_eq!(off.get("/api/suggest/0").status, 404);
    assert!(!off.labels.join("_assist-log.jsonl").exists());
}

/// A tiny tag-balance check, so the page cannot ship with a broken structure.
fn tags_balance(html: &str) -> Result<(), String> {
    const VOID: [&str; 6] = ["meta", "img", "input", "br", "link", "hr"];
    let mut stack: Vec<String> = Vec::new();
    let b = html.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        if html[i..].starts_with("<!--") {
            i += html[i..].find("-->").ok_or("unterminated comment")? + 3;
            continue;
        }
        if html[i..].starts_with("<!") {
            i += html[i..].find('>').ok_or("unterminated doctype")? + 1;
            continue;
        }
        let end = i + html[i..].find('>').ok_or("unterminated tag")?;
        let inner = &html[i + 1..end];
        let (closing, inner) = inner
            .strip_prefix('/')
            .map_or((false, inner), |r| (true, r));
        let name: String = inner
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        i = end + 1;
        if closing {
            match stack.pop() {
                Some(open) if open == name => {}
                other => return Err(format!("</{name}> closes {other:?}")),
            }
        } else if !VOID.contains(&name.as_str()) && !inner.ends_with('/') {
            if name == "script" || name == "style" {
                let close = format!("</{name}>");
                i += html[i..]
                    .find(&close)
                    .ok_or("unterminated script or style")?
                    + close.len();
            } else {
                stack.push(name);
            }
        }
    }
    if stack.is_empty() {
        Ok(())
    } else {
        Err(format!("unclosed: {stack:?}"))
    }
}

fn script_of(html: &str) -> &str {
    let start = html.find("<script nonce=\"{{NONCE}}\">").expect("script")
        + "<script nonce=\"{{NONCE}}\">".len();
    let end = html[start..].find("</script>").expect("end") + start;
    &html[start..end]
}

#[test]
fn the_page_is_one_self_contained_document_that_parses() {
    tags_balance(PAGE).expect("balanced tags");
    assert!(PAGE.starts_with("<!doctype html>"));
    assert_eq!(
        PAGE.matches("{{NONCE}}").count(),
        2,
        "one nonce for the style, one for the script"
    );
    // Nothing external: no remote URL (the SVG namespace is an identifier, not a request), no
    // linked script or stylesheet, no inline handler or style attribute (the policy blocks both).
    let without_ns = PAGE.replace("http://www.w3.org/2000/svg", "");
    assert!(
        !without_ns.contains("http://")
            && !without_ns.contains("https://")
            && !without_ns.contains("//cdn"),
        "external URL in the page"
    );
    for banned in [
        "<link",
        "src=\"http",
        "@import",
        " onclick=",
        " onload=",
        " style=\"",
        "javascript:",
        "eval(",
        "new Function",
        "XMLHttpRequest",
        "WebSocket",
        "navigator.sendBeacon",
        "localStorage",
    ] {
        assert!(!PAGE.contains(banned), "page contains {banned}");
    }
    // The only network calls are fetch() of this server's own /api/ paths and image sources from it.
    let script = script_of(PAGE);
    assert_eq!(script.matches("fetch(").count(), 3);
    assert!(
        script.contains("'/api/")
            && !script
                .replace("http://www.w3.org/2000/svg", "")
                .contains("http")
    );
    // Every control the script wires up exists in the markup.
    let ids: BTreeSet<&str> = script
        .split("$('")
        .skip(1)
        .filter_map(|s| s.split('\'').next())
        .collect();
    for id in ids {
        assert!(
            PAGE.contains(&format!("id=\"{id}\"")),
            "script uses #{id} but the markup has none"
        );
    }
    // The detector suggestion is a separate control that starts unchecked.
    assert!(PAGE.contains("<input id=\"sugg\" type=\"checkbox\">"));
    assert!(!PAGE.contains("id=\"sugg\" type=\"checkbox\" checked"));
    // The 48 px handle (radius 24) and Pointer Events, as promised.
    assert!(
        script.contains("const HANDLE = 24")
            && script.contains("pointerdown")
            && script.contains("pointermove")
    );
    // The slice and flag lists in the page are the schema's.
    for s in golden::SLICES {
        assert!(script.contains(&format!("'{s}'")), "{s}");
    }
    for f in golden::ITEM_FLAGS {
        assert!(script.contains(&format!("'{f}'")), "{f}");
    }
}

#[test]
fn the_script_has_valid_syntax_when_node_is_available() {
    let dir = tempfile::tempdir().expect("tempdir");
    let js = dir.path().join("page.js");
    std::fs::write(&js, script_of(PAGE)).expect("write");
    match std::process::Command::new("node")
        .arg("--check")
        .arg(&js)
        .output()
    {
        Ok(o) => assert!(
            o.status.success(),
            "node --check failed:\n{}",
            String::from_utf8_lossy(&o.stderr)
        ),
        Err(_) => eprintln!(
            "node is not installed; the script syntax was not checked (the browser test covers it)"
        ),
    }
}
