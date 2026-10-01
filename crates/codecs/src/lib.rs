// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Image decoders and encoders (placeholder; real codecs arrive in M1/M6).

/// Placeholder: formats this build can decode. Empty until M1.
pub fn supported_input_formats() -> &'static [&'static str] {
    &[]
}

#[cfg(test)]
mod tests {
    #[test]
    fn no_formats_yet() {
        assert!(super::supported_input_formats().is_empty());
    }
}
