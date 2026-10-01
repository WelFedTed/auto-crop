// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The Auto Crop desktop app. All logic lives in the library so it can be tested.

#![cfg_attr(all(not(debug_assertions), windows), windows_subsystem = "windows")]

fn main() {
    auto_crop_shell::run();
}
