// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Text scale (PLAN 5.3.1 step 2): the median height of the dominant ink-component size on the
//! analysis proxy. Every length of the enhancer (illumination block, threshold window) is a
//! multiple of it, so the same page gives the same result whatever its resolution.
//!
//! The ink mask is local: a pixel is ink when it is clearly darker than the mean of the
//! `(2 * RADIUS + 1)` square around it (an integral image, exact `u32` sums), so a shadow does not
//! move it. Connected components are found with one pass over horizontal runs and a union-find.

use rayon::prelude::*;

/// Radius of the local-mean window, in proxy pixels.
const RADIUS: usize = 24;
/// The sensitive mask: a pixel is ink below this share of its local mean ...
const INK_RATIO: f32 = 0.90;
/// ... and at least this many levels below it (noise on flat paper never qualifies).
const INK_MIN_DROP: f32 = 15.0;
/// Fewer qualifying components than this and the estimate falls back to `short side / 40`.
const MIN_COMPONENTS: usize = 30;

/// The text height found on the proxy.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextScale {
    /// Median component height of the dominant size, in proxy pixels (4 to 60).
    pub height: f32,
    /// Components that took part (0 when the fallback was used).
    pub components: u32,
    /// False when too few components were found and `short side / 40` was used.
    pub confident: bool,
    /// Share of the plausible glyph-sized components that sit near the dominant size. Print is
    /// regular (high); a photograph's texture is not.
    pub concentration: f32,
    /// Share of the ink pixels that belong to those components.
    pub glyph_ink_share: f32,
}

/// What `pick` found.
struct Pick {
    height: f32,
    near: usize,
    plausible: usize,
    near_area: u64,
}

/// One connected component's bounding box and area.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Comp {
    pub x0: u32,
    pub x1: u32,
    pub y0: u32,
    pub y1: u32,
    pub area: u32,
}

impl Comp {
    pub fn w(&self) -> u32 {
        self.x1 - self.x0 + 1
    }
    pub fn h(&self) -> u32 {
        self.y1 - self.y0 + 1
    }
}

/// Estimates the text height of a luma plane (`valid`, when given, marks the page: 0 = outside).
pub fn text_scale(luma: &[u8], w: usize, h: usize, valid: Option<&[u8]>) -> TextScale {
    let fallback = TextScale {
        height: (w.min(h) as f32 / 40.0).clamp(4.0, 60.0),
        components: 0,
        confident: false,
        concentration: 0.0,
        glyph_ink_share: 0.0,
    };
    if w < 16 || h < 16 {
        return fallback;
    }
    // Strict mask first (glyph heights are not inflated by the anti-aliased halo). Faint print
    // fragments under it and most of its ink is then in pieces, not glyphs, so a more sensitive
    // mask is tried unless the strict one already explains the ink; it wins only when clearly
    // more of the ink sits in glyph-sized components.
    let integral = integral_of(luma, w, h);
    let strict = scale_with(luma, &integral, w, h, valid, STRICT.0, STRICT.1);
    if strict.as_ref().is_some_and(|s| s.glyph_ink_share >= 0.80) {
        return strict.unwrap_or(fallback);
    }
    let sensitive = scale_with(luma, &integral, w, h, valid, SENSITIVE.0, SENSITIVE.1);
    match (strict, sensitive) {
        (Some(s), Some(t)) => {
            if t.glyph_ink_share > s.glyph_ink_share + 0.05 {
                t
            } else {
                s
            }
        }
        (Some(s), None) => s,
        (None, Some(t)) => t,
        (None, None) => fallback,
    }
}

/// `(ratio, minimum drop)` of the two ink masks.
pub(super) const STRICT: (f32, f32) = (0.82, 18.0);
pub(super) const SENSITIVE: (f32, f32) = (INK_RATIO, INK_MIN_DROP);

pub(super) fn scale_with(
    luma: &[u8],
    integral: &[u32],
    w: usize,
    h: usize,
    valid: Option<&[u8]>,
    ratio: f32,
    drop: f32,
) -> Option<TextScale> {
    let mut ink = ink_mask(luma, integral, w, h, valid, ratio, drop);
    // Faint print breaks into pieces under the sensitive mask: close one-pixel gaps.
    if ratio >= SENSITIVE.0 {
        ink = erode(&dilate(&ink, w, h), w, h);
    }
    let ink_px = ink.iter().filter(|v| **v != 0).count().max(1);
    let comps = components(&ink, w, h);
    let max_h = (w.min(h) / 6).max(60) as u32;
    let mut best = pick(&comps, max_h);
    // A dot-matrix font breaks a character into dots that are separate components at proxy scale:
    // if the dominant size is dot-sized, close the mask by one pixel and look again.
    if best.as_ref().is_none_or(|b| b.height < 6.0) {
        let closed = dilate(&ink, w, h);
        let comps2 = components(&closed, w, h);
        if let Some(b2) = pick(&comps2, max_h)
            && best
                .as_ref()
                .is_none_or(|b| b2.height > b.height && b2.near >= 20)
        {
            best = Some(b2);
        }
    }
    best.filter(|b| b.near >= MIN_COMPONENTS)
        .map(|b| TextScale {
            height: b.height.clamp(4.0, 60.0),
            components: b.near as u32,
            confident: true,
            concentration: b.near as f32 / b.plausible.max(1) as f32,
            glyph_ink_share: (b.near_area as f32 / ink_px as f32).min(1.0),
        })
}

/// The dominant glyph size and how regular the components are around it.
fn pick(comps: &[Comp], max_h: u32) -> Option<Pick> {
    // Plausible glyph-sized components: not specks, not rules, not huge blobs.
    let ok: Vec<&Comp> = comps
        .iter()
        .filter(|c| {
            c.h() >= 4
                && c.h() <= max_h
                && c.area >= 8
                && c.w() <= 8 * c.h()
                && c.area as f32 >= 0.10 * (c.w() * c.h()) as f32
        })
        .collect();
    if ok.len() < MIN_COMPONENTS {
        return None;
    }
    let mut hist = vec![0u32; max_h as usize + 2];
    for c in &ok {
        hist[c.h() as usize] += 1;
    }
    // [1 2 1] smoothing, then the mode.
    let mut mode = 0usize;
    let mut best = 0u32;
    for i in 1..=max_h as usize {
        let v = hist[i - 1] + 2 * hist[i] + hist[i + 1];
        if v > best {
            best = v;
            mode = i;
        }
    }
    let lo = (mode as f32 * 0.7).floor() as u32;
    let hi = (mode as f32 * 1.4).ceil() as u32;
    let mut near: Vec<u32> = Vec::new();
    let mut near_area = 0u64;
    for c in &ok {
        if (lo..=hi).contains(&c.h()) {
            near.push(c.h());
            near_area += u64::from(c.area);
        }
    }
    if near.is_empty() {
        return None;
    }
    near.sort_unstable();
    Some(Pick {
        height: near[near.len() / 2] as f32,
        near: near.len(),
        plausible: ok.len(),
        near_area,
    })
}

/// 1 where the pixel is clearly darker than its surroundings.
/// Integral image of the luma plane, `(w + 1) x (h + 1)`, exact in `u32` up to 16 MP of 255.
pub(super) fn integral_of(luma: &[u8], w: usize, h: usize) -> Vec<u32> {
    let iw = w + 1;
    let mut integral = vec![0u32; iw * (h + 1)];
    for y in 0..h {
        let mut row = 0u32;
        for x in 0..w {
            row += u32::from(luma[y * w + x]);
            integral[(y + 1) * iw + x + 1] = integral[y * iw + x + 1] + row;
        }
    }
    integral
}

fn ink_mask(
    luma: &[u8],
    integral: &[u32],
    w: usize,
    h: usize,
    valid: Option<&[u8]>,
    ratio: f32,
    min_drop: f32,
) -> Vec<u8> {
    let iw = w + 1;
    let mut ink = vec![0u8; w * h];
    ink.par_chunks_mut(w).enumerate().for_each(|(y, out)| {
        let (y0, y1) = (y.saturating_sub(RADIUS), (y + RADIUS + 1).min(h));
        for (x, o) in out.iter_mut().enumerate() {
            if valid.is_some_and(|v| v[y * w + x] == 0) {
                continue;
            }
            let (x0, x1) = (x.saturating_sub(RADIUS), (x + RADIUS + 1).min(w));
            let sum = integral[y1 * iw + x1] + integral[y0 * iw + x0]
                - integral[y0 * iw + x1]
                - integral[y1 * iw + x0];
            let n = ((y1 - y0) * (x1 - x0)) as f32;
            let mean = sum as f32 / n;
            let v = f32::from(luma[y * w + x]);
            if v < ratio * mean && mean - v >= min_drop {
                *o = 1;
            }
        }
    });
    ink
}

/// 3 x 3 dilation of a 0 / 1 mask.
fn dilate(m: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut horiz = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let a = x.saturating_sub(1);
            let b = (x + 1).min(w - 1);
            horiz[y * w + x] = m[y * w + a] | m[y * w + x] | m[y * w + b];
        }
    }
    let mut out = vec![0u8; w * h];
    for y in 0..h {
        let (a, b) = (y.saturating_sub(1), (y + 1).min(h - 1));
        for x in 0..w {
            out[y * w + x] = horiz[a * w + x] | horiz[y * w + x] | horiz[b * w + x];
        }
    }
    out
}

/// 3 x 3 erosion of a 0 / 1 mask (the border counts as ink-free).
fn erode(m: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut horiz = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let a = x.saturating_sub(1);
            let b = (x + 1).min(w - 1);
            horiz[y * w + x] = m[y * w + a] & m[y * w + x] & m[y * w + b];
        }
    }
    let mut out = vec![0u8; w * h];
    for y in 0..h {
        let (a, b) = (y.saturating_sub(1), (y + 1).min(h - 1));
        for x in 0..w {
            out[y * w + x] = horiz[a * w + x] & horiz[y * w + x] & horiz[b * w + x];
        }
    }
    out
}

fn find(parent: &mut [u32], mut i: u32) -> u32 {
    while parent[i as usize] != i {
        let p = parent[parent[i as usize] as usize];
        parent[i as usize] = p;
        i = p;
    }
    i
}

fn union(parent: &mut [u32], a: u32, b: u32) {
    let (ra, rb) = (find(parent, a), find(parent, b));
    if ra != rb {
        // The smaller index becomes the root, so the labelling is deterministic.
        let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
        parent[hi as usize] = lo;
    }
}

/// 8-connected components of a 0 / 1 mask: one pass over horizontal runs.
pub(crate) fn components(mask: &[u8], w: usize, h: usize) -> Vec<Comp> {
    #[derive(Clone, Copy)]
    struct Run {
        y: u32,
        x0: u32,
        x1: u32,
    }
    let mut runs: Vec<Run> = Vec::new();
    let mut parent: Vec<u32> = Vec::new();
    let mut prev: std::ops::Range<usize> = 0..0;
    for y in 0..h {
        let row = &mask[y * w..(y + 1) * w];
        let start = runs.len();
        let mut x = 0;
        while x < w {
            if row[x] == 0 {
                x += 1;
                continue;
            }
            let x0 = x;
            while x < w && row[x] != 0 {
                x += 1;
            }
            let id = runs.len() as u32;
            runs.push(Run {
                y: y as u32,
                x0: x0 as u32,
                x1: (x - 1) as u32,
            });
            parent.push(id);
            // Join every run of the row above that touches this one (8-connectivity).
            let cur = runs[id as usize];
            for p in prev.clone() {
                let r = runs[p];
                if r.x1 + 1 < cur.x0 {
                    continue;
                }
                if r.x0 > cur.x1 + 1 {
                    break;
                }
                union(&mut parent, id, p as u32);
            }
        }
        prev = start..runs.len();
    }
    let mut comps: Vec<Option<Comp>> = vec![None; runs.len()];
    for (i, r) in runs.iter().enumerate() {
        let root = find(&mut parent, i as u32) as usize;
        let area = r.x1 - r.x0 + 1;
        match &mut comps[root] {
            Some(c) => {
                c.x0 = c.x0.min(r.x0);
                c.x1 = c.x1.max(r.x1);
                c.y0 = c.y0.min(r.y);
                c.y1 = c.y1.max(r.y);
                c.area += area;
            }
            slot @ None => {
                *slot = Some(Comp {
                    x0: r.x0,
                    x1: r.x1,
                    y0: r.y,
                    y1: r.y,
                    area,
                })
            }
        }
    }
    comps.into_iter().flatten().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn components_are_eight_connected_and_counted() {
        // Two diagonal pixels are one component; a separate pixel is another.
        let (w, h) = (6, 4);
        let mut m = vec![0u8; w * h];
        m[0] = 1;
        m[w + 1] = 1;
        m[2 * w + 4] = 1;
        let c = components(&m, w, h);
        assert_eq!(c.len(), 2);
        let big = c.iter().find(|c| c.area == 2).unwrap();
        assert_eq!((big.x0, big.x1, big.y0, big.y1), (0, 1, 0, 1));
    }

    #[test]
    fn a_u_shape_is_one_component_whatever_the_scan_order() {
        // The two arms are separate runs until the bottom row joins them.
        let (w, h) = (5, 3);
        let mut m = vec![0u8; w * h];
        for y in 0..3 {
            m[y * w] = 1;
            m[y * w + 4] = 1;
        }
        for x in 0..5 {
            m[2 * w + x] = 1;
        }
        let c = components(&m, w, h);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].area, 9);
    }
}
