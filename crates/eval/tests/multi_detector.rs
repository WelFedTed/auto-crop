// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The multi-item detector through the harness: files on disk, a manifest with `items`, results
//! byte-identical at one and eight threads. Scenes are drawn in code; nothing here reads `_data`.

use auto_crop_codecs::{Format, encode};
use auto_crop_eval::detector::ItemsDetectorPredictor;
use auto_crop_eval::manifest;
use auto_crop_eval::multi::{MultiRunConfig, run, to_json};
use auto_crop_imgproc::Raster;

const W: u32 = 360;
const H: u32 = 280;

fn scene(seed: u32) -> (Raster, Vec<[[f64; 2]; 4]>) {
    let mut img = Raster::filled(W, H, [24, 26, 28]);
    let mut quads = Vec::new();
    let boxes = [
        (30 + seed * 3, 30, 120u32, 90u32),
        (190, 40 + seed * 2, 130, 100),
        (60, 160, 150, 90),
    ];
    for (x, y, w, h) in boxes {
        for yy in y..y + h {
            for xx in x..x + w {
                let v = 190 + ((xx * 7 + yy * 3 + seed) % 40) as u8;
                img.set_pixel(xx, yy, [v, v - 20, v - 60]);
            }
        }
        let (x0, y0) = (f64::from(x) / f64::from(W), f64::from(y) / f64::from(H));
        let (x1, y1) = (
            f64::from(x + w) / f64::from(W),
            f64::from(y + h) / f64::from(H),
        );
        quads.push([[x0, y0], [x1, y0], [x1, y1], [x0, y1]]);
    }
    (img, quads)
}

#[test]
fn the_detector_runs_through_the_harness_and_is_deterministic_across_threads() {
    let dir = std::env::temp_dir().join(format!("ac-eval-multi-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("images")).expect("dir");
    let mut text = String::new();
    for i in 0..6u32 {
        let (img, quads) = scene(i);
        let png = encode(&img, Format::Png, 90, None).expect("encode");
        std::fs::write(dir.join(format!("images/s{i}.png")), png).expect("write");
        let row = serde_json::json!({
            "v": 1, "id": format!("s{i}"), "image": format!("images/s{i}.png"),
            "scene_id": format!("sc{i}"), "split": "dev", "width": W, "height": H,
            "quad": quads[0], "items": quads,
            "tags": {"separation": "separated", "bed": "black"},
        });
        text.push_str(&row.to_string());
        text.push('\n');
    }
    std::fs::write(dir.join("manifest.jsonl"), &text).expect("manifest");
    let m = manifest::load(&dir.join("manifest.jsonl")).expect("loads");
    let p = ItemsDetectorPredictor::default();
    let cfg = |threads| MultiRunConfig {
        threads,
        commit: "t".to_owned(),
        suite: "unit".to_owned(),
        split: "all".to_owned(),
    };
    let a = run(&m, &p, &cfg(1)).expect("runs");
    let b = run(&m, &p, &cfg(8)).expect("runs");
    assert_eq!(to_json(&a), to_json(&b));
    // Three clean boxes on a plain bed: found, exact, no crash.
    assert_eq!(a.summary.crashed, 0);
    assert_eq!(a.summary.exact_count_rate, Some(1.0));
    assert!(a.summary.item_recall.expect("recall") > 0.99);
    std::fs::remove_dir_all(&dir).ok();
}
