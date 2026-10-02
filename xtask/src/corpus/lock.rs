// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `corpus.lock.toml`: what a public corpus is, where it comes from and exactly which bytes are
//! acceptable (ROADMAP M1.36).
//!
//! ```toml
//! version = 1
//!
//! [[corpus]]
//! name = "smartdoc2015-ch1"          # what you type after `fetch-corpus`
//! adapter = "smartdoc2015-ch1"       # which manifest adapter reads the extracted tree
//! spdx = "CC-BY-4.0"                 # licence of the DATA (not of this repository)
//! licence_url = "https://zenodo.org/records/1230217"
//! attribution = "Cite ... "          # carried into every manifest line
//!
//! [[corpus.file]]
//! variant = "sample"                 # `sample` (for --sample) or `full`
//! url = "https://..."
//! size = 21000000                    # bytes; or the placeholder "TODO-first-fetch"
//! sha256 = "<64 hex>"                # or the placeholder "TODO-first-fetch"
//! extract = "tar"                    # tar (also .tar.gz), zip or none
//! ```
//!
//! A placeholder pin is **rejected**: `fetch-corpus` refuses to download anything whose size or
//! SHA-256 is not recorded, so unverified bytes never reach the cache by accident. The one way
//! to learn the pin is the explicit `--record-hash` mode (trust on first use, printed for the
//! maintainer to review and paste; nothing is extracted).

use serde::Deserialize;
use std::collections::BTreeSet;

/// The marker written into a lock entry that has not been fetched and pinned yet.
pub const PLACEHOLDER: &str = "TODO-first-fetch";

/// Licences of the data that may be fetched and ingested (B2: nothing share-alike, nothing
/// non-commercial, nothing unknown). Anything else, including `NOASSERTION`, is refused.
pub const CLEARED_SPDX: &[&str] = &[
    "CC0-1.0",
    "CC-BY-4.0",
    "MIT",
    "Apache-2.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
];

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum Size {
    Bytes(u64),
    Text(String),
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileEntry {
    /// `sample` or `full`.
    #[serde(default = "default_variant")]
    pub variant: String,
    pub url: String,
    /// File name in the cache; defaults to the last path segment of the URL.
    #[serde(default)]
    pub filename: String,
    pub size: Size,
    pub sha256: String,
    /// `tar` (also `.tar.gz`), `zip` or `none` (keep the file as it is).
    #[serde(default = "default_extract")]
    pub extract: String,
}

fn default_variant() -> String {
    "full".to_owned()
}

fn default_extract() -> String {
    "tar".to_owned()
}

impl FileEntry {
    pub fn file_name(&self) -> String {
        if !self.filename.is_empty() {
            return self.filename.clone();
        }
        let path = self.url.split(['?', '#']).next().unwrap_or("");
        path.rsplit('/').next().unwrap_or("download").to_owned()
    }

    /// The pinned size, when it is a real number.
    pub fn pinned_size(&self) -> Option<u64> {
        match self.size {
            Size::Bytes(n) if n > 0 => Some(n),
            _ => None,
        }
    }

    pub fn pinned_sha256(&self) -> Option<&str> {
        let s = self.sha256.as_str();
        (s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())).then_some(s)
    }
}

#[derive(Debug, Clone, Deserialize)]
/// (`notes` and other free-text keys in the file are for people and ignored here.)
pub struct Corpus {
    pub name: String,
    pub adapter: String,
    #[serde(default)]
    pub title: String,
    pub spdx: String,
    #[serde(default)]
    pub licence_url: String,
    #[serde(default)]
    pub attribution: String,
    #[serde(default, rename = "file")]
    pub files: Vec<FileEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LockFile {
    #[serde(default)]
    pub version: u32,
    #[serde(default, rename = "corpus")]
    pub corpora: Vec<Corpus>,
}

pub fn parse(text: &str) -> Result<LockFile, String> {
    let lock: LockFile = toml::from_str(text).map_err(|e| format!("corpus.lock.toml: {e}"))?;
    if lock.version != 1 {
        return Err(format!(
            "corpus.lock.toml: version must be 1, found {}",
            lock.version
        ));
    }
    Ok(lock)
}

impl LockFile {
    pub fn find(&self, name: &str) -> Result<&Corpus, String> {
        self.corpora.iter().find(|c| c.name == name).ok_or_else(|| {
            let known: Vec<_> = self.corpora.iter().map(|c| c.name.as_str()).collect();
            format!("unknown corpus `{name}` (known: {})", known.join(", "))
        })
    }
}

/// `https` always; `ftp` for the datasets that only publish there (the pinned hash is what
/// protects the bytes); plain `http` only for a loopback host, which is how the test suite
/// serves its fixtures.
pub fn url_scheme_ok(url: &str) -> Result<&'static str, String> {
    if url.starts_with("https://") {
        return Ok("https");
    }
    if url.starts_with("ftp://") {
        return Ok("ftp");
    }
    if let Some(rest) = url.strip_prefix("http://") {
        let host = rest.split(['/', '?', '#']).next().unwrap_or("");
        let host = host.rsplit('@').next().unwrap_or(host);
        let host = host
            .rsplit_once(':')
            .filter(|(_, port)| port.chars().all(|c| c.is_ascii_digit()))
            .map_or(host, |(h, _)| h);
        if !rest
            .split(['/', '?', '#'])
            .next()
            .unwrap_or("")
            .contains('@')
            && (host == "127.0.0.1" || host == "localhost" || host == "[::1]")
        {
            return Ok("http");
        }
        return Err(format!(
            "`{url}`: plain http is only allowed for a loopback host (use https)"
        ));
    }
    Err(format!(
        "`{url}`: only https, ftp (or loopback http) URLs are allowed"
    ))
}

fn is_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !s.starts_with('-')
}

fn safe_file_name(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && !s.contains(['/', '\\', ':', '\0'])
        && !s.starts_with('.')
}

/// Structural problems of the whole lock file (what CI checks even when nothing is fetched).
/// Placeholder pins are fine here: they are legal in the file and refused at fetch time.
pub fn validate(lock: &LockFile) -> Vec<String> {
    let mut out = Vec::new();
    let mut names = BTreeSet::new();
    for c in &lock.corpora {
        if !is_name(&c.name) {
            out.push(format!(
                "corpus `{}`: name must be lowercase a-z, 0-9, `-`",
                c.name
            ));
        }
        if !names.insert(c.name.as_str()) {
            out.push(format!("duplicate corpus `{}`", c.name));
        }
        if c.spdx.trim().is_empty() {
            out.push(format!("{}: spdx is missing", c.name));
        }
        if c.licence_url.is_empty() {
            out.push(format!("{}: licence_url is missing", c.name));
        }
        if CLEARED_SPDX.contains(&c.spdx.as_str())
            && c.spdx != "CC0-1.0"
            && c.attribution.trim().is_empty()
        {
            out.push(format!(
                "{}: {} requires an attribution text",
                c.name, c.spdx
            ));
        }
        if c.files.is_empty() {
            out.push(format!("{}: no [[corpus.file]] entries", c.name));
        }
        for f in &c.files {
            if let Err(e) = url_scheme_ok(&f.url) {
                out.push(format!("{}: {e}", c.name));
            }
            if !["sample", "full"].contains(&f.variant.as_str()) {
                out.push(format!(
                    "{}: variant must be `sample` or `full`, found `{}`",
                    c.name, f.variant
                ));
            }
            if !["tar", "zip", "none"].contains(&f.extract.as_str()) {
                out.push(format!("{}: extract must be tar, zip or none", c.name));
            }
            if !safe_file_name(&f.file_name()) {
                out.push(format!("{}: unsafe file name `{}`", c.name, f.file_name()));
            }
            let size_ok =
                f.pinned_size().is_some() || matches!(&f.size, Size::Text(t) if t == PLACEHOLDER);
            if !size_ok {
                out.push(format!(
                    "{}: size must be a positive integer or \"{PLACEHOLDER}\"",
                    c.name
                ));
            }
            if f.pinned_sha256().is_none() && f.sha256 != PLACEHOLDER {
                out.push(format!(
                    "{}: sha256 must be 64 hex characters or \"{PLACEHOLDER}\"",
                    c.name
                ));
            }
        }
    }
    out
}

/// Why `file` of `corpus` may not be fetched yet, or `Ok`. This is the gate in front of every
/// download: a placeholder or malformed pin, an uncleared licence and an insecure URL are all
/// refused with the reason spelled out.
pub fn check_fetchable(corpus: &Corpus, file: &FileEntry) -> Result<(), String> {
    let name = &corpus.name;
    check_licence(corpus)?;
    url_scheme_ok(&file.url).map_err(|e| format!("REFUSED {name}: {e}"))?;
    if !safe_file_name(&file.file_name()) {
        return Err(format!(
            "REFUSED {name}: unsafe file name `{}`",
            file.file_name()
        ));
    }
    if file.sha256 == PLACEHOLDER || matches!(&file.size, Size::Text(t) if t == PLACEHOLDER) {
        return Err(format!(
            "REFUSED {name}: the lock entry for {} still has the placeholder pin `{PLACEHOLDER}`; \
             nothing is downloaded without a recorded size and SHA-256. Run `cargo xtask \
             fetch-corpus --record-hash {name}` once (trust on first use: it downloads into a \
             quarantine, prints size and hash, extracts nothing and deletes the file), review \
             the values and paste them into corpus.lock.toml",
            file.url
        ));
    }
    if file.pinned_sha256().is_none() {
        return Err(format!(
            "REFUSED {name}: sha256 `{}` is not 64 hex characters",
            file.sha256
        ));
    }
    if file.pinned_size().is_none() {
        return Err(format!(
            "REFUSED {name}: size {:?} is not a positive integer",
            file.size
        ));
    }
    Ok(())
}

/// The licence gate shared by fetch and ingest.
pub fn check_licence(corpus: &Corpus) -> Result<(), String> {
    if CLEARED_SPDX.contains(&corpus.spdx.as_str()) {
        Ok(())
    } else {
        Err(format!(
            "REFUSED {}: licence `{}` is not cleared for use (cleared: {}); see docs/policy/provenance",
            corpus.name,
            corpus.spdx,
            CLEARED_SPDX.join(", ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"
version = 1
[[corpus]]
name = "demo"
adapter = "smartdoc2015-ch1"
spdx = "CC-BY-4.0"
licence_url = "https://example.org/l"
attribution = "Cite it"
  [[corpus.file]]
  variant = "sample"
  url = "https://example.org/a.tar.gz"
  size = 10
  sha256 = "0000000000000000000000000000000000000000000000000000000000000000"
"#;

    #[test]
    fn a_complete_entry_is_valid_and_fetchable() {
        let l = parse(GOOD).unwrap();
        assert!(validate(&l).is_empty(), "{:?}", validate(&l));
        let c = l.find("demo").unwrap();
        assert_eq!(c.files[0].file_name(), "a.tar.gz");
        check_fetchable(c, &c.files[0]).unwrap();
    }

    #[test]
    fn placeholder_pins_are_legal_in_the_file_but_refused_with_a_reason() {
        let t = GOOD
            .replace("size = 10", "size = \"TODO-first-fetch\"")
            .replace(&"0".repeat(64), "TODO-first-fetch");
        let l = parse(&t).unwrap();
        assert!(validate(&l).is_empty());
        let c = l.find("demo").unwrap();
        let e = check_fetchable(c, &c.files[0]).unwrap_err();
        assert!(
            e.contains("REFUSED") && e.contains("placeholder") && e.contains("--record-hash"),
            "{e}"
        );
        // One placeholder is enough.
        let only_hash = GOOD.replace(&"0".repeat(64), "TODO-first-fetch");
        let l = parse(&only_hash).unwrap();
        let c = l.find("demo").unwrap();
        assert!(check_fetchable(c, &c.files[0]).is_err());
    }

    #[test]
    fn garbage_pins_and_bad_urls_are_invalid() {
        let bad_hash = GOOD.replace(&"0".repeat(64), "abc");
        assert!(
            validate(&parse(&bad_hash).unwrap())
                .iter()
                .any(|e| e.contains("sha256"))
        );
        let bad_size = GOOD.replace("size = 10", "size = \"big\"");
        assert!(
            validate(&parse(&bad_size).unwrap())
                .iter()
                .any(|e| e.contains("size"))
        );
        let http = GOOD.replace("https://example.org/a", "http://example.org/a");
        assert!(
            validate(&parse(&http).unwrap())
                .iter()
                .any(|e| e.contains("loopback"))
        );
        let ok_loop = GOOD.replace("https://example.org/a", "http://127.0.0.1:9/a");
        assert!(validate(&parse(&ok_loop).unwrap()).is_empty());
        let sneaky = GOOD.replace("https://example.org/a", "http://127.0.0.1.evil.org/a");
        assert!(!validate(&parse(&sneaky).unwrap()).is_empty());
        let userinfo = GOOD.replace("https://example.org/a", "http://127.0.0.1@evil.org/a");
        assert!(!validate(&parse(&userinfo).unwrap()).is_empty());
        let dup = format!("{GOOD}\n{}", GOOD.replace("version = 1", ""));
        assert!(
            validate(&parse(&dup).unwrap())
                .iter()
                .any(|e| e.contains("duplicate"))
        );
    }

    #[test]
    fn uncleared_licences_are_refused() {
        for spdx in [
            "NOASSERTION",
            "CC-BY-NC-4.0",
            "CC-BY-SA-4.0",
            "GPL-3.0-only",
            "",
        ] {
            let t = GOOD.replace("CC-BY-4.0", spdx);
            let l = parse(&t).unwrap();
            let c = l.find("demo").unwrap();
            let e = check_fetchable(c, &c.files[0]).unwrap_err();
            assert!(e.contains("not cleared"), "{spdx}: {e}");
        }
    }

    #[test]
    fn the_repository_lock_file_is_valid_and_every_real_entry_is_blocked_until_pinned() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../corpus.lock.toml");
        let text = std::fs::read_to_string(path).unwrap();
        let l = parse(&text).unwrap();
        let problems = validate(&l);
        assert!(problems.is_empty(), "{problems:#?}");
        for name in [
            "smartdoc2015-ch1",
            "cord",
            "midv-500",
            "dibco",
            "rawpixls-cc0",
        ] {
            let c = l.find(name).unwrap_or_else(|e| panic!("{e}"));
            for f in &c.files {
                assert!(
                    check_fetchable(c, f).is_err(),
                    "{name}: a real dataset entry must not be fetchable before its first-fetch pin"
                );
            }
        }
    }
}
