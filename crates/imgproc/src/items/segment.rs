// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! From colour planes to foreground components and their rectangles (ROADMAP M10.03-M10.07).

use super::bed::{BedModel, CLASS_BED, CLASS_FG, flood};
use super::color::Lab;
use super::geom::{P, Rect, convex_hull, min_area_rect};
use super::label::{Comp, edt, fill_holes, label8, morph, open, watershed};

/// Everything computed once per image and shared by the base run and the stability re-runs.
pub struct Planes {
    pub w: usize,
    pub h: usize,
    pub lab: Lab,
    pub g1: Vec<f32>,
    pub g4: Vec<f32>,
    pub class: Vec<u8>,
    pub noise: f32,
    /// Lightness of the bed (median of the agreeing border strips).
    pub bed_l: f32,
}

impl Planes {
    pub fn short(&self) -> usize {
        self.w.min(self.h)
    }
}

/// A foreground component and its rectangle.
#[derive(Debug, Clone)]
pub struct Blob {
    pub comp: Comp,
    pub rect: Rect,
    pub fill: f32,
}

pub struct Segmentation {
    pub labels: Vec<u32>,
    pub blobs: Vec<Blob>,
    pub rejected: Vec<(Rect, &'static str)>,
    pub crisp: Vec<bool>,
    pub t_edge: f32,
    pub n_components: usize,
    pub capped: bool,
}

pub const MAX_COMPONENTS: usize = 2000;

pub fn median_sampled(v: &[f32], step: usize) -> f32 {
    let mut s: Vec<f32> = v.iter().step_by(step.max(1)).copied().collect();
    if s.is_empty() {
        return 0.0;
    }
    let mid = s.len() / 2;
    *s.select_nth_unstable_by(mid, f32::total_cmp).1
}

pub fn prepare(lab_full: &Lab, model: &BedModel) -> Planes {
    let (w, h) = (lab_full.w, lab_full.h);
    let lab = lab_full.blurred(1.0);
    let g1 = lab.gradient();
    let g4 = lab.blurred(3.8).gradient();
    let class = model.classify(&lab);
    let bed_g: Vec<f32> = g1
        .iter()
        .zip(&class)
        .filter(|(_, c)| **c == CLASS_BED)
        .map(|(g, _)| *g)
        .step_by(5)
        .collect();
    let noise = median_sampled(&bed_g, 1);
    Planes {
        w,
        h,
        lab,
        g1,
        g4,
        class,
        noise,
        bed_l: model.triage.bed_lab[0],
    }
}

/// Hull of a component's boundary pixels (as pixel squares), in proxy pixel coordinates.
pub fn component_hull(labels: &[u32], w: usize, h: usize, c: &Comp) -> Vec<P> {
    let mut pts: Vec<P> = Vec::new();
    for y in c.y0..=c.y1 {
        for x in c.x0..=c.x1 {
            if labels[y * w + x] != c.id {
                continue;
            }
            let edge = x == 0
                || y == 0
                || x + 1 == w
                || y + 1 == h
                || labels[y * w + x - 1] != c.id
                || labels[y * w + x + 1] != c.id
                || labels[(y - 1) * w + x] != c.id
                || labels[(y + 1) * w + x] != c.id;
            if edge {
                let (fx, fy) = (x as f64, y as f64);
                pts.push((fx, fy));
                pts.push((fx + 1.0, fy));
                pts.push((fx, fy + 1.0));
                pts.push((fx + 1.0, fy + 1.0));
            }
        }
    }
    convex_hull(&mut pts)
}

/// Foreground by flooding the bed from the frame, then components and their rectangles.
/// `gain` scales the crisp-edge threshold (1.0 is the base run; 0.7 and 1.4 are the stability
/// re-runs).
pub fn segment(p: &Planes, gain: f32, min_area_frac: f32) -> Segmentation {
    let (w, h) = (p.w, p.h);
    let t_edge = (4.0 * p.noise.max(0.4)).clamp(3.0, 12.0) * gain;
    let crisp: Vec<bool> = (0..w * h)
        .map(|i| p.g1[i] >= t_edge && p.g1[i] >= 1.8 * p.g4[i])
        .collect();
    let blocking = morph(&crisp, w, h, 1, true);
    let reached = flood(&p.class, &blocking, w, h);
    let fg: Vec<bool> = reached.iter().map(|r| !r).collect();
    let r_open = ((0.004 * p.short() as f32).round() as usize).clamp(1, 3);
    let fg = open(&fg, w, h, r_open);
    let fg = fill_holes(&fg, w, h);
    let (labels, comps) = label8(&fg, w, h);
    let n_components = comps.len();
    let capped = n_components > MAX_COMPONENTS;
    let min_area = (min_area_frac * (w * h) as f32) as usize;
    let short = p.short() as f64;
    let mut blobs = Vec::new();
    let mut rejected = Vec::new();
    let mut comps = comps;
    if capped {
        comps.sort_by_key(|c| std::cmp::Reverse(c.area));
        comps.truncate(MAX_COMPONENTS);
    }
    for c in comps {
        if c.area < min_area {
            continue;
        }
        // Mostly foreground-coloured: a halo of shadow around nothing is not an item.
        let mut strong = 0usize;
        for y in c.y0..=c.y1 {
            for x in c.x0..=c.x1 {
                if labels[y * w + x] == c.id && p.class[y * w + x] == CLASS_FG {
                    strong += 1;
                }
            }
        }
        let hull = component_hull(&labels, w, h, &c);
        let Some(rect) = min_area_rect(&hull) else {
            continue;
        };
        if strong * 4 < c.area {
            rejected.push((rect, "shadow only"));
            continue;
        }
        let (long, shortside) = (rect.hw.max(rect.hh) * 2.0, rect.hw.min(rect.hh) * 2.0);
        if shortside < 0.012 * short || long > 15.0 * shortside {
            rejected.push((rect, "thin shape (lid edge, hair or scratch)"));
            continue;
        }
        let fill = c.area as f64 / rect.area().max(1.0);
        blobs.push(Blob {
            comp: c,
            rect,
            fill: fill as f32,
        });
    }
    Segmentation {
        labels,
        blobs,
        rejected,
        crisp,
        t_edge,
        n_components,
        capped,
    }
}

/// One piece of a split blob.
#[derive(Debug, Clone)]
pub struct Piece {
    pub rect: Rect,
    pub fill: f32,
    pub area: usize,
}

fn piece_of(mask: &[bool], w: usize, h: usize) -> Option<Piece> {
    let (labels, comps) = label8(mask, w, h);
    let c = comps.iter().max_by_key(|c| c.area)?;
    let hull = component_hull(&labels, w, h, c);
    let rect = min_area_rect(&hull)?;
    Some(Piece {
        fill: (c.area as f64 / rect.area().max(1.0)) as f32,
        area: c.area,
        rect,
    })
}

/// Distance-transform watershed split of one blob into rectangular pieces (ROADMAP M10.07).
/// Tries markers at several fractions of the largest distance; accepts the first split whose
/// pieces are all rectangular (fill at least `min_fill`) and big enough, and that cover at least
/// 90% of the blob.
pub fn split_blob(
    labels: &[u32],
    w: usize,
    h: usize,
    c: &Comp,
    min_fill: f32,
    min_area: usize,
    depth: usize,
) -> Option<Vec<Piece>> {
    let mask: Vec<bool> = labels.iter().map(|l| *l == c.id).collect();
    let dt = edt(&mask, w, h);
    let dmax = dt.iter().copied().fold(0.0f32, f32::max);
    if dmax < 3.0 {
        return None;
    }
    for frac in [0.85f32, 0.7, 0.55, 0.42] {
        let core: Vec<bool> = dt.iter().map(|d| *d >= frac * dmax).collect();
        let (ml, mc) = label8(&core, w, h);
        let markers_ok: Vec<&Comp> = mc.iter().filter(|m| m.area >= 3).collect();
        if markers_ok.len() < 2 || markers_ok.len() > 12 {
            continue;
        }
        let mut markers = vec![0u32; w * h];
        let mut next = 1u32;
        for m in &markers_ok {
            for y in m.y0..=m.y1 {
                for x in m.x0..=m.x1 {
                    if ml[y * w + x] == m.id {
                        markers[y * w + x] = next;
                    }
                }
            }
            next += 1;
        }
        let ws = watershed(&dt, &mask, &markers, w, h);
        let mut pieces = Vec::new();
        let mut covered = 0usize;
        let mut ok = true;
        for id in 1..next {
            let pm: Vec<bool> = ws.iter().map(|l| *l == id).collect();
            // Smooth the ragged cut a little before fitting.
            let pm = open(&pm, w, h, 1);
            let Some(piece) = piece_of(&pm, w, h) else {
                ok = false;
                break;
            };
            if piece.area < min_area {
                ok = false;
                break;
            }
            covered += piece.area;
            pieces.push(piece);
        }
        if !ok || covered * 10 < c.area * 9 {
            continue;
        }
        if pieces.iter().all(|p| p.fill >= min_fill) {
            return Some(pieces);
        }
        // A piece that is still not rectangular may be a further touching pair.
        if depth > 0 {
            let mut all = Vec::new();
            let mut good = true;
            for id in 1..next {
                let pm: Vec<bool> = ws.iter().map(|l| *l == id).collect();
                let (pl, pc) = label8(&pm, w, h);
                let Some(big) = pc.iter().max_by_key(|c| c.area) else {
                    good = false;
                    break;
                };
                let piece = piece_of(&pm, w, h)?;
                if piece.fill >= min_fill {
                    all.push(piece);
                } else if let Some(sub) = split_blob(&pl, w, h, big, min_fill, min_area, depth - 1)
                {
                    all.extend(sub);
                } else {
                    good = false;
                    break;
                }
            }
            if good && all.len() >= 2 {
                return Some(all);
            }
        }
    }
    None
}
