// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! File-state hazards (ROADMAP M2.29; PLAN 2.7 step 1, 2.14): what a file's state says about
//! whether it may be read or replaced.
//!
//! * A **cloud placeholder** (OneDrive "files on demand", macOS dataless files) is never read, so it
//!   is never downloaded as a side effect of opening or saving: `CloudNotLocal`, unless the user
//!   opted in to hydration.
//! * A **read-only** file (the attribute, or a mode without write bits) is not replaced: `ReadOnly`
//!   before anything is written.
//! * A **symlink** is replaced at its resolved target and stays valid (`commit::swap`).
//! * A path inside a **sync root** (OneDrive, Dropbox, iCloud Drive, Google Drive folders) is
//!   written like any other, but the save carries the notice [`NOTICE_SYNC_ROOT`] so the front
//!   end can recommend Save as copy: a sync client may upload the temp file or revert the swap.

use crate::error::{ErrKind, Result};
use std::fs::Metadata;
use std::path::Path;

/// The notice a save into a sync root carries (a warning, never a refusal).
pub const NOTICE_SYNC_ROOT: &str = "sync.root";

/// `FILE_ATTRIBUTE_RECALL_ON_OPEN`, `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS`,
/// `FILE_ATTRIBUTE_OFFLINE`: reading the file would download it.
#[cfg(windows)]
const WIN_CLOUD_MASK: u32 = 0x0004_0000 | 0x0040_0000 | 0x0000_1000;
/// `SF_DATALESS` of `st_flags` on macOS.
#[cfg(target_os = "macos")]
const MAC_DATALESS: u32 = 0x4000_0000;

/// Is this file a cloud placeholder whose content is not on this machine?
pub fn is_cloud_placeholder(meta: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & WIN_CLOUD_MASK != 0
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::MetadataExt;
        meta.st_flags() & MAC_DATALESS != 0
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = meta;
        false
    }
}

/// Refuses a file that must not be read: a cloud placeholder (unless `hydrate`). Never touches
/// the content.
pub fn check_readable(path: &Path, hydrate: bool) -> Result<()> {
    let meta = std::fs::metadata(path).map_err(|e| ErrKind::from_io(&e))?;
    if !hydrate && is_cloud_placeholder(&meta) {
        return Err(ErrKind::CloudNotLocal);
    }
    Ok(())
}

/// Refuses a file that must not be replaced: read-only, or a placeholder. Run before a first byte
/// is written.
pub fn check_replaceable(path: &Path, hydrate: bool) -> Result<()> {
    let meta = std::fs::metadata(path).map_err(|e| ErrKind::from_io(&e))?;
    if !hydrate && is_cloud_placeholder(&meta) {
        return Err(ErrKind::CloudNotLocal);
    }
    if meta.permissions().readonly() {
        return Err(ErrKind::ReadOnly);
    }
    Ok(())
}

/// Folder names of the common sync clients (matched case-insensitively as whole path components,
/// or as a prefix for the vendors that append the account: `OneDrive - Contoso`).
const SYNC_ROOTS: [&str; 7] = [
    "onedrive",
    "dropbox",
    "icloud drive",
    "iclouddrive",
    "google drive",
    "googledrive",
    "mobile documents",
];

/// Does `path` sit inside a folder a sync client manages (by name)? A heuristic: it only decides
/// whether to attach a notice.
pub fn in_sync_root(path: &Path) -> bool {
    path.components().any(|c| {
        let n = c.as_os_str().to_string_lossy().to_lowercase();
        SYNC_ROOTS
            .iter()
            .any(|r| n == *r || n.starts_with(&format!("{r} -")))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn an_ordinary_file_passes_both_checks() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.jpg");
        fs::write(&p, b"x").unwrap();
        assert_eq!(check_readable(&p, false), Ok(()));
        assert_eq!(check_replaceable(&p, false), Ok(()));
        assert_eq!(
            check_readable(&d.path().join("missing"), false),
            Err(ErrKind::Unreadable)
        );
    }

    #[test]
    fn a_read_only_file_is_refused_before_anything_is_written() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.jpg");
        fs::write(&p, b"x").unwrap();
        let mut perm = fs::metadata(&p).unwrap().permissions();
        perm.set_readonly(true);
        fs::set_permissions(&p, perm.clone()).unwrap();
        assert_eq!(check_replaceable(&p, false), Err(ErrKind::ReadOnly));
        // Reading it is fine.
        assert_eq!(check_readable(&p, false), Ok(()));
        #[allow(clippy::permissions_set_readonly_false)]
        perm.set_readonly(false);
        fs::set_permissions(&p, perm).unwrap();
        assert_eq!(check_replaceable(&p, false), Ok(()));
    }

    #[test]
    fn sync_roots_are_recognised_by_folder_name() {
        let mut yes = vec![
            "/home/a/Dropbox/scans/a.jpg",
            "/Users/a/Library/Mobile Documents/com~apple~CloudDocs/a.jpg",
        ];
        let mut no = vec!["/home/a/photos/a.jpg", "/mnt/onedrivefake/a.jpg"];
        // A backslash separates components only on Windows.
        if cfg!(windows) {
            yes.extend([
                r"C:\Users\a\OneDrive\Pictures\a.jpg",
                r"C:\Users\a\OneDrive - Contoso\a.jpg",
                r"G:\Google Drive\a.jpg",
            ]);
            no.push(r"C:\Users\a\Pictures\a.jpg");
        }
        for p in yes {
            assert!(in_sync_root(Path::new(p)), "{p}");
        }
        for p in no {
            assert!(!in_sync_root(Path::new(p)), "{p}");
        }
    }

    /// The cloud mask is the three attributes the Windows docs name for "reading downloads it".
    #[cfg(windows)]
    #[test]
    fn the_cloud_mask_names_the_three_recall_attributes() {
        // RECALL_ON_DATA_ACCESS (0x400000), RECALL_ON_OPEN (0x40000), OFFLINE (0x1000).
        assert_eq!(WIN_CLOUD_MASK, 0x0044_1000);
    }
}
