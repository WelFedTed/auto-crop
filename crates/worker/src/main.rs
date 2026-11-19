// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `auto-crop-worker`: a thin wrapper over [`auto_crop_worker::run`] (see the library).

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(auto_crop_worker::run(&args));
}
