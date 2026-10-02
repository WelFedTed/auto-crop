// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! ROADMAP M1.36: `cargo xtask fetch-corpus` against a LOCAL HTTP fixture server that also
//! serves bad downloads. No real network, no real dataset: every byte is generated here.
//! Covers: verified download, extraction and ingest; cache reuse without a request; refusal (and
//! deletion) on a hash or size mismatch, a truncated transfer, an error status and an http
//! redirect; refusal of placeholder pins, uncleared licences and non-loopback http before any
//! request; trust-on-first-use `--record-hash`; hostile archives; a cache inside the repository.

mod common;

use common::*;
use std::path::Path;

struct Fixture {
    tmp: Tmp,
    cache: std::path::PathBuf,
    archive: Vec<u8>,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let tmp = Tmp::new(tag);
        let cache = tmp.join("cache");
        let (entries, _) = smartdoc_entries();
        let members: Vec<(&str, Vec<u8>)> = entries
            .iter()
            .map(|(n, b)| (n.as_str(), b.clone()))
            .collect();
        Self {
            archive: tar(&members),
            tmp,
            cache,
        }
    }

    fn lock(&self, url: &str, size: usize, sha: &str, variant: &str) -> std::path::PathBuf {
        let p = self.tmp.join("corpus.lock.toml");
        std::fs::write(
            &p,
            lock_text(&[LockSpec {
                name: "fixture-smartdoc",
                adapter: "smartdoc2015-ch1",
                spdx: "CC-BY-4.0",
                variant,
                url,
                size: size.to_string(),
                sha256: sha.to_owned(),
                extract: "tar",
            }]),
        )
        .unwrap();
        p
    }

    fn run(&self, lock: &Path, extra: &[&str]) -> std::process::Output {
        let mut args = vec!["fetch-corpus", "--lock", lock.to_str().unwrap()];
        args.extend_from_slice(extra);
        xtask(self.tmp.path(), &self.cache, &args)
    }

    fn downloads(&self) -> Vec<String> {
        let d = self.cache.join("fixture-smartdoc").join("downloads");
        std::fs::read_dir(d)
            .map(|rd| {
                rd.filter_map(Result::ok)
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn src(&self, variant: &str) -> std::path::PathBuf {
        self.cache
            .join("fixture-smartdoc")
            .join(variant)
            .join("src")
    }
}

#[test]
fn a_good_download_is_verified_extracted_ingested_and_then_served_from_the_cache() {
    let f = Fixture::new("good");
    let server = Server::start(vec![("/sample.tar", Route::Bytes(f.archive.clone()))]);
    let lock = f.lock(
        &server.url("/sample.tar"),
        f.archive.len(),
        &sha256_hex(&f.archive),
        "sample",
    );

    let out = f.run(&lock, &["--sample", "fixture-smartdoc"]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(server.hits(), vec!["/sample.tar"]);
    let manifest = auto_crop_eval::manifest::load(&f.src("sample").join("manifest.jsonl"))
        .expect("the ingested manifest validates");
    assert_eq!(
        manifest.items.len(),
        12,
        "4 clips x every 10th of 25 frames"
    );
    let log = text(&out);
    assert!(log.contains("verified"), "{log}");

    // Second run: the cached archive is re-verified locally and no request is made.
    let out = f.run(&lock, &["--sample", "fixture-smartdoc"]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(
        server.hits().len(),
        1,
        "cache hit must not touch the network"
    );
    assert!(text(&out).contains("cached and verified"), "{}", text(&out));
}

#[test]
fn a_corrupted_cached_copy_is_discarded_and_fetched_again() {
    let f = Fixture::new("cachefix");
    let server = Server::start(vec![("/a.tar", Route::Bytes(f.archive.clone()))]);
    let lock = f.lock(
        &server.url("/a.tar"),
        f.archive.len(),
        &sha256_hex(&f.archive),
        "full",
    );
    assert!(
        f.run(&lock, &["--no-ingest", "fixture-smartdoc"])
            .status
            .success()
    );
    let name = f.downloads().into_iter().next().expect("cached file");
    let path = f.cache.join("fixture-smartdoc/downloads").join(&name);
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[600] ^= 0xff;
    std::fs::write(&path, bytes).unwrap();
    let out = f.run(&lock, &["--no-ingest", "fixture-smartdoc"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("cached copy removed"), "{}", text(&out));
    assert_eq!(server.hits().len(), 2);
    assert_eq!(std::fs::read(&path).unwrap(), f.archive);
}

#[test]
fn tampered_bytes_are_refused_and_nothing_is_kept() {
    let f = Fixture::new("tamper");
    let mut bad = f.archive.clone();
    bad[700] ^= 0x01; // same length, different content
    let server = Server::start(vec![("/a.tar", Route::Bytes(bad))]);
    let lock = f.lock(
        &server.url("/a.tar"),
        f.archive.len(),
        &sha256_hex(&f.archive),
        "full",
    );
    let out = f.run(&lock, &["fixture-smartdoc"]);
    let log = text(&out);
    assert!(!out.status.success(), "{log}");
    assert!(
        log.contains("REFUSED") && log.contains("sha256") && log.contains("does not match"),
        "{log}"
    );
    assert!(
        f.downloads().is_empty(),
        "no file may stay in the cache: {:?}",
        f.downloads()
    );
    assert!(!f.src("full").exists(), "nothing may be extracted");
}

#[test]
fn a_wrong_size_is_refused_whether_it_is_too_big_or_too_small() {
    let f = Fixture::new("size");
    let mut longer = f.archive.clone();
    longer.extend_from_slice(&[0u8; 2048]);
    let server = Server::start(vec![
        ("/long.tar", Route::Bytes(longer)),
        (
            "/short.tar",
            Route::Bytes(f.archive[..f.archive.len() - 512].to_vec()),
        ),
    ]);
    for path in ["/long.tar", "/short.tar"] {
        let lock = f.lock(
            &server.url(path),
            f.archive.len(),
            &sha256_hex(&f.archive),
            "full",
        );
        let out = f.run(&lock, &["fixture-smartdoc"]);
        let log = text(&out);
        assert!(!out.status.success(), "{path}: {log}");
        assert!(
            log.contains("REFUSED") || log.contains("download of"),
            "{path}: {log}"
        );
        assert!(f.downloads().is_empty(), "{path}: {:?}", f.downloads());
        assert!(!f.src("full").exists(), "{path}");
    }
}

#[test]
fn a_truncated_transfer_an_error_status_and_a_redirect_to_http_all_fail_cleanly() {
    let f = Fixture::new("badnet");
    let server = Server::start(vec![
        (
            "/cut.tar",
            Route::Truncated {
                claimed: f.archive.len(),
                body: f.archive[..2000].to_vec(),
            },
        ),
        ("/err.tar", Route::Status(403)),
        (
            "/redir.tar",
            Route::Redirect("http://127.0.0.1:1/x.tar".to_owned()),
        ),
    ]);
    for path in ["/cut.tar", "/err.tar", "/redir.tar", "/missing.tar"] {
        let lock = f.lock(
            &server.url(path),
            f.archive.len(),
            &sha256_hex(&f.archive),
            "full",
        );
        let out = f.run(&lock, &["fixture-smartdoc"]);
        let log = text(&out);
        assert!(!out.status.success(), "{path}: {log}");
        assert!(log.contains("download of"), "{path}: {log}");
        assert!(f.downloads().is_empty(), "{path}: {:?}", f.downloads());
        assert!(!f.src("full").exists(), "{path}");
    }
}

#[test]
fn a_placeholder_pin_is_refused_with_the_reason_and_no_request_is_made() {
    let f = Fixture::new("placeholder");
    let server = Server::start(vec![("/a.tar", Route::Bytes(f.archive.clone()))]);
    for (size, sha) in [
        ("TODO-first-fetch".to_owned(), "TODO-first-fetch".to_owned()),
        (f.archive.len().to_string(), "TODO-first-fetch".to_owned()),
        ("TODO-first-fetch".to_owned(), sha256_hex(&f.archive)),
    ] {
        let p = f.tmp.join("corpus.lock.toml");
        std::fs::write(
            &p,
            lock_text(&[LockSpec {
                name: "fixture-smartdoc",
                adapter: "smartdoc2015-ch1",
                spdx: "CC-BY-4.0",
                variant: "full",
                url: &server.url("/a.tar"),
                size,
                sha256: sha,
                extract: "tar",
            }]),
        )
        .unwrap();
        let out = f.run(&p, &["fixture-smartdoc"]);
        let log = text(&out);
        assert!(!out.status.success(), "{log}");
        assert!(
            log.contains("REFUSED")
                && log.contains("TODO-first-fetch")
                && log.contains("placeholder")
                && log.contains("--record-hash"),
            "{log}"
        );
    }
    assert!(
        server.hits().is_empty(),
        "a refused entry must not reach the network"
    );
    assert!(!f.cache.exists(), "and must not create the cache");
}

#[test]
fn the_real_lock_file_refuses_every_dataset_before_any_download() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let tmp = Tmp::new("reallock");
    let cache = tmp.join("cache");
    for name in [
        "smartdoc2015-ch1",
        "cord",
        "dibco",
        "rawpixls-cc0",
        "midv-500",
    ] {
        let out = xtask(&repo, &cache, &["fetch-corpus", name]);
        let log = text(&out);
        assert!(!out.status.success(), "{name}: {log}");
        assert!(log.contains("REFUSED"), "{name}: {log}");
        if name == "midv-500" {
            assert!(log.contains("not cleared"), "{log}");
        } else {
            assert!(log.contains("placeholder"), "{name}: {log}");
        }
    }
    let sample = xtask(
        &repo,
        &cache,
        &["fetch-corpus", "--sample", "smartdoc2015-ch1"],
    );
    assert!(text(&sample).contains("placeholder"), "{}", text(&sample));
    let sample_cord = xtask(&repo, &cache, &["fetch-corpus", "--sample", "cord"]);
    assert!(
        text(&sample_cord).contains("no sample"),
        "{}",
        text(&sample_cord)
    );
    assert!(!cache.exists());
    let list = xtask(&repo, &cache, &["fetch-corpus", "--list"]);
    let log = text(&list);
    assert!(list.status.success(), "{log}");
    assert!(
        log.contains("smartdoc2015-ch1") && log.contains("unpinned"),
        "{log}"
    );
    assert!(
        log.contains("midv-500") && log.contains("licence not cleared"),
        "{log}"
    );
}

#[test]
fn an_uncleared_licence_and_a_non_loopback_http_url_are_refused_without_a_request() {
    let f = Fixture::new("licence");
    let server = Server::start(vec![("/a.tar", Route::Bytes(f.archive.clone()))]);
    let sha = sha256_hex(&f.archive);
    for spdx in ["NOASSERTION", "CC-BY-NC-4.0", "CC-BY-SA-4.0"] {
        let p = f.tmp.join("l.toml");
        std::fs::write(
            &p,
            lock_text(&[LockSpec {
                name: "fixture-smartdoc",
                adapter: "smartdoc2015-ch1",
                spdx,
                variant: "full",
                url: &server.url("/a.tar"),
                size: f.archive.len().to_string(),
                sha256: sha.clone(),
                extract: "tar",
            }]),
        )
        .unwrap();
        let out = f.run(&p, &["fixture-smartdoc"]);
        assert!(!out.status.success());
        assert!(text(&out).contains("not cleared"), "{spdx}: {}", text(&out));
        let rec = f.run(&p, &["--record-hash", "fixture-smartdoc"]);
        assert!(!rec.status.success() && text(&rec).contains("not cleared"));
    }
    let lock = f.lock("http://example.org/a.tar", f.archive.len(), &sha, "full");
    let out = f.run(&lock, &["fixture-smartdoc"]);
    assert!(!out.status.success());
    assert!(text(&out).contains("loopback"), "{}", text(&out));
    assert!(server.hits().is_empty());
}

#[test]
fn record_hash_prints_the_pin_and_keeps_nothing() {
    let f = Fixture::new("record");
    let server = Server::start(vec![("/a.tar", Route::Bytes(f.archive.clone()))]);
    // The lock is fully unpinned, exactly like the real entries.
    let lock = f.lock(&server.url("/a.tar"), 1, "TODO-first-fetch", "full");
    let text_lock = std::fs::read_to_string(&lock)
        .unwrap()
        .replace("size = 1", "size = \"TODO-first-fetch\"");
    std::fs::write(&lock, text_lock).unwrap();
    let out = f.run(&lock, &["--record-hash", "fixture-smartdoc"]);
    let log = text(&out);
    assert!(out.status.success(), "{log}");
    assert!(
        log.contains(&format!("size = {}", f.archive.len())),
        "{log}"
    );
    assert!(
        log.contains(&format!("sha256 = \"{}\"", sha256_hex(&f.archive))),
        "{log}"
    );
    assert!(log.contains("review, then paste"), "{log}");
    assert!(f.downloads().is_empty() && !f.src("full").exists());
    let leftovers: Vec<_> = walk(&f.cache);
    assert!(
        leftovers.is_empty(),
        "quarantine must be empty afterwards: {leftovers:?}"
    );

    // Against a correct pin it says so; against a wrong one it says DIFFERS.
    let pinned = f.lock(
        &server.url("/a.tar"),
        f.archive.len(),
        &sha256_hex(&f.archive),
        "full",
    );
    assert!(
        text(&f.run(&pinned, &["--record-hash", "fixture-smartdoc"])).contains("matches the pin")
    );
    let wrong = f.lock(
        &server.url("/a.tar"),
        f.archive.len(),
        &"0".repeat(64),
        "full",
    );
    assert!(text(&f.run(&wrong, &["--record-hash", "fixture-smartdoc"])).contains("DIFFERS"));
}

fn walk(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.filter_map(Result::ok) {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p.display().to_string());
            }
        }
    }
    out
}

#[test]
fn an_archive_with_a_parent_directory_member_is_refused_before_anything_is_written() {
    let f = Fixture::new("evil");
    let evil = tar(&[
        ("ok.txt", b"fine".to_vec()),
        ("../evil.txt", b"pwned".to_vec()),
    ]);
    let server = Server::start(vec![("/evil.tar", Route::Bytes(evil.clone()))]);
    let lock = f.lock(
        &server.url("/evil.tar"),
        evil.len(),
        &sha256_hex(&evil),
        "full",
    );
    let out = f.run(&lock, &["--no-ingest", "fixture-smartdoc"]);
    let log = text(&out);
    assert!(!out.status.success(), "{log}");
    assert!(log.contains("REFUSED") && log.contains("escape"), "{log}");
    assert!(!f.src("full").exists());
    assert!(
        !f.cache
            .join("fixture-smartdoc")
            .join("full")
            .join("evil.txt")
            .exists()
    );
    assert!(!f.cache.join("fixture-smartdoc").join("evil.txt").exists());
}

#[test]
fn a_zip_archive_extracts_like_a_tar() {
    if !can_unzip() {
        eprintln!("skipped: no unzip on this machine");
        return;
    }
    let f = Fixture::new("zip");
    let archive = zip(&[("a/b.txt", b"hello".to_vec()), ("c.txt", b"x".to_vec())]);
    let server = Server::start(vec![("/a.zip", Route::Bytes(archive.clone()))]);
    let lock = f.lock(
        &server.url("/a.zip"),
        archive.len(),
        &sha256_hex(&archive),
        "full",
    );
    let text_lock = std::fs::read_to_string(&lock)
        .unwrap()
        .replace("extract = \"tar\"", "extract = \"zip\"");
    std::fs::write(&lock, text_lock).unwrap();
    let out = f.run(&lock, &["--no-ingest", "fixture-smartdoc"]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(
        std::fs::read(f.src("full").join("a/b.txt")).unwrap(),
        b"hello"
    );
}

#[test]
fn an_unknown_corpus_and_a_missing_sample_variant_are_reported() {
    let f = Fixture::new("names");
    let lock = f.lock("https://example.org/a.tar", 5, &"1".repeat(64), "full");
    let out = f.run(&lock, &["nope"]);
    assert!(!out.status.success());
    assert!(
        text(&out).contains("unknown corpus `nope`"),
        "{}",
        text(&out)
    );
    let out = f.run(&lock, &["--sample", "fixture-smartdoc"]);
    assert!(!out.status.success());
    assert!(text(&out).contains("no sample"), "{}", text(&out));
    let out = f.run(&lock, &[]);
    assert!(!out.status.success() && text(&out).contains("needs a corpus name"));
}

#[test]
fn a_cache_inside_the_repository_is_refused() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let inside = repo.join("corpus-cache-test-must-not-exist");
    let tmp = Tmp::new("incache");
    let out = xtask(&repo, &inside, &["fetch-corpus", "cord"]);
    let log = text(&out);
    assert!(!out.status.success(), "{log}");
    assert!(log.contains("inside the repository"), "{log}");
    assert!(
        !inside.exists(),
        "nothing may be created inside the checkout"
    );
    // ... and so is explicit `--cache` and an ingest source or output inside it.
    let out = xtask(
        &repo,
        tmp.path(),
        &["fetch-corpus", "--cache", inside.to_str().unwrap(), "cord"],
    );
    assert!(
        text(&out).contains("inside the repository"),
        "{}",
        text(&out)
    );
    let out = xtask(
        &repo,
        tmp.path(),
        &[
            "corpus-ingest",
            "cord",
            "--src",
            repo.join("docs").to_str().unwrap(),
        ],
    );
    assert!(
        text(&out).contains("inside the repository"),
        "{}",
        text(&out)
    );
}

#[test]
fn a_single_file_with_extract_none_is_copied_into_the_tree() {
    // The raw.pixls.us entry pins one index file: no archive to unpack.
    let f = Fixture::new("none");
    let index = pixls_index_lines().join("\n").into_bytes();
    let server = Server::start(vec![("/index.jsonl", Route::Bytes(index.clone()))]);
    let p = f.tmp.join("corpus.lock.toml");
    std::fs::write(
        &p,
        lock_text(&[LockSpec {
            name: "rawpixls-cc0",
            adapter: "rawpixls-cc0",
            spdx: "CC0-1.0",
            variant: "full",
            url: &server.url("/index.jsonl"),
            size: index.len().to_string(),
            sha256: sha256_hex(&index),
            extract: "none",
        }]),
    )
    .unwrap();
    let out = xtask(
        f.tmp.path(),
        &f.cache,
        &[
            "fetch-corpus",
            "--lock",
            p.to_str().unwrap(),
            "rawpixls-cc0",
        ],
    );
    assert!(out.status.success(), "{}", text(&out));
    let src = f.cache.join("rawpixls-cc0/full/src");
    let kept = std::fs::read_to_string(src.join("cc0-samples.jsonl")).unwrap();
    assert_eq!(kept.lines().count(), 3, "{kept}");
}
