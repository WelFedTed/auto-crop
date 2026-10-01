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
    let problems = drift(&d, &a);
    if problems.is_empty() {
        println!(
            "licenses: deny.toml and about.toml agree ({} licences)",
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

    #[test]
    fn real_files_agree() {
        let deny = fs::read_to_string("../deny.toml").unwrap();
        let about = fs::read_to_string("../about.toml").unwrap();
        assert!(drift(&list(&deny, "allow"), &list(&about, "accepted")).is_empty());
    }
}
