// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Thin desktop shell. The only crate allowed to depend on Tauri (not yet added).

/// Placeholder: the engine version string the shell would display.
pub fn engine_ready() -> bool {
    auto_crop_engine::new_edit_state().version >= 1
}

#[cfg(test)]
mod tests {
    #[test]
    fn engine_is_reachable() {
        assert!(super::engine_ready());
    }
}
