// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `cargo xtask build-native [--only a,b] [--target <triple>]` (ROADMAP M0.39).
//!
//! Reads `native-deps.toml`, fetches every archive with `curl`, **refuses any
//! download whose SHA-256 differs from the pin**, extracts it with `tar` and builds
//! the `build = "cmake"` entries into `target/native/prefix`. Build order follows
//! the dependency order: libde265, libjpeg-turbo, libheif.

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

const MANIFEST: &str = "native-deps.toml";
const BANNED: &[&str] = &["x265", "x264", "kvazaar", "libx265", "libx264"];
/// Build order for the entries this command knows how to build.
const ORDER: &[&str] = &["libde265", "libjpeg-turbo", "libheif"];

#[derive(Debug, Clone, Deserialize)]
pub struct Lib {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub min_version: String,
    pub url: String,
    pub sha256: String,
    pub license: String,
    pub kind: String,
    pub build: String,
    #[serde(default)]
    pub flags: Vec<String>,
    /// Upstream GitHub repository (`owner/name`) checked by `native-watch`.
    #[serde(default)]
    pub watch: String,
    /// Restrict `native-watch` to one release line such as `1.28`.
    #[serde(default)]
    pub watch_line: String,
    /// `releases` (default) or `tags` for upstreams whose GitHub mirror has no real releases.
    #[serde(default)]
    pub watch_source: String,
}

#[derive(Deserialize)]
struct Manifest {
    lib: Vec<Lib>,
}

pub fn parse(text: &str) -> Result<Vec<Lib>, String> {
    toml::from_str::<Manifest>(text)
        .map(|m| m.lib)
        .map_err(|e| format!("{MANIFEST}: {e}"))
}

fn ver_parts(v: &str) -> Vec<u64> {
    v.split('.')
        .map(|p| {
            p.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .unwrap_or(0)
        })
        .collect()
}

/// True if version `a` >= `b` (numeric, dot separated).
pub fn version_ge(a: &str, b: &str) -> bool {
    let (mut x, mut y) = (ver_parts(a), ver_parts(b));
    let n = x.len().max(y.len());
    x.resize(n, 0);
    y.resize(n, 0);
    x >= y
}

/// Policy checks on the manifest; returns violations.
pub fn validate(libs: &[Lib]) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for l in libs {
        if !seen.insert(l.name.clone()) {
            out.push(format!("duplicate entry `{}`", l.name));
        }
        if l.sha256.len() != 64 || !l.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
            out.push(format!("{}: sha256 must be 64 hex characters", l.name));
        }
        if !l.url.starts_with("https://") {
            out.push(format!("{}: url must be https", l.name));
        }
        if !l.min_version.is_empty() && !version_ge(&l.version, &l.min_version) {
            out.push(format!(
                "{}: version {} is below the floor {}",
                l.name, l.version, l.min_version
            ));
        }
        if BANNED.iter().any(|b| l.name.to_lowercase().contains(b)) {
            out.push(format!(
                "{}: banned (GPL encoder libraries must never be linked, B12)",
                l.name
            ));
        }
        if l.license.contains("GPL") && !l.license.contains("LGPL") {
            out.push(format!(
                "{}: licence `{}` is not allowed",
                l.name, l.license
            ));
        }
        if !["source", "binary"].contains(&l.kind.as_str())
            || !["cmake", "meson", "none"].contains(&l.build.as_str())
        {
            out.push(format!("{}: bad kind or build", l.name));
        }
    }
    out
}

pub fn sha256_file(path: &Path) -> Result<String, String> {
    let mut f = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Refuses (Err) when the file's SHA-256 differs from `expected`.
pub fn verify(path: &Path, expected: &str) -> Result<(), String> {
    let got = sha256_file(path)?;
    if got.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(format!(
            "REFUSED {}: sha256 {got} does not match the pin {expected}",
            path.display()
        ))
    }
}

/// `canonicalize` returns verbatim `\\?\` paths on Windows, which CMake and MSVC mishandle
/// (include directories silently go missing); strip the prefix.
pub fn plain_path(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy().into_owned();
    match s.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => p,
    }
}

fn run_cmd(cmd: &mut Command, what: &str) -> Result<(), String> {
    let status = cmd
        .status()
        .map_err(|e| format!("{what}: cannot run: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{what} failed with {status}"))
    }
}

fn download(url: &str, dest: &Path) -> Result<(), String> {
    run_cmd(
        Command::new("curl")
            .args(["-fsSL", "--retry", "3", "-o"])
            .arg(dest)
            .arg(url),
        &format!("curl {url}"),
    )
}

fn extract(archive: &Path, into: &Path) -> Result<PathBuf, String> {
    fs::create_dir_all(into).map_err(|e| e.to_string())?;
    run_cmd(
        Command::new("tar")
            .arg("-xf")
            .arg(archive)
            .arg("-C")
            .arg(into),
        "tar",
    )?;
    let mut dirs: Vec<_> = fs::read_dir(into)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .filter(|d| d.path().is_dir())
        .collect();
    if dirs.len() == 1 {
        Ok(dirs.remove(0).path())
    } else {
        Err(format!(
            "expected one top-level directory in {}",
            archive.display()
        ))
    }
}

fn build_cmake(lib: &Lib, src: &Path, build: &Path, prefix: &Path) -> Result<(), String> {
    let mut cfg = Command::new("cmake");
    cfg.arg("-S").arg(src).arg("-B").arg(build);
    cfg.arg("-DCMAKE_BUILD_TYPE=Release");
    cfg.arg(format!("-DCMAKE_INSTALL_PREFIX={}", prefix.display()));
    cfg.arg(format!("-DCMAKE_PREFIX_PATH={}", prefix.display()));
    cfg.args(&lib.flags);
    run_cmd(&mut cfg, &format!("cmake configure {}", lib.name))?;
    let mut b = Command::new("cmake");
    b.arg("--build")
        .arg(build)
        .args(["--config", "Release", "--parallel"]);
    // Memory-limited runners (for example the Linux ARM64 CI runner) cap the job count.
    if let Ok(jobs) = std::env::var("AUTOCROP_BUILD_JOBS")
        && !jobs.is_empty()
    {
        b.arg(jobs);
    }
    run_cmd(&mut b, &format!("cmake build {}", lib.name))?;
    run_cmd(
        Command::new("cmake")
            .arg("--install")
            .arg(build)
            .args(["--config", "Release"]),
        &format!("cmake install {}", lib.name),
    )
}

pub fn run(args: &[String]) -> Result<(), String> {
    let only: Option<Vec<String>> = args
        .iter()
        .position(|a| a == "--only")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.split(',').map(str::to_owned).collect());
    if let Some(t) = args
        .iter()
        .position(|a| a == "--target")
        .and_then(|i| args.get(i + 1))
    {
        println!(
            "build-native: --target {t} noted; cross builds are added with the arm64 jobs (host build only for now)"
        );
    }
    let text = fs::read_to_string(MANIFEST).map_err(|e| format!("{MANIFEST}: {e}"))?;
    let libs = parse(&text)?;
    let problems = validate(&libs);
    if !problems.is_empty() {
        return Err(problems.join("\n"));
    }
    let root = PathBuf::from("target/native");
    let prefix = plain_path(
        fs::canonicalize({
            fs::create_dir_all(root.join("prefix")).map_err(|e| e.to_string())?;
            root.join("prefix")
        })
        .map_err(|e| e.to_string())?,
    );
    for name in ORDER {
        if only.as_ref().is_some_and(|o| !o.iter().any(|n| n == name)) {
            continue;
        }
        let lib = libs
            .iter()
            .find(|l| l.name == *name)
            .ok_or(format!("{name} missing from {MANIFEST}"))?;
        let work = root.join(format!("{}-{}", lib.name, lib.version));
        let archive = work.join("source-archive");
        fs::create_dir_all(&work).map_err(|e| e.to_string())?;
        println!("== {} {}: fetching", lib.name, lib.version);
        if !archive.exists() || verify(&archive, &lib.sha256).is_err() {
            download(&lib.url, &archive)?;
        }
        verify(&archive, &lib.sha256)?; // a wrong hash is refused
        let src_parent = work.join("src");
        let _ = fs::remove_dir_all(&src_parent);
        let src = extract(&archive, &src_parent)?;
        println!("== {}: building (cmake)", lib.name);
        build_cmake(lib, &src, &work.join("build"), &prefix)?;
    }
    println!("build-native: done; prefix {}", prefix.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lib(name: &str, version: &str, min: &str, sha: &str) -> Lib {
        Lib {
            name: name.into(),
            version: version.into(),
            min_version: min.into(),
            url: "https://example.org/x.tar.gz".into(),
            sha256: sha.into(),
            license: "MIT".into(),
            kind: "source".into(),
            build: "cmake".into(),
            flags: vec![],
            watch: String::new(),
            watch_line: String::new(),
            watch_source: String::new(),
        }
    }

    const GOOD: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn version_floor_compares_numerically() {
        assert!(version_ge("3.2.0", "3.1.4"));
        assert!(version_ge("1.23.5", "1.23.5"));
        assert!(!version_ge("3.1.0", "3.1.4"));
        assert!(version_ge("1.10.0", "1.9.9"));
    }

    #[test]
    fn validation_catches_bad_entries() {
        assert!(validate(&[lib("a", "1.0.0", "1.0.0", GOOD)]).is_empty());
        assert_eq!(validate(&[lib("a", "1.0.0", "1.0.0", "short")]).len(), 1);
        assert_eq!(validate(&[lib("a", "3.1.0", "3.1.4", GOOD)]).len(), 1);
        assert_eq!(validate(&[lib("x265", "1.0.0", "", GOOD)]).len(), 1);
        let mut gpl = lib("b", "1.0.0", "", GOOD);
        gpl.license = "GPL-3.0-only".into();
        assert_eq!(validate(&[gpl]).len(), 1);
        assert_eq!(
            validate(&[lib("a", "1.0.0", "", GOOD), lib("a", "1.0.0", "", GOOD)]).len(),
            1
        );
    }

    #[test]
    fn verbatim_windows_prefix_is_stripped() {
        assert_eq!(
            plain_path(PathBuf::from(r"\\?\D:\a\b")),
            PathBuf::from(r"D:\a\b")
        );
        assert_eq!(
            plain_path(PathBuf::from("/home/x")),
            PathBuf::from("/home/x")
        );
    }

    #[test]
    fn a_wrong_hash_is_refused() {
        let p = std::env::temp_dir().join("auto-crop-native-test.bin");
        fs::write(&p, b"hello").unwrap();
        // sha256("hello")
        let ok = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        assert!(verify(&p, ok).is_ok());
        let err = verify(&p, GOOD).unwrap_err();
        assert!(err.starts_with("REFUSED"), "{err}");
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn the_real_manifest_is_valid_and_has_the_security_floors() {
        let libs = parse(&fs::read_to_string("../native-deps.toml").unwrap()).unwrap();
        assert!(validate(&libs).is_empty(), "{:?}", validate(&libs));
        for (name, floor) in [
            ("libheif", "1.23.5"),
            ("libde265", "1.1.3"),
            ("libjpeg-turbo", "3.1.4"),
        ] {
            let l = libs.iter().find(|l| l.name == name).expect(name);
            assert!(
                version_ge(&l.version, floor) && l.min_version == floor,
                "{name}"
            );
        }
        for name in ["dav1d", "libjxl", "libwebp"] {
            assert!(libs.iter().any(|l| l.name == name), "{name}");
        }
        assert!(libs.iter().any(|l| l.name.starts_with("onnxruntime")));
    }
}
