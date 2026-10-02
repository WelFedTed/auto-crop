// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Deterministic byte-mutation sweep over every generated fixture: the probe and the decoder must
//! answer each damaged file with a typed error or a result, and never with a caught panic
//! (`InternalPanic` is a defect to fix or to record, not an acceptable outcome; ROADMAP M1.69).

use super::probe::all_fixture_bytes;
use crate::{CodecError, DecodeLimits, decode_with, probe_with};

const VALUES: [u8; 4] = [0x00, 0xFF, 0x80, 0x55];

fn assert_no_panic(name: &str, what: &str, r: Result<(), CodecError>) {
    if let Err(CodecError::InternalPanic(m)) = r {
        panic!("{name}: {what} panicked: {m}");
    }
}

/// Small enough limits that a mutated size field cannot make a sweep slow or large.
fn sweep_limits() -> DecodeLimits {
    DecodeLimits::default().with_max_pixels(1 << 20)
}

#[test]
fn probe_survives_every_single_byte_mutation_and_every_truncation() {
    let l = sweep_limits();
    for (name, bytes) in all_fixture_bytes() {
        for cut in 0..bytes.len() {
            assert_no_panic(
                name,
                "truncated probe",
                probe_with(&bytes[..cut], &l).map(drop),
            );
        }
        let mut b = bytes.clone();
        for i in 0..bytes.len().min(4096) {
            let keep = b[i];
            for v in VALUES {
                b[i] = v;
                assert_no_panic(name, "mutated probe", probe_with(&b, &l).map(drop));
            }
            b[i] = keep;
        }
    }
}

#[test]
fn decode_survives_header_region_mutations_and_truncations() {
    let l = sweep_limits();
    for (name, bytes) in all_fixture_bytes() {
        // Fixtures are tiny; mutate every byte of the first 160 (headers, IFDs, chunk tables) and a
        // sparse sample of the rest, with a few values each.
        let mut b = bytes.clone();
        for i in (0..bytes.len()).filter(|i| *i < 160 || i % 11 == 0) {
            let keep = b[i];
            for v in [0x00u8, 0xFF, keep ^ 0x5A] {
                b[i] = v;
                assert_no_panic(name, "mutated decode", decode_with(&b, &l).map(drop));
            }
            b[i] = keep;
        }
        for cut in (0..bytes.len()).step_by(7) {
            assert_no_panic(
                name,
                "truncated decode",
                decode_with(&bytes[..cut], &l).map(drop),
            );
        }
    }
}
