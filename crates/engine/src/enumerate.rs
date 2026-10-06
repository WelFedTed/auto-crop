// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Turning dropped or picked paths into a list of image files. Never follows symbolic links or
//! junctions (no loops), caps depth and count, and skips the app's own temp files and `AutoCrop`
//! output folders so a processed folder is not processed again.

use std::fs;
use std::path::{Path, PathBuf};

pub const MAX_DEPTH: usize = 12;
pub const MAX_FILES: usize = 20_000;

/// The extensions this build opens (lower case, without the dot): for the file picker and for
/// listing the formats truthfully in the UI. The same list [`is_candidate`] matches against.
pub fn input_extensions() -> &'static [&'static str] {
    auto_crop_codecs::supported_input_extensions()
}

/// File extensions this build opens (matched case-insensitively; the content is sniffed later).
/// The list is the codecs' own: only formats this build can decode (JPEG, PNG, TIFF and WebP, and
/// with the `heif` feature HEIC, HEIF and AVIF). Opening a format is not the same as being able to
/// write it back: see `Engine::save_items`, which never replaces a source that has no writer.
pub fn is_candidate(path: &Path) -> bool {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    input_extensions().contains(&ext.as_str())
        && !path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with(".autocrop-"))
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Found {
    pub files: Vec<PathBuf>,
    /// Symbolic links and junctions that were not followed.
    pub links_skipped: usize,
    /// True if the file cap was hit.
    pub truncated: bool,
}

fn is_link(meta: &fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // FILE_ATTRIBUTE_REPARSE_POINT covers junctions and other reparse points (including
        // OneDrive placeholders, which must not be hydrated silently).
        if meta.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    false
}

fn walk(dir: &Path, depth: usize, include_subfolders: bool, out: &mut Found) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        if out.files.len() >= MAX_FILES {
            out.truncated = true;
            return;
        }
        let path = e.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if is_link(&meta) {
            out.links_skipped += 1;
        } else if meta.is_dir() {
            let name = e.file_name().to_string_lossy().into_owned();
            if include_subfolders && depth < MAX_DEPTH && name != "AutoCrop" {
                walk(&path, depth + 1, include_subfolders, out);
            }
        } else if meta.is_file() && is_candidate(&path) {
            out.files.push(path);
        }
    }
}

/// Expands files and folders into candidate files, in a stable order, without duplicates.
pub fn collect(roots: &[PathBuf], include_subfolders: bool) -> Found {
    let mut out = Found::default();
    for root in roots {
        let Ok(meta) = fs::symlink_metadata(root) else {
            continue;
        };
        if is_link(&meta) {
            out.links_skipped += 1;
        } else if meta.is_dir() {
            walk(root, 0, include_subfolders, &mut out);
        } else if meta.is_file() && is_candidate(root) {
            out.files.push(root.clone());
        }
    }
    let mut seen = std::collections::HashSet::new();
    out.files.retain(|p| seen.insert(p.clone()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(p: &Path) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, b"x").unwrap();
    }

    #[test]
    fn finds_images_recursively_and_skips_our_own_output_and_temp_files() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        touch(&r.join("a.JPG"));
        touch(&r.join("b.png"));
        touch(&r.join("notes.txt"));
        touch(&r.join(".autocrop-123.tmp"));
        touch(&r.join("sub/c.jpeg"));
        touch(&r.join("AutoCrop/done.jpg"));
        let flat = collect(&[r.to_path_buf()], false);
        assert_eq!(flat.files.len(), 2);
        let deep = collect(&[r.to_path_buf()], true);
        let names: Vec<_> = deep
            .files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["a.JPG", "b.png", "c.jpeg"]);
    }

    #[test]
    fn the_extension_list_is_what_this_build_decodes() {
        for yes in ["a.jpg", "a.JPEG", "a.png", "a.tif", "a.TIFF", "a.webp"] {
            assert!(is_candidate(Path::new(yes)), "{yes}");
        }
        for no in [
            "a.gif",
            "a.bmp",
            "a.jxl",
            "a.pdf",
            "a.txt",
            "a",
            ".autocrop-1.png",
        ] {
            assert!(!is_candidate(Path::new(no)), "{no}");
        }
        // HEIC, HEIF and AVIF are candidates exactly when the codecs can decode them.
        for heif in ["a.heic", "a.HEIF", "a.avif"] {
            assert_eq!(
                is_candidate(Path::new(heif)),
                auto_crop_codecs::supported_input_formats().contains(&"avif"),
                "{heif}"
            );
        }
    }

    #[test]
    fn explicit_files_are_kept_and_duplicates_dropped() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("a.jpg");
        touch(&f);
        let found = collect(&[f.clone(), f.clone(), d.path().join("missing.jpg")], false);
        assert_eq!(found.files, vec![f]);
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_not_followed_so_loops_cannot_hang() {
        let d = tempfile::tempdir().unwrap();
        touch(&d.path().join("a.jpg"));
        std::os::unix::fs::symlink(d.path(), d.path().join("loop")).unwrap();
        let found = collect(&[d.path().to_path_buf()], true);
        assert_eq!(found.files.len(), 1);
        assert_eq!(found.links_skipped, 1);
    }
}
