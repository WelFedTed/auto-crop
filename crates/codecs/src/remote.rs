// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The seam between the decoders and the sandboxed worker pool (ADR-0006, ADR-0013).
//!
//! `codecs` stays free of operating-system code: it only knows a [`RemoteDecoder`], a trait object
//! that the application installs at start-up (`auto-crop-sandbox` implements it with a pool of
//! sandboxed `auto-crop-worker` processes). Every decode of a format the remote decoder
//! [handles](RemoteDecoder::handles) goes there *after* the parent-side checks (file size, header
//! walk, pixel cap, memory estimate, unsupported features) and returns through
//! [`check_reply`], which distrusts the worker's answer: sizes, orientation, frame count and
//! profile length are checked against the same [`DecodeLimits`] again.
//!
//! Without an installed remote decoder nothing changes: HEIC and AVIF decode in-process.

use crate::{CodecError, DecodeLimits, Decoded, Format, Limit};
use std::sync::{Arc, RwLock};

/// One decode as the worker is asked to do it.
#[derive(Debug, Clone, Copy)]
pub struct RemoteRequest<'a> {
    pub bytes: &'a [u8],
    pub limits: &'a DecodeLimits,
    /// Apply the EXIF orientation (false for the scaled decode, which turns afterwards).
    pub apply_orientation: bool,
    /// Composite translucent pixels over white (`alpha.flattened`) instead of dropping alpha.
    pub flatten_alpha: bool,
}

/// A decoder that runs somewhere else (a sandboxed worker process).
pub trait RemoteDecoder: Send + Sync {
    /// True when this decoder takes files of `format`.
    fn handles(&self, format: Format) -> bool;
    /// Decodes. The parent-side checks have already passed; the worker repeats them.
    fn decode(&self, req: RemoteRequest<'_>) -> Result<Decoded, CodecError>;
}

static REMOTE: RwLock<Option<Arc<dyn RemoteDecoder>>> = RwLock::new(None);

/// Installs (or, with `None`, removes) the process-wide remote decoder.
pub fn set_remote_decoder(remote: Option<Arc<dyn RemoteDecoder>>) {
    if let Ok(mut g) = REMOTE.write() {
        *g = remote;
    }
}

/// The installed remote decoder, if it takes `format`.
pub(crate) fn remote_for(format: Format) -> Option<Arc<dyn RemoteDecoder>> {
    let g = REMOTE.read().ok()?;
    g.as_ref().filter(|r| r.handles(format)).cloned()
}

/// True when a remote decoder is installed and takes `format`.
pub fn remote_handles(format: Format) -> bool {
    remote_for(format).is_some()
}

fn limit(limit: Limit, actual: u64, cap: u64) -> CodecError {
    CodecError::LimitExceeded { limit, actual, cap }
}

/// Re-checks a decode that came back from another process against the caps it was given. A worker
/// is untrusted: a compromised one could claim any size, orientation or profile.
pub fn check_reply(d: Decoded, limits: &DecodeLimits) -> Result<Decoded, CodecError> {
    let (w, h) = (u64::from(d.raster.width), u64::from(d.raster.height));
    if w == 0 || h == 0 {
        return Err(CodecError::Corrupt(
            "the worker returned an empty image".into(),
        ));
    }
    if w * h > limits.max_pixels {
        return Err(CodecError::TooLarge(w * h));
    }
    if d.raster.data.len() as u64 != w * h * 3 {
        return Err(CodecError::Corrupt(
            "the worker returned a pixel buffer of the wrong size".into(),
        ));
    }
    if !(1..=8).contains(&d.exif_orientation) {
        return Err(CodecError::Corrupt(
            "the worker returned an invalid orientation".into(),
        ));
    }
    if u64::from(d.frames) > u64::from(limits.max_frames) {
        return Err(limit(
            Limit::Frames,
            u64::from(d.frames),
            u64::from(limits.max_frames),
        ));
    }
    if let Some(icc) = &d.icc
        && icc.len() as u64 > limits.max_metadata_bytes
    {
        return Err(limit(
            Limit::MetadataBytes,
            icc.len() as u64,
            limits.max_metadata_bytes,
        ));
    }
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use auto_crop_imgproc::Raster;

    fn decoded(w: u32, h: u32) -> Decoded {
        Decoded {
            raster: Raster::new(w, h),
            format: Format::Png,
            exif_orientation: 1,
            icc: None,
            source_bit_depth: 8,
            frames: 1,
            notices: vec![],
        }
    }

    #[test]
    fn a_plausible_reply_passes_and_hostile_ones_are_refused() {
        let l = DecodeLimits::default().with_max_pixels(1000);
        assert!(check_reply(decoded(10, 10), &l).is_ok());
        assert_eq!(
            check_reply(decoded(100, 100), &l).unwrap_err(),
            CodecError::TooLarge(10_000)
        );
        let mut bad = decoded(4, 4);
        bad.exif_orientation = 9;
        assert!(matches!(check_reply(bad, &l), Err(CodecError::Corrupt(_))));
        let mut bad = decoded(4, 4);
        bad.frames = u32::MAX;
        assert!(matches!(
            check_reply(bad, &l),
            Err(CodecError::LimitExceeded {
                limit: Limit::Frames,
                ..
            })
        ));
        let mut bad = decoded(4, 4);
        bad.icc = Some(vec![0; (l.max_metadata_bytes + 1) as usize]);
        assert!(matches!(
            check_reply(bad, &l),
            Err(CodecError::LimitExceeded {
                limit: Limit::MetadataBytes,
                ..
            })
        ));
    }

    #[test]
    fn nothing_is_remote_until_a_decoder_is_installed() {
        assert!(!remote_handles(Format::Heic));
    }
}
