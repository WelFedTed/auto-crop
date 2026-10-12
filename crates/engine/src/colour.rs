// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Colour conversion for a format conversion (PLAN 3.5 item 3, 3.7; ROADMAP M6.29): a source whose
//! embedded ICC profile is not sRGB (an iPhone's Display P3 HEIC, an Adobe RGB TIFF) is turned into
//! sRGB so the JPEG or PNG looks the same in every viewer, or is left exactly as it is
//! ([`ColourMode::Preserve`]). The transform is `moxcms` (BSD-3-Clause OR Apache-2.0, pure Rust, already
//! in the tree through the `image` crate): relative colorimetric, 8-bit RGB, matrix/TRC profiles
//! through its matrix-shaper path and LUT profiles through its LUT path.
//!
//! Rules, in the order they apply:
//!
//! 1. No profile: the pixels are taken to be sRGB and nothing changes.
//! 2. The profile cannot be parsed, or is not an RGB profile: pixels and profile are kept as they
//!    are and the notice `colour.profile_unusable` says the conversion was not done (PLAN 3.5: "keep
//!    pixels, embed the original ICC unchanged, emit a Warn").
//! 3. The profile maps a grid of probe colours to sRGB within one level (that is sRGB, whatever its
//!    name or how it is written): pixels and profile are kept, byte-exact.
//! 4. Otherwise every pixel is transformed and the output carries the generated sRGB profile, so a
//!    viewer that reads the profile and one that assumes sRGB agree.
//!
//! The transform is applied in row bands, so the extra memory is a few hundred KB whatever the size.

use auto_crop_imgproc::Raster;
use moxcms::{ColorProfile, DataColorSpace, Layout, RenderingIntent, TransformOptions};
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// What to do with the colours of a source that carries a non-sRGB profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColourMode {
    /// Convert to sRGB (the default: it looks the same everywhere).
    #[default]
    Srgb,
    /// Keep the pixels and embed the source profile as it is.
    Preserve,
}

/// Notice: the profile could not be used, so the pixels and the profile were kept unchanged.
pub const NOTICE_PROFILE_UNUSABLE: &str = "colour.profile_unusable";
/// Notice: the pixels were converted from a wide-gamut profile to sRGB.
pub const NOTICE_TO_SRGB: &str = "colour.converted_to_srgb";

/// What [`to_srgb`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColourResult {
    /// The pixels were converted (the profile to embed is [`ColourResult::icc`]).
    pub converted: bool,
    /// The profile the output carries: the generated sRGB profile after a conversion, the source's
    /// own profile otherwise (`None` if the source had none).
    pub icc: Option<Vec<u8>>,
    /// A notice code (see the constants above), if there is something to say.
    pub notice: Option<&'static str>,
}

fn srgb_profile() -> &'static (ColorProfile, Vec<u8>) {
    static SRGB: OnceLock<(ColorProfile, Vec<u8>)> = OnceLock::new();
    SRGB.get_or_init(|| {
        let p = ColorProfile::new_srgb();
        let bytes = p.encode().unwrap_or_default();
        (p, bytes)
    })
}

fn options() -> TransformOptions {
    TransformOptions {
        rendering_intent: RenderingIntent::RelativeColorimetric,
        ..TransformOptions::default()
    }
}

/// Rows per band of the transform.
const BAND_ROWS: usize = 64;

/// Probe colours: a 5 x 5 x 5 grid over the cube, which any profile that is not sRGB moves by
/// more than one level somewhere (the narrowest wide-gamut profile differs from sRGB by many
/// levels on a saturated colour).
fn probe_grid() -> Vec<u8> {
    let steps = [0u8, 64, 128, 192, 255];
    let mut v = Vec::with_capacity(125 * 3);
    for r in steps {
        for g in steps {
            for b in steps {
                v.extend_from_slice(&[r, g, b]);
            }
        }
    }
    v
}

/// Converts `raster` (RGB8 in the colours `icc` describes) to sRGB in place, under the rules in the
/// module docs. `icc` is the source profile, byte-exact; `None` is "no profile".
pub fn to_srgb(raster: &mut Raster, icc: Option<&[u8]>) -> ColourResult {
    let Some(bytes) = icc.filter(|b| !b.is_empty()) else {
        return ColourResult {
            converted: false,
            icc: None,
            notice: None,
        };
    };
    let keep = |notice: Option<&'static str>| ColourResult {
        converted: false,
        icc: Some(bytes.to_vec()),
        notice,
    };
    let Ok(src) = ColorProfile::new_from_slice(bytes) else {
        return keep(Some(NOTICE_PROFILE_UNUSABLE));
    };
    // A gray or CMYK profile on an RGB raster is not something to convert (the decode made the
    // raster RGB already): keep it, quietly.
    if src.color_space != DataColorSpace::Rgb {
        return keep(None);
    }
    let (dst, dst_bytes) = srgb_profile();
    if dst_bytes.is_empty() {
        return keep(Some(NOTICE_PROFILE_UNUSABLE));
    }
    let Ok(transform) = src.create_transform_8bit(Layout::Rgb, dst, Layout::Rgb, options()) else {
        return keep(Some(NOTICE_PROFILE_UNUSABLE));
    };
    // Already sRGB, whatever the profile says it is called?
    let probe = probe_grid();
    let mut out = vec![0u8; probe.len()];
    if transform.transform(&probe, &mut out).is_err() {
        return keep(Some(NOTICE_PROFILE_UNUSABLE));
    }
    if probe.iter().zip(&out).all(|(a, b)| a.abs_diff(*b) <= 1) {
        return keep(None);
    }
    let stride = raster.width as usize * 3;
    let mut band_out = vec![0u8; stride * BAND_ROWS];
    for band in raster.data.chunks_mut(stride * BAND_ROWS) {
        let dst_band = &mut band_out[..band.len()];
        if transform.transform(band, dst_band).is_err() {
            // Cannot happen for equal-length RGB bands; if it ever does, the band is untouched
            // but the rest of the image would be inconsistent, so refuse the whole conversion
            // by reporting nothing converted. The caller re-reads the original pixels.
            return keep(Some(NOTICE_PROFILE_UNUSABLE));
        }
        band.copy_from_slice(dst_band);
    }
    ColourResult {
        converted: true,
        icc: Some(dst_bytes.clone()),
        notice: Some(NOTICE_TO_SRGB),
    }
}

/// What an embedded profile is, for reports (the CLI manifest, the CI checks).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IccKind {
    /// No profile (the picture is taken to be sRGB).
    None,
    /// A profile that maps every probe colour to sRGB within one level: sRGB itself.
    Srgb,
    /// Any other profile, or one that cannot be read.
    Other,
}

impl IccKind {
    pub fn name(self) -> &'static str {
        match self {
            IccKind::None => "none",
            IccKind::Srgb => "srgb",
            IccKind::Other => "other",
        }
    }
}

/// Classifies the profile `icc` (see [`IccKind`]).
pub fn classify(icc: Option<&[u8]>) -> IccKind {
    let Some(bytes) = icc.filter(|b| !b.is_empty()) else {
        return IccKind::None;
    };
    let Ok(src) = ColorProfile::new_from_slice(bytes) else {
        return IccKind::Other;
    };
    if src.color_space != DataColorSpace::Rgb {
        return IccKind::Other;
    }
    let (dst, _) = srgb_profile();
    let Ok(t) = src.create_transform_8bit(Layout::Rgb, dst, Layout::Rgb, options()) else {
        return IccKind::Other;
    };
    let probe = probe_grid();
    let mut out = vec![0u8; probe.len()];
    if t.transform(&probe, &mut out).is_ok()
        && probe.iter().zip(&out).all(|(a, b)| a.abs_diff(*b) <= 1)
    {
        IccKind::Srgb
    } else {
        IccKind::Other
    }
}

/// The generated sRGB profile (what a converted file embeds).
pub fn srgb_icc() -> &'static [u8] {
    &srgb_profile().1
}

#[cfg(test)]
mod tests;
