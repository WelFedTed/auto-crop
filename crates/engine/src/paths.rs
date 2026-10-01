// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Where the app keeps its data. The shell passes these in; nothing else resolves paths (PLAN 2.7).

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct AppPaths {
    /// Backups, library and logs: `%LOCALAPPDATA%\AutoCrop`, `~/Library/Application Support/AutoCrop`
    /// or `$XDG_DATA_HOME/auto-crop`.
    pub data_dir: PathBuf,
    /// `settings.toml`.
    pub config_dir: PathBuf,
}

impl AppPaths {
    pub fn new(data_dir: impl Into<PathBuf>, config_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            config_dir: config_dir.into(),
        }
    }

    /// Both directories under one root (tests, portable use).
    pub fn under(root: &Path) -> Self {
        Self::new(root.join("data"), root.join("config"))
    }

    /// The per-user defaults for this OS; `None` if the environment gives no home.
    pub fn for_current_user() -> Option<Self> {
        let var = |k: &str| {
            std::env::var_os(k)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        if cfg!(windows) {
            let base = var("LOCALAPPDATA")?.join("AutoCrop");
            Some(Self::new(base.clone(), base))
        } else if cfg!(target_os = "macos") {
            let base = var("HOME")?.join("Library/Application Support/AutoCrop");
            Some(Self::new(base.clone(), base))
        } else {
            let home = var("HOME");
            let data = var("XDG_DATA_HOME")
                .or_else(|| home.clone().map(|h| h.join(".local/share")))?
                .join("auto-crop");
            let config = var("XDG_CONFIG_HOME")
                .or_else(|| home.map(|h| h.join(".config")))?
                .join("auto-crop");
            Some(Self::new(data, config))
        }
    }

    pub fn backups_dir(&self) -> PathBuf {
        self.data_dir.join("backups")
    }

    pub fn settings_file(&self) -> PathBuf {
        self.config_dir.join("settings.toml")
    }

    pub fn samples_dir(&self) -> PathBuf {
        self.data_dir.join("samples")
    }
}
