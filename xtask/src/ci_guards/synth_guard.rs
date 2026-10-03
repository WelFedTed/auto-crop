// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The "synthetic generator shares no code with the app" guard (ROADMAP M1.32: CI fails if
//! `tools/synth` imports app code). The generator exists to produce ground truth that does not share
//! a failure mode with the warp, detector or codecs it checks, so it may not import them, call
//! their binaries, or read their sources.
//!
//! The application is Rust; a Python tool can only depend on it through an import of a built
//! extension (`auto_crop`, `imgproc`, ...), a subprocess call to cargo or to its binaries, or a path
//! into `crates/` or the build output. Over every `*.py` under `tools/synth`:
//!
//! 1. an `import` or `from ... import` naming such a module fails;
//! 2. a single-line string literal naming `cargo`, `auto-crop`, `auto_crop`, `crates/` or
//!    `target/release|debug` fails.
//!
//! Comments and triple-quoted strings (docstrings, which legitimately say what the tool does *not*
//! do) are prose and not scanned. `tools/synth/tests/test_independence.py` carries the forbidden
//! patterns as data and is the one allow-listed file ([`ALLOWED_FILES`]); it is the Python-side
//! twin of this guard and checks the AST of everything else, including itself being the only
//! exception.

use std::path::Path;

/// Files exempt from the guard (each needs a row in `docs/policy/ci-guards.md`).
pub const ALLOWED_FILES: &[&str] = &["tools/synth/tests/test_independence.py"];

const BAD_MODULES: &[&str] = &["auto_crop", "autocrop", "imgproc", "auto_crop_eval"];
const BAD_LITERALS: &[&str] = &[
    "cargo",
    "auto-crop",
    "auto_crop",
    "crates/",
    "target/release",
    "target/debug",
];

/// The single-line string literals of Python `text` with their 1-based line numbers; comments and
/// triple-quoted strings are skipped.
fn literals(text: &str) -> Vec<(usize, String)> {
    let c: Vec<char> = text.chars().collect();
    let (mut i, mut line) = (0, 1);
    let mut out = Vec::new();
    while i < c.len() {
        match c[i] {
            '\n' => {
                line += 1;
                i += 1;
            }
            '#' => {
                while i < c.len() && c[i] != '\n' {
                    i += 1;
                }
            }
            q @ ('"' | '\'') => {
                if c.get(i + 1) == Some(&q) && c.get(i + 2) == Some(&q) {
                    // triple-quoted: skip to the closing triple
                    i += 3;
                    while i < c.len()
                        && !(c[i] == q && c.get(i + 1) == Some(&q) && c.get(i + 2) == Some(&q))
                    {
                        if c[i] == '\\' {
                            i += 1;
                        }
                        if c.get(i) == Some(&'\n') {
                            line += 1;
                        }
                        i += 1;
                    }
                    i += 3;
                } else {
                    let start_line = line;
                    let mut s = String::new();
                    i += 1;
                    while i < c.len() && c[i] != q && c[i] != '\n' {
                        if c[i] == '\\' && i + 1 < c.len() {
                            s.push(c[i + 1]);
                            i += 2;
                            continue;
                        }
                        s.push(c[i]);
                        i += 1;
                    }
                    i += 1;
                    out.push((start_line, s));
                }
            }
            _ => i += 1,
        }
    }
    out
}

/// Problems with the Python files `(path, text)` of `tools/synth`.
pub fn check_files(files: &[(String, String)]) -> Vec<String> {
    let mut out = Vec::new();
    for (path, text) in files {
        if ALLOWED_FILES.contains(&path.as_str()) {
            continue;
        }
        for (no, raw) in text.lines().enumerate() {
            let l = raw.trim_start();
            let module = l
                .strip_prefix("import ")
                .or_else(|| l.strip_prefix("from "))
                .map(|r| r.split([' ', '.', ',']).next().unwrap_or(""));
            if let Some(m) = module
                && BAD_MODULES.contains(&m.to_lowercase().as_str())
            {
                out.push(format!(
                    "{path}:{}: imports `{m}`: the generator must not import app code",
                    no + 1
                ));
            }
        }
        for (no, lit) in literals(text) {
            let low = lit.to_lowercase();
            if let Some(bad) = BAD_LITERALS.iter().find(|b| low.contains(*b)) {
                out.push(format!(
                    "{path}:{no}: string {lit:?} names `{bad}`: the generator must not call or read the app"
                ));
            }
        }
    }
    out
}

fn collect(dir: &Path, root: &Path, out: &mut Vec<(String, String)>) -> Result<(), String> {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => return Ok(()),
    };
    let mut entries: Vec<_> = rd.filter_map(Result::ok).map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if p.is_dir() {
            if !matches!(name.as_str(), "__pycache__" | ".venv" | "venv") {
                collect(&p, root, out)?;
            }
        } else if name.ends_with(".py") {
            let rel = p
                .strip_prefix(root)
                .unwrap_or(&p)
                .to_string_lossy()
                .replace('\\', "/");
            let text = std::fs::read_to_string(&p).map_err(|e| format!("{rel}: {e}"))?;
            out.push((rel, text));
        }
    }
    Ok(())
}

pub fn check_tree(root: &Path) -> Result<Vec<String>, String> {
    let mut files = Vec::new();
    collect(&root.join("tools/synth"), root, &mut files)?;
    Ok(check_files(&files))
}

/// Planted cases: `(label, python source, expected message fragment or None for a control)`.
pub fn cases() -> Vec<(&'static str, &'static str, Option<&'static str>)> {
    vec![
        (
            "control: ordinary imports",
            "import numpy as np\nfrom PIL import Image\nimport cv2\n",
            None,
        ),
        (
            "control: docstring and comment may mention the app",
            "\"\"\"Never imports crates/ or runs cargo or auto-crop-eval.\"\"\"\n# crates/imgproc is not used\nx = 1\n",
            None,
        ),
        (
            "a literal that merely contains cargo",
            "label = \"auto crop\"\nname = \"cargo-free\".replace(\"-free\", \"\")\n",
            Some("cargo"),
        ),
        (
            "import of the app",
            "import auto_crop\n",
            Some("imports `auto_crop`"),
        ),
        (
            "from-import of imgproc",
            "from imgproc.warp import warp\n",
            Some("imports `imgproc`"),
        ),
        (
            "indented import",
            "def f():\n    import auto_crop_eval\n",
            Some("imports `auto_crop_eval`"),
        ),
        (
            "subprocess to cargo",
            "import subprocess\nsubprocess.run([\"cargo\", \"run\"])\n",
            Some("cargo"),
        ),
        (
            "path into the crates",
            "p = 'crates/imgproc/src/warp.rs'\n",
            Some("crates/"),
        ),
        (
            "the harness binary",
            "exe = \"target/release/auto-crop-eval\"\n",
            Some("auto-crop"),
        ),
        (
            "multi-line triple string is skipped, the next literal is not",
            "\"\"\"\ncargo\n\"\"\"\nx = \"auto_crop\"\n",
            Some("auto_crop"),
        ),
    ]
}

pub fn selftest() -> Vec<String> {
    let mut problems = Vec::new();
    for (label, src, expect) in cases() {
        let got = check_files(&[("tools/synth/synth/x.py".to_owned(), src.to_owned())]);
        match expect {
            None if !got.is_empty() => {
                problems.push(format!("synth guard control `{label}` failed: {got:?}"));
            }
            Some(frag) if !got.iter().any(|m| m.contains(frag)) => {
                problems.push(format!(
                    "synth guard did not flag `{label}` (wanted `{frag}`): {got:?}"
                ));
            }
            _ => {}
        }
    }
    let allowed = check_files(&[(ALLOWED_FILES[0].to_owned(), "x = \"cargo\"\n".to_owned())]);
    if !allowed.is_empty() {
        problems.push(format!(
            "synth guard flagged an allow-listed file: {allowed:?}"
        ));
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_planted_case_behaves() {
        let problems = selftest();
        assert!(problems.is_empty(), "{problems:#?}");
    }

    #[test]
    fn literals_report_their_line_and_skip_comments_and_docstrings() {
        let l = literals("a = 'x'  # \"no\"\n\"\"\"doc \"quoted\"\n\"\"\"\nb = \"y\\\"z\"\n");
        assert_eq!(l, vec![(1, "x".to_owned()), (4, "y\"z".to_owned())]);
    }

    #[test]
    fn the_real_tool_is_clean() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let v = check_tree(&root).expect("scans");
        assert!(v.is_empty(), "{v:#?}");
    }
}
