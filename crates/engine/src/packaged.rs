// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Start-up wiring of the HEIC and AVIF decoder for a packaged build (docs/testing/heif-native.md,
//! "What the shell needs"). The compiled-in plugin directory of libheif is the build machine's
//! prefix, which does not exist on the user's machine, so the executable points libheif at the
//! `libheif/` folder that ships next to it before the first decode.

use std::path::{Path, PathBuf};

/// Name of the plugin folder beside the executable (it holds `heif-libde265`, the HEVC decoder).
pub const PLUGIN_DIR_NAME: &str = "libheif";

/// The plugin folder of a packaged build: `<exe dir>/libheif`, if that folder exists.
pub fn packaged_plugin_dir(exe: &Path) -> Option<PathBuf> {
    let dir = exe.parent()?.join(PLUGIN_DIR_NAME);
    dir.is_dir().then_some(dir)
}

/// Points libheif at `<exe dir>/libheif` (when it exists) and leaves the decoder thread count at
/// its default. Call once at start-up, before the first decode; returns the folder that was set.
/// A development build, which has no such folder next to its executable, keeps libheif's
/// built-in plugin directory and `LIBHEIF_PLUGIN_PATH`.
#[cfg(feature = "heif")]
pub fn configure_heif_from_exe() -> Option<PathBuf> {
    let dir = packaged_plugin_dir(&std::env::current_exe().ok()?)?;
    auto_crop_codecs::heif::configure(Some(dir.clone())).then_some(dir)
}

/// Without the `heif` feature there is no decoder to configure.
#[cfg(not(feature = "heif"))]
pub fn configure_heif_from_exe() -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plugin_folder_is_found_only_when_it_exists_beside_the_executable() {
        let d = tempfile::tempdir().unwrap();
        let exe = d.path().join("AutoCrop.exe");
        assert_eq!(packaged_plugin_dir(&exe), None);
        std::fs::create_dir(d.path().join(PLUGIN_DIR_NAME)).unwrap();
        assert_eq!(
            packaged_plugin_dir(&exe),
            Some(d.path().join(PLUGIN_DIR_NAME))
        );
    }
}
