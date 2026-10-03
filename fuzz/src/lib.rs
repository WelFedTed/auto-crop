// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Fuzz entry points (ROADMAP M1.70) shared by libFuzzer and by the plain-`cargo test` replay of
//! `fuzz/regressions/` (M1.71).
//!
//! Every target is a function `fn(&[u8])` in this file; `fuzz_targets/<name>.rs` only forwards the
//! libFuzzer input to it. A function returns normally when the input was handled correctly (accepted
//! or refused with a typed error) and **panics** when it found a bug: a panic anywhere below it (a
//! caught decoder panic comes back as `CodecError::InternalPanic` and is re-raised here), a violated
//! invariant, or a result that contradicts the limits it was given. libFuzzer turns the panic into a
//! crash file; the replay test turns it into a failing test.
//!
//! Targets (all reach their code only through the public API of the crate under test):
//!
//! * [`probe`]: sniff and header probe under the default limits, plus the EXIF reader on the raw bytes.
//! * [`limits`]: the first 8 bytes choose a `DecodeLimits`, the rest is a file; probe and a full decode
//!   under those limits, and the results must respect them.
//! * [`metadata`]: the input is an EXIF, ICC or XMP blob wrapped in a valid JPEG, PNG, WebP or TIFF, so
//!   the metadata paths (EXIF orientation, ICC chunk reassembly, `iCCP` inflation, size caps) are
//!   reached on every execution instead of waiting for a mutation to build a valid container.
//! * [`editstate_json`]: `EditState` deserialisation and the schema migration never panic, and what
//!   they accept survives a round trip.

#![forbid(unsafe_code)]

use auto_crop_codecs::{
    CodecError, DecodeLimits, Decoded, Probe, decode_with, exif_orientation, probe_with, sniff,
};
use auto_crop_core::EditState;

pub mod seeds;

/// A fuzz target body.
pub type Entry = fn(&[u8]);

/// Every target by name. `fuzz_targets/<name>.rs` and `regressions/<name>/` must match this table
/// (`xtask/tests/fuzz_regressions.rs` checks both).
pub const TARGETS: &[(&str, Entry)] = &[
    ("probe", probe),
    ("limits", limits),
    ("metadata", metadata),
    ("editstate_json", editstate_json),
];

/// Looks a target up by name.
pub fn entry(name: &str) -> Option<Entry> {
    TARGETS.iter().find(|(n, _)| *n == name).map(|(_, f)| *f)
}

/// The `-max_len` the fuzz jobs use: larger inputs are cut by libFuzzer, so seeds above it are not
/// written.
pub const MAX_INPUT: usize = 1 << 20;

/// Runs `f` on `bytes` the way the replay test does (no libFuzzer): `Err` carries the panic text.
pub fn replay(f: Entry, bytes: &[u8]) -> Result<(), String> {
    std::panic::catch_unwind(|| f(bytes)).map_err(|p| auto_crop_codecs::guard::panic_message(&*p))
}

/// A caught decoder panic is a bug, not an error to hand back to the user.
fn no_panic(r: &Result<impl Sized, CodecError>) {
    if let Err(CodecError::InternalPanic(msg)) = r {
        panic!("a codec panicked (caught by the guard): {msg}");
    }
}

fn check_probe(data: &[u8], p: &Probe) {
    assert_eq!(sniff(data), Some(p.format), "probe disagrees with sniff");
    assert!(p.width > 0 && p.height > 0, "zero dimension: {p:?}");
    assert!((1..=8).contains(&p.orientation), "orientation: {p:?}");
    assert!(p.frames >= 1, "no frames: {p:?}");
}

/// Sniff and header probe under the default limits.
pub fn probe(data: &[u8]) {
    let limits = DecodeLimits::default();
    let first = probe_with(data, &limits);
    no_panic(&first);
    // Probing is a pure function of the bytes.
    assert_eq!(
        first,
        probe_with(data, &limits),
        "probe is not deterministic"
    );
    if let Ok(p) = &first {
        check_probe(data, p);
    }
    let _ = sniff(data);
    if let Some(o) = exif_orientation(data) {
        assert!((1..=8).contains(&o), "exif orientation {o}");
    }
}

/// The `DecodeLimits` that the first 8 bytes of a `limits` input select. Pixel counts stay at or
/// below 262,144 so that a successful decode is cheap; every other cap ranges from "refuse
/// everything" to "generous".
pub fn limits_from(cfg: &[u8; 8]) -> DecodeLimits {
    let pixels = (u64::from(cfg[0]) + 1) * (u64::from(cfg[1]) + 1) * 4;
    let mut l = DecodeLimits::default().with_max_pixels(pixels);
    l.max_metadata_bytes = (1u64 << (cfg[2] % 22)) - 1;
    l.max_scans = u32::from(cfg[3] % 16);
    l.max_frames = u32::from(cfg[4]);
    l.max_file_bytes = 1u64 << (4 + cfg[5] % 21);
    l.max_file_bytes_streamed = l.max_file_bytes << (cfg[5] / 64);
    if cfg[6] & 0x80 == 0 {
        // Tighten the memory estimate below what the pixel cap alone would allow.
        l.max_est_bytes = (pixels * 9) >> (cfg[6] % 4);
    }
    l
}

/// An input for [`limits`]: a limits header (see [`limits_from`]) in front of `file`.
pub fn limits_input(cfg: [u8; 8], file: &[u8]) -> Vec<u8> {
    let mut v = cfg.to_vec();
    v.extend_from_slice(file);
    v
}

/// The header of the seeds: generous limits (262,144 pixels, 8 KiB metadata, 15 scans, 255 frames).
pub const GENEROUS: [u8; 8] = [255, 255, 13, 15, 255, 20, 0x80, 0];

fn check_decoded(file: &[u8], l: &DecodeLimits, d: &Decoded, probe: &Result<Probe, CodecError>) {
    let (w, h) = (u64::from(d.raster.width), u64::from(d.raster.height));
    assert!(w * h <= l.max_pixels, "decoded {w}x{h} over the cap");
    assert_eq!(d.raster.data.len() as u64, w * h * 3, "raster size");
    assert!(
        file.len() as u64 <= l.file_cap(d.format),
        "file over its cap"
    );
    assert!(d.frames <= l.max_frames.max(1), "frames over the cap");
    assert!((1..=8).contains(&d.exif_orientation));
    if let Some(icc) = &d.icc {
        assert!(icc.len() as u64 <= l.max_metadata_bytes, "ICC over the cap");
    }
    let p = probe
        .as_ref()
        .unwrap_or_else(|e| panic!("decode succeeded but probe failed: {e}"));
    assert_eq!(p.format, d.format);
    assert_eq!(p.orientation, d.exif_orientation);
    let (pw, ph) = (u64::from(p.width), u64::from(p.height));
    assert!(
        (pw, ph) == (w, h) || (pw, ph) == (h, w),
        "probe {pw}x{ph} but decoded {w}x{h}"
    );
}

/// Probe and decode under limits chosen by the input.
pub fn limits(data: &[u8]) {
    let Some((cfg, file)) = data.split_first_chunk::<8>() else {
        return;
    };
    let l = limits_from(cfg);
    let probe = probe_with(file, &l);
    no_panic(&probe);
    if let Ok(p) = &probe {
        check_probe(file, p);
        assert!(
            p.frames <= l.max_frames.max(1),
            "probe frames over the cap: {p:?}"
        );
        assert!(
            p.scans <= l.max_scans.max(1),
            "probe scans over the cap: {p:?}"
        );
    }
    let decoded = decode_with(file, &l);
    no_panic(&decoded);
    if let Ok(d) = &decoded {
        check_decoded(file, &l, d, &probe);
    }
}

/// How many carriers [`carrier`] knows (the first byte of a `metadata` input, modulo this).
pub const CARRIERS: u8 = 11;

/// Wraps `blob` in a small valid file of the kind `kind % CARRIERS` selects (None for kind 0, where
/// the blob is only given to the EXIF reader). Kinds: 1 JPEG APP1 Exif, 2 JPEG APP2 ICC chunk (the
/// blob starts with the sequence number and the total), 3 JPEG APP1 XMP, 4 PNG `eXIf`, 5 PNG
/// `iCCP` with a valid zlib stream around the blob, 6 PNG `iCCP` with the blob as the raw chunk,
/// 7 WebP `EXIF`, 8 WebP `ICCP`, 9 TIFF ICC tag (orientation from the first byte), 10 JPEG ICC in
/// two well-formed chunks.
pub fn carrier(kind: u8, blob: &[u8]) -> Option<Vec<u8>> {
    use auto_crop_codecs::fixtures as fx;
    // A JPEG segment holds at most 65,533 bytes of payload.
    let seg = &blob[..blob.len().min(60_000)];
    let jpeg = || fx::jpeg_baseline(8, 8);
    Some(match kind % CARRIERS {
        0 => return None,
        1 => fx::jpeg_with_exif_blob(&jpeg(), seg),
        2 => fx::jpeg_insert_segment(&jpeg(), 0xE2, &[b"ICC_PROFILE\0", seg].concat()),
        3 => fx::jpeg_insert_segment(
            &jpeg(),
            0xE1,
            &[b"http://ns.adobe.com/xap/1.0/\0".as_slice(), seg].concat(),
        ),
        4 => {
            let mut s = fx::PngSpec::rgb8(8, 8);
            s.before_idat.push((*b"eXIf", blob.to_vec()));
            s.build()
        }
        5 => {
            let mut s = fx::PngSpec::rgb8(8, 8);
            s.before_idat
                .push((*b"iCCP", [b"t\0\0".as_slice(), &fx::zlib(blob)].concat()));
            s.build()
        }
        6 => {
            let mut s = fx::PngSpec::rgb8(8, 8);
            s.before_idat.push((*b"iCCP", blob.to_vec()));
            s.build()
        }
        7 => fx::webp_extended(8, 8, Some(blob), None),
        8 => fx::webp_extended(8, 8, None, Some(blob)),
        9 => fx::tiff_rgb8(
            8,
            8,
            &fx::TiffOpts {
                orientation: blob.first().map(|b| u16::from(*b % 10)),
                icc: Some(blob.to_vec()),
                ..Default::default()
            },
        ),
        _ => {
            let (a, b) = seg.split_at(seg.len() / 2);
            let chunk =
                |seq: u8, part: &[u8]| [b"ICC_PROFILE\0".as_slice(), &[seq, 2], part].concat();
            // Segment 1 first, then 2: insertion goes right after SOI, so insert in reverse.
            let two = fx::jpeg_insert_segment(&jpeg(), 0xE2, &chunk(2, b));
            fx::jpeg_insert_segment(&two, 0xE2, &chunk(1, a))
        }
    })
}

/// An input for [`metadata`]: the carrier byte in front of the blob.
pub fn metadata_input(kind: u8, blob: &[u8]) -> Vec<u8> {
    let mut v = vec![kind];
    v.extend_from_slice(blob);
    v
}

/// EXIF, ICC and XMP blobs inside valid files.
pub fn metadata(data: &[u8]) {
    let Some((&kind, blob)) = data.split_first() else {
        return;
    };
    if let Some(o) = exif_orientation(blob) {
        assert!((1..=8).contains(&o), "exif orientation {o}");
    }
    let Some(file) = carrier(kind, blob) else {
        return;
    };
    // The carrier is 8x8 pixels, so decoding it is cheap whatever the metadata says; the two
    // metadata caps are exercised through a tight and a generous limit.
    for l in [
        DecodeLimits::default(),
        DecodeLimits {
            max_metadata_bytes: 64,
            ..DecodeLimits::default()
        },
    ] {
        let probe = probe_with(&file, &l);
        no_panic(&probe);
        if let Ok(p) = &probe {
            check_probe(&file, p);
        }
        let decoded = decode_with(&file, &l);
        no_panic(&decoded);
        if let Ok(d) = &decoded {
            check_decoded(&file, &l, d, &probe);
        }
    }
}

/// `EditState` JSON: parse, migrate, hash and round-trip, none of which may panic.
pub fn editstate_json(data: &[u8]) {
    // Direct deserialisation, as the manifest and the sidecar reader do for a current document.
    let direct = serde_json::from_slice::<EditState>(data);
    if let Ok(state) = &direct {
        exercise(state);
    }
    // The migration path, which also accepts the early-slice shape and refuses newer schemas.
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    match auto_crop_engine::migrate::migrate_str(text) {
        Ok(state) => {
            exercise(&state);
            // What migrate accepts, serde accepts when written back.
            let json = serde_json::to_string(&state).expect("an EditState always serialises");
            let again = auto_crop_engine::migrate::migrate_str(&json)
                .unwrap_or_else(|e| panic!("migrated state does not round-trip: {e:?}\n{json}"));
            assert_eq!(again, state, "round trip changed the state");
        }
        Err(auto_crop_core::ErrKind::Internal) => {
            panic!("migrate reported an internal error for {text:?}")
        }
        Err(_) => {}
    }
}

fn exercise(state: &EditState) {
    let _ = state.validate();
    let _ = state.render_hash();
    let _ = state.quad();
    let _ = state.included().count();
}
