// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Spike for ROADMAP M0.43: decode a HEIC to RGB8 from bytes with a hand-written binding to
//! libheif's C API (about ten functions). The libde265 HEVC decoder is a separate plugin that
//! `heif_init` loads from the plugin directories (compile-time default plus LIBHEIF_PLUGIN_PATH).
//! If the plugin is absent, decoding an HEVC HEIC fails with a clear error: the "no-hevc" behaviour.

use std::ffi::{CStr, c_char, c_int, c_void};

#[repr(C)]
#[derive(Clone, Copy)]
struct HeifError {
    code: c_int,
    subcode: c_int,
    message: *const c_char,
}

#[repr(C)]
struct InitParams {
    version: c_int,
}

const HEIF_ERROR_OK: c_int = 0;
const COLORSPACE_RGB: c_int = 1;
const CHROMA_INTERLEAVED_RGB: c_int = 10;
const CHANNEL_INTERLEAVED: c_int = 10;

unsafe extern "C" {
    fn heif_get_version() -> *const c_char;
    fn heif_init(params: *mut InitParams) -> HeifError;
    fn heif_deinit();
    fn heif_context_alloc() -> *mut c_void;
    fn heif_context_free(ctx: *mut c_void);
    fn heif_context_read_from_memory_without_copy(ctx: *mut c_void, mem: *const c_void, size: usize, opts: *const c_void) -> HeifError;
    fn heif_context_get_primary_image_handle(ctx: *mut c_void, out: *mut *mut c_void) -> HeifError;
    fn heif_image_handle_release(h: *const c_void);
    fn heif_decode_image(h: *const c_void, out: *mut *mut c_void, colorspace: c_int, chroma: c_int, opts: *const c_void) -> HeifError;
    fn heif_image_release(img: *const c_void);
    fn heif_image_get_width(img: *const c_void, channel: c_int) -> c_int;
    fn heif_image_get_height(img: *const c_void, channel: c_int) -> c_int;
    fn heif_image_get_plane_readonly2(img: *const c_void, channel: c_int, stride: *mut usize) -> *const u8;
}

/// The libheif version the build script verified (>= 1.23.5).
pub const LINKED_VERSION: &str = env!("LIBHEIF_LINKED_VERSION");

/// Runtime library version string.
pub fn runtime_version() -> String {
    unsafe { CStr::from_ptr(heif_get_version()).to_string_lossy().into_owned() }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeicError {
    pub code: i32,
    pub subcode: i32,
    pub message: String,
}

fn check(e: HeifError) -> Result<(), HeicError> {
    if e.code == HEIF_ERROR_OK {
        Ok(())
    } else {
        let message = if e.message.is_null() { String::new() } else { unsafe { CStr::from_ptr(e.message).to_string_lossy().into_owned() } };
        Err(HeicError { code: e.code, subcode: e.subcode, message })
    }
}

/// A decoded image: tightly packed RGB8 (stride removed).
#[derive(Debug)]
pub struct Rgb8 {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

/// Decodes the primary image of `bytes` to RGB8. With `load_plugins` the plugin directories are
/// scanned first (`heif_init`); without it only codecs compiled into libheif are available.
pub fn decode_rgb8(bytes: &[u8], load_plugins: bool) -> Result<Rgb8, HeicError> {
    unsafe {
        if load_plugins {
            let mut p = InitParams { version: 1 };
            check(heif_init(&mut p))?;
        }
        let result = decode_inner(bytes);
        if load_plugins {
            heif_deinit();
        }
        result
    }
}

unsafe fn decode_inner(bytes: &[u8]) -> Result<Rgb8, HeicError> {
    unsafe {
        let ctx = heif_context_alloc();
        let mut handle: *mut c_void = std::ptr::null_mut();
        let mut img: *mut c_void = std::ptr::null_mut();
        let r = (|| {
            check(heif_context_read_from_memory_without_copy(ctx, bytes.as_ptr().cast(), bytes.len(), std::ptr::null()))?;
            check(heif_context_get_primary_image_handle(ctx, &mut handle))?;
            check(heif_decode_image(handle, &mut img, COLORSPACE_RGB, CHROMA_INTERLEAVED_RGB, std::ptr::null()))?;
            let w = heif_image_get_width(img, CHANNEL_INTERLEAVED) as usize;
            let h = heif_image_get_height(img, CHANNEL_INTERLEAVED) as usize;
            let mut stride = 0usize;
            let plane = heif_image_get_plane_readonly2(img, CHANNEL_INTERLEAVED, &mut stride);
            if plane.is_null() || stride < w * 3 {
                return Err(HeicError { code: -1, subcode: 0, message: "no RGB plane".into() });
            }
            let mut pixels = Vec::with_capacity(w * h * 3);
            for y in 0..h {
                pixels.extend_from_slice(std::slice::from_raw_parts(plane.add(y * stride), w * 3));
            }
            Ok(Rgb8 { width: w, height: h, pixels })
        })();
        if !img.is_null() {
            heif_image_release(img);
        }
        if !handle.is_null() {
            heif_image_handle_release(handle);
        }
        heif_context_free(ctx);
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_and_link_time_versions_agree() {
        assert_eq!(runtime_version(), LINKED_VERSION);
    }

    #[test]
    fn garbage_is_an_error_not_a_crash() {
        let e = decode_rgb8(b"definitely not a HEIC file", true).unwrap_err();
        assert_ne!(e.code, 0);
        assert!(!e.message.is_empty());
    }

    #[test]
    fn truncated_input_is_an_error_not_a_crash() {
        assert!(decode_rgb8(&[0, 0, 0, 24, b'f', b't', b'y', b'p', b'h', b'e', b'i', b'c'], true).is_err());
    }
}
