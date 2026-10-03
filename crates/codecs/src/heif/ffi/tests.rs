// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The binding checked against the pinned headers and the loaded library.

use super::*;
use crate::native_header::{enum_values, struct_fields};
use std::mem::{offset_of, size_of};
use std::path::PathBuf;

fn header(name: &str) -> String {
    let p = PathBuf::from(env!("AUTOCROP_HEIF_INCLUDE")).join(name);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

#[test]
fn constants_match_the_pinned_headers() {
    for (file, en, name, value) in CONSTANTS {
        let found = enum_values(&header(file), en)
            .into_iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("{name} not found in enum {en} of {file}"));
        assert_eq!(found.1, i64::from(*value), "{name}");
    }
}

#[test]
fn struct_fields_match_the_pinned_headers() {
    assert_eq!(
        struct_fields(&header("heif_security.h"), "heif_security_limits"),
        LIMITS_FIELDS
    );
    assert_eq!(
        struct_fields(&header("heif_decoding.h"), "heif_decoding_options"),
        OPTIONS_FIELDS
    );
}

/// Offsets worked out from the C declarations for LP64 and LLP64 alike (every pointer and `u64`
/// is 8 bytes and 8-aligned on the 64-bit targets we ship); cross-checked once with `offsetof` in
/// a C compiler against the 1.23.5 headers.
#[cfg(target_pointer_width = "64")]
#[test]
fn the_struct_layouts_match_the_c_declarations() {
    assert_eq!(offset_of!(RawLimits, max_image_size_pixels), 8);
    assert_eq!(offset_of!(RawLimits, max_number_of_tiles), 16);
    assert_eq!(offset_of!(RawLimits, max_items), 28);
    assert_eq!(offset_of!(RawLimits, max_color_profile_size), 32);
    assert_eq!(offset_of!(RawLimits, max_memory_block_size), 40);
    assert_eq!(offset_of!(RawLimits, max_total_memory), 64);
    assert_eq!(offset_of!(RawLimits, max_iso23001_17_pixel_size_bytes), 92);
    assert_eq!(offset_of!(RawLimits, parent), 96);
    assert_eq!(size_of::<RawLimits>(), 104);

    assert_eq!(offset_of!(RawDecodingOptions, start_progress), 8);
    assert_eq!(offset_of!(RawDecodingOptions, progress_user_data), 32);
    assert_eq!(offset_of!(RawDecodingOptions, convert_hdr_to_8bit), 40);
    assert_eq!(offset_of!(RawDecodingOptions, decoder_id), 48);
    assert_eq!(offset_of!(RawDecodingOptions, color_conversion_options), 56);
    assert_eq!(size_of::<RawColorConversion>(), 16);
    assert_eq!(offset_of!(RawDecodingOptions, cancel_decoding), 72);
    assert_eq!(offset_of!(RawDecodingOptions, ignore_sequence_editlist), 88);
    assert_eq!(
        offset_of!(RawDecodingOptions, output_image_nclx_profile),
        96
    );
    assert_eq!(offset_of!(RawDecodingOptions, num_codec_threads), 108);
    assert_eq!(
        offset_of!(RawDecodingOptions, autocorrect_broken_input),
        112
    );
    assert_eq!(
        offset_of!(RawDecodingOptions, output_image_nclx_profile_passthrough),
        113
    );
    assert_eq!(size_of::<RawDecodingOptions>(), 120);
}

#[test]
fn libheif_allocates_the_defaults_the_binding_expects() {
    // (version, convert_hdr_to_8bit, colour-conversion version, downsampling = average,
    // upsampling = bilinear, autocorrect, passthrough): reading these through our struct proves
    // the offsets of the fields before `autocorrect_broken_input`.
    assert_eq!(Options::raw_defaults(), (10, 0, 1, 2, 2, 0, 0));
}

#[test]
fn the_loaded_library_is_the_linked_one_and_not_below_the_floor() {
    assert_eq!(runtime_version(), crate::heif::LINKED_VERSION);
    let v = crate::native_header::parse_version(&runtime_version());
    assert!(v >= crate::native_header::HEIF_VERSION_FLOOR, "{v:?}");
}

#[test]
fn security_limits_only_tighten_and_never_become_zero() {
    assert_eq!(tighten(0, 5), 5);
    assert_eq!(tighten(100, 5), 5);
    assert_eq!(tighten(3, 5), 3);
    // Our own zero would read as "unlimited" to libheif: it is clamped to 1.
    assert_eq!(tighten(100, 0), 1);
    assert_eq!(tighten32(0, 0), 1);
    assert_eq!(tighten32(10, 7), 7);
}

#[test]
fn garbage_and_empty_input_are_errors_not_crashes() {
    init(None).unwrap();
    let lim = Limits {
        max_pixels: 1 << 20,
        max_tiles: 100,
        max_icc_bytes: 1 << 20,
        max_block_bytes: 1 << 28,
        max_total_bytes: 1 << 28,
    };
    for bytes in [
        &b""[..],
        b"definitely not a HEIF file",
        &[0, 0, 0, 24, b'f', b't', b'y', b'p', b'h', b'e', b'i', b'c'],
    ] {
        let e = Context::read(bytes, &lim).err().expect("error");
        assert_ne!(e.code, 0);
        assert!(!e.message.is_empty());
    }
}
