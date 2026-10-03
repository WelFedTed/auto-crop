// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! HEIC, HEIF and AVIF decoding through libheif (cargo feature `heif`, ADR-0009): the pinned,
//! decode-only libheif built by `cargo xtask build-native`, with dav1d for AV1 (AVIF) linked in and
//! libde265 for HEVC (HEIC) as a separate plugin. Everything unsafe lives in [`ffi`]; this module
//! is safe Rust on top of it.
//!
//! The order of a decode (PLAN 3.10.1): file size, the header walk (`parse::heif`: size, depth,
//! orientation, ICC span, item caps, with no pixel allocation), the pixel cap and memory estimate
//! on both the `ispe` size and the cropped size, then libheif with `heif_security_limits` tightened
//! to the same caps, the primary image's size checked again from libheif's own numbers, and only
//! then the pixel decode. The result is 8-bit RGB with the orientation applied once:
//!
//! * libheif applies `clap`, `irot` and `imir` itself, so the pixels are upright;
//!   `Decoded::exif_orientation` reports the equivalent EXIF value for information (what was
//!   applied) and the Exif tag of the file is never consulted (PLAN 3.6: applying it as well
//!   would rotate twice).
//! * more than 8 bits per sample are reduced by libheif (`depth.reduced_to_8`); PQ and HLG
//!   sources are not tone-mapped yet (`hdr.not_tonemapped`);
//! * alpha is dropped, as for PNG and WebP, with `alpha.dropped` when some pixel is translucent;
//! * a file with several top-level images or an image sequence decodes its primary (first) image
//!   with `heic.multi_image`, `heic.sequence` or `anim.first_frame_only`.
//!
//! Libraries and plugins are loaded once per process by [`configure`] or the first decode.

mod ffi;

use crate::decode::{Decoded, extract_icc};
use crate::parse::{Header, HeifCodec};
use crate::{CodecError, DecodeLimits, Format, Limit, est_bytes_for_pixels, guard_item};
use auto_crop_imgproc::Raster;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// The libheif version the build script verified (at least 1.23.5).
pub const LINKED_VERSION: &str = env!("AUTOCROP_HEIF_VERSION");

/// Security floor of libheif (ADR-0004): checked at build time and again on the loaded library.
const VERSION_FLOOR: (u32, u32, u32) = (1, 23, 5);

/// Largest tile count of a grid or overlay (libheif's own default is 16.7 million).
const MAX_TILES: u64 = 65_536;

static PLUGIN_DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
static INIT: OnceLock<Result<(), CodecError>> = OnceLock::new();
static CODEC_THREADS: AtomicU32 = AtomicU32::new(0);

/// Sets the directory the libheif plugins (the libde265 HEVC decoder, `heif-libde265`) are loaded
/// from, in addition to libheif's built-in default directory and `LIBHEIF_PLUGIN_PATH`. Must be
/// called before the first decode or [`init`]; returns false (and changes nothing) afterwards.
pub fn configure(plugin_dir: Option<PathBuf>) -> bool {
    INIT.get().is_none() && PLUGIN_DIR.set(plugin_dir).is_ok()
}

/// Number of threads the codecs (dav1d, libde265) may use per decode; 0 (the default) lets each
/// codec choose, which is every core. The engine lowers it while it decodes several files at once.
pub fn set_codec_threads(n: u32) {
    CODEC_THREADS.store(n, Ordering::Relaxed);
}

fn parse_version(v: &str) -> (u32, u32, u32) {
    let mut it = v
        .split(|c: char| !c.is_ascii_digit())
        .filter(|p| !p.is_empty())
        .map(|p| p.parse().unwrap_or(0));
    (
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
    )
}

/// Loads libheif and its plugins (once per process) and checks the loaded library against the
/// security floor. Called by every decode; call it earlier to get the error early.
pub fn init() -> Result<(), CodecError> {
    INIT.get_or_init(|| {
        let found = ffi::runtime_version();
        if parse_version(&found) < VERSION_FLOOR {
            return Err(CodecError::UnsupportedFeature(format!(
                "the loaded libheif {found} is older than {}.{}.{} (security floor)",
                VERSION_FLOOR.0, VERSION_FLOOR.1, VERSION_FLOOR.2
            )));
        }
        let dir = PLUGIN_DIR.get().and_then(|d| d.as_deref());
        ffi::init(dir).map_err(|e| CodecError::UnsupportedFeature(e.to_string()))
    })
    .clone()
}

/// The version of the libheif that is loaded (for diagnostics and `doctor`).
pub fn runtime_version() -> String {
    ffi::runtime_version()
}

/// True when the libde265 plugin was found: HEVC (HEIC) files can be decoded.
pub fn have_hevc_decoder() -> bool {
    init().is_ok() && ffi::have_decoder(ffi::COMPRESSION_HEVC)
}

/// True when libheif has an AV1 decoder (dav1d): AVIF files can be decoded.
pub fn have_av1_decoder() -> bool {
    init().is_ok() && ffi::have_decoder(ffi::COMPRESSION_AV1)
}

fn limit_err(limits: &DecodeLimits, pixels: u64) -> CodecError {
    CodecError::LimitExceeded {
        limit: Limit::EstBytes,
        actual: est_bytes_for_pixels(pixels),
        cap: limits.max_est_bytes,
    }
}

/// Maps a libheif failure to the typed error.
fn map_err(e: ffi::HeifError, codec: HeifCodec, limits: &DecodeLimits, pixels: u64) -> CodecError {
    let hevc_hint = e.message.contains("HEVC");
    match (e.code, e.subcode) {
        (ffi::ERR_PLUGIN_LOADING, ffi::SUBERR_NO_MATCHING_DECODER) => {
            if codec == HeifCodec::Hevc || hevc_hint {
                CodecError::HevcDecoderMissing
            } else {
                CodecError::UnsupportedFeature(format!("no decoder installed: {}", e.message))
            }
        }
        // AVC, JPEG, JPEG 2000 and VVC items: libheif is built without those decoders.
        (ffi::ERR_UNSUPPORTED_FEATURE, ffi::SUBERR_UNSUPPORTED_CODEC) => {
            CodecError::UnsupportedFeature(format!("HEIF image codec: {}", e.message))
        }
        (ffi::ERR_UNSUPPORTED_FEATURE, _) | (ffi::ERR_PLUGIN_LOADING, _) => {
            CodecError::UnsupportedFeature(e.message)
        }
        // A security limit of libheif (the caps above) or a failed allocation.
        (ffi::ERR_MEMORY, _) => limit_err(limits, pixels),
        (ffi::ERR_CANCELED, _) => CodecError::DecodeTimeout,
        // The codec rejected the bitstream (damaged or truncated coded data).
        (ffi::ERR_DECODER_PLUGIN, _) => {
            CodecError::Corrupt(format!("the image decoder failed: {}", e.message))
        }
        _ => CodecError::Corrupt(e.message),
    }
}

/// Decodes the primary image of a HEIC or AVIF file, `h` being its pre-checked header.
pub(crate) fn decode(
    bytes: &[u8],
    h: &Header,
    limits: &DecodeLimits,
) -> Result<Decoded, CodecError> {
    guard_item(|| decode_inner(bytes, h, limits))
}

fn decode_inner(bytes: &[u8], h: &Header, limits: &DecodeLimits) -> Result<Decoded, CodecError> {
    let info = h
        .heif
        .ok_or_else(|| CodecError::corrupt("the HEIF header walk was not run"))?;
    init()?;
    // Name a missing decoder before touching the file with it.
    if info.codec == HeifCodec::Hevc && !ffi::have_decoder(ffi::COMPRESSION_HEVC) {
        return Err(CodecError::HevcDecoderMissing);
    }
    if info.codec == HeifCodec::Av1 && !ffi::have_decoder(ffi::COMPRESSION_AV1) {
        return Err(CodecError::UnsupportedFeature(
            "libheif has no AV1 decoder (dav1d)".into(),
        ));
    }
    let declared = u64::from(h.width) * u64::from(h.height);
    let flim = ffi::Limits {
        max_pixels: limits.max_pixels,
        max_tiles: MAX_TILES,
        max_icc_bytes: u32::try_from(limits.max_metadata_bytes).unwrap_or(u32::MAX),
        max_block_bytes: limits.max_est_bytes,
        max_total_bytes: limits.max_est_bytes,
    };
    let ctx =
        ffi::Context::read(bytes, &flim).map_err(|e| map_err(e, info.codec, limits, declared))?;
    let handle = ctx
        .primary()
        .map_err(|e| map_err(e, info.codec, limits, declared))?;

    // The caps again, on libheif's own numbers, before any pixel buffer exists.
    let (cw, ch) = handle.size();
    let (iw, ih) = handle.ispe_size();
    let pixels = (u64::from(cw) * u64::from(ch)).max(u64::from(iw) * u64::from(ih));
    if pixels == 0 {
        return Err(CodecError::corrupt("image has a zero dimension"));
    }
    check_pixels(pixels, limits)?;

    let deadline = limits
        .max_decode_ms
        .map(|ms| Instant::now() + Duration::from_millis(ms));
    let opts = ffi::Options::new(deadline, CODEC_THREADS.load(Ordering::Relaxed))
        .map_err(|e| CodecError::Corrupt(e.to_string()))?;
    let rgba = handle.has_alpha();
    let img = handle
        .decode(rgba, &opts)
        .map_err(|e| map_err(e, info.codec, limits, pixels))?;

    let (w, ht) = img.size();
    check_pixels(w as u64 * ht as u64, limits)?;
    let bpp = if rgba { 4 } else { 3 };
    let len = w
        .checked_mul(ht)
        .and_then(|p| p.checked_mul(3))
        .ok_or_else(|| limit_err(limits, w as u64 * ht as u64))?;
    let mut data: Vec<u8> = Vec::new();
    data.try_reserve_exact(len)
        .map_err(|_| limit_err(limits, w as u64 * ht as u64))?;
    let mut translucent = false;
    img.rows(bpp, |_, row| {
        if rgba {
            for px in row.as_chunks::<4>().0 {
                translucent |= px[3] != 255;
                data.extend_from_slice(&px[..3]);
            }
        } else {
            data.extend_from_slice(row);
        }
    })
    .map_err(|e| CodecError::Corrupt(e.to_string()))?;
    let raster = Raster::from_raw(w as u32, ht as u32, data)
        .ok_or_else(|| CodecError::corrupt("pixel buffer size mismatch"))?;

    let top_level = ctx.top_level_images().max(1);
    let mut notices: Vec<&'static str> = Vec::new();
    if ctx.has_sequence() {
        notices.push(if h.format == Format::Avif {
            "anim.first_frame_only"
        } else {
            "heic.sequence"
        });
    }
    if top_level > 1 {
        notices.push("heic.multi_image");
    }
    let source_bits = handle.luma_bits().unwrap_or(h.bit_depth);
    if source_bits > 8 {
        notices.push("depth.reduced_to_8");
    }
    if translucent {
        notices.push("alpha.dropped");
    }
    let icc = extract_icc(bytes, h, None, &mut notices);
    if let Some(n) = info.nclx {
        // H.273: transfer 16 is PQ, 18 is HLG; primaries 1 is BT.709 (sRGB), 2 unspecified.
        if matches!(n.transfer, 16 | 18) {
            notices.push("hdr.not_tonemapped");
        }
        if icc.is_none() && !matches!(n.primaries, 1 | 2) {
            notices.push("colour.nclx_not_srgb");
        }
    }

    Ok(Decoded {
        raster,
        format: h.format,
        exif_orientation: h.orientation,
        icc,
        source_bit_depth: source_bits,
        frames: top_level,
        notices,
    })
}

fn check_pixels(pixels: u64, limits: &DecodeLimits) -> Result<(), CodecError> {
    if pixels > limits.max_pixels {
        return Err(CodecError::TooLarge(pixels));
    }
    if est_bytes_for_pixels(pixels) > limits.max_est_bytes {
        return Err(limit_err(limits, pixels));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
