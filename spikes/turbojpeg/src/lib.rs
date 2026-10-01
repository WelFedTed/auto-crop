// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Spike for ROADMAP M0.44: TurboJPEG 3 through our own libjpeg-turbo (>= 3.1.4).
//! Tests: 1/4 scaled decode, lossless 90 degree rotate, MCU-aligned crop, both into a
//! caller-owned (pre-allocated) buffer, and the errors for the cases that cannot be lossless.

use std::ffi::{CStr, c_char, c_int, c_void};

type Handle = *mut c_void;

const TJINIT_COMPRESS: c_int = 0;
const TJINIT_DECOMPRESS: c_int = 1;
const TJINIT_TRANSFORM: c_int = 2;
const TJSAMP_420: c_int = 2;
const TJPF_RGB: c_int = 0;
const TJPARAM_NOREALLOC: c_int = 2;
const TJPARAM_QUALITY: c_int = 3;
const TJPARAM_SUBSAMP: c_int = 4;
const TJPARAM_JPEGWIDTH: c_int = 5;
const TJPARAM_JPEGHEIGHT: c_int = 6;
const TJPARAM_FASTUPSAMPLE: c_int = 9;
const TJXOP_NONE: c_int = 0;
const TJXOP_ROT90: c_int = 5;
const TJXOPT_PERFECT: c_int = 1;
const TJXOPT_CROP: c_int = 4;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Region {
    x: c_int,
    y: c_int,
    w: c_int,
    h: c_int,
}

#[repr(C)]
struct ScalingFactor {
    num: c_int,
    denom: c_int,
}

#[repr(C)]
struct Transform {
    r: Region,
    op: c_int,
    options: c_int,
    data: *mut c_void,
    custom_filter: *mut c_void,
}

unsafe extern "C" {
    fn tj3InitVersion(init_type: c_int, api_version: c_int) -> Handle;
    fn tj3Destroy(h: Handle);
    fn tj3GetErrorStr(h: Handle) -> *mut c_char;
    fn tj3Set(h: Handle, param: c_int, value: c_int) -> c_int;
    fn tj3Get(h: Handle, param: c_int) -> c_int;
    fn tj3Free(p: *mut c_void);
    fn tj3JPEGBufSize(w: c_int, h: c_int, subsamp: c_int) -> usize;
    fn tj3Compress8(
        h: Handle,
        src: *const u8,
        w: c_int,
        pitch: c_int,
        hgt: c_int,
        pf: c_int,
        jpeg: *mut *mut u8,
        size: *mut usize,
    ) -> c_int;
    fn tj3DecompressHeader(h: Handle, jpeg: *const u8, size: usize) -> c_int;
    fn tj3SetScalingFactor(h: Handle, sf: ScalingFactor) -> c_int;
    fn tj3Decompress8(h: Handle, jpeg: *const u8, size: usize, dst: *mut u8, pitch: c_int, pf: c_int) -> c_int;
    fn tj3Transform(
        h: Handle,
        jpeg: *const u8,
        size: usize,
        n: c_int,
        dst: *mut *mut u8,
        sizes: *mut usize,
        t: *const Transform,
    ) -> c_int;
}

/// The version number the build script verified (>= 3001004).
pub const LINKED_VERSION: u32 = parse_version(env!("TURBOJPEG_VERSION_NUMBER"));

const fn parse_version(s: &str) -> u32 {
    let b = s.as_bytes();
    let (mut i, mut v) = (0, 0u32);
    while i < b.len() {
        v = v * 10 + (b[i] - b'0') as u32;
        i += 1;
    }
    v
}

pub struct Tj(Handle);

impl Drop for Tj {
    fn drop(&mut self) {
        unsafe { tj3Destroy(self.0) }
    }
}

impl Tj {
    pub fn new(kind: c_int) -> Self {
        let h = unsafe { tj3InitVersion(kind, LINKED_VERSION as c_int) };
        assert!(!h.is_null(), "tj3Init failed");
        Tj(h)
    }

    fn err(&self) -> String {
        unsafe { CStr::from_ptr(tj3GetErrorStr(self.0)).to_string_lossy().into_owned() }
    }

    fn check(&self, rc: c_int) -> Result<(), String> {
        if rc == 0 { Ok(()) } else { Err(self.err()) }
    }

    fn set(&self, p: c_int, v: c_int) {
        assert_eq!(unsafe { tj3Set(self.0, p, v) }, 0, "{}", self.err());
    }

    fn get(&self, p: c_int) -> c_int {
        unsafe { tj3Get(self.0, p) }
    }
}

pub fn compress_rgb(rgb: &[u8], w: usize, h: usize, quality: i32) -> Vec<u8> {
    let tj = Tj::new(TJINIT_COMPRESS);
    tj.set(TJPARAM_QUALITY, quality);
    tj.set(TJPARAM_SUBSAMP, TJSAMP_420);
    let (mut buf, mut size) = (std::ptr::null_mut(), 0usize);
    let rc = unsafe {
        tj3Compress8(tj.0, rgb.as_ptr(), w as c_int, (w * 3) as c_int, h as c_int, TJPF_RGB, &mut buf, &mut size)
    };
    tj.check(rc).expect("compress");
    let out = unsafe { std::slice::from_raw_parts(buf, size) }.to_vec();
    unsafe { tj3Free(buf.cast()) };
    out
}

/// Decodes to RGB at 1/`denom` scale (denom 1, 2, 4 or 8). Returns (pixels, width, height).
pub fn decode_scaled(jpeg: &[u8], denom: i32) -> Result<(Vec<u8>, usize, usize), String> {
    let tj = Tj::new(TJINIT_DECOMPRESS);
    tj.set(TJPARAM_FASTUPSAMPLE, 1);
    tj.check(unsafe { tj3DecompressHeader(tj.0, jpeg.as_ptr(), jpeg.len()) })?;
    let (w, h) = (tj.get(TJPARAM_JPEGWIDTH), tj.get(TJPARAM_JPEGHEIGHT));
    tj.check(unsafe { tj3SetScalingFactor(tj.0, ScalingFactor { num: 1, denom }) })?;
    let (sw, sh) = ((w + denom - 1) / denom, (h + denom - 1) / denom);
    let mut out = vec![0u8; (sw * sh * 3) as usize];
    tj.check(unsafe { tj3Decompress8(tj.0, jpeg.as_ptr(), jpeg.len(), out.as_mut_ptr(), sw * 3, TJPF_RGB) })?;
    Ok((out, sw as usize, sh as usize))
}

/// Lossless transform into a **caller-owned** buffer of `capacity` bytes (NOREALLOC).
/// Returns the JPEG bytes written.
fn transform_into(jpeg: &[u8], op: c_int, options: c_int, region: Region, capacity: usize) -> Result<Vec<u8>, String> {
    let tj = Tj::new(TJINIT_TRANSFORM);
    tj.set(TJPARAM_NOREALLOC, 1);
    let mut dst = vec![0u8; capacity];
    let mut ptr = dst.as_mut_ptr();
    let mut size = capacity;
    let t = Transform { r: region, op, options, data: std::ptr::null_mut(), custom_filter: std::ptr::null_mut() };
    tj.check(unsafe { tj3Transform(tj.0, jpeg.as_ptr(), jpeg.len(), 1, &mut ptr, &mut size, &t) })?;
    assert_eq!(ptr, dst.as_mut_ptr(), "the library must write into our buffer, not reallocate");
    dst.truncate(size);
    Ok(dst)
}

/// Lossless rotate by 90 degrees clockwise (MCU-aligned images only: PERFECT).
pub fn rotate90(jpeg: &[u8], w: usize, h: usize) -> Result<Vec<u8>, String> {
    let cap = unsafe { tj3JPEGBufSize(h as c_int, w as c_int, TJSAMP_420) };
    transform_into(jpeg, TJXOP_ROT90, TJXOPT_PERFECT, Region::default(), cap)
}

/// Lossless crop; x, y must be multiples of the MCU size (16 for 4:2:0).
pub fn crop(jpeg: &[u8], x: i32, y: i32, w: i32, h: i32) -> Result<Vec<u8>, String> {
    let cap = unsafe { tj3JPEGBufSize(w, h, TJSAMP_420) };
    transform_into(jpeg, TJXOP_NONE, TJXOPT_CROP, Region { x, y, w, h }, cap)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_image(w: usize, h: usize) -> Vec<u8> {
        let mut v = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let block = if (x / 8 + y / 8) % 2 == 0 { 60 } else { 0 };
                let p = (y * w + x) * 3;
                v[p] = ((x * 4 + block) % 256) as u8;
                v[p + 1] = ((y * 5) % 256) as u8;
                v[p + 2] = (((x + y) * 2 + block) % 256) as u8;
            }
        }
        v
    }

    #[test]
    fn linked_version_is_at_least_3_1_4() {
        assert!(LINKED_VERSION >= 3_001_004, "{LINKED_VERSION}");
    }

    #[test]
    fn quarter_scale_decode_is_one_sixteenth_the_pixels_and_close_to_a_box_filter() {
        let (w, h) = (64, 48);
        let jpeg = compress_rgb(&test_image(w, h), w, h, 95);
        let (full, fw, fh) = decode_scaled(&jpeg, 1).unwrap();
        let (small, sw, sh) = decode_scaled(&jpeg, 4).unwrap();
        assert_eq!((fw, fh, sw, sh), (64, 48, 16, 12));
        let mut total = 0u64;
        for y in 0..sh {
            for x in 0..sw {
                for c in 0..3 {
                    let mut s = 0u32;
                    for dy in 0..4 {
                        for dx in 0..4 {
                            s += full[((y * 4 + dy) * fw + x * 4 + dx) * 3 + c] as u32;
                        }
                    }
                    total += (s / 16).abs_diff(small[(y * sw + x) * 3 + c] as u32) as u64;
                }
            }
        }
        let mean_abs = total as f64 / (sw * sh * 3) as f64;
        println!("1/4 scaled decode: mean abs difference to a 4x4 box filter = {mean_abs:.2}");
        assert!(mean_abs < 10.0, "{mean_abs}");
    }

    #[test]
    fn lossless_rotate90_equals_rotating_the_decoded_pixels() {
        let (w, h) = (64, 48); // multiples of the 16x16 MCU: PERFECT is possible
        let jpeg = compress_rgb(&test_image(w, h), w, h, 92);
        let rotated = rotate90(&jpeg, w, h).expect("rotate");
        let (orig, _, _) = decode_scaled(&jpeg, 1).unwrap();
        let (rot, rw, rh) = decode_scaled(&rotated, 1).unwrap();
        assert_eq!((rw, rh), (h, w));
        let mut max_diff = 0u8;
        for y in 0..h {
            for x in 0..w {
                // 90 degrees clockwise: (x, y) -> (h - 1 - y, x)
                for c in 0..3 {
                    let a = orig[(y * w + x) * 3 + c];
                    let b = rot[(x * rw + (h - 1 - y)) * 3 + c];
                    max_diff = max_diff.max(a.abs_diff(b));
                }
            }
        }
        println!("lossless rotate90: max pixel difference vs rotated decode = {max_diff}");
        assert_eq!(max_diff, 0);
    }

    #[test]
    fn mcu_aligned_crop_equals_the_cropped_decoded_pixels() {
        let (w, h) = (64, 48);
        let jpeg = compress_rgb(&test_image(w, h), w, h, 92);
        let cropped = crop(&jpeg, 16, 16, 32, 16).expect("crop");
        let (orig, _, _) = decode_scaled(&jpeg, 1).unwrap();
        let (c, cw, ch) = decode_scaled(&cropped, 1).unwrap();
        assert_eq!((cw, ch), (32, 16));
        for y in 0..ch {
            for x in 0..cw {
                for k in 0..3 {
                    assert_eq!(c[(y * cw + x) * 3 + k], orig[((y + 16) * w + x + 16) * 3 + k], "at {x},{y}");
                }
            }
        }
    }

    #[test]
    fn a_buffer_that_is_too_small_is_an_error_not_a_reallocation() {
        let (w, h) = (64, 48);
        let jpeg = compress_rgb(&test_image(w, h), w, h, 92);
        assert!(transform_into(&jpeg, TJXOP_ROT90, TJXOPT_PERFECT, Region::default(), 16).is_err());
    }

    #[test]
    fn perfect_rotation_of_a_non_mcu_aligned_image_is_refused() {
        let (w, h) = (70, 50); // not multiples of 16: partial iMCUs on the edges
        let jpeg = compress_rgb(&test_image(w, h), w, h, 92);
        let r = rotate90(&jpeg, w, h);
        println!("non-aligned PERFECT rotate: {r:?}");
        assert!(r.is_err(), "the engine must fall back to a re-encode (or trim) when a rotate cannot be lossless");
    }
}
