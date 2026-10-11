// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! PROTOTYPE (cargo feature `learned`, off by default): the pre- and post-processing of the
//! learned page-corner net of ADR 0010, in Rust, so its cost and its agreement with the Python
//! decoder (`tools/train/decode.py`) can be measured. The shipping detector stays the classical one
//! until the owner decides.
//!
//! * [`letterbox_rgb8`]: the picture scaled so its long edge is `size`, centred on a grey square
//!   (114), as an NCHW `f32` tensor in `-1..1` (`x / 127.5 - 1`). Shrinking uses an exact area
//!   average, enlarging bilinear interpolation (OpenCV's `INTER_AREA` pre-shrink plus a bilinear
//!   warp in the trainer, so values differ by rounding, not by geometry).
//! * [`decode`]: the net's raw output (11 channels at `size / stride`: corner heatmap, page mask,
//!   centre heatmap, eight centre-to-corner offsets in units of 32 px) to a quad: the centre peak
//!   gives four regressed corners, each snapped to the nearest corner-heatmap peak (3x3 weighted
//!   centroid) when one is close enough.
//! * [`LearnedDetector`] runs a net through any [`crate::InferenceBackend`].
//!
//! Pure Rust, no `unsafe`, no dependency beyond the backends the caller picks.

use crate::{InferError, InferenceBackend};

/// Offsets are stored as pixels / 32 of the network input.
pub const OFFSET_UNIT: f64 = 32.0;
/// Grey of the letterbox padding.
pub const PAD_GREY: u8 = 114;

/// How a picture sits on the square canvas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Letterbox {
    pub size: usize,
    /// Canvas pixels per source pixel.
    pub scale: f64,
    /// Canvas position of the source's top-left corner.
    pub dx: f64,
    pub dy: f64,
}

impl Letterbox {
    pub fn new(w: usize, h: usize, size: usize) -> Self {
        let scale = size as f64 / w.max(h).max(1) as f64;
        Self {
            size,
            scale,
            dx: (size as f64 - scale * w as f64) / 2.0,
            dy: (size as f64 - scale * h as f64) / 2.0,
        }
    }

    /// A canvas point in source pixels.
    pub fn to_source(&self, p: [f64; 2]) -> [f64; 2] {
        [(p[0] - self.dx) / self.scale, (p[1] - self.dy) / self.scale]
    }
}

/// One axis of a separable resample: for every output index the first source index and the weights.
fn axis_weights(src: usize, dst: usize) -> Vec<(usize, Vec<f32>)> {
    let ratio = src as f64 / dst as f64;
    (0..dst)
        .map(|o| {
            if ratio > 1.0 {
                // Exact area average over [o * ratio, (o + 1) * ratio).
                let (a, b) = (o as f64 * ratio, (o + 1) as f64 * ratio);
                let first = a.floor() as usize;
                let last = (b.ceil() as usize).min(src);
                let w: Vec<f32> = (first..last)
                    .map(|i| ((b.min((i + 1) as f64) - a.max(i as f64)) / ratio) as f32)
                    .collect();
                (first, w)
            } else {
                // Bilinear at the pixel centre.
                let c = (o as f64 + 0.5) * ratio - 0.5;
                let i0 = c.floor();
                let t = (c - i0) as f32;
                let (i0c, i1c) = (
                    (i0.max(0.0) as usize).min(src - 1),
                    ((i0 + 1.0).max(0.0) as usize).min(src - 1),
                );
                if i0c == i1c {
                    (i0c, vec![1.0])
                } else {
                    (i0c, vec![1.0 - t, t])
                }
            }
        })
        .collect()
}

/// The picture (`rgb`: `w * h * 3` bytes, row-major) letterboxed to `size` x `size`, as an NCHW
/// tensor (`3 * size * size` values) in `-1..1`, and the placement.
pub fn letterbox_rgb8(rgb: &[u8], w: usize, h: usize, size: usize) -> (Vec<f32>, Letterbox) {
    assert_eq!(rgb.len(), w * h * 3, "rgb must hold w * h * 3 bytes");
    let lb = Letterbox::new(w, h, size);
    let (nw, nh) = (
        ((w as f64 * lb.scale).round() as usize).clamp(1, size),
        ((h as f64 * lb.scale).round() as usize).clamp(1, size),
    );
    let (xs, ys) = (axis_weights(w, nw), axis_weights(h, nh));
    // Horizontal pass into a (h x nw x 3) f32 buffer, then vertical.
    let mut tmp = vec![0f32; h * nw * 3];
    for y in 0..h {
        let row = &rgb[y * w * 3..(y + 1) * w * 3];
        for (ox, (first, wts)) in xs.iter().enumerate() {
            let mut acc = [0f32; 3];
            for (k, wt) in wts.iter().enumerate() {
                let p = (first + k) * 3;
                acc[0] += wt * f32::from(row[p]);
                acc[1] += wt * f32::from(row[p + 1]);
                acc[2] += wt * f32::from(row[p + 2]);
            }
            tmp[(y * nw + ox) * 3..(y * nw + ox) * 3 + 3].copy_from_slice(&acc);
        }
    }
    let plane = size * size;
    let pad = f32::from(PAD_GREY) / 127.5 - 1.0;
    let mut out = vec![pad; 3 * plane];
    let (ox0, oy0) = (
        ((size - nw) as f64 / 2.0).round() as usize,
        ((size - nh) as f64 / 2.0).round() as usize,
    );
    for (oy, (first, wts)) in ys.iter().enumerate() {
        for ox in 0..nw {
            let mut acc = [0f32; 3];
            for (k, wt) in wts.iter().enumerate() {
                let p = ((first + k) * nw + ox) * 3;
                acc[0] += wt * tmp[p];
                acc[1] += wt * tmp[p + 1];
                acc[2] += wt * tmp[p + 2];
            }
            let at = (oy0 + oy) * size + ox0 + ox;
            for c in 0..3 {
                out[c * plane + at] = acc[c].round().clamp(0.0, 255.0) / 127.5 - 1.0;
            }
        }
    }
    (out, lb)
}

/// A decoded quad in canvas pixels (clockwise, starting at the corner nearest up-left) and the
/// features that a confidence can be built from.
#[derive(Debug, Clone, PartialEq)]
pub struct Decoded {
    pub quad: [[f64; 2]; 4],
    /// Smallest and mean heatmap value over the four snapped corners (0 for an unmatched corner).
    pub min_peak: f64,
    pub mean_peak: f64,
    /// Corners that found a heatmap peak.
    pub matched: usize,
    /// Value of the page-centre peak.
    pub centre_peak: f64,
}

fn sigmoid(x: f32) -> f64 {
    1.0 / (1.0 + (-f64::from(x)).exp())
}

fn signed_area(q: &[[f64; 2]; 4]) -> f64 {
    let mut s = 0.0;
    for i in 0..4 {
        let (a, b) = (q[i], q[(i + 1) % 4]);
        s += a[0] * b[1] - a[1] * b[0];
    }
    s / 2.0
}

fn is_convex(q: &[[f64; 2]; 4]) -> bool {
    let mut sign = 0i8;
    for i in 0..4 {
        let (a, b, c) = (q[i], q[(i + 1) % 4], q[(i + 2) % 4]);
        let cr = (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]);
        let s = if cr > 0.0 {
            1
        } else if cr < 0.0 {
            -1
        } else {
            return false;
        };
        if sign != 0 && s != sign {
            return false;
        }
        sign = s;
    }
    true
}

/// Clockwise on screen (y down), starting at the corner nearest the up-left direction.
fn canonical(q: &[[f64; 2]; 4]) -> [[f64; 2]; 4] {
    let c = [
        q.iter().map(|p| p[0]).sum::<f64>() / 4.0,
        q.iter().map(|p| p[1]).sum::<f64>() / 4.0,
    ];
    let mut v: Vec<(f64, [f64; 2])> = q
        .iter()
        .map(|p| ((p[1] - c[1]).atan2(p[0] - c[0]), *p))
        .collect();
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    let tau = std::f64::consts::TAU;
    let pi = std::f64::consts::PI;
    let dist = |ang: f64| ((ang + 3.0 * pi / 4.0 + pi).rem_euclid(tau) - pi).abs();
    let k = (0..4)
        .min_by(|&i, &j| dist(v[i].0).total_cmp(&dist(v[j].0)))
        .unwrap_or(0);
    [v[k].1, v[(k + 1) % 4].1, v[(k + 2) % 4].1, v[(k + 3) % 4].1]
}

/// Local maxima of the corner heatmap above 0.05 (3x3 neighbourhood, ties count), strongest first,
/// at most 24, each as a 3x3 weighted centroid `(x, y, value)` in cells.
fn peaks(heat: &[f64], s: usize) -> Vec<(f64, f64, f64)> {
    let at = |y: isize, x: isize| -> f64 {
        if y < 0 || x < 0 || y >= s as isize || x >= s as isize {
            0.0
        } else {
            heat[y as usize * s + x as usize]
        }
    };
    let mut found: Vec<(usize, usize, f64)> = Vec::new();
    for y in 0..s {
        for x in 0..s {
            let v = heat[y * s + x];
            if v <= 0.05 {
                continue;
            }
            let mut mx = 0.0f64;
            for dy in -1..=1isize {
                for dx in -1..=1isize {
                    mx = mx.max(at(y as isize + dy, x as isize + dx));
                }
            }
            if v >= mx {
                found.push((y, x, v));
            }
        }
    }
    found.sort_by(|a, b| b.2.total_cmp(&a.2));
    found.truncate(24);
    found
        .into_iter()
        .map(|(y, x, v)| {
            let (mut sw, mut sx, mut sy) = (0.0, 0.0, 0.0);
            for yy in y.saturating_sub(1)..(y + 2).min(s) {
                for xx in x.saturating_sub(1)..(x + 2).min(s) {
                    let w = heat[yy * s + xx];
                    sw += w;
                    sx += w * xx as f64;
                    sy += w * yy as f64;
                }
            }
            (sx / sw, sy / sw, v)
        })
        .collect()
}

/// Decodes the raw output (`11 * s * s` values, channel-major) of a net with input `size` and the
/// given `stride` (`s = size / stride`) into a quad. `None` when no convex quad of a usable area
/// can be made.
pub fn decode(out: &[f32], size: usize, stride: usize) -> Option<Decoded> {
    let s = size / stride;
    if out.len() != 11 * s * s {
        return None;
    }
    let plane = s * s;
    let heat: Vec<f64> = out[..plane].iter().map(|&v| sigmoid(v)).collect();
    let ctr: Vec<f64> = out[2 * plane..3 * plane]
        .iter()
        .map(|&v| sigmoid(v))
        .collect();
    let (mut ci, mut cv) = (0usize, f64::MIN);
    for (i, &v) in ctr.iter().enumerate() {
        if v > cv {
            (ci, cv) = (i, v);
        }
    }
    let (cy, cx) = (ci / s, ci % s);
    let st = stride as f64;
    let cell = [(cx as f64 + 0.5) * st, (cy as f64 + 0.5) * st];
    let mut reg = [[0.0f64; 2]; 4];
    for (k, r) in reg.iter_mut().enumerate() {
        r[0] = cell[0] + f64::from(out[(3 + 2 * k) * plane + ci]) * OFFSET_UNIT;
        r[1] = cell[1] + f64::from(out[(4 + 2 * k) * plane + ci]) * OFFSET_UNIT;
    }
    let pk = peaks(&heat, s);
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    for r in &reg {
        for a in 0..2 {
            lo[a] = lo[a].min(r[a]);
            hi[a] = hi[a].max(r[a]);
        }
    }
    let diag = (hi[0] - lo[0]).hypot(hi[1] - lo[1]) + 1e-6;
    let radius = (3.0 * st).max(0.12 * diag);
    let mut used = vec![false; pk.len()];
    let mut quad = reg;
    let mut scores = [0.0f64; 4];
    for k in 0..4 {
        let mut best: Option<(f64, usize, [f64; 2], f64)> = None;
        for (i, &(px, py, sc)) in pk.iter().enumerate() {
            if used[i] || sc < 0.1 {
                continue;
            }
            let pos = [(px + 0.5) * st, (py + 0.5) * st];
            let d = (pos[0] - reg[k][0]).hypot(pos[1] - reg[k][1]);
            if d <= radius {
                let cost = d / radius - 0.5 * sc;
                if best.is_none_or(|b| cost < b.0) {
                    best = Some((cost, i, pos, sc));
                }
            }
        }
        if let Some((_, i, pos, sc)) = best {
            used[i] = true;
            quad[k] = pos;
            scores[k] = sc;
        }
    }
    let mut matched = scores.iter().filter(|&&s| s > 0.0).count();
    if !is_convex(&quad) || signed_area(&quad).abs() < 4.0 {
        if is_convex(&reg) && signed_area(&reg).abs() >= 4.0 {
            quad = reg;
            scores = [0.0; 4];
            matched = 0;
        } else {
            return None;
        }
    }
    Some(Decoded {
        quad: canonical(&quad),
        min_peak: scores.iter().copied().fold(f64::MAX, f64::min),
        mean_peak: scores.iter().sum::<f64>() / 4.0,
        matched,
        centre_peak: cv,
    })
}

/// A learned corner net on a backend: letterbox, run, decode, back to source pixels.
pub struct LearnedDetector {
    backend: Box<dyn InferenceBackend>,
    size: usize,
    stride: usize,
}

/// A quad in source pixels (same order as [`Decoded::quad`]) and the decoder's features.
#[derive(Debug, Clone, PartialEq)]
pub struct Detection {
    pub quad: [[f64; 2]; 4],
    pub decoded: Decoded,
}

impl LearnedDetector {
    pub fn new(backend: Box<dyn InferenceBackend>, size: usize, stride: usize) -> Self {
        Self {
            backend,
            size,
            stride,
        }
    }

    /// `rgb`: `w * h * 3` bytes. `Ok(None)` when the net found no usable quad.
    pub fn detect(
        &mut self,
        rgb: &[u8],
        w: usize,
        h: usize,
    ) -> Result<Option<Detection>, InferError> {
        let (x, lb) = letterbox_rgb8(rgb, w, h, self.size);
        let out = self.backend.run(&x, [1, 3, self.size, self.size])?;
        Ok(
            decode(&out.data, self.size, self.stride).map(|d| Detection {
                quad: d.quad.map(|p| lb.to_source(p)),
                decoded: d,
            }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A raw output with a Gaussian-ish peak per corner, a centre peak and exact offsets.
    fn synthetic(size: usize, stride: usize, quad: &[[f64; 2]; 4], jitter: f64) -> Vec<f32> {
        let s = size / stride;
        let plane = s * s;
        let mut out = vec![-8.0f32; 11 * plane];
        let st = stride as f64;
        for c in quad {
            let (cx, cy) = (
                (c[0] / st - 0.5).round() as isize,
                (c[1] / st - 0.5).round() as isize,
            );
            for (dy, dx, v) in [
                (0, 0, 6.0f32),
                (0, 1, 1.0),
                (0, -1, 1.0),
                (1, 0, 1.0),
                (-1, 0, 1.0),
            ] {
                let (y, x) = (cy + dy, cx + dx);
                if (0..s as isize).contains(&y) && (0..s as isize).contains(&x) {
                    out[y as usize * s + x as usize] = v;
                }
            }
        }
        let centre = [
            quad.iter().map(|p| p[0]).sum::<f64>() / 4.0,
            quad.iter().map(|p| p[1]).sum::<f64>() / 4.0,
        ];
        let (ccx, ccy) = (
            (centre[0] / st - 0.5).round() as usize,
            (centre[1] / st - 0.5).round() as usize,
        );
        out[2 * plane + ccy * s + ccx] = 6.0;
        let cell = [(ccx as f64 + 0.5) * st, (ccy as f64 + 0.5) * st];
        for (k, p) in quad.iter().enumerate() {
            out[(3 + 2 * k) * plane + ccy * s + ccx] =
                ((p[0] - cell[0] + jitter) / OFFSET_UNIT) as f32;
            out[(4 + 2 * k) * plane + ccy * s + ccx] =
                ((p[1] - cell[1] + jitter) / OFFSET_UNIT) as f32;
        }
        out
    }

    const Q: [[f64; 2]; 4] = [[60.0, 50.0], [200.0, 70.0], [190.0, 220.0], [50.0, 200.0]];

    fn near(a: [f64; 2], b: [f64; 2], tol: f64) -> bool {
        (a[0] - b[0]).hypot(a[1] - b[1]) <= tol
    }

    #[test]
    fn decodes_the_peaks_of_a_clean_output_and_snaps_to_them() {
        // Regression offsets are off by 3 px; the heatmap peaks snap the corners back.
        let d = decode(&synthetic(256, 4, &Q, 3.0), 256, 4).expect("a quad");
        assert_eq!(d.matched, 4);
        for (got, want) in d.quad.iter().zip(Q) {
            assert!(near(*got, want, 2.5), "{got:?} vs {want:?}");
        }
        assert!(d.min_peak > 0.9 && d.centre_peak > 0.99);
    }

    #[test]
    fn a_corner_without_a_peak_keeps_its_regressed_position() {
        let mut o = synthetic(256, 4, &Q, 0.0);
        let s = 64;
        // Erase the heatmap around corner 2 (an occluded or out-of-frame corner).
        for v in o[..s * s].iter_mut() {
            *v = -8.0;
        }
        let d = decode(&o, 256, 4).expect("a quad");
        assert_eq!(d.matched, 0);
        assert!(d.quad.iter().zip(Q).all(|(g, w)| near(*g, w, 1e-3)));
    }

    #[test]
    fn the_order_is_clockwise_from_the_up_left_corner_for_any_input_order() {
        let c = canonical(&[Q[2], Q[0], Q[3], Q[1]]);
        assert_eq!(c, Q);
        assert!(signed_area(&c) > 0.0, "clockwise on screen with y down");
    }

    #[test]
    fn wrong_sizes_and_degenerate_outputs_decode_to_nothing() {
        assert!(decode(&[0.0; 10], 256, 4).is_none());
        assert!(decode(&vec![-8.0; 11 * 64 * 64], 256, 4).is_none());
    }

    #[test]
    fn letterbox_centres_the_picture_and_maps_canvas_points_back() {
        let (w, h) = (40usize, 20usize);
        let rgb: Vec<u8> = (0..w * h).flat_map(|_| [200u8, 100, 50]).collect();
        let (x, lb) = letterbox_rgb8(&rgb, w, h, 64);
        assert_eq!(x.len(), 3 * 64 * 64);
        assert!((lb.scale - 1.6).abs() < 1e-12 && lb.dx == 0.0 && (lb.dy - 16.0).abs() < 1e-9);
        // Padding is grey, the picture keeps its colour (flat input stays flat through the resample).
        let pad = f32::from(PAD_GREY) / 127.5 - 1.0;
        assert!((x[0] - pad).abs() < 1e-6);
        let mid = 32 * 64 + 32;
        assert!((x[mid] - (200.0 / 127.5 - 1.0)).abs() < 1e-6);
        assert!((x[64 * 64 + mid] - (100.0 / 127.5 - 1.0)).abs() < 1e-6);
        assert!(near(lb.to_source([32.0, 32.0]), [20.0, 10.0], 1e-9));
    }

    #[test]
    fn downscaling_averages_the_area() {
        // 4x4 checkerboard of 0 and 255 into 2x2 canvas pixels: every output is the mean.
        let rgb: Vec<u8> = (0..16)
            .flat_map(|i| {
                let v = if (i / 4 + i % 4) % 2 == 0 { 0u8 } else { 255 };
                [v, v, v]
            })
            .collect();
        let (x, _) = letterbox_rgb8(&rgb, 4, 4, 2);
        for v in x {
            assert!((v - (127.5f32.round() / 127.5 - 1.0)).abs() < 1e-6, "{v}");
        }
    }
}
