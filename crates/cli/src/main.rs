// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Headless command-line tool (placeholder).

fn banner() -> String {
    format!(
        "auto-crop {} (pre-alpha, no commands yet)",
        env!("CARGO_PKG_VERSION")
    )
}

fn main() {
    println!("{}", banner());
}

#[cfg(test)]
mod tests {
    #[test]
    fn banner_names_the_tool() {
        assert!(super::banner().starts_with("auto-crop "));
    }
}
