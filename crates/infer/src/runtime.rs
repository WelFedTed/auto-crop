// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Finding and verifying the ONNX Runtime library (ADR-0007, finding 4).
//!
//! `ort` with `load-dynamic` will happily call `LoadLibrary("onnxruntime.dll")`, and Windows ships a
//! system `onnxruntime.dll` of another version (the spike picked it up). Loading a library runs its
//! initialisation code, so a wrong or planted library is not just a version-skew risk. The rules
//! here, enforced **before** anything is loaded:
//!
//! 1. The path must be **absolute**. A bare name, a relative path and anything that would go through
//!    a search path is refused ([`RuntimeError::NotAbsolute`]).
//! 2. The file's SHA-256 must equal the pin of the library inside the archive pinned in
//!    `native-deps.toml` ([`PINNED_VERSION`], [`PINS`]); anything else, including another ONNX
//!    Runtime version, is refused ([`RuntimeError::WrongRuntime`]). So "version check against the
//!    pin" is exact, not `>=` as in `ort`'s own check (which only needs the minor version to be at
//!    least the one `ort` was built for).
//! 3. Only the verified path is handed to `ort::init_from`; the first load in a process wins and a
//!    later request for another path is refused ([`RuntimeError::AlreadyLoaded`]).
//!
//! Where the library comes from: [`locate`] takes the `AUTOCROP_ORT_DYLIB` variable (development:
//! `cargo xtask fetch-ort` prints the path) and otherwise the file name next to the executable
//! (what the installer layout will ship, M6). Nothing else is searched.
//!
//! The pins in [`PINS`] are the SHA-256 of the runtime library file *inside* the archive, so
//! they are checked against the archive pin by `cargo xtask fetch-ort`, which also refuses to
//! extract an archive whose hash differs from `native-deps.toml` (ADR-0004).

use std::path::{Path, PathBuf};

/// The ONNX Runtime version of the pinned archives (`native-deps.toml`, ADR-0007).
pub const PINNED_VERSION: &str = "1.28.2";

/// Environment variable naming the runtime library for development (an absolute path).
pub const ENV_VAR: &str = "AUTOCROP_ORT_DYLIB";

/// One pinned runtime library.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pin {
    /// `native-deps.toml` entry whose archive holds this library.
    pub native_deps_name: &'static str,
    /// `std::env::consts::OS`.
    pub os: &'static str,
    /// `std::env::consts::ARCH`.
    pub arch: &'static str,
    /// File name of the library (inside the archive's `lib/` directory and next to the exe).
    pub file_name: &'static str,
    /// Lower-case hex SHA-256 of that file.
    pub sha256: &'static str,
}

/// The runtime libraries we can verify. Intel Mac has none (ONNX Runtime ships no `osx-x86_64`
/// binary): rten is the backend there (ADR-0007, finding 5). Windows and Linux ARM64 get pins with
/// their archives in M4 (ADR-0007, finding 6).
pub const PINS: &[Pin] = &[
    Pin {
        native_deps_name: "onnxruntime-win-x64",
        os: "windows",
        arch: "x86_64",
        file_name: "onnxruntime.dll",
        sha256: "1becbd71adbf49609d33195e29c9214969db3c3de69c81425bebe5a4b69aef97",
    },
    Pin {
        native_deps_name: "onnxruntime-linux-x64",
        os: "linux",
        arch: "x86_64",
        file_name: "libonnxruntime.so.1.28.2",
        sha256: "088f24b1fc56714d3efaaeb3ac2ee486a5d7b50ccfbb3dd26fd1a612534a05fb",
    },
    Pin {
        native_deps_name: "onnxruntime-osx-arm64",
        os: "macos",
        arch: "aarch64",
        file_name: "libonnxruntime.1.28.2.dylib",
        sha256: "c986ef16fd63406bc2cee8cc7cc056189b07c587beb9b79d29e31c3cca129e98",
    },
];

/// The pin for this host, if there is a runtime for it.
pub fn pin_for_host() -> Option<&'static Pin> {
    PINS.iter()
        .find(|p| p.os == std::env::consts::OS && p.arch == std::env::consts::ARCH)
}

#[derive(Debug, thiserror::Error, PartialEq, Eq, Clone)]
pub enum RuntimeError {
    #[error(
        "no pinned ONNX Runtime for {os}/{arch} (Intel Mac and ARM64 have none yet): use the rten backend"
    )]
    NoPinForHost {
        os: &'static str,
        arch: &'static str,
    },
    #[error(
        "ONNX Runtime path `{0}` is not absolute: the runtime is loaded by absolute path only, never through a search path"
    )]
    NotAbsolute(String),
    #[error(
        "ONNX Runtime not found: set {ENV_VAR} to the absolute path of `{file}` (run `cargo xtask fetch-ort`) or place it next to the executable (looked at `{looked}`)"
    )]
    NotFound { file: &'static str, looked: String },
    #[error("cannot read `{path}`: {message}")]
    Unreadable { path: String, message: String },
    #[error(
        "REFUSED ONNX Runtime `{path}`: sha256 {got} is not the pinned ONNX Runtime {PINNED_VERSION} library ({expected}); wrong version or not the pinned build"
    )]
    WrongRuntime {
        path: String,
        expected: String,
        got: String,
    },
    #[error(
        "ONNX Runtime was already loaded from `{loaded}`; a process loads one runtime, refusing `{requested}`"
    )]
    AlreadyLoaded { loaded: String, requested: String },
    #[error("ort could not load the verified runtime `{path}`: {message}")]
    Load { path: String, message: String },
}

/// Reduces `path` to an absolute path or refuses it. No search path is ever consulted, so a bare
/// name such as `onnxruntime.dll` is refused even when a file with that name is in the working
/// directory.
pub fn require_absolute(path: &Path) -> Result<PathBuf, RuntimeError> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Err(RuntimeError::NotAbsolute(path.display().to_string()))
    }
}

/// Chooses the runtime path: the variable when it is set and non-empty (it must be absolute), else
/// `file_name` in `exe_dir`. Pure, so it is unit-tested without touching the environment.
pub fn locate(
    env_value: Option<&std::ffi::OsStr>,
    exe_dir: Option<&Path>,
    file_name: &'static str,
) -> Result<PathBuf, RuntimeError> {
    if let Some(v) = env_value.filter(|v| !v.is_empty()) {
        return require_absolute(Path::new(v));
    }
    let dir = exe_dir.ok_or_else(|| RuntimeError::NotFound {
        file: file_name,
        looked: "(the executable's directory is unknown)".to_owned(),
    })?;
    let candidate = dir.join(file_name);
    if candidate.is_file() {
        Ok(candidate)
    } else {
        Err(RuntimeError::NotFound {
            file: file_name,
            looked: candidate.display().to_string(),
        })
    }
}

/// [`locate`] for this process: the real environment, the real executable, this host's file name.
pub fn locate_for_current_process() -> Result<PathBuf, RuntimeError> {
    let pin = pin_for_host().ok_or(RuntimeError::NoPinForHost {
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
    })?;
    let env = std::env::var_os(ENV_VAR);
    let exe = std::env::current_exe().ok();
    locate(
        env.as_deref(),
        exe.as_deref().and_then(Path::parent),
        pin.file_name,
    )
}

/// Hashes the library at `path` and refuses it unless it is `pin`'s library. Returns the
/// absolute path that was checked.
pub fn verify_file(path: &Path, pin: &Pin) -> Result<PathBuf, RuntimeError> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let abs = require_absolute(path)?;
    let shown = abs.display().to_string();
    let unreadable = |e: std::io::Error| RuntimeError::Unreadable {
        path: shown.clone(),
        message: e.to_string(),
    };
    let mut f = std::fs::File::open(&abs).map_err(unreadable)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).map_err(unreadable)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let got = crate::model::hex(&hasher.finalize());
    if got == pin.sha256 {
        Ok(abs)
    } else {
        Err(RuntimeError::WrongRuntime {
            path: shown,
            expected: pin.sha256.to_owned(),
            got,
        })
    }
}

/// [`verify_file`] against this host's pin.
pub fn verify_for_host(path: &Path) -> Result<PathBuf, RuntimeError> {
    let pin = pin_for_host().ok_or(RuntimeError::NoPinForHost {
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
    })?;
    verify_file(path, pin)
}

/// Loads the ONNX Runtime from `path` into this process after [`verify_for_host`] accepted it, and
/// makes it the runtime every `ort` session uses. Calling it again with the same path is a no-op;
/// with another path it is refused. The `ORT_DYLIB_PATH` variable is never consulted: the library
/// is set here, before any other `ort` call.
#[cfg(feature = "ort")]
pub fn load(path: &Path) -> Result<PathBuf, RuntimeError> {
    use std::sync::Mutex;
    static LOADED: Mutex<Option<PathBuf>> = Mutex::new(None);
    let verified = verify_for_host(path)?;
    let mut slot = LOADED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(loaded) = slot.as_ref() {
        return if *loaded == verified {
            Ok(verified)
        } else {
            Err(RuntimeError::AlreadyLoaded {
                loaded: loaded.display().to_string(),
                requested: verified.display().to_string(),
            })
        };
    }
    let builder = ort::init_from(&verified).map_err(|e| RuntimeError::Load {
        path: verified.display().to_string(),
        message: e.to_string(),
    })?;
    builder.with_name("auto-crop").commit();
    *slot = Some(verified.clone());
    Ok(verified)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn the_pins_cover_the_three_required_oses_and_are_well_formed() {
        for os in ["windows", "linux", "macos"] {
            assert!(PINS.iter().any(|p| p.os == os), "{os}");
        }
        for p in PINS {
            assert!(
                crate::model::is_sha256_hex(p.sha256),
                "{}",
                p.native_deps_name
            );
            assert!(p.native_deps_name.starts_with("onnxruntime-"));
            if p.os != "windows" {
                assert!(p.file_name.contains(PINNED_VERSION), "{}", p.file_name);
            }
        }
    }

    #[test]
    fn a_bare_name_or_relative_path_is_never_accepted() {
        for bad in [
            "onnxruntime.dll",
            "libonnxruntime.so",
            "./onnxruntime.dll",
            "lib/x.so",
        ] {
            assert!(
                matches!(
                    require_absolute(Path::new(bad)),
                    Err(RuntimeError::NotAbsolute(_))
                ),
                "{bad}"
            );
            assert!(
                matches!(
                    locate(Some(OsStr::new(bad)), None, "x"),
                    Err(RuntimeError::NotAbsolute(_))
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn the_variable_wins_and_must_be_absolute() {
        let abs = std::env::temp_dir().join("x-runtime");
        assert_eq!(locate(Some(abs.as_os_str()), None, "x"), Ok(abs.clone()));
        // An empty variable counts as unset.
        assert!(matches!(
            locate(Some(OsStr::new("")), None, "x"),
            Err(RuntimeError::NotFound { .. })
        ));
    }

    #[test]
    fn next_to_the_exe_is_used_only_when_the_file_is_there() {
        let dir =
            std::env::temp_dir().join(format!("auto-crop-infer-locate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(matches!(
            locate(None, Some(&dir), "lib-not-there.bin"),
            Err(RuntimeError::NotFound { .. })
        ));
        std::fs::write(dir.join("lib-there.bin"), b"x").unwrap();
        assert_eq!(
            locate(None, Some(&dir), "lib-there.bin"),
            Ok(dir.join("lib-there.bin"))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_is_not_the_pinned_library_is_refused_before_any_load() {
        let dir =
            std::env::temp_dir().join(format!("auto-crop-infer-verify-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("onnxruntime-fake.bin");
        std::fs::write(&fake, b"MZ not an onnx runtime").unwrap();
        let pin = Pin {
            native_deps_name: "onnxruntime-test",
            os: "test",
            arch: "test",
            file_name: "f",
            // sha256("hello")
            sha256: "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
        };
        match verify_file(&fake, &pin) {
            Err(RuntimeError::WrongRuntime { expected, got, .. }) => {
                assert_eq!(expected, pin.sha256);
                assert_ne!(got, expected);
            }
            other => panic!("{other:?}"),
        }
        std::fs::write(&fake, b"hello").unwrap();
        assert_eq!(verify_file(&fake, &pin), Ok(fake.clone()));
        assert!(matches!(
            verify_file(&dir.join("missing.bin"), &pin),
            Err(RuntimeError::Unreadable { .. })
        ));
        assert!(matches!(
            verify_file(Path::new("relative.bin"), &pin),
            Err(RuntimeError::NotAbsolute(_))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
