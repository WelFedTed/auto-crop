// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

fn main() {
    #[cfg(feature = "gui")]
    tauri_build::build();
}
