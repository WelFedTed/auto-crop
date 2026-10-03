// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Scenes drawn in code (no image files, nothing from `_data`): rectangles on a bed with a soft
//! shadow, noise, rotation, gaps from wide to none.

use super::geom::convex_iou;
use super::*;

struct Item {
    c: (f64, f64),
    w: f64,
    h: f64,
    angle_deg: f64,
    colour: [u8; 3],
    /// A 4 px lighter border, like a print's white border.
    border: Option<[u8; 3]>,
}

fn hash(x: u32, y: u32, s: u32) -> f32 {
    let mut v =
        x.wrapping_mul(0x9E37_79B1) ^ y.wrapping_mul(0x85EB_CA77) ^ s.wrapping_mul(0xC2B2_AE3D);
    v ^= v >> 15;
    v = v.wrapping_mul(0x2C1B_3C6D);
    v ^= v >> 12;
    (v & 0xFFFF) as f32 / 65535.0 - 0.5
}

fn scene(w: u32, h: u32, bed: [u8; 3], items: &[Item], shadow: bool) -> Raster {
    let mut r = Raster::filled(w, h, bed);
    for it in items {
        let a = it.angle_deg.to_radians();
        let (ca, sa) = (a.cos(), a.sin());
        for y in 0..h {
            for x in 0..w {
                // soft shadow: offset (3, 4), measured with a smooth falloff
                let local = |dx: f64, dy: f64| {
                    let (px, py) = (f64::from(x) - dx - it.c.0, f64::from(y) - dy - it.c.1);
                    (px * ca + py * sa, -px * sa + py * ca)
                };
                let (lx, ly) = local(0.0, 0.0);
                let inside = lx.abs() <= it.w / 2.0 && ly.abs() <= it.h / 2.0;
                if inside {
                    let edge = (it.w / 2.0 - lx.abs()).min(it.h / 2.0 - ly.abs());
                    let col = match it.border {
                        Some(b) if edge < 4.0 => b,
                        _ => it.colour,
                    };
                    // A little texture so that the interior is not flat.
                    let t = (hash(x, y, 7) * 14.0) as i32
                        + (((f64::from(x) * 0.3).sin() * 10.0) as i32);
                    let px = col.map(|c| (i32::from(c) + t).clamp(0, 255) as u8);
                    r.set_pixel(x, y, px);
                } else if shadow {
                    let (sx, sy) = local(3.0, 4.0);
                    let d = (it.w / 2.0 - sx.abs()).min(it.h / 2.0 - sy.abs());
                    if d > -6.0 {
                        let k = ((d + 6.0) / 12.0).clamp(0.0, 1.0) as f32 * 0.35;
                        let p = r.pixel(x, y);
                        r.set_pixel(x, y, p.map(|c| (f32::from(c) * (1.0 - k)) as u8));
                    }
                }
            }
        }
    }
    // sensor noise everywhere
    for y in 0..h {
        for x in 0..w {
            let n = hash(x, y, 1) * 5.0;
            let p = r.pixel(x, y);
            r.set_pixel(x, y, p.map(|c| (f32::from(c) + n).clamp(0.0, 255.0) as u8));
        }
    }
    r
}

fn photo(c: (f64, f64), w: f64, h: f64, angle: f64) -> Item {
    Item {
        c,
        w,
        h,
        angle_deg: angle,
        colour: [90, 120, 70],
        border: Some([245, 245, 242]),
    }
}

fn quad_px(it: &ItemCandidate, w: u32, h: u32) -> Vec<P> {
    it.quad
        .iter()
        .map(|p| (p.x * f64::from(w), p.y * f64::from(h)))
        .collect()
}

fn truth(it: &Item) -> Vec<P> {
    Rect {
        c: it.c,
        u: (
            it.angle_deg.to_radians().cos(),
            it.angle_deg.to_radians().sin(),
        ),
        hw: it.w / 2.0,
        hh: it.h / 2.0,
    }
    .corners()
    .to_vec()
}

fn best_iou(det: &ItemsDetection, w: u32, h: u32, t: &Item) -> f64 {
    det.items
        .iter()
        .map(|d| convex_iou(&quad_px(d, w, h), &truth(t)))
        .fold(0.0, f64::max)
}

fn opts() -> ItemsOptions {
    ItemsOptions::default()
}

#[test]
fn separated_photos_on_a_white_bed_are_found_with_tight_quads() {
    let items = [
        photo((120.0, 110.0), 150.0, 100.0, 5.0),
        photo((330.0, 120.0), 120.0, 160.0, -8.0),
        photo((170.0, 300.0), 170.0, 115.0, 0.0),
        photo((380.0, 310.0), 130.0, 130.0, 20.0),
    ];
    let img = scene(520, 420, [238, 238, 238], &items, true);
    let det = detect_items(&img, &opts());
    assert_eq!(det.items.len(), 4, "{:?}", det.scan_flags);
    for it in &items {
        let iou = best_iou(&det, 520, 420, it);
        assert!(iou > 0.96, "iou {iou}");
    }
    assert_eq!(det.scan_flags.outcome, Outcome::Many(4));
    assert!(det.scan_flags.bed_like);
}

#[test]
fn touching_photos_are_never_accepted() {
    let items = [
        photo((150.0, 200.0), 200.0, 150.0, 0.0),
        photo((350.5, 200.0), 200.0, 150.0, 0.0),
    ];
    let img = scene(520, 400, [238, 238, 238], &items, true);
    let det = detect_items(&img, &opts());
    assert!(!det.items.is_empty());
    assert!(!det.auto_accept(0.95), "{:?}", det.scan_confidence());
    let codes: Vec<ReasonCode> = det
        .scan_confidence()
        .reasons
        .iter()
        .map(|r| r.code)
        .collect();
    assert!(
        codes.contains(&ReasonCode::TouchingItems) || codes.contains(&ReasonCode::ItemsTooClose),
        "{codes:?}"
    );
}

#[test]
fn an_overlap_stays_one_flagged_cluster() {
    let items = [
        photo((170.0, 190.0), 190.0, 140.0, 0.0),
        photo((290.0, 230.0), 170.0, 130.0, 30.0),
    ];
    let img = scene(520, 400, [238, 238, 238], &items, true);
    let det = detect_items(&img, &opts());
    assert!(!det.auto_accept(0.95));
    assert!(
        det.items
            .iter()
            .any(|i| i.kind == ItemKind::Cluster || i.confidence.forced.is_some())
    );
}

#[test]
fn a_close_but_clear_gap_is_accepted_and_a_tiny_one_is_not() {
    let mk = |gap: f64| {
        [
            photo((150.0, 200.0), 200.0, 150.0, 0.0),
            photo((250.0 + 100.0 + gap, 200.0), 200.0, 150.0, 0.0),
        ]
    };
    let wide = scene(600, 400, [170, 170, 170], &mk(26.0), true);
    let det = detect_items(&wide, &opts());
    assert_eq!(det.items.len(), 2, "{:?}", det.scan_flags);
    let tight = scene(600, 400, [170, 170, 170], &mk(2.0), true);
    let det = detect_items(&tight, &opts());
    assert!(!det.auto_accept(0.95));
}

#[test]
fn an_empty_bed_has_no_items_and_is_held() {
    let img = scene(400, 300, [30, 30, 30], &[], false);
    let det = detect_items(&img, &opts());
    assert!(det.items.is_empty());
    assert_eq!(det.scan_flags.outcome, Outcome::NoItems);
    assert!(
        det.scan_flags
            .reasons
            .iter()
            .any(|r| r.code == ReasonCode::NoDocument)
    );
    assert!(!det.auto_accept(0.5));
}

#[test]
fn never_means_the_single_item_route_and_one_item_is_one() {
    let items = [photo((200.0, 150.0), 300.0, 200.0, 0.0)];
    let img = scene(400, 300, [20, 20, 20], &items, false);
    let never = detect_items(
        &img,
        &ItemsOptions {
            policy: SplitPolicy::Never,
            ..opts()
        },
    );
    assert!(never.items.is_empty());
    assert_eq!(never.scan_flags.outcome, Outcome::SingleItemRoute);
    // One ordinary item with a bed margin is `One`, cropped from this quad.
    let items = [photo((200.0, 150.0), 300.0, 200.0, 0.0)];
    let img = scene(400, 300, [20, 20, 20], &items, false);
    let det = detect_items(&img, &opts());
    assert_eq!(det.items.len(), 1);
    assert_eq!(det.scan_flags.outcome, Outcome::One);
}

#[test]
fn a_clipped_item_is_flagged_partial_frame() {
    let items = [
        photo((120.0, 110.0), 150.0, 100.0, 0.0),
        photo((470.0, 300.0), 200.0, 160.0, 0.0),
    ];
    let img = scene(520, 400, [20, 20, 22], &items, false);
    let det = detect_items(&img, &opts());
    assert_eq!(det.items.len(), 2);
    assert!(det.items.iter().any(|i| {
        i.partial_frame
            && i.confidence
                .reasons
                .iter()
                .any(|r| r.code == ReasonCode::PartialFrame)
    }));
    assert!(!det.auto_accept(0.95));
}

#[test]
fn reading_order_is_rows_then_left_to_right_and_detection_is_deterministic() {
    let items = [
        photo((380.0, 300.0), 130.0, 110.0, 0.0),
        photo((120.0, 100.0), 130.0, 110.0, 0.0),
        photo((380.0, 105.0), 130.0, 110.0, 0.0),
        photo((120.0, 305.0), 130.0, 110.0, 0.0),
    ];
    let img = scene(520, 420, [238, 238, 238], &items, true);
    let a = detect_items(&img, &opts());
    let b = detect_items(&img, &opts());
    assert_eq!(a.items.len(), 4);
    let cx: Vec<f64> = a
        .items
        .iter()
        .map(|i| i.quad.iter().map(|p| p.x).sum::<f64>() / 4.0)
        .collect();
    let cy: Vec<f64> = a
        .items
        .iter()
        .map(|i| i.quad.iter().map(|p| p.y).sum::<f64>() / 4.0)
        .collect();
    assert!(
        cy[0] < 0.5 && cy[1] < 0.5 && cy[2] > 0.5 && cy[3] > 0.5,
        "{cy:?}"
    );
    assert!(cx[0] < cx[1] && cx[2] < cx[3], "{cx:?}");
    for (x, y) in a.items.iter().zip(&b.items) {
        assert_eq!(x.quad, y.quad);
        assert_eq!(x.confidence, y.confidence);
    }
}

#[test]
fn dust_and_a_hairline_are_not_items() {
    let items = [photo((200.0, 150.0), 180.0, 130.0, 3.0)];
    let mut img = scene(520, 400, [238, 238, 238], &items, true);
    for k in 0..30u32 {
        let (x, y) = (hash(k, 3, 9) + 0.5, hash(k, 4, 9) + 0.5);
        let (x, y) = ((x * 500.0) as u32 + 8, (y * 380.0) as u32 + 8);
        if (x as f64 - 200.0).abs() > 110.0 || (y as f64 - 150.0).abs() > 90.0 {
            img.set_pixel(x, y, [60, 60, 60]);
            img.set_pixel(x + 1, y, [60, 60, 60]);
        }
    }
    for x in 280..500u32 {
        img.set_pixel(x, 330 + (x / 60), [90, 90, 90]);
    }
    let det = detect_items(&img, &opts());
    assert_eq!(det.items.len(), 1, "{:?}", det.diagnostics.rejected.len());
}

#[test]
fn thirty_three_small_items_are_capped_and_flagged() {
    let mut items = Vec::new();
    for r in 0..6 {
        for c in 0..6 {
            if items.len() < 33 {
                items.push(Item {
                    c: (60.0 + c as f64 * 90.0, 50.0 + r as f64 * 80.0),
                    w: 50.0,
                    h: 45.0,
                    angle_deg: 0.0,
                    colour: [60, 90, 150],
                    border: None,
                });
            }
        }
    }
    let img = scene(640, 520, [240, 240, 240], &items, false);
    let det = detect_items(
        &img,
        &ItemsOptions {
            min_area_frac: 0.002,
            ..opts()
        },
    );
    assert_eq!(det.items.len(), 32);
    assert!(
        det.scan_flags
            .reasons
            .iter()
            .any(|r| r.code == ReasonCode::TooManyItems)
    );
    assert!(!det.auto_accept(0.5));
}
