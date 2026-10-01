// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Thin desktop shell. The only crate allowed to depend on Tauri, and only behind the `gui`
//! feature. Every decision lives in `auto-crop-engine`; this crate maps its API onto Tauri
//! commands, the `acimg` image scheme, native pickers and drag-and-drop (PLAN 2.5, 8.6.4).
//!
//! The webview never gets a path it can act on: pickers and drops run here, items are opaque ids.

/// Placeholder kept so the crate has something to test without the GUI feature.
pub fn engine_ready() -> bool {
    auto_crop_engine::new_edit_state().version >= 1
}

/// Parses `/<token>/<id>/<kind>` (the path of an `acimg` request). Strict: anything else is
/// `None`, and no part of the URL is ever used as a filesystem path.
pub fn parse_image_path(path: &str, token: &str) -> Option<(u32, auto_crop_engine::ImageKind)> {
    let mut parts = path.trim_start_matches('/').split('/');
    let (t, id, kind) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || t.len() != token.len() || t != token {
        return None;
    }
    if id.is_empty() || id.len() > 10 || !id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((id.parse().ok()?, auto_crop_engine::ImageKind::parse(kind)?))
}

#[cfg(feature = "gui")]
mod app;

/// Starts the app (blocks until the window closes).
#[cfg(feature = "gui")]
pub fn run() {
    app::run();
}

#[cfg(test)]
mod tests {
    use super::*;
    use auto_crop_engine::ImageKind;

    #[test]
    fn engine_is_reachable() {
        assert!(engine_ready());
    }

    #[test]
    fn the_image_path_parser_is_strict() {
        let t = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            parse_image_path(&format!("/{t}/7/thumb"), t),
            Some((7, ImageKind::Thumb))
        );
        assert_eq!(
            parse_image_path(&format!("{t}/12/result"), t),
            Some((12, ImageKind::Result))
        );
        assert_eq!(
            parse_image_path(&format!("/{t}/1/src"), t),
            Some((1, ImageKind::Src))
        );
        // Wrong token, traversal, extra segments, junk ids and unknown kinds are all refused.
        assert_eq!(
            parse_image_path(&format!("/{}/7/thumb", "0".repeat(32)), t),
            None
        );
        assert_eq!(parse_image_path(&format!("/{t}/../7/thumb"), t), None);
        assert_eq!(parse_image_path(&format!("/{t}/7/thumb/x"), t), None);
        assert_eq!(parse_image_path(&format!("/{t}/-1/thumb"), t), None);
        assert_eq!(
            parse_image_path(&format!("/{t}/99999999999/thumb"), t),
            None
        );
        assert_eq!(parse_image_path(&format!("/{t}/7/other"), t), None);
        assert_eq!(parse_image_path(&format!("/{t}/7"), t), None);
        assert_eq!(parse_image_path("", t), None);
    }
}
