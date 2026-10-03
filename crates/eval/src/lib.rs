// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The accuracy harness (ROADMAP M1.45-M1.50, PLAN 7.2 and 7.4): runs a [`predictor::Predictor`]
//! over a manifest of images with ground-truth page quads and reports geometry metrics.
//!
//! Layout:
//!
//! * Metric core, independent of the code it measures (imports no project crate): [`geom`],
//!   [`metrics`], [`stats`], [`calib`].
//! * Data and plumbing: [`manifest`], [`predictor`], [`run`], [`report`], [`compare`], [`noise`],
//!   [`publish`] (the publishing guard type), [`splits`] (`check-splits`) and [`selfcheck`].
//! * Adapters and data that do use the app crates: [`detector`] (the real classical detector),
//!   [`synth`] (the Rust STAND-IN synthetic suite writer; the real generator is the Python tool in
//!   `tools/synth`) and [`variants`] (the decoder check on the generator's format, EXIF and
//!   colour-space variants).
//!
//! Synthetic numbers detect regressions between builds. They never back a real-world accuracy
//! claim, and nothing here may publish a per-image row (golden-set policy, B21).

pub mod calib;
pub mod compare;
pub mod detector;
pub mod geom;
pub mod manifest;
pub mod metrics;
pub mod noise;
pub mod predictor;
pub mod publish;
pub mod report;
pub mod run;
pub mod selfcheck;
pub mod splits;
pub mod stats;
pub mod synth;
pub mod variants;

#[cfg(test)]
mod independence {
    /// The metric core must not import any project crate: the code that scores the detector may
    /// not share a homography, a clipper or a statistic with the detector's own pipeline (M1.46).
    #[test]
    fn metric_modules_import_no_project_crate() {
        let sources = [
            ("geom.rs", include_str!("geom.rs")),
            ("metrics.rs", include_str!("metrics.rs")),
            ("stats.rs", include_str!("stats.rs")),
            ("calib.rs", include_str!("calib.rs")),
        ];
        for (name, text) in sources {
            // Only look at code before the test module so this file's own words do not matter.
            for banned in [
                "auto_crop_imgproc",
                "auto_crop_codecs",
                "auto_crop_core",
                "auto_crop_engine",
            ] {
                assert!(!text.contains(banned), "{name} references {banned}");
            }
        }
    }
}
