// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Warp minification guard (ROADMAP M1.26). A 6 x 6 Lanczos footprint spans six *source* pixels,
//! so when the map shrinks the image it samples sparsely and aliases. Above a local scale of
//! [`MAX_LOCAL_SCALE`] (1.5, PROVISIONAL: source pixels per output pixel) each 64-row output band
//! is therefore warped from the nearest pre-reduced level of a power-of-two box-filter pyramid
//! instead: the smallest level `l` with `scale / 2^l <= 1.5`, so the kernel never sees more than
//! 1.5x minification and never less than 0.75x (a mild enlargement of an already reduced level).
//!
//! * The scale is the largest singular value of the map's Jacobian (source pixels per output
//!   pixel along the worst axis), the maximum over a 5 x 3 grid of sample points in the band.
//! * A level of reduction `f = 2^l` is the exact area average of `f x f` source blocks
//!   (rounded; blocks clipped at the right and bottom border), built directly from the source.
//!   Levels are built only if some band needs them, and kept only for the duration of the call.
//! * Bands choose their level independently, so a perspective map may switch levels between
//!   bands; on content the lower level can represent, the switch is invisible (<= 2 LSB, tested).
//!
//! The result is independent of the thread count.

use crate::cancel::{Cancel, Cancelled};
use crate::pixels::{Image, ImageRef, Sample};
use crate::warp::{BAND_ROWS, View, warp_with_views};
use rayon::prelude::*;

/// Source pixels per output pixel above which a band is warped from a reduced level.
pub const MAX_LOCAL_SCALE: f64 = 1.5;

/// Deepest level considered (a 4096x reduction).
const MAX_LEVEL: u32 = 12;

/// The local scale of `m` at output pixel `(u, v)`: the largest singular value of the Jacobian of
/// the map there. `None` if the point maps to infinity.
pub fn local_scale(m: &[f64; 9], u: f64, v: f64) -> Option<f64> {
    let w = m[6] * u + m[7] * v + m[8];
    if w.is_nan() || w.abs() <= 1e-300 {
        return None;
    }
    let x = (m[0] * u + m[1] * v + m[2]) / w;
    let y = (m[3] * u + m[4] * v + m[5]) / w;
    // d(x)/du = (m0 - x m6) / w, and so on.
    let (a, b) = ((m[0] - x * m[6]) / w, (m[1] - x * m[7]) / w);
    let (c, d) = ((m[3] - y * m[6]) / w, (m[4] - y * m[7]) / w);
    // Largest singular value of [[a, b], [c, d]].
    let (p, q) = (a * a + b * b + c * c + d * d, (a * d - b * c).abs());
    let s = (0.5 * (p + (p * p - 4.0 * q * q).max(0.0).sqrt())).sqrt();
    s.is_finite().then_some(s)
}

/// Smallest `l` with `scale / 2^l <= max_scale` (0 when `scale <= max_scale`).
pub fn level_for_scale(scale: f64, max_scale: f64) -> u32 {
    let mut l = 0;
    while l < MAX_LEVEL && scale / f64::from(1u32 << l) > max_scale {
        l += 1;
    }
    l
}

/// Level chosen for output rows `v0..v1` of a `dw`-wide output.
fn band_level(
    m: &[f64; 9],
    dw: usize,
    v0: usize,
    v1: usize,
    min_dim: usize,
    max_scale: f64,
) -> u32 {
    let mut worst: f64 = 0.0;
    let rows = [v0 as f64, 0.5 * (v0 + v1 - 1) as f64, (v1 - 1) as f64];
    for v in rows {
        for k in 0..5 {
            let u = (dw.saturating_sub(1)) as f64 * f64::from(k) / 4.0;
            if let Some(s) = local_scale(m, u, v) {
                worst = worst.max(s);
            }
        }
    }
    // A level below one pixel is meaningless.
    let cap = (usize::BITS - 1 - min_dim.max(1).leading_zeros()).min(MAX_LEVEL);
    level_for_scale(worst, max_scale).min(cap)
}

/// Exact area average of `f x f` blocks (clipped at the border), rounded to nearest.
pub fn reduce_box<T: Sample + Into<u32> + TryFrom<u32>>(
    src: ImageRef<'_, T>,
    f: usize,
) -> Image<T> {
    assert!(f >= 1);
    let (sw, sh, c) = (
        src.width as usize,
        src.height as usize,
        usize::from(src.channels),
    );
    let (ow, oh) = (sw.div_ceil(f).max(1), sh.div_ceil(f).max(1));
    let mut out = Image::<T>::new(ow as u32, oh as u32, src.channels);
    if sw == 0 || sh == 0 {
        return out;
    }
    out.data
        .par_chunks_mut(ow * c)
        .enumerate()
        .for_each(|(oy, row)| {
            let mut sums = vec![0u64; ow * c];
            let (y0, y1) = (oy * f, ((oy + 1) * f).min(sh));
            for y in y0..y1 {
                let line = &src.data[y * sw * c..(y + 1) * sw * c];
                for (ox, block) in line.chunks(f * c).enumerate() {
                    let acc = &mut sums[ox * c..ox * c + c];
                    for px in block.chunks_exact(c) {
                        for (a, v) in acc.iter_mut().zip(px) {
                            *a += u64::from((*v).into());
                        }
                    }
                }
            }
            for (ox, o) in row.chunks_exact_mut(c).enumerate() {
                let n = ((y1 - y0) * ((ox + 1) * f).min(sw).saturating_sub(ox * f)) as u64;
                for (k, v) in o.iter_mut().enumerate() {
                    let avg = (sums[ox * c + k] + n / 2) / n;
                    *v = T::try_from(avg as u32).ok().unwrap_or_default();
                }
            }
        });
    out
}

/// The matrix mapping output pixel centres to the pixel centres of the level reduced by `f`.
fn level_matrix(m: &[f64; 9], f: f64) -> [f64; 9] {
    let off = 0.5 / f - 0.5;
    let mut o = *m;
    for col in 0..3 {
        o[col] = m[col] / f + off * m[6 + col];
        o[3 + col] = m[3 + col] / f + off * m[6 + col];
    }
    o
}

/// Like [`crate::warp::warp_perspective_image`], with the minification guard at
/// [`MAX_LOCAL_SCALE`].
pub fn warp_perspective_guarded<T: Sample + Into<u32> + TryFrom<u32>>(
    src: ImageRef<'_, T>,
    dst_to_src: &[f64; 9],
    out_w: u32,
    out_h: u32,
    cancel: &dyn Cancel,
) -> Result<Image<T>, Cancelled> {
    warp_perspective_guarded_with(src, dst_to_src, out_w, out_h, cancel, MAX_LOCAL_SCALE)
}

/// The guard with an explicit threshold (for calibration; the engine uses [`MAX_LOCAL_SCALE`]).
pub fn warp_perspective_guarded_with<T: Sample + Into<u32> + TryFrom<u32>>(
    src: ImageRef<'_, T>,
    dst_to_src: &[f64; 9],
    out_w: u32,
    out_h: u32,
    cancel: &dyn Cancel,
    max_scale: f64,
) -> Result<Image<T>, Cancelled> {
    let (dw, dh) = (out_w as usize, out_h as usize);
    let c = usize::from(src.channels);
    if src.width == 0 || src.height == 0 || dw == 0 || dh == 0 {
        return Ok(Image {
            width: out_w,
            height: out_h,
            channels: src.channels,
            data: vec![T::default(); dw * dh * c],
        });
    }
    let (sw, sh) = (src.width as usize, src.height as usize);
    let bands = dh.div_ceil(BAND_ROWS);
    let levels: Vec<u32> = (0..bands)
        .map(|b| {
            band_level(
                dst_to_src,
                dw,
                b * BAND_ROWS,
                ((b + 1) * BAND_ROWS).min(dh),
                sw.min(sh),
                max_scale,
            )
        })
        .collect();
    // Build each needed level once (level 0 is the source itself).
    let mut needed: Vec<u32> = levels.iter().copied().filter(|l| *l > 0).collect();
    needed.sort_unstable();
    needed.dedup();
    let mut built: Vec<(u32, Image<T>)> = Vec::new();
    for l in needed {
        if cancel.is_cancelled() {
            return Err(Cancelled);
        }
        built.push((l, reduce_box(src, 1usize << l)));
    }
    let view_for_band = |band: usize| -> View<'_, T> {
        let l = levels[band];
        if l == 0 {
            View {
                data: src.data,
                w: sw,
                h: sh,
                m: *dst_to_src,
            }
        } else {
            let (_, img) = built
                .iter()
                .find(|(k, _)| *k == l)
                .expect("level was built");
            View {
                data: &img.data,
                w: img.width as usize,
                h: img.height as usize,
                m: level_matrix(dst_to_src, f64::from(1u32 << l)),
            }
        }
    };
    let data = warp_with_views(src.channels, dw, dh, cancel, view_for_band)?;
    Ok(Image {
        width: out_w,
        height: out_h,
        channels: src.channels,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cancel::NeverCancel;
    use crate::warp::warp_perspective_image;

    #[test]
    fn scale_of_a_uniform_map_is_its_factor() {
        let m = [2.5, 0.0, 10.0, 0.0, 1.5, -3.0, 0.0, 0.0, 1.0];
        assert!((local_scale(&m, 7.0, 9.0).unwrap() - 2.5).abs() < 1e-12);
        // A rotation does not change it.
        let (s, c) = 0.3f64.sin_cos();
        let r = [2.0 * c, -2.0 * s, 0.0, 2.0 * s, 2.0 * c, 0.0, 0.0, 0.0, 1.0];
        assert!((local_scale(&r, 5.0, 5.0).unwrap() - 2.0).abs() < 1e-12);
        // Perspective: scale grows where the denominator shrinks.
        let p = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, -0.002, 1.0];
        assert!(local_scale(&p, 0.0, 400.0).unwrap() > local_scale(&p, 0.0, 0.0).unwrap());
    }

    #[test]
    fn level_choice_keeps_the_kernel_between_075x_and_15x() {
        for (scale, level) in [
            (0.4, 0),
            (1.0, 0),
            (1.5, 0),
            (1.51, 1),
            (3.0, 1),
            (3.01, 2),
            (6.0, 2),
            (6.5, 3),
        ] {
            assert_eq!(
                level_for_scale(scale, MAX_LOCAL_SCALE),
                level,
                "scale {scale}"
            );
        }
    }

    #[test]
    fn box_reduction_averages_blocks_and_handles_ragged_borders() {
        let mut img = Image::<u8>::new(5, 3, 1);
        for (i, v) in img.data.iter_mut().enumerate() {
            *v = (i * 10) as u8;
        }
        let r = reduce_box(img.as_ref(), 2);
        assert_eq!((r.width, r.height), (3, 2));
        // Top-left block: pixels 0, 10, 50, 60 -> mean 30.
        assert_eq!(r.data[0], 30);
        // Right border block (column 4 only, rows 0-1): 40 and 90 -> 65.
        assert_eq!(r.data[2], 65);
        // Bottom row (row 2 only), first block: 100 and 110 -> 105.
        assert_eq!(r.data[3], 105);
        assert_eq!(reduce_box(img.as_ref(), 1), img);
    }

    #[test]
    fn mild_maps_are_identical_to_the_plain_warp() {
        let mut img = Image::<u8>::new(120, 90, 3);
        for (i, v) in img.data.iter_mut().enumerate() {
            *v = (i.wrapping_mul(2_654_435_761) >> 11) as u8;
        }
        let m = [1.2, 0.1, 3.0, -0.1, 1.1, 2.0, 0.0001, 0.0, 1.0];
        let a = warp_perspective_guarded(img.as_ref(), &m, 100, 80, &NeverCancel).unwrap();
        let b = warp_perspective_image(img.as_ref(), &m, 100, 80, &NeverCancel).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn same_bytes_at_1_and_8_threads() {
        let mut img = Image::<u8>::new(900, 700, 3);
        for (i, v) in img.data.iter_mut().enumerate() {
            *v = (i.wrapping_mul(2_654_435_761) >> 11) as u8;
        }
        // Scale 1.2 at the top rising past 4 at the bottom: levels 0, 1 and 2 all occur.
        let m = [1.2, 0.0, 0.0, 0.0, 1.2, 0.0, 0.0, 0.0035, 1.0];
        let run = |n| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(n)
                .build()
                .unwrap()
                .install(|| warp_perspective_guarded(img.as_ref(), &m, 300, 260, &NeverCancel))
                .unwrap()
        };
        assert_eq!(run(1), run(8));
    }

    #[test]
    fn cancel_before_the_level_build_returns_cancelled() {
        let img = Image::<u8>::new(64, 64, 1);
        let flag = std::sync::atomic::AtomicBool::new(true);
        let m = [4.0, 0.0, 0.0, 0.0, 4.0, 0.0, 0.0, 0.0, 1.0];
        assert_eq!(
            warp_perspective_guarded(img.as_ref(), &m, 16, 16, &flag).unwrap_err(),
            Cancelled
        );
    }
}
