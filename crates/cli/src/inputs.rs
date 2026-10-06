// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Turning the command line's files, folders and patterns into a list of candidate images
//! (ROADMAP M2.40). Links, junctions and other reparse points are never followed, so a loop
//! cannot hang the walk and a file is never reached twice through two routes. The walk is
//! bounded in depth and in file count, skips hidden and system entries, the app's own temp files,
//! the `AutoCrop` copy folders and the backup store, and counts everything it did not take.

use crate::args::InputArgs;
use crate::glob;
use crate::manifest::code;
use auto_crop_engine::enumerate::is_candidate;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// A file to process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// Absolute path (not resolved through links).
    pub path: PathBuf,
    /// What the path is shown as: the argument as typed, or the walked path under it.
    pub display: String,
    /// The folder a walked file was found under (for `--output` to mirror the tree), or its own
    /// folder for a file named directly.
    pub root: PathBuf,
}

/// Why an input did not become a candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    /// Nothing at that path.
    NotFound,
    /// A pattern that matched nothing.
    NoMatch,
    /// A file whose extension is not an image format this build reads.
    Unsupported,
    /// A link or junction named directly.
    Link,
}

impl Problem {
    pub fn code(self) -> &'static str {
        match self {
            Problem::NotFound => code::NOT_FOUND,
            Problem::NoMatch => code::NO_MATCH,
            Problem::Unsupported => code::UNSUPPORTED_FORMAT,
            Problem::Link => code::LINK,
        }
    }
}

#[derive(Debug, Default)]
pub struct Expansion {
    pub candidates: Vec<Candidate>,
    /// Inputs named by the user that gave nothing: (as typed, why).
    pub problems: Vec<(String, Problem)>,
    /// Files met in a walk that are not images this build reads.
    pub ignored_non_image: usize,
    /// Links, junctions and other reparse points that were not followed.
    pub links_skipped: usize,
    /// Hidden, system, temp and store entries that were left out.
    pub hidden_skipped: usize,
    /// Excluded by `--include` / `--exclude`.
    pub filtered_out: usize,
    /// The file cap was reached.
    pub truncated: bool,
    /// Folders not entered because of `--max-depth`.
    pub depth_limited: usize,
}

/// A link, junction or other reparse point (OneDrive placeholders included).
pub fn is_link(meta: &fs::Metadata) -> bool {
    meta.file_type().is_symlink() || is_reparse(meta)
}

#[cfg(windows)]
pub fn is_reparse(meta: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    meta.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
pub fn is_reparse(_meta: &fs::Metadata) -> bool {
    false
}

#[cfg(windows)]
fn hidden_or_system(meta: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    meta.file_attributes() & 0x6 != 0
}

#[cfg(not(windows))]
fn hidden_or_system(_meta: &fs::Metadata) -> bool {
    false
}

fn absolute(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

fn key(p: &Path) -> String {
    let s = p.to_string_lossy().into_owned();
    if cfg!(windows) { s.to_lowercase() } else { s }
}

struct Walker<'a> {
    args: &'a InputArgs,
    store: Option<PathBuf>,
    seen: HashSet<String>,
    out: Expansion,
}

impl Walker<'_> {
    fn full(&self) -> bool {
        self.out.candidates.len() >= self.args.max_files
    }

    fn filtered(&self, path: &Path) -> bool {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let ci = cfg!(windows);
        let included = self.args.include.is_empty()
            || self
                .args
                .include
                .iter()
                .any(|g| glob::matches(g, &name, ci));
        let excluded = self
            .args
            .exclude
            .iter()
            .any(|g| glob::matches(g, &name, ci));
        !included || excluded
    }

    fn add(&mut self, path: PathBuf, display: String, root: &Path) {
        if self.filtered(&path) {
            self.out.filtered_out += 1;
            return;
        }
        if !self.seen.insert(key(&path)) {
            return;
        }
        if self.full() {
            self.out.truncated = true;
            return;
        }
        self.out.candidates.push(Candidate {
            path,
            display,
            root: root.to_path_buf(),
        });
    }

    fn is_store(&self, p: &Path) -> bool {
        self.store.as_ref().is_some_and(|s| key(s) == key(p))
    }

    /// Walks `dir` (already known to be a real folder).
    fn walk(&mut self, dir: &Path, display: &str, root: &Path, depth: usize) {
        let Ok(rd) = fs::read_dir(dir) else {
            // An unreadable folder is reported through the problem list by the caller only for
            // the top level; below it, it is simply not entered.
            return;
        };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            if self.full() {
                self.out.truncated = true;
                return;
            }
            let path = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if is_link(&meta) {
                self.out.links_skipped += 1;
                continue;
            }
            if name.starts_with('.') || hidden_or_system(&meta) {
                self.out.hidden_skipped += 1;
                continue;
            }
            let shown = format!(
                "{}{}{name}",
                display,
                if display.ends_with(['/', '\\']) || display.is_empty() {
                    ""
                } else {
                    std::path::MAIN_SEPARATOR_STR
                }
            );
            if meta.is_dir() {
                if name == "AutoCrop" || self.is_store(&path) {
                    self.out.hidden_skipped += 1;
                } else if self.args.recursive {
                    if depth < self.args.max_depth {
                        self.walk(&path, &shown, root, depth + 1);
                    } else {
                        self.out.depth_limited += 1;
                    }
                }
            } else if meta.is_file() {
                if is_candidate(&path) {
                    self.add(path, shown, root);
                } else {
                    self.out.ignored_non_image += 1;
                }
            }
        }
    }

    /// One input, literal (it exists) or already expanded from a pattern.
    fn take(&mut self, typed: &str, path: &Path) {
        let Ok(meta) = fs::symlink_metadata(path) else {
            self.out
                .problems
                .push((typed.to_owned(), Problem::NotFound));
            return;
        };
        let abs = absolute(path);
        if is_link(&meta) {
            self.out.problems.push((typed.to_owned(), Problem::Link));
            self.out.links_skipped += 1;
        } else if meta.is_dir() {
            if self.is_store(&abs) {
                self.out.hidden_skipped += 1;
            } else {
                self.walk(&abs, typed, &abs.clone(), 0);
            }
        } else if meta.is_file() {
            if is_candidate(path) {
                let root = abs.parent().map(Path::to_path_buf).unwrap_or_default();
                self.add(abs, typed.to_owned(), &root);
            } else {
                self.out
                    .problems
                    .push((typed.to_owned(), Problem::Unsupported));
            }
        } else {
            self.out
                .problems
                .push((typed.to_owned(), Problem::Unsupported));
        }
    }
}

/// Expands every input. `store` is the backup store folder (never walked).
pub fn expand(args: &InputArgs, store: Option<&Path>) -> Expansion {
    let mut w = Walker {
        args,
        store: store.map(absolute),
        seen: HashSet::new(),
        out: Expansion::default(),
    };
    for typed in &args.paths {
        let literal = Path::new(typed);
        if fs::symlink_metadata(literal).is_ok() {
            w.take(typed, literal);
        } else if glob::is_pattern(typed) {
            let hits = glob::expand(typed);
            if hits.is_empty() {
                w.out.problems.push((typed.clone(), Problem::NoMatch));
            }
            for hit in hits {
                let shown = hit.to_string_lossy().into_owned();
                w.take(&shown, &hit);
            }
        } else {
            w.out.problems.push((typed.clone(), Problem::NotFound));
        }
        if w.out.truncated {
            break;
        }
    }
    w.out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(p: &Path) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, b"x").unwrap();
    }

    fn args(paths: &[&Path]) -> InputArgs {
        InputArgs {
            paths: paths
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect(),
            recursive: false,
            max_depth: 64,
            max_files: 50_000,
            include: vec![],
            exclude: vec![],
        }
    }

    fn names(e: &Expansion) -> Vec<String> {
        e.candidates
            .iter()
            .map(|c| c.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn folders_files_and_counts() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        for f in [
            "a.JPG",
            "b.png",
            "notes.txt",
            ".autocrop-1.tmp",
            ".hid.jpg",
            "sub/c.jpeg",
            "AutoCrop/done.jpg",
        ] {
            touch(&r.join(f));
        }
        let flat = expand(&args(&[r]), None);
        assert_eq!(names(&flat), ["a.JPG", "b.png"]);
        assert_eq!(flat.ignored_non_image, 1);
        assert_eq!(
            flat.hidden_skipped, 3,
            "two dot files and the AutoCrop folder"
        );
        let mut a = args(&[r]);
        a.recursive = true;
        let deep = expand(&a, None);
        assert_eq!(names(&deep), ["a.JPG", "b.png", "c.jpeg"]);
        // The same file named twice and reached through a folder is taken once.
        a.paths.push(r.join("a.JPG").to_string_lossy().into_owned());
        assert_eq!(expand(&a, None).candidates.len(), 3);
    }

    #[test]
    fn explicit_problems_are_reported_by_kind() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        touch(&r.join("a.jpg"));
        touch(&r.join("doc.pdf"));
        let missing = r.join("missing.jpg");
        let nomatch = r.join("zz*.jpg");
        let e = expand(
            &args(&[&r.join("a.jpg"), &r.join("doc.pdf"), &missing, &nomatch]),
            None,
        );
        assert_eq!(e.candidates.len(), 1);
        let kinds: Vec<_> = e.problems.iter().map(|(_, k)| *k).collect();
        assert_eq!(
            kinds,
            [Problem::Unsupported, Problem::NotFound, Problem::NoMatch]
        );
    }

    #[test]
    fn depth_count_include_exclude_and_the_store_are_honoured() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        for f in [
            "a.jpg",
            "x/b.jpg",
            "x/y/c.jpg",
            "store/original.jpg",
            "keep_1.png",
            "skip_1.png",
        ] {
            touch(&r.join(f));
        }
        let mut a = args(&[r]);
        a.recursive = true;
        a.max_depth = 1;
        let e = expand(&a, Some(&r.join("store")));
        assert_eq!(names(&e), ["a.jpg", "keep_1.png", "skip_1.png", "b.jpg"]);
        assert_eq!(e.depth_limited, 1, "x/y was not entered");
        assert_eq!(e.hidden_skipped, 1, "the store");
        a.max_depth = 64;
        a.max_files = 2;
        let e = expand(&a, None);
        assert_eq!(e.candidates.len(), 2);
        assert!(e.truncated);
        a.max_files = 100;
        a.include = vec!["*_1.png".into()];
        a.exclude = vec!["skip*".into()];
        let e = expand(&a, None);
        assert_eq!(names(&e), ["keep_1.png"]);
        assert!(e.filtered_out >= 4);
    }

    #[test]
    fn patterns_expand_and_dedupe() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        for f in ["a.jpg", "b.jpg", "c.png"] {
            touch(&r.join(f));
        }
        let pat = r.join("*.jpg");
        let e = expand(
            &InputArgs {
                paths: vec![
                    pat.to_string_lossy().into_owned(),
                    r.join("a.jpg").to_string_lossy().into_owned(),
                ],
                ..args(&[])
            },
            None,
        );
        assert_eq!(names(&e), ["a.jpg", "b.jpg"]);
    }

    /// A symlink loop (or any link) is never followed: the walk ends and each file is taken
    /// once. Needs permission to create links, so it is skipped quietly where that is refused.
    #[test]
    fn symlink_loops_end_and_files_are_taken_once() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        touch(&r.join("a.jpg"));
        touch(&r.join("sub/b.jpg"));
        let link = r.join("sub/loop");
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(r, &link).is_ok();
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_dir(r, &link).is_ok();
        if !made {
            eprintln!("cannot create symlinks here; skipped");
            return;
        }
        let mut a = args(&[r]);
        a.recursive = true;
        let e = expand(&a, None);
        assert_eq!(names(&e), ["a.jpg", "b.jpg"]);
        assert_eq!(e.links_skipped, 1);
        // Naming the link itself is a skipped input, not a walk.
        let e = expand(&args(&[&link]), None);
        assert!(e.candidates.is_empty());
        assert_eq!(e.problems.len(), 1);
        assert_eq!(e.problems[0].1, Problem::Link);
    }
}
