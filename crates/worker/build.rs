// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The worker is a small executable, and `packaging/macos/assemble-app.sh` rewrites its install
//! names and adds the bundle's rpath with `install_name_tool`, which needs room in the Mach-O
//! header ("larger updated load commands do not fit"). The linker pads the header for it.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg-bins=-Wl,-headerpad_max_install_names");
    }
}
