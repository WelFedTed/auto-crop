// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Binarisation kernels (ROADMAP M1.27): global Otsu and the local Sauvola and NICK thresholds.
//!
//! The local thresholds need the sum and the sum of squares of every `(2r + 1)` square window. A
//! whole-image integral image would be 2 x 8 bytes per pixel (about 192 MB at 12 MP) and its
//! squared-sum plane overflows `u32` above 66,000 pixels of value 255 (`imageproc` uses `u32`
//! there). These kernels instead keep *column* sums for the current window rows, in `u64`, for
//! one strip of output rows at a time: memory is a few `u64` per column per worker thread
//! (O(width), independent of the window and the image height), every sum is exact, and
//! the work per pixel is O(1) however large the window.
//!
//! * A window is clipped at the image border (statistics over the pixels that exist).
//! * A pixel becomes 255 when its value is strictly above the threshold, else 0.
//! * The threshold is computed by one shared formula from the exact integer sums, so the result
//!   is bit-identical to a naive per-pixel window scan, to any strip height, and at any thread
//!   count.

use rayon::prelude::*;

/// Output rows per rayon task (the strip). Any height gives the same result.
pub const STRIP_ROWS: usize = 256;

/// Sauvola's threshold from window statistics: `m * (1 + k * (s / r - 1))`, `s` the standard
/// deviation of the window, `r` the dynamic range of the deviation (128 for 8-bit).
pub fn sauvola_threshold(sum: u64, sumsq: u64, n: u64, k: f64, r: f64) -> f64 {
    let (m, s) = mean_and_std(sum, sumsq, n);
    m * (1.0 + k * (s / r - 1.0))
}

/// NICK threshold (Khurshid et al. 2009): `m + k * sqrt((sum(p^2) - m^2) / n)`, `k` about -0.1.
pub fn nick_threshold(sum: u64, sumsq: u64, n: u64, k: f64) -> f64 {
    let m = sum as f64 / n as f64;
    m + k * ((sumsq as f64 - m * m) / n as f64).max(0.0).sqrt()
}

/// Mean and (population) standard deviation of a window from its exact sums. The variance
/// numerator `n * sumsq - sum^2` is computed in `u128` so no window size can overflow it.
fn mean_and_std(sum: u64, sumsq: u64, n: u64) -> (f64, f64) {
    let num = u128::from(n) * u128::from(sumsq) - u128::from(sum) * u128::from(sum);
    let var = num as f64 / (n as f64 * n as f64);
    (sum as f64 / n as f64, var.max(0.0).sqrt())
}

/// Applies `threshold(sum, sumsq, n)` over every `(2 * radius + 1)` window and writes the 0 / 255
/// result. `gray` is `width * height` row-major.
fn local_binarize<F>(
    gray: &[u8],
    width: usize,
    height: usize,
    radius: usize,
    strip_rows: usize,
    threshold: &F,
) -> Vec<u8>
where
    F: Fn(u64, u64, u64) -> f64 + Sync,
{
    assert_eq!(gray.len(), width * height, "gray must be width * height");
    let mut out = vec![0u8; width * height];
    if width == 0 || height == 0 {
        return out;
    }
    let strip_rows = strip_rows.max(1);
    out.par_chunks_mut(width * strip_rows)
        .enumerate()
        .for_each(|(i, chunk)| {
            let y0 = i * strip_rows;
            let rows = chunk.len() / width;
            binarize_strip(gray, width, height, radius, y0, rows, chunk, threshold);
        });
    out
}

#[allow(clippy::too_many_arguments)]
fn binarize_strip<F>(
    gray: &[u8],
    w: usize,
    h: usize,
    r: usize,
    y0: usize,
    rows: usize,
    out: &mut [u8],
    threshold: &F,
) where
    F: Fn(u64, u64, u64) -> f64,
{
    let mut colsum = vec![0u64; w];
    let mut colsq = vec![0u64; w];
    let mut psum = vec![0u64; w + 1];
    let mut psq = vec![0u64; w + 1];
    // Window rows of the strip's first row, with the strip's halo.
    for y in y0.saturating_sub(r)..=(y0 + r).min(h - 1) {
        for (x, v) in gray[y * w..(y + 1) * w].iter().enumerate() {
            let v = u64::from(*v);
            colsum[x] += v;
            colsq[x] += v * v;
        }
    }
    for y in y0..y0 + rows {
        for x in 0..w {
            psum[x + 1] = psum[x] + colsum[x];
            psq[x + 1] = psq[x] + colsq[x];
        }
        let nr = ((y + r).min(h - 1) - y.saturating_sub(r) + 1) as u64;
        let line = &gray[y * w..(y + 1) * w];
        let orow = &mut out[(y - y0) * w..(y - y0 + 1) * w];
        for x in 0..w {
            let (lo, hi) = (x.saturating_sub(r), (x + r).min(w - 1));
            let n = nr * (hi - lo + 1) as u64;
            let (s, q) = (psum[hi + 1] - psum[lo], psq[hi + 1] - psq[lo]);
            orow[x] = if f64::from(line[x]) > threshold(s, q, n) {
                255
            } else {
                0
            };
        }
        // Slide the window down one row: the row at `y + r + 1` enters, the one at `y - r` leaves.
        if y + 1 < y0 + rows {
            if y + r + 1 < h {
                for (x, v) in gray[(y + r + 1) * w..(y + r + 2) * w].iter().enumerate() {
                    let v = u64::from(*v);
                    colsum[x] += v;
                    colsq[x] += v * v;
                }
            }
            if y >= r {
                for (x, v) in gray[(y - r) * w..(y - r + 1) * w].iter().enumerate() {
                    let v = u64::from(*v);
                    colsum[x] -= v;
                    colsq[x] -= v * v;
                }
            }
        }
    }
}

/// Sauvola binarisation with a square `window` (rounded down to odd: radius `window / 2`).
/// Typical document parameters: window 31 to 101, `k` 0.25, `r` 128.
pub fn sauvola(gray: &[u8], width: u32, height: u32, window: u32, k: f64, r: f64) -> Vec<u8> {
    sauvola_with_strips(gray, width, height, window, k, r, STRIP_ROWS)
}

/// [`sauvola`] with an explicit strip height (the result does not depend on it).
pub fn sauvola_with_strips(
    gray: &[u8],
    width: u32,
    height: u32,
    window: u32,
    k: f64,
    r: f64,
    strip_rows: usize,
) -> Vec<u8> {
    local_binarize(
        gray,
        width as usize,
        height as usize,
        (window / 2) as usize,
        strip_rows,
        &|s, q, n| sauvola_threshold(s, q, n, k, r),
    )
}

/// NICK binarisation with a square `window` and `k` about -0.1.
pub fn nick(gray: &[u8], width: u32, height: u32, window: u32, k: f64) -> Vec<u8> {
    nick_with_strips(gray, width, height, window, k, STRIP_ROWS)
}

/// [`nick`] with an explicit strip height.
pub fn nick_with_strips(
    gray: &[u8],
    width: u32,
    height: u32,
    window: u32,
    k: f64,
    strip_rows: usize,
) -> Vec<u8> {
    local_binarize(
        gray,
        width as usize,
        height as usize,
        (window / 2) as usize,
        strip_rows,
        &|s, q, n| nick_threshold(s, q, n, k),
    )
}

/// 256-bin histogram with `u64` counts (parallel, exact).
pub fn histogram(gray: &[u8]) -> [u64; 256] {
    gray.par_chunks(1 << 20)
        .fold(
            || [0u64; 256],
            |mut h, c| {
                for v in c {
                    h[usize::from(*v)] += 1;
                }
                h
            },
        )
        .reduce(
            || [0u64; 256],
            |mut a, b| {
                for (x, y) in a.iter_mut().zip(b) {
                    *x += y;
                }
                a
            },
        )
}

/// Otsu's threshold from a histogram: the `t` maximising the between-class variance of the
/// classes `<= t` and `> t`. Counts are `u64` and sums are exact up to 2^53, far beyond 100 MP.
/// A flat image (one populated bin) returns 0, so everything above black is foreground.
pub fn otsu_from_histogram(hist: &[u64; 256]) -> u8 {
    let total: u64 = hist.iter().sum();
    let sum_all: u64 = hist.iter().enumerate().map(|(i, c)| i as u64 * c).sum();
    let (mut w0, mut sum0) = (0u64, 0u64);
    let (mut best_t, mut best_var) = (0u8, -1.0f64);
    for (t, count) in hist.iter().enumerate().take(255) {
        w0 += count;
        sum0 += t as u64 * count;
        let w1 = total - w0;
        if w0 == 0 || w1 == 0 {
            continue;
        }
        let m0 = sum0 as f64 / w0 as f64;
        let m1 = (sum_all - sum0) as f64 / w1 as f64;
        let between = (w0 as f64) * (w1 as f64) * (m0 - m1) * (m0 - m1);
        if between > best_var {
            (best_t, best_var) = (t as u8, between);
        }
    }
    best_t
}

/// Otsu's global threshold of an 8-bit grey image.
pub fn otsu_threshold(gray: &[u8]) -> u8 {
    otsu_from_histogram(&histogram(gray))
}

/// Pixels strictly above `t` become 255, the rest 0.
pub fn binarize(gray: &[u8], t: u8) -> Vec<u8> {
    gray.par_iter()
        .map(|v| if *v > t { 255 } else { 0 })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(w: usize, h: usize, seed: u32) -> Vec<u8> {
        let mut s = seed | 1;
        (0..w * h)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                (s >> 8) as u8
            })
            .collect()
    }

    /// A page-like image: bright paper with a lighting gradient and dark text-like bars.
    fn page(w: usize, h: usize) -> Vec<u8> {
        let mut g = noise(w, h, 9);
        for (i, v) in g.iter_mut().enumerate() {
            let (x, y) = (i % w, i / w);
            let paper = 150 + (x * 70 / w) as i32 + (y * 20 / h) as i32;
            let ink = if (y / 7) % 3 == 0 && (x / 5) % 4 != 0 {
                -110
            } else {
                0
            };
            *v = (paper + ink + i32::from(*v % 9) - 4).clamp(0, 255) as u8;
        }
        g
    }

    /// Per-pixel window scan with plain integer sums: the oracle.
    fn naive<F: Fn(u64, u64, u64) -> f64>(g: &[u8], w: usize, h: usize, r: usize, f: F) -> Vec<u8> {
        let mut out = vec![0u8; w * h];
        for y in 0..h {
            for x in 0..w {
                let (mut s, mut q, mut n) = (0u64, 0u64, 0u64);
                for yy in y.saturating_sub(r)..=(y + r).min(h - 1) {
                    for xx in x.saturating_sub(r)..=(x + r).min(w - 1) {
                        let v = u64::from(g[yy * w + xx]);
                        s += v;
                        q += v * v;
                        n += 1;
                    }
                }
                out[y * w + x] = if f64::from(g[y * w + x]) > f(s, q, n) {
                    255
                } else {
                    0
                };
            }
        }
        out
    }

    #[test]
    fn sauvola_and_nick_equal_the_naive_scan_at_every_strip_height() {
        let (w, h) = (83, 61);
        for g in [noise(w, h, 3), page(w, h)] {
            for window in [1u32, 3, 9, 31, 51, 101, 400] {
                let r = (window / 2) as usize;
                let want_s = naive(&g, w, h, r, |s, q, n| {
                    sauvola_threshold(s, q, n, 0.25, 128.0)
                });
                let want_n = naive(&g, w, h, r, |s, q, n| nick_threshold(s, q, n, -0.1));
                for strip in [1usize, 2, 7, 64, 1000] {
                    assert_eq!(
                        sauvola_with_strips(&g, w as u32, h as u32, window, 0.25, 128.0, strip),
                        want_s,
                        "sauvola window {window} strip {strip}"
                    );
                    assert_eq!(
                        nick_with_strips(&g, w as u32, h as u32, window, -0.1, strip),
                        want_n,
                        "nick window {window} strip {strip}"
                    );
                }
            }
        }
    }

    #[test]
    fn same_bytes_at_1_and_8_threads() {
        let (w, h) = (300, 700);
        let g = page(w, h);
        let run = |n: usize| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(n)
                .build()
                .unwrap()
                .install(|| {
                    (
                        sauvola(&g, w as u32, h as u32, 51, 0.25, 128.0),
                        nick(&g, w as u32, h as u32, 51, -0.1),
                        otsu_threshold(&g),
                    )
                })
        };
        assert_eq!(run(1), run(8));
    }

    #[test]
    fn sauvola_separates_ink_from_uneven_paper() {
        let (w, h) = (400, 300);
        let g = page(w, h);
        let sv = sauvola(&g, w as u32, h as u32, 51, 0.25, 128.0);
        // Ink pixels (the bars) are black, paper white, across the whole lighting gradient.
        let (mut ink_black, mut ink, mut paper_white, mut paper) = (0u32, 0u32, 0u32, 0u32);
        for y in 0..h {
            for x in 0..w {
                let is_ink = (y / 7) % 3 == 0 && (x / 5) % 4 != 0;
                if is_ink {
                    ink += 1;
                    ink_black += u32::from(sv[y * w + x] == 0);
                } else {
                    paper += 1;
                    paper_white += u32::from(sv[y * w + x] == 255);
                }
            }
        }
        assert!(
            f64::from(ink_black) / f64::from(ink) > 0.95,
            "ink black {ink_black}/{ink}"
        );
        assert!(
            f64::from(paper_white) / f64::from(paper) > 0.95,
            "paper white {paper_white}/{paper}"
        );
    }

    #[test]
    fn otsu_finds_the_valley_of_a_two_class_histogram() {
        let mut hist = [0u64; 256];
        for (v, c) in [(40usize, 1000u64), (45, 900), (200, 5000), (210, 4000)] {
            hist[v] = c;
        }
        let t = otsu_from_histogram(&hist);
        assert!((45..200).contains(&usize::from(t)), "t = {t}");
        let g: Vec<u8> = (0..1000)
            .map(|i| if i % 3 == 0 { 30 } else { 220 })
            .collect();
        let t = otsu_threshold(&g);
        assert!((30..220).contains(&t));
        let b = binarize(&g, t);
        assert_eq!(b.iter().filter(|v| **v == 0).count(), 334);
        // Degenerate: a flat image has one class; everything above black is foreground.
        assert_eq!(otsu_threshold(&[77u8; 50]), 0);
        assert_eq!(otsu_threshold(&[]), 0);
    }

    #[test]
    fn counts_far_above_u32_do_not_overflow() {
        // 2 x 10^10 pixels in two clusters: far more than u32::MAX counts and sums.
        let mut big = [0u64; 256];
        big[50] = 6_000_000_000;
        big[210] = 14_000_000_000;
        let mut small = [0u64; 256];
        small[50] = 6;
        small[210] = 14;
        assert_eq!(otsu_from_histogram(&big), otsu_from_histogram(&small));
        assert!(u64::from(u32::MAX) < 20_000_000_000);
        // The window sum of squares of a 12 MP window of 255s needs 41 bits: past u32.
        assert!(12_000_000u64 * 255 * 255 > u64::from(u32::MAX));
    }

    #[test]
    fn a_constant_255_image_beyond_the_u32_limits_stays_white() {
        // 4400 x 4400 = 19.4 MP of 255, window 4001: each window sums to over 10^9 and its
        // squares to about 10^12, both beyond u32 (the squares by a factor of 200).
        let (w, h) = (4400usize, 4400usize);
        let g = vec![255u8; w * h];
        let window = 4001u32;
        let sv = sauvola(&g, w as u32, h as u32, window, 0.25, 128.0);
        assert!(
            sv.iter().all(|v| *v == 255),
            "Sauvola must keep a white page white"
        );
        let nk = nick(&g, w as u32, h as u32, window, -0.1);
        assert!(
            nk.iter().all(|v| *v == 255),
            "NICK must keep a white page white"
        );
        // One window's exact statistics, to show the sums really passed u32.
        let n = (window as u64) * (window as u64);
        assert!(n * 255 * 255 > u64::from(u32::MAX) * 100);
        assert_eq!(otsu_threshold(&g), 0);
        assert!(binarize(&g, 0).iter().all(|v| *v == 255));
    }

    #[test]
    fn thresholds_follow_the_published_formulas() {
        // Constant window of value 100: s = 0, so T = m (1 - k).
        let (n, v) = (25u64, 100u64);
        let t = sauvola_threshold(n * v, n * v * v, n, 0.25, 128.0);
        assert!((t - 75.0).abs() < 1e-9);
        // NICK: m + k sqrt((sumsq - m^2) / n).
        let t = nick_threshold(n * v, n * v * v, n, -0.1);
        let m = 100.0;
        assert!(
            (t - (m - 0.1 * ((n * v * v) as f64 - m * m).max(0.0).sqrt() / (n as f64).sqrt()))
                .abs()
                < 1e-9
        );
    }
}
