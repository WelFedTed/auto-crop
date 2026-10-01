// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `settings.toml`: a corrupt or missing file falls back to the defaults (PLAN 2.10).

use crate::paths::AppPaths;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// Off by default (B3): originals are replaced after a verified backup.
    pub save_as_copy: bool,
    /// Days to keep backups; `None` = never delete.
    pub retention_days: Option<u32>,
    /// The first-write sheet has been acknowledged.
    pub first_write_ack: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            save_as_copy: false,
            retention_days: Some(30),
            first_write_ack: false,
        }
    }
}

/// On-disk form: TOML has no null, so "never delete" is stored as 0 days.
#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Stored {
    save_as_copy: bool,
    retention_days: u32,
    first_write_ack: bool,
}

impl Default for Stored {
    fn default() -> Self {
        Settings::default().into()
    }
}

impl From<Settings> for Stored {
    fn from(s: Settings) -> Self {
        Self {
            save_as_copy: s.save_as_copy,
            retention_days: s.retention_days.unwrap_or(0),
            first_write_ack: s.first_write_ack,
        }
    }
}

impl From<Stored> for Settings {
    fn from(s: Stored) -> Self {
        Self {
            save_as_copy: s.save_as_copy,
            retention_days: (s.retention_days != 0).then_some(s.retention_days),
            first_write_ack: s.first_write_ack,
        }
    }
}

impl Settings {
    /// Only the retention options the UI offers are accepted.
    pub fn sanitised(mut self) -> Self {
        if let Some(d) = self.retention_days
            && !matches!(d, 7 | 30 | 90 | 365)
        {
            self.retention_days = Some(30);
        }
        self
    }

    pub fn load(paths: &AppPaths) -> Self {
        std::fs::read_to_string(paths.settings_file())
            .ok()
            .and_then(|s| toml::from_str::<Stored>(&s).ok())
            .map(|s| Settings::from(s).sanitised())
            .unwrap_or_default()
    }

    pub fn save(&self, paths: &AppPaths) -> std::io::Result<()> {
        std::fs::create_dir_all(&paths.config_dir)?;
        let text = toml::to_string_pretty(&Stored::from(self.clone()))
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        let tmp = paths.config_dir.join(".settings.toml.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, paths.settings_file())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_survives_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(dir.path());
        assert_eq!(Settings::load(&paths), Settings::default());
        let s = Settings {
            save_as_copy: true,
            retention_days: None,
            first_write_ack: true,
        };
        s.save(&paths).unwrap();
        assert_eq!(Settings::load(&paths), s);
        std::fs::write(paths.settings_file(), "this is [not toml").unwrap();
        assert_eq!(Settings::load(&paths), Settings::default());
    }

    #[test]
    fn odd_retention_values_fall_back_to_30_days() {
        let s = Settings {
            retention_days: Some(5),
            ..Settings::default()
        }
        .sanitised();
        assert_eq!(s.retention_days, Some(30));
    }
}
