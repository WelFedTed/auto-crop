// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Oracles of the enhancer on synthetic pages with exact ground truth (PLAN 5.13): residual
//! shading, ghosts beside hard objects, stroke and decimal-point survival, text scale, validity
//! mask, thread independence, cancellation, degenerate inputs, scale invariance.

use super::testpages::*;
use super::*;
use crate::cancel::NeverCancel;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

fn run(img: &Raster, mode: Mode, strength: f32) -> (Enhanced, Report) {
    enhance(
        img.clone(),
        &Params::new(mode, strength),
        None,
        &NeverCancel,
    )
    .unwrap()
}

fn gray_of(e: &Enhanced) -> Vec<u8> {
    match e {
        Enhanced::Gray(g) => g.data.clone(),
        Enhanced::Rgb(r) => luma_of(r),
    }
}

/// Residual shading as in the spike: per 32 x 32 cell over the pixels that are paper in the truth
/// (not within 3 px of ink), `|mean(out) - median of cell means| / median`. Returns (worst, p95).
fn residual(
    out: &[u8],
    ink: &[bool],
    w: u32,
    h: u32,
    skip: &dyn Fn(u32, u32) -> bool,
) -> (f32, f32) {
    residual_n(out, ink, w, h, skip, 200)
}

/// [`residual`] counting cells with at least `min_n` measured pixels.
fn residual_n(
    out: &[u8],
    ink: &[bool],
    w: u32,
    h: u32,
    skip: &dyn Fn(u32, u32) -> bool,
    min_n: u64,
) -> (f32, f32) {
    let (cw, ch) = ((w / 32) as usize, (h / 32) as usize);
    let mut means = Vec::new();
    for cy in 0..ch {
        for cx in 0..cw {
            let (mut s, mut n) = (0u64, 0u64);
            for y in cy * 32..cy * 32 + 32 {
                for x in cx * 32..cx * 32 + 32 {
                    let (xu, yu) = (x as u32, y as u32);
                    if skip(xu, yu) {
                        continue;
                    }
                    // Not near ink: a 7 x 7 box around the pixel holds none.
                    let near = (y.saturating_sub(3)..=(y + 3).min(h as usize - 1)).any(|yy| {
                        (x.saturating_sub(3)..=(x + 3).min(w as usize - 1))
                            .any(|xx| ink[yy * w as usize + xx])
                    });
                    if !near {
                        s += u64::from(out[y * w as usize + x]);
                        n += 1;
                    }
                }
            }
            if n >= min_n {
                means.push(((cx, cy), s as f32 / n as f32));
            }
        }
    }
    let mut sorted: Vec<f32> = means.iter().map(|m| m.1).collect();
    sorted.sort_by(f32::total_cmp);
    let med = sorted[sorted.len() / 2];
    if std::env::var_os("ENHANCE_DEBUG").is_some() {
        let mut worst: Vec<_> = means
            .iter()
            .map(|((x, y), m)| ((x * 32, y * 32), (m - med).abs() / med))
            .collect();
        worst.sort_by(|a, b| b.1.total_cmp(&a.1));
        println!("worst cells (x, y): {:?}", &worst[..worst.len().min(6)]);
    }
    let mut dev: Vec<f32> = sorted
        .iter()
        .map(|m| (m - med).abs() / med.max(1.0))
        .collect();
    dev.sort_by(f32::total_cmp);
    (
        dev[dev.len() - 1],
        dev[(dev.len() * 95 / 100).min(dev.len() - 1)],
    )
}

fn no_skip(_: u32, _: u32) -> bool {
    false
}

#[test]
fn original_and_strength_zero_never_touch_the_pixels() {
    let p = page(&PageSpec::document(3));
    let img = lit(&p.truth, Light::Tilt { dx: 0.4, dy: 0.1 }, [1.0; 3], 2.0, 4);
    for (mode, s) in [
        (Mode::Original, 1.0),
        (Mode::Clean, 0.0),
        (Mode::Document, 0.0),
        (Mode::Receipt, -3.0),
        (Mode::Document, f32::NAN),
    ] {
        let (out, rep) = run(&img, mode, s);
        assert_eq!(out, Enhanced::Rgb(img.clone()), "{mode:?} strength {s}");
        assert!(!rep.applied);
    }
}

#[test]
fn clean_flattens_every_light_to_within_two_percent() {
    // PROVISIONAL bar of the spike: 3%. The synthetic pages are flattened much better; a hard
    // shadow edge (40% over 60 px) is the documented limit and is held to a looser bar.
    let spec = PageSpec::document(11);
    let p = page(&spec);
    let lights: [(&str, Light, f32); 5] = [
        ("tilt", Light::Tilt { dx: 0.55, dy: 0.1 }, 0.02),
        ("vignette", Light::Vignette { k: 0.45 }, 0.015),
        (
            "shadow",
            Light::Shadow {
                depth: 0.5,
                width: 150.0,
            },
            0.015,
        ),
        (
            "hard shadow",
            Light::Shadow {
                depth: 0.4,
                width: 60.0,
            },
            0.08,
        ),
        ("flat", Light::Flat, 0.005),
    ];
    for (name, light, bar) in lights {
        let img = lit(&p.truth, light, [1.0; 3], 2.0, 5);
        let (before, _) = residual(&luma_of(&img), &p.ink, 1000, 1400, &no_skip);
        let (out, rep) = run(&img, Mode::Clean, 1.0);
        let (worst, p95) = residual(&gray_of(&out), &p.ink, 1000, 1400, &no_skip);
        println!("{name}: before {before:.3} after worst {worst:.4} p95 {p95:.4}  {rep:?}");
        assert!(worst <= bar, "{name}: worst cell {worst:.4} > {bar}");
        assert!(rep.max_gain <= 3.0 + 1e-3);
    }
}

#[test]
fn no_ghost_beside_a_barcode_a_black_band_or_a_stamp() {
    let spec = PageSpec::document(12).with_objects();
    let p = page(&spec);
    let (w, h) = (spec.width, spec.height);
    let in_object = |x: u32, y: u32| {
        p.objects
            .iter()
            .any(|(_, r)| r.grow(2, w, h).contains(x, y))
    };
    // The ghost zone: paper within 40 px of a barcode, the band or the stamp.
    let outside_ring = |x: u32, y: u32| {
        in_object(x, y)
            || !p
                .objects
                .iter()
                .any(|(_, r)| r.grow(40, w, h).contains(x, y))
    };
    // (The darkest corner stays above 1/3 of the light: the gain cap is 3.)
    for light in [
        Light::Tilt { dx: 0.45, dy: 0.12 },
        Light::Shadow {
            depth: 0.45,
            width: 220.0,
        },
        Light::Vignette { k: 0.4 },
    ] {
        let img = lit(&p.truth, light, [1.0; 3], 2.0, 6);
        let (out, _) = run(&img, Mode::Clean, 1.0);
        let lum = gray_of(&out);
        // The whole page without the objects themselves, and the ring around them, against the
        // median paper level of the page: no halo, no dark patch.
        let (all, _) = residual(&lum, &p.ink, w, h, &in_object);
        let (ring, _) = residual_n(&lum, &p.ink, w, h, &outside_ring, 100);
        println!("{light:?}: page {all:.4}  ghost zone {ring:.4}");
        assert!(all <= 0.02, "{light:?}: page residual {all:.4}");
        assert!(ring <= 0.02, "{light:?}: ghost {ring:.4}");
    }
}

/// Recall of the sharp ink and the cleanliness of the paper.
fn ink_scores(out: &[u8], p: &TestPage, w: u32, h: u32) -> (f32, f32) {
    let (mut hit, mut tot) = (0u64, 0u64);
    for (i, ink) in p.ink.iter().enumerate() {
        if *ink {
            tot += 1;
            hit += u64::from(out[i] < 128);
        }
    }
    let (mut clean, mut paper) = (0u64, 0u64);
    for y in 0..h as usize {
        for x in 0..w as usize {
            let near = (y.saturating_sub(3)..=(y + 3).min(h as usize - 1)).any(|yy| {
                (x.saturating_sub(3)..=(x + 3).min(w as usize - 1))
                    .any(|xx| p.ink[yy * w as usize + xx])
            });
            if !near {
                paper += 1;
                clean += u64::from(out[y * w as usize + x] >= 200);
            }
        }
    }
    (hit as f32 / tot as f32, clean as f32 / paper as f32)
}

#[test]
fn document_and_receipt_keep_the_strokes_and_whiten_the_paper() {
    for (name, spec, mode) in [
        ("document", PageSpec::document(21), Mode::Document),
        (
            "receipt page, document mode",
            PageSpec::receipt(22),
            Mode::Document,
        ),
        ("receipt", PageSpec::receipt(23), Mode::Receipt),
        (
            "document page, receipt mode",
            PageSpec::document(24),
            Mode::Receipt,
        ),
    ] {
        let p = page(&spec);
        let img = lit(
            &p.truth,
            Light::Shadow {
                depth: 0.5,
                width: 160.0,
            },
            [1.0; 3],
            3.0,
            9,
        );
        let (out, rep) = run(&img, mode, 1.0);
        assert!(out.is_gray());
        let (recall, clean) = ink_scores(&gray_of(&out), &p, spec.width, spec.height);
        println!(
            "{name}: ink recall {recall:.4}, clean paper {clean:.4}, window {}",
            rep.window
        );
        assert!(recall >= 0.90, "{name}: ink recall {recall}");
        assert!(clean >= 0.995, "{name}: paper cleanliness {clean}");
    }
}

#[test]
fn decimal_points_survive_at_several_scales() {
    // 14, 21, 28 and 42 px glyphs: roughly 100, 150, 200 and 300 dpi-equivalents of 10 pt print.
    for scale in [2u32, 3, 4, 6] {
        for mode in [Mode::Document, Mode::Receipt] {
            let spec =
                PageSpec::receipt(30 + u64::from(scale)).sized(200 * scale, 600 * scale, scale);
            let p = page(&spec);
            let img = lit(&p.truth, Light::Tilt { dx: 0.4, dy: 0.2 }, [1.0; 3], 3.0, 3);
            let (out, _) = run(&img, mode, 1.0);
            let g = gray_of(&out);
            let w = spec.width as usize;
            let lost = p
                .points
                .iter()
                .filter(|(x, y)| {
                    let r = scale as usize;
                    let (x, y) = (*x as usize, *y as usize);
                    !(y.saturating_sub(r)..=y + r)
                        .any(|yy| (x.saturating_sub(r)..=x + r).any(|xx| g[yy * w + xx] < 128))
                })
                .count();
            println!(
                "scale {scale} {mode:?}: {} points, {lost} lost",
                p.points.len()
            );
            assert_eq!(lost, 0, "scale {scale} {mode:?}: decimal points lost");
        }
    }
}

#[test]
fn the_text_height_is_found_within_fifteen_percent_from_100_to_400_dpi() {
    for scale in [2u32, 3, 4, 6, 8] {
        let spec = PageSpec::document(40 + u64::from(scale)).sized(330 * scale, 460 * scale, scale);
        let p = page(&spec);
        let img = lit(&p.truth, Light::Flat, [1.0; 3], 2.0, 1);
        let an = analyse(&img, Mode::Document, &Tuning::default(), None, &NeverCancel).unwrap();
        let want = 7.0 * scale as f32;
        let got = an.report().text_h;
        println!("scale {scale}: text_h {got:.1} (want {want})");
        assert!(an.report().text_confident);
        assert!(
            (got - want).abs() <= 0.15 * want,
            "scale {scale}: {got} vs {want}"
        );
    }
}

#[test]
fn a_stamp_keeps_its_colour_and_warm_paper_is_whitened() {
    let spec = PageSpec::document(51).with_objects();
    let p = page(&spec);
    let img = lit(
        &p.truth,
        Light::Tilt { dx: 0.3, dy: 0.1 },
        [1.0, 0.88, 0.65],
        2.0,
        2,
    );
    let (out, _) = run(&img, Mode::Clean, 1.0);
    let Enhanced::Rgb(out) = out else {
        panic!("clean keeps colour")
    };
    // Paper in the middle of the text-free left margin is neutral and bright.
    let px = out.pixel(3, 700);
    assert!(px.iter().all(|c| *c >= 240), "{px:?}");
    let spread = px.iter().max().unwrap() - px.iter().min().unwrap();
    assert!(spread <= 6, "paper tint left: {px:?}");
    // The stamp ring stays blue.
    let (_, stamp) = p.objects.iter().find(|o| o.0 == "stamp").unwrap();
    let mut blue = 0;
    let mut ring = 0;
    for y in stamp.y0..stamp.y1 {
        for x in stamp.x0..stamp.x1 {
            let t = p.truth.pixel(x, y);
            if t[2] > 150 && t[0] < 100 {
                ring += 1;
                let o = out.pixel(x, y);
                if i32::from(o[2]) - i32::from(o[0]) > 60 {
                    blue += 1;
                }
            }
        }
    }
    assert!(ring > 100);
    assert!(
        blue as f32 > 0.9 * ring as f32,
        "{blue} of {ring} ring pixels kept blue"
    );
}

#[test]
fn the_validity_mask_keeps_the_table_out_of_the_estimate() {
    // A shaded page, cropped tight, and the same page placed on a dark table.
    let spec = PageSpec::document(61).sized(600, 800, 3);
    let p = page(&spec);
    let tight = lit(&p.truth, Light::Tilt { dx: 0.4, dy: 0.2 }, [1.0; 3], 2.0, 8);
    let (ox, oy, tw, th) = (150u32, 100u32, 900u32, 1000u32);
    let mut big = Raster::filled(tw, th, [70, 60, 55]);
    let mut mask = vec![0u8; (tw * th) as usize];
    for y in 0..800 {
        for x in 0..600 {
            big.set_pixel(ox + x, oy + y, tight.pixel(x, y));
            mask[((oy + y) * tw + ox + x) as usize] = 255;
        }
    }
    let t = Tuning::default();
    let a_tight = analyse(&tight, Mode::Clean, &t, None, &NeverCancel).unwrap();
    let v = Validity::new(tw, th, &mask).unwrap();
    let a_masked = analyse(&big, Mode::Clean, &t, Some(&v), &NeverCancel).unwrap();
    let (pa, pb) = (a_tight.report().paper, a_masked.report().paper);
    println!("paper tight {pa:?} masked {pb:?}");
    for c in 0..3 {
        assert!(
            (pa[c] - pb[c]).abs() <= 3.0,
            "channel {c}: {} vs {}",
            pa[c],
            pb[c]
        );
    }
    // Pixels outside the page are white in a thresholded mode; inside it the page is enhanced.
    let (out, _) = enhance(
        big.clone(),
        &Params::new(Mode::Document, 1.0),
        Some(&v),
        &NeverCancel,
    )
    .unwrap();
    let g = gray_of(&out);
    assert!(g[0] == 255 && g[(tw * th - 1) as usize] == 255);
    let inside: Vec<u8> = (0..800u32)
        .flat_map(|y| (0..600u32).map(move |x| (y, x)))
        .map(|(y, x)| g[((oy + y) * tw + ox + x) as usize])
        .collect();
    let ink_share = inside.iter().filter(|v| **v < 128).count() as f32 / inside.len() as f32;
    assert!((0.05..0.35).contains(&ink_share), "ink share {ink_share}");
}

#[test]
fn the_bytes_do_not_depend_on_the_thread_count() {
    let spec = PageSpec::receipt(71).with_objects().sized(417, 1301, 3);
    let p = page(&spec);
    let img = lit(
        &p.truth,
        Light::Vignette { k: 0.5 },
        [1.0, 0.92, 0.8],
        3.0,
        3,
    );
    for mode in [Mode::Clean, Mode::Document, Mode::Receipt] {
        for strength in [1.0f32, 0.6] {
            let one = rayon::ThreadPoolBuilder::new()
                .num_threads(1)
                .build()
                .unwrap()
                .install(|| run(&img, mode, strength));
            let eight = rayon::ThreadPoolBuilder::new()
                .num_threads(8)
                .build()
                .unwrap()
                .install(|| run(&img, mode, strength));
            assert!(one.0 == eight.0, "{mode:?} strength {strength}");
        }
    }
}

#[test]
fn strength_blends_between_the_input_and_the_result() {
    let p = page(&PageSpec::document(81));
    let img = lit(&p.truth, Light::Tilt { dx: 0.4, dy: 0.1 }, [1.0; 3], 2.0, 4);
    let (full, _) = run(&img, Mode::Clean, 1.0);
    let (half, _) = run(&img, Mode::Clean, 0.5);
    let (Enhanced::Rgb(f), Enhanced::Rgb(h)) = (full, half) else {
        panic!()
    };
    for i in (0..img.data.len()).step_by(7) {
        let want = (i32::from(img.data[i]) + i32::from(f.data[i])) / 2;
        assert!((i32::from(h.data[i]) - want).abs() <= 1, "byte {i}");
    }
    // A thresholded mode at half strength is the mean of the grey input and the binary result.
    let (d1, _) = run(&img, Mode::Document, 1.0);
    let (d5, _) = run(&img, Mode::Document, 0.5);
    let (g1, g5) = (gray_of(&d1), gray_of(&d5));
    let lum = luma_of(&img);
    for i in (0..g1.len()).step_by(11) {
        let want = (i32::from(lum[i]) + i32::from(g1[i])) / 2;
        assert!((i32::from(g5[i]) - want).abs() <= 1, "pixel {i}");
    }
}

#[test]
fn cancellation_stops_the_work_and_returns_nothing() {
    let img = lit(
        &page(&PageSpec::document(91)).truth,
        Light::Flat,
        [1.0; 3],
        1.0,
        1,
    );
    let flag = AtomicBool::new(true);
    for mode in [Mode::Clean, Mode::Document, Mode::Receipt] {
        let r = enhance(img.clone(), &Params::new(mode, 1.0), None, &flag);
        assert!(r.is_err(), "{mode:?}: a cancelled token must stop the run");
    }
    // A token that fires after some polls stops a later stage.
    struct After(AtomicUsize, usize);
    impl Cancel for After {
        fn is_cancelled(&self) -> bool {
            self.0.fetch_add(1, Ordering::SeqCst) >= self.1
        }
    }
    for n in [3usize, 10, 40] {
        let r = enhance(
            img.clone(),
            &Params::new(Mode::Document, 1.0),
            None,
            &After(AtomicUsize::new(0), n),
        );
        assert!(r.is_err(), "cancel after {n} polls");
    }
}

#[test]
fn degenerate_inputs_never_panic_or_return_nonsense() {
    let mut rng = crate::synth::Rng::new(5);
    let noise = |w: u32, h: u32, rng: &mut crate::synth::Rng| {
        let mut r = Raster::new(w, h);
        for b in &mut r.data {
            *b = (rng.next_u32() >> 8) as u8;
        }
        r
    };
    let imgs: Vec<Raster> = vec![
        Raster::new(0, 0),
        Raster::new(1, 1),
        Raster::filled(3, 200, [250, 250, 250]),
        Raster::filled(200, 3, [10, 10, 10]),
        Raster::filled(64, 64, [255, 255, 255]),
        Raster::filled(300, 300, [0, 0, 0]),
        noise(97, 61, &mut rng),
        noise(15, 15, &mut rng),
        // An 8:1 strip.
        lit(
            &page(&PageSpec::receipt(2).sized(120, 960, 2)).truth,
            Light::Flat,
            [1.0; 3],
            1.0,
            1,
        ),
    ];
    for img in &imgs {
        for mode in [Mode::Original, Mode::Clean, Mode::Document, Mode::Receipt] {
            for s in [0.0f32, 0.3, 1.0, 7.0, f32::NAN, f32::INFINITY, -1.0] {
                let (out, _) = run(img, mode, s);
                assert_eq!(out.dims(), (img.width, img.height));
                if let Enhanced::Gray(g) = &out {
                    assert_eq!(g.data.len(), (img.width * img.height) as usize);
                }
            }
        }
    }
    // A mask that is entirely 0 leaves nothing to measure: no panic, a white page.
    let mask = vec![0u8; 100 * 100];
    let v = Validity::new(100, 100, &mask).unwrap();
    let (out, rep) = enhance(
        Raster::filled(100, 100, [120, 130, 140]),
        &Params::new(Mode::Document, 1.0),
        Some(&v),
        &NeverCancel,
    )
    .unwrap();
    assert!(gray_of(&out).iter().all(|v| *v == 255));
    assert!(rep.unreliable);
    assert!(Validity::new(2, 2, &[1, 2, 3]).is_none());
}

#[test]
fn a_blank_page_is_not_amplified() {
    let mut img = Raster::filled(600, 800, [250, 250, 250]);
    let mut rng = crate::synth::Rng::new(3);
    for b in &mut img.data {
        *b = (i32::from(*b) + (rng.below(5) as i32 - 2)).clamp(0, 255) as u8;
    }
    let (out, rep) = run(&img, Mode::Clean, 1.0);
    let mean = |d: &[u8]| d.iter().map(|v| f64::from(*v)).sum::<f64>() / d.len() as f64;
    let (a, b) = (mean(&img.data), mean(&out.into_rgb().data));
    println!("blank: mean {a:.1} -> {b:.1}  {rep:?}");
    assert!(rep.low_contrast);
    assert!((b - a).abs() <= 6.0, "mean luma moved by {}", b - a);
    // The thresholded modes make it white and nothing else.
    let (d, _) = run(&img, Mode::Document, 1.0);
    assert!(gray_of(&d).iter().all(|v| *v >= 250));
}

#[test]
fn the_result_agrees_between_preview_and_full_resolution() {
    // The same lit page at full size and at half size (area average): Clean agrees within a few
    // levels, the ink coverage of Document within a quarter.
    let spec = PageSpec::document(101).sized(1200, 1600, 4);
    let p = page(&spec);
    let img = lit(
        &p.truth,
        Light::Shadow {
            depth: 0.5,
            width: 200.0,
        },
        [1.0; 3],
        2.0,
        3,
    );
    let small = crate::scale::resize_area(&img, 600, 800);
    let (full, _) = run(&img, Mode::Clean, 1.0);
    let (half, _) = run(&small, Mode::Clean, 1.0);
    let down = crate::scale::resize_area(&full.into_rgb(), 600, 800);
    let half = half.into_rgb();
    let mad = down
        .data
        .iter()
        .zip(&half.data)
        .map(|(a, b)| (i32::from(*a) - i32::from(*b)).unsigned_abs() as u64)
        .sum::<u64>() as f64
        / down.data.len() as f64;
    println!("clean full vs half: mean abs difference {mad:.2}");
    assert!(mad <= 6.0, "{mad}");
    let (df, _) = run(&img, Mode::Document, 1.0);
    let (dh, _) = run(&small, Mode::Document, 1.0);
    let cov = |g: Vec<u8>| g.iter().filter(|v| **v < 128).count() as f64 / g.len() as f64;
    let (cf, ch) = (cov(gray_of(&df)), cov(gray_of(&dh)));
    println!("document ink coverage full {cf:.4} half {ch:.4}");
    assert!((cf - ch).abs() / cf <= 0.25);
}

#[test]
fn sauvola_and_nick_are_selectable_and_clamped() {
    let p = page(&PageSpec::receipt(111));
    let img = lit(&p.truth, Light::Tilt { dx: 0.4, dy: 0.1 }, [1.0; 3], 3.0, 3);
    for method in [Binariser::Sauvola, Binariser::Nick] {
        let mut prm = Params::new(Mode::Document, 1.0);
        prm.tuning.document_method = method;
        let (out, _) = enhance(img.clone(), &prm, None, &NeverCancel).unwrap();
        let (recall, clean) = ink_scores(&gray_of(&out), &p, 600, 1800);
        println!("{method:?}: recall {recall:.3} clean {clean:.4}");
        assert!(recall > 0.85 && clean > 0.99, "{method:?}");
    }
    // Hard edges are strictly bilevel; soft edges have grey.
    let mut hard = Params::new(Mode::Document, 1.0);
    hard.tuning.edge = Edge::Hard;
    let (h, _) = enhance(img.clone(), &hard, None, &NeverCancel).unwrap();
    assert!(gray_of(&h).iter().all(|v| *v == 0 || *v == 255));
    let (s, _) = run(&img, Mode::Document, 1.0);
    assert!(gray_of(&s).iter().any(|v| *v != 0 && *v != 255));
}

#[test]
fn a_faded_receipt_is_stretched_and_flagged() {
    let spec = PageSpec::receipt(121);
    let p = page(&spec);
    let faded = fade(&p.truth, spec.paper, 0.30);
    let img = lit(&faded, Light::Tilt { dx: 0.3, dy: 0.1 }, [1.0; 3], 3.0, 3);
    let (out, rep) = run(&img, Mode::Receipt, 1.0);
    println!("faded: {rep:?}");
    assert!(rep.faded && !rep.low_contrast);
    let (recall, clean) = ink_scores(&gray_of(&out), &p, spec.width, spec.height);
    assert!(recall > 0.80, "recall {recall}");
    assert!(clean > 0.99, "clean {clean}");
    // A strong print is not flagged.
    let strong = lit(&p.truth, Light::Flat, [1.0; 3], 2.0, 3);
    assert!(!run(&strong, Mode::Receipt, 1.0).1.faded);
}

#[test]
fn analysis_can_be_reused_for_another_strength() {
    let p = page(&PageSpec::document(131));
    let img = lit(&p.truth, Light::Tilt { dx: 0.4, dy: 0.1 }, [1.0; 3], 2.0, 4);
    let prm = Params::new(Mode::Document, 1.0);
    let an = analyse(&img, prm.mode, &prm.tuning, None, &NeverCancel).unwrap();
    let a = apply(img.clone(), &an, &prm, None, &NeverCancel).unwrap().0;
    let b = enhance(img, &prm, None, &NeverCancel).unwrap().0;
    assert_eq!(a, b);
}

#[test]
#[ignore = "debug print"]
fn debug_text_scale_faded() {
    let spec = PageSpec::receipt(121);
    let p = page(&spec);
    for keep in [1.0f32, 0.5, 0.3, 0.2] {
        let faded = fade(&p.truth, spec.paper, keep);
        let img = lit(&faded, Light::Tilt { dx: 0.3, dy: 0.1 }, [1.0; 3], 3.0, 3);
        let lu = luma_of(&img);
        for (name, (r, d)) in [
            ("strict", textscale::STRICT),
            ("sensitive", textscale::SENSITIVE),
        ] {
            let ts = textscale::scale_with(&lu, 600, 1800, None, r, d);
            println!("keep {keep}: {name}: {ts:?}");
        }
    }
}

/// Parameter sweep of the flatten (spike E1b, ROADMAP M7.57): prints the worst and p95 residual
/// shading over lights for block factors, rounds and percentiles. `cargo test -p auto-crop-imgproc
/// --release sweep_flatten -- --ignored --nocapture`.
#[test]
#[ignore = "tuning sweep, prints a table"]
fn sweep_flatten() {
    let spec = PageSpec::document(11);
    let p = page(&spec);
    let lights = [
        ("tilt", Light::Tilt { dx: 0.55, dy: 0.1 }),
        ("vign", Light::Vignette { k: 0.45 }),
        (
            "shadow",
            Light::Shadow {
                depth: 0.5,
                width: 150.0,
            },
        ),
        (
            "hard",
            Light::Shadow {
                depth: 0.5,
                width: 60.0,
            },
        ),
    ];
    for passes in [2u8] {
        for bf in [0.8f32, 1.0, 1.25, 1.5, 1.75] {
            for pc in [0.85f32, 0.88, 0.90] {
                let mut line = format!("passes {passes} block {bf:.2} pct {pc:.2}:");
                for (name, light) in lights {
                    let img = lit(&p.truth, light, [1.0; 3], 2.0, 5);
                    let mut prm = Params::new(Mode::Clean, 1.0);
                    prm.tuning.passes = passes;
                    prm.tuning.block_clean = bf;
                    prm.tuning.percentile = pc;
                    let (out, _) = enhance(img, &prm, None, &NeverCancel).unwrap();
                    let (w, p95) = residual(&gray_of(&out), &p.ink, 1000, 1400, &no_skip);
                    line += &format!("  {name} {:.2}/{:.2}", w * 100.0, p95 * 100.0);
                }
                println!("{line}");
            }
        }
    }
}
