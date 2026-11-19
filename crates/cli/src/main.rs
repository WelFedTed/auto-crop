// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `auto-crop`: the command-line tool as its own executable; everything is in the library.

fn main() -> std::process::ExitCode {
    auto_crop_cli::run(false)
}
