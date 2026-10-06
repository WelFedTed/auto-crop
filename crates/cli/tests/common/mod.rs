// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Shared helpers of the CLI integration tests: synthetic images, a sandboxed run of the real
//! binary (its own `--home`, so a test never reads or writes a real backup store), and hashing.

#![allow(dead_code)]

use auto_crop_codecs::{Format, encode};
use auto_crop_engine::util::blake3_hex;
use auto_crop_imgproc::Raster;
use auto_crop_imgproc::synth::{PaperKind, Scene, render_scene};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A document on a desk, clearly detectable (the same family the engine's samples use).
pub fn doc_scene(seed: u64, corners: [(f64, f64); 4]) -> Raster {
    render_scene(&Scene {
        width: 1200,
        height: 900,
        background: [120, 100, 80],
        paper: [244, 242, 236],
        ink: [60, 64, 76],
        kind: PaperKind::Document,
        corners,
        seed,
        noise: 3.0,
        blur_radius: 1,
        shadow: true,
    })
}

pub const GOOD: [(f64, f64); 4] = [(0.18, 0.10), (0.82, 0.14), (0.80, 0.92), (0.20, 0.88)];

pub fn write_jpeg(path: &Path, r: &Raster, q: u8) -> PathBuf {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, encode(r, Format::Jpeg, q, None).unwrap()).unwrap();
    path.to_path_buf()
}

pub fn write_png(path: &Path, r: &Raster) -> PathBuf {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, encode(r, Format::Png, 90, None).unwrap()).unwrap();
    path.to_path_buf()
}

/// A good document photo as a JPEG.
pub fn good_photo(path: &Path, seed: u64) -> PathBuf {
    write_jpeg(path, &doc_scene(seed, GOOD), 90)
}

/// A desk with no paper on it: the detector fails.
pub fn no_document(path: &Path) -> PathBuf {
    write_jpeg(
        path,
        &doc_scene(9, [(1.4, 1.4), (1.9, 1.4), (1.9, 1.9), (1.4, 1.9)]),
        88,
    )
}

/// Four colour blocks on a light scanner bed (a multi-item scan).
pub fn bed_scan(path: &Path) -> PathBuf {
    let (w, h) = (1600u32, 1200u32);
    let mut r = Raster::filled(w, h, [236, 236, 232]);
    let quads: [([f64; 4], [u8; 3]); 4] = [
        ([0.05, 0.05, 0.46, 0.46], [200, 40, 40]),
        ([0.55, 0.05, 0.95, 0.46], [40, 190, 40]),
        ([0.05, 0.55, 0.45, 0.95], [40, 40, 200]),
        ([0.56, 0.55, 0.96, 0.95], [200, 190, 40]),
    ];
    for (b, col) in quads {
        for y in (b[1] * f64::from(h)) as u32..(b[3] * f64::from(h)) as u32 {
            for x in (b[0] * f64::from(w)) as u32..(b[2] * f64::from(w)) as u32 {
                let t = ((x / 9 + y / 9) % 2) as u8 * 6;
                r.set_pixel(
                    x,
                    y,
                    [
                        col[0].saturating_sub(t),
                        col[1].saturating_sub(t),
                        col[2].saturating_sub(t),
                    ],
                );
            }
        }
    }
    write_jpeg(path, &r, 90)
}

pub struct Run {
    pub out: Output,
}

impl Run {
    pub fn code(&self) -> i32 {
        self.out.status.code().unwrap_or(-1)
    }
    pub fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.out.stdout).into_owned()
    }
    pub fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.out.stderr).into_owned()
    }
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.stdout()).unwrap_or_else(|e| {
            panic!(
                "stdout is not JSON ({e}):\n{}\nstderr:\n{}",
                self.stdout(),
                self.stderr()
            )
        })
    }
}

/// The sandbox of one test: a temp folder with `work/` (the images) and `home/` (the store).
pub struct Sandbox {
    pub dir: tempfile::TempDir,
}

impl Sandbox {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("work")).unwrap();
        Self { dir }
    }
    pub fn work(&self) -> PathBuf {
        self.dir.path().join("work")
    }
    pub fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }
    pub fn command(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_auto-crop"));
        c.arg("--home").arg(self.home());
        c.env_remove("AUTO_CROP_HOME");
        c.env_remove("AUTO_CROP_FORCE_NO_AVX2");
        c.env_remove("AUTO_CROP_TEST_DELAY_MS");
        c.env_remove("AUTO_CROP_TEST_CANCEL_AFTER");
        c
    }
    pub fn run(&self, args: &[&str]) -> Run {
        let mut c = self.command();
        c.args(args);
        Run {
            out: c.output().expect("the binary starts"),
        }
    }
    pub fn run_with_env(&self, args: &[&str], env: &[(&str, &str)]) -> Run {
        let mut c = self.command();
        c.args(args);
        for (k, v) in env {
            c.env(k, v);
        }
        Run {
            out: c.output().expect("the binary starts"),
        }
    }
}

pub fn hash(p: &Path) -> String {
    blake3_hex(&std::fs::read(p).unwrap())
}

/// Every file under `dir` with its hash and size, sorted: a snapshot of a tree.
pub fn tree(dir: &Path) -> Vec<(String, String)> {
    fn walk(d: &Path, root: &Path, out: &mut Vec<(String, String)>) {
        let Ok(rd) = std::fs::read_dir(d) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, root, out);
            } else {
                let rel = p
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, hash(&p)));
            }
        }
    }
    let mut v = Vec::new();
    walk(dir, dir, &mut v);
    v.sort();
    v
}

pub fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

// ------------------------------------------------------------------ a small JSON Schema checker

/// The schema file of the run manifest, from the docs.
pub fn manifest_schema() -> serde_json::Value {
    let p =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/schema/run-manifest.v1.schema.json");
    serde_json::from_str(&std::fs::read_to_string(p).expect("the schema file exists"))
        .expect("valid JSON")
}

fn type_ok(t: &str, v: &serde_json::Value) -> bool {
    match t {
        "null" => v.is_null(),
        "boolean" => v.is_boolean(),
        "integer" => v.as_i64().is_some() || v.as_u64().is_some(),
        "number" => v.is_number(),
        "string" => v.is_string(),
        "object" => v.is_object(),
        "array" => v.is_array(),
        _ => false,
    }
}

/// Checks `v` against `schema` (the subset the manifest schema uses: `type`, `const`, `enum`,
/// `required`, `properties`, `items`, `$ref` into `$defs`, `oneOf`). Extra fields are allowed:
/// within version 1 fields are only added. Collects every violation.
pub fn validate(
    root: &serde_json::Value,
    schema: &serde_json::Value,
    v: &serde_json::Value,
    at: &str,
    errs: &mut Vec<String>,
) {
    use serde_json::Value;
    if let Some(r) = schema.get("$ref").and_then(Value::as_str) {
        let name = r.rsplit('/').next().unwrap();
        validate(root, &root["$defs"][name], v, at, errs);
        return;
    }
    if let Some(c) = schema.get("const")
        && c != v
    {
        errs.push(format!("{at}: expected {c}, got {v}"));
    }
    if let Some(Value::Array(e)) = schema.get("enum")
        && !e.contains(v)
    {
        errs.push(format!("{at}: {v} is not one of {e:?}"));
    }
    match schema.get("type") {
        Some(Value::String(t)) if !type_ok(t, v) => errs.push(format!("{at}: not a {t}: {v}")),
        Some(Value::Array(ts)) if !ts.iter().filter_map(Value::as_str).any(|t| type_ok(t, v)) => {
            errs.push(format!("{at}: type {ts:?} does not fit {v}"));
        }
        _ => {}
    }
    if let Some(Value::Array(opts)) = schema.get("oneOf") {
        let fits = opts
            .iter()
            .filter(|o| {
                let mut e = Vec::new();
                validate(root, o, v, at, &mut e);
                e.is_empty()
            })
            .count();
        if fits != 1 {
            errs.push(format!("{at}: matches {fits} of the oneOf options: {v}"));
        }
    }
    if let (Some(Value::Array(req)), Some(obj)) = (schema.get("required"), v.as_object()) {
        for k in req.iter().filter_map(Value::as_str) {
            if !obj.contains_key(k) {
                errs.push(format!("{at}: missing `{k}`"));
            }
        }
    }
    if let (Some(Value::Object(props)), Some(obj)) = (schema.get("properties"), v.as_object()) {
        for (k, sub) in props {
            if let Some(child) = obj.get(k) {
                validate(root, sub, child, &format!("{at}/{k}"), errs);
            }
        }
    }
    if let (Some(items), Some(arr)) = (schema.get("items"), v.as_array()) {
        for (i, child) in arr.iter().enumerate() {
            validate(root, items, child, &format!("{at}/{i}"), errs);
        }
    }
}

pub fn assert_valid_manifest(doc: &serde_json::Value) {
    let schema = manifest_schema();
    let mut errs = Vec::new();
    validate(&schema, &schema, doc, "", &mut errs);
    assert!(
        errs.is_empty(),
        "the manifest does not match its schema:\n{}",
        errs.join("\n")
    );
}
