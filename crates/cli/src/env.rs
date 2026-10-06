// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The machine the CLI runs on: where it keeps things, the CPU floor, the settings it honours.
//! The CLI reads `settings.toml` only for the backup retention (PLAN 2.12): everything else comes
//! from the command line, so a run reproduces from its arguments and a GUI toggle never changes
//! what a script does.

use crate::args::Global;
use auto_crop_core::{CancelToken, SplitPolicy, SplitProfile};
use auto_crop_engine::{AppPaths, Settings};
use std::path::PathBuf;

/// What every command needs.
pub struct Env {
    pub paths: AppPaths,
    pub global: Global,
    pub cancel: CancelToken,
}

/// Where backups and settings live: `--home DIR`, else `AUTO_CROP_HOME`, else the per-user folders.
pub fn app_paths(g: &Global) -> Result<AppPaths, String> {
    let home = g.home.clone().or_else(|| {
        std::env::var_os("AUTO_CROP_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    });
    match home {
        Some(h) => Ok(AppPaths::under(&std::path::absolute(&h).unwrap_or(h))),
        None => AppPaths::for_current_user()
            .ok_or_else(|| "cannot find your home folder; give --home DIR".to_owned()),
    }
}

/// The settings a run starts from: the stored ones for retention only (unless `--no-config`).
pub fn base_settings(paths: &AppPaths, g: &Global) -> Settings {
    let stored = if g.no_config {
        Settings::default()
    } else {
        Settings::load(paths)
    };
    Settings {
        // A CLI run is never a GUI "Save as copy" and never shows the GUI's first-write sheet.
        save_as_copy: false,
        first_write_ack: true,
        split_policy: SplitPolicy::Auto,
        split_profile: SplitProfile::Photos,
        auto_save_splits: false,
        retention_days: stored.retention_days,
    }
}

/// The CPU floor of the supported build (PLAN C1, ROADMAP M2.48): AVX2 on x86-64. The
/// `AUTO_CROP_FORCE_NO_AVX2` variable simulates a CPU without it (for the probe test).
pub fn cpu_floor() -> Result<(), String> {
    if has_avx2() {
        Ok(())
    } else {
        Err(
            "this CPU does not support AVX2, the minimum Auto Crop is built and tested for; \
             nothing was processed (run `auto-crop doctor` for details)"
                .to_owned(),
        )
    }
}

#[cfg(target_arch = "x86_64")]
pub fn has_avx2() -> bool {
    if std::env::var_os("AUTO_CROP_FORCE_NO_AVX2").is_some_and(|v| !v.is_empty()) {
        return false;
    }
    std::arch::is_x86_feature_detected!("avx2")
}

/// Other architectures have no AVX2 floor.
#[cfg(not(target_arch = "x86_64"))]
pub fn has_avx2() -> bool {
    true
}

/// `AVX2` is only a question on x86-64.
pub fn avx2_applies() -> bool {
    cfg!(target_arch = "x86_64")
}

/// A hidden numeric environment setting, for the tests.
pub fn test_hook(name: &str) -> Option<u64> {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_comes_from_the_flag_and_settings_are_cli_neutral() {
        let g = Global {
            home: Some(PathBuf::from("somewhere")),
            ..Global::default()
        };
        let p = app_paths(&g).unwrap();
        assert!(p.backups_dir().ends_with("backups"));
        let d = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(d.path());
        Settings {
            split_policy: SplitPolicy::Never,
            auto_save_splits: true,
            save_as_copy: true,
            retention_days: Some(90),
            ..Settings::default()
        }
        .save(&paths)
        .unwrap();
        let s = base_settings(&paths, &Global::default());
        assert_eq!(s.retention_days, Some(90), "retention is honoured");
        assert_eq!(
            s.split_policy,
            SplitPolicy::Auto,
            "the GUI's split setting is not"
        );
        assert!(!s.auto_save_splits && !s.save_as_copy);
        let none = base_settings(
            &paths,
            &Global {
                no_config: true,
                ..Global::default()
            },
        );
        assert_eq!(none.retention_days, Some(30));
    }
}
