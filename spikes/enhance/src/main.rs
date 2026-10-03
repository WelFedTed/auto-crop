// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! M1.65 flatten spike. `cargo run --release` prints the table and exits non-zero if an
//! acceptance bar is missed; `cargo test --release` asserts the same bars.
//!
//! Acceptance (ROADMAP M1.65, bars PROVISIONAL): residual shading <= 3% of the paper level on
//! synthetic gradients, no barcode ghosts, gain <= 3x.

#![allow(clippy::needless_range_loop)] // throwaway numeric code: index loops read clearer here

mod flatten;
mod pages;

use flatten::{Gray, Params, Stats, flatten};
use pages::{Light, PAPER, Page, Rect, light_page, truth_page, truth_page_with};

pub const BAR: f32 = 0.03;
/// Evaluation cell edge in pixels (the residual is the mean over paper pixels of a cell).
const CELL: usize = 32;

#[derive(Debug, Clone, Copy)]
pub struct Residual {
    /// Worst cell deviation from the page's paper level, as a fraction of it.
    pub max: f32,
    /// 95th percentile over cells.
    pub p95: f32,
    /// Offset of the page's paper level (median cell mean) from the target white, as a fraction.
    /// A uniform offset is the tone stage's job (white point), not shading, so it is reported
    /// but not counted in `max`.
    pub bias: f32,
    pub cells: usize,
    /// Top-left pixel of the worst cell.
    pub worst: (usize, usize),
}

/// Residual shading of `out`: the mean of the output over the truth-paper pixels of every
/// `CELL` x `CELL` cell with at least 25% paper pixels, compared with the median of those means
/// over the whole page; the deviation is |mean - median| / paper. `only` restricts the statistics
/// to cells that touch any of the given rectangles (the ghost zones), still against the page-wide
/// median.
pub fn residual(out: &Gray, truth: &Gray, only: Option<&[Rect]>) -> Residual {
    let mut means: Vec<(usize, usize, f32)> = Vec::new();
    for cy in 0..truth.h / CELL {
        for cx in 0..truth.w / CELL {
            let (x0, y0) = (cx * CELL, cy * CELL);
            let (mut s, mut n) = (0u32, 0u32);
            for y in y0..y0 + CELL {
                for x in x0..x0 + CELL {
                    let i = y * truth.w + x;
                    if truth.data[i] == PAPER {
                        s += u32::from(out.data[i]);
                        n += 1;
                    }
                }
            }
            if n >= (CELL * CELL / 4) as u32 {
                means.push((x0, y0, s as f32 / n as f32));
            }
        }
    }
    let mut sorted: Vec<f32> = means.iter().map(|m| m.2).collect();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let level = sorted
        .get(sorted.len() / 2)
        .copied()
        .unwrap_or(f32::from(PAPER));
    let picked: Vec<&(usize, usize, f32)> = means
        .iter()
        .filter(|(x0, y0, _)| {
            only.is_none_or(|rects| {
                rects
                    .iter()
                    .any(|r| *x0 < r.2 && x0 + CELL > r.0 && *y0 < r.3 && y0 + CELL > r.1)
            })
        })
        .collect();
    let dev = |m: &(usize, usize, f32)| (m.2 - level).abs() / f32::from(PAPER);
    let worst = picked
        .iter()
        .max_by(|a, b| dev(a).total_cmp(&dev(b)))
        .map_or((0, 0), |m| (m.0, m.1));
    let mut devs: Vec<f32> = picked.iter().map(|m| dev(m)).collect();
    devs.sort_by(|a, b| a.total_cmp(b));
    let cells = devs.len();
    Residual {
        max: devs.last().copied().unwrap_or(0.0),
        p95: devs
            .get(cells.saturating_sub(1) * 95 / 100)
            .copied()
            .unwrap_or(0.0),
        bias: (level - f32::from(PAPER)) / f32::from(PAPER),
        worst,
        cells,
    }
}

pub struct CaseResult {
    pub name: String,
    pub whole: Residual,
    pub ghost: Residual,
    pub stats: Stats,
    pub min_light: f32,
}

/// Grown rectangles (one block of margin) around every hard object.
pub fn ghost_zones(page: &Page) -> Vec<Rect> {
    page.objects
        .iter()
        .map(|(_, r)| {
            Rect(
                r.0.saturating_sub(40),
                r.1.saturating_sub(40),
                r.2 + 40,
                r.3 + 40,
            )
        })
        .collect()
}

pub fn run_case(name: &str, page: &Page, light: Light, p: &Params, seed: u64) -> CaseResult {
    let input = light_page(&page.truth, light, 2.0, seed);
    let (out, stats) = flatten(&input, p);
    let mut min_light = 1.0f32;
    for y in (0..page.truth.h).step_by(7) {
        for x in (0..page.truth.w).step_by(7) {
            min_light = min_light.min(light.at(x, y, page.truth.w, page.truth.h));
        }
    }
    CaseResult {
        name: name.to_string(),
        whole: residual(&out, &page.truth, None),
        ghost: residual(&out, &page.truth, Some(&ghost_zones(page))),
        stats,
        min_light,
    }
}

pub fn cases() -> Vec<(&'static str, Light)> {
    vec![
        ("flat (control)", Light::Flat),
        ("tilt 30% x + 10% y", Light::Tilt { dx: 0.30, dy: 0.10 }),
        ("tilt 55% x (gain 2.2)", Light::Tilt { dx: 0.55, dy: 0.0 }),
        ("vignette 45%", Light::Vignette { k: 0.45 }),
        (
            "soft shadow 50%, 150 px",
            Light::Shadow {
                depth: 0.50,
                width: 150.0,
            },
        ),
    ]
}

/// Informational hazards (printed, not gating): known limits of a luma-only block map.
pub fn hazards() -> Vec<(&'static str, bool, Light)> {
    vec![
        (
            "hard shadow 60%, 60 px",
            false,
            Light::Shadow {
                depth: 0.60,
                width: 60.0,
            },
        ),
        ("grey photo block, flat", true, Light::Flat),
    ]
}

/// Held-out check: page seeds and lighting parameters that were not used while tuning the
/// parameters (the tuning sweep only ever saw page seed 11 and the `cases()` lights).
pub fn holdout() -> Vec<(String, Page, Light, u64, bool)> {
    let lights = [
        ("flat", Light::Flat),
        ("tilt 20% x + 35% y", Light::Tilt { dx: 0.20, dy: 0.35 }),
        ("vignette 50%", Light::Vignette { k: 0.50 }),
        (
            "soft shadow 45%, 220 px",
            Light::Shadow {
                depth: 0.45,
                width: 220.0,
            },
        ),
        (
            "steep shadow 40%, 120 px (informational)",
            Light::Shadow {
                depth: 0.40,
                width: 120.0,
            },
        ),
    ];
    let mut out = Vec::new();
    for page_seed in [21u64, 22, 23] {
        for (i, (name, light)) in lights.iter().enumerate() {
            out.push((
                format!("page {page_seed}: {name}"),
                truth_page(page_seed),
                *light,
                300 + page_seed * 10 + i as u64,
                !name.contains("informational"),
            ));
        }
    }
    out
}

/// `--cells SEED X Y`: raw and final map values around pixel (X, Y) of the flat page SEED.
fn cells(seed: u64, px: usize, py: usize) {
    let page = truth_page(seed);
    let input = light_page(&page.truth, Light::Flat, 2.0, 1);
    let p = Params::default();
    let (gw, gh, raw, valid, fin) = flatten::debug_cells(&input, &p);
    let (cx, cy) = (px / p.stride, py / p.stride);
    for gy in cy.saturating_sub(4)..(cy + 5).min(gh) {
        print!("y{:>5}: ", gy * p.stride + p.block / 2);
        for gx in cx.saturating_sub(6)..(cx + 7).min(gw) {
            let i = gy * gw + gx;
            print!(
                "{:>4.0}{}{:>4.0} ",
                raw[i],
                if valid[i] { ' ' } else { '*' },
                fin[i]
            );
        }
        println!();
    }
}

/// `--map N`: signed residual (percent of paper) per evaluation cell for gating case N, every
/// 3rd column and 4th row; `.` where a cell has too little paper.
fn map(n: usize) {
    let page = truth_page(11);
    let (name, light) = cases().remove(n);
    let input = light_page(&page.truth, light, 2.0, 100 + n as u64);
    let (out, _) = flatten(&input, &Params::default());
    println!("{name}");
    for cy in (0..page.truth.h / CELL).step_by(4) {
        for cx in (0..page.truth.w / CELL).step_by(3) {
            let (mut s, mut c) = (0u32, 0u32);
            for y in cy * CELL..(cy + 1) * CELL {
                for x in cx * CELL..(cx + 1) * CELL {
                    let i = y * page.truth.w + x;
                    if page.truth.data[i] == PAPER {
                        s += u32::from(out.data[i]);
                        c += 1;
                    }
                }
            }
            if c >= 256 {
                print!(
                    "{:>6.1}",
                    (s as f32 / c as f32 / f32::from(PAPER) - 1.0) * 100.0
                );
            } else {
                print!("{:>6}", ".");
            }
        }
        println!();
    }
}

/// `--sweep`: parameter variations on the two hardest gating cases (printed, never gating).
fn sweep() {
    let page = truth_page(11);
    let base = Params::default();
    let mut variants: Vec<(String, Params)> = vec![("default".into(), base)];
    for q in [0.85f32, 0.90] {
        variants.push((
            format!("percentile {q}"),
            Params {
                percentile: q,
                ..base
            },
        ));
    }
    for s in [1.0f32, 1.5, 3.0] {
        for n in [2usize, 3, 4] {
            variants.push((
                format!("smooth {s} passes {n}"),
                Params {
                    smooth: s,
                    passes: n,
                    ..base
                },
            ));
        }
    }
    for f in [1usize, 4] {
        variants.push((format!("pre {f}"), Params { pre: f, ..base }));
    }
    for r in [0usize, 5, 12] {
        variants.push((
            format!("fill radius {r}"),
            Params {
                plane_radius: r,
                ..base
            },
        ));
    }
    for ink in [0.5f32, 0.7] {
        variants.push((
            format!("reject_ink {ink}"),
            Params {
                reject_ink: ink,
                ..base
            },
        ));
    }
    for (b, s) in [(32usize, 16usize), (48, 16), (24, 12)] {
        variants.push((
            format!("block {b} stride {s}"),
            Params {
                block: b,
                stride: s,
                ..base
            },
        ));
    }
    for (name, p) in variants {
        print!("{name:<24}");
        for (i, (cname, light)) in cases().into_iter().enumerate().skip(1) {
            let r = run_case(cname, &page, light, &p, 100 + i as u64);
            print!(
                "  {:>5.2}%/{:>5.2}% ({}/{})",
                r.whole.max * 100.0,
                r.ghost.max * 100.0,
                (r.stats.rejected_dark + r.stats.rejected_ink) / p.passes,
                r.stats.cells
            );
        }
        println!();
    }
}

fn main() {
    if std::env::args().any(|a| a == "--sweep") {
        sweep();
        return;
    }
    if let Some(i) = std::env::args().position(|a| a == "--cells") {
        let a: Vec<usize> = std::env::args()
            .skip(i + 1)
            .filter_map(|v| v.parse().ok())
            .collect();
        cells(a[0] as u64, a[1], a[2]);
        return;
    }
    if let Some(i) = std::env::args().position(|a| a == "--map") {
        map(std::env::args()
            .nth(i + 1)
            .and_then(|v| v.parse().ok())
            .unwrap_or(1));
        return;
    }
    let page = truth_page(11);
    let mp = (page.truth.w * page.truth.h) as f64 / 1e6;
    let p = Params::default();
    println!(
        "page {}x{} ({mp:.2} MP), block {} stride {} percentile {:.0}%, white {}, max gain {}",
        page.truth.w,
        page.truth.h,
        p.block,
        p.stride,
        p.percentile * 100.0,
        p.white,
        p.max_gain
    );
    println!(
        "{:<26} {:>6} {:>8} {:>8} {:>8} {:>8} {:>7} {:>7} {:>9} {:>9}",
        "case",
        "minL",
        "resMax",
        "resP95",
        "bias",
        "ghostMax",
        "gain",
        "reject",
        "grid ms",
        "apply ms"
    );
    let mut ok = true;
    for (i, (name, light)) in cases().into_iter().enumerate() {
        let r = run_case(name, &page, light, &p, 100 + i as u64);
        let pass = r.whole.max <= BAR && r.ghost.max <= BAR && r.stats.max_gain <= p.max_gain;
        ok &= pass;
        println!(
            "{:<26} {:>6.2} {:>7.2}% {:>7.2}% {:>+7.2}% {:>7.2}% {:>6.2}x {:>3}/{:<3} {:>9.1} {:>9.1}  {}",
            r.name,
            r.min_light,
            r.whole.max * 100.0,
            r.whole.p95 * 100.0,
            r.whole.bias * 100.0,
            r.ghost.max * 100.0,
            r.stats.max_gain,
            (r.stats.rejected_dark + r.stats.rejected_ink) / p.passes,
            r.stats.cells,
            r.stats.grid_ms,
            r.stats.apply_ms,
            if pass { "PASS" } else { "FAIL" }
        );
        if !pass {
            println!("    worst cell at {:?}", r.whole.worst);
        }
    }
    println!("-- hazards (informational, not gating) --");
    for (i, (name, grey, light)) in hazards().into_iter().enumerate() {
        let pg = truth_page_with(11, grey);
        let r = run_case(name, &pg, light, &p, 200 + i as u64);
        println!(
            "{:<26} {:>6.2} {:>7.2}% {:>7.2}% {:>+7.2}% {:>7.2}% {:>6.2}x {:>3}/{:<3}",
            r.name,
            r.min_light,
            r.whole.max * 100.0,
            r.whole.p95 * 100.0,
            r.whole.bias * 100.0,
            r.ghost.max * 100.0,
            r.stats.max_gain,
            (r.stats.rejected_dark + r.stats.rejected_ink) / p.passes,
            r.stats.cells
        );
    }
    println!("-- held-out pages and lights (not used for tuning) --");
    let mut worst_whole = 0.0f32;
    let mut worst_ghost = 0.0f32;
    for (name, pg, light, seed, gating) in holdout() {
        let r = run_case(&name, &pg, light, &p, seed);
        if gating {
            worst_whole = worst_whole.max(r.whole.max);
            worst_ghost = worst_ghost.max(r.ghost.max);
        }
        println!(
            "{:<34} {:>6.2} {:>7.2}% {:>7.2}% {:>+7.2}% {:>7.2}% {:>6.2}x",
            r.name,
            r.min_light,
            r.whole.max * 100.0,
            r.whole.p95 * 100.0,
            r.whole.bias * 100.0,
            r.ghost.max * 100.0,
            r.stats.max_gain
        );
        if gating && r.whole.max > BAR {
            println!("    worst cell at {:?}", r.whole.worst);
        }
    }
    let hold_ok = worst_whole <= BAR && worst_ghost <= BAR;
    println!(
        "held-out worst residual {:.2}%, worst ghost zone {:.2}%: {}",
        worst_whole * 100.0,
        worst_ghost * 100.0,
        if hold_ok { "PASS" } else { "FAIL" }
    );
    ok &= hold_ok;
    // Negative control: without rejection the black band must leave a visible ghost, otherwise the
    // ghost metric could not have caught a failure.
    let nr = Params { reject: false, ..p };
    let r = run_case(
        "shadow, NO rejection",
        &page,
        Light::Shadow {
            depth: 0.50,
            width: 150.0,
        },
        &nr,
        100,
    );
    println!(
        "{:<26} {:>6.2} {:>7.2}% {:>7.2}% {:>+7.2}% {:>7.2}% {:>6.2}x   (negative control: ghost expected)",
        r.name,
        r.min_light,
        r.whole.max * 100.0,
        r.whole.p95 * 100.0,
        r.whole.bias * 100.0,
        r.ghost.max * 100.0,
        r.stats.max_gain
    );
    if r.ghost.max <= BAR {
        println!("NEGATIVE CONTROL FAILED: the ghost metric did not detect the missing rejection");
        ok = false;
    }
    println!("{}", if ok { "ALL BARS MET" } else { "BARS MISSED" });
    std::process::exit(if ok { 0 } else { 1 });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_gradients_meet_the_bars() {
        let page = truth_page(11);
        let p = Params::default();
        for (i, (name, light)) in cases().into_iter().enumerate() {
            let r = run_case(name, &page, light, &p, 100 + i as u64);
            assert!(r.whole.max <= BAR, "{name}: residual {:.3}", r.whole.max);
            assert!(r.ghost.max <= BAR, "{name}: ghost {:.3}", r.ghost.max);
            assert!(
                r.stats.max_gain <= p.max_gain + 1e-3,
                "{name}: gain {}",
                r.stats.max_gain
            );
        }
    }

    #[test]
    fn held_out_pages_and_lights_meet_the_bars() {
        let p = Params::default();
        for (name, page, light, seed, gating) in holdout() {
            if !gating {
                continue;
            }
            let r = run_case(&name, &page, light, &p, seed);
            assert!(r.whole.max <= BAR, "{name}: residual {:.3}", r.whole.max);
            assert!(r.ghost.max <= BAR, "{name}: ghost {:.3}", r.ghost.max);
        }
    }

    #[test]
    fn the_ghost_metric_catches_missing_rejection() {
        let page = truth_page(11);
        let p = Params {
            reject: false,
            ..Params::default()
        };
        let r = run_case(
            "x",
            &page,
            Light::Shadow {
                depth: 0.5,
                width: 150.0,
            },
            &p,
            100,
        );
        assert!(
            r.ghost.max > BAR,
            "no ghost without rejection: {:.3}",
            r.ghost.max
        );
    }
}
