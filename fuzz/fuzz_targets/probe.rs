// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| auto_crop_fuzz::probe(data));
