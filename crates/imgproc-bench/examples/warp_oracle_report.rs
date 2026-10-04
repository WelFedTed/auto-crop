// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Cross-OS measurement of the warp oracles (ROADMAP M1.25, M1.78).
//!
//! `warp_oracle_report dump DIR` runs the warp kernel on every case of the two oracle fixtures
//! (`crates/imgproc/tests/fixtures/warp_*.json`: NumPy Lanczos3 and cv2 Lanczos4) and writes each
//! output as raw little-endian bytes plus a `cases.json` index into DIR. CI runs it on every OS and
//! uploads DIR; `warp_oracle_report compare DIR_A DIR_B ...` then reports, per case and per pair of
//! operating systems, whether the outputs are byte-identical and otherwise the MSSIM
//! (`image-compare`, never `dssim-core`) and the worst per-sample difference, and the MSSIM of each
//! output against the fixture's reference. Exit status 1 when a bar is missed: >= 0.99 against the
//! reference (same OS) and >= 0.98 between operating systems (ROADMAP M1.25).

use auto_crop_imgproc::cancel::NeverCancel;
use auto_crop_imgproc::pixels::ImageRef;
use auto_crop_imgproc::warp::warp_perspective_image;
use image::RgbImage;
use image_compare::{Algorithm, rgb_similarity_structure};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const SAME_OS_BAR: f64 = 0.99;
const CROSS_OS_BAR: f64 = 0.98;
const FIXTURES: [&str; 2] = ["warp_numpy_lanczos3.json", "warp_cv2_lanczos4.json"];

fn hex_bytes(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
        .collect()
}

fn fixture(name: &str) -> Value {
    let path = format!(
        "{}/../imgproc/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}")))
        .expect("fixture json")
}

fn u16s(b: &[u8]) -> Vec<u16> {
    b.as_chunks::<2>()
        .0
        .iter()
        .map(|p| u16::from_le_bytes(*p))
        .collect()
}

fn le_bytes(v: &[u16]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// FNV-1a 64: a fingerprint for the log, not a security hash.
fn fnv(b: &[u8]) -> String {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for x in b {
        h ^= u64::from(*x);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

fn mssim(a: &[u8], b: &[u8], w: u32, h: u32) -> f64 {
    let a = RgbImage::from_raw(w, h, a.to_vec()).expect("rgb a");
    let b = RgbImage::from_raw(w, h, b.to_vec()).expect("rgb b");
    rgb_similarity_structure(&Algorithm::MSSIMSimple, &a, &b)
        .expect("mssim")
        .score
}

fn dump(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut index = Vec::new();
    for file in FIXTURES {
        let doc = fixture(file);
        for c in doc["cases"].as_array().expect("cases") {
            let name = c["name"].as_str().expect("name");
            let (w, h) = (
                c["w"].as_u64().unwrap() as u32,
                c["h"].as_u64().unwrap() as u32,
            );
            let (ow, oh) = (
                c["out_w"].as_u64().unwrap() as u32,
                c["out_h"].as_u64().unwrap() as u32,
            );
            let (ch, bits) = (
                c["channels"].as_u64().unwrap() as u8,
                c["bits"].as_u64().unwrap(),
            );
            let m: [f64; 9] = c["matrix"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_f64().unwrap())
                .collect::<Vec<_>>()
                .try_into()
                .unwrap();
            let src = hex_bytes(c["src"].as_str().unwrap());
            let (ours, reference) = if bits == 8 {
                let out = warp_perspective_image(
                    ImageRef::new(w, h, ch, &src[..]).unwrap(),
                    &m,
                    ow,
                    oh,
                    &NeverCancel,
                )
                .map_err(|e| format!("{name}: {e:?}"))?;
                (out.data, hex_bytes(c["expected"].as_str().unwrap()))
            } else {
                let data = u16s(&src);
                let out = warp_perspective_image(
                    ImageRef::new(w, h, ch, &data[..]).unwrap(),
                    &m,
                    ow,
                    oh,
                    &NeverCancel,
                )
                .map_err(|e| format!("{name}: {e:?}"))?;
                (
                    le_bytes(&out.data),
                    hex_bytes(c["expected"].as_str().unwrap()),
                )
            };
            let key = format!("{}--{name}", file.trim_end_matches(".json"));
            std::fs::write(dir.join(format!("{key}.bin")), &ours).map_err(|e| e.to_string())?;
            std::fs::write(dir.join(format!("{key}.ref")), &reference)
                .map_err(|e| e.to_string())?;
            index.push(json!({
                "key": key, "w": ow, "h": oh, "channels": ch, "bits": bits,
                "fingerprint": fnv(&ours),
            }));
        }
    }
    let text = serde_json::to_string_pretty(&json!({
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "cases": index,
    }))
    .unwrap();
    std::fs::write(dir.join("cases.json"), text + "\n").map_err(|e| e.to_string())?;
    println!("wrote {} case outputs to {}", index_len(dir), dir.display());
    Ok(())
}

fn index_len(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|d| {
            d.filter(|e| {
                e.as_ref()
                    .is_ok_and(|e| e.path().extension().is_some_and(|x| x == "bin"))
            })
            .count()
        })
        .unwrap_or(0)
}

/// Worst per-sample absolute difference (in units of the sample: 8-bit levels or 16-bit levels).
fn worst_diff(a: &[u8], b: &[u8], bits: u64) -> u64 {
    if bits == 8 {
        a.iter()
            .zip(b)
            .map(|(x, y)| u64::from(x.abs_diff(*y)))
            .max()
            .unwrap_or(0)
    } else {
        u16s(a)
            .iter()
            .zip(u16s(b))
            .map(|(x, y)| u64::from(x.abs_diff(y)))
            .max()
            .unwrap_or(0)
    }
}

fn compare(dirs: &[PathBuf]) -> Result<bool, String> {
    let read_index = |d: &Path| -> Result<Value, String> {
        serde_json::from_str(
            &std::fs::read_to_string(d.join("cases.json"))
                .map_err(|e| format!("{}: {e}", d.display()))?,
        )
        .map_err(|e| e.to_string())
    };
    let indexes: Vec<Value> = dirs
        .iter()
        .map(|d| read_index(d))
        .collect::<Result<_, _>>()?;
    let label = |i: usize| {
        format!(
            "{}-{}",
            indexes[i]["os"].as_str().unwrap_or("?"),
            indexes[i]["arch"].as_str().unwrap_or("?")
        )
    };
    let mut ok = true;
    let (mut min_same, mut min_cross) = (1.0f64, 1.0f64);
    let (mut identical, mut total_pairs, mut max_lsb) = (0usize, 0usize, 0u64);
    println!("| case | OS pair | identical | worst diff | MSSIM |\n|---|---|---|---|---|");
    for case in indexes[0]["cases"].as_array().unwrap() {
        let key = case["key"].as_str().unwrap();
        let (w, h) = (
            case["w"].as_u64().unwrap() as u32,
            case["h"].as_u64().unwrap() as u32,
        );
        let (ch, bits) = (
            case["channels"].as_u64().unwrap(),
            case["bits"].as_u64().unwrap(),
        );
        let rgb8 = bits == 8 && ch == 3;
        let outs: Vec<Vec<u8>> = dirs
            .iter()
            .map(|d| std::fs::read(d.join(format!("{key}.bin"))).map_err(|e| e.to_string()))
            .collect::<Result<_, _>>()?;
        let reference =
            std::fs::read(dirs[0].join(format!("{key}.ref"))).map_err(|e| e.to_string())?;
        if rgb8 {
            for (i, o) in outs.iter().enumerate() {
                let s = mssim(o, &reference, w, h);
                min_same = min_same.min(s);
                if s < SAME_OS_BAR {
                    ok = false;
                    println!(
                        "| {key} | {} vs reference | no | | **{s:.5}** (< {SAME_OS_BAR}) |",
                        label(i)
                    );
                }
            }
        }
        for i in 0..outs.len() {
            for j in (i + 1)..outs.len() {
                total_pairs += 1;
                if outs[i] == outs[j] {
                    identical += 1;
                    continue;
                }
                let d = worst_diff(&outs[i], &outs[j], bits);
                max_lsb = max_lsb.max(d);
                let s = if rgb8 {
                    Some(mssim(&outs[i], &outs[j], w, h))
                } else {
                    None
                };
                if let Some(s) = s {
                    min_cross = min_cross.min(s);
                    if s < CROSS_OS_BAR {
                        ok = false;
                    }
                }
                println!(
                    "| {key} | {} vs {} | no | {d} | {} |",
                    label(i),
                    label(j),
                    s.map_or_else(|| "n/a".to_owned(), |s| format!("{s:.6}"))
                );
            }
        }
    }
    println!();
    println!(
        "Cross-OS pairs: {identical} of {total_pairs} case comparisons byte-identical; worst per-sample difference among the others: {max_lsb}."
    );
    println!(
        "Lowest MSSIM against the fixture reference (RGB8 cases, same OS bar {SAME_OS_BAR}): {min_same:.5}."
    );
    println!(
        "Lowest MSSIM between operating systems (RGB8 cases that differ, bar {CROSS_OS_BAR}): {}.",
        if identical == total_pairs && total_pairs > 0 {
            "1.00000 (all outputs identical)".to_owned()
        } else {
            format!("{min_cross:.6}")
        }
    );
    println!(
        "Fingerprints per OS: {}",
        (0..dirs.len())
            .map(|i| {
                let all: String = indexes[i]["cases"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|c| c["fingerprint"].as_str().unwrap().to_owned())
                    .collect();
                format!("{}={}", label(i), fnv(all.as_bytes()))
            })
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(ok)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("dump") if args.len() == 2 => dump(Path::new(&args[1])).map(|()| true),
        Some("compare") if args.len() >= 3 => {
            compare(&args[1..].iter().map(PathBuf::from).collect::<Vec<_>>())
        }
        _ => Err("usage: warp_oracle_report dump DIR | compare DIR DIR [DIR...]".to_owned()),
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => {
            eprintln!("an SSIM bar was missed");
            ExitCode::from(1)
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}
