// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The "no dataset bytes in the repository" guard (ROADMAP M1.36, B21). Public corpora are
//! fetched into a cache outside the checkout and never vendored; this guard makes a commit that
//! contains one fail CI instead of relying on review.
//!
//! Over `git ls-files` (the tracked files, with their sizes):
//!
//! 1. **Archives, columnar data and video** (`zip tar tgz gz xz bz2 zst 7z rar parquet h5 npz
//!    avi mp4 mov mkv ...`) are never tracked, anywhere.
//! 2. **Scans and RAW photos** (`tif tiff heic heif dng cr2 nef arw ...`) may be tracked only in
//!    a fixtures directory ([`FIXTURE_DIRS`]), where the plan allows small synthetic or
//!    self-made test files (at most [`MAX_FIXTURE_BYTES`] each).
//! 3. **Ordinary images** (`png jpg jpeg webp bmp gif avif jxl`) may be tracked in a fixtures
//!    directory or under [`IMAGE_PREFIXES`] (icons, documentation, the UI, packaging).
//! 4. **No file larger than [`MAX_TRACKED_BYTES`]**, whatever its name.
//! 5. **Dataset-looking directories** (`corpus-cache`, `corpora`, `datasets`, `golden`, ...) hold
//!    no tracked file.
//! 6. **`.gitignore`** keeps ignoring [`REQUIRED_IGNORES`], so a stray `git add .` of a cache
//!    placed in the checkout adds nothing.
//!
//! [`ALLOWED_FILES`] is the explicit exception list; every entry needs a row in
//! `docs/policy/ci-guards.md`.

use std::path::Path;
use std::process::Command;

pub const MAX_TRACKED_BYTES: u64 = 5 * 1024 * 1024;
pub const MAX_FIXTURE_BYTES: u64 = 5 * 1024 * 1024;

/// Tracked files exempt from every rule above (none today).
pub const ALLOWED_FILES: &[&str] = &[];

const ARCHIVE_EXT: &[&str] = &[
    "zip", "tar", "tgz", "gz", "xz", "bz2", "zst", "lz4", "7z", "rar", "parquet", "h5", "hdf5",
    "npz", "tfrecord", "avi", "mp4", "mov", "mkv", "webm",
];
const SCAN_EXT: &[&str] = &[
    "tif", "tiff", "heic", "heif", "dng", "cr2", "cr3", "nef", "arw", "orf", "rw2", "raf", "pef",
    "srw", "3fr", "iiq",
];
const IMAGE_EXT: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp", "gif", "avif", "jxl"];

/// Where scans, RAW files and images may live as test fixtures.
pub const FIXTURE_DIRS: &[&str] = &[
    "crates/*/tests/fixtures/",
    "crates/*/fixtures/",
    "xtask/fixtures/",
    "xtask/tests/fixtures/",
];

/// Where ordinary (non-scan) images may live besides the fixtures.
pub const IMAGE_PREFIXES: &[&str] = &[
    "crates/shell/icons/",
    "spikes/gui-tauri/icons/",
    "ui/",
    "docs/",
    "packaging/",
];

/// Directory names that mean "data lives here".
const DATA_DIRS: &[&str] = &[
    "corpus-cache",
    "corpus_cache",
    "corpora",
    "datasets",
    "dataset",
    "golden",
];

/// Lines `.gitignore` must keep.
pub const REQUIRED_IGNORES: &[&str] = &[
    "/corpus-cache/",
    "/corpora/",
    "/datasets/",
    "/golden/",
    // The owner's private sample images and golden set (B21).
    "/_data",
];

fn matches_dir(path: &str, pattern: &str) -> bool {
    let mut p = path.split('/');
    for part in pattern.trim_end_matches('/').split('/') {
        match p.next() {
            Some(seg) if part == "*" || seg == part => {}
            _ => return false,
        }
    }
    // The pattern is a directory prefix: the path must continue below it.
    p.next().is_some()
}

fn in_any(path: &str, dirs: &[&str]) -> bool {
    dirs.iter().any(|d| matches_dir(path, d))
}

fn ext(path: &str) -> String {
    path.rsplit('/')
        .next()
        .and_then(|f| f.rsplit_once('.'))
        .map(|(_, e)| e.to_lowercase())
        .unwrap_or_default()
}

/// Problems with a set of tracked files `(path with forward slashes, size in bytes)`.
pub fn check_files(files: &[(String, u64)]) -> Vec<String> {
    let mut out = Vec::new();
    for (path, size) in files {
        if ALLOWED_FILES.contains(&path.as_str()) {
            continue;
        }
        let lower = path.to_lowercase();
        let e = ext(&lower);
        let fixture = in_any(&lower, FIXTURE_DIRS);
        if ARCHIVE_EXT.contains(&e.as_str()) {
            out.push(format!(
                "{path}: an archive, columnar or video file is tracked; datasets are fetched into the cache outside the repo, never committed (B21)"
            ));
        } else if SCAN_EXT.contains(&e.as_str()) {
            if !fixture {
                out.push(format!(
                    "{path}: a scan or RAW file is tracked outside a fixtures directory ({})",
                    FIXTURE_DIRS.join(", ")
                ));
            } else if *size > MAX_FIXTURE_BYTES {
                out.push(format!(
                    "{path}: fixture is {size} bytes, over the {MAX_FIXTURE_BYTES} limit"
                ));
            }
        } else if IMAGE_EXT.contains(&e.as_str()) && !fixture && !in_any(&lower, IMAGE_PREFIXES) {
            out.push(format!(
                "{path}: an image is tracked outside the fixtures and asset directories"
            ));
        }
        if *size > MAX_TRACKED_BYTES && !out.iter().any(|m| m.starts_with(&format!("{path}:"))) {
            out.push(format!(
                "{path}: {size} bytes is over the {MAX_TRACKED_BYTES} limit for a tracked file"
            ));
        }
        if let Some(d) = lower
            .split('/')
            .rev()
            .skip(1)
            .find(|seg| DATA_DIRS.contains(seg))
        {
            out.push(format!("{path}: tracked inside a `{d}` directory"));
        }
    }
    out
}

/// Missing lines of the required ignore list.
pub fn check_gitignore(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    REQUIRED_IGNORES
        .iter()
        .filter(|r| !lines.contains(r))
        .map(|r| format!(".gitignore: `{r}` must stay ignored (dataset cache and golden data)"))
        .collect()
}

/// The tracked files of the repository at `root`, with sizes.
pub fn tracked_files(root: &Path) -> Result<Vec<(String, u64)>, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-z"])
        .output()
        .map_err(|e| format!("corpus guard: cannot run git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "corpus guard: git ls-files failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter(|p| !p.is_empty())
        .filter_map(|p| {
            let meta = std::fs::symlink_metadata(root.join(p)).ok()?;
            Some((p.to_owned(), meta.len()))
        })
        .collect())
}

pub fn check_tree(root: &Path) -> Result<Vec<String>, String> {
    let mut v = check_files(&tracked_files(root)?);
    let ignore =
        std::fs::read_to_string(root.join(".gitignore")).map_err(|e| format!(".gitignore: {e}"))?;
    v.extend(check_gitignore(&ignore));
    Ok(v)
}

/// Planted cases: `(label, files, expected message fragment or None for a control)`.
#[allow(clippy::type_complexity)]
pub fn cases() -> Vec<(&'static str, Vec<(String, u64)>, Option<&'static str>)> {
    let f = |p: &str, s: u64| (p.to_owned(), s);
    vec![
        (
            "control: ordinary sources",
            vec![f("crates/eval/src/lib.rs", 4000), f("Cargo.lock", 150_000)],
            None,
        ),
        (
            "control: icons and docs images",
            vec![
                f("crates/shell/icons/32x32.png", 900),
                f("docs/img/flow.png", 80_000),
            ],
            None,
        ),
        (
            "control: small fixture scan",
            vec![
                f("crates/codecs/tests/fixtures/a.tiff", 4000),
                f("crates/codecs/tests/fixtures/b.heic", 1_000_000),
            ],
            None,
        ),
        (
            "smartdoc archive",
            vec![f("data/testDataset.tar.gz", 100)],
            Some("archive"),
        ),
        (
            "zip anywhere",
            vec![f("docs/sample.zip", 100)],
            Some("archive"),
        ),
        (
            "zip in fixtures still banned",
            vec![f("crates/eval/tests/fixtures/x.zip", 100)],
            Some("archive"),
        ),
        (
            "parquet (CORD on HuggingFace)",
            vec![f("cord/train-0000.parquet", 100)],
            Some("archive"),
        ),
        (
            "video frames source",
            vec![f("midv/clip.AVI", 100)],
            Some("archive"),
        ),
        (
            "raw photo outside fixtures",
            vec![f("samples/IMG_0001.CR2", 100)],
            Some("scan or RAW"),
        ),
        (
            "midv tif outside fixtures",
            vec![f("midv500/01_alb_id/images/TS/TS01/TS01_01.tif", 100)],
            Some("scan or RAW"),
        ),
        (
            "oversized fixture",
            vec![f(
                "crates/codecs/tests/fixtures/big.dng",
                MAX_FIXTURE_BYTES + 1,
            )],
            Some("over the"),
        ),
        (
            "receipt photo in src",
            vec![f("crates/eval/src/receipt.jpg", 100)],
            Some("image is tracked"),
        ),
        (
            "cord png at the root",
            vec![f("train/image/receipt_00000.png", 100)],
            Some("image is tracked"),
        ),
        (
            "huge text file",
            vec![f("notes.txt", MAX_TRACKED_BYTES + 1)],
            Some("limit for a tracked file"),
        ),
        (
            "cache dir name",
            vec![f("corpus-cache/x/readme.txt", 10)],
            Some("`corpus-cache` directory"),
        ),
        (
            "golden dir name",
            vec![f("golden/labels/a.json", 10)],
            Some("`golden` directory"),
        ),
        (
            "nested datasets dir",
            vec![f("tools/datasets/index.txt", 10)],
            Some("`datasets` directory"),
        ),
    ]
}

pub fn gitignore_cases() -> Vec<(&'static str, String, usize)> {
    vec![
        (
            "control: all required lines",
            REQUIRED_IGNORES.join("\n"),
            0,
        ),
        ("one missing", REQUIRED_IGNORES[1..].join("\n"), 1),
        ("none", "target\n".to_owned(), REQUIRED_IGNORES.len()),
        (
            "a commented-out line does not count",
            REQUIRED_IGNORES
                .iter()
                .map(|l| format!("# {l}"))
                .collect::<Vec<_>>()
                .join("\n"),
            REQUIRED_IGNORES.len(),
        ),
    ]
}

pub fn selftest() -> Vec<String> {
    let mut problems = Vec::new();
    for (label, files, expect) in cases() {
        let got = check_files(&files);
        match expect {
            None if !got.is_empty() => {
                problems.push(format!("corpus guard control `{label}` failed: {got:?}"))
            }
            Some(frag) if !got.iter().any(|m| m.contains(frag)) => {
                problems.push(format!(
                    "corpus guard did not flag `{label}` (wanted `{frag}`): {got:?}"
                ));
            }
            _ => {}
        }
    }
    for (label, text, missing) in gitignore_cases() {
        let got = check_gitignore(&text);
        if got.len() != missing {
            problems.push(format!(
                "corpus guard gitignore case `{label}`: expected {missing} problem(s), got {got:?}"
            ));
        }
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
    fn the_real_repository_is_clean() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let v = check_tree(&root).unwrap();
        assert!(v.is_empty(), "{v:#?}");
    }

    #[test]
    fn directory_patterns_match_whole_segments() {
        assert!(matches_dir(
            "crates/codecs/tests/fixtures/a.png",
            "crates/*/tests/fixtures/"
        ));
        assert!(!matches_dir(
            "crates/codecs/tests/fixtures",
            "crates/*/tests/fixtures/"
        ));
        assert!(!matches_dir(
            "crates/codecs/tests/fixtures-x/a.png",
            "crates/*/tests/fixtures/"
        ));
        assert!(!matches_dir(
            "other/crates/codecs/tests/fixtures/a.png",
            "crates/*/tests/fixtures/"
        ));
    }

    /// The ignore rules are real: git itself, not just the text of `.gitignore`, ignores
    /// anything below the cache directories (skipped when git is unavailable).
    #[test]
    fn git_really_ignores_cache_directories() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        for p in [
            "corpus-cache/smartdoc2015-ch1/full/src/metadata.csv",
            "corpora/cord/train/image/receipt_00000.png",
            "datasets/x/y.tar.gz",
            "golden/labels/a.json",
        ] {
            let Ok(status) = Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["check-ignore", "-q", p])
                .status()
            else {
                return;
            };
            assert!(status.success(), "git does not ignore {p}");
        }
    }
}
