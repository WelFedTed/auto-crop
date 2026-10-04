// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Shared pieces of the golden-set tooling: argument parsing, the data layout, time stamps, git
//! facts, and the "keep it private" check.
//!
//! Layout under the data directory (default `_data/`, gitignored, never committed):
//!
//! ```text
//! _data/                       the owner's images (top level only; sub-folders are ignored)
//! _data/golden/labels/         one <image file name>.json per image (+ _state.json, logs)
//! _data/golden/splits.lock.json
//! _data/golden/eval-log.jsonl  append-only, hash-chained; eval-log.head holds the count and tip
//! _data/golden/results/        full per-image results (local only)
//! _data/golden/aggregates/     aggregate-only metrics (the only files fit to publish)
//! ```

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

/// Minimal option parsing: `--name value`, `--flag`, positionals.
pub struct Args {
    pub rest: Vec<String>,
}

impl Args {
    pub fn new(rest: &[String]) -> Self {
        Self {
            rest: rest.to_vec(),
        }
    }

    pub fn value(&self, name: &str) -> Result<Option<String>, String> {
        match self.rest.iter().position(|a| a == name) {
            None => Ok(None),
            Some(i) => self
                .rest
                .get(i + 1)
                .filter(|v| !v.starts_with("--"))
                .cloned()
                .map(Some)
                .ok_or_else(|| format!("{name} needs a value")),
        }
    }

    pub fn flag(&self, name: &str) -> bool {
        self.rest.iter().any(|a| a == name)
    }

    /// Arguments that are neither an option nor the value of one of `value_opts`.
    pub fn positionals(&self, value_opts: &[&str]) -> Vec<String> {
        let mut out = Vec::new();
        let mut skip = false;
        for a in &self.rest {
            if skip {
                skip = false;
            } else if value_opts.contains(&a.as_str()) {
                skip = true;
            } else if !a.starts_with("--") {
                out.push(a.clone());
            }
        }
        out
    }

    /// Errors on any `--option` that is neither a flag nor a value option of the command.
    pub fn reject_unknown(&self, flags: &[&str], value_opts: &[&str]) -> Result<(), String> {
        let mut skip = false;
        for a in &self.rest {
            if skip {
                skip = false;
            } else if value_opts.contains(&a.as_str()) {
                skip = true;
            } else if a.starts_with("--") && !flags.contains(&a.as_str()) {
                return Err(format!("unknown option {a}"));
            }
        }
        Ok(())
    }
}

/// Where everything lives.
#[derive(Debug, Clone)]
pub struct Paths {
    pub data: PathBuf,
    pub images: PathBuf,
    pub labels: PathBuf,
    pub golden: PathBuf,
}

pub const DATA_OPTS: [&str; 3] = ["--data", "--images", "--labels"];

impl Paths {
    pub fn from_args(a: &Args) -> Result<Self, String> {
        let data = PathBuf::from(a.value("--data")?.unwrap_or_else(|| "_data".to_owned()));
        let golden = data.join("golden");
        Ok(Self {
            images: a
                .value("--images")?
                .map_or_else(|| data.clone(), PathBuf::from),
            labels: a
                .value("--labels")?
                .map_or_else(|| golden.join("labels"), PathBuf::from),
            data,
            golden,
        })
    }

    pub fn lock(&self) -> PathBuf {
        self.golden.join("splits.lock.json")
    }
    pub fn eval_log(&self) -> PathBuf {
        self.golden.join("eval-log.jsonl")
    }
    pub fn eval_head(&self) -> PathBuf {
        self.golden.join("eval-log.head")
    }
    pub fn results(&self) -> PathBuf {
        self.golden.join("results")
    }
    pub fn aggregates(&self) -> PathBuf {
        self.golden.join("aggregates")
    }
    pub fn noise_floor(&self) -> PathBuf {
        self.golden.join("noise-floor.json")
    }
}

/// `2026-10-04T12:34:56Z` for a Unix time.
pub fn rfc3339(unix: u64) -> String {
    let (days, rem) = ((unix / 86_400) as i64, unix % 86_400);
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

pub fn utc_now() -> String {
    rfc3339(unix_now())
}

/// `20261004T123456Z`, for file names.
pub fn utc_compact() -> String {
    utc_now().replace(['-', ':'], "")
}

fn git_out(args: &[&str]) -> Option<String> {
    let o = Command::new("git").args(args).output().ok()?;
    o.status
        .success()
        .then(|| String::from_utf8_lossy(&o.stdout).trim().to_owned())
}

/// `(commit, dirty)` of the checkout the tool runs in; `("unknown", false)` outside one.
pub fn git_commit() -> (String, bool) {
    let commit = git_out(&["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".to_owned());
    let dirty =
        git_out(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|s| !s.is_empty());
    (commit, dirty)
}

/// Who is running the tool: git's `user.name`, else the OS user.
pub fn who() -> String {
    git_out(&["config", "user.name"])
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("USERNAME").ok())
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_else(|| "unknown".to_owned())
}

/// Refuses a directory that lies inside a git repository without being ignored by it: the golden
/// data (images, labels, results) must never be one `git add .` away from a commit (B21). A
/// directory outside any repository passes.
pub fn ensure_private(path: &Path) -> Result<(), String> {
    let abs = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path)
    };
    // The nearest existing ancestor decides which repository (if any) we are in.
    let mut probe = abs.as_path();
    while !probe.exists() {
        match probe.parent() {
            Some(p) => probe = p,
            None => return Ok(()),
        }
    }
    let probe_dir = if probe.is_dir() {
        probe
    } else {
        probe.parent().unwrap_or(probe)
    };
    let top = Command::new("git")
        .arg("-C")
        .arg(probe_dir)
        .args(["rev-parse", "--show-toplevel"])
        .output();
    let Ok(top) = top else { return Ok(()) };
    if !top.status.success() {
        return Ok(());
    }
    let ignored = Command::new("git")
        .arg("-C")
        .arg(String::from_utf8_lossy(&top.stdout).trim())
        .args(["check-ignore", "-q", "--"])
        .arg(&abs)
        .status();
    match ignored {
        Ok(s) if s.success() => Ok(()),
        Ok(s) if s.code() == Some(1) => Err(format!(
            "{} is inside a git repository and is not ignored by it; golden data must stay under the gitignored _data/ (or outside the repository)",
            abs.display()
        )),
        _ => Ok(()),
    }
}

/// Writes `bytes` to `path` through a temporary file and a rename, creating parent folders.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("cannot replace {}: {e}", path.display()))
}

pub fn sha256_bytes(bytes: &[u8]) -> String {
    auto_crop_eval::manifest::sha256_hex(bytes)
}

pub fn sha256_file(path: &Path) -> Result<String, String> {
    auto_crop_eval::golden::sha256_file(path)
}

pub fn pct(v: Option<f64>) -> String {
    v.map_or_else(|| "n/a".to_owned(), |x| format!("{:.2}%", x * 100.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_are_rfc3339_utc() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339(1_791_114_496), "2026-10-04T11:48:16Z");
        assert_eq!(rfc3339(4_102_444_799), "2099-12-31T23:59:59Z");
    }

    #[test]
    fn options_and_positionals() {
        let a = Args::new(&["dir", "--labels", "l", "--open", "other"].map(str::to_owned));
        assert_eq!(a.value("--labels").expect("ok"), Some("l".to_owned()));
        assert!(a.flag("--open"));
        assert_eq!(a.positionals(&["--labels"]), ["dir", "other"]);
        assert!(a.reject_unknown(&["--open"], &["--labels"]).is_ok());
        assert!(a.reject_unknown(&[], &["--labels"]).is_err());
        assert!(
            Args::new(&["--labels".to_owned()])
                .value("--labels")
                .is_err()
        );
    }

    #[test]
    fn the_private_check_passes_outside_a_repository_and_refuses_unignored_paths_inside_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(ensure_private(&dir.path().join("anything")).is_ok());
        let git = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        };
        if !git(&["init", "-q"]) {
            eprintln!("git is not available; skipping the in-repository half");
            return;
        }
        std::fs::write(dir.path().join(".gitignore"), "/_data\n").expect("write");
        assert!(ensure_private(&dir.path().join("_data").join("golden")).is_ok());
        let err = ensure_private(&dir.path().join("labels")).expect_err("not ignored");
        assert!(err.contains("not ignored"), "{err}");
    }
}
