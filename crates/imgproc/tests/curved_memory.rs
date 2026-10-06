// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Memory bound and speed of the curved-page resampler at 12 MP (docs/dev/curved-pages.md, ROADMAP
//! M12.30): the extra peak heap while flattening stays within the output image plus a small
//! per-band and per-edge scratch, i.e. there is no full-frame f32 copy of the source or the
//! output and no full-frame coordinate map. Like `warp_memory.rs`, this binary holds only these
//! tests so nothing else allocates concurrently. The timing test is `#[ignore]` (it measures, it
//! does not assert a budget): `cargo test --release -p auto-crop-imgproc --test curved_memory --
//! --ignored --nocapture`.

use auto_crop_core::{Curve, CurveWarp, Pt};
use auto_crop_imgproc::cancel::NeverCancel;
use auto_crop_imgproc::curved::{output_size, render_curved_image};
use auto_crop_imgproc::pixels::Image;
use auto_crop_imgproc::render::Limits;
use auto_crop_imgproc::warp::BAND_ROWS;
use peak_alloc::PeakAlloc;
use std::sync::Mutex;
use std::time::Instant;

#[global_allocator]
static ALLOC: PeakAlloc = PeakAlloc;

/// The tests share the global counters, so they run one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

const UNLIMITED: Limits = Limits {
    max_pixels: u64::MAX,
    max_edge: u32::MAX,
};

/// A page that fills most of the frame, with all four edges bowed (a handheld receipt).
fn page() -> CurveWarp {
    let c = |p: &[(f64, f64)]| Curve::new(p.iter().map(|&(x, y)| Pt::new(x, y)).collect()).unwrap();
    CurveWarp {
        top: c(&[
            (0.06, 0.08),
            (0.3, 0.04),
            (0.55, 0.07),
            (0.8, 0.045),
            (0.95, 0.07),
        ]),
        right: c(&[(0.95, 0.07), (0.975, 0.35), (0.96, 0.65), (0.94, 0.93)]),
        bottom: c(&[(0.94, 0.93), (0.7, 0.97), (0.4, 0.94), (0.05, 0.96)]),
        left: c(&[(0.05, 0.96), (0.03, 0.6), (0.045, 0.3), (0.06, 0.08)]),
        quarter_turns: 0,
        mirror: false,
    }
}

fn source(w: u32, h: u32) -> Image<u8> {
    let mut src = Image::<u8>::new(w, h, 3);
    for (i, v) in src.data.iter_mut().enumerate() {
        *v = (i.wrapping_mul(31) >> 3) as u8;
    }
    src
}

fn render(src: &Image<u8>, page: &CurveWarp, threads: usize) -> (usize, usize, (u32, u32)) {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap();
    // Warm the rayon pool and the lazily built weight table so they are not billed to the render.
    pool.install(|| render_curved_image(src.as_ref(), page, Limits::pixels(64 * 64), &NeverCancel))
        .unwrap();
    let baseline = ALLOC.current_usage();
    ALLOC.reset_peak_usage();
    let out = pool
        .install(|| render_curved_image(src.as_ref(), page, UNLIMITED, &NeverCancel))
        .unwrap();
    let extra = ALLOC.peak_usage().saturating_sub(baseline);
    (extra, out.data.len(), (out.width, out.height))
}

/// Per-band scratch: four position rows (two f64, two i64) per worker thread, the edge tables
/// (four vectors of `(f64, f64)` per page column or row) and slack for the thread pool.
fn budget(out: (u32, u32), threads: usize) -> usize {
    threads * (out.0 as usize * 32 + BAND_ROWS * 64)
        + (out.0 as usize + out.1 as usize) * 32 * 2
        + 256 * 1024
}

#[test]
fn curved_12mp_extra_heap_is_the_output_plus_the_band_scratch() {
    let src = source(4000, 3000);
    let page = page();
    let size = output_size(4000, 3000, &page, UNLIMITED).unwrap();
    println!("page flattens to {} x {}", size.0, size.1);
    for threads in [1usize, 8] {
        let (extra, out_bytes, dims) = render(&src, &page, threads);
        let slack = budget(dims, threads);
        println!(
            "12 MP, {threads} thread(s): extra {extra} B, output {out_bytes} B, scratch budget {slack} B"
        );
        assert_eq!(dims, size);
        // The output, the scratch, and 1 MiB: nowhere near a second copy of the image.
        assert!(extra <= out_bytes + slack + 1024 * 1024, "{extra}");
        assert!(extra < out_bytes + 16 * 1024 * 1024, "{extra}");
    }
}

#[test]
#[ignore = "measures, does not assert; run with --release -- --ignored --nocapture"]
fn curved_12mp_timing() {
    let src = source(4000, 3000);
    let page = page();
    for threads in [1usize, 8] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        let mut best = f64::INFINITY;
        let mut dims = (0, 0);
        for _ in 0..6 {
            let t = Instant::now();
            let out = pool
                .install(|| render_curved_image(src.as_ref(), &page, UNLIMITED, &NeverCancel))
                .unwrap();
            best = best.min(t.elapsed().as_secs_f64() * 1000.0);
            dims = (out.width, out.height);
        }
        println!(
            "curved 12 MP source -> {} x {}: best of 6 on {threads} thread(s): {best:.0} ms",
            dims.0, dims.1
        );
        // The same size through the plain homography warp, for scale (a straight quad of the page's
        // corners).
        let c = page.corners();
        let px = c.map(|p| (p.x * 4000.0 - 0.5, p.y * 3000.0 - 0.5));
        let (w, h) = (f64::from(dims.0), f64::from(dims.1));
        let m = auto_crop_imgproc::geometry::homography(
            [
                (-0.5, -0.5),
                (w - 0.5, -0.5),
                (w - 0.5, h - 0.5),
                (-0.5, h - 0.5),
            ],
            px,
        )
        .unwrap();
        let mut best = f64::INFINITY;
        for _ in 0..6 {
            let t = Instant::now();
            pool.install(|| {
                auto_crop_imgproc::warp::warp_perspective_image(
                    src.as_ref(),
                    &m,
                    dims.0,
                    dims.1,
                    &NeverCancel,
                )
            })
            .unwrap();
            best = best.min(t.elapsed().as_secs_f64() * 1000.0);
        }
        println!("homography, same size: best of 6 on {threads} thread(s): {best:.0} ms");
    }
}
