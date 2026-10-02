// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Memory of the local thresholds (ROADMAP M1.27): O(width) scratch per worker, not the ~192 MB of
//! two whole-image u64 integral planes at 12 MP. A counting allocator measures the extra peak
//! above the heap in use when the call starts; this binary runs one test at a time.

use auto_crop_imgproc::threshold::{nick, sauvola};
use peak_alloc::PeakAlloc;
use std::sync::Mutex;

#[global_allocator]
static ALLOC: PeakAlloc = PeakAlloc;

static SERIAL: Mutex<()> = Mutex::new(());

fn extra_peak(f: impl FnOnce() -> Vec<u8>) -> (usize, usize) {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let baseline = ALLOC.current_usage();
    ALLOC.reset_peak_usage();
    let out = f();
    (ALLOC.peak_usage().saturating_sub(baseline), out.len())
}

#[test]
fn twelve_megapixel_thresholds_need_the_output_plus_a_few_columns_of_scratch() {
    let (w, h) = (4000u32, 3000u32);
    let gray: Vec<u8> = (0..w as usize * h as usize)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect();
    let integral_planes = (w as usize * h as usize) * 16;
    for window in [31u32, 101] {
        let (s_extra, out) = extra_peak(|| sauvola(&gray, w, h, window, 0.25, 128.0));
        let (n_extra, _) = extra_peak(|| nick(&gray, w, h, window, -0.1));
        println!(
            "window {window}: sauvola extra {s_extra} B, nick extra {n_extra} B, output {out} B, \
             two u64 integral planes would be {integral_planes} B"
        );
        // Output plus at most 2 MB of scratch (4 u64 columns x 4000 x worker threads x slack).
        for extra in [s_extra, n_extra] {
            assert!(extra <= out + 2 * 1024 * 1024, "{extra} B");
            assert!(extra * 8 < integral_planes);
        }
    }
}
