// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Start-up wiring of the HEIC and AVIF decoder for a packaged build (docs/testing/heif-native.md,
//! "What the shell needs"). The compiled-in plugin directory of libheif is the build machine's
//! prefix, which does not exist on the user's machine, so the executable points libheif at the
//! `libheif/` plugin folder that ships with it before the first decode. Where that folder sits
//! depends on the package layout (docs/packaging.md):
//!
//! | Layout | Executable | Plugin folder |
//! |---|---|---|
//! | Windows zip, flat folder | `<dir>/AutoCrop.exe` | `<dir>/libheif` |
//! | macOS app bundle | `AutoCrop.app/Contents/MacOS/AutoCrop` | `AutoCrop.app/Contents/Frameworks/libheif` |
//! | Linux tarball, AppDir (AppImage), deb | `<root>/bin/AutoCrop` | `<root>/lib/libheif` |
//!
//! The candidates are plain paths relative to the executable, tried in that order; the first one
//! that exists as a folder wins. They are checked on every OS (a Linux layout on Windows is
//! harmless: the folder simply is not there), which keeps the resolution testable everywhere.

use std::path::{Path, PathBuf};

/// Name of the plugin folder (it holds `heif-libde265`, the HEVC decoder).
pub const PLUGIN_DIR_NAME: &str = "libheif";

/// Where the plugin folder may sit for an executable at `exe`, most specific layout first:
/// beside the executable (Windows zip, development copies), `../Frameworks/libheif` (macOS app
/// bundle, `Contents/MacOS` to `Contents/Frameworks`) and `../lib/libheif` (Linux `bin` to `lib`).
pub fn plugin_dir_candidates(exe: &Path) -> Vec<PathBuf> {
    let Some(dir) = exe.parent() else {
        return Vec::new();
    };
    vec![
        dir.join(PLUGIN_DIR_NAME),
        dir.join("..").join("Frameworks").join(PLUGIN_DIR_NAME),
        dir.join("..").join("lib").join(PLUGIN_DIR_NAME),
    ]
}

/// The plugin folder of a packaged build: the first of [`plugin_dir_candidates`] that exists.
pub fn packaged_plugin_dir(exe: &Path) -> Option<PathBuf> {
    plugin_dir_candidates(exe)
        .into_iter()
        .find(|dir| dir.is_dir())
}

/// Points libheif at the packaged plugin folder (when there is one, see [`packaged_plugin_dir`])
/// and leaves the decoder thread count at its default. Call once at start-up, before the first
/// decode; returns the folder that was set. A development build, which has no such folder next to
/// its executable, keeps libheif's built-in plugin directory and `LIBHEIF_PLUGIN_PATH`.
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
    use std::fs;

    /// Creates `<root>/<rel>/libheif` and returns it.
    fn plugin_dir(root: &Path, rel: &str) -> PathBuf {
        let dir = root.join(rel).join(PLUGIN_DIR_NAME);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The same folder with `..` segments resolved, for comparing across layouts.
    fn canon(p: Option<PathBuf>) -> Option<PathBuf> {
        p.map(|p| p.canonicalize().unwrap())
    }

    #[test]
    fn windows_layout_plugin_folder_beside_the_executable() {
        let d = tempfile::tempdir().unwrap();
        let exe = d.path().join("AutoCrop.exe");
        assert_eq!(packaged_plugin_dir(&exe), None);
        let plugins = plugin_dir(d.path(), ".");
        assert_eq!(canon(packaged_plugin_dir(&exe)), canon(Some(plugins)));
    }

    #[test]
    fn macos_bundle_layout_plugin_folder_in_contents_frameworks() {
        let d = tempfile::tempdir().unwrap();
        let macos = d.path().join("AutoCrop.app/Contents/MacOS");
        fs::create_dir_all(&macos).unwrap();
        let exe = macos.join("AutoCrop");
        assert_eq!(packaged_plugin_dir(&exe), None);
        let plugins = plugin_dir(d.path(), "AutoCrop.app/Contents/Frameworks");
        assert_eq!(canon(packaged_plugin_dir(&exe)), canon(Some(plugins)));
        // The command-line tool inside the bundle resolves the same folder.
        let cli = macos.join("auto-crop");
        assert_eq!(
            canon(packaged_plugin_dir(&cli)),
            canon(packaged_plugin_dir(&exe))
        );
    }

    #[test]
    fn linux_layout_plugin_folder_in_lib_beside_bin() {
        let d = tempfile::tempdir().unwrap();
        // The tarball, the AppDir (`usr/bin`, `usr/lib`) and the deb (`/usr/lib/auto-crop/bin`)
        // all have `bin` and `lib` as siblings.
        for root in ["auto-crop-linux", "AppDir/usr", "pkg/usr/lib/auto-crop"] {
            let bin = d.path().join(root).join("bin");
            fs::create_dir_all(&bin).unwrap();
            let exe = bin.join("AutoCrop");
            assert_eq!(packaged_plugin_dir(&exe), None, "{root}");
            let plugins = plugin_dir(&d.path().join(root), "lib");
            assert_eq!(
                canon(packaged_plugin_dir(&exe)),
                canon(Some(plugins)),
                "{root}"
            );
        }
    }

    #[test]
    fn the_folder_beside_the_executable_wins_over_the_other_layouts() {
        let d = tempfile::tempdir().unwrap();
        let bin = d.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        plugin_dir(d.path(), "lib");
        plugin_dir(d.path(), "Frameworks");
        let beside = plugin_dir(&bin, ".");
        let exe = bin.join("AutoCrop");
        assert_eq!(canon(packaged_plugin_dir(&exe)), canon(Some(beside)));
    }

    #[test]
    fn a_file_called_libheif_is_not_a_plugin_folder_and_a_bare_path_has_no_candidates() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join(PLUGIN_DIR_NAME), b"not a folder").unwrap();
        assert_eq!(packaged_plugin_dir(&d.path().join("AutoCrop")), None);
        assert!(plugin_dir_candidates(Path::new("/")).is_empty());
    }
}
