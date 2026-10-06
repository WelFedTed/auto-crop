// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Wildcards. `cmd.exe` and PowerShell do not expand `*.jpg`, so the CLI does it itself: `*`
//! (any run of characters but a separator), `?` (one character), `[abc]`, `[a-z]` and `[!abc]`
//! in a name, and a whole component `**` for any number of folders. Matching is case-insensitive
//! on Windows only (Unix names are case-sensitive). Links are never followed while a pattern is
//! expanded.

use std::fs;
use std::path::{Component, Path, PathBuf};

/// Whether `s` contains a wildcard.
pub fn is_pattern(s: &str) -> bool {
    s.contains(['*', '?', '['])
}

fn fold(c: char, ci: bool) -> char {
    if ci {
        c.to_lowercase().next().unwrap_or(c)
    } else {
        c
    }
}

/// Matches `name` (one path component) against `pattern`.
pub fn matches(pattern: &str, name: &str, case_insensitive: bool) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = name.chars().collect();
    match_at(&p, &t, case_insensitive)
}

fn match_at(p: &[char], t: &[char], ci: bool) -> bool {
    // Iterative with one backtrack point per `*`: linear in practice, never exponential.
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        let step = match p.get(pi) {
            Some('*') => {
                star = Some((pi, ti));
                pi += 1;
                continue;
            }
            Some('?') => Some(1),
            Some('[') => match class(p, pi, t[ti], ci) {
                Class::Hit(next) => Some(next - pi),
                Class::Miss => None,
                // An unterminated bracket is an ordinary character.
                Class::Unterminated => (t[ti] == '[').then_some(1),
            },
            Some(&c) if fold(c, ci) == fold(t[ti], ci) => Some(1),
            _ => None,
        };
        match step {
            Some(n) => {
                pi += n;
                ti += 1;
            }
            None => match star {
                Some((sp, st)) => {
                    pi = sp + 1;
                    ti = st + 1;
                    star = Some((sp, st + 1));
                }
                None => return false,
            },
        }
    }
    while p.get(pi) == Some(&'*') {
        pi += 1;
    }
    pi == p.len()
}

enum Class {
    /// `c` is in the class; the index after it.
    Hit(usize),
    Miss,
    Unterminated,
}

/// Tries the `[...]` class starting at `p[at]` against `c`.
fn class(p: &[char], at: usize, c: char, ci: bool) -> Class {
    let mut i = at + 1;
    let negate = matches!(p.get(i), Some('!' | '^'));
    if negate {
        i += 1;
    }
    let start = i;
    let mut hit = false;
    loop {
        let Some(&first) = p.get(i) else {
            return Class::Unterminated;
        };
        if first == ']' && i > start {
            return if hit != negate {
                Class::Hit(i + 1)
            } else {
                Class::Miss
            };
        }
        let lo = fold(first, ci);
        if p.get(i + 1) == Some(&'-') && p.get(i + 2).is_some_and(|e| *e != ']') {
            let hi = fold(p[i + 2], ci);
            hit |= (lo..=hi).contains(&fold(c, ci));
            i += 3;
        } else {
            hit |= lo == fold(c, ci);
            i += 1;
        }
    }
}

fn is_link(meta: &fs::Metadata) -> bool {
    meta.file_type().is_symlink() || crate::inputs::is_reparse(meta)
}

/// Expands `pattern` into existing paths, sorted. A `**` component matches any depth of
/// sub-folders (not links). No match is an empty list.
pub fn expand(pattern: &str) -> Vec<PathBuf> {
    let ci = cfg!(windows);
    let path = Path::new(pattern);
    let mut base = PathBuf::new();
    let mut rest: Vec<String> = Vec::new();
    let mut in_pattern = false;
    for comp in path.components() {
        let s = comp.as_os_str().to_string_lossy().into_owned();
        let meta = matches!(comp, Component::Normal(_)) && is_pattern(&s);
        if !in_pattern && !meta {
            base.push(comp.as_os_str());
        } else {
            in_pattern = true;
            rest.push(s);
        }
    }
    if rest.is_empty() {
        return if fs::symlink_metadata(&base).is_ok() {
            vec![base]
        } else {
            Vec::new()
        };
    }
    let start = if base.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        base.clone()
    };
    let mut out = Vec::new();
    walk(&start, &rest, ci, &mut out, 0);
    // Present paths the way the user wrote them: without a leading `./` they did not type.
    let mut out: Vec<PathBuf> = out
        .into_iter()
        .map(|p| {
            if base.as_os_str().is_empty() {
                p.strip_prefix(".").map(Path::to_path_buf).unwrap_or(p)
            } else {
                p
            }
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

const MAX_MATCHES: usize = 1_000_000;
const MAX_DEPTH: usize = 128;

fn walk(dir: &Path, rest: &[String], ci: bool, out: &mut Vec<PathBuf>, depth: usize) {
    if out.len() >= MAX_MATCHES || depth > MAX_DEPTH {
        return;
    }
    let Some((head, tail)) = rest.split_first() else {
        out.push(dir.to_path_buf());
        return;
    };
    if head == "**" {
        // Zero folders, then every sub-folder (never a link).
        walk(dir, tail, ci, out, depth + 1);
        let Ok(rd) = fs::read_dir(dir) else { return };
        let mut subs: Vec<PathBuf> = rd
            .flatten()
            .filter(|e| e.metadata().is_ok_and(|m| !is_link(&m) && m.is_dir()))
            .map(|e| e.path())
            .collect();
        subs.sort();
        for s in subs {
            walk(&s, rest, ci, out, depth + 1);
        }
        return;
    }
    if !is_pattern(head) {
        let next = dir.join(head);
        if fs::symlink_metadata(&next).is_ok() {
            walk(&next, tail, ci, out, depth + 1);
        }
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut names: Vec<(String, PathBuf)> = rd
        .flatten()
        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
        .collect();
    names.sort();
    for (name, path) in names {
        // A pattern does not match hidden names unless it starts with a dot itself.
        if name.starts_with('.') && !head.starts_with('.') {
            continue;
        }
        if !matches(head, &name, ci) {
            continue;
        }
        if tail.is_empty() {
            out.push(path);
        } else if fs::symlink_metadata(&path).is_ok_and(|m| !is_link(&m) && m.is_dir()) {
            walk(&path, tail, ci, out, depth + 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcards_match_like_a_shell() {
        let m = |p, t| matches(p, t, false);
        assert!(m("*.jpg", "a.jpg") && m("*.jpg", ".jpg") && !m("*.jpg", "a.png"));
        assert!(m("a?c", "abc") && !m("a?c", "ac") && !m("a?c", "abbc"));
        assert!(m("[ab]*", "bx") && !m("[ab]*", "cx"));
        assert!(m("img_[0-9][0-9].jpg", "img_07.jpg") && !m("img_[0-9][0-9].jpg", "img_7a.jpg"));
        assert!(m("[!a]b", "cb") && !m("[!a]b", "ab"));
        assert!(m("a*b*c", "aXXbYYc") && !m("a*b*c", "aXXbYY"));
        assert!(
            m("[", "[") && m("a[", "a["),
            "an unterminated bracket is literal-ish: no panic"
        );
        assert!(m("*", "") && m("", "") && !m("", "a"));
        assert!(matches("*.JPG", "photo.jpg", true) && !matches("*.JPG", "photo.jpg", false));
        // Pathological patterns stay fast (no exponential backtracking).
        let long = "a".repeat(5_000);
        assert!(!m("*a*a*a*a*a*b", &long));
    }

    #[test]
    fn expands_in_folders_without_following_links() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        for f in [
            "a.jpg",
            "b.jpg",
            "c.png",
            ".hidden.jpg",
            "sub/d.jpg",
            "sub/deep/e.jpg",
        ] {
            let p = r.join(f);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, b"x").unwrap();
        }
        let names = |v: Vec<PathBuf>| -> Vec<String> {
            v.iter()
                .map(|p| {
                    p.strip_prefix(r)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/")
                })
                .collect()
        };
        let pat = |s: &str| r.join(s).to_string_lossy().into_owned();
        assert_eq!(names(expand(&pat("*.jpg"))), ["a.jpg", "b.jpg"]);
        assert_eq!(names(expand(&pat("*/*.jpg"))), ["sub/d.jpg"]);
        assert_eq!(
            names(expand(&pat("**/*.jpg"))),
            ["a.jpg", "b.jpg", "sub/d.jpg", "sub/deep/e.jpg"]
        );
        assert!(expand(&pat("*.gif")).is_empty());
        assert!(expand(&pat("nothing/*.jpg")).is_empty());
    }
}
