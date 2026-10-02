// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `cargo xtask licenses --check` (ROADMAP M0.09).
//!
//! The licences accepted for third-party notices (`about.toml`) must equal the
//! cargo-deny allow-list (`deny.toml`), so a licence can never be notice-able
//! but not allowed (or the reverse).

use std::collections::BTreeSet;
use std::fs;

/// Quoted strings inside the first `key = [ ... ]` array of a TOML text.
pub fn list(text: &str, key: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut inside = false;
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if !inside {
            let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
            if compact.starts_with(&format!("{key}=[")) {
                inside = true;
            } else {
                continue;
            }
        }
        let mut rest = line;
        while let Some(start) = rest.find('"') {
            let after = &rest[start + 1..];
            let Some(end) = after.find('"') else { break };
            out.insert(after[..end].to_owned());
            rest = &after[end + 1..];
        }
        if line.contains(']') {
            break;
        }
    }
    out
}

pub fn drift(deny: &BTreeSet<String>, about: &BTreeSet<String>) -> Vec<String> {
    let mut out = Vec::new();
    for l in deny.difference(about) {
        out.push(format!(
            "`{l}` is allowed in deny.toml but not accepted in about.toml"
        ));
    }
    for l in about.difference(deny) {
        out.push(format!(
            "`{l}` is accepted in about.toml but not allowed in deny.toml"
        ));
    }
    out
}

/// Every native library pinned in `native-deps.toml` must have a notice in `about.hbs` that names
/// it and states its licence, and the IJG acknowledgement must be present when a library carries
/// the IJG licence (ROADMAP M1.73).
pub fn native_notice_problems(native_deps: &str, about_hbs: &str) -> Result<Vec<String>, String> {
    let doc: toml::Table = native_deps
        .parse()
        .map_err(|e| format!("native-deps.toml: {e}"))?;
    let libs = doc
        .get("lib")
        .and_then(|l| l.as_array())
        .ok_or("native-deps.toml has no [[lib]] entries")?;
    let mut out = Vec::new();
    let mut ijg = false;
    for l in libs {
        let name = l.get("name").and_then(|n| n.as_str()).unwrap_or_default();
        let license = l
            .get("license")
            .and_then(|n| n.as_str())
            .unwrap_or_default();
        // The three prebuilt ONNX Runtime archives share one notice.
        let notice_name = if name.starts_with("onnxruntime") {
            "ONNX Runtime"
        } else {
            name
        };
        let line = about_hbs
            .lines()
            .find(|row| row.contains(&format!("**{notice_name}**")));
        match line {
            None => out.push(format!(
                "about.hbs has no native-library notice for `{notice_name}` (pinned in native-deps.toml)"
            )),
            Some(row) if !row.contains(license) => out.push(format!(
                "about.hbs notice for `{notice_name}` does not state its licence `{license}`"
            )),
            Some(_) => {}
        }
        ijg |= license.contains("IJG");
    }
    if ijg && !about_hbs.contains("Independent JPEG Group") {
        out.push(
            "a native library is IJG-licensed but about.hbs lacks the Independent JPEG Group acknowledgement"
                .to_owned(),
        );
    }
    Ok(out)
}

pub fn run(args: &[String]) -> Result<(), String> {
    if !args.iter().any(|a| a == "--check") {
        return Err("usage: cargo xtask licenses --check".to_owned());
    }
    let deny = fs::read_to_string("deny.toml").map_err(|e| format!("deny.toml: {e}"))?;
    let about = fs::read_to_string("about.toml").map_err(|e| format!("about.toml: {e}"))?;
    let (d, a) = (list(&deny, "allow"), list(&about, "accepted"));
    if d.is_empty() || a.is_empty() {
        return Err("could not read the licence lists (allow / accepted)".to_owned());
    }
    let mut problems = drift(&d, &a);
    let native =
        fs::read_to_string("native-deps.toml").map_err(|e| format!("native-deps.toml: {e}"))?;
    let hbs = fs::read_to_string("about.hbs").map_err(|e| format!("about.hbs: {e}"))?;
    problems.extend(native_notice_problems(&native, &hbs)?);
    if problems.is_empty() {
        println!(
            "licenses: deny.toml and about.toml agree ({} licences); native-library notices complete",
            d.len()
        );
        Ok(())
    } else {
        Err(problems.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_multiline_arrays_and_ignores_comments() {
        let t = "[licenses]\nallow = [\n    \"MIT\", # permissive\n    \"Apache-2.0\",\n]\nother = [\"X\"]\n";
        let s = list(t, "allow");
        assert_eq!(s.len(), 2);
        assert!(s.contains("MIT") && s.contains("Apache-2.0"));
    }

    #[test]
    fn agreement_has_no_drift() {
        let a: BTreeSet<String> = ["MIT".to_owned()].into();
        assert!(drift(&a, &a).is_empty());
    }

    #[test]
    fn planted_unlisted_licence_is_drift() {
        let deny: BTreeSet<String> = ["MIT".to_owned()].into();
        let about: BTreeSet<String> = ["MIT".to_owned(), "GPL-3.0-only".to_owned()].into();
        assert_eq!(drift(&deny, &about).len(), 1);
        assert_eq!(drift(&about, &deny).len(), 1);
    }

    const NATIVE: &str = "[[lib]]
name = \"libjpeg-turbo\"
license = \"IJG AND BSD-3-Clause AND Zlib\"
[[lib]]
name = \"onnxruntime-linux-x64\"
license = \"MIT\"
";

    #[test]
    fn complete_native_notices_pass() {
        let hbs = "- **libjpeg-turbo** (IJG AND BSD-3-Clause AND Zlib): based in part on the work of the Independent JPEG Group.
- **ONNX Runtime** (MIT): x
";
        assert!(native_notice_problems(NATIVE, hbs).unwrap().is_empty());
    }

    #[test]
    fn planted_missing_native_notice_fails() {
        let hbs = "- **ONNX Runtime** (MIT): x
";
        let v = native_notice_problems(NATIVE, hbs).unwrap();
        assert!(v.iter().any(|m| m.contains("libjpeg-turbo")), "{v:?}");
        assert!(
            v.iter().any(|m| m.contains("Independent JPEG Group")),
            "{v:?}"
        );
    }

    #[test]
    fn planted_wrong_native_licence_fails() {
        let hbs = "- **libjpeg-turbo** (BSD-3-Clause): Independent JPEG Group
- **ONNX Runtime** (MIT): x
";
        let v = native_notice_problems(NATIVE, hbs).unwrap();
        assert!(
            v.iter().any(|m| m.contains("does not state its licence")),
            "{v:?}"
        );
    }

    #[test]
    fn real_native_notices_are_complete() {
        let native = fs::read_to_string("../native-deps.toml").unwrap();
        let hbs = fs::read_to_string("../about.hbs").unwrap();
        let v = native_notice_problems(&native, &hbs).unwrap();
        assert!(v.is_empty(), "{v:?}");
    }

    #[test]
    fn real_files_agree() {
        let deny = fs::read_to_string("../deny.toml").unwrap();
        let about = fs::read_to_string("../about.toml").unwrap();
        assert!(drift(&list(&deny, "allow"), &list(&about, "accepted")).is_empty());
    }
}
