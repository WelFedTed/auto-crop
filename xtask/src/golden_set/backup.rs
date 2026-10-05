// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `cargo xtask golden backup <dest>`, `restore-check <dest>` and `restore <dest> --to <dir>`
//! (ROADMAP M1.85, local version).
//!
//! A backup is a plain copy plus a SHA-256 manifest. There is deliberately **no cryptography in
//! this tool**: confidentiality comes from putting the destination on a BitLocker (Windows), FileVault
//! or VeraCrypt volume on a second disk (see docs/testing/golden-workflow.md), and integrity comes from
//! the manifest, which `restore-check` verifies file by file (and against the hashes in
//! `splits.lock.json`, which is the M1.85 acceptance test).
//!
//! Layout of a backup: `images/<name>` (every image a label points to), `labels/<name>` (every
//! file of the labels folder: labels, `_state.json`, the labelling and assist logs), `meta/<name>`
//! (the lock, the evaluation log and its head, the noise floor) and `meta/aggregates/<name>`, plus
//! `backup-manifest.json`. The regenerable `results/` folder is not backed up.

use super::common::{
    Args, DATA_OPTS, Paths, ensure_private, git_commit, sha256_file, utc_now, write_atomic,
};
use super::lock;
use auto_crop_eval::golden;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const MANIFEST: &str = "backup-manifest.json";
pub const BACKUP_SCHEMA: &str = "auto-crop-golden-backup/1";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackupFile {
    /// Forward-slash path relative to the backup root.
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackupManifest {
    pub schema: String,
    pub created: String,
    pub commit: String,
    pub files: Vec<BackupFile>,
}

/// Source path of each file of the backup, keyed by its path inside the backup.
fn collect(p: &Paths) -> Result<(BTreeMap<String, PathBuf>, Vec<String>), String> {
    let mut files = BTreeMap::new();
    let mut problems = Vec::new();
    if p.labels.is_dir() {
        for e in std::fs::read_dir(&p.labels)
            .map_err(|e| format!("cannot read {}: {e}", p.labels.display()))?
            .filter_map(Result::ok)
        {
            let path = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if path.is_file() && !name.ends_with(".tmp") {
                files.insert(format!("labels/{name}"), path);
            }
        }
    }
    for lf in golden::read_label_dir(&p.labels)? {
        match lf.label {
            Ok(l) if golden::is_plain_file_name(&l.image) => {
                let src = p.images.join(&l.image);
                if src.is_file() {
                    files.insert(format!("images/{}", l.image), src);
                } else {
                    problems.push(format!("{}: its image {} is missing", lf.stem, l.image));
                }
            }
            Ok(l) => problems.push(format!("{}: bad image name {:?}", lf.stem, l.image)),
            Err(e) => problems.push(format!("{}: {e}", lf.stem)),
        }
    }
    for (name, path) in [
        ("splits.lock.json", p.lock()),
        ("eval-log.jsonl", p.eval_log()),
        ("eval-log.head", p.eval_head()),
        ("noise-floor.json", p.noise_floor()),
    ] {
        if path.is_file() {
            files.insert(format!("meta/{name}"), path);
        }
    }
    if let Ok(rd) = std::fs::read_dir(p.aggregates()) {
        for e in rd.filter_map(Result::ok) {
            if e.path().is_file() {
                files.insert(
                    format!("meta/aggregates/{}", e.file_name().to_string_lossy()),
                    e.path(),
                );
            }
        }
    }
    Ok((files, problems))
}

/// Where a backup path lives in a data layout rooted at `p`.
fn live_path(p: &Paths, rel: &str) -> Option<PathBuf> {
    if let Some(n) = rel.strip_prefix("images/") {
        Some(p.images.join(n))
    } else if let Some(n) = rel.strip_prefix("labels/") {
        Some(p.labels.join(n))
    } else {
        rel.strip_prefix("meta/").map(|n| p.golden.join(n))
    }
}

fn is_inside(child: &Path, parent: &Path) -> bool {
    match (child.canonicalize(), parent.canonicalize()) {
        (Ok(c), Ok(p)) => c.starts_with(p),
        _ => false,
    }
}

/// Copies `src` to `dst` through a temporary file and verifies the copy by hashing it again.
fn copy_verified(src: &Path, dst: &Path, expected: &str) -> Result<(), String> {
    if let Some(dir) = dst.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let mut tmp = dst.as_os_str().to_owned();
    tmp.push(".part");
    let tmp = PathBuf::from(tmp);
    std::fs::copy(src, &tmp)
        .map_err(|e| format!("cannot copy {} to {}: {e}", src.display(), tmp.display()))?;
    let got = sha256_file(&tmp)?;
    if got != expected {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "{} changed while it was copied (hash mismatch); nothing was written for it",
            src.display()
        ));
    }
    std::fs::rename(&tmp, dst).map_err(|e| format!("cannot place {}: {e}", dst.display()))
}

/// Backs up into `dest`; returns (copied, unchanged, problems).
pub fn backup(p: &Paths, dest: &Path) -> Result<(usize, usize, Vec<String>), String> {
    std::fs::create_dir_all(dest).map_err(|e| format!("cannot create {}: {e}", dest.display()))?;
    for src in [&p.data, &p.images, &p.labels, &p.golden] {
        if src.exists() && (is_inside(dest, src) || is_inside(src, dest)) {
            return Err(format!(
                "the backup folder {} and the data folder {} must be separate (a backup on the same folder is not a backup)",
                dest.display(),
                src.display()
            ));
        }
    }
    ensure_private(dest)?;
    let (files, problems) = collect(p)?;
    let (mut copied, mut same) = (0, 0);
    let mut entries = Vec::new();
    for (rel, src) in &files {
        let sha = sha256_file(src)?;
        let size = std::fs::metadata(src).map_err(|e| e.to_string())?.len();
        let dst = dest.join(rel);
        if dst.is_file() && sha256_file(&dst).is_ok_and(|h| h == sha) {
            same += 1;
        } else {
            copy_verified(src, &dst, &sha)?;
            copied += 1;
        }
        entries.push(BackupFile {
            path: rel.clone(),
            sha256: sha,
            size,
        });
    }
    let m = BackupManifest {
        schema: BACKUP_SCHEMA.to_owned(),
        created: utc_now(),
        commit: git_commit().0,
        files: entries,
    };
    let mut json = serde_json::to_string_pretty(&m).expect("manifest serialises");
    json.push('\n');
    write_atomic(&dest.join(MANIFEST), json.as_bytes())?;
    Ok((copied, same, problems))
}

fn read_manifest(dest: &Path) -> Result<BackupManifest, String> {
    let text = std::fs::read_to_string(dest.join(MANIFEST))
        .map_err(|e| format!("{} has no readable {MANIFEST}: {e}", dest.display()))?;
    let m: BackupManifest = serde_json::from_str(&text).map_err(|e| format!("{MANIFEST}: {e}"))?;
    if m.schema != BACKUP_SCHEMA {
        return Err(format!("{MANIFEST}: unknown schema {:?}", m.schema));
    }
    Ok(m)
}

/// Result of verifying a backup.
#[derive(Debug, Default)]
pub struct Verification {
    pub errors: Vec<String>,
    pub notes: Vec<String>,
    pub files: usize,
    pub lock_entries_checked: usize,
}

/// Verifies every file of the backup against its manifest and the lock inside it, and compares
/// with the live data when `live` is given.
pub fn restore_check(dest: &Path, live: Option<&Paths>) -> Result<Verification, String> {
    let m = read_manifest(dest)?;
    let mut v = Verification {
        files: m.files.len(),
        ..Verification::default()
    };
    let mut known = std::collections::BTreeSet::new();
    for f in &m.files {
        known.insert(f.path.clone());
        let path = dest.join(&f.path);
        match sha256_file(&path) {
            Ok(h) if h == f.sha256 => {}
            Ok(_) => v
                .errors
                .push(format!("{}: contents differ from the manifest", f.path)),
            Err(_) => v
                .errors
                .push(format!("{}: missing from the backup", f.path)),
        }
    }
    // Files in the backup folder that the manifest does not know.
    let mut stack = vec![dest.to_owned()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d)
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
        {
            let path = e.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(rel) = path.strip_prefix(dest) {
                let rel = rel.to_string_lossy().replace('\\', "/");
                if rel != MANIFEST && !known.contains(&rel) {
                    v.notes.push(format!(
                        "{rel}: in the backup folder but not in the manifest"
                    ));
                }
            }
        }
    }
    // M1.85 acceptance: the lock's hashes match the backed-up files.
    let by_path: BTreeMap<&str, &BackupFile> =
        m.files.iter().map(|f| (f.path.as_str(), f)).collect();
    if let Some(lf) = by_path.get("meta/splits.lock.json") {
        let text = std::fs::read_to_string(dest.join(&lf.path))
            .map_err(|e| format!("cannot read the backed-up lock: {e}"))?;
        let lock: lock::Lock =
            serde_json::from_str(&text).map_err(|e| format!("backed-up lock: {e}"))?;
        for e in &lock.entries {
            v.lock_entries_checked += 1;
            let img = format!("images/{}", e.image);
            let lab = format!("labels/{}.json", e.id);
            let check = |key: &str, want: &str, what: &str, errors: &mut Vec<String>| match by_path
                .get(key)
            {
                None => errors.push(format!("lock entry {}: {what} is not in the backup", e.id)),
                Some(f) if f.sha256 != want => errors.push(format!(
                    "lock entry {}: the backed-up {what} does not match the lock's SHA-256",
                    e.id
                )),
                Some(_) => {}
            };
            check(&img, &e.image_sha256, "image", &mut v.errors);
            check(&lab, &e.label_sha256, "label", &mut v.errors);
        }
    } else {
        v.notes
            .push("the backup has no splits.lock.json; only the manifest was verified".to_owned());
    }
    if let Some(p) = live {
        let (mut missing, mut differ) = (0, 0);
        for f in &m.files {
            match live_path(p, &f.path).map(|x| sha256_file(&x)) {
                Some(Ok(h)) if h == f.sha256 => {}
                Some(Ok(_)) => differ += 1,
                _ => missing += 1,
            }
        }
        if missing + differ > 0 {
            v.notes.push(format!(
                "live data differs from this backup: {differ} file(s) changed, {missing} missing or unreadable (back up again if the live data is the newer one)"
            ));
        } else {
            v.notes
                .push("live data is identical to this backup".to_owned());
        }
    }
    Ok(v)
}

/// Restores into an empty `to` folder laid out as a data folder, verifying each file; then checks
/// the lock against the restored files.
pub fn restore(dest: &Path, to: &Path) -> Result<usize, String> {
    let m = read_manifest(dest)?;
    if to.exists()
        && std::fs::read_dir(to)
            .map_err(|e| e.to_string())?
            .next()
            .is_some()
    {
        return Err(format!(
            "{} is not empty; restore into a fresh folder",
            to.display()
        ));
    }
    ensure_private(to)?;
    let golden_dir = to.join("golden");
    let target = Paths {
        data: to.to_owned(),
        images: to.to_owned(),
        labels: golden_dir.join("labels"),
        golden: golden_dir,
    };
    for f in &m.files {
        let src = dest.join(&f.path);
        let got = sha256_file(&src)?;
        if got != f.sha256 {
            return Err(format!(
                "{}: the backup copy is corrupt (hash differs from the manifest)",
                f.path
            ));
        }
        let dst =
            live_path(&target, &f.path).ok_or_else(|| format!("unexpected path {}", f.path))?;
        copy_verified(&src, &dst, &f.sha256)?;
    }
    if let Some(l) = lock::read_lock(&target.lock())? {
        let verdict = lock::verify(&target, &l, true)?;
        if !verdict.errors.is_empty() {
            return Err(format!(
                "restored files do not match the lock: {}",
                verdict.errors.join("; ")
            ));
        }
    }
    Ok(m.files.len())
}

/// `cargo xtask golden backup <dest>`.
pub fn run_backup(args: &[String]) -> Result<(), String> {
    let a = Args::new(args);
    a.reject_unknown(&[], &DATA_OPTS)?;
    let pos = a.positionals(&DATA_OPTS);
    let [dest] = pos.as_slice() else {
        return Err("usage: golden backup <dest dir> [--data DIR]".to_owned());
    };
    let p = Paths::from_args(&a)?;
    let (copied, same, problems) = backup(&p, Path::new(dest))?;
    for pr in &problems {
        println!("PROBLEM {pr}");
    }
    println!(
        "backup: {copied} file(s) copied, {same} already up to date, manifest written to {dest}/{MANIFEST}"
    );
    println!(
        "this is a plain copy with SHA-256 hashes, not encryption: keep {dest} on a BitLocker, FileVault or VeraCrypt volume on a second disk (docs/testing/golden-workflow.md)"
    );
    if problems.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} problem(s): the backup is incomplete",
            problems.len()
        ))
    }
}

/// `cargo xtask golden restore-check <dest> [--require-current]`.
pub fn run_restore_check(args: &[String]) -> Result<(), String> {
    let a = Args::new(args);
    a.reject_unknown(&["--require-current", "--no-live"], &DATA_OPTS)?;
    let pos = a.positionals(&DATA_OPTS);
    let [dest] = pos.as_slice() else {
        return Err(
            "usage: golden restore-check <backup dir> [--require-current] [--no-live]".to_owned(),
        );
    };
    let p = Paths::from_args(&a)?;
    let live = (!a.flag("--no-live") && p.labels.is_dir()).then_some(&p);
    let v = restore_check(Path::new(dest), live)?;
    for e in &v.errors {
        println!("ERROR   {e}");
    }
    for n in &v.notes {
        println!("note    {n}");
    }
    println!(
        "restore-check: {} file(s) verified, {} lock entr(ies) matched against the backed-up hashes, {} error(s)",
        v.files,
        v.lock_entries_checked,
        v.errors.len()
    );
    let stale =
        a.flag("--require-current") && v.notes.iter().any(|n| n.starts_with("live data differs"));
    if !v.errors.is_empty() {
        Err(format!("{} error(s)", v.errors.len()))
    } else if stale {
        Err("the live data differs from the backup (--require-current)".to_owned())
    } else {
        Ok(())
    }
}

/// `cargo xtask golden restore <backup dir> --to <empty dir>`.
pub fn run_restore(args: &[String]) -> Result<(), String> {
    let a = Args::new(args);
    a.reject_unknown(&[], &["--to"])?;
    let pos = a.positionals(&["--to"]);
    let [dest] = pos.as_slice() else {
        return Err("usage: golden restore <backup dir> --to <empty dir>".to_owned());
    };
    let to = a.value("--to")?.ok_or("restore needs --to <empty dir>")?;
    let n = restore(Path::new(dest), Path::new(&to))?;
    println!(
        "restored {n} file(s) into {to}; every hash matched the manifest and the lock. Use it with --data {to}"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::golden_set::lock::tests::synthetic_set;

    fn lock_args(p: &Paths) -> Vec<String> {
        [
            "--data",
            &p.data.display().to_string(),
            "--images",
            &p.images.display().to_string(),
            "--labels",
            &p.labels.display().to_string(),
        ]
        .map(str::to_owned)
        .to_vec()
    }

    #[test]
    fn backup_restore_check_and_restore_round_trip_and_catch_corruption() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = dir.path().join("live");
        let p = synthetic_set(&src, 24);
        lock::run_lock(&lock_args(&p)).expect("locks");
        let dest = dir.path().join("backup");
        let (copied, same, problems) = backup(&p, &dest).expect("backs up");
        assert!(problems.is_empty(), "{problems:?}");
        assert!(copied > 24 * 2, "{copied}");
        assert_eq!(same, 0);
        // A second backup copies nothing.
        let (copied2, same2, _) = backup(&p, &dest).expect("backs up again");
        assert_eq!((copied2, same2), (0, copied));
        let v = restore_check(&dest, Some(&p)).expect("checks");
        assert!(v.errors.is_empty(), "{:?}", v.errors);
        assert_eq!(v.lock_entries_checked, 24);
        assert!(
            v.notes.iter().any(|n| n.contains("identical")),
            "{:?}",
            v.notes
        );
        // The CLI wrapper agrees.
        let mut a = lock_args(&p);
        a.insert(0, dest.display().to_string());
        run_restore_check(&a).expect("cli ok");
        // Restore into a fresh folder, then the restored copy passes the golden check.
        let back = dir.path().join("restored");
        assert_eq!(restore(&dest, &back).expect("restores"), copied);
        let rp = Paths {
            data: back.clone(),
            images: back.clone(),
            labels: back.join("golden").join("labels"),
            golden: back.join("golden"),
        };
        assert_eq!(lock::check_all(&rp, true).expect("check"), 0);
        // A non-empty target is refused.
        assert!(restore(&dest, &back).is_err());
        // Live data changed after the backup: a note, and --require-current turns it into an error.
        std::fs::write(p.images.join("img000.png"), b"changed").expect("write");
        let v = restore_check(&dest, Some(&p)).expect("checks");
        assert!(v.errors.is_empty());
        assert!(
            v.notes.iter().any(|n| n.contains("differs")),
            "{:?}",
            v.notes
        );
        let mut stale = a.clone();
        stale.push("--require-current".to_owned());
        assert!(run_restore_check(&stale).is_err());
        // Corrupting a backed-up image is an error, and restore refuses it.
        std::fs::write(dest.join("images").join("img003.png"), b"bitrot").expect("write");
        let v = restore_check(&dest, None).expect("checks");
        assert!(
            v.errors.iter().any(|e| e.contains("img003.png")),
            "{:?}",
            v.errors
        );
        assert!(restore(&dest, &dir.path().join("again")).is_err());
        // A deleted backup file is an error too.
        std::fs::remove_file(dest.join("labels").join("img004.png.json")).expect("remove");
        let v = restore_check(&dest, None).expect("checks");
        assert!(
            v.errors
                .iter()
                .any(|e| e.contains("missing from the backup"))
        );
        // Extra files are reported, not fatal.
        std::fs::write(dest.join("stray.txt"), b"x").expect("write");
        let v = restore_check(&dest, None).expect("checks");
        assert!(v.notes.iter().any(|n| n.contains("stray.txt")));
    }

    #[test]
    fn a_backup_inside_the_data_folder_is_refused_and_a_missing_image_is_reported() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = synthetic_set(&dir.path().join("live"), 3);
        let inside = p.data.join("backup-here");
        assert!(backup(&p, &inside).is_err());
        let outside = dir.path().join("elsewhere");
        std::fs::remove_file(p.images.join("img001.png")).expect("remove");
        let (_, _, problems) = backup(&p, &outside).expect("runs");
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("img001.png"));
    }
}
