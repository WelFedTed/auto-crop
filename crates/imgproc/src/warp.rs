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
pub const BAND_ROWS: usize = auto_crop_core::BAND_ROWS as usize;
const LUT_N: usize = 1024;
/// 1.5 * 2^52: adding it to a value below 2^51 leaves the rounded integer in the low mantissa bits.
const MAGIC: f64 = 6_755_399_441_055_744.0;
const MAGIC_BITS: i64 = MAGIC.to_bits() as i64;

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

/// `LUT_N + 1` entries: fractional position `i / LUT_N` (the last one is the whole-pixel weights
/// of the next pixel, present so 16-bit sampling can interpolate between neighbouring entries).
fn lut() -> &'static [[f32; 6]] {
    static LUT: OnceLock<Vec<[f32; 6]>> = OnceLock::new();
    LUT.get_or_init(|| {
        (0..=LUT_N)
            .map(|i| lanczos3_weights(i as f64 / LUT_N as f64))
            .collect()
    })
}

/// Fractional bits of the per-pixel source position (16-bit samples use all of them, 8-bit
/// samples round to the 10 bits of the weight table).
const POS_BITS: u32 = 16;

/// Splits a fixed-point position `p` (source coordinate plus one, in 1/65536 pixel) into the
/// tap origin `x0` (the pixel at or left of the position), whether it is a whole pixel, and the
/// six tap weights. 8-bit samples use the nearest table entry (at most 1/2048 px off, under 0.2
/// LSB on a full-scale edge); 16-bit samples interpolate between entries (error well under one
/// 16-bit LSB).
#[inline(always)]
fn locate<T: Sample>(lut: &[[f32; 6]], p: i64) -> (i32, bool, [f32; 6]) {
    if T::PRECISE {
        let f = (p & ((1 << POS_BITS) - 1)) as usize;
        let i = f >> (POS_BITS as usize - 10);
        let t = (f & ((1 << (POS_BITS - 10)) - 1)) as f32 * (1.0 / (1 << (POS_BITS - 10)) as f32);
        let (a, b) = (&lut[i], &lut[i + 1]);
        let w = std::array::from_fn(|k| a[k] + t * (b[k] - a[k]));
        (((p >> POS_BITS) - 1) as i32, f == 0, w)
    } else {
        let q = (p + (1 << (POS_BITS - 11))) >> (POS_BITS - 10);
        let i = (q & (LUT_N as i64 - 1)) as usize;
        (((q >> 10) - 1) as i32, i == 0, lut[i])
    }
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

/// Round to the nearest 1/65536 and read the integer out of the mantissa: a plain `as i64` is a
/// saturating conversion (several instructions); this one vectorises.
#[inline(always)]
fn fixed(t: f64) -> i64 {
    (t * (1u64 << POS_BITS) as f64 + MAGIC).to_bits() as i64 - MAGIC_BITS
}

/// Samples one output row at the fixed-point source positions `tx`, `ty` (`tx[u] < 0` means
/// "outside the source": the pixel is left as it is, zero). Shared by the homography warp and the
/// map-driven warp, so both produce bytes by the very same kernel.
#[inline(always)]
fn sample_row<T: Sample, const C: usize, const N: usize>(
    view: &View<'_, T>,
    lut: &[[f32; 6]],
    tx: &[i64],
    ty: &[i64],
    row: &mut [T],
) {
    let (sw, sh) = (view.w, view.h);
    for u in 0..tx.len() {
        let (px, py) = (tx[u], ty[u]);
        if px < 0 {
            continue;
        }
        let (x0, xwhole, wx) = locate::<T>(lut, px);
        let (y0, ywhole, wy) = locate::<T>(lut, py);
        let o = &mut row[u * C..u * C + C];

        // Whole-pixel position: copy (exact, and much cheaper).
        if xwhole && ywhole {
            let ix = x0.clamp(0, sw as i32 - 1) as usize;
            let iy = y0.clamp(0, sh as i32 - 1) as usize;
            let s = (iy * sw + ix) * C;
            o.copy_from_slice(&view.data[s..s + C]);
            continue;
        }

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
            convolve::<T, C, N>([tap(0), tap(1), tap(2), tap(3), tap(4), tap(5)], &wx, &wy)
        } else {
            convolve_edge::<T, C, N>(view, x0, y0, &wx, &wy)
        };
        for c in 0..C {
            o[c] = T::from_f32_round(acc[c]);
        }
    }
}

/// Converts a source position (pixel-centre coordinates) to fixed point, or `-1` for "outside":
/// more than half a pixel beyond the edge, NaN or infinite.
#[inline(always)]
fn to_fixed(x: f64, y: f64, x_hi: f64, y_hi: f64) -> (i64, i64) {
    // Half a pixel of slack so edge pixels sample cleanly; also rejects NaN and infinity.
    let inside = x >= -0.5 && x < x_hi && y >= -0.5 && y < y_hi;
    if inside {
        (fixed(x + 1.0), fixed(y + 1.0))
    } else {
        (-1, -1)
    }
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
    // Per row: source position of every output pixel as 1/65536-pixel fixed point of `x + 1`
    // (so it is never negative), or -1 for "outside the source". Computed in f64 so 100 MP
    // sources keep sub-0.001 px accuracy; the loop has no data-dependent branches.
    let mut tx = vec![-1i64; dw];
    let mut ty = vec![-1i64; dw];
    for (r, row) in chunk.chunks_exact_mut(dw * C).enumerate() {
        let v = (v0 + r) as f64;
        let (nx0, ny0, d0) = (m[1] * v + m[2], m[4] * v + m[5], m[7] * v + m[8]);
        for u in 0..dw {
            let uf = u as f64;
            let inv = 1.0 / (m[6] * uf + d0);
            let x = (m[0] * uf + nx0) * inv;
            let y = (m[3] * uf + ny0) * inv;
            (tx[u], ty[u]) = to_fixed(x, y, x_hi, y_hi);
        }
        sample_row::<T, C, N>(view, lut, &tx, &ty, row);
    }
}

/// A map provider: fills `xs` and `ys` with the source position (pixel-centre coordinates, the
/// same convention as the matrix of [`warp_perspective_image`]) of every pixel of output row `row`.
/// Called once per output row, from several threads at once.
pub type MapRow<'a> = dyn Fn(usize, &mut [f64], &mut [f64]) + Sync + 'a;

/// Warps one band driven by a map provider instead of a matrix.
fn warp_band_map<T: Sample, const C: usize, const N: usize>(
    view: &View<'_, T>,
    map: &MapRow<'_>,
    v0: usize,
    dw: usize,
    chunk: &mut [T],
) {
    let lut = lut();
    let (x_hi, y_hi) = (view.w as f64 - 0.5, view.h as f64 - 0.5);
    let mut xs = vec![0.0f64; dw];
    let mut ys = vec![0.0f64; dw];
    let mut tx = vec![-1i64; dw];
    let mut ty = vec![-1i64; dw];
    for (r, row) in chunk.chunks_exact_mut(dw * C).enumerate() {
        map(v0 + r, &mut xs, &mut ys);
        for u in 0..dw {
            (tx[u], ty[u]) = to_fixed(xs[u], ys[u], x_hi, y_hi);
        }
        sample_row::<T, C, N>(view, lut, &tx, &ty, row);
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

/// The band loop of the map-driven warp: ~64-row bands, one cancel check per band, extra heap
/// of four row-sized vectors per worker thread (no full-frame map, no f32 copy of anything).
fn warp_map_with_view<T: Sample>(
    view: &View<'_, T>,
    channels: u8,
    dw: usize,
    dh: usize,
    cancel: &dyn Cancel,
    map: &MapRow<'_>,
) -> Result<Vec<T>, Cancelled> {
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
                let v0 = band * BAND_ROWS;
                match c {
                    1 => warp_band_map::<T, 1, 12>(view, map, v0, dw, chunk),
                    2 => warp_band_map::<T, 2, 16>(view, map, v0, dw, chunk),
                    3 => warp_band_map::<T, 3, 20>(view, map, v0, dw, chunk),
                    _ => warp_band_map::<T, 4, 24>(view, map, v0, dw, chunk),
                }
                Ok(())
            });
    result.map(|()| out)
}

/// Resamples `src` into a new `out_w` x `out_h` image where output pixel `(x, y)` takes the source
/// position `map` gives for its row (the coordinate-provider seam of ROADMAP M4.25 and M12.30).
/// Lanczos3, u8 or u16, 1 to 4 channels, bit-identical across thread counts, stops at the next
/// band boundary with `Err(Cancelled)`. Positions more than half a pixel outside the source give
/// zero, exactly as [`warp_perspective_image`].
pub fn warp_map_image<T: Sample>(
    src: ImageRef<'_, T>,
    out_w: u32,
    out_h: u32,
    cancel: &dyn Cancel,
    map: &MapRow<'_>,
) -> Result<Image<T>, Cancelled> {
    let (dw, dh) = (out_w as usize, out_h as usize);
    let data = if src.width == 0 || src.height == 0 {
        vec![T::default(); dw * dh * usize::from(src.channels)]
    } else {
        let view = View {
            data: src.data,
            w: src.width as usize,
            h: src.height as usize,
            m: [0.0; 9],
        };
        warp_map_with_view(&view, src.channels, dw, dh, cancel, map)?
    };
    Ok(Image {
        width: out_w,
        height: out_h,
        channels: src.channels,
        data,
    })
}

/// A dense source-space grid: `cols * rows` node positions (source pixel-centre coordinates),
/// row-major, covering the output from edge to edge (node `(i, j)` sits at the output's
/// `(i / (cols - 1), j / (rows - 1))` of its width and height).
#[derive(Debug, Clone, Copy)]
pub struct SrcGrid<'a> {
    pub cols: usize,
    pub rows: usize,
    pub nodes: &'a [(f64, f64)],
}

/// [`warp_map_image`] driven by a [`SrcGrid`]: output pixel `(x, y)` (its centre, `(x + 0.5) /
/// out_w` across) takes the bilinear interpolation of the four surrounding nodes. `None` for a
/// grid with fewer than 2 x 2 nodes or the wrong node count.
pub fn warp_grid_image<T: Sample>(
    src: ImageRef<'_, T>,
    grid: SrcGrid<'_>,
    out_w: u32,
    out_h: u32,
    cancel: &dyn Cancel,
) -> Option<Result<Image<T>, Cancelled>> {
    if grid.cols < 2 || grid.rows < 2 || grid.nodes.len() != grid.cols * grid.rows {
        return None;
    }
    let (ow, oh) = (f64::from(out_w.max(1)), f64::from(out_h.max(1)));
    let map = move |y: usize, xs: &mut [f64], ys: &mut [f64]| {
        let gv =
            ((y as f64 + 0.5) / oh * (grid.rows - 1) as f64).clamp(0.0, (grid.rows - 1) as f64);
        let j = (gv.floor() as usize).min(grid.rows - 2);
        let fv = gv - j as f64;
        let (top, bot) = (
            &grid.nodes[j * grid.cols..(j + 1) * grid.cols],
            &grid.nodes[(j + 1) * grid.cols..(j + 2) * grid.cols],
        );
        for (x, (px, py)) in xs.iter_mut().zip(ys.iter_mut()).enumerate() {
            let gu =
                ((x as f64 + 0.5) / ow * (grid.cols - 1) as f64).clamp(0.0, (grid.cols - 1) as f64);
            let i = (gu.floor() as usize).min(grid.cols - 2);
            let fu = gu - i as f64;
            let (a, b, c, d) = (top[i], top[i + 1], bot[i], bot[i + 1]);
            *px = (1.0 - fv) * ((1.0 - fu) * a.0 + fu * b.0) + fv * ((1.0 - fu) * c.0 + fu * d.0);
            *py = (1.0 - fv) * ((1.0 - fu) * a.1 + fu * b.1) + fv * ((1.0 - fu) * c.1 + fu * d.1);
        }
    };
    Some(warp_map_image(src, out_w, out_h, cancel, &map))
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
