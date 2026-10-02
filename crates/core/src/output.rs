// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `OutputSpec` (PLAN 2.3): how the rendered pixels become files. It is per batch and overridable
//! per image, and deliberately NOT part of `EditState`: the same edit can be written many ways.
//!
//! Safety invariant (PLAN 2.1 rule 2): there is no field, value, setting or flag that skips the
//! backup or the verify step of an in-place write. The type has no backup switch and `Verify` has
//! no `Off`; unknown JSON fields are rejected so a hand-written `"backup": false` or
//! `"verify": "off"` fails to parse instead of being ignored.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OutFormat {
    Jpeg,
    Png,
    Webp,
    Avif,
    Tiff,
    Pdf,
    Jxl,
    /// Resolves per PLAN 3.2.3; `FsPlan` decides whether the source is replaceable.
    #[default]
    KeepSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Quality {
    /// Estimate from the source's quantisation tables, clamped to `floor..=cap` (PROVISIONAL).
    MatchSource {
        floor: u8,
        cap: u8,
    },
    Fixed {
        value: u8,
    },
    Lossless,
}

impl Default for Quality {
    fn default() -> Self {
        Quality::MatchSource { floor: 80, cap: 95 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Bilevel {
    Png,
    TiffG4,
}

/// Bit depth. 1-bit is an export option (B13), never part of `EditState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Bits {
    #[default]
    Eight,
    One {
        bilevel: Bilevel,
    },
}

/// C2: sRGB for HEIC/HEIF to JPG and for enhanced output (see the presets below); every other
/// source keeps its profile and pixels, which is why `PreserveWideGamut` is the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Colour {
    Srgb,
    #[default]
    PreserveWideGamut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Metadata {
    Keep {
        #[serde(rename = "stripLocation")]
        strip_location: bool,
    },
    Strip,
}

impl Default for Metadata {
    fn default() -> Self {
        Metadata::Keep {
            strip_location: false,
        }
    }
}

/// A folder chosen by Rust and referred to by an opaque id, never by a webview-supplied path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FolderId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CopyDir {
    /// `<source folder>/AutoCrop/`.
    SourceSubfolder,
    Picked {
        folder: FolderId,
    },
}

/// B3: the default is `InPlace`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Target {
    #[default]
    InPlace,
    Copy {
        dir: CopyDir,
        /// Tokens `{name}`, `{ext}`, `{n}`, `{date}`; one grammar and sanitiser in `FsPlan`.
        template: String,
    },
}

/// What happens when the output name is taken. UI "Keep both" is `Rename`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Collision {
    #[default]
    Rename,
    Skip,
    /// The existing unrelated file goes through the same backup-then-swap.
    Replace,
}

/// How hard the temp output is re-checked. There is no `Off` and never will be: both modes re-read
/// the temp and compare its blake3 and check dimensions, Orientation and ICC (PLAN 2.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Verify {
    /// Re-decode at full size.
    #[default]
    Full,
    /// Re-decode at reduced scale where the codec allows it.
    Fast,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LosslessJpeg {
    #[default]
    Auto,
    Never,
}

/// The steps a write must perform. Derived from the spec; never configurable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteRequirements {
    /// A verified backup of whatever file the swap replaces exists before the swap.
    pub backup_before_swap: bool,
    /// The temp output is re-read and its blake3 compared (and the checks of `verify` run)
    /// before it is swapped into place.
    pub verify_before_swap: bool,
    pub verify: Verify,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct OutputSpec {
    pub format: OutFormat,
    pub quality: Quality,
    pub bits: Bits,
    pub colour: Colour,
    pub metadata: Metadata,
    pub target: Target,
    pub collision: Collision,
    pub verify: Verify,
    pub lossless_jpeg: LosslessJpeg,
    pub keep_mtime: bool,
}

impl Default for OutputSpec {
    fn default() -> Self {
        Self {
            format: OutFormat::KeepSource,
            quality: Quality::default(),
            bits: Bits::Eight,
            colour: Colour::PreserveWideGamut,
            metadata: Metadata::default(),
            target: Target::InPlace,
            collision: Collision::Rename,
            verify: Verify::Full,
            lossless_jpeg: LosslessJpeg::Auto,
            keep_mtime: true,
        }
    }
}

impl OutputSpec {
    /// HEIC and HEIF to JPG: sRGB output (C2).
    pub fn heic_to_jpg() -> Self {
        Self {
            format: OutFormat::Jpeg,
            colour: Colour::Srgb,
            ..Self::default()
        }
    }

    /// The same spec for enhanced output, which is always sRGB (C2).
    pub fn for_enhanced_output(mut self) -> Self {
        self.colour = Colour::Srgb;
        self
    }

    /// Whether writing this spec can replace an existing file: in place, or a copy whose
    /// collision policy is `Replace`.
    pub fn may_overwrite(&self) -> bool {
        matches!(self.target, Target::InPlace) || self.collision == Collision::Replace
    }

    /// The write steps this spec cannot turn off. Verification is unconditional; a backup is
    /// required whenever the write can replace an existing file.
    pub fn write_requirements(&self) -> WriteRequirements {
        WriteRequirements {
            backup_before_swap: self.may_overwrite(),
            verify_before_swap: true,
            verify: self.verify,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn any_spec() -> impl Strategy<Value = OutputSpec> {
        let format = prop_oneof![
            Just(OutFormat::Jpeg),
            Just(OutFormat::Png),
            Just(OutFormat::Webp),
            Just(OutFormat::Avif),
            Just(OutFormat::Tiff),
            Just(OutFormat::Pdf),
            Just(OutFormat::Jxl),
            Just(OutFormat::KeepSource),
        ];
        let quality = prop_oneof![
            (0u8..=100, 0u8..=100).prop_map(|(floor, cap)| Quality::MatchSource { floor, cap }),
            (0u8..=100).prop_map(|value| Quality::Fixed { value }),
            Just(Quality::Lossless),
        ];
        let bits = prop_oneof![
            Just(Bits::Eight),
            Just(Bits::One {
                bilevel: Bilevel::Png
            }),
            Just(Bits::One {
                bilevel: Bilevel::TiffG4
            }),
        ];
        let colour = prop_oneof![Just(Colour::Srgb), Just(Colour::PreserveWideGamut)];
        let metadata = prop_oneof![
            Just(Metadata::Strip),
            prop::bool::ANY.prop_map(|strip_location| Metadata::Keep { strip_location }),
        ];
        let target = prop_oneof![
            Just(Target::InPlace),
            (prop::bool::ANY, 0u32..9, "[a-z{}_]{0,12}").prop_map(|(sub, id, template)| {
                Target::Copy {
                    dir: if sub {
                        CopyDir::SourceSubfolder
                    } else {
                        CopyDir::Picked {
                            folder: FolderId(id),
                        }
                    },
                    template,
                }
            }),
        ];
        let collision = prop_oneof![
            Just(Collision::Rename),
            Just(Collision::Skip),
            Just(Collision::Replace)
        ];
        let verify = prop_oneof![Just(Verify::Full), Just(Verify::Fast)];
        let lossless = prop_oneof![Just(LosslessJpeg::Auto), Just(LosslessJpeg::Never)];
        (
            (format, quality, bits, colour, metadata),
            (target, collision, verify, lossless, prop::bool::ANY),
        )
            .prop_map(
                |(
                    (format, quality, bits, colour, metadata),
                    (target, collision, verify, lossless_jpeg, keep_mtime),
                )| OutputSpec {
                    format,
                    quality,
                    bits,
                    colour,
                    metadata,
                    target,
                    collision,
                    verify,
                    lossless_jpeg,
                    keep_mtime,
                },
            )
    }

    proptest! {
        /// PLAN 2.1 rule 2: no value of any field skips the backup of an overwritten file or the
        /// verify step.
        #[test]
        fn no_value_skips_backup_or_verify(spec in any_spec()) {
            let r = spec.write_requirements();
            prop_assert!(r.verify_before_swap);
            if matches!(spec.target, Target::InPlace) || spec.collision == Collision::Replace {
                prop_assert!(r.backup_before_swap);
            }
            prop_assert_eq!(r.verify, spec.verify);
            // And the spec survives JSON unchanged.
            let json = serde_json::to_string(&spec).unwrap();
            prop_assert_eq!(serde_json::from_str::<OutputSpec>(&json).unwrap(), spec);
        }
    }

    #[test]
    fn the_default_overwrites_in_place_and_so_requires_backup_and_verify() {
        let d = OutputSpec::default();
        assert_eq!(d.target, Target::InPlace);
        assert_eq!(d.colour, Colour::PreserveWideGamut);
        let r = d.write_requirements();
        assert!(r.backup_before_swap && r.verify_before_swap);
        // A copy that cannot clobber needs no backup, but is still verified.
        let copy = OutputSpec {
            target: Target::Copy {
                dir: CopyDir::SourceSubfolder,
                template: "{name}".into(),
            },
            ..OutputSpec::default()
        };
        let r = copy.write_requirements();
        assert!(!r.backup_before_swap && r.verify_before_swap);
        let replace = OutputSpec {
            collision: Collision::Replace,
            ..copy
        };
        assert!(replace.write_requirements().backup_before_swap);
    }

    #[test]
    fn there_is_no_way_to_ask_for_no_backup_or_no_verify() {
        for bad in [
            r#"{"verify":"off"}"#,
            r#"{"verify":false}"#,
            r#"{"backup":false}"#,
            r#"{"skipBackup":true}"#,
        ] {
            assert!(serde_json::from_str::<OutputSpec>(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn colour_defaults_and_presets_follow_c2() {
        assert_eq!(OutputSpec::default().colour, Colour::PreserveWideGamut);
        let h = OutputSpec::heic_to_jpg();
        assert_eq!((h.format, h.colour), (OutFormat::Jpeg, Colour::Srgb));
        assert_eq!(
            OutputSpec::default().for_enhanced_output().colour,
            Colour::Srgb
        );
        assert_eq!(
            serde_json::from_str::<OutputSpec>("{}").unwrap(),
            OutputSpec::default()
        );
    }

    #[test]
    fn json_shape_is_camel_case_with_tagged_variants() {
        let v = serde_json::to_value(OutputSpec::default()).unwrap();
        assert_eq!(v["format"], "keepSource");
        assert_eq!(v["target"]["kind"], "inPlace");
        assert_eq!(v["collision"], "rename");
        assert_eq!(v["verify"], "full");
        assert_eq!(v["metadata"]["stripLocation"], false);
        assert_eq!(v["keepMtime"], true);
    }
}
