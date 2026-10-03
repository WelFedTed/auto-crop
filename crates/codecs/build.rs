// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Build script of `auto-crop-codecs`. Without the `turbojpeg` and `heif` features it does
//! nothing, so the default build needs no C toolchain and no native library (ROADMAP M1.18).
//!
//! With `turbojpeg` it links the libjpeg-turbo that `cargo xtask build-native` built from the pin
//! in `native-deps.toml` (default prefix `target/native/prefix`, or `AUTOCROP_NATIVE_PREFIX`) and
//! **refuses to build against anything older than 3.1.4**. With `heif` it links the libheif of the
//! same prefix (which has dav1d linked in and finds the libde265 plugin at run time) and **refuses
//! anything older than 1.23.5**. It never searches the system, never uses pkg-config or vcpkg,
//! and never falls back to a vendored copy (ADR-0004, ADR-0008, ADR-0009).

#[path = "native_header.rs"]
#[allow(dead_code)] // the enum and #define readers are for the tests
mod native_header;

use std::{
    env, fs,
    path::{Path, PathBuf},
    process,
};

fn fail(msg: &str) -> ! {
    eprintln!("error: {msg}");
    process::exit(1);
}

fn read(include: &Path, name: &str, what: &str) -> String {
    let p = include.join(name);
    println!("cargo:rerun-if-changed={}", p.display());
    fs::read_to_string(&p).unwrap_or_else(|e| {
        fail(&format!(
            "cannot read {}: {e} (run `cargo xtask build-native{what}` first, or set AUTOCROP_NATIVE_PREFIX)",
            p.display()
        ))
    })
}

/// Link search path, and on Unix an rpath so test and bench binaries find the shared library
/// without `LD_LIBRARY_PATH`.
fn link_dir(lib: &Path) {
    println!("cargo:rustc-link-search=native={}", lib.display());
    let os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if os == "linux" || os == "macos" {
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib.display());
    }
}

fn turbojpeg(prefix: &Path) {
    let include = prefix.join("include");
    let (jconfig, turbojpeg_h) = (
        read(&include, "jconfig.h", " --only libjpeg-turbo"),
        read(&include, "turbojpeg.h", " --only libjpeg-turbo"),
    );
    let version =
        native_header::check_versions(&jconfig, &turbojpeg_h).unwrap_or_else(|e| fail(&e));
    println!("cargo:rustc-env=AUTOCROP_TURBOJPEG_VERSION={version}");
    // 3.2 exports `tj3InitVersion` (and makes `tj3Init` a macro over it); 3.1.x exports `tj3Init`.
    if native_header::declares_init_version(&turbojpeg_h) {
        println!("cargo:rustc-cfg=tj3_init_version");
    }
    println!(
        "cargo:rustc-env=AUTOCROP_TURBOJPEG_INCLUDE={}",
        include.display()
    );

    let lib = prefix.join("lib");
    if env::var_os("AUTOCROP_TURBOJPEG_STATIC").is_some() {
        // For the sanitizer job: a static libjpeg-turbo compiled with -fsanitize=address links
        // against the sanitizer runtime that rustc puts into the executable.
        println!("cargo:rustc-link-search=native={}", lib.display());
        println!("cargo:rustc-link-lib=static=turbojpeg");
    } else {
        link_dir(&lib);
        println!("cargo:rustc-link-lib=dylib=turbojpeg");
    }
}

fn heif(prefix: &Path) {
    let include = prefix.join("include");
    let heif_dir = include.join("libheif");
    let version_h = read(&heif_dir, "heif_version.h", "");
    let version = native_header::check_heif_version(&version_h).unwrap_or_else(|e| fail(&e));
    // The security-limits API (heif_security.h) is what bounds a hostile file; a library without
    // it is not usable.
    let _ = read(&heif_dir, "heif_security.h", "");
    println!("cargo:rustc-env=AUTOCROP_HEIF_VERSION={version}");
    println!(
        "cargo:rustc-env=AUTOCROP_HEIF_INCLUDE={}",
        heif_dir.display()
    );
    link_dir(&prefix.join("lib"));
    println!("cargo:rustc-link-lib=dylib=heif");
}

fn main() {
    println!("cargo:rustc-check-cfg=cfg(tj3_init_version)");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=native_header.rs");
    println!("cargo:rerun-if-env-changed=AUTOCROP_NATIVE_PREFIX");
    println!("cargo:rerun-if-env-changed=AUTOCROP_TURBOJPEG_STATIC");
    let want_turbo = env::var_os("CARGO_FEATURE_TURBOJPEG").is_some();
    let want_heif = env::var_os("CARGO_FEATURE_HEIF").is_some();
    if !want_turbo && !want_heif {
        return;
    }
    let prefix = env::var("AUTOCROP_NATIVE_PREFIX")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"))
                .join("../../target/native/prefix")
        });
    if want_turbo {
        turbojpeg(&prefix);
    }
    if want_heif {
        heif(&prefix);
    }
}
