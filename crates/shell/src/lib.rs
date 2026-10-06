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
    /// `/<token>/<id>/crop/<crop>/curve-<thumb|result>?<curves>`: the flattened preview of a
    /// candidate curve set for one crop (a drag in progress). The curves come in the query, see
    /// [`parse_curve_query`]; nothing is committed or cached.
    CurvePreview(u32, u32, auto_crop_engine::CropImage),
}

/// The longest query of a curve preview that is accepted: 4 curves of at most 32 points, 2 numbers
/// of at most 11 characters each, with room to spare.
const MAX_CURVE_QUERY: usize = 4096;

/// Parses the query of a curve preview: `t=x,y,x,y,...&r=...&b=...&l=...&q=<0..3>&m=<0|1>` (the
/// four edges top, right, bottom and left as flat lists of normalised coordinates, the quarter
/// turns and the mirror). Strict: every key exactly once, only numbers that are finite, 2 to 32
/// points per edge; anything else is `None`. The curves are NOT validated for geometry here: the
/// renderer does that and refuses what is degenerate.
pub fn parse_curve_query(query: &str) -> Option<auto_crop_engine::CurveWarp> {
    use auto_crop_engine::{Curve, CurveWarp, Pt};
    if query.is_empty() || query.len() > MAX_CURVE_QUERY {
        return None;
    }
    let (mut t, mut r, mut b, mut l) = (None, None, None, None);
    let (mut q, mut m) = (None, None);
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=')?;
        let slot = match key {
            "t" => &mut t,
            "r" => &mut r,
            "b" => &mut b,
            "l" => &mut l,
            "q" => &mut q,
            "m" => &mut m,
            _ => return None,
        };
        if slot.replace(value).is_some() {
            return None;
        }
    }
    let curve = |v: Option<&str>| -> Option<Curve> {
        let nums: Vec<f64> = v?
            .split(',')
            .map(|n| {
                // digits, one dot, an optional minus sign: no exponent, no plus, no spaces
                let ok = !n.is_empty()
                    && n.bytes()
                        .all(|c| c.is_ascii_digit() || c == b'.' || c == b'-');
                if ok {
                    n.parse::<f64>().ok().filter(|x| x.is_finite())
                } else {
                    None
                }
            })
            .collect::<Option<Vec<f64>>>()?;
        if !nums.len().is_multiple_of(2) {
            return None;
        }
        Curve::new(nums.chunks(2).map(|c| Pt::new(c[0], c[1])).collect()).ok()
    };
    let quarter_turns: u8 = q?.parse().ok().filter(|v| *v < 4)?;
    let mirror = match m? {
        "0" => false,
        "1" => true,
        _ => return None,
    };
    Some(CurveWarp {
        top: curve(t)?,
        right: curve(r)?,
        bottom: curve(b)?,
        left: curve(l)?,
        quarter_turns,
        mirror,
    })
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
        ["crop", crop, "curve-thumb"] => Some(ImagePath::CurvePreview(
            id,
            parse_id(crop)?,
            auto_crop_engine::CropImage::Thumb,
        )),
        ["crop", crop, "curve-result"] => Some(ImagePath::CurvePreview(
            id,
            parse_id(crop)?,
            auto_crop_engine::CropImage::Result,
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

    #[test]
    fn the_curve_preview_path_is_strict() {
        let t = "0123456789abcdef0123456789abcdef";
        let p = |path: String| parse_image_path(&path, t);
        assert_eq!(
            p(format!("/{t}/7/crop/3/curve-thumb")),
            Some(ImagePath::CurvePreview(7, 3, CropImage::Thumb))
        );
        assert_eq!(
            p(format!("/{t}/7/crop/12/curve-result")),
            Some(ImagePath::CurvePreview(7, 12, CropImage::Result))
        );
        for bad in [
            format!("/{t}/7/curve-thumb"),
            format!("/{t}/7/crop/curve-thumb"),
            format!("/{t}/7/crop/3/curve-src"),
            format!("/{t}/7/crop/3/curve-thumb/x"),
            format!("/{t}/7/crop/-1/curve-thumb"),
            format!("/{t}/7/crop/3/Curve-thumb"),
            format!("/{}/7/crop/3/curve-thumb", "0".repeat(32)),
        ] {
            assert_eq!(p(bad.clone()), None, "{bad}");
        }
    }

    const STRAIGHT: &str =
        "t=0.1,0.1,0.9,0.1&r=0.9,0.1,0.9,0.9&b=0.9,0.9,0.1,0.9&l=0.1,0.9,0.1,0.1&q=0&m=0";

    #[test]
    fn a_curve_query_becomes_a_curve_set() {
        let c = parse_curve_query(STRAIGHT).expect("a straight page");
        assert_eq!(c.top.points().len(), 2);
        assert_eq!((c.quarter_turns, c.mirror), (0, false));
        assert!(c.validate().is_ok());
        let bent = "t=0.1,0.1,0.5,0.06,0.9,0.1&r=0.9,0.1,0.9,0.9&b=0.9,0.9,0.1,0.9&l=0.1,0.9,0.1,0.1&q=3&m=1";
        let c = parse_curve_query(bent).expect("a bent page");
        assert_eq!(c.top.points().len(), 3);
        assert_eq!((c.quarter_turns, c.mirror), (3, true));
        assert!(c.validate().is_ok());
        // Negative numbers are legal: a page cut by the frame.
        assert!(parse_curve_query(&STRAIGHT.replace("t=0.1,0.1", "t=-0.1,0.1")).is_some());
    }

    #[test]
    fn a_curve_query_is_parsed_strictly() {
        let bad: Vec<String> = vec![
            String::new(),
            "t=0.1,0.1,0.9,0.1".into(),
            // a repeated key, an unknown key, a missing value, an odd count of numbers
            format!("{STRAIGHT}&t=0.1,0.1,0.9,0.1"),
            format!("{STRAIGHT}&x=1"),
            STRAIGHT.replace("&q=0", "&q"),
            STRAIGHT.replace("t=0.1,0.1,0.9,0.1", "t=0.1,0.1,0.9"),
            // one point is not a curve; 33 points are too many
            STRAIGHT.replace("t=0.1,0.1,0.9,0.1", "t=0.1,0.1"),
            STRAIGHT.replace(
                "t=0.1,0.1,0.9,0.1",
                &format!("t={}", vec!["0.5,0.5"; 33].join(",")),
            ),
            // numbers that are not plain decimals
            STRAIGHT.replace("0.9,0.1&r", "1e-1,0.1&r"),
            STRAIGHT.replace("0.9,0.1&r", "+0.9,0.1&r"),
            STRAIGHT.replace("0.9,0.1&r", "nan,0.1&r"),
            STRAIGHT.replace("0.9,0.1&r", "0.9, 0.1&r"),
            STRAIGHT.replace("0.9,0.1&r", "0.9,,0.1&r"),
            STRAIGHT.replace("0.9,0.1&r", "0.9.1,0.1&r"),
            // turns out of range, a mirror that is not 0 or 1
            STRAIGHT.replace("q=0", "q=4"),
            STRAIGHT.replace("q=0", "q=-1"),
            STRAIGHT.replace("m=0", "m=2"),
            STRAIGHT.replace("m=0", "m=true"),
            // far too long
            format!("{STRAIGHT}&{}", "z".repeat(5000)),
        ];
        for q in bad {
            assert!(parse_curve_query(&q).is_none(), "{q:.80}");
        }
    }

    #[test]
    fn the_query_the_webview_builds_parses_to_the_same_curves() {
        // `curveQuery` in ui/src/lib/curve.ts writes 7 decimals with trailing zeros removed.
        let q = "t=0.1,0.1,0.5,0.0612346,0.9,0.1&r=0.9,0.1,0.9,0.9&b=0.9,0.9,0.1,0.9&l=0.1,0.9,0.1,0.1&q=0&m=0";
        let c = parse_curve_query(q).unwrap();
        assert!((c.top.points()[1].y - 0.0612346).abs() < 1e-12);
    }
}
