// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `cargo xtask check-identity` (ROADMAP M0.02).
//!
//! `identity.toml` holds the application identifier once. Any other
//! `io.github.<owner>.<App>` identifier found in the repository is an error.

use std::fs;
use std::path::Path;

const PREFIX: &str = "io.github.";
const TEXT_EXTS: &[&str] = &[
    "md", "toml", "yml", "yaml", "json", "jsonl", "rs", "txt", "desktop", "xml", "hbs", "csv",
    "ts", "js", "svelte", "html",
];
const SKIP_DIRS: &[&str] = &[".git", "target", "node_modules", "LICENSES", "research"];

pub fn app_id(identity_toml: &str) -> Option<String> {
    identity_toml.lines().find_map(|l| {
        let l = l.trim();
        let rest = l
            .strip_prefix("app_id")?
            .trim_start()
            .strip_prefix('=')?
            .trim();
        Some(rest.trim_matches('"').to_owned())
    })
}

/// Identifier-like tokens starting with `io.github.` and having at least 4 segments.
pub fn ids_in(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(pos) = text[from..].find(PREFIX) {
        let start = from + pos;
        let tail = &text[start..];
        let end = tail
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-'))
            .unwrap_or(tail.len());
        let token = tail[..end].trim_end_matches('.');
        if token.split('.').count() >= 4 {
            out.push(token.to_owned());
        }
        from = start + PREFIX.len();
    }
    out
}

/// File-name suffixes that legitimately follow the app id (Flatpak manifest, desktop file, metainfo).
const ID_SUFFIXES: &[&str] = &[
    "yml",
    "yaml",
    "json",
    "desktop",
    "metainfo.xml",
    "appdata.xml",
    "flatpakref",
    "flatpakrepo",
];

fn is_id_file_name(token: &str, id: &str) -> bool {
    token
        .strip_prefix(id)
        .and_then(|r| r.strip_prefix('.'))
        .is_some_and(|s| ID_SUFFIXES.contains(&s))
}

pub fn violations(file: &str, text: &str, id: &str) -> Vec<String> {
    ids_in(text)
        .into_iter()
        .filter(|t| t != id && !is_id_file_name(t, id))
        .map(|t| format!("{file}: `{t}` is not the app id `{id}`"))
        .collect()
}

fn walk(dir: &Path, id: &str, out: &mut Vec<String>) -> Result<(), String> {
    for entry in fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(Result::ok)
    {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if !SKIP_DIRS.contains(&name.as_str()) {
                walk(&path, id, out)?;
            }
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| TEXT_EXTS.contains(&e))
            && !matches!(
                path.file_name().and_then(|n| n.to_str()),
                Some("identity.toml" | "identity.rs")
            )
            && let Ok(text) = fs::read_to_string(&path)
        {
            out.extend(violations(&path.display().to_string(), &text, id));
        }
    }
    Ok(())
}

pub fn run(_args: &[String]) -> Result<(), String> {
    let toml = fs::read_to_string("identity.toml").map_err(|e| format!("identity.toml: {e}"))?;
    let id = app_id(&toml).ok_or("identity.toml has no app_id")?;
    if ids_in(&id).first() != Some(&id) {
        return Err(format!(
            "app_id `{id}` is not an io.github.<owner>.<App> identifier"
        ));
    }
    let mut bad = Vec::new();
    walk(Path::new("."), &id, &mut bad)?;
    if bad.is_empty() {
        println!("check-identity: only `{id}` is used");
        Ok(())
    } else {
        Err(bad.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_app_id() {
        assert_eq!(
            app_id("# c\napp_id = \"io.github.a.B\"\n").as_deref(),
            Some("io.github.a.B")
        );
    }

    #[test]
    fn finds_ids_and_ignores_prose() {
        let t = "see io.github.welfedted.AutoCrop. also io.github.<user>.<App> and io.github.";
        assert_eq!(ids_in(t), vec!["io.github.welfedted.AutoCrop"]);
    }

    #[test]
    fn planted_other_id_is_a_violation() {
        let v = violations(
            "f.md",
            "id io.github.someone.Other here",
            "io.github.welfedted.AutoCrop",
        );
        assert_eq!(v.len(), 1);
        assert!(
            violations(
                "f.md",
                "io.github.welfedted.AutoCrop",
                "io.github.welfedted.AutoCrop"
            )
            .is_empty()
        );
    }

    #[test]
    fn id_file_names_are_not_violations() {
        let id = "io.github.welfedted.AutoCrop";
        for ok in [
            "io.github.welfedted.AutoCrop.yml",
            "io.github.welfedted.AutoCrop.desktop",
            "io.github.welfedted.AutoCrop.metainfo.xml",
        ] {
            assert!(violations("f", ok, id).is_empty(), "{ok}");
        }
        assert_eq!(
            violations("f", "io.github.welfedted.AutoCrop.Other", id).len(),
            1
        );
    }

    #[test]
    fn case_variants_are_violations() {
        assert_eq!(
            violations(
                "f",
                "io.github.WelFedTed.AutoCrop",
                "io.github.welfedted.AutoCrop"
            )
            .len(),
            1
        );
    }
}
