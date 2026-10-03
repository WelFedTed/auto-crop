// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Build script of `auto-crop-codecs`. Without the `turbojpeg` feature it does nothing, so the
//! default build needs no C toolchain and no native library (ROADMAP M1.18).
//!
//! With the feature it links the libjpeg-turbo that `cargo xtask build-native` built from the pin
//! in `native-deps.toml` (default prefix `target/native/prefix`, or `AUTOCROP_NATIVE_PREFIX`) and
//! **refuses to build against anything older than 3.1.4**. It never searches the system, never
//! uses pkg-config or vcpkg, and never falls back to a vendored copy (ADR-0004, ADR-0008).

#[path = "native_header.rs"]
#[allow(dead_code)] // the enum and #define readers are for the tests
mod native_header;

use std::{env, fs, path::PathBuf, process};

fn fail(msg: &str) -> ! {
    eprintln!("error: {msg}");
    process::exit(1);
}

fn main() {
    println!("cargo:rustc-check-cfg=cfg(tj3_init_version)");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=native_header.rs");
    println!("cargo:rerun-if-env-changed=AUTOCROP_NATIVE_PREFIX");
    println!("cargo:rerun-if-env-changed=AUTOCROP_TURBOJPEG_STATIC");
    if env::var_os("CARGO_FEATURE_TURBOJPEG").is_none() {
        return;
    }
    let prefix = env::var("AUTOCROP_NATIVE_PREFIX")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"))
                .join("../../target/native/prefix")
        });
    let include = prefix.join("include");
    let read = |name: &str| {
        let p = include.join(name);
        println!("cargo:rerun-if-changed={}", p.display());
        fs::read_to_string(&p).unwrap_or_else(|e| {
            fail(&format!(
                "cannot read {}: {e} (run `cargo xtask build-native --only libjpeg-turbo` first, or set AUTOCROP_NATIVE_PREFIX)",
                p.display()
            ))
        })
    };
    let (jconfig, turbojpeg_h) = (read("jconfig.h"), read("turbojpeg.h"));
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
    println!("cargo:rustc-link-search=native={}", lib.display());
    if env::var_os("AUTOCROP_TURBOJPEG_STATIC").is_some() {
        // For the sanitizer job: a static libjpeg-turbo compiled with -fsanitize=address links
        // against the sanitizer runtime that rustc puts into the executable.
        println!("cargo:rustc-link-lib=static=turbojpeg");
    } else {
        println!("cargo:rustc-link-lib=dylib=turbojpeg");
        let os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
        if os == "linux" || os == "macos" {
            // Test and bench binaries find the library without LD_LIBRARY_PATH.
            println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib.display());
        }
    }
}
