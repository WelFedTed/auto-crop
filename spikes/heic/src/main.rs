// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `heic-probe <file>`: decode a HEIC to RGB8 and print one result line.
//! Exit 0 and `OK WxH ...` on success; exit 1 and `ERR code=.. sub=.. msg=..` on failure.
//! Used by CI to prove the plugin-present case and the no-HEVC-decoder case.

fn main() {
    let path = std::env::args().nth(1).expect("usage: heic-probe <file>");
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    match spike_heic::decode_rgb8(&bytes, true) {
        Ok(img) => {
            let nonzero = img.pixels.iter().any(|&b| b != 0);
            println!(
                "OK {}x{} bytes={} nonzero={} libheif={}",
                img.width,
                img.height,
                img.pixels.len(),
                nonzero,
                spike_heic::runtime_version()
            );
        }
        Err(e) => {
            println!("ERR code={} sub={} msg={}", e.code, e.subcode, e.message);
            std::process::exit(1);
        }
    }
}
