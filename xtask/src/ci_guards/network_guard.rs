// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The "no network crates in shipped code" guard (ROADMAP M1.72; decisions B18 and C4: offline
//! and private by default, no telemetry).
//!
//! Two layers:
//!
//! * **Manifests.** No first-party manifest (`crates/*/Cargo.toml`, the workspace dependency
//!   table) may name a banned or restricted crate, in any dependency kind.
//! * **The shipped dependency graph.** `cargo metadata` is resolved for each desktop target
//!   triple with all workspace features; starting from the shipped roots (`auto-crop-cli`,
//!   `auto-crop-shell`) only *normal* edges are followed (dev and build dependencies do not ship;
//!   proc-macro crates run at build time and are not expanded). Then:
//!   - a **banned** crate (HTTP clients and servers, TLS stacks, QUIC, WebSocket) anywhere in
//!     that closure fails the guard, even through Tauri;
//!   - a **restricted** crate (low-level socket and async-IO crates and the webview's HTTP
//!     binding) fails the guard unless it is reachable only through the GUI framework stack
//!     ([`FRAMEWORK`]), which needs them internally. First-party code may not use them.
//!
//! Mobile-only edges (Tauri pulls `reqwest` for Android and iOS) are excluded because only the
//! desktop triples in [`TRIPLES`] are resolved.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Command;

/// Desktop targets that ship (B9).
pub const TRIPLES: &[&str] = &[
    "x86_64-pc-windows-msvc",
    "aarch64-apple-darwin",
    "x86_64-unknown-linux-gnu",
];

/// Workspace packages whose dependency closure ships.
pub const SHIPPED_ROOTS: &[&str] = &["auto-crop-cli", "auto-crop-shell"];

/// HTTP, TLS, QUIC and WebSocket crates: never in a shipped build, not even through a framework.
pub const BANNED: &[&str] = &[
    "reqwest",
    "hyper",
    "hyper-util",
    "hyper-rustls",
    "hyper-tls",
    "h2",
    "h3",
    "ureq",
    "curl",
    "curl-sys",
    "isahc",
    "surf",
    "attohttpc",
    "minreq",
    "awc",
    "actix-web",
    "axum",
    "tiny_http",
    "openssl",
    "openssl-sys",
    "native-tls",
    "rustls",
    "tokio-rustls",
    "tokio-native-tls",
    "webpki",
    "webpki-roots",
    "boring",
    "schannel",
    "tungstenite",
    "tokio-tungstenite",
    "quinn",
    "quinn-proto",
];

/// Socket and async-IO crates (and the webview's libsoup binding): fine inside the GUI framework
/// stack, which needs them, never for first-party code.
pub const RESTRICTED: &[&str] = &[
    "tokio",
    "mio",
    "socket2",
    "http",
    "async-std",
    "smol",
    "hickory-resolver",
    "hickory-proto",
    "trust-dns-resolver",
    "soup3",
    "soup3-sys",
];

/// Packages treated as the opaque GUI framework stack when judging [`RESTRICTED`] crates.
pub fn is_framework(name: &str) -> bool {
    name == "tauri"
        || name.starts_with("tauri-")
        || matches!(
            name,
            "wry" | "tao" | "rfd" | "tray-icon" | "muda" | "webview2-com"
        )
        || name.starts_with("webkit2gtk")
}

/// Manifest-level check of one first-party Cargo.toml (or the root `[workspace.dependencies]`).
pub fn check_manifest(path: &str, text: &str) -> Result<Vec<String>, String> {
    let doc: toml::Table = text.parse().map_err(|e| format!("{path}: {e}"))?;
    let mut out = Vec::new();
    let mut tables: Vec<&toml::Table> = Vec::new();
    let kinds = ["dependencies", "dev-dependencies", "build-dependencies"];
    for k in kinds {
        if let Some(t) = doc.get(k).and_then(|v| v.as_table()) {
            tables.push(t);
        }
    }
    if let Some(targets) = doc.get("target").and_then(|v| v.as_table()) {
        for cfg in targets.values().filter_map(|v| v.as_table()) {
            for k in kinds {
                if let Some(t) = cfg.get(k).and_then(|v| v.as_table()) {
                    tables.push(t);
                }
            }
        }
    }
    if let Some(t) = doc
        .get("workspace")
        .and_then(|v| v.as_table())
        .and_then(|w| w.get("dependencies"))
        .and_then(|v| v.as_table())
    {
        tables.push(t);
    }
    for t in tables {
        for (key, val) in t {
            let real = val
                .as_table()
                .and_then(|d| d.get("package"))
                .and_then(|p| p.as_str())
                .unwrap_or(key);
            if BANNED.contains(&real) || RESTRICTED.contains(&real) {
                out.push(format!(
                    "{path}: dependency `{real}` is a network/socket crate; shipped crates stay offline (B18, C4)"
                ));
            }
        }
    }
    Ok(out)
}

struct Graph {
    names: HashMap<String, String>,
    proc_macro: HashSet<String>,
    edges: HashMap<String, Vec<String>>,
    roots: Vec<String>,
}

fn parse_metadata(json: &[u8], roots: &[&str]) -> Result<Graph, String> {
    let v: serde_json::Value =
        serde_json::from_slice(json).map_err(|e| format!("cargo metadata: {e}"))?;
    let mut names = HashMap::new();
    let mut proc_macro = HashSet::new();
    let mut root_ids = Vec::new();
    for p in v["packages"].as_array().ok_or("no packages")? {
        let id = p["id"].as_str().ok_or("package without id")?.to_owned();
        let name = p["name"].as_str().ok_or("package without name")?.to_owned();
        let is_pm = p["targets"].as_array().is_some_and(|ts| {
            ts.iter().any(|t| {
                t["kind"]
                    .as_array()
                    .is_some_and(|k| k.iter().any(|x| x == "proc-macro"))
            })
        });
        if is_pm {
            proc_macro.insert(id.clone());
        }
        if roots.contains(&name.as_str()) {
            root_ids.push(id.clone());
        }
        names.insert(id, name);
    }
    let mut edges: HashMap<String, Vec<String>> = HashMap::new();
    for n in v["resolve"]["nodes"].as_array().ok_or("no resolve nodes")? {
        let id = n["id"].as_str().ok_or("node without id")?.to_owned();
        let mut deps = Vec::new();
        for d in n["deps"].as_array().into_iter().flatten() {
            let normal = d["dep_kinds"]
                .as_array()
                .is_some_and(|ks| ks.iter().any(|k| k["kind"].is_null()));
            if normal && let Some(pkg) = d["pkg"].as_str() {
                deps.push(pkg.to_owned());
            }
        }
        edges.insert(id, deps);
    }
    Ok(Graph {
        names,
        proc_macro,
        edges,
        roots: root_ids,
    })
}

/// Walks the normal-edge closure from the roots. Proc-macro crates are visited, not expanded;
/// framework crates are expanded only when `expand_framework`. Returns id -> parent id.
fn walk(g: &Graph, expand_framework: bool) -> HashMap<String, Option<String>> {
    let mut seen: HashMap<String, Option<String>> = HashMap::new();
    let mut stack: Vec<(String, Option<String>)> =
        g.roots.iter().map(|r| (r.clone(), None)).collect();
    while let Some((id, parent)) = stack.pop() {
        if seen.contains_key(&id) {
            continue;
        }
        seen.insert(id.clone(), parent);
        let name = g.names.get(&id).map_or("", String::as_str);
        if g.proc_macro.contains(&id) {
            continue;
        }
        if !expand_framework && is_framework(name) {
            continue;
        }
        for d in g.edges.get(&id).into_iter().flatten() {
            stack.push((d.clone(), Some(id.clone())));
        }
    }
    seen
}

fn chain(g: &Graph, seen: &HashMap<String, Option<String>>, id: &str) -> String {
    let mut parts = Vec::new();
    let mut cur = Some(id.to_owned());
    while let Some(c) = cur {
        parts.push(g.names.get(&c).cloned().unwrap_or_default());
        cur = seen.get(&c).cloned().flatten();
    }
    parts.join(" <- ")
}

/// Judges a `cargo metadata` document (one target triple).
pub fn check_metadata(json: &[u8], roots: &[&str], triple: &str) -> Result<Vec<String>, String> {
    let g = parse_metadata(json, roots)?;
    if g.roots.is_empty() {
        return Err(format!(
            "none of the shipped roots {roots:?} exist in the metadata"
        ));
    }
    let mut out = Vec::new();
    let full = walk(&g, true);
    for id in full.keys() {
        let name = g.names.get(id).map_or("", String::as_str);
        if BANNED.contains(&name) {
            out.push(format!(
                "[{triple}] banned network crate `{name}` in the shipped build: {}",
                chain(&g, &full, id)
            ));
        }
    }
    let outside = walk(&g, false);
    for id in outside.keys() {
        let name = g.names.get(id).map_or("", String::as_str);
        if RESTRICTED.contains(&name) {
            out.push(format!(
                "[{triple}] socket crate `{name}` reachable outside the GUI framework stack: {}",
                chain(&g, &outside, id)
            ));
        }
    }
    out.sort();
    Ok(out)
}

fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned())
}

/// Resolves and judges one manifest for every triple.
pub fn check_workspace(
    manifest: Option<&Path>,
    roots: &[&str],
    triples: &[&str],
) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for t in triples {
        let mut cmd = Command::new(cargo());
        cmd.args([
            "metadata",
            "--format-version",
            "1",
            "--all-features",
            "--filter-platform",
            t,
        ]);
        if let Some(m) = manifest {
            cmd.arg("--manifest-path").arg(m);
        }
        let o = cmd
            .output()
            .map_err(|e| format!("cannot run cargo metadata: {e}"))?;
        if !o.status.success() {
            return Err(format!(
                "cargo metadata failed for {t}: {}",
                String::from_utf8_lossy(&o.stderr)
            ));
        }
        out.extend(check_metadata(&o.stdout, roots, t)?);
    }
    Ok(out)
}

/// Checks every first-party manifest under `root`.
pub fn check_manifests(root: &Path) -> Result<Vec<String>, String> {
    let mut paths = vec![root.join("Cargo.toml")];
    if let Ok(rd) = std::fs::read_dir(root.join("crates")) {
        let mut crates: Vec<_> = rd.filter_map(Result::ok).map(|d| d.path()).collect();
        crates.sort();
        for c in crates {
            let m = c.join("Cargo.toml");
            if m.exists() {
                paths.push(m);
            }
        }
    }
    let mut out = Vec::new();
    for p in paths {
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        let rel = p
            .strip_prefix(root)
            .unwrap_or(&p)
            .to_string_lossy()
            .replace('\\', "/");
        out.extend(check_manifest(&rel, &text)?);
    }
    Ok(out)
}

// ---- self-test: planted workspaces ----

/// A fake crate: (directory name, package name, extra manifest text).
type Fake = (&'static str, &'static str, &'static str);

/// One planted workspace. The first crate is the shipped root (named `auto-crop-cli`).
pub struct Case {
    pub name: &'static str,
    pub crates: &'static [Fake],
    /// Text the failure output must contain; `None` = a control that must pass.
    pub marker: Option<&'static str>,
}

pub const CASES: &[Case] = &[
    Case {
        name: "control-clean",
        crates: &[
            (
                "root",
                "auto-crop-cli",
                "[dependencies]\nleaf = { path = \"../leaf\" }\n",
            ),
            ("leaf", "leaf", ""),
        ],
        marker: None,
    },
    Case {
        name: "banned-direct-reqwest",
        crates: &[
            (
                "root",
                "auto-crop-cli",
                "[dependencies]\nreqwest = { path = \"../reqwest\" }\n",
            ),
            ("reqwest", "reqwest", ""),
        ],
        marker: Some("banned network crate `reqwest`"),
    },
    Case {
        name: "banned-nested-rustls",
        crates: &[
            (
                "root",
                "auto-crop-cli",
                "[dependencies]\nmiddle = { path = \"../middle\" }\n",
            ),
            (
                "middle",
                "middle",
                "[dependencies]\nrustls = { path = \"../rustls\" }\n",
            ),
            ("rustls", "rustls", ""),
        ],
        marker: Some("banned network crate `rustls`"),
    },
    Case {
        name: "banned-through-framework-still-fails",
        crates: &[
            (
                "root",
                "auto-crop-shell",
                "[dependencies]\ntauri = { path = \"../tauri\" }\n",
            ),
            (
                "tauri",
                "tauri",
                "[dependencies]\nhyper = { path = \"../hyper\" }\n",
            ),
            ("hyper", "hyper", ""),
        ],
        marker: Some("banned network crate `hyper`"),
    },
    Case {
        name: "restricted-direct-tokio",
        crates: &[
            (
                "root",
                "auto-crop-cli",
                "[dependencies]\ntokio = { path = \"../tokio\" }\n",
            ),
            ("tokio", "tokio", ""),
        ],
        marker: Some("socket crate `tokio`"),
    },
    Case {
        name: "restricted-through-first-party",
        crates: &[
            (
                "root",
                "auto-crop-cli",
                "[dependencies]\nmiddle = { path = \"../middle\" }\n",
            ),
            (
                "middle",
                "middle",
                "[dependencies]\nsocket2 = { path = \"../socket2\" }\n",
            ),
            ("socket2", "socket2", ""),
        ],
        marker: Some("socket crate `socket2`"),
    },
    Case {
        name: "control-restricted-via-framework",
        crates: &[
            (
                "root",
                "auto-crop-shell",
                "[dependencies]\ntauri = { path = \"../tauri\" }\n",
            ),
            (
                "tauri",
                "tauri",
                "[dependencies]\ntokio = { path = \"../tokio\" }\nhttp = { path = \"../http\" }\n",
            ),
            ("tokio", "tokio", ""),
            ("http", "http", ""),
        ],
        marker: None,
    },
    Case {
        name: "control-dev-and-build-dependencies-do-not-ship",
        crates: &[
            (
                "root",
                "auto-crop-cli",
                "[dev-dependencies]\nreqwest = { path = \"../reqwest\" }\n[build-dependencies]\nureq = { path = \"../ureq\" }\n",
            ),
            ("reqwest", "reqwest", ""),
            ("ureq", "ureq", ""),
        ],
        marker: None,
    },
    Case {
        name: "control-mobile-only-edge-is-not-desktop",
        crates: &[
            (
                "root",
                "auto-crop-shell",
                "[dependencies]\ntauri = { path = \"../tauri\" }\n",
            ),
            (
                "tauri",
                "tauri",
                "[target.'cfg(any(target_os = \"android\", target_os = \"ios\"))'.dependencies]\nreqwest = { path = \"../reqwest\" }\n",
            ),
            ("reqwest", "reqwest", ""),
        ],
        marker: None,
    },
];

/// Planted first-party manifests: (name, text, marker).
pub const MANIFEST_CASES: &[(&str, &str, Option<&str>)] = &[
    ("control-manifest", "[dependencies]\nserde = \"1\"\n", None),
    (
        "manifest-direct-reqwest",
        "[dependencies]\nreqwest = \"0.12\"\n",
        Some("`reqwest`"),
    ),
    (
        "manifest-optional-ureq-in-target-table",
        "[target.'cfg(unix)'.dependencies]\nureq = { version = \"3\", optional = true }\n",
        Some("`ureq`"),
    ),
    (
        "manifest-renamed-package",
        "[dependencies]\nclient = { package = \"hyper\", version = \"1\" }\n",
        Some("`hyper`"),
    ),
    (
        "manifest-workspace-dependency",
        "[workspace.dependencies]\nnative-tls = \"0.2\"\n",
        Some("`native-tls`"),
    ),
    (
        "manifest-dev-dependency-counts-for-first-party",
        "[dev-dependencies]\ntokio = \"1\"\n",
        Some("`tokio`"),
    ),
];

fn write_case(base: &Path, case: &Case) -> Result<std::path::PathBuf, String> {
    let dir = base.join(case.name);
    let _ = std::fs::remove_dir_all(&dir);
    for (i, (d, name, extra)) in case.crates.iter().enumerate() {
        let cdir = dir.join(d);
        std::fs::create_dir_all(cdir.join("src")).map_err(|e| e.to_string())?;
        let mut m = format!(
            "[package]\nname = \"{name}\"\nversion = \"0.0.0\"\nedition = \"2024\"\npublish = false\n\n{extra}\n"
        );
        if i == 0 {
            m.push_str("\n[workspace]\n");
        }
        std::fs::write(cdir.join("Cargo.toml"), m).map_err(|e| e.to_string())?;
        std::fs::write(cdir.join("src/lib.rs"), "").map_err(|e| e.to_string())?;
    }
    Ok(dir.join(case.crates[0].0).join("Cargo.toml"))
}

/// Runs every planted workspace and manifest; returns problems.
pub fn selftest(base: &Path) -> Result<Vec<String>, String> {
    let mut problems = Vec::new();
    for case in CASES {
        let manifest = write_case(base, case)?;
        let roots = [case.crates[0].1];
        let v = check_workspace(Some(&manifest), &roots, TRIPLES)?;
        problems.extend(super::unsafe_guard::judge(
            "network",
            case.name,
            &v,
            case.marker,
        ));
    }
    for (name, text, marker) in MANIFEST_CASES {
        let v = check_manifest("Cargo.toml", text)?;
        problems.extend(super::unsafe_guard::judge(
            "network-manifest",
            name,
            &v,
            *marker,
        ));
    }
    Ok(problems)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framework_names() {
        assert!(is_framework("tauri"));
        assert!(is_framework("tauri-plugin-dialog"));
        assert!(is_framework("wry"));
        assert!(is_framework("webkit2gtk-sys"));
        assert!(!is_framework("auto-crop-engine"));
        assert!(!is_framework("tokio"));
    }

    #[test]
    fn planted_workspaces_and_manifests_behave() {
        let base = std::env::temp_dir().join(format!("auto-crop-net-guard-{}", std::process::id()));
        let r = selftest(&base);
        let _ = std::fs::remove_dir_all(&base);
        let problems = r.unwrap();
        assert!(problems.is_empty(), "{problems:#?}");
    }

    #[test]
    fn the_real_manifests_are_clean() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let v = check_manifests(&root).unwrap();
        assert!(v.is_empty(), "{v:#?}");
    }

    #[test]
    fn the_real_shipped_graph_is_clean() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../Cargo.toml");
        let v = check_workspace(Some(&root), SHIPPED_ROOTS, TRIPLES).unwrap();
        assert!(v.is_empty(), "{v:#?}");
    }
}
