// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Job queue, edit pipeline and safe writes.

pub mod api;
pub mod commit;
mod engine;
pub mod enumerate;
pub mod error;
pub mod memory;
pub mod migrate;
pub mod paths;
pub mod samples;
pub mod settings;
pub mod source;
pub mod store;
pub mod util;

pub use api::*;
pub use engine::{Engine, Notify};
pub use error::ErrKind;
pub use paths::AppPaths;
pub use settings::Settings;

use auto_crop_core::EditState;

/// Runs `f` and converts a panic into an error string instead of unwinding
/// further. Decoder glue relies on `panic = "unwind"` (see `check-profiles`).
pub fn run_isolated<T>(f: impl FnOnce() -> T + std::panic::UnwindSafe) -> Result<T, String> {
    std::panic::catch_unwind(f).map_err(|p| {
        p.downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| p.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "panic".to_owned())
    })
}

/// Placeholder: a fresh edit state for an image.
pub fn new_edit_state() -> EditState {
    EditState::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_state_is_default() {
        assert_eq!(new_edit_state(), EditState::default());
    }

    #[test]
    fn a_panic_is_caught_not_aborted() {
        let r: Result<(), String> = run_isolated(|| panic!("boom"));
        assert_eq!(r, Err("boom".to_owned()));
    }
}
