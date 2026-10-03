// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Reading the pinned libjpeg-turbo headers (ROADMAP M1.18). Included by `build.rs` (version gate
//! and the include path) and by the unit tests of this crate through `include!`, so the gate that
//! stops a build against a too-old library is itself tested.

/// libjpeg-turbo 3.1.4 as a version number; older releases have a known double free (ADR-0004).
pub const VERSION_FLOOR: u32 = 3_001_004;

/// `#define NAME 3002000` -> 3002000.
pub fn define_number(text: &str, name: &str) -> Option<u32> {
    text.lines().find_map(|l| {
        let mut it = l.split_whitespace();
        (it.next() == Some("#define") && it.next() == Some(name))
            .then(|| it.next())
            .flatten()?
            .parse()
            .ok()
    })
}

/// Checks the version numbers read from `jconfig.h` (`LIBJPEG_TURBO_VERSION_NUMBER`, always
/// present) and `turbojpeg.h` (`TURBOJPEG_VERSION_NUMBER`, when present). Returns the number to
/// hand to `tj3InitVersion`, or the reason the library is refused.
pub fn check_versions(jconfig: &str, turbojpeg_h: &str) -> Result<u32, String> {
    let lib = define_number(jconfig, "LIBJPEG_TURBO_VERSION_NUMBER")
        .ok_or("cannot read LIBJPEG_TURBO_VERSION_NUMBER from jconfig.h")?;
    let api = define_number(turbojpeg_h, "TURBOJPEG_VERSION_NUMBER");
    for (what, v) in [("jconfig.h", Some(lib)), ("turbojpeg.h", api)] {
        if let Some(v) = v
            && v < VERSION_FLOOR
        {
            return Err(format!(
                "libjpeg-turbo {v} ({what}) is older than 3.1.4 ({VERSION_FLOOR}); refusing to link"
            ));
        }
    }
    Ok(api.unwrap_or(lib))
}

/// True when `turbojpeg.h` declares the function `tj3InitVersion` (TurboJPEG 3.2 and later).
pub fn declares_init_version(turbojpeg_h: &str) -> bool {
    strip_comments(turbojpeg_h)
        .lines()
        .any(|l| l.contains("tj3InitVersion") && !l.trim_start().starts_with("#define"))
}

/// Strips `/* ... */` and `// ...` comments.
pub fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
        } else if b[i] == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else {
            out.push(b[i] as char);
            i += 1;
        }
    }
    out
}

/// The members of `enum <name> { ... }` with their values (implicit values count up from the
/// previous one).
pub fn enum_values(text: &str, name: &str) -> Vec<(String, i64)> {
    let clean = strip_comments(text);
    let Some(start) = clean.find(&format!("enum {name}")) else {
        return Vec::new();
    };
    let rest = &clean[start..];
    let (Some(open), Some(close)) = (rest.find('{'), rest.find('}')) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut next = 0i64;
    for item in rest[open + 1..close].split(',') {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        let (n, v) = match item.split_once('=') {
            Some((n, v)) => (n.trim(), eval(v.trim()).unwrap_or(next)),
            None => (item, next),
        };
        out.push((n.to_owned(), v));
        next = v + 1;
    }
    out
}

/// `#define NAME (1 << 3)` or `#define NAME 4` -> value.
pub fn define_value(text: &str, name: &str) -> Option<i64> {
    strip_comments(text).lines().find_map(|l| {
        let l = l.trim();
        let rest = l.strip_prefix("#define")?.trim_start();
        let rest = rest.strip_prefix(name)?;
        rest.starts_with(char::is_whitespace)
            .then(|| eval(rest.trim()))
            .flatten()
    })
}

fn eval(expr: &str) -> Option<i64> {
    let e = expr
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .trim();
    if let Some((a, b)) = e.split_once("<<") {
        return Some(eval(a)? << eval(b)?);
    }
    match e.strip_prefix("0x") {
        Some(hex) => i64::from_str_radix(hex, 16).ok(),
        None => e.parse().ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jconfig(v: u32) -> String {
        format!(
            "#define LIBJPEG_TURBO_VERSION 3.x
#define LIBJPEG_TURBO_VERSION_NUMBER {v}
"
        )
    }

    fn tj_header(v: u32) -> String {
        format!(
            "/* TurboJPEG */
#define TURBOJPEG_VERSION_NUMBER {v}
"
        )
    }

    #[test]
    fn the_pin_and_newer_releases_pass_the_gate() {
        assert_eq!(
            check_versions(&jconfig(3_002_000), &tj_header(3_002_000)),
            Ok(3_002_000)
        );
        assert_eq!(
            check_versions(&jconfig(3_001_004), &tj_header(3_001_004)),
            Ok(3_001_004)
        );
        // A header without TURBOJPEG_VERSION_NUMBER falls back to the jconfig number.
        assert_eq!(
            check_versions(&jconfig(3_002_000), "/* none */"),
            Ok(3_002_000)
        );
    }

    #[test]
    fn libjpeg_turbo_3_1_0_and_3_1_3_are_refused() {
        // ROADMAP M1.18 acceptance: the version check is red with 3.1.0 (double free fixed in 3.1.4).
        for old in [3_001_000, 3_001_003, 3_000_002, 2_001_005] {
            let e = check_versions(&jconfig(old), &tj_header(3_002_000)).unwrap_err();
            assert!(e.contains("older than 3.1.4"), "{e}");
            let e = check_versions(&jconfig(3_002_000), &tj_header(old)).unwrap_err();
            assert!(
                e.contains("older than 3.1.4") && e.contains("turbojpeg.h"),
                "{e}"
            );
        }
    }

    #[test]
    fn an_unreadable_version_is_refused_not_assumed() {
        assert!(check_versions("", &tj_header(3_002_000)).is_err());
        assert!(
            check_versions(
                "#define LIBJPEG_TURBO_VERSION_NUMBER abc
",
                ""
            )
            .is_err()
        );
        assert_eq!(
            define_number(
                "#define X 7
",
                "Y"
            ),
            None
        );
    }

    #[test]
    fn the_init_function_is_detected_from_the_header() {
        let v32 = "#define tj3Init(initType) tj3InitVersion(initType, 3)
DLLEXPORT tjhandle tj3InitVersion(int initType, int apiVersion);
";
        assert!(declares_init_version(v32));
        assert!(!declares_init_version(
            "DLLEXPORT tjhandle tj3Init(int initType);
"
        ));
        assert!(!declares_init_version(
            "/* tj3InitVersion is not here */
"
        ));
        assert!(!declares_init_version(
            "#define tj3Init(t) tj3InitVersion(t, 3)
"
        ));
    }

    const HEADER: &str = r#"
/**
 * Parameters
 */
enum TJPARAM {
  /**
   * Error handling behavior
   */
  TJPARAM_STOPONWARNING,
  TJPARAM_BOTTOMUP,
  // a line comment
  TJPARAM_NOREALLOC,
  TJPARAM_QUALITY
};

enum TJXOP { TJXOP_NONE, TJXOP_HFLIP = 1, TJXOP_VFLIP, TJXOP_ROT90 = 5, TJXOP_ROT180 };

/** Perfect */
#define TJXOPT_PERFECT  (1 << 0)
#define TJXOPT_CROP  (1 << 2)
#define TJXOPT_FLAT  4
#define TJXOPT_HEX  0x10
"#;

    #[test]
    fn enums_and_defines_are_read_with_their_values() {
        let p = enum_values(HEADER, "TJPARAM");
        let get = |n: &str| p.iter().find(|(k, _)| k == n).map(|(_, v)| *v);
        assert_eq!(get("TJPARAM_STOPONWARNING"), Some(0));
        assert_eq!(get("TJPARAM_NOREALLOC"), Some(2));
        assert_eq!(get("TJPARAM_QUALITY"), Some(3));
        let x = enum_values(HEADER, "TJXOP");
        let get = |n: &str| x.iter().find(|(k, _)| k == n).map(|(_, v)| *v);
        assert_eq!(get("TJXOP_VFLIP"), Some(2));
        assert_eq!(get("TJXOP_ROT90"), Some(5));
        assert_eq!(get("TJXOP_ROT180"), Some(6));
        assert!(enum_values(HEADER, "NOPE").is_empty());
        assert_eq!(define_value(HEADER, "TJXOPT_PERFECT"), Some(1));
        assert_eq!(define_value(HEADER, "TJXOPT_CROP"), Some(4));
        assert_eq!(define_value(HEADER, "TJXOPT_FLAT"), Some(4));
        assert_eq!(define_value(HEADER, "TJXOPT_HEX"), Some(16));
        assert_eq!(define_value(HEADER, "TJXOPT_MISSING"), None);
    }
}
