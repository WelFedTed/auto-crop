// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Keeps `docs/provenance.md` (the human-written register of every tool, font, dataset and
//! weight in use, with SPDX licence, use and source; ROADMAP M1.74) honest. It is run by
//! `cargo xtask provenance`.
//!
//! The register is prose plus tables; each entry is a table row whose first cell is the name in
//! backticks. Checked mechanically:
//!
//! * every `[[lib]]` in `native-deps.toml` has a row that states its SPDX licence;
//! * every package pinned in `tools/*/requirements.txt` has a row;
//! * every `@fontsource/*` font package in `ui/package.json` has a row;
//! * every id in the provenance log (`provenance.jsonl`) is mentioned (cleared, pending or excluded);
//! * every crate in the `[bans] deny` list of `deny.toml` is on the excluded list.
//!
//! Rust crates and npm code packages are out of scope on purpose: `cargo deny` and `cargo about`
//! gate and notice the crates, and the register points at them.

use std::fs;

/// Is there a table row whose first cell is `` `name` ``?
pub fn has_row<'a>(register: &'a str, name: &str) -> Option<&'a str> {
    let prefix = format!("| `{name}`");
    register
        .lines()
        .find(|l| l.trim_start().starts_with(&prefix))
}

/// Package names from a pip requirements file (`name==version`, comments ignored).
pub fn requirement_names(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter(|l| !l.is_empty() && !l.starts_with('-'))
        .filter_map(|l| {
            let name: String = l
                .chars()
                .take_while(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
                .collect();
            if name.is_empty() { None } else { Some(name) }
        })
        .collect()
}

/// `@fontsource/*` package names in a `package.json` text.
pub fn font_packages(package_json: &str) -> Result<Vec<String>, String> {
    let v: serde_json::Value =
        serde_json::from_str(package_json).map_err(|e| format!("package.json: {e}"))?;
    let mut out = Vec::new();
    for section in ["dependencies", "devDependencies"] {
        if let Some(o) = v.get(section).and_then(|d| d.as_object()) {
            out.extend(o.keys().filter(|k| k.starts_with("@fontsource/")).cloned());
        }
    }
    out.sort();
    Ok(out)
}

pub struct Inputs<'a> {
    pub register: &'a str,
    pub native_deps: &'a str,
    pub requirements: &'a [(String, String)], // (file, text)
    pub package_json: &'a str,
    pub provenance_ids: &'a [String],
    pub banned_crates: &'a [String],
}

/// All problems with the register; empty when it is complete.
pub fn problems(i: &Inputs) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let doc: toml::Table = i
        .native_deps
        .parse()
        .map_err(|e| format!("native-deps.toml: {e}"))?;
    for l in doc
        .get("lib")
        .and_then(|l| l.as_array())
        .into_iter()
        .flatten()
    {
        let name = l.get("name").and_then(|n| n.as_str()).unwrap_or_default();
        let license = l
            .get("license")
            .and_then(|n| n.as_str())
            .unwrap_or_default();
        match has_row(i.register, name) {
            None => out.push(format!(
                "docs/provenance.md has no row for native library `{name}`"
            )),
            Some(row) if !row.contains(license) => out.push(format!(
                "docs/provenance.md row for `{name}` does not state its SPDX licence `{license}`"
            )),
            Some(_) => {}
        }
    }
    for (file, text) in i.requirements {
        for name in requirement_names(text) {
            if has_row(i.register, &name).is_none() {
                out.push(format!(
                    "docs/provenance.md has no row for `{name}` (pinned in {file})"
                ));
            }
        }
    }
    for name in font_packages(i.package_json)? {
        if has_row(i.register, &name).is_none() {
            out.push(format!(
                "docs/provenance.md has no row for font package `{name}`"
            ));
        }
    }
    for id in i.provenance_ids {
        if !i.register.contains(&format!("`{id}`")) {
            out.push(format!(
                "docs/provenance.md does not mention provenance-log entry `{id}`"
            ));
        }
    }
    for name in i.banned_crates {
        if has_row(i.register, name).is_none() {
            out.push(format!(
                "docs/provenance.md excluded list has no row for banned crate `{name}` (deny.toml)"
            ));
        }
    }
    Ok(out)
}

/// Reads the repository files and checks the register.
pub fn check_repo(provenance_ids: &[String]) -> Result<Vec<String>, String> {
    let read = |p: &str| fs::read_to_string(p).map_err(|e| format!("{p}: {e}"));
    let register = read("docs/provenance.md")?;
    let native = read("native-deps.toml")?;
    let package_json = read("ui/package.json")?;
    let deny = read("deny.toml")?;
    let banned = crate::deny_selftest::banned_crates(&deny)?;
    let mut reqs = Vec::new();
    if let Ok(rd) = fs::read_dir("tools") {
        let mut dirs: Vec<_> = rd.filter_map(Result::ok).map(|d| d.path()).collect();
        dirs.sort();
        for d in dirs {
            let f = d.join("requirements.txt");
            if let Ok(t) = fs::read_to_string(&f) {
                reqs.push((f.to_string_lossy().replace('\\', "/"), t));
            }
        }
    }
    problems(&Inputs {
        register: &register,
        native_deps: &native,
        requirements: &reqs,
        package_json: &package_json,
        provenance_ids,
        banned_crates: &banned,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NATIVE: &str = "[[lib]]\nname = \"libheif\"\nlicense = \"LGPL-3.0-or-later\"\n";
    const PKG: &str = "{\"dependencies\":{\"@fontsource/ibm-plex-sans\":\"^5\",\"svelte\":\"5\"}}";

    fn good_register() -> String {
        "| `libheif` | native | LGPL-3.0-or-later | x |\n| `numpy` | python | BSD-3-Clause | x |\n| `@fontsource/ibm-plex-sans` | font | OFL-1.1 | x |\n| `dataset:cord` | dataset | CC-BY-4.0 | x |\n| `heic` | crate | AGPL | excluded |\n".to_owned()
    }

    fn inputs<'a>(
        register: &'a str,
        reqs: &'a [(String, String)],
        ids: &'a [String],
        banned: &'a [String],
    ) -> Inputs<'a> {
        Inputs {
            register,
            native_deps: NATIVE,
            requirements: reqs,
            package_json: PKG,
            provenance_ids: ids,
            banned_crates: banned,
        }
    }

    #[test]
    fn a_complete_register_has_no_problems() {
        let reqs = vec![(
            "tools/x/requirements.txt".to_owned(),
            "numpy==2.5.3\n".to_owned(),
        )];
        let ids = vec!["dataset:cord".to_owned()];
        let banned = vec!["heic".to_owned()];
        let r = good_register();
        assert!(
            problems(&inputs(&r, &reqs, &ids, &banned))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn each_kind_of_omission_is_caught() {
        let reqs = vec![(
            "tools/x/requirements.txt".to_owned(),
            "numpy==2.5.3\nopencv-python-headless==5.0.0.93\n".to_owned(),
        )];
        let ids = vec!["dataset:cord".to_owned(), "weights:doctr".to_owned()];
        let banned = vec!["heic".to_owned(), "purecv".to_owned()];
        let r = good_register()
            .replace(
                "| `libheif` | native | LGPL-3.0-or-later |",
                "| `libheif` | native | MIT |",
            )
            .replace("| `@fontsource/ibm-plex-sans` | font | OFL-1.1 | x |\n", "");
        let v = problems(&inputs(&r, &reqs, &ids, &banned)).unwrap();
        let all = v.join("\n");
        assert!(
            all.contains("does not state its SPDX licence `LGPL-3.0-or-later`"),
            "{all}"
        );
        assert!(all.contains("opencv-python-headless"), "{all}");
        assert!(
            all.contains("font package `@fontsource/ibm-plex-sans`"),
            "{all}"
        );
        assert!(all.contains("`weights:doctr`"), "{all}");
        assert!(all.contains("banned crate `purecv`"), "{all}");
        assert_eq!(v.len(), 5, "{all}");
    }

    #[test]
    fn a_missing_native_row_is_caught() {
        let v = problems(&inputs("", &[], &[], &[])).unwrap();
        assert!(
            v.iter().any(|m| m.contains("native library `libheif`")),
            "{v:?}"
        );
    }

    #[test]
    fn parses_requirements_and_fonts() {
        assert_eq!(
            requirement_names(
                "# c\nnumpy==2.5.3  # pin\n\n-r other.txt\nopencv-python-headless==5.0.0.93\n"
            ),
            vec!["numpy", "opencv-python-headless"]
        );
        assert_eq!(
            font_packages(PKG).unwrap(),
            vec!["@fontsource/ibm-plex-sans"]
        );
    }

    #[test]
    fn row_matching_needs_the_name_in_the_first_cell() {
        assert!(has_row("| `a` | x |\n", "a").is_some());
        assert!(has_row("text `a` mention\n", "a").is_none());
        assert!(has_row("| `ab` | x |\n", "a").is_none());
    }
}
