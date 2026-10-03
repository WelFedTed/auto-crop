// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The libjpeg-turbo path (cargo feature `turbojpeg`, ROADMAP M1.18 to M1.21): DCT-domain scaled
//! decode, lossless transforms into caller-owned buffers and the baseline JPEG encoder. Everything
//! unsafe lives in [`ffi`]; this module is safe Rust on top of it.
//!
//! The library is the one built from the pin in `native-deps.toml` (>= 3.1.4, checked by the build
//! script). Untrusted bytes still pass the same pre-checks as every other decoder
//! ([`crate::decode::precheck`]: size caps, header probe, pixel cap, plausibility, truncation)
//! before libjpeg-turbo sees them, and the library runs with `TJPARAM_STOPONWARNING` (a damaged
//! scan is an error, never a grey-filled success) and the scan cap of the limits.

pub(crate) mod ffi;
mod transform;

pub use ffi::LINKED_VERSION;
pub use transform::{Transformer, transform};

use crate::{CodecError, DecodeLimits};
use auto_crop_imgproc::Raster;
use ffi::{
    Handle, TJINIT_COMPRESS, TJINIT_DECOMPRESS, TJPARAM_DENSITYUNITS, TJPARAM_QUALITY,
    TJPARAM_SCANLIMIT, TJPARAM_STOPONWARNING, TJPARAM_SUBSAMP, TJPARAM_XDENSITY, TJPARAM_YDENSITY,
    TJPF_GRAY, TJPF_RGB, TJSAMP_420, TJSAMP_GRAY,
};

/// Maps a TurboJPEG error text to the typed error.
pub(crate) fn map_err(msg: String) -> CodecError {
    if msg.contains("nsupported") {
        CodecError::UnsupportedFeature(msg)
    } else {
        CodecError::Corrupt(msg)
    }
}

/// Decodes `bytes` to RGB8 at 1/`denom` scale (1, 2, 4 or 8) with libjpeg-turbo. `width` and
/// `height` are the stored size from the pre-checked header; the library must agree. No EXIF turn
/// is applied. Returns the raster at `ceil(width / denom) x ceil(height / denom)`.
pub(crate) fn decode_rgb(
    bytes: &[u8],
    denom: u32,
    width: u32,
    height: u32,
    limits: &DecodeLimits,
) -> Result<Raster, CodecError> {
    let mut tj = Handle::new(TJINIT_DECOMPRESS).map_err(CodecError::Corrupt)?;
    tj.set(TJPARAM_STOPONWARNING, 1).map_err(map_err)?;
    tj.set(
        TJPARAM_SCANLIMIT,
        i32::try_from(limits.max_scans).unwrap_or(i32::MAX),
    )
    .map_err(map_err)?;
    let info = tj.read_header(bytes).map_err(map_err)?;
    if (info.width, info.height) != (width as usize, height as usize) {
        return Err(CodecError::corrupt(
            "header and decoder disagree about the image size",
        ));
    }
    if info.precision != 8 {
        return Err(CodecError::UnsupportedFeature(format!(
            "{}-bit JPEG",
            info.precision
        )));
    }
    let max_samples = usize::try_from(limits.max_pixels.saturating_mul(3)).unwrap_or(usize::MAX);
    let denom = i32::try_from(denom).map_err(|_| CodecError::corrupt("bad scaling factor"))?;
    let (rgb, sw, sh) = tj
        .decompress8(bytes, denom, TJPF_RGB, max_samples)
        .map_err(map_err)?;
    Raster::from_raw(sw as u32, sh as u32, rgb)
        .ok_or_else(|| CodecError::corrupt("pixel buffer size mismatch"))
}

/// Baseline JPEG of 8-bit `data` (`width * height` grey samples or RGB triples) at `quality`
/// (1..=100), 4:2:0 for colour; embeds `dpi` (JFIF density) and the ICC profile when given.
pub(crate) fn compress(
    data: &[u8],
    width: usize,
    height: usize,
    gray: bool,
    quality: u8,
    dpi: Option<(u32, u32)>,
    icc: Option<&[u8]>,
) -> Result<Vec<u8>, CodecError> {
    let mut tj = Handle::new(TJINIT_COMPRESS).map_err(CodecError::Encode)?;
    let enc = |e: String| CodecError::Encode(e);
    tj.set(TJPARAM_QUALITY, i32::from(quality.clamp(1, 100)))
        .map_err(enc)?;
    tj.set(TJPARAM_SUBSAMP, if gray { TJSAMP_GRAY } else { TJSAMP_420 })
        .map_err(enc)?;
    if let Some((x, y)) = dpi {
        let to = |v: u32| i32::try_from(v.clamp(1, 65_535)).unwrap_or(1);
        tj.set(TJPARAM_DENSITYUNITS, 1).map_err(enc)?; // 1 = dots per inch
        tj.set(TJPARAM_XDENSITY, to(x)).map_err(enc)?;
        tj.set(TJPARAM_YDENSITY, to(y)).map_err(enc)?;
    }
    tj.compress8(
        data,
        width,
        height,
        if gray { TJPF_GRAY } else { TJPF_RGB },
        icc,
    )
    .map_err(enc)
}

#[cfg(test)]
mod tests;
