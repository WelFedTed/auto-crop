// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Lossless JPEG transforms through libjpeg-turbo (ROADMAP M1.20): the same operations, policies
//! and realised-rectangle rules as the safe-Rust [`crate::jpeg_lossless`] (both use
//! `jpeg_lossless::plan`), but the coefficient shuffling is done by `tj3Transform` into a buffer
//! that this struct owns and **reuses across calls** (`TJPARAM_NOREALLOC`: the library writes into
//! our memory or fails, it never reallocates). That reuse is what the sanitizer job exercises.

use super::ffi::{
    Handle, JpegInfo, Region, TJINIT_TRANSFORM, TJPARAM_OPTIMIZE, TJPARAM_SCANLIMIT,
    TJPARAM_STOPONWARNING, TJSAMP_411, TJSAMP_420, TJSAMP_422, TJSAMP_440, TJSAMP_441, TJSAMP_444,
    TJSAMP_GRAY, TJXOP_HFLIP, TJXOP_NONE, TJXOP_ROT90, TJXOP_ROT180, TJXOP_ROT270, TJXOP_TRANSPOSE,
    TJXOP_TRANSVERSE, TJXOP_VFLIP, TJXOPT_CROP, TJXOPT_PERFECT, TJXOPT_TRIM, TransformSpec,
    jpeg_buf_size,
};
use super::map_err;
use crate::jpeg_lossless::{Op, Plan, Policy, Rect, Transformed, plan};
use crate::{CodecError, DecodeLimits, Format, guard_item};

/// iMCU size in pixels for a TurboJPEG subsampling constant.
fn mcu_size(subsamp: i32) -> Option<(usize, usize)> {
    Some(match subsamp {
        TJSAMP_444 | TJSAMP_GRAY => (8, 8),
        TJSAMP_422 => (16, 8),
        TJSAMP_420 => (16, 16),
        TJSAMP_440 => (8, 16),
        TJSAMP_411 => (32, 8),
        TJSAMP_441 => (8, 32),
        _ => return None,
    })
}

fn spec(op: Op, policy: Policy, rect: Rect) -> TransformSpec {
    let trim = match policy {
        Policy::Perfect => TJXOPT_PERFECT,
        Policy::Snap => TJXOPT_TRIM,
    };
    let to_i = |v: u32| i32::try_from(v).unwrap_or(i32::MAX);
    match op {
        Op::Crop(_) => TransformSpec {
            op: TJXOP_NONE,
            options: TJXOPT_CROP,
            region: Region {
                x: to_i(rect.x),
                y: to_i(rect.y),
                w: to_i(rect.w),
                h: to_i(rect.h),
            },
        },
        other => TransformSpec {
            op: match other {
                Op::Rotate90 => TJXOP_ROT90,
                Op::Rotate180 => TJXOP_ROT180,
                Op::Rotate270 => TJXOP_ROT270,
                Op::FlipH => TJXOP_HFLIP,
                Op::FlipV => TJXOP_VFLIP,
                Op::Transpose => TJXOP_TRANSPOSE,
                _ => TJXOP_TRANSVERSE,
            },
            options: trim,
            region: Region::default(),
        },
    }
}

/// A reusable libjpeg-turbo lossless transformer: one handle and one output buffer that live as
/// long as the value, so a batch of transforms allocates once.
pub struct Transformer {
    tj: Handle,
    buf: Vec<u8>,
}

impl Transformer {
    pub fn new() -> Result<Self, CodecError> {
        let mut tj = Handle::new(TJINIT_TRANSFORM).map_err(CodecError::Corrupt)?;
        tj.set(TJPARAM_OPTIMIZE, 1).map_err(map_err)?;
        Ok(Self {
            tj,
            buf: Vec::new(),
        })
    }

    /// Capacity of the reused output buffer (grows, never shrinks).
    pub fn buffer_capacity(&self) -> usize {
        self.buf.len()
    }

    /// Applies `op` to the JPEG `bytes` without decoding a pixel. Same contract as
    /// [`crate::jpeg_lossless::transform`]; additionally accepts progressive input. A panic inside
    /// becomes [`CodecError::InternalPanic`].
    pub fn transform(
        &mut self,
        bytes: &[u8],
        op: Op,
        policy: Policy,
        limits: &DecodeLimits,
    ) -> Result<Transformed, CodecError> {
        guard_item(|| self.transform_inner(bytes, op, policy, limits))
    }

    fn transform_inner(
        &mut self,
        bytes: &[u8],
        op: Op,
        policy: Policy,
        limits: &DecodeLimits,
    ) -> Result<Transformed, CodecError> {
        // Same pre-checks as every decode: caps, header probe, plausibility, truncation.
        let (format, _h) = crate::decode::precheck(bytes, limits)?;
        if format != Format::Jpeg {
            return Err(CodecError::Unsupported);
        }
        self.tj.set(TJPARAM_STOPONWARNING, 1).map_err(map_err)?;
        self.tj
            .set(
                TJPARAM_SCANLIMIT,
                i32::try_from(limits.max_scans).unwrap_or(i32::MAX),
            )
            .map_err(map_err)?;
        let info: JpegInfo = self.tj.read_header(bytes).map_err(map_err)?;
        if info.precision != 8 {
            return Err(CodecError::UnsupportedFeature(format!(
                "{}-bit JPEG",
                info.precision
            )));
        }
        let (mcu_w, mcu_h) = mcu_size(info.subsamp).ok_or_else(|| {
            CodecError::UnsupportedFeature("unusual chroma subsampling".to_owned())
        })?;
        let Plan {
            rect,
            perfect,
            transposes,
        } = plan(op, policy, info.width, info.height, mcu_w, mcu_h)?;
        let (out_w, out_h) = if transposes {
            (rect.h as usize, rect.w as usize)
        } else {
            (rect.w as usize, rect.h as usize)
        };
        let spec = spec(op, policy, rect);

        // First try a buffer of about the input's size; fall back to the worst case once.
        let small = bytes.len() + bytes.len() / 2 + (64 << 10);
        let worst = jpeg_buf_size(out_w, out_h, info.subsamp)
            .map_err(CodecError::Corrupt)?
            .saturating_add(bytes.len());
        let mut cap = small.min(worst);
        let n = loop {
            if self.buf.len() < cap {
                self.buf.resize(cap, 0);
            }
            match self.tj.transform(bytes, &spec, &mut self.buf[..cap]) {
                Ok(n) => break n,
                Err(_) if cap < worst => cap = worst,
                Err(e) => return Err(map_err(e)),
            }
        };
        let out = self.buf[..n].to_vec();

        // The result must be a JPEG of exactly the size the plan promised.
        let check = self.tj.read_header(&out).map_err(map_err)?;
        if (check.width, check.height) != (out_w, out_h) {
            return Err(CodecError::corrupt(
                "libjpeg-turbo produced an image of an unexpected size",
            ));
        }
        Ok(Transformed {
            bytes: out,
            width: out_w as u32,
            height: out_h as u32,
            rect,
            perfect,
        })
    }
}

/// One-shot [`Transformer::transform`].
pub fn transform(
    bytes: &[u8],
    op: Op,
    policy: Policy,
    limits: &DecodeLimits,
) -> Result<Transformed, CodecError> {
    Transformer::new()?.transform(bytes, op, policy, limits)
}
