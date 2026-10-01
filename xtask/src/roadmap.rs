// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `cargo xtask roadmap-check` (ROADMAP M0.78).
//!
//! Validates ROADMAP.md: checkbox syntax, item IDs (valid and unique), GATE
//! line form, the progress table (`done / total` per section) and, with
//! `--baseline <ref>`, that no ID present in the baseline was dropped unless it
//! is listed in `docs/roadmap/retired.txt`. `--write` regenerates the table.

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::process::Command;

const ROADMAP: &str = "ROADMAP.md";
const RETIRED: &str = "docs/roadmap/retired.txt";
const SIZE_WARN_BYTES: usize = 450 * 1024;
const TABLE_START: &str = "<!-- progress:start -->";
const TABLE_END: &str = "<!-- progress:end -->";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// "P", "M4", ... or the full heading text for non-milestone sections.
    pub key: String,
    pub title: String,
    pub done: usize,
    pub total: usize,
}

#[derive(Debug, Default)]
pub struct Report {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub sections: Vec<Section>,
    pub ids: BTreeSet<String>,
}

fn valid_id(s: &str) -> bool {
    // letters, optional digits, '.', digits: M4.01, X.41, B.39, S.08, P.01
    let (head, tail) = match s.split_once('.') {
        Some(p) => p,
        None => return false,
    };
    let letters = head.chars().take_while(|c| c.is_ascii_uppercase()).count();
    letters >= 1
        && head[letters..].chars().all(|c| c.is_ascii_digit())
        && !tail.is_empty()
        && tail.chars().all(|c| c.is_ascii_digit())
}

enum Cb<'a> {
    Ok { done: bool, rest: &'a str },
    Bad,
}

fn checkbox(line: &str) -> Option<Cb<'_>> {
    let r = line.trim_start().strip_prefix("- [")?;
    let c = r.chars().next()?;
    let after = &r[c.len_utf8()..];
    if (c == ' ' || c == 'x') && after.starts_with("] ") {
        Some(Cb::Ok {
            done: c == 'x',
            rest: &after[2..],
        })
    } else {
        Some(Cb::Bad)
    }
}

fn section_key(title: &str) -> String {
    if let Some((head, _)) = title.split_once(" - ") {
        let h = head.trim();
        let milestone = h == "P"
            || (h.starts_with('M') && h[1..].chars().all(|c| c.is_ascii_digit()) && h.len() > 1);
        if milestone {
            return h.to_owned();
        }
    }
    title.trim().to_owned()
}

pub fn check(text: &str) -> Report {
    let mut rep = Report::default();
    let mut current: Option<Section> = None;
    let mut ids_seen: HashSet<String> = HashSet::new();
    for (i, line) in text.lines().enumerate() {
        let n = i + 1;
        if let Some(title) = line.strip_prefix("## ") {
            if let Some(s) = current.take() {
                rep.sections.push(s);
            }
            current = Some(Section {
                key: section_key(title),
                title: title.trim().to_owned(),
                done: 0,
                total: 0,
            });
            continue;
        }
        let Some(cb) = checkbox(line) else { continue };
        match cb {
            Cb::Bad => rep.errors.push(format!(
                "{ROADMAP}:{n}: malformed checkbox (use `- [ ]` or `- [x]`): {}",
                short(line)
            )),
            Cb::Ok { done, rest } => {
                if let Some(s) = current.as_mut() {
                    s.total += 1;
                    if done {
                        s.done += 1;
                    }
                }
                if rest.starts_with("**GATE") {
                    let ok = rest.starts_with("**GATE:**")
                        || (rest.starts_with("**GATE (G") && rest.contains("):**"));
                    if !ok {
                        rep.errors.push(format!("{ROADMAP}:{n}: malformed GATE line (need **GATE:** or **GATE (G<n>):**): {}", short(line)));
                    }
                } else if let Some(inner) = rest.strip_prefix("**") {
                    let id = inner.split_whitespace().next().unwrap_or("");
                    if !valid_id(id) {
                        rep.errors.push(format!(
                            "{ROADMAP}:{n}: invalid item ID `{id}`: {}",
                            short(line)
                        ));
                    } else if !ids_seen.insert(id.to_owned()) {
                        rep.errors.push(format!("{ROADMAP}:{n}: duplicate ID {id}"));
                    } else {
                        rep.ids.insert(id.to_owned());
                    }
                } else {
                    rep.errors.push(format!(
                        "{ROADMAP}:{n}: checkbox item without a bold ID: {}",
                        short(line)
                    ));
                }
            }
        }
    }
    if let Some(s) = current.take() {
        rep.sections.push(s);
    }
    rep.sections.retain(|s| s.total > 0);
    if text.len() > SIZE_WARN_BYTES {
        rep.warnings.push(format!(
            "{ROADMAP} is {} KB (> 450 KB): archive fully ticked milestones to docs/roadmap/archive/",
            text.len() / 1024
        ));
    }
    rep
}

fn short(line: &str) -> String {
    line.chars().take(90).collect()
}

fn progress_text(done: usize, total: usize) -> String {
    let pct = (100 * done).checked_div(total).unwrap_or(0);
    format!("{done} / {total} ({pct}%)")
}

/// Expected progress string for a table row, or None if the row is unknown.
fn expected_for_row(first: &str, title: &str, sections: &[Section]) -> Option<String> {
    if first == "**Total**" {
        let d: usize = sections.iter().map(|s| s.done).sum();
        let t: usize = sections.iter().map(|s| s.total).sum();
        return Some(format!("**{}**", progress_text(d, t)));
    }
    let hit = if first == "X" {
        sections.iter().find(|s| s.title == title)
    } else {
        sections.iter().find(|s| s.key == first)
    };
    hit.map(|s| progress_text(s.done, s.total))
}

/// Compares (or, with `fix`, rewrites) the progress table. Returns (new_text, errors).
pub fn table(text: &str, sections: &[Section], fix: bool) -> (String, Vec<String>) {
    let mut errors = Vec::new();
    let mut out = Vec::new();
    let mut inside = false;
    let mut row = 0usize;
    for line in text.lines() {
        if line.trim() == TABLE_START {
            inside = true;
            row = 0;
            out.push(line.to_owned());
            continue;
        }
        if line.trim() == TABLE_END {
            inside = false;
            out.push(line.to_owned());
            continue;
        }
        if inside && line.starts_with('|') {
            row += 1;
            if row <= 2 {
                out.push(line.to_owned()); // header and separator
                continue;
            }
            let cells: Vec<&str> = line
                .trim()
                .trim_matches('|')
                .split('|')
                .map(str::trim)
                .collect();
            let first = cells.first().copied().unwrap_or("");
            let title = cells.get(1).copied().unwrap_or("");
            let last = cells.last().copied().unwrap_or("");
            match expected_for_row(first, title, sections) {
                None => {
                    errors.push(format!(
                        "progress table row `{first}` ({title}) matches no section"
                    ));
                    out.push(line.to_owned());
                }
                Some(exp) if exp != last => {
                    errors.push(format!("stale progress for `{first}` {title}: table says `{last}`, checkboxes say `{exp}`"));
                    if fix {
                        let cut = line
                            .trim_end()
                            .trim_end_matches('|')
                            .rfind('|')
                            .unwrap_or(0);
                        out.push(format!("{}| {exp} |", &line[..cut]));
                    } else {
                        out.push(line.to_owned());
                    }
                }
                Some(_) => out.push(line.to_owned()),
            }
            continue;
        }
        out.push(line.to_owned());
    }
    let mut s = out.join("\n");
    if text.ends_with('\n') {
        s.push('\n');
    }
    (s, errors)
}

pub fn parse_retired(text: &str) -> HashSet<String> {
    text.lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter(|l| !l.is_empty())
        .filter_map(|l| l.split_whitespace().next())
        .map(str::to_owned)
        .collect()
}

pub fn dropped_ids(
    baseline: &BTreeSet<String>,
    now: &BTreeSet<String>,
    retired: &HashSet<String>,
) -> Vec<String> {
    baseline
        .iter()
        .filter(|id| !now.contains(*id) && !retired.contains(*id))
        .cloned()
        .collect()
}

fn ids_of(text: &str) -> BTreeSet<String> {
    check(text).ids
}

pub fn run(args: &[String]) -> Result<(), String> {
    let write = args.iter().any(|a| a == "--write");
    let baseline = args
        .iter()
        .position(|a| a == "--baseline")
        .and_then(|i| args.get(i + 1))
        .cloned();
    let text = fs::read_to_string(ROADMAP).map_err(|e| format!("cannot read {ROADMAP}: {e}"))?;
    let rep = check(&text);
    let mut errors = rep.errors.clone();
    let (fixed, table_errors) = table(&text, &rep.sections, write);
    if write {
        if fixed != text {
            fs::write(ROADMAP, &fixed).map_err(|e| format!("cannot write {ROADMAP}: {e}"))?;
            println!(
                "roadmap-check: progress table regenerated ({} rows changed)",
                table_errors.len()
            );
        }
    } else {
        errors.extend(table_errors);
    }
    if let Some(r) = baseline {
        let spec = format!("{r}:{ROADMAP}");
        let out = Command::new("git")
            .args(["show", &spec])
            .output()
            .map_err(|e| format!("git: {e}"))?;
        if out.status.success() {
            let base_ids = ids_of(&String::from_utf8_lossy(&out.stdout));
            let retired = fs::read_to_string(RETIRED)
                .map(|t| parse_retired(&t))
                .unwrap_or_default();
            for id in dropped_ids(&base_ids, &rep.ids, &retired) {
                errors.push(format!(
                    "ID {id} exists in {r} but was dropped (list it in {RETIRED} if it was retired)"
                ));
            }
        } else {
            println!("roadmap-check: baseline `{r}` has no {ROADMAP}; skipping dropped-ID check");
        }
    }
    for w in &rep.warnings {
        eprintln!("warning: {w}");
    }
    if errors.is_empty() {
        let d: usize = rep.sections.iter().map(|s| s.done).sum();
        let t: usize = rep.sections.iter().map(|s| s.total).sum();
        println!("roadmap-check: ok ({} IDs, {d}/{t} ticked)", rep.ids.len());
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "\
# Roadmap

<!-- progress:start -->
| Milestone | Title | Release | Progress |
|---|---|---|---|
| P | Planning | none | 1 / 2 (50%) |
| M0 | Foundations | none | 0 / 2 (0%) |
| **Total** | | | **1 / 4 (25%)** |
<!-- progress:end -->

## P - Planning
- [x] **P.01 Interview** - done.
- [ ] **P.02 Review** - open.

## M0 - Foundations
- [ ] **M0.01 Repo** - create it.
- [ ] **GATE (G0):** repo is public.
";

    #[test]
    fn good_roadmap_passes() {
        let rep = check(GOOD);
        assert!(rep.errors.is_empty(), "{:?}", rep.errors);
        let (_, errs) = table(GOOD, &rep.sections, false);
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(rep.ids.len(), 3);
    }

    #[test]
    fn bad_checkbox_is_rejected() {
        for bad in [
            "- [X] **M0.03 A** - x",
            "- [] **M0.03 A** - x",
            "- [y] **M0.03 A** - x",
        ] {
            let rep = check(&format!("## M0 - A\n{bad}\n"));
            assert_eq!(rep.errors.len(), 1, "{bad}: {:?}", rep.errors);
        }
    }

    #[test]
    fn duplicate_and_invalid_ids_are_rejected() {
        let rep =
            check("## M0 - A\n- [ ] **M0.01 A** - x\n- [ ] **M0.01 B** - y\n- [ ] **Foo T** - z\n");
        assert_eq!(rep.errors.len(), 2, "{:?}", rep.errors);
    }

    #[test]
    fn malformed_gate_is_rejected() {
        let rep = check(
            "## M0 - A\n- [ ] **GATE** missing colon\n- [ ] **GATE (G2):** fine\n- [ ] **GATE:** fine\n",
        );
        assert_eq!(rep.errors.len(), 1, "{:?}", rep.errors);
    }

    #[test]
    fn stale_table_is_detected_and_fixed() {
        let stale = GOOD.replace("0 / 2 (0%) |", "1 / 2 (50%) |");
        let rep = check(&stale);
        let (_, errs) = table(&stale, &rep.sections, false);
        assert_eq!(errs.len(), 1, "{errs:?}");
        let (fixed, _) = table(&stale, &rep.sections, true);
        let (_, errs2) = table(&fixed, &check(&fixed).sections, false);
        assert!(errs2.is_empty(), "{errs2:?}");
        assert!(fixed.contains("| M0 | Foundations | none | 0 / 2 (0%) |"));
    }

    #[test]
    fn ticking_a_box_makes_the_table_stale() {
        let ticked = GOOD.replace("- [ ] **M0.01 Repo**", "- [x] **M0.01 Repo**");
        let (_, errs) = table(&ticked, &check(&ticked).sections, false);
        assert_eq!(errs.len(), 2, "{errs:?}"); // M0 row and Total row
    }

    #[test]
    fn dropped_ids_need_a_retired_entry() {
        let base: BTreeSet<String> = ["M0.01", "M0.02"].iter().map(|s| (*s).to_owned()).collect();
        let now: BTreeSet<String> = ["M0.01"].iter().map(|s| (*s).to_owned()).collect();
        assert_eq!(dropped_ids(&base, &now, &HashSet::new()), vec!["M0.02"]);
        let retired = parse_retired("# comment\nM0.02 merged into M0.01\n");
        assert!(dropped_ids(&base, &now, &retired).is_empty());
    }

    #[test]
    fn valid_ids() {
        for ok in ["M4.01", "X.41", "B.39", "S.08", "P.01", "M13.91"] {
            assert!(valid_id(ok), "{ok}");
        }
        for bad in ["M4", "4.01", "m4.01", "M4.", "M4.0a", ""] {
            assert!(!valid_id(bad), "{bad}");
        }
    }
}
