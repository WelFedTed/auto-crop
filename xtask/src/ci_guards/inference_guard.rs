// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The inference-runtime guard (ROADMAP M1.55; ADR-0007, ADR-0004, B2, B12).
//!
//! The network guard bounds *normal* edges of the shipped graph, but `ort` pulls its network
//! machinery (`ureq`, TLS) only as a **build** dependency of `ort-sys`, switched on by the
//! `download-binaries` and `tls-*` features. Those features also mean "link an unpinned runtime
//! downloaded at build time", which the hash policy (D4) forbids. So the manifests are checked:
//!
//! * `ort` is pinned exactly (`=`), has `default-features = false`, enables `load-dynamic` and no
//!   feature outside {`std`, `load-dynamic`, `tracing`, `ndarray`, `api-*`}: nothing that downloads,
//!   copies libraries, links statically or turns on a GPU provider (those need their own ADR).
//! * `ort-sys` is never named directly; `tract-*` is never named (ADR-0007 rejected it).
//! * Only `crates/infer` may name `ort`, `rten` or `rten-tensor` (the `InferenceBackend` trait is
//!   the seam; `cargo xtask check-deps` enforces the same on the resolved graph).

/// The only manifest that may name the inference runtimes.
pub const INFER_MANIFEST: &str = "crates/infer/Cargo.toml";

const RUNTIME_CRATES: &[&str] = &["ort", "rten", "rten-tensor"];

fn feature_allowed(f: &str) -> bool {
    matches!(f, "std" | "load-dynamic" | "tracing" | "ndarray") || f.starts_with("api-")
}

fn dep_tables(doc: &toml::Table) -> Vec<&toml::Table> {
    let kinds = ["dependencies", "dev-dependencies", "build-dependencies"];
    let mut tables = Vec::new();
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
    tables
}

/// Checks one first-party manifest (`path` is workspace-relative with `/`).
pub fn check_manifest(path: &str, text: &str) -> Result<Vec<String>, String> {
    let doc: toml::Table = text.parse().map_err(|e| format!("{path}: {e}"))?;
    let mut out = Vec::new();
    for t in dep_tables(&doc) {
        for (key, val) in t {
            let spec = val.as_table();
            let real = spec
                .and_then(|d| d.get("package"))
                .and_then(|p| p.as_str())
                .unwrap_or(key);
            if real.starts_with("tract-") || real == "tract" {
                out.push(format!(
                    "{path}: dependency `{real}`: tract is rejected (ADR-0007: wrong int8 results, slowest, largest)"
                ));
            }
            if real == "ort-sys" {
                out.push(format!(
                    "{path}: dependency `ort-sys` must not be named directly; use `ort` through crates/infer (ADR-0007)"
                ));
            }
            if RUNTIME_CRATES.contains(&real) && path != INFER_MANIFEST {
                out.push(format!(
                    "{path}: dependency `{real}` outside {INFER_MANIFEST}; the inference runtimes live behind the InferenceBackend trait only (ADR-0007)"
                ));
            }
            if real != "ort" {
                continue;
            }
            let Some(spec) = spec else {
                out.push(format!(
                    "{path}: dependency `ort` needs a table with an exact version, default-features = false and the load-dynamic feature (ADR-0007)"
                ));
                continue;
            };
            let version = spec.get("version").and_then(|v| v.as_str()).unwrap_or("");
            if !version.starts_with('=') {
                out.push(format!(
                    "{path}: dependency `ort` must be pinned with an exact `=` version (a release candidate; ADR-0007), got `{version}`"
                ));
            }
            if spec.get("default-features").and_then(|v| v.as_bool()) != Some(false) {
                out.push(format!(
                    "{path}: dependency `ort` must set default-features = false: its defaults are download-binaries, tls-native and copy-dylibs (an unpinned runtime fetched at build time; ADR-0007 finding 3)"
                ));
            }
            let features: Vec<&str> = spec
                .get("features")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|f| f.as_str()).collect())
                .unwrap_or_default();
            if !features.contains(&"load-dynamic") {
                out.push(format!(
                    "{path}: dependency `ort` must enable `load-dynamic` (our SHA-256-pinned runtime, loaded by absolute path)"
                ));
            }
            for f in features.iter().filter(|f| !feature_allowed(f)) {
                out.push(format!(
                    "{path}: dependency `ort` feature `{f}` is not allowed (only std, load-dynamic, tracing, ndarray, api-*): downloads, static linking and execution providers need their own ADR"
                ));
            }
        }
    }
    Ok(out)
}

/// Checks the root manifest, `crates/*/Cargo.toml` and `xtask/Cargo.toml`.
pub fn check_manifests(root: &std::path::Path) -> Result<Vec<String>, String> {
    let mut paths = vec![root.join("Cargo.toml"), root.join("xtask/Cargo.toml")];
    if let Ok(rd) = std::fs::read_dir(root.join("crates")) {
        let mut crates: Vec<_> = rd.filter_map(Result::ok).map(|d| d.path()).collect();
        crates.sort();
        paths.extend(crates.into_iter().map(|c| c.join("Cargo.toml")));
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

/// Planted manifests: (name, manifest path, text, marker). `None` = a control that must pass.
pub const CASES: &[(&str, &str, &str, Option<&str>)] = &[
    (
        "control-the-real-ort-spec",
        "crates/infer/Cargo.toml",
        "[dependencies]\nort = { version = \"=2.0.0-rc.13\", optional = true, default-features = false, features = [\"std\", \"load-dynamic\"] }\nrten = { version = \"=0.26.0\", optional = true }\n",
        None,
    ),
    (
        "ort-with-default-features",
        "crates/infer/Cargo.toml",
        "[dependencies]\nort = { version = \"=2.0.0-rc.13\", features = [\"load-dynamic\"] }\n",
        Some("default-features = false"),
    ),
    (
        "ort-bare-version-string",
        "crates/infer/Cargo.toml",
        "[dependencies]\nort = \"=2.0.0-rc.13\"\n",
        Some("needs a table"),
    ),
    (
        "ort-download-binaries",
        "crates/infer/Cargo.toml",
        "[dependencies]\nort = { version = \"=2.0.0-rc.13\", default-features = false, features = [\"load-dynamic\", \"download-binaries\"] }\n",
        Some("`download-binaries` is not allowed"),
    ),
    (
        "ort-tls-feature",
        "crates/infer/Cargo.toml",
        "[dependencies]\nort = { version = \"=2.0.0-rc.13\", default-features = false, features = [\"load-dynamic\", \"tls-native\"] }\n",
        Some("`tls-native` is not allowed"),
    ),
    (
        "ort-gpu-provider-feature",
        "crates/infer/Cargo.toml",
        "[dependencies]\nort = { version = \"=2.0.0-rc.13\", default-features = false, features = [\"load-dynamic\", \"cuda\"] }\n",
        Some("`cuda` is not allowed"),
    ),
    (
        "ort-without-load-dynamic",
        "crates/infer/Cargo.toml",
        "[dependencies]\nort = { version = \"=2.0.0-rc.13\", default-features = false, features = [\"std\"] }\n",
        Some("must enable `load-dynamic`"),
    ),
    (
        "ort-unpinned",
        "crates/infer/Cargo.toml",
        "[dependencies]\nort = { version = \"2.0.0-rc.13\", default-features = false, features = [\"load-dynamic\"] }\n",
        Some("exact `=` version"),
    ),
    (
        "ort-renamed-package",
        "crates/infer/Cargo.toml",
        "[dependencies]\nonnx = { package = \"ort\", version = \"=2.0.0-rc.13\", features = [\"load-dynamic\"] }\n",
        Some("default-features = false"),
    ),
    (
        "ort-in-another-crate",
        "crates/engine/Cargo.toml",
        "[dependencies]\nort = { version = \"=2.0.0-rc.13\", default-features = false, features = [\"load-dynamic\"] }\n",
        Some("outside crates/infer/Cargo.toml"),
    ),
    (
        "rten-in-another-crate",
        "crates/engine/Cargo.toml",
        "[dependencies]\nrten = \"=0.26.0\"\n",
        Some("outside crates/infer/Cargo.toml"),
    ),
    (
        "ort-sys-directly",
        "crates/infer/Cargo.toml",
        "[dependencies]\nort-sys = \"=2.0.0-rc.13\"\n",
        Some("`ort-sys` must not be named directly"),
    ),
    (
        "tract",
        "crates/infer/Cargo.toml",
        "[dependencies]\ntract-onnx = \"=0.23.8\"\n",
        Some("tract is rejected"),
    ),
];

pub fn selftest() -> Vec<String> {
    let mut problems = Vec::new();
    for (name, path, text, marker) in CASES {
        match check_manifest(path, text) {
            Ok(v) => problems.extend(super::unsafe_guard::judge(
                "inference-manifest",
                name,
                &v,
                *marker,
            )),
            Err(e) => problems.push(format!("inference-manifest/{name}: {e}")),
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planted_manifests_behave() {
        let problems = selftest();
        assert!(problems.is_empty(), "{problems:#?}");
    }

    #[test]
    fn the_real_manifests_are_clean() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let v = check_manifests(&root).unwrap();
        assert!(v.is_empty(), "{v:#?}");
    }
}
