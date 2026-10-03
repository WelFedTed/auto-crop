// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The only `unsafe` code of the libheif path (ADR-0005, ADR-0009): a thin binding of the two
//! dozen libheif C functions the decoder needs, written by hand against the pinned headers (the
//! unit tests read `libheif/heif_*.h` from the build prefix and check every constant and struct
//! field order below against them). The ported spike is `spikes/heic`.
//!
//! The safe types in this module ([`Context`], [`Handle`], [`Image`], [`Options`]) establish,
//! before every foreign call, the invariants libheif relies on: the input bytes outlive the
//! context (`read_from_memory_without_copy` keeps a pointer to them), every pointer comes from the
//! matching `heif_*` allocator and is released exactly once, security limits are set (and never
//! zero, which libheif reads as "unlimited") before any parsing, and pixel rows are read with the
//! plane's own width, height and stride. Nothing outside this directory touches a raw pointer.

#![allow(unsafe_code)]

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::marker::PhantomData;
use std::ptr::NonNull;
use std::time::Instant;

// Constants: values of the pinned headers (checked by `constants_match_the_pinned_headers`).
pub const ERR_OK: c_int = 0;
pub const ERR_UNSUPPORTED_FEATURE: c_int = 4;
pub const ERR_MEMORY: c_int = 6;
pub const ERR_DECODER_PLUGIN: c_int = 7;
pub const ERR_PLUGIN_LOADING: c_int = 11;
pub const ERR_CANCELED: c_int = 12;
#[cfg(test)]
pub const SUBERR_SECURITY_LIMIT: c_int = 1000;
pub const SUBERR_UNSUPPORTED_CODEC: c_int = 3000;
pub const SUBERR_NO_MATCHING_DECODER: c_int = 6003;
pub const COLORSPACE_RGB: c_int = 1;
pub const CHROMA_INTERLEAVED_RGB: c_int = 10;
pub const CHROMA_INTERLEAVED_RGBA: c_int = 11;
pub const CHANNEL_INTERLEAVED: c_int = 10;
pub const COMPRESSION_HEVC: c_int = 1;
pub const COMPRESSION_AV1: c_int = 4;

/// (kind, header file, enum name, member, value) for every constant above; the header test walks
/// this table.
#[cfg(test)]
pub const CONSTANTS: &[(&str, &str, &str, c_int)] = &[
    ("heif_error.h", "heif_error_code", "heif_error_Ok", ERR_OK),
    (
        "heif_error.h",
        "heif_error_code",
        "heif_error_Unsupported_feature",
        ERR_UNSUPPORTED_FEATURE,
    ),
    (
        "heif_error.h",
        "heif_error_code",
        "heif_error_Memory_allocation_error",
        ERR_MEMORY,
    ),
    (
        "heif_error.h",
        "heif_error_code",
        "heif_error_Decoder_plugin_error",
        ERR_DECODER_PLUGIN,
    ),
    (
        "heif_error.h",
        "heif_error_code",
        "heif_error_Plugin_loading_error",
        ERR_PLUGIN_LOADING,
    ),
    (
        "heif_error.h",
        "heif_error_code",
        "heif_error_Canceled",
        ERR_CANCELED,
    ),
    (
        "heif_error.h",
        "heif_suberror_code",
        "heif_suberror_Security_limit_exceeded",
        SUBERR_SECURITY_LIMIT,
    ),
    (
        "heif_error.h",
        "heif_suberror_code",
        "heif_suberror_Unsupported_codec",
        SUBERR_UNSUPPORTED_CODEC,
    ),
    (
        "heif_error.h",
        "heif_suberror_code",
        "heif_suberror_No_matching_decoder_installed",
        SUBERR_NO_MATCHING_DECODER,
    ),
    (
        "heif_image.h",
        "heif_colorspace",
        "heif_colorspace_RGB",
        COLORSPACE_RGB,
    ),
    (
        "heif_image.h",
        "heif_chroma",
        "heif_chroma_interleaved_RGB",
        CHROMA_INTERLEAVED_RGB,
    ),
    (
        "heif_image.h",
        "heif_chroma",
        "heif_chroma_interleaved_RGBA",
        CHROMA_INTERLEAVED_RGBA,
    ),
    (
        "heif_image.h",
        "heif_channel",
        "heif_channel_interleaved",
        CHANNEL_INTERLEAVED,
    ),
    (
        "heif_context.h",
        "heif_compression_format",
        "heif_compression_HEVC",
        COMPRESSION_HEVC,
    ),
    (
        "heif_context.h",
        "heif_compression_format",
        "heif_compression_AV1",
        COMPRESSION_AV1,
    ),
];

#[repr(C)]
#[derive(Clone, Copy)]
struct RawError {
    code: c_int,
    subcode: c_int,
    message: *const c_char,
}

#[repr(C)]
struct RawInitParams {
    version: c_int,
}

/// `heif_security_limits`, version 4 (libheif 1.22 and later; the build refuses older headers).
/// Field order is checked against `heif_security.h` by the tests.
#[repr(C)]
#[allow(dead_code)] // mirrors the C struct; only some fields are written
pub struct RawLimits {
    pub version: u8,
    pub max_image_size_pixels: u64,
    pub max_number_of_tiles: u64,
    pub max_bayer_pattern_pixels: u32,
    pub max_items: u32,
    pub max_color_profile_size: u32,
    pub max_memory_block_size: u64,
    pub max_components: u32,
    pub max_iloc_extents_per_item: u32,
    pub max_size_entity_group: u32,
    pub max_children_per_box: u32,
    pub max_total_memory: u64,
    pub max_sample_description_box_entries: u32,
    pub max_sample_group_description_box_entries: u32,
    pub max_sequence_frames: u32,
    pub max_number_of_file_brands: u32,
    pub max_bad_pixels: u32,
    pub max_iso23001_17_pixel_size_bytes: u32,
    pub parent: *const RawLimits,
}

/// Field names of [`RawLimits`] in header order (the test compares them with `heif_security.h`).
#[cfg(test)]
pub const LIMITS_FIELDS: &[&str] = &[
    "version",
    "max_image_size_pixels",
    "max_number_of_tiles",
    "max_bayer_pattern_pixels",
    "max_items",
    "max_color_profile_size",
    "max_memory_block_size",
    "max_components",
    "max_iloc_extents_per_item",
    "max_size_entity_group",
    "max_children_per_box",
    "max_total_memory",
    "max_sample_description_box_entries",
    "max_sample_group_description_box_entries",
    "max_sequence_frames",
    "max_number_of_file_brands",
    "max_bad_pixels",
    "max_iso23001_17_pixel_size_bytes",
    "parent",
];

#[repr(C)]
#[allow(dead_code)]
struct RawColorConversion {
    version: u8,
    downsampling: c_int,
    upsampling: c_int,
    only_use_preferred: u8,
}

type VoidFn = Option<extern "C" fn()>;

/// `heif_decoding_options`, version 10. Only fields up to the ones set below are written; the
/// struct is allocated by libheif (`heif_decoding_options_alloc`), never by us.
#[repr(C)]
#[allow(dead_code)]
struct RawDecodingOptions {
    version: u8,
    ignore_transformations: u8,
    start_progress: VoidFn,
    on_progress: VoidFn,
    end_progress: VoidFn,
    progress_user_data: *mut c_void,
    convert_hdr_to_8bit: u8,
    strict_decoding: u8,
    decoder_id: *const c_char,
    color_conversion_options: RawColorConversion,
    cancel_decoding: Option<extern "C" fn(*mut c_void) -> c_int>,
    color_conversion_options_ext: *mut c_void,
    ignore_sequence_editlist: c_int,
    output_image_nclx_profile: *mut c_void,
    num_library_threads: c_int,
    num_codec_threads: c_int,
    autocorrect_broken_input: u8,
    output_image_nclx_profile_passthrough: u8,
}

/// Field names of [`RawDecodingOptions`] in header order (the tests compare them with
/// `heif_decoding.h`).
#[cfg(test)]
pub const OPTIONS_FIELDS: &[&str] = &[
    "version",
    "ignore_transformations",
    "start_progress",
    "on_progress",
    "end_progress",
    "progress_user_data",
    "convert_hdr_to_8bit",
    "strict_decoding",
    "decoder_id",
    "color_conversion_options",
    "cancel_decoding",
    "color_conversion_options_ext",
    "ignore_sequence_editlist",
    "output_image_nclx_profile",
    "num_library_threads",
    "num_codec_threads",
    "autocorrect_broken_input",
    "output_image_nclx_profile_passthrough",
];

// SAFETY: these declarations match the pinned libheif headers (1.23.5 or newer; `build.rs` refuses
// an older library and the tests compare constants and struct fields with the headers). Every call
// site below states why its arguments are valid.
unsafe extern "C" {
    fn heif_get_version() -> *const c_char;
    fn heif_init(params: *mut RawInitParams) -> RawError;
    fn heif_load_plugins(
        directory: *const c_char,
        out_plugins: *mut *const c_void,
        out_n: *mut c_int,
        out_size: c_int,
    ) -> RawError;
    fn heif_have_decoder_for_format(format: c_int) -> c_int;

    fn heif_context_alloc() -> *mut c_void;
    fn heif_context_free(ctx: *mut c_void);
    fn heif_context_get_security_limits(ctx: *const c_void) -> *mut RawLimits;
    fn heif_context_read_from_memory_without_copy(
        ctx: *mut c_void,
        mem: *const c_void,
        size: usize,
        opts: *const c_void,
    ) -> RawError;
    fn heif_context_get_number_of_top_level_images(ctx: *mut c_void) -> c_int;
    fn heif_context_has_sequence(ctx: *const c_void) -> c_int;
    fn heif_context_get_primary_image_handle(ctx: *mut c_void, out: *mut *mut c_void) -> RawError;

    fn heif_image_handle_release(h: *const c_void);
    fn heif_image_handle_get_width(h: *const c_void) -> c_int;
    fn heif_image_handle_get_height(h: *const c_void) -> c_int;
    fn heif_image_handle_get_ispe_width(h: *const c_void) -> c_int;
    fn heif_image_handle_get_ispe_height(h: *const c_void) -> c_int;
    fn heif_image_handle_has_alpha_channel(h: *const c_void) -> c_int;
    fn heif_image_handle_get_luma_bits_per_pixel(h: *const c_void) -> c_int;

    fn heif_decoding_options_alloc() -> *mut RawDecodingOptions;
    fn heif_decoding_options_free(o: *mut RawDecodingOptions);
    fn heif_decode_image(
        h: *const c_void,
        out: *mut *mut c_void,
        colorspace: c_int,
        chroma: c_int,
        opts: *const RawDecodingOptions,
    ) -> RawError;

    fn heif_image_release(img: *const c_void);
    fn heif_image_get_width(img: *const c_void, channel: c_int) -> c_int;
    fn heif_image_get_height(img: *const c_void, channel: c_int) -> c_int;
    fn heif_image_get_plane_readonly2(
        img: *const c_void,
        channel: c_int,
        stride: *mut usize,
    ) -> *const u8;
}

/// A libheif error: category, detail code and the message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeifError {
    pub code: i32,
    pub subcode: i32,
    pub message: String,
}

impl std::fmt::Display for HeifError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "libheif error {}/{}: {}",
            self.code, self.subcode, self.message
        )
    }
}

impl HeifError {
    fn local(message: impl Into<String>) -> Self {
        Self {
            code: -1,
            subcode: 0,
            message: message.into(),
        }
    }
}

fn check(e: RawError) -> Result<(), HeifError> {
    if e.code == ERR_OK {
        return Ok(());
    }
    let message = if e.message.is_null() {
        String::new()
    } else {
        // SAFETY: libheif documents `message` as always defined (a NUL-terminated string that
        // stays valid for the library's lifetime or until the next call on the same context); it
        // is copied immediately.
        unsafe { CStr::from_ptr(e.message) }
            .to_string_lossy()
            .into_owned()
    };
    Err(HeifError {
        code: e.code,
        subcode: e.subcode,
        message,
    })
}

/// The libheif version string of the loaded library (for example `1.23.5`).
pub fn runtime_version() -> String {
    // SAFETY: `heif_get_version` takes no arguments and returns a static NUL-terminated string.
    unsafe { CStr::from_ptr(heif_get_version()) }
        .to_string_lossy()
        .into_owned()
}

/// Initialises libheif once (`heif_init` loads the plugins of the default directories and of
/// `LIBHEIF_PLUGIN_PATH`) and, with `plugin_dir`, loads the plugins found there too. Never
/// deinitialised: the library lives as long as the process (libheif forbids `heif_deinit` during
/// process exit).
pub fn init(plugin_dir: Option<&std::path::Path>) -> Result<(), HeifError> {
    let mut params = RawInitParams { version: 1 };
    // SAFETY: `params` is a valid, initialised `heif_init_params` that lives across the call.
    check(unsafe { heif_init(&mut params) })?;
    if let Some(dir) = plugin_dir {
        let c = CString::new(dir.to_string_lossy().as_bytes())
            .map_err(|_| HeifError::local("the plugin directory contains a NUL byte"))?;
        let mut out = [std::ptr::null::<c_void>(); 16];
        let mut n: c_int = 0;
        // SAFETY: `c` is a NUL-terminated string, `out` has room for the 16 entries announced in
        // the last argument and `n` is a valid out pointer; all outlive the call.
        check(unsafe { heif_load_plugins(c.as_ptr(), out.as_mut_ptr(), &mut n, 16) })?;
    }
    Ok(())
}

/// True when a decoder plugin for `format` (`COMPRESSION_*`) is installed.
pub fn have_decoder(format: c_int) -> bool {
    // SAFETY: plain value argument; the function only reads the plugin registry.
    unsafe { heif_have_decoder_for_format(format) != 0 }
}

/// The caps handed to libheif before it parses anything. A zero would mean "unlimited" to libheif,
/// so every field is clamped to at least 1 and only ever tightens the library's own default.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_pixels: u64,
    pub max_tiles: u64,
    pub max_icc_bytes: u32,
    pub max_block_bytes: u64,
    pub max_total_bytes: u64,
}

/// A libheif context reading `bytes` in place (no copy), so the bytes must outlive it.
pub struct Context<'a> {
    ptr: NonNull<c_void>,
    _bytes: PhantomData<&'a [u8]>,
}

impl<'a> Context<'a> {
    /// Parses the container boxes of `bytes` under `limits`.
    pub fn read(bytes: &'a [u8], limits: &Limits) -> Result<Self, HeifError> {
        // SAFETY: allocates a fresh context; null is checked below.
        let raw = unsafe { heif_context_alloc() };
        let ptr = NonNull::new(raw).ok_or_else(|| HeifError::local("out of memory"))?;
        // From here the `Drop` of `ctx` frees the context on every path.
        let ctx = Context {
            ptr,
            _bytes: PhantomData,
        };
        // SAFETY: `ctx.ptr` is a live context; the returned pointer addresses the context's own
        // limits struct, valid until the context is freed, and `RawLimits` mirrors the version 4
        // layout that every libheif at or above the pinned 1.23.5 uses.
        let l = unsafe { heif_context_get_security_limits(ctx.ptr.as_ptr()) };
        if l.is_null() {
            return Err(HeifError::local("libheif returned no security limits"));
        }
        // SAFETY: `l` is non-null, aligned and exclusively ours until the next call on `ctx`.
        let l = unsafe { &mut *l };
        l.max_image_size_pixels = tighten(l.max_image_size_pixels, limits.max_pixels);
        l.max_number_of_tiles = tighten(l.max_number_of_tiles, limits.max_tiles);
        l.max_color_profile_size = tighten32(l.max_color_profile_size, limits.max_icc_bytes);
        l.max_memory_block_size = tighten(l.max_memory_block_size, limits.max_block_bytes);
        l.max_total_memory = tighten(l.max_total_memory, limits.max_total_bytes);
        // SAFETY: `bytes` outlives `ctx` (the `'a` bound of the type), the length is the slice's,
        // and a null options pointer is documented as "defaults".
        check(unsafe {
            heif_context_read_from_memory_without_copy(
                ctx.ptr.as_ptr(),
                bytes.as_ptr().cast(),
                bytes.len(),
                std::ptr::null(),
            )
        })?;
        Ok(ctx)
    }

    /// Top-level images of the file (thumbnails and grid tiles excluded).
    pub fn top_level_images(&self) -> u32 {
        // SAFETY: `self.ptr` is a live context.
        let n = unsafe { heif_context_get_number_of_top_level_images(self.ptr.as_ptr()) };
        u32::try_from(n).unwrap_or(0)
    }

    /// True when the file has an image sequence (a `moov` track).
    pub fn has_sequence(&self) -> bool {
        // SAFETY: `self.ptr` is a live context.
        unsafe { heif_context_has_sequence(self.ptr.as_ptr()) != 0 }
    }

    /// The primary image.
    pub fn primary(&self) -> Result<Handle<'_>, HeifError> {
        let mut out: *mut c_void = std::ptr::null_mut();
        // SAFETY: `self.ptr` is a live context and `out` is a valid out pointer.
        check(unsafe { heif_context_get_primary_image_handle(self.ptr.as_ptr(), &mut out) })?;
        let ptr = NonNull::new(out)
            .ok_or_else(|| HeifError::local("libheif returned no image handle"))?;
        Ok(Handle {
            ptr,
            _ctx: PhantomData,
        })
    }
}

fn tighten(library: u64, ours: u64) -> u64 {
    // 0 means "unlimited" to libheif, in both the library's value and ours.
    match (library, ours.max(1)) {
        (0, o) => o,
        (l, o) => l.min(o),
    }
}

fn tighten32(library: u32, ours: u32) -> u32 {
    match (library, ours.max(1)) {
        (0, o) => o,
        (l, o) => l.min(o),
    }
}

impl Drop for Context<'_> {
    fn drop(&mut self) {
        // SAFETY: `ptr` came from `heif_context_alloc` and is freed exactly once, here. Handles
        // and images keep their own references, so freeing the context first is also fine.
        unsafe { heif_context_free(self.ptr.as_ptr()) }
    }
}

/// An image of a context (its primary image).
pub struct Handle<'c> {
    ptr: NonNull<c_void>,
    _ctx: PhantomData<&'c ()>,
}

impl Handle<'_> {
    /// The size after `clap`, `irot` and `imir`.
    pub fn size(&self) -> (u32, u32) {
        // SAFETY: `self.ptr` is a live image handle.
        let (w, h) = unsafe {
            (
                heif_image_handle_get_width(self.ptr.as_ptr()),
                heif_image_handle_get_height(self.ptr.as_ptr()),
            )
        };
        (u32::try_from(w).unwrap_or(0), u32::try_from(h).unwrap_or(0))
    }

    /// The size before the transformations (the `ispe` property).
    pub fn ispe_size(&self) -> (u32, u32) {
        // SAFETY: `self.ptr` is a live image handle.
        let (w, h) = unsafe {
            (
                heif_image_handle_get_ispe_width(self.ptr.as_ptr()),
                heif_image_handle_get_ispe_height(self.ptr.as_ptr()),
            )
        };
        (u32::try_from(w).unwrap_or(0), u32::try_from(h).unwrap_or(0))
    }

    pub fn has_alpha(&self) -> bool {
        // SAFETY: `self.ptr` is a live image handle.
        unsafe { heif_image_handle_has_alpha_channel(self.ptr.as_ptr()) != 0 }
    }

    /// Bits per luma sample as stored, if the file says.
    pub fn luma_bits(&self) -> Option<u8> {
        // SAFETY: `self.ptr` is a live image handle.
        let b = unsafe { heif_image_handle_get_luma_bits_per_pixel(self.ptr.as_ptr()) };
        u8::try_from(b).ok().filter(|b| *b > 0)
    }

    /// Decodes to 8-bit interleaved RGB (`rgba` false) or RGBA, applying `clap`, `irot` and
    /// `imir`. Higher bit depths are reduced to 8 by libheif (`convert_hdr_to_8bit`).
    pub fn decode(&self, rgba: bool, opts: &Options) -> Result<Image, HeifError> {
        let mut out: *mut c_void = std::ptr::null_mut();
        // SAFETY: `self.ptr` is a live handle; `out` is a valid out pointer; `opts.ptr` is a live
        // options struct (kept alive, with the cancel state it points to, by `opts`).
        check(unsafe {
            heif_decode_image(
                self.ptr.as_ptr(),
                &mut out,
                COLORSPACE_RGB,
                if rgba {
                    CHROMA_INTERLEAVED_RGBA
                } else {
                    CHROMA_INTERLEAVED_RGB
                },
                opts.ptr.as_ptr(),
            )
        })?;
        let ptr = NonNull::new(out).ok_or_else(|| HeifError::local("libheif returned no image"))?;
        Ok(Image { ptr })
    }
}

impl Drop for Handle<'_> {
    fn drop(&mut self) {
        // SAFETY: `ptr` came from `heif_context_get_primary_image_handle` and is released once.
        unsafe { heif_image_handle_release(self.ptr.as_ptr()) }
    }
}

/// How the cancel callback decides: a wall-clock deadline.
struct CancelState {
    deadline: Option<Instant>,
}

extern "C" fn cancel_cb(user: *mut c_void) -> c_int {
    // A panic must not unwind into C; treat one as "cancel".
    std::panic::catch_unwind(|| {
        // SAFETY: `user` is the `CancelState` box owned by the `Options` that registered this
        // callback, which outlives the decode call (it is only read here).
        let st = unsafe { &*user.cast::<CancelState>() };
        c_int::from(st.deadline.is_some_and(|d| Instant::now() >= d))
    })
    .unwrap_or(1)
}

/// Decoding options: 8-bit output, the input's own colour description kept, a deadline and a
/// codec thread count.
pub struct Options {
    ptr: NonNull<RawDecodingOptions>,
    /// Kept alive for the pointer stored in `progress_user_data`.
    _cancel: Box<CancelState>,
}

impl Options {
    /// `codec_threads` of 0 lets the decoder choose.
    pub fn new(deadline: Option<Instant>, codec_threads: u32) -> Result<Self, HeifError> {
        // SAFETY: allocates a default options struct owned by us until `Drop`; null is checked.
        let raw = unsafe { heif_decoding_options_alloc() };
        let ptr = NonNull::new(raw).ok_or_else(|| HeifError::local("out of memory"))?;
        let mut cancel = Box::new(CancelState { deadline });
        // SAFETY: `ptr` is a live, library-allocated `heif_decoding_options` of version 10 (every
        // libheif at or above the pinned 1.23.5), so every field written exists; `cancel` is
        // heap-allocated, so its address is stable for as long as `self` holds the box.
        unsafe {
            let o = &mut *ptr.as_ptr();
            o.convert_hdr_to_8bit = 1;
            o.output_image_nclx_profile_passthrough = 1;
            o.progress_user_data = (&mut *cancel as *mut CancelState).cast();
            o.cancel_decoding = Some(cancel_cb);
            o.num_codec_threads = c_int::try_from(codec_threads).unwrap_or(0);
        }
        Ok(Self {
            ptr,
            _cancel: cancel,
        })
    }

    /// The defaults libheif allocated, for the layout tests: (version, convert_hdr_to_8bit,
    /// color_conversion version, downsampling, upsampling, autocorrect, passthrough).
    #[cfg(test)]
    pub fn raw_defaults() -> (u8, u8, u8, c_int, c_int, u8, u8) {
        // SAFETY: allocates and frees one options struct; the fields are read before the free.
        unsafe {
            let raw = heif_decoding_options_alloc();
            let o = &*raw;
            let r = (
                o.version,
                o.convert_hdr_to_8bit,
                o.color_conversion_options.version,
                o.color_conversion_options.downsampling,
                o.color_conversion_options.upsampling,
                o.autocorrect_broken_input,
                o.output_image_nclx_profile_passthrough,
            );
            heif_decoding_options_free(raw);
            r
        }
    }
}

impl Drop for Options {
    fn drop(&mut self) {
        // SAFETY: `ptr` came from `heif_decoding_options_alloc` and is freed once. The decode call
        // that used it has returned, so libheif no longer reads the cancel state.
        unsafe { heif_decoding_options_free(self.ptr.as_ptr()) }
    }
}

/// A decoded image owned by libheif.
pub struct Image {
    ptr: NonNull<c_void>,
}

impl Image {
    /// Width and height of the interleaved plane.
    pub fn size(&self) -> (usize, usize) {
        // SAFETY: `self.ptr` is a live image.
        let (w, h) = unsafe {
            (
                heif_image_get_width(self.ptr.as_ptr(), CHANNEL_INTERLEAVED),
                heif_image_get_height(self.ptr.as_ptr(), CHANNEL_INTERLEAVED),
            )
        };
        (
            usize::try_from(w).unwrap_or(0),
            usize::try_from(h).unwrap_or(0),
        )
    }

    /// Calls `row(y, bytes)` for every row of the interleaved plane, `bytes_per_pixel * width`
    /// bytes each (the stride padding is skipped).
    pub fn rows(
        &self,
        bytes_per_pixel: usize,
        mut row: impl FnMut(usize, &[u8]),
    ) -> Result<(), HeifError> {
        let (w, h) = self.size();
        let row_len = w
            .checked_mul(bytes_per_pixel)
            .ok_or_else(|| HeifError::local("row size overflows"))?;
        let mut stride = 0usize;
        // SAFETY: `self.ptr` is a live image and `stride` is a valid out pointer. The returned
        // plane stays valid while the image lives (`&self`).
        let plane = unsafe {
            heif_image_get_plane_readonly2(self.ptr.as_ptr(), CHANNEL_INTERLEAVED, &mut stride)
        };
        if plane.is_null() || stride < row_len {
            return Err(HeifError::local(
                "the decoded image has no interleaved plane",
            ));
        }
        for y in 0..h {
            let off = y
                .checked_mul(stride)
                .ok_or_else(|| HeifError::local("plane offset overflows"))?;
            // SAFETY: libheif guarantees `stride * height` readable bytes for the plane of the
            // channel whose own width and height were used above, and `off + row_len <= stride *
            // (y + 1) <= stride * h`. The slice does not outlive this iteration.
            let bytes = unsafe { std::slice::from_raw_parts(plane.add(off), row_len) };
            row(y, bytes);
        }
        Ok(())
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        // SAFETY: `ptr` came from `heif_decode_image` and is released once.
        unsafe { heif_image_release(self.ptr.as_ptr()) }
    }
}

#[cfg(test)]
mod tests;
