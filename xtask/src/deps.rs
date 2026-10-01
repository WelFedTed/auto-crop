// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `cargo xtask check-deps`: dependency-direction rules (ROADMAP M0.30).
//!
//! * Only `auto-crop-shell` may depend on `tauri*` or `wry`.
//! * `auto-crop-core` may only have `serde` and `thiserror` as normal dependencies.
//! * `auto-crop-imgproc` must not depend on an I/O or codec crate.
//! * `auto-crop-cli` must not depend on `auto-crop-shell`.

use std::process::Command;

/// One workspace package and the names of its *normal* dependencies.
#[derive(Debug, Clone)]
pub struct Pkg {
    pub name: String,
    pub normal: Vec<String>,
    /// Normal, dev and build dependencies together.
    pub all: Vec<String>,
}

const CODEC_OR_IO: &[&str] = &[
    "auto-crop-codecs",
    "image",
    "png",
    "tiff",
    "jpeg-decoder",
    "turbojpeg",
    "libheif-rs",
    "libheif-sys",
    "webp",
    "ravif",
    "resvg",
    "rawler",
    "pdfium-render",
];

fn is_codec_or_io(dep: &str) -> bool {
    CODEC_OR_IO.contains(&dep) || dep.starts_with("zune-") || dep.starts_with("libheif")
}

fn is_tauri(dep: &str) -> bool {
    dep == "tauri" || dep.starts_with("tauri-") || dep == "wry"
}

/// Returns a list of human-readable violations.
pub fn check(pkgs: &[Pkg]) -> Vec<String> {
    let mut out = Vec::new();
    for p in pkgs {
        if p.name != "auto-crop-shell" {
            for d in &p.all {
                if is_tauri(d) {
                    out.push(format!(
                        "{} depends on `{d}`; only auto-crop-shell may depend on Tauri",
                        p.name
                    ));
                }
            }
        }
        match p.name.as_str() {
            "auto-crop-core" => {
                for d in &p.normal {
                    if d != "serde" && d != "thiserror" {
                        out.push(format!("auto-crop-core has normal dependency `{d}`; only serde and thiserror are allowed"));
                    }
                }
            }
            "auto-crop-imgproc" => {
                for d in &p.all {
                    if is_codec_or_io(d) {
                        out.push(format!(
                            "auto-crop-imgproc depends on I/O or codec crate `{d}`"
                        ));
                    }
                }
            }
            "auto-crop-cli" if p.all.iter().any(|d| d == "auto-crop-shell") => {
                out.push("auto-crop-cli depends on auto-crop-shell".to_owned());
            }
            _ => {}
        }
    }
    out
}

fn load() -> Result<Vec<Pkg>, String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let output = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .map_err(|e| format!("cannot run cargo metadata: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    parse(&output.stdout)
}

pub fn parse(json: &[u8]) -> Result<Vec<Pkg>, String> {
    let v: serde_json::Value = serde_json::from_slice(json).map_err(|e| e.to_string())?;
    let packages = v["packages"].as_array().ok_or("no packages array")?;
    let mut out = Vec::new();
    for p in packages {
        let name = p["name"].as_str().ok_or("package without name")?.to_owned();
        let mut normal = Vec::new();
        let mut all = Vec::new();
        for d in p["dependencies"].as_array().into_iter().flatten() {
            let dn = d["name"].as_str().unwrap_or_default().to_owned();
            if d["kind"].is_null() {
                normal.push(dn.clone());
            }
            all.push(dn);
        }
        out.push(Pkg { name, normal, all });
    }
    Ok(out)
}

pub fn run(_args: &[String]) -> Result<(), String> {
    let pkgs = load()?;
    let violations = check(&pkgs);
    if violations.is_empty() {
        println!(
            "check-deps: {} workspace packages, no violations",
            pkgs.len()
        );
        Ok(())
    } else {
        Err(violations.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg(name: &str, normal: &[&str]) -> Pkg {
        let normal: Vec<String> = normal.iter().map(|s| (*s).to_owned()).collect();
        Pkg {
            name: name.to_owned(),
            all: normal.clone(),
            normal,
        }
    }

    #[test]
    fn clean_workspace_passes() {
        let pkgs = vec![
            pkg("auto-crop-core", &["serde", "thiserror"]),
            pkg("auto-crop-imgproc", &[]),
            pkg("auto-crop-engine", &["auto-crop-core"]),
            pkg("auto-crop-cli", &["auto-crop-engine"]),
            pkg("auto-crop-shell", &["auto-crop-engine", "tauri"]),
        ];
        assert!(check(&pkgs).is_empty());
    }

    #[test]
    fn planted_tauri_in_engine_fails() {
        let v = check(&[pkg("auto-crop-engine", &["tauri"])]);
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v[0].contains("only auto-crop-shell"));
    }

    #[test]
    fn extra_core_dependency_fails() {
        assert_eq!(
            check(&[pkg("auto-crop-core", &["serde", "image"])]).len(),
            1
        );
    }

    #[test]
    fn imgproc_codec_dependency_fails() {
        assert_eq!(check(&[pkg("auto-crop-imgproc", &["zune-jpeg"])]).len(), 1);
        assert_eq!(
            check(&[pkg("auto-crop-imgproc", &["auto-crop-codecs"])]).len(),
            1
        );
    }

    #[test]
    fn cli_must_not_use_shell() {
        assert_eq!(
            check(&[pkg("auto-crop-cli", &["auto-crop-shell"])]).len(),
            1
        );
    }

    #[test]
    fn parses_cargo_metadata_shape() {
        let json = br#"{"packages":[{"name":"a","dependencies":[{"name":"serde","kind":null},{"name":"proptest","kind":"dev"}]}]}"#;
        let pkgs = parse(json).unwrap();
        assert_eq!(pkgs[0].normal, vec!["serde"]);
        assert_eq!(pkgs[0].all, vec!["serde", "proptest"]);
    }
}
