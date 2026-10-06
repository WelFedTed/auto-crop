// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `auto-crop restore`: put originals back from the backup store (ROADMAP M2.46). The restore
//! itself is the engine's (`restore_file_derived`: verified against the recorded hash, temp file,
//! atomic replace, reversible: what was there is kept in the backup entry). This module finds
//! the backup for what the user named (a file path, one of a split scan's output paths, a backup
//! id, or a whole run), applies `--if-modified`, and reports. `--dry-run` reads the store and
//! the files and says what would happen; it writes nothing.

use crate::args::{IfModified, RestoreArgs};
use crate::env::{Env, base_settings};
use crate::exit;
use crate::manifest::code;
use crate::pipeline::err_code;
use auto_crop_core::ErrKind;
use auto_crop_engine::store::{BackupKind, BackupState, Manifest, Store};
use auto_crop_engine::util::blake3_hex;
use auto_crop_engine::{DerivedAction, Engine, Housekeeping, RestoreMode};
use serde::Serialize;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RestoreItem {
    /// What the user named.
    pub target: String,
    pub backup_id: Option<String>,
    /// `restored`, `would_restore`, `failed` or `skipped`.
    pub status: &'static str,
    pub code: Option<String>,
    /// The name the original came back under (a restore as a copy is `name (restored).ext`).
    pub restored_as: Option<String>,
    /// For a split scan: what became of each derived file.
    pub derived: Vec<DerivedOut>,
    /// The file as it is now, in a dry run: `as_saved`, `modified`, `missing` or `original`.
    pub current: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DerivedOut {
    pub name: String,
    pub state: String,
}

fn key(p: &str) -> String {
    let s = std::path::absolute(Path::new(p))
        .unwrap_or_else(|_| PathBuf::from(p))
        .to_string_lossy()
        .into_owned();
    if cfg!(windows) { s.to_lowercase() } else { s }
}

fn is_backup_id(s: &str) -> bool {
    let id = s.split('/').next().unwrap_or("");
    id.len() == 28 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

fn hash_of(p: &Path) -> Option<String> {
    fs::read(p).ok().map(|b| blake3_hex(&b))
}

/// The manifest to restore for what the user typed.
enum Found {
    One(Box<Manifest>),
    AlreadyRestored(String),
    None,
}

fn find(store: &Store, target: &str) -> Found {
    if is_backup_id(target) {
        let id = target.split('/').next().unwrap_or("");
        return match store.read(id) {
            Some(m) if m.state == BackupState::Restored => Found::AlreadyRestored(m.id),
            Some(m) => Found::One(Box::new(m)),
            None => Found::None,
        };
    }
    let want = key(target);
    // Newest first: the latest save of that file is the one a restore undoes.
    store
        .list()
        .into_iter()
        .filter(|m| m.state == BackupState::Saved)
        .find(|m| key(&m.original_path) == want || m.outputs.iter().any(|o| key(&o.path) == want))
        .map_or(Found::None, |m| Found::One(Box::new(m)))
}

/// What a dry run reports about the file as it is now.
fn current_state(m: &Manifest) -> &'static str {
    let target = Path::new(&m.original_path);
    let Some(cur) = hash_of(target) else {
        return "missing";
    };
    if cur == m.original_blake3 {
        return "original";
    }
    match m.outputs.first() {
        Some(o) if o.blake3 == cur => "as_saved",
        _ if m.kind == BackupKind::OneToN => "as_saved",
        _ => "modified",
    }
}

fn outcome_item(target: &str, m: &Manifest, out: auto_crop_engine::RestoreOutcome) -> RestoreItem {
    let derived: Vec<DerivedOut> = out
        .derived
        .iter()
        .map(|d| DerivedOut {
            name: d.name.clone(),
            state: serde_json::to_value(d.state)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default(),
        })
        .collect();
    let (status, c) = if out.ok {
        ("restored", None)
    } else if out.needs_choice {
        ("failed", Some(code::MODIFIED_SINCE_SAVE.to_owned()))
    } else {
        (
            "failed",
            Some(err_code(out.error.unwrap_or(ErrKind::Internal))),
        )
    };
    RestoreItem {
        target: target.to_owned(),
        backup_id: Some(m.id.clone()),
        status,
        code: c,
        restored_as: out.restored,
        derived,
        current: None,
    }
}

fn line(it: &RestoreItem) -> String {
    let mut s = format!("{:<13} {}", it.status.replace('_', " "), it.target);
    if let Some(r) = &it.restored_as {
        s.push_str(&format!(" -> {r}"));
    }
    if let Some(c) = &it.current {
        s.push_str(&format!(" (file is {})", c.replace('_', " ")));
    }
    if let Some(c) = &it.code {
        s.push_str(&format!("  {c}"));
    }
    if it.code.as_deref() == Some(code::MODIFIED_SINCE_SAVE) {
        s.push_str(" (use --if-modified backup to restore anyway, or copy)");
    }
    s
}

pub fn run(args: RestoreArgs, env: &Env) -> u8 {
    let g = &env.global;
    let settings = base_settings(&env.paths, g);
    // A real restore first finishes or undoes a save that a crash interrupted, but never purges:
    // the backup you are about to restore must not be deleted for being past its retention.
    // A dry run touches nothing.
    let engine = Engine::with_settings(
        env.paths.clone(),
        settings,
        if args.dry_run {
            Housekeeping::None
        } else {
            Housekeeping::Recover
        },
    );
    let store = Store::new(env.paths.backups_dir());
    let mode = match args.if_modified {
        IfModified::Fail => RestoreMode::Auto,
        IfModified::Backup => RestoreMode::ReplaceAnyway,
        IfModified::Copy => RestoreMode::AsCopy,
    };
    let derived = if args.remove_derived {
        DerivedAction::Remove
    } else {
        DerivedAction::Keep
    };

    // What to restore: (what the user named, the manifest or why not).
    let mut jobs: Vec<(String, Found)> = Vec::new();
    if let Some(run) = &args.run {
        let mut ms: Vec<Manifest> = store
            .list()
            .into_iter()
            .filter(|m| m.run_id == *run && m.state == BackupState::Saved)
            .collect();
        ms.reverse(); // oldest first, like the app's "restore all from this run"
        if ms.is_empty() {
            jobs.push((format!("--run {run}"), Found::None));
        }
        for m in ms {
            jobs.push((m.original_path.clone(), Found::One(Box::new(m))));
        }
    } else {
        for t in &args.targets {
            jobs.push((t.clone(), find(&store, t)));
        }
    }

    let mut items: Vec<RestoreItem> = Vec::new();
    for (target, found) in jobs {
        if env.cancel.is_cancelled() {
            break;
        }
        let item = match found {
            Found::None => RestoreItem {
                target,
                backup_id: None,
                status: "failed",
                code: Some(code::NO_BACKUP.to_owned()),
                restored_as: None,
                derived: Vec::new(),
                current: None,
            },
            Found::AlreadyRestored(id) => RestoreItem {
                target,
                backup_id: Some(id),
                status: "skipped",
                code: Some(code::ALREADY_RESTORED.to_owned()),
                restored_as: None,
                derived: Vec::new(),
                current: None,
            },
            Found::One(m) if args.dry_run => {
                let cur = current_state(&m);
                let blocked = cur == "modified" && args.if_modified == IfModified::Fail;
                RestoreItem {
                    target,
                    backup_id: Some(m.id.clone()),
                    status: if blocked { "failed" } else { "would_restore" },
                    code: blocked.then(|| code::MODIFIED_SINCE_SAVE.to_owned()),
                    restored_as: None,
                    derived: Vec::new(),
                    current: Some(cur),
                }
            }
            Found::One(m) => {
                let out =
                    engine.restore_file_derived(&format!("{}/0", m.id), mode, derived, &|_| {});
                outcome_item(&target, &m, out)
            }
        };
        if !g.json && !g.ndjson && (!g.quiet || item.status == "failed") {
            eprintln!("{}", line(&item));
        }
        if g.ndjson {
            let line =
                serde_json::to_string(&serde_json::json!({"v": 1, "t": "item", "item": item}))
                    .unwrap_or_default();
            let _ = writeln!(std::io::stdout().lock(), "{line}");
        }
        items.push(item);
    }
    let failed = items.iter().filter(|i| i.status == "failed").count();
    let done = items
        .iter()
        .filter(|i| matches!(i.status, "restored" | "would_restore"))
        .count();
    let cancelled = env.cancel.is_cancelled();
    let code = if cancelled {
        exit::CANCELLED
    } else if failed > 0 {
        exit::FAILED
    } else {
        exit::OK
    };
    if g.json {
        let doc = serde_json::json!({
            "schema": "auto-crop/restore", "v": 1, "dry_run": args.dry_run,
            "summary": {"items": items.len(), "restored": done, "failed": failed},
            "exit_code": code, "items": items,
        });
        let _ = writeln!(
            std::io::stdout().lock(),
            "{}",
            serde_json::to_string_pretty(&doc).unwrap_or_default()
        );
    } else if g.ndjson {
        let line = serde_json::to_string(&serde_json::json!({"v": 1, "t": "end", "summary": {"items": items.len(), "restored": done, "failed": failed}, "exit_code": code})).unwrap_or_default();
        let _ = writeln!(std::io::stdout().lock(), "{line}");
    } else if !g.quiet {
        eprintln!(
            "{}: {} of {} {}, {} failed",
            if args.dry_run { "dry run" } else { "done" },
            done,
            items.len(),
            if args.dry_run {
                "would be restored"
            } else {
                "restored"
            },
            failed
        );
    }
    code
}
