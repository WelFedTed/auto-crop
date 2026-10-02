// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Strip-wise Lanczos3 perspective warp (ADR-0002, ROADMAP M1.24): u8 and u16 samples, 1 to 4
//! channels, rayon over 64-row output bands, no full-frame f32 buffer, a [`Cancel`] check at the
//! start of every band. Integer-centre convention: pixel `(i, j)` has its centre at `(i, j)`.
//! Output outside the source (more than half a pixel beyond its edge) is zero.
//!
//! Kernel layout (safe, portable, SSE2-friendly; no `unsafe`, no target features):
//!
//! * per output row, the source coordinates are computed in f64 in one vectorisable pre-pass, so
//!   100 MP sources keep sub-0.001 px accuracy;
//! * the 6x6 Lanczos footprint is evaluated vertical-first: the six source rows (each `6 * C`
//!   contiguous samples) are blended into one `6 * C` f32 vector with plain mul and add, which the
//!   compiler turns into 4-wide SIMD, then a short horizontal pass produces the `C` channels;
//! * weights come from a 1024-step table (nearest step, at most 1/2048 px error), pre-normalised
//!   to sum to exactly one; whole-pixel positions use weights `[0, 0, 1, 0, 0, 0]`, so identity,
//!   integer shifts and 90-degree turns are bit-exact;
//! * pixels whose footprint touches the border take a clamped-gather slow path; all others read
//!   straight from the source rows.
//!
//! Results are independent of the thread count: every output pixel is a pure function of the
//! source and the matrix.

use crate::Raster;
use crate::cancel::{Cancel, Cancelled, NeverCancel};
use crate::pixels::{Image, ImageRef, Sample};
use rayon::prelude::*;
use std::sync::OnceLock;

/// Output rows per rayon task, and the cancellation granularity (PLAN 2.8).
pub const BAND_ROWS: usize = 64;
const LUT_N: usize = 1024;
/// 1.5 * 2^52: adding it to a value below 2^51 leaves the rounded integer in the low mantissa bits.
const MAGIC: f64 = 6_755_399_441_055_744.0;

fn lanczos3(x: f64) -> f64 {
    if x == x.round() {
        // Exactly zero at the other whole pixels, so whole-pixel positions are bit-exact.
        return if x == 0.0 { 1.0 } else { 0.0 };
    }
    if x.abs() >= 3.0 {
        return 0.0;
    }
    let px = std::f64::consts::PI * x;
    3.0 * px.sin() * (px / 3.0).sin() / (px * px)
}

/// Normalised Lanczos3 weights for the six taps `x0 - 2 ..= x0 + 3` at fractional position `f`.
pub(crate) fn lanczos3_weights(f: f64) -> [f32; 6] {
    let mut w = [0.0f64; 6];
    let mut sum = 0.0;
    for (k, wk) in w.iter_mut().enumerate() {
        *wk = lanczos3(f - (k as f64 - 2.0));
        sum += *wk;
    }
    std::array::from_fn(|k| (w[k] / sum) as f32)
}

/// `LUT_N` entries: fractional position `i / LUT_N`.
fn lut() -> &'static [[f32; 6]] {
    static LUT: OnceLock<Vec<[f32; 6]>> = OnceLock::new();
    LUT.get_or_init(|| {
        (0..LUT_N)
            .map(|i| lanczos3_weights(i as f64 / LUT_N as f64))
            .collect()
    })
}

/// One source image (or one pyramid level of it) with the matrix that maps output pixel centres
/// to this image's pixel centres.
pub(crate) struct View<'a, T: Sample> {
    pub data: &'a [T],
    pub w: usize,
    pub h: usize,
    pub m: [f64; 9],
}

/// Blends the six rows into one vector, then reduces horizontally. `rows[k]` holds the `6 * C`
/// samples of tap row `k` and then padding up to `N` (a multiple of four of at least `5 * C + 4`, so
/// the blend is whole SIMD vectors and the horizontal pass can load four lanes per tap); the
/// padding lanes are computed and ignored.
#[inline(always)]
fn convolve<T: Sample, const C: usize, const N: usize>(
    rows: [&[T; N]; 6],
    wx: &[f32; 6],
    wy: &[f32; 6],
) -> [f32; C] {
    let mut v = [0.0f32; N];
    for k in 0..6 {
        let w = wy[k];
        let r = rows[k];
        for i in 0..N {
            v[i] += w * r[i].to_f32();
        }
    }
    // Horizontal pass: four lanes starting at each pixel (lanes past `C` read the next pixel and
    // are ignored), so every step is one unaligned vector load, multiply and add whatever `C` is.
    let mut acc = [0.0f32; 4];
    for j in 0..6 {
        let w = wx[j];
        for l in 0..4 {
            acc[l] += w * v[j * C + l];
        }
    }
    std::array::from_fn(|c| acc[c])
}

/// The border path: gathers the 6 x 6 footprint with clamped coordinates, then convolves. Kept
/// out of line so the hot interior loop stays small.
#[inline(never)]
fn convolve_edge<T: Sample, const C: usize, const N: usize>(
    view: &View<'_, T>,
    x0: i32,
    y0: i32,
    wx: &[f32; 6],
    wy: &[f32; 6],
) -> [f32; C] {
    let (sw, sh) = (view.w, view.h);
    let mut patch = [[T::default(); N]; 6]; // padding lanes stay zero
    for (k, prow) in patch.iter_mut().enumerate() {
        let yi = (y0 + k as i32 - 2).clamp(0, sh as i32 - 1) as usize;
        for j in 0..6 {
            let xi = (x0 + j as i32 - 2).clamp(0, sw as i32 - 1) as usize;
            let s = (yi * sw + xi) * C;
            prow[j * C..j * C + C].copy_from_slice(&view.data[s..s + C]);
        }
    }
    convolve::<T, C, N>(
        [
            &patch[0], &patch[1], &patch[2], &patch[3], &patch[4], &patch[5],
        ],
        wx,
        wy,
    )
}

/// Warps one band of output rows (`v0..v0 + rows`) into `chunk`.
fn warp_band<T: Sample, const C: usize, const N: usize>(
    view: &View<'_, T>,
    v0: usize,
    dw: usize,
    chunk: &mut [T],
) {
    let lut = lut();
    let (sw, sh) = (view.w, view.h);
    let m = &view.m;
    let (x_hi, y_hi) = (sw as f64 - 0.5, sh as f64 - 0.5);
    // Per row: source position of every output pixel as 1/1024-pixel fixed point of `x + 1`
    // (rounded to the nearest step), or -1 for "outside the source". Computed in f64 so 100 MP
    // sources keep sub-0.001 px accuracy; the loop has no data-dependent branches.
    let mut tx = vec![-1i32; dw];
    let mut ty = vec![-1i32; dw];
    // Round to the nearest 1/1024 and read the integer out of the mantissa: a plain `as i32`
    // is a saturating conversion (several instructions); this one vectorises.
    let fixed = |t: f64| -> i32 { (t * LUT_N as f64 + MAGIC).to_bits() as i32 };
    for (r, row) in chunk.chunks_exact_mut(dw * C).enumerate() {
        let v = (v0 + r) as f64;
        let (nx0, ny0, d0) = (m[1] * v + m[2], m[4] * v + m[5], m[7] * v + m[8]);
        for u in 0..dw {
            let uf = u as f64;
            let inv = 1.0 / (m[6] * uf + d0);
            let x = (m[0] * uf + nx0) * inv;
            let y = (m[3] * uf + ny0) * inv;
            // Half a pixel of slack so edge pixels sample cleanly; also rejects NaN and infinity.
            let inside = x >= -0.5 && x < x_hi && y >= -0.5 && y < y_hi;
            tx[u] = if inside { fixed(x + 1.0) } else { -1 };
            ty[u] = if inside { fixed(y + 1.0) } else { -1 };
        }
        for u in 0..dw {
            let (px, py) = (tx[u], ty[u]);
            if px < 0 {
                continue;
            }
            // `px >> 10` is x0 + 1 (x0 = floor of the rounded position), the low bits are the
            // weight-table index; a position rounded up to a whole pixel has index 0.
            let (x0, y0) = ((px >> 10) - 1, (py >> 10) - 1);
            let mask = LUT_N as i32 - 1;
            let (wxi, wyi) = ((px & mask) as usize, (py & mask) as usize);
            let o = &mut row[u * C..u * C + C];

            // Whole-pixel position: copy (exact, and much cheaper).
            if (wxi | wyi) == 0 {
                let ix = x0.clamp(0, sw as i32 - 1) as usize;
                let iy = y0.clamp(0, sh as i32 - 1) as usize;
                let s = (iy * sw + ix) * C;
                o.copy_from_slice(&view.data[s..s + C]);
                continue;
            }

            let (wx, wy) = (&lut[wxi], &lut[wyi]);
            let acc: [f32; C] = if x0 >= 2
                && y0 >= 2
                && (x0 - 2) as usize * C + N <= sw * C
                && ((y0 + 3) as usize) < sh
            {
                let (x0, y0) = (x0 as usize, y0 as usize);
                let tap = |k: usize| -> &[T; N] {
                    let s = ((y0 - 2 + k) * sw + (x0 - 2)) * C;
                    <&[T; N]>::try_from(&view.data[s..s + N]).expect("window of N samples")
                };
                convolve::<T, C, N>([tap(0), tap(1), tap(2), tap(3), tap(4), tap(5)], wx, wy)
            } else {
                convolve_edge::<T, C, N>(view, x0, y0, wx, wy)
            };
            for c in 0..C {
                o[c] = T::from_f32_round(acc[c]);
            }
        }
    }
}

/// Output buffer plus the band loop shared by the plain and the guarded warp.
pub(crate) fn warp_with_views<'v, T, F>(
    channels: u8,
    dw: usize,
    dh: usize,
    cancel: &dyn Cancel,
    view_for_band: F,
) -> Result<Vec<T>, Cancelled>
where
    T: Sample,
    F: Fn(usize) -> View<'v, T> + Sync,
{
    let c = usize::from(channels);
    let mut out = vec![T::default(); dw * dh * c];
    if dw == 0 || dh == 0 {
        return Ok(out);
    }
    let band_len = dw * c * BAND_ROWS;
    let result: Result<(), Cancelled> =
        out.par_chunks_mut(band_len)
            .enumerate()
            .try_for_each(|(band, chunk)| {
                if cancel.is_cancelled() {
                    return Err(Cancelled);
                }
                let view = view_for_band(band);
                let v0 = band * BAND_ROWS;
                match c {
                    1 => warp_band::<T, 1, 12>(&view, v0, dw, chunk),
                    2 => warp_band::<T, 2, 16>(&view, v0, dw, chunk),
                    3 => warp_band::<T, 3, 20>(&view, v0, dw, chunk),
                    _ => warp_band::<T, 4, 24>(&view, v0, dw, chunk),
                }
                Ok(())
            });
    result.map(|()| out)
}

/// Resamples `src` through `dst_to_src` (a homography from output pixel centres to source pixel
/// centres) into a new `out_w` x `out_h` image. An empty source gives an all-zero image. Stops
/// at the next band boundary with `Err(Cancelled)` once `cancel` reports true.
pub fn warp_perspective_image<T: Sample>(
    src: ImageRef<'_, T>,
    dst_to_src: &[f64; 9],
    out_w: u32,
    out_h: u32,
    cancel: &dyn Cancel,
) -> Result<Image<T>, Cancelled> {
    let (dw, dh) = (out_w as usize, out_h as usize);
    let data = if src.width == 0 || src.height == 0 {
        vec![T::default(); dw * dh * usize::from(src.channels)]
    } else {
        let (sw, sh) = (src.width as usize, src.height as usize);
        warp_with_views(src.channels, dw, dh, cancel, |_| View {
            data: src.data,
            w: sw,
            h: sh,
            m: *dst_to_src,
        })?
    };
    Ok(Image {
        width: out_w,
        height: out_h,
        channels: src.channels,
        data,
    })
}

/// Resamples an 8-bit RGB raster. Returns an all-black image for an empty source.
pub fn warp_perspective(src: &Raster, dst_to_src: &[f64; 9], out_w: u32, out_h: u32) -> Raster {
    let dw = out_w as usize;
    let dh = out_h as usize;
    let data = if src.width == 0 || src.height == 0 {
        vec![0u8; dw * dh * 3]
    } else {
        let (sw, sh) = (src.width as usize, src.height as usize);
        warp_with_views(3, dw, dh, &NeverCancel, |_| View {
            data: &src.data[..],
            w: sw,
            h: sh,
            m: *dst_to_src,
        })
        .expect("NeverCancel never cancels")
    };
    Raster {
        width: out_w,
        height: out_h,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const ID: [f64; 9] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];

    fn gradient(w: u32, h: u32) -> Raster {
        let mut r = Raster::new(w, h);
        for y in 0..h {
            for x in 0..w {
                r.set_pixel(x, y, [(x * 255 / w) as u8, (y * 255 / h) as u8, 90]);
            }
        }
        r
    }

    fn noise<T: Sample + From<u8>>(w: u32, h: u32, ch: u8) -> Image<T> {
        let mut img = Image::<T>::new(w, h, ch);
        let mut s = 0x9E37_79B9u32;
        for v in &mut img.data {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            *v = T::from((s % 251) as u8);
        }
        img
    }

    fn pool(n: usize) -> rayon::ThreadPool {
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build()
            .unwrap()
    }

    #[test]
    fn identity_warp_reproduces_the_source_exactly() {
        let src = gradient(40, 30);
        let out = warp_perspective(&src, &ID, 40, 30);
        assert_eq!(out, src);
    }

    #[test]
    fn identity_is_exact_for_gray8_rgb8_and_16_bit() {
        let g: Image<u8> = noise(37, 29, 1);
        let o = warp_perspective_image(g.as_ref(), &ID, 37, 29, &NeverCancel).unwrap();
        assert_eq!(o, g);
        let mut w16 = Image::<u16>::new(33, 21, 3);
        for (i, v) in w16.data.iter_mut().enumerate() {
            *v = ((i as u32).wrapping_mul(2_654_435_761) >> 16) as u16;
        }
        let o = warp_perspective_image(w16.as_ref(), &ID, 33, 21, &NeverCancel).unwrap();
        assert_eq!(o, w16);
        let rgba: Image<u8> = noise(20, 20, 4);
        let o = warp_perspective_image(rgba.as_ref(), &ID, 20, 20, &NeverCancel).unwrap();
        assert_eq!(o, rgba);
    }

    #[test]
    fn an_integer_shift_moves_pixels_exactly() {
        let src = gradient(40, 30);
        // Output (u, v) samples source (u + 5, v + 3).
        let shift = [1.0, 0.0, 5.0, 0.0, 1.0, 3.0, 0.0, 0.0, 1.0];
        let out = warp_perspective(&src, &shift, 20, 20);
        for v in 0..20 {
            for u in 0..20 {
                assert_eq!(out.pixel(u, v), src.pixel(u + 5, v + 3));
            }
        }
    }

    #[test]
    fn ninety_degree_turns_are_exact() {
        let src = gradient(23, 17);
        // Clockwise quarter turn: output (u, v) of a 17 x 23 image samples source (v, 16 - u)
        // after the usual (h - 1 - y, x) rule: source x = v, source y = (17 - 1) - u.
        let m = [0.0, 1.0, 0.0, -1.0, 0.0, 16.0, 0.0, 0.0, 1.0];
        let out = warp_perspective(&src, &m, 17, 23);
        assert_eq!(out, src.rotated_quarter_turns(1));
    }

    #[test]
    fn outside_the_source_is_black_and_empty_inputs_do_not_panic() {
        let src = Raster::filled(8, 8, [200, 200, 200]);
        let far = [1.0, 0.0, 1000.0, 0.0, 1.0, 1000.0, 0.0, 0.0, 1.0];
        assert!(
            warp_perspective(&src, &far, 4, 4)
                .data
                .iter()
                .all(|b| *b == 0)
        );
        assert_eq!(
            warp_perspective(&Raster::new(0, 0), &ID, 4, 4).data.len(),
            48
        );
        assert_eq!(warp_perspective(&src, &ID, 0, 5).data.len(), 0);
        // A matrix that sends everything to infinity is all outside, not a panic.
        let inf = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0];
        assert!(
            warp_perspective(&src, &inf, 4, 4)
                .data
                .iter()
                .all(|b| *b == 0)
        );
    }

    #[test]
    fn flat_images_stay_flat_through_a_perspective_warp() {
        // Weights sum to one, so a flat image must come back flat (checks the normalisation).
        let src = Raster::filled(50, 40, [77, 130, 251]);
        let m = [0.9, 0.1, 3.0, -0.05, 1.1, 2.0, 0.0004, 0.0002, 1.0];
        let out = warp_perspective(&src, &m, 40, 30);
        for p in out.data.chunks(3) {
            assert!(p == [77, 130, 251] || p == [0, 0, 0], "{p:?}");
        }
        let inside = out.data.chunks(3).filter(|p| *p == [77, 130, 251]).count();
        assert!(inside > 40 * 30 / 2);
    }

    #[test]
    fn same_bytes_at_1_and_8_threads() {
        let src: Image<u8> = noise(300, 211, 3);
        let m = [0.93, 0.21, -4.0, -0.17, 0.88, 9.5, 0.0003, -0.0002, 1.0];
        let run = |n| {
            pool(n)
                .install(|| warp_perspective_image(src.as_ref(), &m, 410, 333, &NeverCancel))
                .unwrap()
        };
        let (a, b) = (run(1), run(8));
        assert_eq!(a, b);
        let g: Image<u16> = {
            let mut i = Image::<u16>::new(120, 90, 1);
            for (k, v) in i.data.iter_mut().enumerate() {
                *v = ((k * 7919) % 65521) as u16;
            }
            i
        };
        let run16 = |n| {
            pool(n)
                .install(|| warp_perspective_image(g.as_ref(), &m, 100, 100, &NeverCancel))
                .unwrap()
        };
        assert_eq!(run16(1), run16(8));
    }

    /// Returns true after `limit` polls and counts every poll.
    struct CancelAfter {
        limit: usize,
        polls: AtomicUsize,
    }

    impl Cancel for CancelAfter {
        fn is_cancelled(&self) -> bool {
            self.polls.fetch_add(1, Ordering::SeqCst) >= self.limit
        }
    }

    #[test]
    fn cancel_stops_at_a_band_boundary_and_returns_no_image() {
        let src: Image<u8> = noise(64, 64, 3);
        let bands = 40usize;
        let token = CancelAfter {
            limit: 3,
            polls: AtomicUsize::new(0),
        };
        let r = pool(1).install(|| {
            warp_perspective_image(src.as_ref(), &ID, 64, (bands * BAND_ROWS) as u32, &token)
        });
        assert_eq!(r.unwrap_err(), Cancelled);
        // One poll per band started: it stopped within one band of the flip, far short of 40.
        assert!(token.polls.load(Ordering::SeqCst) <= 3 + 2);
    }

    #[test]
    fn cancel_while_running_returns_within_two_band_times() {
        use std::sync::atomic::AtomicBool;
        use std::time::{Duration, Instant};
        let src: Image<u8> = noise(2000, 1500, 3);
        // Scale 0.25 so every one of the 6000 output rows lands inside the source.
        let m = [0.25, 0.04, 3.3, -0.03, 0.25, 2.2, 0.0, 0.0, 1.0];
        let p = pool(2);
        // Time one band (best of five, after warm-up), then a full run with the flag raised
        // after ~10 band-times.
        let band = (0..5)
            .map(|_| {
                let t = Instant::now();
                p.install(|| {
                    warp_perspective_image(src.as_ref(), &m, 2000, BAND_ROWS as u32, &NeverCancel)
                })
                .unwrap();
                t.elapsed()
            })
            .min()
            .unwrap();
        let flag = AtomicBool::new(false);
        let raised = std::sync::Mutex::new(None);
        let done = std::thread::scope(|s| {
            s.spawn(|| {
                std::thread::sleep(band * 10);
                *raised.lock().unwrap() = Some(Instant::now());
                flag.store(true, Ordering::SeqCst);
            });
            let r = p.install(|| warp_perspective_image(src.as_ref(), &m, 2000, 6000, &flag));
            (r.is_err(), Instant::now())
        });
        assert!(done.0, "the run was long enough to be cancelled");
        let latency = done.1 - raised.lock().unwrap().unwrap();
        // Two band-times plus generous scheduler slack: shared CI runners deschedule threads for
        // tens of milliseconds. The deterministic poll-count test above is the tight check.
        assert!(
            latency <= band * 2 + Duration::from_millis(400),
            "latency {latency:?}, band {band:?}"
        );
    }
}
