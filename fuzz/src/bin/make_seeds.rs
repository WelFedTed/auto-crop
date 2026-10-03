// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `make-seeds <dir>`: writes the seed corpus of every target to `<dir>/<target>/` (default `seeds`,
//! ignored by version control; cargo-fuzz keeps what it discovers in `corpus/<target>`).

fn main() {
    let dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "seeds".to_owned());
    match auto_crop_fuzz::seeds::write_all(std::path::Path::new(&dir)) {
        Ok(n) => println!("wrote {n} seeds below {dir}"),
        Err(e) => {
            eprintln!("make-seeds: {e}");
            std::process::exit(1);
        }
    }
}
