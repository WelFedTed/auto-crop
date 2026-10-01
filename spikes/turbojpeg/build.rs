// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Links the libjpeg-turbo built by `cargo xtask build-native` and refuses a version below 3.1.4.

use std::{env, fs, path::PathBuf};

const FLOOR: u32 = 3_001_004;

fn main() {
    let prefix = env::var("AUTOCROP_NATIVE_PREFIX")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("../../target/native/prefix"));
    println!("cargo:rerun-if-env-changed=AUTOCROP_NATIVE_PREFIX");
    let header = prefix.join("include/turbojpeg.h");
    let text = fs::read_to_string(&header).unwrap_or_else(|e| {
        panic!("cannot read {}: {e} (run `cargo xtask build-native` first)", header.display())
    });
    let version: u32 = text
        .lines()
        .find_map(|l| {
            let mut it = l.split_whitespace();
            (it.next() == Some("#define") && it.next() == Some("TURBOJPEG_VERSION_NUMBER"))
                .then(|| it.next())
                .flatten()?
                .parse()
                .ok()
        })
        .expect("TURBOJPEG_VERSION_NUMBER not found in turbojpeg.h");
    assert!(version >= FLOOR, "libjpeg-turbo {version} is older than 3.1.4 ({FLOOR}); refusing to link");
    println!("cargo:rustc-env=TURBOJPEG_VERSION_NUMBER={version}");
    println!("cargo:rustc-link-search=native={}", prefix.join("lib").display());
    println!("cargo:rustc-link-lib=dylib=turbojpeg");
}
