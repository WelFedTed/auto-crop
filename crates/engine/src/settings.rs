// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `settings.toml`: a corrupt or missing file falls back to the defaults (PLAN 2.10).

use crate::paths::AppPaths;
use auto_crop_core::{SplitPolicy, SplitProfile};
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
    /// Whether to look for several items on one scan (PLAN 4.7). `Auto` finds them and shows them;
    /// whether they are saved without a look is `auto_save_splits`.
    pub split_policy: SplitPolicy,
    /// What the items on a scan are (M10.12).
    pub split_profile: SplitProfile,
    /// EXPERIMENTAL (M10.29, the 0.x preview rule): save a split scan without a review when every
    /// item is Good at the Strict cutoff. Off by default: a split replaces one file with several,
    /// so by default a split scan is held until the user accepts it.
    pub auto_save_splits: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            save_as_copy: false,
            retention_days: Some(30),
            first_write_ack: false,
            split_policy: SplitPolicy::Auto,
            split_profile: SplitProfile::Photos,
            auto_save_splits: false,
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
    split_policy: SplitPolicy,
    split_profile: SplitProfile,
    auto_save_splits: bool,
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
            split_policy: s.split_policy,
            split_profile: s.split_profile,
            auto_save_splits: s.auto_save_splits,
        }
    }
}

impl From<Stored> for Settings {
    fn from(s: Stored) -> Self {
        Self {
            save_as_copy: s.save_as_copy,
            retention_days: (s.retention_days != 0).then_some(s.retention_days),
            first_write_ack: s.first_write_ack,
            split_policy: s.split_policy,
            split_profile: s.split_profile,
            auto_save_splits: s.auto_save_splits,
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
            split_policy: SplitPolicy::Always,
            split_profile: SplitProfile::Receipts,
            auto_save_splits: true,
        };
        s.save(&paths).unwrap();
        assert_eq!(Settings::load(&paths), s);
        std::fs::write(paths.settings_file(), "this is [not toml").unwrap();
        assert_eq!(Settings::load(&paths), Settings::default());
    }

    #[test]
    fn splitting_is_found_but_not_saved_unseen_by_default() {
        let d = Settings::default();
        assert_eq!(d.split_policy, SplitPolicy::Auto);
        assert!(
            !d.auto_save_splits,
            "auto-saving splits is Experimental and off"
        );
        // A settings file written before M10 still loads, with the defaults for the new keys.
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(dir.path());
        std::fs::create_dir_all(&paths.config_dir).unwrap();
        std::fs::write(
            paths.settings_file(),
            "save_as_copy = true\nretention_days = 90\nfirst_write_ack = true\n",
        )
        .unwrap();
        let s = Settings::load(&paths);
        assert!(s.save_as_copy && s.first_write_ack && s.retention_days == Some(90));
        assert_eq!(
            (s.split_policy, s.auto_save_splits),
            (SplitPolicy::Auto, false)
        );
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
