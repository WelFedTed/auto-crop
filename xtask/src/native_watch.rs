// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `cargo xtask native-watch [--fail-on-stale] [--fake name=version]` (ROADMAP M0.10).
//!
//! For every `native-deps.toml` entry with a `watch` repository, asks GitHub for the newest
//! release and prints `STALE name pinned=.. latest=..` when upstream is ahead of the pin.
//! The security workflow turns each STALE line into a `security-native` issue. `--fake` replaces
//! a pin for the run (the workflow's self-test uses it to prove the watch fires).

use crate::native::{Lib, parse, version_ge};
use std::fs;
use std::process::Command;

/// `v1.2.3` and `libheif-1.2.3` style tags to `1.2.3`.
pub fn normalize_tag(tag: &str) -> String {
    let t = tag.rsplit(['-', '/']).next().unwrap_or(tag);
    t.trim_start_matches(['v', 'V']).to_owned()
}

/// True when `latest` is strictly newer than `pinned` (numeric, dot separated).
pub fn is_stale(pinned: &str, latest: &str) -> bool {
    pinned != latest && version_ge(latest, pinned)
}

/// Highest tag in `tags` that belongs to `line` (`""` = any), ignoring pre-releases.
pub fn newest(tags: &[String], line: &str) -> Option<String> {
    tags.iter()
        .map(|t| normalize_tag(t))
        .filter(|v| v.chars().all(|c| c.is_ascii_digit() || c == '.'))
        .filter(|v| line.is_empty() || v == line || v.starts_with(&format!("{line}.")))
        .fold(None, |best: Option<String>, v| match best {
            Some(b) if version_ge(&b, &v) => Some(b),
            _ => Some(v),
        })
}

fn fetch_tags(repo: &str, source: &str) -> Result<Vec<String>, String> {
    let kind = if source == "tags" { "tags" } else { "releases" };
    let url = format!("https://api.github.com/repos/{repo}/{kind}?per_page=100");
    let mut cmd = Command::new("curl");
    cmd.args([
        "-fsSL",
        "--retry",
        "3",
        "-H",
        "Accept: application/vnd.github+json",
    ]);
    if let Ok(token) = std::env::var("GITHUB_TOKEN")
        && !token.is_empty()
    {
        cmd.args(["-H", &format!("Authorization: Bearer {token}")]);
    }
    let out = cmd.arg(&url).output().map_err(|e| format!("curl: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{repo}: HTTP request failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("{repo}: {e}"))?;
    let releases = v.as_array().ok_or(format!("{repo}: unexpected response"))?;
    if kind == "tags" {
        return Ok(releases
            .iter()
            .filter_map(|r| r["name"].as_str().map(str::to_owned))
            .collect());
    }
    Ok(releases
        .iter()
        .filter(|r| {
            !r["prerelease"].as_bool().unwrap_or(false) && !r["draft"].as_bool().unwrap_or(false)
        })
        .filter_map(|r| r["tag_name"].as_str().map(str::to_owned))
        .collect())
}

pub fn run(args: &[String]) -> Result<(), String> {
    let fail = args.iter().any(|a| a == "--fail-on-stale");
    let fake: Vec<(String, String)> = args
        .windows(2)
        .filter(|w| w[0] == "--fake")
        .filter_map(|w| {
            w[1].split_once('=')
                .map(|(a, b)| (a.to_owned(), b.to_owned()))
        })
        .collect();
    let libs: Vec<Lib> =
        parse(&fs::read_to_string("native-deps.toml").map_err(|e| e.to_string())?)?;
    let mut stale = 0;
    let mut errors = Vec::new();
    for lib in libs.iter().filter(|l| !l.watch.is_empty()) {
        let pinned = fake
            .iter()
            .find(|(n, _)| *n == lib.name)
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| lib.version.clone());
        match fetch_tags(&lib.watch, &lib.watch_source).map(|t| newest(&t, &lib.watch_line)) {
            Ok(Some(latest)) if is_stale(&pinned, &latest) => {
                stale += 1;
                println!(
                    "STALE {} pinned={pinned} latest={latest} url=https://github.com/{}/releases",
                    lib.name, lib.watch
                );
            }
            Ok(Some(latest)) => println!("ok    {} pinned={pinned} latest={latest}", lib.name),
            Ok(None) => errors.push(format!("{}: no release found for {}", lib.name, lib.watch)),
            Err(e) => errors.push(e),
        }
    }
    for e in &errors {
        eprintln!("warning: {e}");
    }
    if fail && stale > 0 {
        return Err(format!("{stale} native pin(s) are behind upstream"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_are_normalised() {
        assert_eq!(normalize_tag("v1.23.5"), "1.23.5");
        assert_eq!(normalize_tag("libheif-1.23.5"), "1.23.5");
        assert_eq!(normalize_tag("3.2.0"), "3.2.0");
    }

    #[test]
    fn staleness_is_numeric() {
        assert!(is_stale("1.23.5", "1.23.6"));
        assert!(is_stale("1.9.9", "1.10.0"));
        assert!(!is_stale("1.23.5", "1.23.5"));
        assert!(!is_stale("1.23.6", "1.23.5"));
    }

    #[test]
    fn newest_respects_the_release_line_and_skips_prereleases() {
        let tags: Vec<String> = [
            "v1.29.0",
            "v1.28.2",
            "v1.28.10",
            "v1.28.3",
            "v1.30.0-rc1",
            "v1.27.9",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        assert_eq!(newest(&tags, "1.28").as_deref(), Some("1.28.10"));
        assert_eq!(newest(&tags, "").as_deref(), Some("1.29.0"));
        assert_eq!(newest(&tags, "2.0"), None);
    }

    #[test]
    fn a_fake_old_pin_would_be_flagged() {
        let latest = newest(&["v1.23.5".to_owned()], "").unwrap();
        assert!(is_stale("1.0.0", &latest));
    }
}
