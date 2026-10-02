// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `cargo xtask fetch-corpus` and `cargo xtask corpus-ingest` (ROADMAP M1.36-M1.38).
//!
//! * [`lock`]: `corpus.lock.toml`, the pins (URL, size, SHA-256, SPDX licence, attribution).
//! * [`fetch`]: cache outside the repo, `curl`, verify-before-use, safe extraction.
//! * [`adapters`]: dataset trees to harness manifests.
//!
//! Developer guide: `docs/testing/corpora.md`. Nothing here runs in normal CI against the real
//! network; the tests use a loopback HTTP fixture server and synthetic dataset trees.

pub mod adapters;
pub mod fetch;
pub mod lock;

use adapters::common::{CorpusInfo, IngestOpts, SceneBy};
use std::path::{Path, PathBuf};

pub const DEFAULT_LOCK: &str = "corpus.lock.toml";

#[derive(Debug, Default)]
struct Args {
    name: Option<String>,
    sample: bool,
    list: bool,
    record_hash: bool,
    no_ingest: bool,
    lock: Option<String>,
    cache: Option<String>,
    src: Option<String>,
    out: Option<String>,
    opts: IngestOpts,
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = args.iter();
    let value = |it: &mut std::slice::Iter<'_, String>, flag: &str| -> Result<String, String> {
        it.next()
            .filter(|v| !v.starts_with("--"))
            .cloned()
            .ok_or_else(|| format!("{flag} needs a value"))
    };
    let number = |s: String, flag: &str| -> Result<usize, String> {
        s.parse::<usize>()
            .map_err(|_| format!("{flag}: not a number: {s}"))
    };
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--sample" => a.sample = true,
            "--list" => a.list = true,
            "--record-hash" => a.record_hash = true,
            "--no-ingest" => a.no_ingest = true,
            "--lock" => a.lock = Some(value(&mut it, "--lock")?),
            "--cache" => a.cache = Some(value(&mut it, "--cache")?),
            "--src" => a.src = Some(value(&mut it, "--src")?),
            "--out" => a.out = Some(value(&mut it, "--out")?),
            "--every" => a.opts.every = number(value(&mut it, "--every")?, "--every")?,
            "--dev-percent" => {
                let n = number(value(&mut it, "--dev-percent")?, "--dev-percent")?;
                if n > 100 {
                    return Err("--dev-percent must be 0..=100".to_owned());
                }
                a.opts.dev_percent = n as u32;
            }
            "--contact-sheet" => {
                a.opts.contact_sheet =
                    number(value(&mut it, "--contact-sheet")?, "--contact-sheet")?;
            }
            "--scene-by" => {
                a.opts.scene_by = Some(SceneBy::parse(&value(&mut it, "--scene-by")?)?);
            }
            flag if flag.starts_with("--") => return Err(format!("unknown option {flag}")),
            name => {
                if a.name.replace(name.to_owned()).is_some() {
                    return Err("only one corpus name may be given".to_owned());
                }
            }
        }
    }
    Ok(a)
}

fn load_lock(path: Option<&str>) -> Result<lock::LockFile, String> {
    let path = path.unwrap_or(DEFAULT_LOCK);
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let l = lock::parse(&text)?;
    let problems = lock::validate(&l);
    if problems.is_empty() {
        Ok(l)
    } else {
        Err(format!("{path} is invalid:\n  {}", problems.join("\n  ")))
    }
}

fn info_of(c: &lock::Corpus) -> CorpusInfo {
    CorpusInfo {
        name: c.name.clone(),
        adapter: c.adapter.clone(),
        spdx: c.spdx.clone(),
        attribution: c.attribution.clone(),
        licence_url: c.licence_url.clone(),
    }
}

fn list(l: &lock::LockFile) {
    for c in &l.corpora {
        let state = if lock::check_licence(c).is_err() {
            "licence not cleared (refused)".to_owned()
        } else if c.files.iter().any(|f| lock::check_fetchable(c, f).is_err()) {
            "unpinned (TODO-first-fetch, refused until recorded)".to_owned()
        } else {
            "pinned".to_owned()
        };
        let variants: Vec<&str> = c.files.iter().map(|f| f.variant.as_str()).collect();
        println!(
            "{:<18} {:<10} {:<12} {} [{}]",
            c.name,
            c.spdx,
            variants.join("+"),
            state,
            c.title
        );
    }
}

fn ingest_into(c: &lock::Corpus, src: &Path, out: &Path, opts: &IngestOpts) -> Result<(), String> {
    lock::check_licence(c)?;
    fetch::ensure_outside_repo(src, true)?;
    fetch::ensure_outside_repo(out, true)?;
    let report = adapters::ingest(&c.adapter, src, out, &info_of(c), opts)?;
    let text = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
    std::fs::write(out.join("ingest-report.json"), text + "\n").map_err(|e| e.to_string())?;
    println!("{}", report.summary());
    Ok(())
}

/// `cargo xtask fetch-corpus ...`
pub fn run_fetch(args: &[String]) -> Result<(), String> {
    let a = parse_args(args)?;
    let l = load_lock(a.lock.as_deref())?;
    if a.list {
        list(&l);
        return Ok(());
    }
    let name = a
        .name
        .as_deref()
        .ok_or("fetch-corpus needs a corpus name (see --list)")?;
    let corpus = l.find(name)?;
    let cache = fetch::cache_root(a.cache.as_deref())?;
    fetch::ensure_outside_repo(&cache, false)?;
    let variant = if a.sample { "sample" } else { "full" };
    if a.record_hash {
        let files: Vec<_> = corpus
            .files
            .iter()
            .filter(|f| f.variant == variant)
            .collect();
        if files.is_empty() {
            return Err(format!("{name}: the lock has no `{variant}` file"));
        }
        for f in files {
            let r = fetch::record_hash(&cache, corpus, f)?;
            println!(
                "\n# {}\nsize = {}\nsha256 = \"{}\"",
                r.url, r.size, r.sha256
            );
            match (f.pinned_size(), f.pinned_sha256()) {
                (Some(s), Some(h)) if s == r.size && h.eq_ignore_ascii_case(&r.sha256) => {
                    println!("# matches the pin already in the lock");
                }
                (Some(_), Some(_)) => println!("# DIFFERS from the pin in the lock: do not paste"),
                _ => println!("# review, then paste into corpus.lock.toml"),
            }
        }
        return Ok(());
    }
    let fetched = fetch::fetch_variant(&cache, corpus, variant)?;
    println!(
        "{name}: ready in {} ({} archive(s) downloaded or re-extracted this run)",
        fetched.src.display(),
        fetched.downloaded
    );
    if a.no_ingest {
        return Ok(());
    }
    ingest_into(corpus, &fetched.src, &fetched.src, &a.opts)
}

/// `cargo xtask corpus-ingest ...`
pub fn run_ingest(args: &[String]) -> Result<(), String> {
    let a = parse_args(args)?;
    let l = load_lock(a.lock.as_deref())?;
    let name = a
        .name
        .as_deref()
        .ok_or("corpus-ingest needs a corpus name")?;
    let corpus = l.find(name)?;
    let src = PathBuf::from(a.src.as_deref().ok_or("corpus-ingest needs --src DIR")?);
    let out = a.out.as_deref().map_or_else(|| src.clone(), PathBuf::from);
    ingest_into(corpus, &src, &out, &a.opts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_owned()).collect()
    }

    #[test]
    fn arguments_are_parsed_and_junk_is_rejected() {
        let a = parse_args(&s(&[
            "--sample",
            "--every",
            "5",
            "--scene-by",
            "document",
            "cord",
        ]))
        .unwrap();
        assert!(a.sample);
        assert_eq!(a.opts.every, 5);
        assert_eq!(a.opts.scene_by, Some(SceneBy::Document));
        assert_eq!(a.name.as_deref(), Some("cord"));
        for bad in [
            &["--every"][..],
            &["--every", "x"],
            &["--bogus"],
            &["a", "b"],
            &["--dev-percent", "101"],
            &["--scene-by", "frame"],
        ] {
            assert!(parse_args(&s(bad)).is_err(), "{bad:?}");
        }
    }
}
