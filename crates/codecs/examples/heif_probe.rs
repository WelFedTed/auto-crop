// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `heif_probe [--repeat N] [--threads N] [--plugin-dir DIR] <file>`: decodes a HEIC, HEIF or AVIF file through
//! libheif and prints one result line, for CI and for timing.
//!
//! Exit 0 and `OK 4000x3000 orientation=1 depth=8 frames=1 notices=[..] ms=123.4 ...` on success;
//! exit 1 and `ERR code=hevc_decoder_missing msg=...` on failure. CI runs it once with the libde265
//! plugin in place and once with the plugin directory moved away, which must give
//! `hevc_decoder_missing` for a HEVC file (the `no-hevc` build, ADR-0005, ADR-0009) while an AVIF
//! still decodes. With `--repeat N` the decode runs N times and the line reports the mean and the
//! best time in milliseconds (the first run, which loads the libraries, is not counted).
//!
//! Run: `cargo run --release -p auto-crop-codecs --features heif --example heif_probe -- file.avif`
//! (after `cargo xtask build-native`; the libraries must be findable: `PATH` on Windows,
//! `LD_LIBRARY_PATH` or the rpath of the build on Linux and macOS).

use auto_crop_codecs::{DecodeLimits, decode_with, heif, probe};
use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let (mut repeat, mut plugin_dir, mut file) = (1usize, None, None);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--repeat" => repeat = args.next().and_then(|v| v.parse().ok()).unwrap_or(1).max(1),
            "--plugin-dir" => plugin_dir = args.next().map(std::path::PathBuf::from),
            "--threads" => {
                heif::set_codec_threads(args.next().and_then(|v| v.parse().ok()).unwrap_or(0))
            }
            _ => file = Some(a),
        }
    }
    let path =
        file.expect("usage: heif_probe [--repeat N] [--threads N] [--plugin-dir DIR] <file>");
    if plugin_dir.is_some() {
        heif::configure(plugin_dir);
    }
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let limits = DecodeLimits::default();
    let p = probe(&bytes);
    let first = decode_with(&bytes, &limits);
    match first {
        Ok(d) => {
            let mut times = Vec::new();
            for _ in 1..repeat {
                let t = Instant::now();
                let r = decode_with(&bytes, &limits);
                times.push(t.elapsed().as_secs_f64() * 1000.0);
                if let Err(e) = r {
                    println!("ERR code={} msg={e}", e.code());
                    std::process::exit(1);
                }
            }
            let (mean, best) = if times.is_empty() {
                (f64::NAN, f64::NAN)
            } else {
                (
                    times.iter().sum::<f64>() / times.len() as f64,
                    times.iter().cloned().fold(f64::INFINITY, f64::min),
                )
            };
            println!(
                "OK {}x{} orientation={} depth={} frames={} icc={} nonzero={} notices={:?} \
                 probe={:?} mean_ms={mean:.1} best_ms={best:.1} libheif={} hevc={} av1={}",
                d.raster.width,
                d.raster.height,
                d.exif_orientation,
                d.source_bit_depth,
                d.frames,
                d.icc.as_ref().map_or(0, Vec::len),
                d.raster.data.iter().any(|b| *b != 0),
                d.notices,
                p.map(|p| (p.width, p.height, p.orientation)).ok(),
                heif::runtime_version(),
                heif::have_hevc_decoder(),
                heif::have_av1_decoder(),
            );
        }
        Err(e) => {
            println!("ERR code={} msg={e}", e.code());
            std::process::exit(1);
        }
    }
}
