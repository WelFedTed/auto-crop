// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Records what `auto-crop --version` prints beyond the crate version: the commit it was built
//! from and the target. Both fall back to `unknown` (a source tarball has no `.git`); nothing
//! here touches the network.

use std::process::Command;

fn main() {
    let sha = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=AUTO_CROP_GIT_SHA={sha}");
    println!(
        "cargo:rustc-env=AUTO_CROP_TARGET={}",
        std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_owned())
    );
    println!(
        "cargo:rustc-env=AUTO_CROP_PROFILE={}",
        std::env::var("PROFILE").unwrap_or_else(|_| "unknown".to_owned())
    );
    println!("cargo:rerun-if-env-changed=AUTO_CROP_GIT_SHA");
    println!("cargo:rerun-if-changed=build.rs");
}
