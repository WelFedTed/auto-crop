// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The only `unsafe` code of the libjpeg-turbo path (ROADMAP M1.18, ADR-0004, ADR-0008): a thin
//! binding of the dozen TurboJPEG 3 functions the codecs need, written by hand against the pinned
//! `turbojpeg.h` (the unit tests read that header and check every constant below against it).
//!
//! Every function of the safe wrapper [`Handle`] establishes, before the foreign call, the
//! invariants the C side relies on: slice lengths cover every byte the library may write, the
//! destination of a lossless transform is a caller-owned buffer that the library may not
//! reallocate (`TJPARAM_NOREALLOC`), and library-allocated memory is freed exactly once with
//! `tj3Free`. Nothing outside this directory touches a raw pointer.

#![allow(unsafe_code)]

use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr::NonNull;

// Constants: values of the pinned `turbojpeg.h` (checked by `constants_match_the_pinned_header`).
pub const TJINIT_COMPRESS: c_int = 0;
pub const TJINIT_DECOMPRESS: c_int = 1;
pub const TJINIT_TRANSFORM: c_int = 2;
pub const TJSAMP_444: c_int = 0;
pub const TJSAMP_422: c_int = 1;
pub const TJSAMP_420: c_int = 2;
pub const TJSAMP_GRAY: c_int = 3;
pub const TJSAMP_440: c_int = 4;
pub const TJSAMP_411: c_int = 5;
pub const TJSAMP_441: c_int = 6;
pub const TJPF_RGB: c_int = 0;
pub const TJPF_GRAY: c_int = 6;
pub const TJPARAM_STOPONWARNING: c_int = 0;
pub const TJPARAM_NOREALLOC: c_int = 2;
pub const TJPARAM_QUALITY: c_int = 3;
pub const TJPARAM_SUBSAMP: c_int = 4;
pub const TJPARAM_JPEGWIDTH: c_int = 5;
pub const TJPARAM_JPEGHEIGHT: c_int = 6;
pub const TJPARAM_PRECISION: c_int = 7;
pub const TJPARAM_COLORSPACE: c_int = 8;
pub const TJPARAM_OPTIMIZE: c_int = 11;
pub const TJPARAM_SCANLIMIT: c_int = 13;
pub const TJPARAM_XDENSITY: c_int = 20;
pub const TJPARAM_YDENSITY: c_int = 21;
pub const TJPARAM_DENSITYUNITS: c_int = 22;
pub const TJXOP_NONE: c_int = 0;
pub const TJXOP_HFLIP: c_int = 1;
pub const TJXOP_VFLIP: c_int = 2;
pub const TJXOP_TRANSPOSE: c_int = 3;
pub const TJXOP_TRANSVERSE: c_int = 4;
pub const TJXOP_ROT90: c_int = 5;
pub const TJXOP_ROT180: c_int = 6;
pub const TJXOP_ROT270: c_int = 7;
pub const TJXOPT_PERFECT: c_int = 1;
pub const TJXOPT_TRIM: c_int = 2;
pub const TJXOPT_CROP: c_int = 4;

/// (kind, header name, value) for every constant above; the header test walks this table. Kind
/// `enum:TJPARAM` is a member of that C enum, `define` a `#define`.
#[cfg(test)]
pub const CONSTANTS: &[(&str, &str, c_int)] = &[
    ("enum:TJINIT", "TJINIT_COMPRESS", TJINIT_COMPRESS),
    ("enum:TJINIT", "TJINIT_DECOMPRESS", TJINIT_DECOMPRESS),
    ("enum:TJINIT", "TJINIT_TRANSFORM", TJINIT_TRANSFORM),
    ("enum:TJSAMP", "TJSAMP_444", TJSAMP_444),
    ("enum:TJSAMP", "TJSAMP_422", TJSAMP_422),
    ("enum:TJSAMP", "TJSAMP_420", TJSAMP_420),
    ("enum:TJSAMP", "TJSAMP_GRAY", TJSAMP_GRAY),
    ("enum:TJSAMP", "TJSAMP_440", TJSAMP_440),
    ("enum:TJSAMP", "TJSAMP_411", TJSAMP_411),
    ("enum:TJSAMP", "TJSAMP_441", TJSAMP_441),
    ("enum:TJPF", "TJPF_RGB", TJPF_RGB),
    ("enum:TJPF", "TJPF_GRAY", TJPF_GRAY),
    (
        "enum:TJPARAM",
        "TJPARAM_STOPONWARNING",
        TJPARAM_STOPONWARNING,
    ),
    ("enum:TJPARAM", "TJPARAM_NOREALLOC", TJPARAM_NOREALLOC),
    ("enum:TJPARAM", "TJPARAM_QUALITY", TJPARAM_QUALITY),
    ("enum:TJPARAM", "TJPARAM_SUBSAMP", TJPARAM_SUBSAMP),
    ("enum:TJPARAM", "TJPARAM_JPEGWIDTH", TJPARAM_JPEGWIDTH),
    ("enum:TJPARAM", "TJPARAM_JPEGHEIGHT", TJPARAM_JPEGHEIGHT),
    ("enum:TJPARAM", "TJPARAM_PRECISION", TJPARAM_PRECISION),
    ("enum:TJPARAM", "TJPARAM_COLORSPACE", TJPARAM_COLORSPACE),
    ("enum:TJPARAM", "TJPARAM_OPTIMIZE", TJPARAM_OPTIMIZE),
    ("enum:TJPARAM", "TJPARAM_SCANLIMIT", TJPARAM_SCANLIMIT),
    ("enum:TJPARAM", "TJPARAM_XDENSITY", TJPARAM_XDENSITY),
    ("enum:TJPARAM", "TJPARAM_YDENSITY", TJPARAM_YDENSITY),
    ("enum:TJPARAM", "TJPARAM_DENSITYUNITS", TJPARAM_DENSITYUNITS),
    ("enum:TJXOP", "TJXOP_NONE", TJXOP_NONE),
    ("enum:TJXOP", "TJXOP_HFLIP", TJXOP_HFLIP),
    ("enum:TJXOP", "TJXOP_VFLIP", TJXOP_VFLIP),
    ("enum:TJXOP", "TJXOP_TRANSPOSE", TJXOP_TRANSPOSE),
    ("enum:TJXOP", "TJXOP_TRANSVERSE", TJXOP_TRANSVERSE),
    ("enum:TJXOP", "TJXOP_ROT90", TJXOP_ROT90),
    ("enum:TJXOP", "TJXOP_ROT180", TJXOP_ROT180),
    ("enum:TJXOP", "TJXOP_ROT270", TJXOP_ROT270),
    ("define", "TJXOPT_PERFECT", TJXOPT_PERFECT),
    ("define", "TJXOPT_TRIM", TJXOPT_TRIM),
    ("define", "TJXOPT_CROP", TJXOPT_CROP),
];

type RawHandle = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Region {
    pub x: c_int,
    pub y: c_int,
    pub w: c_int,
    pub h: c_int,
}

#[repr(C)]
struct ScalingFactor {
    num: c_int,
    denom: c_int,
}

#[repr(C)]
struct RawTransform {
    r: Region,
    op: c_int,
    options: c_int,
    data: *mut c_void,
    custom_filter: *mut c_void,
}

// SAFETY: these declarations match the pinned `turbojpeg.h` (3.1.4 or newer; `build.rs` refuses an
// older library and the constants are checked against the header in the tests). Every call site
// below states why its arguments are valid.
unsafe extern "C" {
    #[cfg(tj3_init_version)]
    fn tj3InitVersion(init_type: c_int, api_version: c_int) -> RawHandle;
    #[cfg(not(tj3_init_version))]
    fn tj3Init(init_type: c_int) -> RawHandle;
    fn tj3Destroy(h: RawHandle);
    fn tj3GetErrorStr(h: RawHandle) -> *mut c_char;
    fn tj3Set(h: RawHandle, param: c_int, value: c_int) -> c_int;
    fn tj3Get(h: RawHandle, param: c_int) -> c_int;
    fn tj3Free(p: *mut c_void);
    fn tj3JPEGBufSize(w: c_int, h: c_int, subsamp: c_int) -> usize;
    fn tj3SetICCProfile(h: RawHandle, icc: *mut u8, size: usize) -> c_int;
    fn tj3Compress8(
        h: RawHandle,
        src: *const u8,
        w: c_int,
        pitch: c_int,
        hgt: c_int,
        pf: c_int,
        jpeg: *mut *mut u8,
        size: *mut usize,
    ) -> c_int;
    fn tj3DecompressHeader(h: RawHandle, jpeg: *const u8, size: usize) -> c_int;
    fn tj3SetScalingFactor(h: RawHandle, sf: ScalingFactor) -> c_int;
    fn tj3Decompress8(
        h: RawHandle,
        jpeg: *const u8,
        size: usize,
        dst: *mut u8,
        pitch: c_int,
        pf: c_int,
    ) -> c_int;
    fn tj3Transform(
        h: RawHandle,
        jpeg: *const u8,
        size: usize,
        n: c_int,
        dst: *mut *mut u8,
        sizes: *mut usize,
        t: *const RawTransform,
    ) -> c_int;
}

/// The version number `build.rs` verified against the pinned headers (>= 3001004).
pub const LINKED_VERSION: u32 = parse_version(env!("AUTOCROP_TURBOJPEG_VERSION"));

const fn parse_version(s: &str) -> u32 {
    let b = s.as_bytes();
    let (mut i, mut v) = (0, 0u32);
    while i < b.len() {
        v = v * 10 + (b[i] - b'0') as u32;
        i += 1;
    }
    v
}

/// Bytes per pixel of the pixel formats this crate uses.
pub fn pixel_size(pf: c_int) -> usize {
    if pf == TJPF_GRAY { 1 } else { 3 }
}

/// A transform request: operation, option flags and (for `TJXOPT_CROP`) the region.
#[derive(Clone, Copy, Debug)]
pub struct TransformSpec {
    pub op: c_int,
    pub options: c_int,
    pub region: Region,
}

/// Image facts read from a JPEG header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JpegInfo {
    pub width: usize,
    pub height: usize,
    pub subsamp: c_int,
    pub colorspace: c_int,
    pub precision: c_int,
}

/// An owned TurboJPEG instance (compress, decompress or transform). One thread at a time.
pub struct Handle(NonNull<c_void>);

// SAFETY: a TurboJPEG handle owns all its state and has no thread affinity; moving it to another
// thread is fine as long as it is used from one thread at a time, which `&mut self` on every
// operation enforces. It is deliberately not `Sync`.
unsafe impl Send for Handle {}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the pointer came from `tj3InitVersion` and is destroyed exactly once.
        unsafe { tj3Destroy(self.0.as_ptr()) }
    }
}

fn dim(v: usize) -> Result<c_int, String> {
    c_int::try_from(v).map_err(|_| "dimension does not fit in a C int".to_owned())
}

impl Handle {
    pub fn new(kind: c_int) -> Result<Self, String> {
        // SAFETY: plain call with integers; a null result is handled below.
        #[cfg(tj3_init_version)]
        let h = unsafe { tj3InitVersion(kind, LINKED_VERSION as c_int) };
        // SAFETY: as above (TurboJPEG 3.1.x, which has no version-checking initialiser).
        #[cfg(not(tj3_init_version))]
        let h = unsafe { tj3Init(kind) };
        NonNull::new(h)
            .map(Handle)
            .ok_or_else(|| "tj3Init failed (out of memory)".to_owned())
    }

    fn error(&self) -> String {
        // SAFETY: the handle is live; the returned string is owned by the handle and valid until
        // the next call, and is copied out here.
        unsafe {
            let p = tj3GetErrorStr(self.0.as_ptr());
            if p.is_null() {
                "unknown TurboJPEG error".to_owned()
            } else {
                CStr::from_ptr(p).to_string_lossy().into_owned()
            }
        }
    }

    fn check(&self, rc: c_int) -> Result<(), String> {
        if rc == 0 { Ok(()) } else { Err(self.error()) }
    }

    pub fn set(&mut self, param: c_int, value: c_int) -> Result<(), String> {
        // SAFETY: live handle, integer arguments.
        let rc = unsafe { tj3Set(self.0.as_ptr(), param, value) };
        self.check(rc)
    }

    pub fn get(&self, param: c_int) -> c_int {
        // SAFETY: live handle, integer argument.
        unsafe { tj3Get(self.0.as_ptr(), param) }
    }

    /// Parses the JPEG header (no pixels are produced).
    pub fn read_header(&mut self, jpeg: &[u8]) -> Result<JpegInfo, String> {
        // SAFETY: `jpeg` is a valid readable slice for `len` bytes; the library only reads it.
        let rc = unsafe { tj3DecompressHeader(self.0.as_ptr(), jpeg.as_ptr(), jpeg.len()) };
        self.check(rc)?;
        Ok(JpegInfo {
            width: usize::try_from(self.get(TJPARAM_JPEGWIDTH)).unwrap_or(0),
            height: usize::try_from(self.get(TJPARAM_JPEGHEIGHT)).unwrap_or(0),
            subsamp: self.get(TJPARAM_SUBSAMP),
            colorspace: self.get(TJPARAM_COLORSPACE),
            precision: self.get(TJPARAM_PRECISION),
        })
    }

    /// Decodes `jpeg` at 1/`denom` scale (1, 2, 4 or 8) into a fresh buffer of 8-bit samples in
    /// pixel format `pf`. `max_samples` bounds the allocation. Returns (pixels, width, height).
    pub fn decompress8(
        &mut self,
        jpeg: &[u8],
        denom: c_int,
        pf: c_int,
        max_samples: usize,
    ) -> Result<(Vec<u8>, usize, usize), String> {
        let info = self.read_header(jpeg)?;
        // SAFETY: live handle, integer arguments.
        let rc = unsafe { tj3SetScalingFactor(self.0.as_ptr(), ScalingFactor { num: 1, denom }) };
        self.check(rc)?;
        // The same formula as the library's TJSCALED: the output is exactly this size.
        let d = usize::try_from(denom).map_err(|_| "bad scaling factor".to_owned())?;
        if d == 0 {
            return Err("bad scaling factor".to_owned());
        }
        let (sw, sh) = (info.width.div_ceil(d), info.height.div_ceil(d));
        let samples = sw
            .checked_mul(sh)
            .and_then(|n| n.checked_mul(pixel_size(pf)))
            .filter(|&n| n > 0 && n <= max_samples)
            .ok_or_else(|| "decoded size exceeds the allowed buffer".to_owned())?;
        let mut out = vec![0u8; samples];
        let pitch = dim(sw * pixel_size(pf))?;
        // SAFETY: `jpeg` is readable for `len` bytes; `out` is writable for `sw * sh * pixel_size`
        // bytes, which is `pitch * height` for the scaled size the library will produce (checked
        // above from the same header the library re-reads).
        let rc = unsafe {
            tj3Decompress8(
                self.0.as_ptr(),
                jpeg.as_ptr(),
                jpeg.len(),
                out.as_mut_ptr(),
                pitch,
                pf,
            )
        };
        self.check(rc)?;
        Ok((out, sw, sh))
    }

    /// Compresses 8-bit `src` (`width * height * pixel_size(pf)` bytes) with the quality,
    /// subsampling and density already set on the handle; embeds `icc` when given.
    pub fn compress8(
        &mut self,
        src: &[u8],
        width: usize,
        height: usize,
        pf: c_int,
        icc: Option<&[u8]>,
    ) -> Result<Vec<u8>, String> {
        let need = width
            .checked_mul(height)
            .and_then(|n| n.checked_mul(pixel_size(pf)))
            .ok_or("image size overflows")?;
        if src.len() < need || width == 0 || height == 0 {
            return Err("pixel buffer is smaller than width x height".to_owned());
        }
        if let Some(icc) = icc.filter(|p| !p.is_empty()) {
            let rc = {
                // SAFETY: the profile slice outlives this function (and so the compress call
                // below); the library only reads it and copies it into the handle.
                unsafe { tj3SetICCProfile(self.0.as_ptr(), icc.as_ptr().cast_mut(), icc.len()) }
            };
            self.check(rc)?;
        }
        let mut buf: *mut u8 = std::ptr::null_mut();
        let mut size = 0usize;
        // SAFETY: `src` is readable for `pitch * height` bytes (checked above); with a null
        // `buf` the library allocates the output itself and returns it through `buf`/`size`.
        let rc = unsafe {
            tj3Compress8(
                self.0.as_ptr(),
                src.as_ptr(),
                dim(width)?,
                dim(width * pixel_size(pf))?,
                dim(height)?,
                pf,
                &mut buf,
                &mut size,
            )
        };
        let r = self.check(rc);
        let out = if r.is_ok() && !buf.is_null() {
            // SAFETY: on success `buf` points to `size` initialised bytes owned by the library.
            Some(unsafe { std::slice::from_raw_parts(buf, size) }.to_vec())
        } else {
            None
        };
        if !buf.is_null() {
            // SAFETY: allocated by the library (`tj3Alloc`), freed exactly once, not used after.
            unsafe { tj3Free(buf.cast()) };
        }
        r?;
        out.ok_or_else(|| "compress returned no data".to_owned())
    }

    /// Lossless transform of `jpeg` into the caller-owned `dst` (the library may not reallocate
    /// it: `TJPARAM_NOREALLOC`). Returns the number of bytes written.
    pub fn transform(
        &mut self,
        jpeg: &[u8],
        spec: &TransformSpec,
        dst: &mut [u8],
    ) -> Result<usize, String> {
        self.set(TJPARAM_NOREALLOC, 1)?;
        let raw = RawTransform {
            r: spec.region,
            op: spec.op,
            options: spec.options,
            data: std::ptr::null_mut(),
            custom_filter: std::ptr::null_mut(),
        };
        let first = dst.as_mut_ptr();
        let mut ptr = first;
        let mut size = dst.len();
        // SAFETY: `jpeg` is readable; `ptr`/`size` describe the caller-owned writable `dst`, and
        // NOREALLOC makes the library fail instead of reallocating or freeing it; `raw` outlives
        // the call and its pointer fields are null.
        let rc = unsafe {
            tj3Transform(
                self.0.as_ptr(),
                jpeg.as_ptr(),
                jpeg.len(),
                1,
                &mut ptr,
                &mut size,
                &raw,
            )
        };
        if ptr != first {
            // Defensive: the library handed back a buffer of its own; release it and refuse.
            // SAFETY: a pointer different from ours was allocated by the library.
            unsafe { tj3Free(ptr.cast()) };
            return Err("the library replaced the caller-owned buffer".to_owned());
        }
        self.check(rc)?;
        if size > dst.len() {
            return Err("the library reported more output than the buffer holds".to_owned());
        }
        Ok(size)
    }
}

/// Worst-case size of a compressed `w` x `h` image with this subsampling.
pub fn jpeg_buf_size(w: usize, h: usize, subsamp: c_int) -> Result<usize, String> {
    // SAFETY: pure function of its integer arguments.
    let n = unsafe { tj3JPEGBufSize(dim(w)?, dim(h)?, subsamp) };
    if n == 0 {
        Err("image dimensions are too large to compress".to_owned())
    } else {
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use crate::native_header::{define_value, enum_values};
    use std::path::PathBuf;

    #[test]
    fn constants_match_the_pinned_header() {
        let header = PathBuf::from(env!("AUTOCROP_TURBOJPEG_INCLUDE")).join("turbojpeg.h");
        let text = std::fs::read_to_string(&header).expect("turbojpeg.h");
        for (kind, name, ours) in super::CONSTANTS {
            let theirs = match kind.strip_prefix("enum:") {
                Some(e) => enum_values(&text, e)
                    .into_iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, v)| v),
                None => define_value(&text, name),
            };
            assert_eq!(
                theirs,
                Some(i64::from(*ours)),
                "{name} differs from {header:?}"
            );
        }
    }
}
