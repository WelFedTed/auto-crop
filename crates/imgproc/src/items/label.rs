// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Binary-mask kernels: morphology, connected components, Euclidean distance transform and a
//! marker watershed.

use std::collections::BinaryHeap;

/// Dilation (`dilate = true`) or erosion by a `(2r+1)` square, as two separable passes.
/// Outside the frame counts as the value that does not change the result.
#[allow(clippy::needless_range_loop)]
pub fn morph(m: &[bool], w: usize, h: usize, r: usize, dilate: bool) -> Vec<bool> {
    if r == 0 {
        return m.to_vec();
    }
    let mut rows = vec![false; w * h];
    for y in 0..h {
        let src = &m[y * w..(y + 1) * w];
        let dst = &mut rows[y * w..(y + 1) * w];
        // Running window count of set pixels.
        let mut count = 0usize;
        let win = |x: usize| (x.saturating_sub(r), (x + r).min(w - 1));
        let (a0, a1) = win(0);
        for v in &src[a0..=a1] {
            count += usize::from(*v);
        }
        for x in 0..w {
            let (lo, hi) = win(x);
            let full = if dilate {
                count > 0
            } else {
                count == hi - lo + 1
            };
            dst[x] = full;
            // slide
            if x + 1 < w {
                let (nlo, nhi) = win(x + 1);
                if nlo > lo {
                    count -= usize::from(src[lo]);
                }
                if nhi > hi {
                    count += usize::from(src[nhi]);
                }
            }
        }
    }
    let mut out = vec![false; w * h];
    for x in 0..w {
        let mut count = 0usize;
        let win = |y: usize| (y.saturating_sub(r), (y + r).min(h - 1));
        let (a0, a1) = win(0);
        for yy in a0..=a1 {
            count += usize::from(rows[yy * w + x]);
        }
        for y in 0..h {
            let (lo, hi) = win(y);
            out[y * w + x] = if dilate {
                count > 0
            } else {
                count == hi - lo + 1
            };
            if y + 1 < h {
                let (nlo, nhi) = win(y + 1);
                if nlo > lo {
                    count -= usize::from(rows[lo * w + x]);
                }
                if nhi > hi {
                    count += usize::from(rows[nhi * w + x]);
                }
            }
        }
    }
    out
}

pub fn open(m: &[bool], w: usize, h: usize, r: usize) -> Vec<bool> {
    morph(&morph(m, w, h, r, false), w, h, r, true)
}

#[derive(Debug, Clone)]
pub struct Comp {
    pub id: u32,
    pub area: usize,
    pub x0: usize,
    pub y0: usize,
    pub x1: usize,
    pub y1: usize,
    /// Pixels on the first row, last column, last row and first column of the frame (top, right,
    /// bottom, left).
    pub border: [usize; 4],
}

/// 8-connected labelling. Label 0 is background; components are numbered from 1 in scan order.
pub fn label8(m: &[bool], w: usize, h: usize) -> (Vec<u32>, Vec<Comp>) {
    let mut lab = vec![0u32; w * h];
    let mut comps: Vec<Comp> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    for start in 0..w * h {
        if !m[start] || lab[start] != 0 {
            continue;
        }
        let id = comps.len() as u32 + 1;
        let mut c = Comp {
            id,
            area: 0,
            x0: usize::MAX,
            y0: usize::MAX,
            x1: 0,
            y1: 0,
            border: [0; 4],
        };
        lab[start] = id;
        stack.push(start);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            c.area += 1;
            c.x0 = c.x0.min(x);
            c.x1 = c.x1.max(x);
            c.y0 = c.y0.min(y);
            c.y1 = c.y1.max(y);
            c.border[0] += usize::from(y == 0);
            c.border[1] += usize::from(x + 1 == w);
            c.border[2] += usize::from(y + 1 == h);
            c.border[3] += usize::from(x == 0);
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                        continue;
                    }
                    let j = ny as usize * w + nx as usize;
                    if m[j] && lab[j] == 0 {
                        lab[j] = id;
                        stack.push(j);
                    }
                }
            }
        }
        comps.push(c);
    }
    (lab, comps)
}

/// Fills background holes that do not touch the frame (4-connected background).
pub fn fill_holes(m: &[bool], w: usize, h: usize) -> Vec<bool> {
    let mut outside = vec![false; w * h];
    let mut stack: Vec<usize> = Vec::new();
    let seed = |i: usize, outside: &mut Vec<bool>, stack: &mut Vec<usize>| {
        if !m[i] && !outside[i] {
            outside[i] = true;
            stack.push(i);
        }
    };
    for x in 0..w {
        seed(x, &mut outside, &mut stack);
        seed((h - 1) * w + x, &mut outside, &mut stack);
    }
    for y in 0..h {
        seed(y * w, &mut outside, &mut stack);
        seed(y * w + w - 1, &mut outside, &mut stack);
    }
    while let Some(i) = stack.pop() {
        let (x, y) = (i % w, i / w);
        let mut push = |j: usize| {
            if !m[j] && !outside[j] {
                outside[j] = true;
                stack.push(j);
            }
        };
        if x > 0 {
            push(i - 1);
        }
        if x + 1 < w {
            push(i + 1);
        }
        if y > 0 {
            push(i - w);
        }
        if y + 1 < h {
            push(i + w);
        }
    }
    (0..w * h).map(|i| m[i] || !outside[i]).collect()
}

fn edt_1d(f: &[f32], d: &mut [f32], v: &mut [usize], z: &mut [f32]) {
    let n = f.len();
    let mut k = 0usize;
    v[0] = 0;
    z[0] = f32::NEG_INFINITY;
    z[1] = f32::INFINITY;
    for q in 1..n {
        loop {
            let p = v[k];
            let s =
                ((f[q] + (q * q) as f32) - (f[p] + (p * p) as f32)) / (2.0 * (q as f32 - p as f32));
            if s <= z[k] {
                // cannot underflow: z[0] is -inf
                k -= 1;
                continue;
            }
            k += 1;
            v[k] = q;
            z[k] = s;
            z[k + 1] = f32::INFINITY;
            break;
        }
    }
    k = 0;
    for (q, dq) in d.iter_mut().enumerate() {
        while z[k + 1] < q as f32 {
            k += 1;
        }
        let dx = q as f32 - v[k] as f32;
        *dq = dx * dx + f[v[k]];
    }
}

/// Euclidean distance (in pixels) from every set pixel to the nearest unset pixel or frame edge
/// (the frame counts as background only where `frame_is_bg`).
pub fn edt(m: &[bool], w: usize, h: usize) -> Vec<f32> {
    const INF: f32 = 1e12;
    let mut g: Vec<f32> = m.iter().map(|&v| if v { INF } else { 0.0 }).collect();
    let n = w.max(h);
    let (mut v, mut z) = (vec![0usize; n], vec![0.0f32; n + 1]);
    let mut col = vec![0.0f32; h];
    let mut outc = vec![0.0f32; h];
    for x in 0..w {
        for y in 0..h {
            col[y] = g[y * w + x];
        }
        edt_1d(&col, &mut outc, &mut v, &mut z);
        for y in 0..h {
            g[y * w + x] = outc[y];
        }
    }
    let mut outr = vec![0.0f32; w];
    for y in 0..h {
        let row = g[y * w..(y + 1) * w].to_vec();
        edt_1d(&row, &mut outr, &mut v, &mut z);
        g[y * w..(y + 1) * w].copy_from_slice(&outr);
    }
    g.iter().map(|v| v.min(INF).sqrt()).collect()
}

/// Marker-controlled watershed on a distance map: floods from the markers downhill, so regions
/// meet along the saddle (the neck) between two blobs. `markers` holds 0 or a region id.
#[allow(clippy::needless_range_loop)]
pub fn watershed(dt: &[f32], mask: &[bool], markers: &[u32], w: usize, h: usize) -> Vec<u32> {
    let mut lab = markers.to_vec();
    let mut heap: BinaryHeap<(u32, usize)> = BinaryHeap::new();
    let key = |i: usize| (dt[i].max(0.0) * 64.0) as u32;
    for i in 0..w * h {
        if lab[i] != 0 {
            heap.push((key(i), i));
        }
    }
    while let Some((_, i)) = heap.pop() {
        let (x, y) = (i % w, i / w);
        let l = lab[i];
        for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
            let (nx, ny) = (x as i32 + dx, y as i32 + dy);
            if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                continue;
            }
            let j = ny as usize * w + nx as usize;
            if mask[j] && lab[j] == 0 {
                lab[j] = l;
                heap.push((key(j), j));
            }
        }
    }
    lab
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect_mask(w: usize, h: usize, x0: usize, y0: usize, x1: usize, y1: usize) -> Vec<bool> {
        (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                x >= x0 && x < x1 && y >= y0 && y < y1
            })
            .collect()
    }

    #[test]
    fn erosion_and_dilation_by_a_square() {
        let m = rect_mask(30, 20, 5, 5, 15, 12);
        let e = morph(&m, 30, 20, 2, false);
        assert_eq!(e.iter().filter(|v| **v).count(), 6 * 3);
        let d = morph(&m, 30, 20, 2, true);
        assert_eq!(d.iter().filter(|v| **v).count(), 14 * 11);
        assert_eq!(open(&m, 30, 20, 2), m);
    }

    #[test]
    fn labelling_counts_components_and_border_contact() {
        let mut m = rect_mask(30, 20, 0, 0, 5, 5);
        for (i, v) in rect_mask(30, 20, 10, 10, 15, 14).iter().enumerate() {
            m[i] |= *v;
        }
        let (lab, comps) = label8(&m, 30, 20);
        assert_eq!(comps.len(), 2);
        assert_eq!(comps[0].area, 25);
        assert!(comps[0].border.iter().sum::<usize>() > 0 && comps[1].border == [0; 4]);
        assert_eq!(lab[0], 1);
        assert_eq!(lab[10 * 30 + 10], 2);
    }

    #[test]
    fn holes_are_filled_unless_they_reach_the_frame() {
        let mut m = rect_mask(20, 20, 2, 2, 18, 18);
        for y in 8..12 {
            for x in 8..12 {
                m[y * 20 + x] = false;
            }
        }
        let f = fill_holes(&m, 20, 20);
        assert!(f[10 * 20 + 10]);
        assert!(!f[0]);
    }

    #[test]
    fn distance_transform_is_euclidean() {
        let m = rect_mask(21, 21, 5, 5, 16, 16);
        let d = edt(&m, 21, 21);
        assert!((d[10 * 21 + 10] - 6.0).abs() < 1e-4);
        assert!((d[5 * 21 + 5] - 1.0).abs() < 1e-4);
        assert_eq!(d[0], 0.0);
    }

    #[test]
    fn watershed_splits_two_blobs_at_the_neck() {
        let (w, h) = (60, 24);
        let mut m = rect_mask(w, h, 2, 2, 28, 22);
        for (i, v) in rect_mask(w, h, 32, 2, 58, 22).iter().enumerate() {
            m[i] |= *v;
        }
        for y in 9..15 {
            for x in 28..32 {
                m[y * w + x] = true;
            }
        }
        let dt = edt(&m, w, h);
        let mut markers = vec![0u32; w * h];
        markers[12 * w + 15] = 1;
        markers[12 * w + 45] = 2;
        let l = watershed(&dt, &m, &markers, w, h);
        assert_eq!(l[5 * w + 5], 1);
        assert_eq!(l[20 * w + 55], 2);
        let boundary = (0..w).find(|&x| l[12 * w + x] == 2).expect("second region");
        assert!((28..=32).contains(&boundary), "{boundary}");
    }
}
