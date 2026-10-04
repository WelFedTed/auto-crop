// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `cargo xtask golden lock` and `golden check` (ROADMAP M1.44, local version).
//!
//! `splits.lock.json` freezes, per image, the split (`dev` or `locked`), the scene, and the
//! SHA-256 of the image and of its label file. Locking is incremental: images already in the lock
//! keep their split for good (a locked image or label that changed is an error, never silently
//! re-locked); new images join, with a scene that is already in the lock inheriting its split so a
//! scene can never straddle both. New scenes are assigned deterministically, in the order of
//! `SHA-256("autocrop-golden-split-v1:" + scene)`, to `dev` while the dev share is below the
//! PROVISIONAL 30% and to `locked` otherwise.
//!
//! Without scene metadata a scene is the file itself; give near-duplicates (several photos of one
//! receipt) a shared `scene_id` in the label before locking.

use super::common::{
    Args, DATA_OPTS, Paths, ensure_private, git_commit, sha256_bytes, sha256_file, utc_now,
    write_atomic,
};
use super::log;
use auto_crop_eval::golden::{self, CheckOptions, GoldenLabel};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const LOCK_SCHEMA: &str = "auto-crop-golden-lock/1";
/// PROVISIONAL share of images in the dev split (decision log A-4 style: replace with a measured
/// choice once slices are known).
pub const DEV_PERCENT: usize = 30;
const SPLIT_SALT: &str = "autocrop-golden-split-v1:";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LockEntry {
    pub id: String,
    pub image: String,
    pub scene_id: String,
    /// `dev` or `locked`.
    pub split: String,
    pub image_sha256: String,
    pub label_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Withdrawn {
    pub id: String,
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lock {
    pub schema: String,
    /// Increases every time the entries change.
    pub version: u64,
    pub dev_percent: usize,
    pub created: String,
    pub updated: String,
    pub commit: String,
    pub entries: Vec<LockEntry>,
    /// Images removed on request (B21 withdrawal): their entries are gone, the fact is kept.
    #[serde(default)]
    pub withdrawn: Vec<Withdrawn>,
}

pub fn read_lock(path: &std::path::Path) -> Result<Option<Lock>, String> {
    match std::fs::read_to_string(path) {
        Ok(t) => serde_json::from_str(&t)
            .map(Some)
            .map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

fn scene_key(scene: &str) -> String {
    sha256_bytes(format!("{SPLIT_SALT}{scene}").as_bytes())
}

/// Splits for the `new` `(id, scene)` pairs given the `existing` `(scene, split)` assignments.
/// A scene already assigned keeps its split; new scenes go in hash order to `dev` while
/// `dev * 100 < percent * (images so far + this scene's images)`. Pure and deterministic.
pub fn assign_splits(
    existing: &[(String, String)],
    new: &[(String, String)],
    percent: usize,
) -> BTreeMap<String, String> {
    let mut scene_split: BTreeMap<String, String> = BTreeMap::new();
    let (mut dev, mut total) = (0usize, existing.len());
    for (scene, split) in existing {
        scene_split.insert(scene.clone(), split.clone());
        if split == "dev" {
            dev += 1;
        }
    }
    let mut out = BTreeMap::new();
    let mut fresh: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (id, scene) in new {
        match scene_split.get(scene) {
            Some(s) => {
                out.insert(id.clone(), s.clone());
                total += 1;
                if s == "dev" {
                    dev += 1;
                }
            }
            None => fresh.entry(scene.as_str()).or_default().push(id.as_str()),
        }
    }
    let mut order: Vec<(&str, &Vec<&str>)> = fresh.iter().map(|(s, v)| (*s, v)).collect();
    order.sort_by_key(|(s, _)| scene_key(s));
    for (scene, ids) in order {
        let split = if dev * 100 < percent * (total + ids.len()) {
            "dev"
        } else {
            "locked"
        };
        for id in ids {
            out.insert((*id).to_owned(), split.to_owned());
        }
        total += ids.len();
        if split == "dev" {
            dev += ids.len();
        }
        scene_split.insert(scene.to_owned(), split.to_owned());
    }
    out
}

/// Scenes that appear in both splits of `entries`.
pub fn scene_violations(entries: &[LockEntry]) -> Vec<String> {
    let mut by: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for e in entries {
        by.entry(e.scene_id.as_str())
            .or_default()
            .insert(e.split.as_str());
    }
    by.into_iter()
        .filter(|(_, s)| s.len() > 1)
        .map(|(scene, _)| format!("scene {scene:?} is in both the dev and the locked split"))
        .collect()
}

struct Current {
    label: GoldenLabel,
    label_sha: String,
}

fn load_current(p: &Paths) -> Result<BTreeMap<String, Current>, String> {
    let mut out = BTreeMap::new();
    for lf in golden::read_label_dir(&p.labels)? {
        let Ok(label) = lf.label else { continue };
        let label_sha = sha256_file(&lf.path)?;
        out.insert(label.id.clone(), Current { label, label_sha });
    }
    Ok(out)
}

fn to_json(l: &Lock) -> String {
    let mut s = serde_json::to_string_pretty(l).expect("lock serialises");
    s.push('\n');
    s
}

/// Everything wrong with the lock compared with the files on disk.
#[derive(Debug, Default)]
pub struct Verdict {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

/// Checks the lock against the files: a locked image or label that was edited or deleted, an
/// assisted locked label and a scene in both splits are errors; changed dev entries and
/// unlocked new labels are warnings.
pub fn verify(p: &Paths, lock: &Lock, hash_images: bool) -> Result<Verdict, String> {
    let mut v = Verdict::default();
    if lock.schema != LOCK_SCHEMA {
        v.errors
            .push(format!("unknown lock schema {:?}", lock.schema));
    }
    let current = load_current(p)?;
    let withdrawn: BTreeSet<&str> = lock.withdrawn.iter().map(|w| w.id.as_str()).collect();
    for e in &lock.entries {
        let locked = e.split == "locked";
        let mut problem = |what: String| {
            if locked {
                v.errors.push(format!("locked image {}: {what}", e.id));
            } else {
                v.warnings.push(format!(
                    "dev image {}: {what} (run `cargo xtask golden lock` to refresh the dev entry)",
                    e.id
                ));
            }
        };
        match current.get(&e.id) {
            None => problem("its label file is missing".to_owned()),
            Some(c) => {
                if c.label_sha != e.label_sha256 {
                    problem("its label was edited after locking".to_owned());
                }
                if locked && c.label.assisted {
                    problem("its label is marked assisted".to_owned());
                }
            }
        }
        if hash_images {
            let img = p.images.join(&e.image);
            match sha256_file(&img) {
                Ok(h) if h != e.image_sha256 => {
                    problem("the image was edited after locking".to_owned())
                }
                Ok(_) => {}
                Err(_) => problem("the image file is missing".to_owned()),
            }
        }
    }
    v.errors.extend(scene_violations(&lock.entries));
    let in_lock: BTreeSet<&str> = lock.entries.iter().map(|e| e.id.as_str()).collect();
    let unlocked = current
        .values()
        .filter(|c| {
            !c.label.assisted
                && !in_lock.contains(c.label.id.as_str())
                && !withdrawn.contains(c.label.id.as_str())
        })
        .count();
    if unlocked > 0 {
        v.warnings.push(format!(
            "{unlocked} label(s) are not in the lock yet (run `cargo xtask golden lock`)"
        ));
    }
    Ok(v)
}

/// The full golden check: label validity, the lock, and the evaluation log. Returns the number of
/// errors found (printing everything).
pub fn check_all(p: &Paths, hash_images: bool) -> Result<usize, String> {
    let rep = golden::check_dir(
        &p.labels,
        &p.images,
        CheckOptions {
            no_hash: !hash_images,
        },
    )?;
    let mut errors = rep.errors.len();
    for e in &rep.errors {
        println!("ERROR   {e}");
    }
    for w in &rep.warnings {
        println!("warning {w}");
    }
    match read_lock(&p.lock())? {
        None => {
            errors += 1;
            println!("ERROR   no splits.lock.json yet: run `cargo xtask golden lock`");
        }
        Some(lock) => {
            let v = verify(p, &lock, hash_images)?;
            errors += v.errors.len();
            for e in &v.errors {
                println!("ERROR   {e}");
            }
            for w in &v.warnings {
                println!("warning {w}");
            }
            let dev = lock.entries.iter().filter(|e| e.split == "dev").count();
            println!(
                "lock: version {}, {} dev + {} locked image(s), {} withdrawn",
                lock.version,
                dev,
                lock.entries.len() - dev,
                lock.withdrawn.len()
            );
        }
    }
    match log::verify(p) {
        Ok(s) => println!(
            "eval log: intact, {} evaluation(s), {} of the locked set",
            s.entries.len(),
            s.locked_evaluations()
        ),
        Err(e) => {
            errors += 1;
            println!("ERROR   eval log: {e}");
        }
    }
    Ok(errors)
}

/// `cargo xtask golden check [--no-hash]`.
pub fn run_check(args: &[String]) -> Result<(), String> {
    let a = Args::new(args);
    a.reject_unknown(&["--no-hash"], &DATA_OPTS)?;
    let p = Paths::from_args(&a)?;
    let errors = check_all(&p, !a.flag("--no-hash"))?;
    if errors == 0 {
        println!("golden check: ok");
        Ok(())
    } else {
        Err(format!("golden check failed with {errors} error(s)"))
    }
}

/// `cargo xtask golden lock [--withdraw ID]`.
pub fn run_lock(args: &[String]) -> Result<(), String> {
    let a = Args::new(args);
    let opts = ["--data", "--images", "--labels", "--withdraw"];
    a.reject_unknown(&[], &opts)?;
    let p = Paths::from_args(&a)?;
    ensure_private(&p.golden)?;
    ensure_private(&p.labels)?;
    let existing = read_lock(&p.lock())?;
    if let Some(id) = a.value("--withdraw")? {
        return withdraw(&p, existing, &id);
    }
    let rep = golden::check_dir(&p.labels, &p.images, CheckOptions::default())?;
    if !rep.errors.is_empty() {
        for e in &rep.errors {
            println!("ERROR   {e}");
        }
        return Err("fix the label errors first (cargo xtask check-labels)".to_owned());
    }
    let current = load_current(&p)?;
    let now = utc_now();
    let (commit, _) = git_commit();
    let mut lock = existing.clone().unwrap_or_else(|| Lock {
        schema: LOCK_SCHEMA.to_owned(),
        version: 0,
        dev_percent: DEV_PERCENT,
        created: now.clone(),
        updated: now.clone(),
        commit: commit.clone(),
        entries: Vec::new(),
        withdrawn: Vec::new(),
    });
    let mut errors = Vec::new();
    let mut notes = Vec::new();
    let mut kept = Vec::new();
    for e in &lock.entries {
        let locked = e.split == "locked";
        let Some(c) = current.get(&e.id) else {
            if locked {
                errors.push(format!("locked image {}: its label file is missing", e.id));
            } else {
                notes.push(format!(
                    "dev image {} dropped: its label file is gone",
                    e.id
                ));
            }
            continue;
        };
        let image_sha = sha256_file(&p.images.join(&e.image)).unwrap_or_default();
        if locked {
            if c.label_sha != e.label_sha256 {
                errors.push(format!("locked image {}: its label was edited", e.id));
            }
            if image_sha != e.image_sha256 {
                errors.push(format!(
                    "locked image {}: the image was edited or is missing",
                    e.id
                ));
            }
            kept.push(e.clone());
        } else if c.label.assisted {
            notes.push(format!(
                "dev image {} dropped: its label is now marked assisted",
                e.id
            ));
        } else {
            if c.label_sha != e.label_sha256 || image_sha != e.image_sha256 {
                notes.push(format!(
                    "dev image {} refreshed (label or image changed)",
                    e.id
                ));
            }
            kept.push(LockEntry {
                id: e.id.clone(),
                image: c.label.image.clone(),
                scene_id: c.label.scene_id.clone(),
                split: "dev".to_owned(),
                image_sha256: image_sha,
                label_sha256: c.label_sha.clone(),
            });
        }
    }
    if !errors.is_empty() {
        for e in &errors {
            println!("ERROR   {e}");
        }
        return Err(
            "a locked image or label changed; restore it from the backup (cargo xtask golden restore-check) or withdraw it (--withdraw ID); the lock is never silently rewritten"
                .to_owned(),
        );
    }
    let withdrawn: BTreeSet<String> = lock.withdrawn.iter().map(|w| w.id.clone()).collect();
    let in_lock: BTreeSet<String> = kept.iter().map(|e| e.id.clone()).collect();
    let mut new: Vec<(String, String)> = Vec::new();
    let mut assisted = 0;
    for (id, c) in &current {
        if in_lock.contains(id) || withdrawn.contains(id) {
            continue;
        }
        if c.label.assisted {
            assisted += 1;
            continue;
        }
        new.push((id.clone(), c.label.scene_id.clone()));
    }
    let existing_scenes: Vec<(String, String)> = kept
        .iter()
        .map(|e| (e.scene_id.clone(), e.split.clone()))
        .collect();
    let assigned = assign_splits(&existing_scenes, &new, lock.dev_percent);
    for (id, split) in &assigned {
        let c = &current[id];
        kept.push(LockEntry {
            id: id.clone(),
            image: c.label.image.clone(),
            scene_id: c.label.scene_id.clone(),
            split: split.clone(),
            image_sha256: c.label.image_sha256.clone(),
            label_sha256: c.label_sha.clone(),
        });
    }
    kept.sort_by(|a, b| a.id.cmp(&b.id));
    let violations = scene_violations(&kept);
    if !violations.is_empty() {
        for v in &violations {
            println!("ERROR   {v}");
        }
        return Err(
            "a dev image now shares its scene_id with a locked one; give it its old scene_id back"
                .to_owned(),
        );
    }
    let changed = kept != lock.entries;
    lock.entries = kept;
    if changed {
        lock.version += 1;
        lock.updated = now;
        lock.commit = commit;
        write_atomic(&p.lock(), to_json(&lock).as_bytes())?;
    }
    for n in &notes {
        println!("note    {n}");
    }
    let dev = lock.entries.iter().filter(|e| e.split == "dev").count();
    let scenes: BTreeSet<&str> = lock.entries.iter().map(|e| e.scene_id.as_str()).collect();
    println!(
        "lock {}: version {}, {} image(s) in {} scene(s): {} dev + {} locked ({} new){}",
        if changed { "written" } else { "unchanged" },
        lock.version,
        lock.entries.len(),
        scenes.len(),
        dev,
        lock.entries.len() - dev,
        assigned.len(),
        if assisted > 0 {
            format!("; {assisted} assisted label(s) left out")
        } else {
            String::new()
        }
    );
    println!(
        "the locked split ({} image(s)) is for confirming results once per release candidate; do not tune on it",
        lock.entries.len() - dev
    );
    Ok(())
}

fn withdraw(p: &Paths, existing: Option<Lock>, id: &str) -> Result<(), String> {
    let mut lock = existing.ok_or("no lock to withdraw from")?;
    let before = lock.entries.len();
    lock.entries.retain(|e| e.id != id);
    if lock.entries.len() == before {
        return Err(format!("{id} is not in the lock"));
    }
    lock.withdrawn.push(Withdrawn {
        id: id.to_owned(),
        at: utc_now(),
    });
    lock.version += 1;
    lock.updated = utc_now();
    write_atomic(&p.lock(), to_json(&lock).as_bytes())?;
    println!(
        "withdrew {id} from the lock (version {}); now delete its image and label, and every copy in your backups",
        lock.version
    );
    Ok(())
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::golden::log::tests::paths;
    use auto_crop_eval::golden::{GoldenItem, label_to_json, new_label};

    pub const GOOD: [[f64; 2]; 4] = [[0.1, 0.1], [0.9, 0.1], [0.9, 0.9], [0.1, 0.9]];

    /// Writes `n` synthetic images and labels (never real data) and returns the layout.
    pub fn synthetic_set(dir: &std::path::Path, n: usize) -> Paths {
        let mut p = paths(dir);
        p.images = dir.join("images");
        std::fs::create_dir_all(&p.images).expect("mkdir");
        std::fs::create_dir_all(&p.labels).expect("mkdir");
        for i in 0..n {
            add_label(&p, &format!("img{i:03}.png"), None);
        }
        p
    }

    pub fn add_label(p: &Paths, name: &str, scene: Option<&str>) {
        let bytes = format!("synthetic image bytes {name}");
        std::fs::write(p.images.join(name), &bytes).expect("write");
        let mut l = new_label(name, &sha256_bytes(bytes.as_bytes()), 100, 80);
        l.slices = vec!["flatbed-single".to_owned()];
        l.items = vec![GoldenItem::new(GOOD)];
        if let Some(s) = scene {
            l.scene_id = s.to_owned();
        }
        std::fs::write(p.labels.join(format!("{name}.json")), label_to_json(&l)).expect("write");
    }

    fn args(p: &Paths) -> Vec<String> {
        vec![
            "--data".to_owned(),
            p.data.display().to_string(),
            "--images".to_owned(),
            p.images.display().to_string(),
            "--labels".to_owned(),
            p.labels.display().to_string(),
        ]
    }

    #[test]
    fn the_split_is_deterministic_close_to_30_percent_and_scene_disjoint() {
        let new: Vec<(String, String)> = (0..100)
            .map(|i| (format!("i{i}"), format!("scene{}", i / 2)))
            .collect();
        let a = assign_splits(&[], &new, 30);
        let b = assign_splits(&[], &new, 30);
        assert_eq!(a, b);
        let dev = a.values().filter(|s| *s == "dev").count();
        assert!((28..=32).contains(&dev), "dev share {dev}");
        for i in 0..50 {
            assert_eq!(a[&format!("i{}", 2 * i)], a[&format!("i{}", 2 * i + 1)]);
        }
        // Later additions never move existing images and follow their scene.
        let existing: Vec<(String, String)> = new
            .iter()
            .map(|(id, scene)| (scene.clone(), a[id].clone()))
            .collect();
        let more = vec![
            ("late1".to_owned(), "scene3".to_owned()),
            ("late2".to_owned(), "fresh".to_owned()),
        ];
        let m = assign_splits(&existing, &more, 30);
        assert_eq!(m["late1"], a["i6"]);
        assert!(m.contains_key("late2"));
    }

    #[test]
    fn lock_check_and_every_kind_of_tampering() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = synthetic_set(dir.path(), 20);
        add_label(&p, "pair-a.png", Some("pair"));
        add_label(&p, "pair-b.png", Some("pair"));
        // Nothing to check before locking.
        assert!(run_check(&args(&p)).is_err());
        run_lock(&args(&p)).expect("locks");
        run_check(&args(&p)).expect("checks");
        let lock = read_lock(&p.lock()).expect("reads").expect("exists");
        assert_eq!(lock.entries.len(), 22);
        assert!(scene_violations(&lock.entries).is_empty());
        let split = |id: &str| {
            lock.entries
                .iter()
                .find(|e| e.id == id)
                .expect("entry")
                .split
                .clone()
        };
        assert_eq!(split("pair-a.png"), split("pair-b.png"));
        // Locking again changes nothing.
        let before = std::fs::read(p.lock()).expect("read");
        run_lock(&args(&p)).expect("idempotent");
        assert_eq!(before, std::fs::read(p.lock()).expect("read"));
        let locked_id = lock
            .entries
            .iter()
            .find(|e| e.split == "locked")
            .expect("a locked image")
            .id
            .clone();
        let dev_id = lock
            .entries
            .iter()
            .find(|e| e.split == "dev")
            .expect("a dev image")
            .id
            .clone();
        // Editing a locked label turns the check red and the lock refuses to move.
        let label_path = p.labels.join(format!("{locked_id}.json"));
        let original = std::fs::read_to_string(&label_path).expect("read");
        std::fs::write(&label_path, original.replace("0.9", "0.8")).expect("write");
        assert!(run_check(&args(&p)).is_err());
        assert!(run_lock(&args(&p)).is_err());
        assert_eq!(
            before,
            std::fs::read(p.lock()).expect("read"),
            "lock must not be rewritten"
        );
        std::fs::write(&label_path, &original).expect("restore");
        run_check(&args(&p)).expect("green again");
        // Editing a locked image is caught too.
        let image_path = p.images.join(&locked_id);
        let img = std::fs::read(&image_path).expect("read");
        std::fs::write(&image_path, b"tampered").expect("write");
        let e = run_check(&args(&p)).expect_err("red");
        assert!(e.contains("error"), "{e}");
        std::fs::write(&image_path, img).expect("restore");
        // A dev edit is only a warning; locking refreshes the dev entry.
        let dev_label = p.labels.join(format!("{dev_id}.json"));
        let t = std::fs::read_to_string(&dev_label).expect("read");
        std::fs::write(&dev_label, t.replace("0.9", "0.85")).expect("write");
        run_check(&args(&p)).expect("dev edits do not fail the check");
        run_lock(&args(&p)).expect("refreshes");
        assert_eq!(read_lock(&p.lock()).expect("r").expect("l").version, 2);
        // Scenes in both splits are caught even if someone edits the lock by hand.
        let mut forged = read_lock(&p.lock()).expect("r").expect("l");
        let (a, b) = (
            forged
                .entries
                .iter()
                .position(|e| e.split == "dev")
                .expect("dev"),
            forged
                .entries
                .iter()
                .position(|e| e.split == "locked")
                .expect("locked"),
        );
        forged.entries[a].scene_id = "shared".to_owned();
        forged.entries[b].scene_id = "shared".to_owned();
        assert!(!scene_violations(&forged.entries).is_empty());
        std::fs::write(p.lock(), to_json(&forged)).expect("write");
        assert!(run_check(&args(&p)).is_err());
    }

    #[test]
    fn new_images_join_and_old_assignments_never_move() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = synthetic_set(dir.path(), 30);
        run_lock(&args(&p)).expect("locks");
        let first = read_lock(&p.lock()).expect("r").expect("l");
        for i in 0..10 {
            add_label(&p, &format!("later{i}.png"), None);
        }
        run_lock(&args(&p)).expect("extends");
        let second = read_lock(&p.lock()).expect("r").expect("l");
        assert_eq!(second.entries.len(), 40);
        assert_eq!(second.version, 2);
        for e in &first.entries {
            let now = second.entries.iter().find(|x| x.id == e.id).expect("kept");
            assert_eq!(now, e, "an existing entry moved");
        }
    }

    #[test]
    fn assisted_labels_are_never_locked_and_withdrawal_is_recorded() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = synthetic_set(dir.path(), 5);
        add_label(&p, "peeked.png", None);
        let path = p.labels.join("peeked.png.json");
        let t = std::fs::read_to_string(&path).expect("read");
        std::fs::write(
            &path,
            t.replace("\"assisted\": false", "\"assisted\": true"),
        )
        .expect("w");
        run_lock(&args(&p)).expect("locks");
        let lock = read_lock(&p.lock()).expect("r").expect("l");
        assert_eq!(lock.entries.len(), 5);
        assert!(lock.entries.iter().all(|e| e.id != "peeked.png"));
        let victim = lock.entries[0].id.clone();
        let mut a = args(&p);
        a.extend(["--withdraw".to_owned(), victim.clone()]);
        run_lock(&a).expect("withdraws");
        let lock = read_lock(&p.lock()).expect("r").expect("l");
        assert_eq!(lock.entries.len(), 4);
        assert_eq!(lock.withdrawn[0].id, victim);
        // The withdrawn label is not re-added by a later lock.
        run_lock(&args(&p)).expect("locks");
        assert_eq!(
            read_lock(&p.lock()).expect("r").expect("l").entries.len(),
            4
        );
        assert!(
            run_lock(&{
                let mut b = args(&p);
                b.extend(["--withdraw".to_owned(), "nope.png".to_owned()]);
                b
            })
            .is_err()
        );
    }

    #[test]
    fn a_label_error_blocks_locking() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = synthetic_set(dir.path(), 3);
        let path = p.labels.join("img001.png.json");
        let t = std::fs::read_to_string(&path).expect("read");
        std::fs::write(&path, t.replace("\"flatbed-single\"", "\"bogus\"")).expect("w");
        assert!(run_lock(&args(&p)).is_err());
        assert!(read_lock(&p.lock()).expect("r").is_none());
    }
}
