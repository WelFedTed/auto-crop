// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Links the libheif built by `cargo xtask build-native` and refuses a version below 1.23.5.

use std::{env, fs, path::PathBuf};

fn parse(v: &str) -> Vec<u32> {
    v.split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

fn main() {
    let prefix = env::var("AUTOCROP_NATIVE_PREFIX")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("../../target/native/prefix"));
    println!("cargo:rerun-if-env-changed=AUTOCROP_NATIVE_PREFIX");
    let header = prefix.join("include/libheif/heif_version.h");
    let text = fs::read_to_string(&header).unwrap_or_else(|e| {
        panic!("cannot read {}: {e} (run `cargo xtask build-native` first)", header.display())
    });
    let version = text
        .lines()
        .find_map(|l| {
            let mut it = l.split_whitespace();
            (it.next() == Some("#define") && it.next() == Some("LIBHEIF_VERSION"))
                .then(|| it.next().map(|v| v.trim_matches('"').to_owned()))
                .flatten()
        })
        .expect("LIBHEIF_VERSION not found in heif_version.h");
    assert!(parse(&version) >= parse("1.23.5"), "libheif {version} is older than the 1.23.5 floor; refusing to link");
    println!("cargo:rustc-env=LIBHEIF_LINKED_VERSION={version}");
    println!("cargo:rustc-link-search=native={}", prefix.join("lib").display());
    println!("cargo:rustc-link-lib=dylib=heif");
}
