// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The two build-time helpers of the analysis stand-ins (ROADMAP M1.55):
//!
//! * `cargo xtask fetch-ort [--path-only]` downloads the ONNX Runtime archive of this host from
//!   `native-deps.toml` through the same SHA-256-verified downloader as `build-native` (a wrong hash
//!   is refused), checks that the library inside it is the one `auto_crop_infer::runtime` pins, and
//!   installs only that library into `target/native/prefix/lib`. It needs no compiler: the archive
//!   is a binary release (ADR-0004, ADR-0007).
//! * `cargo xtask make-standin-net [--out DIR]` generates the random-weight 256x256
//!   MobileNetV3-class corner net (`spikes/inference/gen_models.py --only-quadnet`, needs Python
//!   with `numpy` and `onnx`) into `target/standin`, with a sidecar SHA-256 the harness checks
//!   before it loads the model. The weights are deterministic random numbers, never trained,
//!   never committed (about 2.7 MB).

use crate::native;
use auto_crop_infer::model::sha256_hex;
use auto_crop_infer::runtime::{self, PINNED_VERSION};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const MANIFEST: &str = "native-deps.toml";
const GENERATOR: &str = "spikes/inference/gen_models.py";
const DEFAULT_NET_DIR: &str = "target/standin";
pub const NET_FILE: &str = "quadnet.onnx";

fn value_of<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

/// `cargo xtask fetch-ort [--path-only]`.
pub fn run_fetch_ort(args: &[String]) -> Result<(), String> {
    let path_only = args.iter().any(|a| a == "--path-only");
    let say = |s: String| {
        if path_only {
            eprintln!("{s}");
        } else {
            println!("{s}");
        }
    };
    let pin = runtime::pin_for_host().ok_or_else(|| {
        format!(
            "no pinned ONNX Runtime for {}/{} (Intel Mac has no binary, ARM64 is pinned in M4): use the rten backend",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    })?;
    let text = fs::read_to_string(MANIFEST).map_err(|e| format!("{MANIFEST}: {e}"))?;
    let libs = native::parse(&text)?;
    let problems = native::validate(&libs);
    if !problems.is_empty() {
        return Err(problems.join("\n"));
    }
    let lib = libs
        .iter()
        .find(|l| l.name == pin.native_deps_name)
        .ok_or(format!("{} missing from {MANIFEST}", pin.native_deps_name))?;
    if lib.version != PINNED_VERSION {
        return Err(format!(
            "{MANIFEST} pins {} {} but auto-crop-infer pins ONNX Runtime {PINNED_VERSION}; update the library hashes in crates/infer/src/runtime.rs",
            lib.name, lib.version
        ));
    }
    let root = PathBuf::from("target/native");
    let work = root.join(format!("{}-{}", lib.name, lib.version));
    fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let file = lib.url.rsplit('/').next().unwrap_or("archive");
    let archive = work.join(file);
    say(format!("== {} {}: fetching", lib.name, lib.version));
    if !archive.exists() || native::verify(&archive, &lib.sha256).is_err() {
        native::download(&lib.url, &archive)?;
    }
    native::verify(&archive, &lib.sha256)?; // a wrong hash is refused
    let src_parent = work.join("src");
    let _ = fs::remove_dir_all(&src_parent);
    let src = native::extract(&archive, &src_parent)?;
    // The library in the archive must be the one the loader will accept.
    let extracted = src.join("lib").join(pin.file_name);
    let extracted = native::plain_path(
        fs::canonicalize(&extracted).map_err(|e| format!("{}: {e}", extracted.display()))?,
    );
    runtime::verify_file(&extracted, pin).map_err(|e| e.to_string())?;
    let lib_dir = root.join("prefix").join("lib");
    fs::create_dir_all(&lib_dir).map_err(|e| e.to_string())?;
    let dest = lib_dir.join(pin.file_name);
    fs::copy(&extracted, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    let dest = native::plain_path(fs::canonicalize(&dest).map_err(|e| e.to_string())?);
    runtime::verify_file(&dest, pin).map_err(|e| e.to_string())?;
    let _ = fs::remove_dir_all(&src_parent);
    say(format!(
        "fetch-ort: ONNX Runtime {PINNED_VERSION} installed and verified ({}, sha256 {})",
        dest.display(),
        pin.sha256
    ));
    if path_only {
        println!("{}", dest.display());
    } else {
        println!("{}={}", runtime::ENV_VAR, dest.display());
    }
    Ok(())
}

/// The runtime library `fetch-ort` installed, as an absolute path, for the perf harness when
/// `AUTOCROP_ORT_DYLIB` is not set. `None` lets `auto_crop_infer` locate it (variable, then the
/// executable's directory). The path is still verified against the pin before it is loaded.
pub fn dev_runtime() -> Option<PathBuf> {
    if std::env::var_os(runtime::ENV_VAR).is_some_and(|v| !v.is_empty()) {
        return None;
    }
    let pin = runtime::pin_for_host()?;
    let p = std::env::current_dir()
        .ok()?
        .join("target/native/prefix/lib")
        .join(pin.file_name);
    p.is_file().then_some(p)
}

/// A Python 3 interpreter: `AUTOCROP_PYTHON`, else `python3`, `python`, `py -3`.
fn python() -> Result<(String, Vec<String>), String> {
    if let Ok(p) = std::env::var("AUTOCROP_PYTHON")
        && !p.is_empty()
    {
        return Ok((p, vec![]));
    }
    for (prog, pre) in [
        ("python3", &[][..]),
        ("python", &[][..]),
        ("py", &["-3"][..]),
    ] {
        let Ok(o) = Command::new(prog).args(pre).arg("--version").output() else {
            continue;
        };
        let text =
            String::from_utf8_lossy(&o.stdout).to_string() + &String::from_utf8_lossy(&o.stderr);
        if o.status.success() && text.trim().starts_with("Python 3") {
            return Ok((
                prog.to_owned(),
                pre.iter().map(|s| (*s).to_owned()).collect(),
            ));
        }
    }
    Err("Python 3 not found (make-standin-net runs spikes/inference/gen_models.py); set AUTOCROP_PYTHON".to_owned())
}

/// The sidecar digest file next to a model.
pub fn sidecar(model: &Path) -> PathBuf {
    auto_crop_infer::model::sidecar_path(model)
}

/// Reads `model` and checks it against its sidecar digest (`<model>.sha256`), the pin the harness
/// passes to `VerifiedModel::from_bytes_pinned`.
pub fn read_pinned(model: &Path) -> Result<auto_crop_infer::VerifiedModel, String> {
    auto_crop_infer::VerifiedModel::from_file_with_sidecar(model)
        .map_err(|e| format!("{e} (run `cargo xtask make-standin-net`)"))
}

/// Generates (or reuses) the net; returns its path. The cache key is the SHA-256 of the generator
/// script: change the script and the net is rebuilt.
pub fn make_standin_net(out_dir: &Path) -> Result<PathBuf, String> {
    let model = out_dir.join(NET_FILE);
    let stamp_path = out_dir.join("quadnet.generator.sha256");
    let generator = fs::read(GENERATOR).map_err(|e| format!("{GENERATOR}: {e}"))?;
    let stamp = sha256_hex(&generator);
    if model.is_file()
        && fs::read_to_string(&stamp_path).is_ok_and(|s| s.trim() == stamp)
        && read_pinned(&model).is_ok()
    {
        println!("make-standin-net: up to date ({})", model.display());
        return Ok(model);
    }
    fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let (prog, pre) = python()?;
    let status = Command::new(&prog)
        .args(&pre)
        .arg(GENERATOR)
        .arg(out_dir)
        .arg("--only-quadnet")
        .status()
        .map_err(|e| format!("cannot run {prog}: {e}"))?;
    if !status.success() {
        return Err(format!(
            "{GENERATOR} failed with {status} (needs `python -m pip install numpy onnx`)"
        ));
    }
    let bytes = fs::read(&model).map_err(|e| format!("{}: {e}", model.display()))?;
    fs::write(sidecar(&model), format!("{}\n", sha256_hex(&bytes))).map_err(|e| e.to_string())?;
    fs::write(&stamp_path, format!("{stamp}\n")).map_err(|e| e.to_string())?;
    println!(
        "make-standin-net: {} ({} bytes, sha256 {}); random weights, STAND-IN, never committed",
        model.display(),
        bytes.len(),
        sha256_hex(&bytes)
    );
    Ok(model)
}

/// `cargo xtask make-standin-net [--out DIR]`.
pub fn run_make_standin_net(args: &[String]) -> Result<(), String> {
    let out = value_of(args, "--out").unwrap_or(DEFAULT_NET_DIR);
    make_standin_net(Path::new(out)).map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_runtime_pins_agree_with_native_deps_toml() {
        let libs = native::parse(&fs::read_to_string("../native-deps.toml").unwrap()).unwrap();
        for pin in runtime::PINS {
            let lib = libs
                .iter()
                .find(|l| l.name == pin.native_deps_name)
                .unwrap_or_else(|| panic!("{} missing", pin.native_deps_name));
            assert_eq!(lib.version, PINNED_VERSION, "{}", lib.name);
            assert_eq!((lib.kind.as_str(), lib.build.as_str()), ("binary", "none"));
        }
    }

    #[test]
    fn the_sidecar_name_appends_sha256() {
        assert_eq!(
            sidecar(Path::new("target/standin/quadnet.onnx")),
            PathBuf::from("target/standin/quadnet.onnx.sha256")
        );
    }

    #[test]
    fn a_model_that_differs_from_its_sidecar_is_refused() {
        let dir = std::env::temp_dir().join(format!("auto-crop-standin-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let m = dir.join("m.onnx");
        fs::write(&m, b"abc").unwrap();
        fs::write(sidecar(&m), format!("{}\n", sha256_hex(b"abc"))).unwrap();
        assert!(read_pinned(&m).is_ok());
        fs::write(&m, b"abd").unwrap();
        let err = read_pinned(&m).unwrap_err();
        assert!(err.contains("REFUSED"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }
}
