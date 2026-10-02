// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The `unsafe` guard (ROADMAP M1.72).
//!
//! Policy:
//!
//! 1. `unsafe` code may appear only inside a module directory named `ffi` or `simd`
//!    (`crates/<crate>/src/**/ffi/**` or `.../simd/**`), or in a file on [`ALLOWED_FILES`].
//! 2. Every `unsafe` block, `unsafe impl`, `unsafe extern` block and `unsafe fn` carries a
//!    `// SAFETY:` comment directly above it (or on the same line). An `unsafe fn` or
//!    `unsafe trait` declaration may instead carry a `# Safety` doc section (the standard
//!    place to state the caller's obligations). An `unsafe fn` inside an `unsafe impl` is
//!    covered by the impl's own `// SAFETY:` comment.
//! 3. The lint escape hatch (`#[allow(unsafe_code)]`, `unsafe_code = "allow"` in a manifest)
//!    follows rule 1: it is accepted only where `unsafe` itself is.
//!
//! The scan covers `crates/**`, `xtask/src/**` and `xtask/tests/**`. It does not cover
//! `spikes/` (throwaway code outside the workspace, never shipped) or third-party crates.

use std::fs;
use std::path::{Path, PathBuf};

use super::rust_lex::{Lexed, Tok, lex};

/// Files outside `ffi/` and `simd/` that may contain `unsafe`, with the reason. Their `unsafe`
/// still needs `// SAFETY:` comments. Add to this list only with the owner's agreement.
pub const ALLOWED_FILES: &[(&str, &str)] = &[(
    "xtask/src/alloc_count.rs",
    "counting GlobalAlloc used by `hostile-run` to measure decoder heap; xtask is a developer tool and is never shipped",
)];

/// Directory names that may hold `unsafe`.
pub const ALLOWED_DIRS: &[&str] = &["ffi", "simd"];

/// Roots scanned by the guard, relative to the repository root.
const SCAN_ROOTS: &[&str] = &["crates", "xtask/src", "xtask/tests"];

/// True when `rel` (forward slashes, relative to the repo root) may hold `unsafe`.
pub fn placement_allowed(rel: &str) -> bool {
    if ALLOWED_FILES.iter().any(|(f, _)| *f == rel) {
        return true;
    }
    let mut parts: Vec<&str> = rel.split('/').collect();
    parts.pop(); // the file name itself is not a module directory
    parts.iter().any(|p| ALLOWED_DIRS.contains(p))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Block,
    Impl,
    Decl,
}

fn is_ident(t: &Tok, s: &str) -> bool {
    matches!(t, Tok::Ident(x) if x == s)
}

fn classify(lexed: &Lexed, idx: usize) -> Kind {
    let mut j = idx + 1;
    while let Some((t, _)) = lexed.toks.get(j) {
        match t {
            Tok::Lit => j += 1,
            Tok::Ident(w) if w == "extern" => j += 1,
            Tok::Ident(w) if w == "fn" || w == "trait" => return Kind::Decl,
            Tok::Ident(w) if w == "impl" => return Kind::Impl,
            _ => return Kind::Block,
        }
    }
    Kind::Block
}

fn comment_ok(text: &str, kind: Kind) -> bool {
    text.contains("SAFETY:") || (kind == Kind::Decl && text.contains("# Safety"))
}

/// Is the `unsafe` on `line` documented: a comment on the same line, or a comment block directly
/// above it (attribute lines in between are skipped; a blank line ends the search)?
fn documented(lexed: &Lexed, line: usize, kind: Kind) -> bool {
    if comment_ok(&lexed.comments[line], kind) {
        return true;
    }
    let mut l = line;
    while l > 1 {
        l -= 1;
        if lexed.has_code[l] {
            if lexed.first_tok[l] == Some(Tok::Punct('#')) {
                continue; // an attribute between the comment and the item
            }
            return false;
        }
        if lexed.comments[l].is_empty() {
            return false; // blank line
        }
        if comment_ok(&lexed.comments[l], kind) {
            return true;
        }
    }
    false
}

/// Checks one Rust source file. `path` is only used in messages; `allowed` says whether the file
/// sits where `unsafe` is permitted.
pub fn check_source(path: &str, src: &str, allowed: bool) -> Vec<String> {
    let lexed = lex(src);
    let mut out = Vec::new();
    let mut stack: Vec<bool> = Vec::new(); // true = body of an `unsafe impl`
    let mut pending_unsafe_impl = false;
    for (idx, (tok, line)) in lexed.toks.iter().enumerate() {
        match tok {
            Tok::Punct('{') => {
                stack.push(pending_unsafe_impl);
                pending_unsafe_impl = false;
            }
            Tok::Punct('}') => {
                stack.pop();
            }
            Tok::Ident(w) if w == "unsafe" => {
                let kind = classify(&lexed, idx);
                if kind == Kind::Impl {
                    pending_unsafe_impl = true;
                }
                if !allowed {
                    out.push(format!(
                        "{path}:{line}: `unsafe` outside an ffi/ or simd/ module directory"
                    ));
                    continue;
                }
                let inside_unsafe_impl = stack.last() == Some(&true);
                if kind == Kind::Decl && inside_unsafe_impl {
                    continue;
                }
                if !documented(&lexed, *line, kind) {
                    out.push(format!(
                        "{path}:{line}: `unsafe` without a `// SAFETY:` comment"
                    ));
                }
            }
            Tok::Ident(w) if w == "unsafe_code" && !allowed => {
                // Look back inside the attribute for `allow(` / `expect(`.
                let mut k = idx;
                let mut steps = 0;
                while k > 0 && steps < 8 {
                    k -= 1;
                    steps += 1;
                    let t = &lexed.toks[k].0;
                    if matches!(t, Tok::Punct('[')) {
                        break;
                    }
                    if is_ident(t, "allow") || is_ident(t, "expect") {
                        out.push(format!(
                            "{path}:{line}: lint escape hatch `allow(unsafe_code)` outside an ffi/ or simd/ module directory"
                        ));
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Checks one manifest: `unsafe_code = "allow"` is the manifest-level escape hatch.
pub fn check_manifest(path: &str, text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("");
        let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
        if compact == "unsafe_code=\"allow\"" {
            out.push(format!(
                "{path}:{}: `unsafe_code = \"allow\"` in a manifest; allow it per module with a SAFETY-commented ffi/ or simd/ directory instead",
                i + 1
            ));
        }
    }
    out
}

fn collect(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(()), // a scan root that does not exist (yet) has nothing to scan
    };
    for entry in entries.filter_map(Result::ok) {
        let p = entry.path();
        let name = entry.file_name();
        if p.is_dir() {
            if name == "target" || name == "node_modules" || name == ".git" {
                continue;
            }
            collect(&p, files)?;
        } else if p.extension().is_some_and(|e| e == "rs" || e == "toml") {
            files.push(p);
        }
    }
    Ok(())
}

/// Scans the tree under `root` and returns every violation.
pub fn check_tree(root: &Path) -> Result<Vec<String>, String> {
    let mut files = Vec::new();
    for r in SCAN_ROOTS {
        collect(&root.join(r), &mut files)?;
    }
    files.sort();
    let mut out = Vec::new();
    for f in &files {
        let rel = f
            .strip_prefix(root)
            .unwrap_or(f)
            .to_string_lossy()
            .replace('\\', "/");
        let text = fs::read_to_string(f).map_err(|e| format!("{rel}: {e}"))?;
        if f.extension().is_some_and(|e| e == "toml") {
            if f.file_name().is_some_and(|n| n == "Cargo.toml") {
                out.extend(check_manifest(&rel, &text));
            }
        } else {
            out.extend(check_source(&rel, &text, placement_allowed(&rel)));
        }
    }
    // The root manifest sets the workspace baseline.
    let root_manifest = root.join("Cargo.toml");
    if let Ok(text) = fs::read_to_string(&root_manifest) {
        out.extend(check_manifest("Cargo.toml", &text));
    }
    Ok(out)
}

/// Planted violations for the self-test: (name, path, source, marker the output must contain).
/// A `None` marker means the case is a control that must pass.
pub const CASES: &[(&str, &str, &str, Option<&str>)] = &[
    (
        "control-safe-code",
        "crates/demo/src/lib.rs",
        "pub fn f() -> u32 { 1 }\n",
        None,
    ),
    (
        "control-words-in-comments-and-strings",
        "crates/demo/src/lib.rs",
        "// unsafe is forbidden here\n/// An `unsafe` word in a doc comment.\npub const S: &str = \"unsafe { }\";\npub const R: &str = r#\"unsafe fn x() {}\"#;\n#[allow(dead_code)]\nfn unsafe_code_is_a_name() {}\n",
        None,
    ),
    (
        "control-ffi-with-safety",
        "crates/demo/src/ffi/mod.rs",
        "#![allow(unsafe_code)]\n/// # Safety\n/// `p` must be valid.\npub unsafe fn read(p: *const u8) -> u8 {\n    // SAFETY: the caller guarantees `p` is valid.\n    unsafe { *p }\n}\n",
        None,
    ),
    (
        "control-simd-unsafe-impl",
        "crates/demo/src/simd/avx2.rs",
        "struct W;\n// SAFETY: W has no interior state.\nunsafe impl Send for W {}\nfn g(p: *const u8) -> u8 {\n    let v = 1; // SAFETY: trailing comment counts\n    let _ = v;\n    // SAFETY: p is valid for reads.\n    #[allow(clippy::needless_return)]\n    let x = unsafe { *p };\n    x\n}\n",
        None,
    ),
    (
        "unsafe-block-outside-ffi",
        "crates/demo/src/lib.rs",
        "pub fn f(p: *const u8) -> u8 {\n    // SAFETY: valid.\n    unsafe { *p }\n}\n",
        Some("outside an ffi/ or simd/"),
    ),
    (
        "unsafe-fn-outside-ffi",
        "crates/demo/src/decode.rs",
        "/// # Safety\n/// valid\npub unsafe fn f(p: *const u8) -> u8 { *p }\n",
        Some("outside an ffi/ or simd/"),
    ),
    (
        "unsafe-impl-outside-ffi",
        "crates/demo/src/lib.rs",
        "struct W;\n// SAFETY: no state.\nunsafe impl Sync for W {}\n",
        Some("outside an ffi/ or simd/"),
    ),
    (
        "ffi-without-safety-comment",
        "crates/demo/src/ffi/mod.rs",
        "pub fn f(p: *const u8) -> u8 {\n    unsafe { *p }\n}\n",
        Some("without a `// SAFETY:` comment"),
    ),
    (
        "ffi-blank-line-breaks-the-comment",
        "crates/demo/src/ffi/mod.rs",
        "pub fn f(p: *const u8) -> u8 {\n    // SAFETY: valid.\n\n    unsafe { *p }\n}\n",
        Some("without a `// SAFETY:` comment"),
    ),
    (
        "ffi-unsafe-impl-without-safety",
        "crates/demo/src/simd/mod.rs",
        "struct W;\nunsafe impl Send for W {}\n",
        Some("without a `// SAFETY:` comment"),
    ),
    (
        "ffi-unsafe-fn-without-docs",
        "crates/demo/src/ffi/mod.rs",
        "pub unsafe fn f(p: *const u8) -> u8 {\n    // SAFETY: caller contract is undocumented.\n    unsafe { *p }\n}\n",
        Some("without a `// SAFETY:` comment"),
    ),
    (
        "allow-unsafe-code-outside-ffi",
        "crates/demo/src/lib.rs",
        "#![allow(unsafe_code)]\npub fn f() {}\n",
        Some("lint escape hatch"),
    ),
    (
        "file-named-ffi-is-not-a-directory",
        "crates/demo/src/ffi.rs",
        "// SAFETY: valid.\nunsafe fn f() {}\n",
        Some("outside an ffi/ or simd/"),
    ),
];

/// Planted manifests for the self-test: (name, text, marker).
pub const MANIFEST_CASES: &[(&str, &str, Option<&str>)] = &[
    (
        "control-manifest-forbid",
        "[lints.rust]\nunsafe_code = \"forbid\"\n",
        None,
    ),
    (
        "control-manifest-comment",
        "# unsafe_code = \"allow\" is not allowed\n[lints.rust]\nunsafe_code = \"deny\"\n",
        None,
    ),
    (
        "manifest-allows-unsafe-code",
        "[lints.rust]\nunsafe_code = \"allow\"\n",
        Some("unsafe_code = \"allow\""),
    ),
];

/// Judges one planted case: `None` when it behaved, else a problem description.
pub fn judge(
    kind: &str,
    name: &str,
    violations: &[String],
    marker: Option<&str>,
) -> Option<String> {
    match marker {
        None if !violations.is_empty() => {
            Some(format!("{kind} control `{name}` failed: {violations:?}"))
        }
        Some(_) if violations.is_empty() => Some(format!("{kind} plant `{name}` was NOT detected")),
        Some(m) if !violations.iter().any(|s| s.contains(m)) => Some(format!(
            "{kind} plant `{name}` failed for the wrong reason: {violations:?}"
        )),
        _ => None,
    }
}

/// Runs every planted case; returns problems (a control that failed or a plant that passed).
pub fn selftest() -> Vec<String> {
    let mut problems = Vec::new();
    for (name, path, src, marker) in CASES {
        let v = check_source(path, src, placement_allowed(path));
        problems.extend(judge("unsafe", name, &v, *marker));
    }
    for (name, text, marker) in MANIFEST_CASES {
        let v = check_manifest("Cargo.toml", text);
        problems.extend(judge("manifest", name, &v, *marker));
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_planted_case_behaves() {
        let problems = selftest();
        assert!(problems.is_empty(), "{problems:#?}");
    }

    #[test]
    fn placement_rules() {
        assert!(placement_allowed("crates/codecs/src/ffi/heif.rs"));
        assert!(placement_allowed("crates/imgproc/src/simd/mod.rs"));
        assert!(placement_allowed("crates/imgproc/src/kernels/simd/avx2.rs"));
        assert!(!placement_allowed("crates/imgproc/src/simd.rs"));
        assert!(!placement_allowed("crates/core/src/lib.rs"));
        assert!(placement_allowed("xtask/src/alloc_count.rs"));
        assert!(!placement_allowed("xtask/src/main.rs"));
        assert!(!placement_allowed("crates/foo/src/ffi_helpers/mod.rs"));
    }

    #[test]
    fn the_real_tree_is_clean() {
        // CARGO_MANIFEST_DIR is xtask/; the repository root is its parent.
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let v = check_tree(&root).unwrap();
        assert!(v.is_empty(), "{v:#?}");
    }

    #[test]
    fn a_planted_tree_fails() {
        let dir =
            std::env::temp_dir().join(format!("auto-crop-unsafe-guard-{}", std::process::id()));
        let src = dir.join("crates/demo/src");
        fs::create_dir_all(&src).unwrap();
        fs::write(
            src.join("lib.rs"),
            "pub fn f(p: *const u8) -> u8 {\n    // SAFETY: valid.\n    unsafe { *p }\n}\n",
        )
        .unwrap();
        let v = check_tree(&dir).unwrap();
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v[0].contains("crates/demo/src/lib.rs:3"), "{v:?}");
    }
}
