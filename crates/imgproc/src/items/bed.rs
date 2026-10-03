// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The bed (or table) colour model and the background flood fill (ROADMAP M10.01, M10.03).
//!
//! The background is a set of Lab colour cells learnt from a ring just inside the frame: a cell
//! counts only if it is common and appears in several parts of the ring, so a photo lying on one
//! edge does not become "background", while a wood grain or a lid gradient (many cells, each seen
//! all around) does. A pixel is one of three classes: tight (a background colour), loose or
//! shadow (a little off it, or the same hue but darker) and foreground. The flood starts at the
//! frame and spreads through tight pixels, and through loose or shadow pixels that are not on a
//! crisp edge, so a soft shadow joins the bed while the crisp outline of an item stops the flood.
//! Whatever the flood does not reach is foreground.

use super::color::{Lab, de76};

const L_CELL: f32 = 4.0;
const AB_CELL: f32 = 4.0;
const L_BINS: usize = 27;
const AB_BINS: usize = 64;

pub const CLASS_FG: u8 = 0;
pub const CLASS_SOFT: u8 = 1;
pub const CLASS_BED: u8 = 2;

#[inline]
fn cell_index(l: f32, a: f32, b: f32) -> usize {
    let li = ((l / L_CELL) as isize).clamp(0, L_BINS as isize - 1) as usize;
    let ai = (((a + 128.0) / AB_CELL) as isize).clamp(0, AB_BINS as isize - 1) as usize;
    let bi = (((b + 128.0) / AB_CELL) as isize).clamp(0, AB_BINS as isize - 1) as usize;
    (li * AB_BINS + ai) * AB_BINS + bi
}

fn unpack(i: usize) -> (isize, isize, isize) {
    let bi = i % AB_BINS;
    let ai = (i / AB_BINS) % AB_BINS;
    let li = i / (AB_BINS * AB_BINS);
    (li as isize, ai as isize, bi as isize)
}

fn pack(l: isize, a: isize, b: isize) -> Option<usize> {
    if l < 0
        || a < 0
        || b < 0
        || l >= L_BINS as isize
        || a >= AB_BINS as isize
        || b >= AB_BINS as isize
    {
        None
    } else {
        Some((l as usize * AB_BINS + a as usize) * AB_BINS + b as usize)
    }
}

/// What the border strips say about the scene (M10.01).
#[derive(Debug, Clone)]
pub struct Triage {
    /// Three or four sides agree on one colour within dE76 8.
    pub bed_like: bool,
    pub sides_agreeing: u8,
    /// Median Lab of the agreeing sides (or of all four when none agree).
    pub bed_lab: [f32; 3],
    /// Largest dE76 between the median colours of any two sides.
    pub spread: f32,
}

pub struct BedModel {
    pub triage: Triage,
    tight: Vec<bool>,
    soft: Vec<bool>,
    /// Number of qualifying (background) colour cells.
    pub cells: usize,
}

fn median(v: &mut [f32]) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    let mid = v.len() / 2;
    *v.select_nth_unstable_by(mid, f32::total_cmp).1
}

/// Border strips of 1.5% (at least 3 px), starting 0.5% in: the median Lab of each of the four.
fn triage(lab: &Lab) -> Triage {
    let (w, h) = (lab.w, lab.h);
    let s = w.min(h) as f32;
    let off = ((0.005 * s).round() as usize).max(1);
    let thick = ((0.015 * s).round() as usize).max(3);
    let strip = |x0: usize, y0: usize, x1: usize, y1: usize| -> [f32; 3] {
        let (mut l, mut a, mut b) = (Vec::new(), Vec::new(), Vec::new());
        for y in y0..y1.min(h) {
            for x in x0..x1.min(w) {
                let i = y * w + x;
                l.push(lab.l[i]);
                a.push(lab.a[i]);
                b.push(lab.b[i]);
            }
        }
        [median(&mut l), median(&mut a), median(&mut b)]
    };
    let sides = [
        strip(0, off, w, off + thick),
        strip(w.saturating_sub(off + thick), 0, w - off.min(w - 1), h),
        strip(0, h.saturating_sub(off + thick), w, h - off.min(h - 1)),
        strip(off, 0, off + thick, h),
    ];
    let mut best = (0u8, 0usize);
    let mut spread = 0.0f32;
    for i in 0..4 {
        let mut n = 0;
        for j in 0..4 {
            if i != j {
                let d = de76(sides[i], sides[j]);
                spread = spread.max(d);
                if d <= 8.0 {
                    n += 1;
                }
            }
        }
        if n > best.0 {
            best = (n, i);
        }
    }
    let members: Vec<[f32; 3]> = (0..4)
        .filter(|&j| j == best.1 || de76(sides[best.1], sides[j]) <= 8.0)
        .map(|j| sides[j])
        .collect();
    let pick = |c: usize| {
        let mut v: Vec<f32> = members.iter().map(|m| m[c]).collect();
        median(&mut v)
    };
    let bed_lab = [pick(0), pick(1), pick(2)];
    Triage {
        bed_like: best.0 >= 2,
        sides_agreeing: best.0 + 1,
        bed_lab,
        spread,
    }
}

/// Colour clusters of the candidate cells (single linkage within two cells per axis); keeps the
/// cells of every cluster whose total count is at least 40% of the largest cluster's.
fn keep_main_clusters(core: &[usize], count: &[u32]) -> Vec<usize> {
    use std::collections::HashMap;
    let index: HashMap<usize, usize> = core.iter().enumerate().map(|(i, c)| (*c, i)).collect();
    let mut cluster = vec![usize::MAX; core.len()];
    let mut totals: Vec<u64> = Vec::new();
    for start in 0..core.len() {
        if cluster[start] != usize::MAX {
            continue;
        }
        let id = totals.len();
        totals.push(0);
        let mut stack = vec![start];
        cluster[start] = id;
        while let Some(i) = stack.pop() {
            totals[id] += u64::from(count[core[i]]);
            let (l, a, b) = unpack(core[i]);
            for dl in -2isize..=2 {
                for da in -2isize..=2 {
                    for db in -2isize..=2 {
                        if let Some(j) = pack(l + dl, a + da, b + db)
                            && let Some(&k) = index.get(&j)
                            && cluster[k] == usize::MAX
                        {
                            cluster[k] = id;
                            stack.push(k);
                        }
                    }
                }
            }
        }
    }
    let biggest = totals.iter().copied().max().unwrap_or(0);
    core.iter()
        .enumerate()
        .filter(|(i, _)| totals[cluster[*i]] * 5 >= biggest * 2)
        .map(|(_, c)| *c)
        .collect()
}

impl BedModel {
    pub fn learn(lab: &Lab) -> BedModel {
        let (w, h) = (lab.w, lab.h);
        let tri = triage(lab);
        let s = w.min(h);
        let m0 = ((0.01 * s as f32).round() as usize).max(1);
        let m1 = ((0.04 * s as f32).round() as usize).max(m0 + 3);
        let n_cells = L_BINS * AB_BINS * AB_BINS;
        let mut count = vec![0u32; n_cells];
        let mut seg = vec![0u16; n_cells];
        let mut total = 0u32;
        for y in 0..h {
            for x in 0..w {
                let (dl, dr, dt, db) = (x, w - 1 - x, y, h - 1 - y);
                let d = dl.min(dr).min(dt).min(db);
                if d < m0 || d > m1 || (x + y) % 2 != 0 {
                    continue;
                }
                // Which of the 16 ring segments: side by nearest border, then position along it.
                let (side, t) = if d == dt {
                    (0, x as f32 / w as f32)
                } else if d == dr {
                    (1, y as f32 / h as f32)
                } else if d == db {
                    (2, 1.0 - x as f32 / w as f32)
                } else {
                    (3, 1.0 - y as f32 / h as f32)
                };
                let sg = side * 4 + ((t * 4.0) as usize).min(3);
                let i = y * w + x;
                let c = cell_index(lab.l[i], lab.a[i], lab.b[i]);
                count[c] += 1;
                seg[c] |= 1 << sg;
                total += 1;
            }
        }
        let min_count = ((total as f32 * 0.0015) as u32).max(3);
        let mut core: Vec<usize> = (0..n_cells)
            .filter(|&c| count[c] >= min_count && seg[c].count_ones() >= 3)
            .collect();
        if core.is_empty() {
            // A tiny or odd frame: fall back to the most common cells.
            let mut idx: Vec<usize> = (0..n_cells).filter(|&c| count[c] > 0).collect();
            idx.sort_by_key(|&c| std::cmp::Reverse(count[c]));
            core = idx.into_iter().take(12).collect();
        }
        // Items lying on the frame put their own colours into the ring. Group the candidate cells
        // into colour clusters (cells within two steps of each other) and keep a cluster only if it
        // is at least 40% as common as the commonest one: a wood grain or a two-tone checker
        // passes, a few white print borders on a dark table do not.
        core = keep_main_clusters(&core, &count);
        let mut tight = vec![false; n_cells];
        let mut soft = vec![false; n_cells];
        for &c in &core {
            let (l, a, b) = unpack(c);
            // A little off the bed colour: up to 12 units darker or chroma-shifted, but only 4 units
            // lighter (a lighter pixel is paper, not bed).
            for dl in -3isize..=1 {
                for da in -3isize..=3 {
                    for db in -3isize..=3 {
                        let Some(j) = pack(l + dl, a + da, b + db) else {
                            continue;
                        };
                        if dl.abs() <= 1 && da.abs() <= 1 && db.abs() <= 1 {
                            tight[j] = true;
                        }
                        soft[j] = true;
                    }
                }
            }
            // The same hue, darker (a shadow on the bed), by up to 8 cells (32 L units).
            for k in 1..=8isize {
                for da in -1isize..=1 {
                    for db in -1isize..=1 {
                        if let Some(j) = pack(l - k, a + da, b + db) {
                            soft[j] = true;
                        }
                    }
                }
            }
        }
        let cells = core.len();
        BedModel {
            triage: tri,
            tight,
            soft,
            cells,
        }
    }

    /// 2 = a background colour, 1 = near one or a darker shade of one, 0 = foreground colour.
    pub fn classify(&self, lab: &Lab) -> Vec<u8> {
        (0..lab.w * lab.h)
            .map(|i| {
                let c = cell_index(lab.l[i], lab.a[i], lab.b[i]);
                if self.tight[c] {
                    CLASS_BED
                } else if self.soft[c] {
                    CLASS_SOFT
                } else {
                    CLASS_FG
                }
            })
            .collect()
    }
}

/// The part of the frame the flood can reach: seeded by background-coloured pixels in the outer
/// band, spreading through tight pixels and through soft pixels that are not on a crisp edge.
pub fn flood(class: &[u8], crisp_blocking: &[bool], w: usize, h: usize) -> Vec<bool> {
    let mut reached = vec![false; w * h];
    let mut stack: Vec<usize> = Vec::new();
    let band = ((0.012 * w.min(h) as f32).round() as usize).max(2);
    for y in 0..h {
        for x in 0..w {
            if x.min(w - 1 - x).min(y).min(h - 1 - y) < band {
                let i = y * w + x;
                if class[i] == CLASS_BED {
                    reached[i] = true;
                    stack.push(i);
                }
            }
        }
    }
    let pass = |j: usize| class[j] == CLASS_BED || (class[j] == CLASS_SOFT && !crisp_blocking[j]);
    while let Some(i) = stack.pop() {
        let (x, y) = (i % w, i / w);
        let go = |j: usize, reached: &mut Vec<bool>, stack: &mut Vec<usize>| {
            if !reached[j] && pass(j) {
                reached[j] = true;
                stack.push(j);
            }
        };
        if x > 0 {
            go(i - 1, &mut reached, &mut stack);
        }
        if x + 1 < w {
            go(i + 1, &mut reached, &mut stack);
        }
        if y > 0 {
            go(i - w, &mut reached, &mut stack);
        }
        if y + 1 < h {
            go(i + w, &mut reached, &mut stack);
        }
    }
    reached
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Raster;
    use crate::items::color::to_lab;

    fn scene(bg: [u8; 3], item: [u8; 3]) -> Raster {
        let mut r = Raster::filled(120, 80, bg);
        for y in 20..60 {
            for x in 30..90 {
                r.set_pixel(x, y, item);
            }
        }
        r
    }

    #[test]
    fn a_flat_bed_is_bed_like_and_the_item_is_foreground() {
        let lab = to_lab(&scene([240, 240, 240], [40, 80, 160]));
        let m = BedModel::learn(&lab);
        assert!(m.triage.bed_like && m.triage.sides_agreeing == 4);
        let class = m.classify(&lab);
        assert_eq!(class[5 * 120 + 5], CLASS_BED);
        assert_eq!(class[40 * 120 + 60], CLASS_FG);
        let blocking = vec![false; 120 * 80];
        let f = flood(&class, &blocking, 120, 80);
        assert!(f[5 * 120 + 5] && !f[40 * 120 + 60]);
    }

    #[test]
    fn four_different_sides_are_not_bed_like() {
        let mut r = Raster::filled(120, 80, [200, 200, 200]);
        for y in 0..80 {
            for x in 0..120 {
                let v = if y < 10 {
                    20
                } else if y > 70 {
                    120
                } else if x < 10 {
                    220
                } else {
                    60
                };
                r.set_pixel(x, y, [v, v, v]);
            }
        }
        let m = BedModel::learn(&to_lab(&r));
        assert!(!m.triage.bed_like, "{:?}", m.triage);
    }

    #[test]
    fn a_shadow_joins_the_bed_but_a_crisp_edge_stops_the_flood() {
        let lab = to_lab(&scene([240, 240, 240], [40, 80, 160]));
        // Pretend the pixels just right of x = 90 are a dark-grey shadow.
        let mut shadowed = lab;
        for y in 20..60 {
            for x in 90..100 {
                let i = y * 120 + x;
                shadowed.l[i] *= 0.8;
            }
        }
        let m = BedModel::learn(&shadowed);
        let class = m.classify(&shadowed);
        assert_ne!(class[40 * 120 + 95], CLASS_FG);
        let free = flood(&class, &vec![false; 120 * 80], 120, 80);
        assert!(free[40 * 120 + 95]);
        let blocking = vec![true; 120 * 80];
        // Everything soft is blocked; the strictly background-coloured pixels still flood.
        let blocked = flood(&class, &blocking, 120, 80);
        assert!(blocked[5 * 120 + 5] && !blocked[40 * 120 + 95]);
    }
}
