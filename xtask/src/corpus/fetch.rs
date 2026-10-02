// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The download side of `cargo xtask fetch-corpus` (ROADMAP M1.36): the cache location, `curl`,
//! verification and safe extraction.
//!
//! * **Network access is `curl`, run as a separate program** (like `xtask build-native`), so
//!   xtask gains no HTTP or TLS crate. The shipped crates stay under the network guard of
//!   `docs/policy/ci-guards.md`; xtask is a developer tool, never shipped.
//! * **Cache outside the repository.** `--cache DIR`, else `AUTOCROP_CORPUS_CACHE`, else a
//!   per-user cache directory; a location inside the repository checkout is refused, so dataset
//!   bytes cannot end up in a `git add`.
//! * **Verified before use, refused on mismatch.** A download is written to `<file>.part`, its
//!   size and SHA-256 are compared with the lock entry and only then renamed into place. A
//!   mismatch deletes the file. A cached file is re-verified on every use.
//! * **Hostile archives.** The member list is checked for absolute paths and `..` before
//!   anything is written, and the extracted tree is scanned for symbolic links.

use super::lock::{self, Corpus, FileEntry};
use crate::native::sha256_file;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

pub const CACHE_ENV: &str = "AUTOCROP_CORPUS_CACHE";

/// The directory name the extracted tree of a variant lives in: `<cache>/<name>/<variant>/src`.
pub const SRC_DIR: &str = "src";
const MARKER: &str = ".auto-crop-extracted";

/// Where the cache lives: the explicit argument, else `AUTOCROP_CORPUS_CACHE`, else the
/// per-user cache directory (`%LOCALAPPDATA%\auto-crop\corpus`, `~/Library/Caches/auto-crop/corpus`
/// or `$XDG_CACHE_HOME/auto-crop/corpus`, falling back to `~/.cache/auto-crop/corpus`).
pub fn cache_root(explicit: Option<&str>) -> Result<PathBuf, String> {
    if let Some(p) = explicit.filter(|p| !p.is_empty()) {
        return Ok(PathBuf::from(p));
    }
    if let Ok(p) = std::env::var(CACHE_ENV)
        && !p.is_empty()
    {
        return Ok(PathBuf::from(p));
    }
    default_cache_root(&|k| std::env::var(k).ok())
}

/// [`cache_root`]'s fallback with the environment injected (so it is testable on any OS).
pub fn default_cache_root(env: &dyn Fn(&str) -> Option<String>) -> Result<PathBuf, String> {
    let nonempty = |k: &str| env(k).filter(|v| !v.is_empty());
    let base = if cfg!(windows) {
        nonempty("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        nonempty("HOME").map(|h| PathBuf::from(h).join("Library").join("Caches"))
    } else {
        nonempty("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| nonempty("HOME").map(|h| PathBuf::from(h).join(".cache")))
    };
    base.map(|b| b.join("auto-crop").join("corpus"))
        .ok_or_else(|| {
            format!("cannot find a per-user cache directory; set {CACHE_ENV} or pass --cache DIR")
        })
}

/// The nearest ancestor of the working directory that holds `.git` (a directory, or a file in a
/// worktree).
pub fn repo_root() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    cwd.ancestors()
        .find(|d| d.join(".git").exists())
        .map(Path::to_path_buf)
}

/// `path` made absolute with symbolic links and `..` resolved as far as it exists (the rest is
/// appended lexically), so a not-yet-created directory can still be compared with the repo.
pub fn resolve(path: &Path) -> PathBuf {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    let mut existing = abs.clone();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    while !existing.exists() {
        match existing.file_name() {
            Some(n) => tail.push(n.to_owned()),
            None => break,
        }
        if !existing.pop() {
            break;
        }
    }
    let mut out = existing.canonicalize().unwrap_or(existing);
    for part in tail.into_iter().rev() {
        if part == ".." {
            out.pop();
        } else if part != "." {
            out.push(part);
        }
    }
    out
}

/// Refuses a directory inside the repository checkout. `allow_target` permits `<repo>/target`
/// (ignored by git, wiped by `cargo clean`) for generated output such as manifests.
pub fn ensure_outside_repo(dir: &Path, allow_target: bool) -> Result<(), String> {
    ensure_outside(dir, repo_root().as_deref(), allow_target)
}

pub fn ensure_outside(dir: &Path, repo: Option<&Path>, allow_target: bool) -> Result<(), String> {
    let Some(repo) = repo else { return Ok(()) };
    let repo = resolve(repo);
    let dir = resolve(dir);
    if dir.starts_with(&repo) {
        if allow_target && dir.starts_with(repo.join("target")) {
            return Ok(());
        }
        return Err(format!(
            "REFUSED {}: inside the repository checkout {}; corpus data lives outside the repo \
             (set {CACHE_ENV} or pass --cache with a directory elsewhere)",
            dir.display(),
            repo.display()
        ));
    }
    Ok(())
}

/// `<cache>/<corpus>`
pub fn corpus_dir(cache: &Path, corpus: &Corpus) -> PathBuf {
    cache.join(&corpus.name)
}

/// The extracted tree of a variant.
pub fn variant_src(cache: &Path, corpus: &Corpus, variant: &str) -> PathBuf {
    corpus_dir(cache, corpus).join(variant).join(SRC_DIR)
}

fn downloads_dir(cache: &Path, corpus: &Corpus) -> PathBuf {
    corpus_dir(cache, corpus).join("downloads")
}

fn cached_path(cache: &Path, corpus: &Corpus, f: &FileEntry) -> PathBuf {
    let sha = f.pinned_sha256().unwrap_or("unpinned").to_lowercase();
    downloads_dir(cache, corpus).join(format!("{}-{}", &sha[..sha.len().min(16)], f.file_name()))
}

/// The `curl` arguments for one download. `--proto` and `--proto-redir` pin the scheme (a
/// redirect may only lead to https), `--max-filesize` stops a server that sends more than the
/// pinned size, and the error is a non-zero exit (`--fail`), never an HTML error page on disk.
pub fn curl_args(url: &str, scheme: &str, out: &Path, max_size: Option<u64>) -> Vec<String> {
    let mut a: Vec<String> = [
        "--fail",
        "--silent",
        "--show-error",
        "--location",
        "--max-redirs",
        "5",
        "--retry",
        "2",
        "--proto",
    ]
    .map(str::to_owned)
    .to_vec();
    a.push(format!("={scheme}"));
    a.push("--proto-redir".to_owned());
    a.push("=https".to_owned());
    if let Some(n) = max_size {
        a.push("--max-filesize".to_owned());
        a.push(n.to_string());
    }
    a.push("--output".to_owned());
    a.push(out.display().to_string());
    a.push(url.to_owned());
    a
}

fn download(url: &str, dest: &Path, max_size: Option<u64>) -> Result<(), String> {
    let scheme = lock::url_scheme_ok(url)?;
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let _ = fs::remove_file(dest);
    let status = Command::new("curl")
        .args(curl_args(url, scheme, dest, max_size))
        .status()
        .map_err(|e| format!("cannot run curl ({e}); install curl, xtask uses it for downloads"))?;
    if status.success() {
        Ok(())
    } else {
        let _ = fs::remove_file(dest);
        Err(format!("download of {url} failed (curl {status})"))
    }
}

/// Size, then SHA-256; the message of a mismatch starts with `REFUSED`.
pub fn verify(path: &Path, size: u64, sha256: &str) -> Result<(), String> {
    let got_size = fs::metadata(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .len();
    if got_size != size {
        return Err(format!(
            "REFUSED {}: size {got_size} does not match the pinned {size}",
            path.display()
        ));
    }
    let got = sha256_file(path)?;
    if got.eq_ignore_ascii_case(sha256) {
        Ok(())
    } else {
        Err(format!(
            "REFUSED {}: sha256 {got} does not match the pin {sha256}",
            path.display()
        ))
    }
}

/// Downloads (or finds in the cache) and verifies one file; returns its path in the cache.
pub fn fetch_file(cache: &Path, corpus: &Corpus, f: &FileEntry) -> Result<PathBuf, String> {
    lock::check_fetchable(corpus, f)?;
    let (size, sha) = (
        f.pinned_size().unwrap_or(0),
        f.pinned_sha256().unwrap_or(""),
    );
    let dest = cached_path(cache, corpus, f);
    if dest.exists() {
        match verify(&dest, size, sha) {
            Ok(()) => {
                println!("{}: cached and verified {}", corpus.name, dest.display());
                return Ok(dest);
            }
            Err(e) => {
                let _ = fs::remove_file(&dest);
                println!(
                    "{}: cached copy removed ({e}); downloading again",
                    corpus.name
                );
            }
        }
    }
    let part = PathBuf::from(format!("{}.part", dest.display()));
    println!("{}: downloading {} ({size} bytes)", corpus.name, f.url);
    download(&f.url, &part, Some(size))?;
    if let Err(e) = verify(&part, size, sha) {
        let _ = fs::remove_file(&part);
        return Err(format!("{e} (the download was deleted)"));
    }
    fs::rename(&part, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    println!("{}: verified {}", corpus.name, dest.display());
    Ok(dest)
}

/// The result of `--record-hash` for one file.
#[derive(Debug, PartialEq, Eq)]
pub struct Recorded {
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

/// Trust-on-first-use: downloads into `<cache>/_quarantine`, measures, deletes the file and
/// returns the numbers to review and paste into the lock. Nothing is extracted or kept.
pub fn record_hash(cache: &Path, corpus: &Corpus, f: &FileEntry) -> Result<Recorded, String> {
    lock::check_licence(corpus)?;
    lock::url_scheme_ok(&f.url).map_err(|e| format!("REFUSED {}: {e}", corpus.name))?;
    let dir = cache.join("_quarantine").join(&corpus.name);
    let part = dir.join(format!("{}.part", f.file_name()));
    println!("{}: recording {} into quarantine", corpus.name, f.url);
    download(&f.url, &part, None)?;
    let size = fs::metadata(&part).map_err(|e| e.to_string())?.len();
    let sha256 = sha256_file(&part);
    let _ = fs::remove_file(&part);
    let _ = fs::remove_dir(&dir);
    Ok(Recorded {
        url: f.url.clone(),
        size,
        sha256: sha256?,
    })
}

/// Tar listings and zip listings can name `/etc/x`, `../x` or `C:\x`.
pub fn unsafe_member(name: &str) -> bool {
    let n = name.trim_end_matches(['\r', '\n']);
    n.starts_with(['/', '\\'])
        || n.contains('\0')
        || n.as_bytes().get(1) == Some(&b':')
        || n.split(['/', '\\']).any(|c| c == "..")
}

fn tar_program() -> PathBuf {
    // On Windows a `tar` earlier on PATH (Git's GNU tar) misreads `C:\...` as host:path; the
    // system bsdtar handles both tar and zip.
    if cfg!(windows)
        && let Some(root) = std::env::var_os("SystemRoot")
    {
        let p = PathBuf::from(root).join("System32").join("tar.exe");
        if p.exists() {
            return p;
        }
    }
    PathBuf::from("tar")
}

fn run_capture(cmd: &mut Command, what: &str) -> Result<String, String> {
    let out = cmd
        .output()
        .map_err(|e| format!("{what}: cannot run: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{what} failed with {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn zip_uses_unzip() -> bool {
    !cfg!(windows) && !cfg!(target_os = "macos")
}

/// Lists the archive's members.
fn list_members(archive: &Path, kind: &str) -> Result<Vec<String>, String> {
    let text = if kind == "zip" && zip_uses_unzip() {
        run_capture(Command::new("unzip").arg("-Z1").arg(archive), "unzip -Z1")?
    } else {
        run_capture(
            Command::new(tar_program()).arg("-tf").arg(archive),
            "tar -tf",
        )?
    };
    Ok(text.lines().map(str::to_owned).collect())
}

/// Extracts `archive` into the existing directory `dest` after checking every member name, then
/// refuses the result if it contains a symbolic link.
pub fn extract(archive: &Path, kind: &str, dest: &Path) -> Result<(), String> {
    for m in list_members(archive, kind)? {
        if unsafe_member(&m) {
            return Err(format!(
                "REFUSED {}: member `{m}` would escape the extraction directory",
                archive.display()
            ));
        }
    }
    let status = if kind == "zip" && zip_uses_unzip() {
        Command::new("unzip")
            .args(["-q", "-o"])
            .arg(archive)
            .arg("-d")
            .arg(dest)
            .status()
    } else {
        Command::new(tar_program())
            .arg("-xf")
            .arg(archive)
            .arg("-C")
            .arg(dest)
            .status()
    }
    .map_err(|e| format!("cannot extract {}: {e}", archive.display()))?;
    if !status.success() {
        return Err(format!(
            "extracting {} failed with {status}",
            archive.display()
        ));
    }
    reject_symlinks(dest)
}

fn reject_symlinks(dir: &Path) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let ty = entry.file_type().map_err(|e| e.to_string())?;
        if ty.is_symlink() {
            return Err(format!(
                "REFUSED: the archive contains a symbolic link: {}",
                entry.path().display()
            ));
        }
        if ty.is_dir() {
            reject_symlinks(&entry.path())?;
        }
    }
    Ok(())
}

/// What `fetch_variant` did.
#[derive(Debug)]
pub struct Fetched {
    pub src: PathBuf,
    pub downloaded: usize,
}

/// Fetches, verifies and extracts every file of `variant`; returns the extracted tree. Every
/// entry is checked (placeholders, licence, URL) before the first byte is requested, so a lock
/// with one unpinned file fetches nothing at all.
pub fn fetch_variant(cache: &Path, corpus: &Corpus, variant: &str) -> Result<Fetched, String> {
    let files: Vec<&FileEntry> = corpus
        .files
        .iter()
        .filter(|f| f.variant == variant)
        .collect();
    if files.is_empty() {
        let have: Vec<_> = corpus.files.iter().map(|f| f.variant.as_str()).collect();
        return Err(format!(
            "{}: the lock has no `{variant}` file (it has: {}); {}",
            corpus.name,
            have.join(", "),
            if variant == "sample" {
                "this corpus has no sample, run without --sample"
            } else {
                "nothing to fetch"
            }
        ));
    }
    for f in &files {
        lock::check_fetchable(corpus, f)?;
    }
    let mut paths = Vec::new();
    for f in &files {
        paths.push(fetch_file(cache, corpus, f)?);
    }
    let src = variant_src(cache, corpus, variant);
    let stamp = files
        .iter()
        .map(|f| f.pinned_sha256().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");
    if fs::read_to_string(src.join(MARKER)).is_ok_and(|m| m == stamp) {
        return Ok(Fetched { src, downloaded: 0 });
    }
    // Extract into a sibling directory and swap it in, so an interrupted run never leaves a
    // half-extracted tree behind a valid marker.
    let tmp = src.with_file_name(format!("{SRC_DIR}.tmp"));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let result = (|| {
        for (f, p) in files.iter().zip(&paths) {
            match f.extract.as_str() {
                "none" => {
                    fs::copy(p, tmp.join(f.file_name())).map_err(|e| e.to_string())?;
                }
                kind => extract(p, kind, &tmp)?,
            }
        }
        fs::write(tmp.join(MARKER), &stamp).map_err(|e| e.to_string())
    })();
    if let Err(e) = result {
        let _ = fs::remove_dir_all(&tmp);
        return Err(e);
    }
    let _ = fs::remove_dir_all(&src);
    fs::rename(&tmp, &src).map_err(|e| format!("{}: {e}", src.display()))?;
    Ok(Fetched {
        src,
        downloaded: files.len(),
    })
}

/// True when `p` is a plain relative path with no `..` (used for names read from data files).
pub fn is_plain_relative(p: &str) -> bool {
    let path = Path::new(p);
    !p.is_empty()
        && !p.contains(['\\', ':', '\0'])
        && !path.is_absolute()
        && path
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostile_member_names_are_unsafe() {
        for bad in [
            "/etc/passwd",
            "../x",
            "a/../../x",
            "a\\..\\x",
            "C:\\x",
            "c:/x",
            "\\\\server\\share",
            "a\0b",
        ] {
            assert!(unsafe_member(bad), "{bad:?}");
        }
        for ok in ["a/b.png", "frames/bg01/x..y.png", "./a", "dir/"] {
            assert!(!unsafe_member(ok), "{ok:?}");
        }
    }

    #[test]
    fn curl_is_pinned_to_the_scheme_and_https_redirects() {
        let a = curl_args("https://h/x", "https", Path::new("o.part"), Some(10));
        let joined = a.join(" ");
        assert!(joined.contains("--proto =https"), "{joined}");
        assert!(joined.contains("--proto-redir =https"), "{joined}");
        assert!(joined.contains("--max-filesize 10"), "{joined}");
        assert!(a.contains(&"--fail".to_owned()));
        assert_eq!(a.last().map(String::as_str), Some("https://h/x"));
        let a = curl_args("ftp://h/x", "ftp", Path::new("o"), None);
        assert!(a.join(" ").contains("--proto =ftp"));
        assert!(!a.contains(&"--max-filesize".to_owned()));
    }

    #[test]
    fn the_default_cache_is_a_per_user_directory() {
        let env = |k: &str| match k {
            "LOCALAPPDATA" => Some("C:/Users/u/AppData/Local".to_owned()),
            "XDG_CACHE_HOME" => Some("/xdg".to_owned()),
            "HOME" => Some("/home/u".to_owned()),
            _ => None,
        };
        let p = default_cache_root(&env).unwrap();
        assert!(p.ends_with(Path::new("auto-crop").join("corpus")), "{p:?}");
        assert!(default_cache_root(&|_| None).is_err());
    }

    #[test]
    fn a_cache_inside_the_repository_is_refused_but_target_is_allowed_for_output() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let inside = repo.join("corpus-cache").join("x");
        let e = ensure_outside(&inside, Some(&repo), false).unwrap_err();
        assert!(e.contains("inside the repository"), "{e}");
        assert!(ensure_outside(&repo.join("target").join("c"), Some(&repo), false).is_err());
        assert!(ensure_outside(&repo.join("target").join("c"), Some(&repo), true).is_ok());
        // `..` cannot be used to sneak back in.
        let sneaky = repo.join("target").join("..").join("corpus-cache");
        assert!(ensure_outside(&sneaky, Some(&repo), true).is_err());
        let elsewhere = std::env::temp_dir().join("auto-crop-not-in-repo");
        assert!(ensure_outside(&elsewhere, Some(&repo), false).is_ok());
    }

    #[test]
    fn plain_relative_paths() {
        assert!(is_plain_relative("a/b/c.raw"));
        for bad in ["", "/a", "../a", "a/../b", "C:/a", "a\\b"] {
            assert!(!is_plain_relative(bad), "{bad}");
        }
    }
}
