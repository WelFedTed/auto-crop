// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The workflow policy guard (ROADMAP M1.80). A cheap, line-based backstop that runs in the
//! same job as everything else; `zizmor` (the `workflow lint` job) remains the deep audit.
//!
//! Every file in `.github/workflows/` must satisfy:
//!
//! 1. Every `uses:` is pinned to a full 40-hex commit SHA (local `./` actions excepted), and
//!    every container image to an `@sha256:` digest.
//! 2. A top-level `permissions:` block exists and grants nothing but `read`; write scopes are
//!    granted per job, never `write-all`.
//! 3. No `pull_request_target` and no `workflow_run` trigger (they run with secrets on
//!    untrusted code).
//! 4. A `pull_request` trigger has no `paths:` / `paths-ignore:` filter: a required check that
//!    is skipped by a path filter never reports and blocks docs-only pull requests. The filter
//!    belongs inside the job (see `accuracy-smoke.yml`).
//! 5. No tag trigger, no `release` trigger, no `release*.yml` file and no publishing step (`cargo
//!    publish`, `gh release create`, ...) before the release milestone: pre-1.0 a test tag must
//!    create nothing. [`RELEASE_ALLOWED`] is the explicit allow-list, empty today.

use std::fs;
use std::path::Path;

/// Workflow files allowed to hold release machinery (tags, release trigger, publishing steps).
/// Empty until the release milestone introduces `release.yml` (M2.78); add it here, with the
/// owner's agreement, in the same change.
pub const RELEASE_ALLOWED: &[&str] = &[];

const FORBIDDEN_TRIGGERS: &[&str] = &["pull_request_target", "workflow_run", "release"];

const PUBLISH_MARKERS: &[&str] = &[
    "cargo publish",
    "gh release create",
    "gh release upload",
    "action-gh-release",
    "release-plz",
];

fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.chars().all(|c| c.is_ascii_hexdigit())
}

fn clean_value(v: &str) -> String {
    let v = match v.find(" #") {
        Some(i) => &v[..i],
        None => v,
    };
    v.trim().trim_matches(|c| c == '"' || c == '\'').to_owned()
}

/// `value` of a `key: value` line (`- ` list marker allowed), when the key matches.
fn key_value<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let t = line.trim_start();
    let t = t.strip_prefix("- ").unwrap_or(t).trim_start();
    let rest = t.strip_prefix(key)?.strip_prefix(':')?;
    Some(rest)
}

fn check_uses(name: &str, n: usize, value: &str, out: &mut Vec<String>) {
    let v = clean_value(value);
    if v.starts_with("./") {
        return;
    }
    if let Some(img) = v.strip_prefix("docker://") {
        if !img.contains("@sha256:") {
            out.push(format!(
                "{name}:{n}: `uses: {v}` is not pinned to an @sha256: digest"
            ));
        }
        return;
    }
    match v.split_once('@') {
        Some((_, r)) if is_hex(r, 40) => {}
        _ => out.push(format!(
            "{name}:{n}: `uses: {v}` is not pinned to a full 40-character commit SHA"
        )),
    }
}

fn check_image(name: &str, n: usize, value: &str, out: &mut Vec<String>) {
    let v = clean_value(value);
    if v.is_empty() {
        return;
    }
    match v.split_once("@sha256:") {
        Some((_, d)) if is_hex(d, 64) => {}
        _ => out.push(format!(
            "{name}:{n}: container image `{v}` is not pinned to an @sha256: digest"
        )),
    }
}

/// Checks one workflow file's text. `name` is the file name (used in messages and the allow-list).
pub fn check_workflow(name: &str, text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    let release_ok = RELEASE_ALLOWED.contains(&name);

    if !release_ok && name.starts_with("release") {
        out.push(format!(
            "{name}: a release workflow file is not allowed before the release milestone"
        ));
    }

    // Structure: top-level keys.
    let mut has_permissions = false;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let t = line.trim();
        let n = i + 1;
        if t.is_empty() || t.starts_with('#') {
            i += 1;
            continue;
        }
        if indent(line) == 0 {
            if let Some(rest) = key_value(line, "permissions") {
                has_permissions = true;
                let v = clean_value(rest);
                if v == "write-all" {
                    out.push(format!("{name}:{n}: `permissions: write-all`"));
                }
                // Block form: every scope must be read (or none).
                let mut j = i + 1;
                while j < lines.len() && (lines[j].trim().is_empty() || indent(lines[j]) > 0) {
                    let c = clean_value(lines[j]);
                    if c.ends_with(": write") {
                        out.push(format!(
                            "{name}:{}: top-level permissions must be read-only; grant `{c}` on the job that needs it",
                            j + 1
                        ));
                    }
                    j += 1;
                }
            } else if line.starts_with("on:")
                || line.starts_with("\"on\":")
                || line.starts_with("'on':")
            {
                check_triggers(name, &lines, i, release_ok, &mut out);
            }
        }
        i += 1;
    }
    if !has_permissions {
        out.push(format!(
            "{name}: no top-level `permissions:` block (default token scope is too wide)"
        ));
    }

    // Line checks.
    for (idx, line) in lines.iter().enumerate() {
        let n = idx + 1;
        let t = line.trim();
        if t.starts_with('#') {
            continue;
        }
        if let Some(v) = key_value(line, "uses") {
            check_uses(name, n, v, &mut out);
        }
        if let Some(v) = key_value(line, "image") {
            check_image(name, n, v, &mut out);
        }
        if let Some(v) = key_value(line, "container")
            && !clean_value(v).is_empty()
        {
            check_image(name, n, v, &mut out);
        }
        if let Some(v) = key_value(line, "permissions")
            && clean_value(v) == "write-all"
            && indent(line) > 0
        {
            out.push(format!("{name}:{n}: `permissions: write-all`"));
        }
        if !release_ok {
            let code = line.split(" #").next().unwrap_or(line);
            for m in PUBLISH_MARKERS {
                if code.contains(m) {
                    out.push(format!(
                        "{name}:{n}: publishing step (`{m}`) before the release milestone"
                    ));
                }
            }
        }
    }
    out
}

fn check_triggers(name: &str, lines: &[&str], at: usize, release_ok: bool, out: &mut Vec<String>) {
    let first = lines[at];
    let after = first.split_once(':').map_or("", |x| x.1);
    let inline = clean_value(after);
    let mut triggers: Vec<(String, usize)> = Vec::new();
    if !inline.is_empty() {
        for t in inline.trim_matches(|c| c == '[' || c == ']').split(',') {
            triggers.push((t.trim().to_owned(), at + 1));
        }
    } else {
        let mut base: Option<usize> = None;
        let mut current = String::new();
        let mut j = at + 1;
        while j < lines.len() {
            let l = lines[j];
            let t = l.trim();
            if t.is_empty() || t.starts_with('#') {
                j += 1;
                continue;
            }
            let ind = indent(l);
            if ind == 0 {
                break;
            }
            let b = *base.get_or_insert(ind);
            if ind == b {
                let t = t.strip_prefix("- ").unwrap_or(t);
                let key = t.split(':').next().unwrap_or("").trim().to_owned();
                current.clone_from(&key);
                triggers.push((key, j + 1));
            } else if ind > b {
                let sub = t.split(':').next().unwrap_or("").trim();
                if current == "pull_request" && (sub == "paths" || sub == "paths-ignore") {
                    out.push(format!(
                        "{name}:{}: `pull_request` has a `{sub}` filter; a required check skipped by a path filter never reports. Move the filter into the job",
                        j + 1
                    ));
                }
                if !release_ok && current == "push" && (sub == "tags" || sub == "tags-ignore") {
                    out.push(format!(
                        "{name}:{}: `push` has a `{sub}` trigger; no tag may start a workflow before the release milestone",
                        j + 1
                    ));
                }
            }
            j += 1;
        }
    }
    for (t, n) in triggers {
        if FORBIDDEN_TRIGGERS.contains(&t.as_str()) && !(t == "release" && release_ok) {
            out.push(format!("{name}:{n}: forbidden trigger `{t}`"));
        }
    }
}

/// Checks every workflow under `root/.github/workflows`.
pub fn check_tree(root: &Path) -> Result<Vec<String>, String> {
    let dir = root.join(".github/workflows");
    let mut files: Vec<_> = fs::read_dir(&dir)
        .map_err(|e| format!("cannot read {}: {e}", dir.display()))?
        .filter_map(Result::ok)
        .map(|d| d.path())
        .filter(|p| p.extension().is_some_and(|e| e == "yml" || e == "yaml"))
        .collect();
    files.sort();
    let mut out = Vec::new();
    for f in files {
        let name = f
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let text = fs::read_to_string(&f).map_err(|e| format!("{name}: {e}"))?;
        out.extend(check_workflow(&name, &text));
    }
    Ok(out)
}

const SHA: &str = "3d3c42e5aac5ba805825da76410c181273ba90b1";
const DIGEST: &str = "43b29f65a41eb9c35e1cd5323e3bdf3b655c2357a9f4f1ff2f9c2798e5045d80";

fn good() -> String {
    format!(
        "name: X\n\non:\n  push:\n    branches: [main]\n  pull_request:\n    types: [opened, synchronize]\n\npermissions:\n  contents: read\n\njobs:\n  a:\n    runs-on: ubuntu-22.04\n    container: fedora@sha256:{DIGEST}\n    steps:\n      - uses: actions/checkout@{SHA} # v7\n        with:\n          persist-credentials: false\n      - uses: ./local-action\n      - run: cargo test\n"
    )
}

/// Planted workflows for the self-test: (name, file name, text, marker). `None` = control.
pub fn cases() -> Vec<(&'static str, &'static str, String, Option<&'static str>)> {
    let g = good();
    vec![
        ("control-clean", "ci.yml", g.clone(), None),
        (
            "control-comment-mentions-forbidden-things",
            "ci.yml",
            format!("# pull_request_target and `cargo publish` are forbidden\n{g}"),
            None,
        ),
        (
            "unpinned-tag",
            "ci.yml",
            g.replace(&format!("@{SHA} # v7"), "@v4"),
            Some("40-character commit SHA"),
        ),
        (
            "unpinned-short-sha",
            "ci.yml",
            g.replace(SHA, "3d3c42e"),
            Some("40-character commit SHA"),
        ),
        (
            "unpinned-container",
            "ci.yml",
            g.replace(&format!("@sha256:{DIGEST}"), ":latest"),
            Some("@sha256: digest"),
        ),
        (
            "missing-permissions",
            "ci.yml",
            g.replace("permissions:\n  contents: read\n\n", ""),
            Some("no top-level `permissions:`"),
        ),
        (
            "write-all",
            "ci.yml",
            g.replace("  contents: read", "  contents: write"),
            Some("top-level permissions must be read-only"),
        ),
        (
            "write-all-job-level",
            "ci.yml",
            g.replace("    runs-on: ubuntu-22.04\n", "    runs-on: ubuntu-22.04\n    permissions: write-all\n"),
            Some("write-all"),
        ),
        (
            "pull-request-target",
            "ci.yml",
            g.replace("  pull_request:\n", "  pull_request_target:\n"),
            Some("forbidden trigger `pull_request_target`"),
        ),
        (
            "workflow-run",
            "ci.yml",
            g.replace("  pull_request:\n", "  workflow_run:\n"),
            Some("forbidden trigger `workflow_run`"),
        ),
        (
            "inline-trigger-list",
            "ci.yml",
            g.replace("on:\n  push:\n    branches: [main]\n  pull_request:\n    types: [opened, synchronize]\n", "on: [push, pull_request_target]\n"),
            Some("forbidden trigger `pull_request_target`"),
        ),
        (
            "pull-request-path-filter",
            "ci.yml",
            g.replace("    types: [opened, synchronize]", "    paths: [\"crates/**\"]"),
            Some("path filter"),
        ),
        (
            "pull-request-paths-ignore",
            "ci.yml",
            g.replace("    types: [opened, synchronize]", "    paths-ignore: [\"docs/**\"]"),
            Some("path filter"),
        ),
        (
            "push-tag-trigger",
            "ci.yml",
            g.replace("    branches: [main]", "    tags: [\"v*\"]"),
            Some("no tag may start a workflow"),
        ),
        (
            "release-trigger",
            "ci.yml",
            g.replace("  pull_request:\n", "  release:\n"),
            Some("forbidden trigger `release`"),
        ),
        (
            "release-workflow-file",
            "release.yml",
            g.clone(),
            Some("release workflow file"),
        ),
        (
            "publishing-step",
            "ci.yml",
            g.replace("cargo test", "cargo publish"),
            Some("publishing step"),
        ),
        (
            "gh-release-step",
            "ci.yml",
            g.replace("cargo test", "gh release create v1 --notes x"),
            Some("publishing step"),
        ),
    ]
}

/// Runs every planted workflow; returns problems.
pub fn selftest() -> Vec<String> {
    let mut problems = Vec::new();
    for (name, file, text, marker) in cases() {
        let v = check_workflow(file, &text);
        problems.extend(super::unsafe_guard::judge("workflow", name, &v, marker));
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_planted_workflow_behaves() {
        let problems = selftest();
        assert!(problems.is_empty(), "{problems:#?}");
    }

    #[test]
    fn the_real_workflows_are_clean() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let v = check_tree(&root).unwrap();
        assert!(v.is_empty(), "{v:#?}");
    }
}
