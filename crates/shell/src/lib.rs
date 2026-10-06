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

/// What an `acimg` request asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImagePath {
    /// `/<token>/<id>/<thumb|src|result>`: the whole image.
    Whole(u32, auto_crop_engine::ImageKind),
    /// `/<token>/<id>/crop/<crop>/<thumb|result>`: one crop of a scan.
    Crop(u32, u32, auto_crop_engine::CropImage),
}

/// A numeric id: digits only, at most 10 of them (so it fits a `u32` or is refused).
fn parse_id(s: &str) -> Option<u32> {
    if s.is_empty() || s.len() > 10 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// Parses the path of an `acimg` request: `/<token>/<id>/<kind>` or
/// `/<token>/<id>/crop/<crop>/<kind>` (the query, such as the `?k=<renderKey>` cache buster, is
/// not part of the path and is ignored). Strict: anything else is `None`, and no part of the URL
/// is ever used as a filesystem path.
pub fn parse_image_path(path: &str, token: &str) -> Option<ImagePath> {
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let (t, id, rest) = match parts.as_slice() {
        [t, id, rest @ ..] => (*t, *id, rest),
        _ => return None,
    };
    if t.len() != token.len() || t != token {
        return None;
    }
    let id = parse_id(id)?;
    match rest {
        [kind] => Some(ImagePath::Whole(
            id,
            auto_crop_engine::ImageKind::parse(kind)?,
        )),
        ["crop", crop, kind] => {
            let kind = match *kind {
                "thumb" => auto_crop_engine::CropImage::Thumb,
                "result" => auto_crop_engine::CropImage::Result,
                _ => return None,
            };
            Some(ImagePath::Crop(id, parse_id(crop)?, kind))
        }
        _ => None,
    }
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
    use auto_crop_engine::{CropImage, ImageKind};

    #[test]
    fn engine_is_reachable() {
        assert!(engine_ready());
    }

    #[test]
    fn the_image_path_parser_is_strict() {
        let t = "0123456789abcdef0123456789abcdef";
        let p = |path: String| parse_image_path(&path, t);
        assert_eq!(
            p(format!("/{t}/7/thumb")),
            Some(ImagePath::Whole(7, ImageKind::Thumb))
        );
        assert_eq!(
            p(format!("{t}/12/result")),
            Some(ImagePath::Whole(12, ImageKind::Result))
        );
        assert_eq!(
            p(format!("/{t}/1/src")),
            Some(ImagePath::Whole(1, ImageKind::Src))
        );
        // One crop of a scan: thumb or result, never src.
        assert_eq!(
            p(format!("/{t}/7/crop/3/thumb")),
            Some(ImagePath::Crop(7, 3, CropImage::Thumb))
        );
        assert_eq!(
            p(format!("/{t}/7/crop/12/result")),
            Some(ImagePath::Crop(7, 12, CropImage::Result))
        );
        assert_eq!(p(format!("/{t}/7/crop/3/src")), None);
        assert_eq!(p(format!("/{t}/7/crop/3/other")), None);
        // Missing parts.
        assert_eq!(p(format!("/{t}/7/crop/thumb")), None);
        assert_eq!(p(format!("/{t}/7/crop/3")), None);
        assert_eq!(p(format!("/{t}/7/crop")), None);
        assert_eq!(p(format!("/{t}/7/crop//thumb")), None);
        assert_eq!(p(format!("/{t}/7")), None);
        assert_eq!(p(format!("/{t}")), None);
        assert_eq!(p(format!("/{t}//thumb")), None);
        // Extra parts, and a "crop" form that is not exactly `crop/<n>/<kind>`.
        assert_eq!(p(format!("/{t}/7/crop/3/thumb/x")), None);
        assert_eq!(p(format!("/{t}/7/thumb/x")), None);
        assert_eq!(p(format!("/{t}/7/crops/3/thumb")), None);
        assert_eq!(p(format!("/{t}/7/Crop/3/thumb")), None);
        assert_eq!(p(format!("/{t}/7/thumb/")), None);
        // Wrong token, traversal, junk ids and unknown kinds are all refused.
        assert_eq!(p(format!("/{}/7/thumb", "0".repeat(32))), None);
        assert_eq!(p(format!("/{}/7/crop/3/thumb", "0".repeat(32))), None);
        assert_eq!(p(format!("/{t}x/7/thumb")), None);
        assert_eq!(p(format!("/{t}/../7/thumb")), None);
        assert_eq!(p(format!("/{t}/7/crop/../thumb")), None);
        assert_eq!(p(format!("/{t}/7/crop/../../thumb")), None);
        assert_eq!(p(format!("/{t}/-1/thumb")), None);
        assert_eq!(p(format!("/{t}/+1/thumb")), None);
        assert_eq!(p(format!("/{t}/1.5/thumb")), None);
        assert_eq!(p(format!("/{t}/7/crop/-1/thumb")), None);
        assert_eq!(p(format!("/{t}/7/crop/x/thumb")), None);
        assert_eq!(p(format!("/{t}/7/crop/3 /thumb")), None);
        assert_eq!(p(format!("/{t}/99999999999/thumb")), None);
        assert_eq!(p(format!("/{t}/99999999999/crop/3/thumb")), None);
        assert_eq!(p(format!("/{t}/7/crop/99999999999/thumb")), None);
        // Ten digits that do not fit a u32.
        assert_eq!(p(format!("/{t}/9999999999/thumb")), None);
        assert_eq!(p(format!("/{t}/7/other")), None);
        assert_eq!(parse_image_path("", t), None);
    }
}
