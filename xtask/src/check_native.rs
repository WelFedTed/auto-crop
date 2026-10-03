// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `cargo xtask check-native [--prefix <dir>]` (ROADMAP M0.42, M0.45).
//!
//! Inspects the libraries built by `build-native` (default prefix
//! `target/native/prefix`):
//! * no `x265_` / `x264_` symbols anywhere (GPL encoders must never be linked, B12);
//! * every dependency of every shared library is on `packaging/allowed-libs.txt`;
//! * libjpeg-turbo's `jconfig.h` reports at least version 3.1.4 (CVE floor);
//! * no `embedded-libheif` feature anywhere in the Cargo manifests.

use goblin::Object;
use std::fs;
use std::path::{Path, PathBuf};

const ALLOWED: &str = "packaging/allowed-libs.txt";
/// Symbol prefixes that must never appear in a built library: the GPL encoders (B12) and the
/// encoders of the AV1 family, which the decode-only build leaves out (libaom's and rav1e's
/// encoder entry points, SVT-AV1's encoder handle). dav1d's own `dav1d_` symbols are expected.
const FORBIDDEN: &[&str] = &[
    "x265_",
    "x264_",
    "aom_codec_av1_cx",
    "rav1e_context_new",
    "svt_av1_enc_init",
];
/// libjpeg-turbo 3.1.4 as LIBJPEG_TURBO_VERSION_NUMBER.
const JPEG_TURBO_FLOOR: u32 = 3_001_004;

/// Forbidden symbol markers present in the bytes.
pub fn forbidden_symbols(bytes: &[u8]) -> Vec<&'static str> {
    FORBIDDEN
        .iter()
        .copied()
        .filter(|m| bytes.windows(m.len()).any(|w| w == m.as_bytes()))
        .collect()
}

/// Names a binary depends on (lower case, base name only).
pub fn libs_needed(bytes: &[u8]) -> Result<Vec<String>, String> {
    let names: Vec<String> = match Object::parse(bytes).map_err(|e| e.to_string())? {
        Object::Elf(e) => e.libraries.iter().map(|s| (*s).to_owned()).collect(),
        Object::PE(p) => p.libraries.iter().map(|s| (*s).to_owned()).collect(),
        Object::Mach(goblin::mach::Mach::Binary(m)) => m
            .libs
            .iter()
            .filter(|l| **l != "self")
            .map(|s| (*s).to_owned())
            .collect(),
        _ => Vec::new(),
    };
    Ok(names
        .into_iter()
        .map(|n| n.rsplit(['/', '\\']).next().unwrap_or(&n).to_lowercase())
        .collect())
}

/// True for the main libheif library (not a plugin such as `heif-libde265`).
pub fn is_libheif(path: &Path) -> bool {
    let n = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();
    let stem = n.strip_prefix("lib").unwrap_or(&n);
    stem.starts_with("heif") && !stem.starts_with("heif-")
}

/// True when the dependencies of libheif include dav1d (AVIF is read through it).
pub fn links_dav1d(needed: &[String]) -> bool {
    needed
        .iter()
        .any(|n| n.starts_with("dav1d") || n.starts_with("libdav1d"))
}

/// Allow-list lines: one name per line, `#` comments; a trailing `*` matches any suffix.
pub fn is_allowed(name: &str, allow: &[String]) -> bool {
    allow.iter().any(|p| match p.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => name == p,
    })
}

pub fn parse_allow(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.split('#').next().unwrap_or("").trim().to_lowercase())
        .filter(|l| !l.is_empty())
        .collect()
}

/// `#define LIBJPEG_TURBO_VERSION_NUMBER 3002000` -> 3002000.
pub fn jpeg_turbo_version(jconfig: &str) -> Option<u32> {
    jconfig.lines().find_map(|l| {
        let mut it = l.split_whitespace();
        (it.next() == Some("#define") && it.next() == Some("LIBJPEG_TURBO_VERSION_NUMBER"))
            .then(|| it.next())
            .flatten()?
            .parse()
            .ok()
    })
}

fn is_shared_lib(p: &Path) -> bool {
    let n = p
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();
    n.ends_with(".dll") || n.ends_with(".dylib") || n.contains(".so")
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.filter_map(Result::ok) {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if is_shared_lib(&p) {
                out.push(p);
            }
        }
    }
}

fn manifests_with_embedded_libheif(root: &Path, hits: &mut Vec<String>) {
    if let Ok(rd) = fs::read_dir(root) {
        for e in rd.filter_map(Result::ok) {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if p.is_dir() {
                if !["target", ".git", "node_modules", "docs", "research"].contains(&name.as_str())
                {
                    manifests_with_embedded_libheif(&p, hits);
                }
            } else if name == "Cargo.toml"
                && fs::read_to_string(&p).is_ok_and(|t| t.contains("embedded-libheif"))
            {
                hits.push(p.display().to_string());
            }
        }
    }
}

pub fn run(args: &[String]) -> Result<(), String> {
    let prefix = args
        .iter()
        .position(|a| a == "--prefix")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "target/native/prefix".to_owned());
    let prefix = PathBuf::from(prefix);
    let allow = parse_allow(&fs::read_to_string(ALLOWED).map_err(|e| format!("{ALLOWED}: {e}"))?);
    let mut libs = Vec::new();
    walk(&prefix, &mut libs);
    if libs.is_empty() {
        return Err(format!(
            "no shared libraries found under {} (run build-native first)",
            prefix.display()
        ));
    }
    let mut problems = Vec::new();
    for lib in &libs {
        let bytes = fs::read(lib).map_err(|e| format!("{}: {e}", lib.display()))?;
        for m in forbidden_symbols(&bytes) {
            problems.push(format!(
                "{}: contains forbidden symbols `{m}*` (GPL encoder linked?)",
                lib.display()
            ));
        }
        match libs_needed(&bytes) {
            Ok(needed) => {
                println!("{}: needs {}", lib.display(), needed.join(", "));
                if is_libheif(lib) {
                    // AVIF is read through dav1d (decision B12, PLAN 3.4.2): a libheif that does
                    // not link it would silently drop AVIF support.
                    if links_dav1d(&needed) {
                        println!("{}: links dav1d (AVIF)", lib.display());
                    } else {
                        problems.push(format!(
                            "{}: libheif does not link dav1d (build with WITH_DAV1D=ON)",
                            lib.display()
                        ));
                    }
                }
                for n in needed.iter().filter(|n| !is_allowed(n, &allow)) {
                    problems.push(format!(
                        "{}: dependency `{n}` is not on {ALLOWED}",
                        lib.display()
                    ));
                }
            }
            Err(e) => println!("{}: not inspected ({e})", lib.display()),
        }
    }
    // libjpeg-turbo version floor
    let jconfig = prefix.join("include/jconfig.h");
    match fs::read_to_string(&jconfig)
        .ok()
        .and_then(|t| jpeg_turbo_version(&t))
    {
        Some(v) if v >= JPEG_TURBO_FLOOR => {
            println!("libjpeg-turbo version number {v} >= {JPEG_TURBO_FLOOR}")
        }
        Some(v) => problems.push(format!(
            "libjpeg-turbo version number {v} is below the floor {JPEG_TURBO_FLOOR} (3.1.4)"
        )),
        None => problems.push(format!(
            "cannot read LIBJPEG_TURBO_VERSION_NUMBER from {}",
            jconfig.display()
        )),
    }
    let mut embedded = Vec::new();
    manifests_with_embedded_libheif(Path::new("."), &mut embedded);
    for m in embedded {
        problems.push(format!(
            "{m}: uses `embedded-libheif` (stale, pulls vcpkg/x265); use our own build"
        ));
    }
    if problems.is_empty() {
        println!("check-native: {} libraries clean", libs.len());
        Ok(())
    } else {
        Err(problems.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_forbidden_symbols() {
        assert_eq!(forbidden_symbols(b"..x265_encoder_open.."), vec!["x265_"]);
        assert_eq!(forbidden_symbols(b"x264_encoder_open x265_param").len(), 2);
        assert!(forbidden_symbols(b"libde265 decoder only").is_empty());
    }

    #[test]
    fn av1_encoders_are_forbidden_but_the_dav1d_decoder_is_fine() {
        assert_eq!(
            forbidden_symbols(b"..aom_codec_av1_cx.."),
            vec!["aom_codec_av1_cx"]
        );
        assert_eq!(forbidden_symbols(b"rav1e_context_new").len(), 1);
        assert_eq!(forbidden_symbols(b"svt_av1_enc_init_handle").len(), 1);
        assert!(forbidden_symbols(b"dav1d_open dav1d_send_data dav1d_get_picture").is_empty());
    }

    #[test]
    fn libheif_is_told_from_its_plugins_and_must_link_dav1d() {
        for yes in [
            "heif.dll",
            "libheif.so.1.23.5",
            "libheif.1.dylib",
            "libheif.so",
        ] {
            assert!(is_libheif(Path::new(yes)), "{yes}");
        }
        for no in [
            "heif-libde265.dll",
            "libheif-libde265.so",
            "libde265.so.0",
            "turbojpeg.dll",
        ] {
            assert!(!is_libheif(Path::new(no)), "{no}");
        }
        assert!(links_dav1d(&["dav1d.dll".into(), "kernel32.dll".into()]));
        assert!(links_dav1d(&["libdav1d.so.7".into()]));
        assert!(links_dav1d(&["libdav1d.7.dylib".into()]));
        assert!(!links_dav1d(&["libde265.so.0".into(), "libc.so.6".into()]));
        // dav1d is on the allow-list (system libraries only otherwise).
        let allow = parse_allow(&std::fs::read_to_string("../packaging/allowed-libs.txt").unwrap());
        for ok in ["dav1d.dll", "libdav1d.so.7", "libdav1d.7.dylib"] {
            assert!(is_allowed(ok, &allow), "{ok}");
        }
        assert!(!is_allowed("libaom.so.3", &allow));
        assert!(!is_allowed("libx265.so.199", &allow));
    }

    #[test]
    fn allow_list_supports_prefix_wildcards() {
        let allow = parse_allow("# system\nkernel32.dll\napi-ms-win-crt-*\nlibc.so.6 # glibc\n");
        assert!(is_allowed("kernel32.dll", &allow));
        assert!(is_allowed("api-ms-win-crt-runtime-l1-1-0.dll", &allow));
        assert!(is_allowed("libc.so.6", &allow));
        assert!(!is_allowed("libx265.so.199", &allow));
    }

    #[test]
    fn reads_the_jpeg_turbo_version_number() {
        assert_eq!(
            jpeg_turbo_version("#define LIBJPEG_TURBO_VERSION_NUMBER 3002000\n"),
            Some(3_002_000)
        );
        assert_eq!(jpeg_turbo_version("#define OTHER 1\n"), None);
    }

    #[test]
    fn dev_only_x265_in_a_fake_library_fails() {
        // A planted binary containing an x265 symbol name must be flagged.
        let bytes = b"\x7fELF-not-real x265_encoder_encode";
        assert_eq!(forbidden_symbols(bytes), vec!["x265_"]);
    }
}
