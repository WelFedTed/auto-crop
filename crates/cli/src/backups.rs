// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `auto-crop backups list | show | purge` (ROADMAP M2.47). Listings never write and never purge.
//! `purge` is the one command that destroys a stored original, so it needs a selection
//! (`--expired`, `--older-than`, `--id` or `--all`), a confirmation (`--yes`, or an answer at a
//! terminal; a script without `--yes` is refused), and it never touches a backup that is still
//! being committed or one that is pinned in the app (except by an explicit `--id`).

use crate::args::{BackupsCmd, PurgeSelect};
use crate::env::{Env, base_settings};
use crate::exit;
use crate::manifest::code;
use crate::report::stdin_is_tty;
use auto_crop_engine::store::{BackupState, Manifest, Store};
use auto_crop_engine::util::{now_secs, rfc3339};
use auto_crop_engine::{Engine, Housekeeping};
use serde::Serialize;
use std::fs;
use std::io::{BufRead, Write};
use std::path::Path;

fn mb(bytes: u64) -> String {
    if bytes >= 1 << 30 {
        format!("{:.1} GB", bytes as f64 / f64::from(1u32 << 30))
    } else {
        format!("{:.1} MB", bytes as f64 / f64::from(1u32 << 20))
    }
}

fn out_json<T: Serialize>(v: &T) {
    let _ = writeln!(
        std::io::stdout().lock(),
        "{}",
        serde_json::to_string_pretty(v).unwrap_or_default()
    );
}

fn dir_bytes(p: &Path) -> u64 {
    let Ok(rd) = fs::read_dir(p) else { return 0 };
    rd.flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => dir_bytes(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

fn state_word(s: BackupState) -> &'static str {
    match s {
        BackupState::BackedUp => "backed_up",
        BackupState::Saved => "saved",
        BackupState::Restored => "restored",
    }
}

pub fn run(cmd: BackupsCmd, env: &Env) -> u8 {
    let g = &env.global;
    let store = Store::new(env.paths.backups_dir());
    match cmd {
        BackupsCmd::List => {
            // No recovery, no purge: listing never writes.
            let engine = Engine::with_settings(
                env.paths.clone(),
                base_settings(&env.paths, g),
                Housekeeping::None,
            );
            let view = engine.list_backups();
            if g.json {
                #[derive(Serialize)]
                struct Doc<'a> {
                    schema: &'static str,
                    v: u32,
                    #[serde(flatten)]
                    view: &'a auto_crop_engine::BackupsView,
                }
                out_json(&Doc {
                    schema: "auto-crop/backups",
                    v: 1,
                    view: &view,
                });
                return exit::OK;
            }
            let mut out = std::io::stdout().lock();
            let _ = writeln!(
                out,
                "backups in {} ({} used)",
                view.location,
                mb(view.used_bytes)
            );
            if view.runs.is_empty() {
                let _ = writeln!(out, "none");
            }
            for r in &view.runs {
                let exp = r
                    .expires_at
                    .as_deref()
                    .map_or(String::new(), |e| format!(", expires {e}"));
                let _ = writeln!(
                    out,
                    "run {}  {}  {} files, {}{exp}{}",
                    r.id,
                    r.created_at,
                    r.file_count,
                    mb(r.total_bytes),
                    if r.pinned { ", pinned" } else { "" }
                );
                for f in &r.files {
                    let id = f.id.split('/').next().unwrap_or(&f.id);
                    let state = if f.restored {
                        "restored"
                    } else if f.changed_since_saved {
                        "changed since saved"
                    } else {
                        "saved"
                    };
                    let _ = writeln!(out, "  {id}  {}  [{state}]", f.display_path);
                }
            }
            exit::OK
        }
        BackupsCmd::Show(id) => {
            let id = id.split('/').next().unwrap_or("").to_owned();
            let Some(m) = store.read(&id) else {
                eprintln!("error: no backup {id} ({})", code::NO_BACKUP);
                return exit::FAILED;
            };
            if g.json {
                #[derive(Serialize)]
                struct Doc<'a> {
                    schema: &'static str,
                    v: u32,
                    state: &'static str,
                    backup: &'a Manifest,
                }
                out_json(&Doc {
                    schema: "auto-crop/backup",
                    v: 1,
                    state: state_word(m.state),
                    backup: &m,
                });
                return exit::OK;
            }
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "backup {}", m.id);
            let _ = writeln!(out, "  run:       {} ({})", m.run_id, m.run_name);
            let _ = writeln!(out, "  made:      {}", rfc3339(m.created_at));
            let _ = writeln!(
                out,
                "  expires:   {}",
                m.purge_after.map_or("never".to_owned(), rfc3339)
            );
            let _ = writeln!(
                out,
                "  state:     {}{}",
                state_word(m.state),
                if m.pinned { ", pinned" } else { "" }
            );
            let _ = writeln!(
                out,
                "  original:  {} ({} bytes, {})",
                m.original_path, m.original_size, m.format
            );
            let _ = writeln!(out, "  blake3:    {}", m.original_blake3);
            let _ = writeln!(
                out,
                "  stored in: {}",
                store
                    .entry_path(&m.id)
                    .map_or(String::new(), |p| p.display().to_string())
            );
            for o in &m.outputs {
                let _ = writeln!(out, "  output:    {} ({} bytes)", o.path, o.size);
            }
            exit::OK
        }
        BackupsCmd::Purge {
            select,
            yes,
            dry_run,
        } => purge(&store, select, yes, dry_run, env),
    }
}

#[derive(Debug, Serialize)]
struct Candidate {
    id: String,
    original_path: String,
    state: &'static str,
    bytes: u64,
}

fn purge(store: &Store, select: PurgeSelect, yes: bool, dry_run: bool, env: &Env) -> u8 {
    let g = &env.global;
    let now = now_secs();
    let all = store.list();
    let mut missing: Vec<String> = Vec::new();
    let chosen: Vec<Manifest> = match &select {
        PurgeSelect::Expired => all
            .into_iter()
            .filter(|m| {
                m.purge_after.is_some_and(|t| t <= now)
                    && !m.pinned
                    && m.state != BackupState::BackedUp
            })
            .collect(),
        PurgeSelect::OlderThan(days) => {
            let cutoff = now - i64::from(*days) * 86_400;
            all.into_iter()
                .filter(|m| m.created_at <= cutoff && !m.pinned && m.state != BackupState::BackedUp)
                .collect()
        }
        PurgeSelect::All => all
            .into_iter()
            .filter(|m| !m.pinned && m.state != BackupState::BackedUp)
            .collect(),
        PurgeSelect::Ids(ids) => {
            let mut v = Vec::new();
            for id in ids {
                let id = id.split('/').next().unwrap_or("");
                match all.iter().find(|m| m.id == id) {
                    // A backup still being committed may be what a crash recovery needs.
                    Some(m) if m.state == BackupState::BackedUp => {
                        eprintln!("error: backup {id} is still being committed and is kept");
                        missing.push(id.to_owned());
                    }
                    Some(m) => v.push(m.clone()),
                    None => {
                        eprintln!("error: no backup {id} ({})", code::NO_BACKUP);
                        missing.push(id.to_owned());
                    }
                }
            }
            v
        }
    };
    let cands: Vec<Candidate> = chosen
        .iter()
        .map(|m| Candidate {
            id: m.id.clone(),
            original_path: m.original_path.clone(),
            state: state_word(m.state),
            bytes: store.entry_path(&m.id).map_or(0, |p| dir_bytes(&p)),
        })
        .collect();
    let total: u64 = cands.iter().map(|c| c.bytes).sum();
    if !g.json && !g.quiet {
        for c in &cands {
            eprintln!(
                "{} {}  {}  ({})",
                if dry_run { "would delete" } else { "delete" },
                c.id,
                c.original_path,
                mb(c.bytes)
            );
        }
    }
    let fail_code = if missing.is_empty() {
        exit::OK
    } else {
        exit::FAILED
    };
    if dry_run || cands.is_empty() {
        if g.json {
            out_json(
                &serde_json::json!({"schema": "auto-crop/backups-purge", "v": 1, "dry_run": dry_run, "deleted": 0, "candidates": cands, "bytes": total}),
            );
        } else if !g.quiet {
            eprintln!(
                "{}: {} backups ({}); nothing was deleted",
                if dry_run {
                    "dry run"
                } else {
                    "nothing to purge"
                },
                cands.len(),
                mb(total)
            );
        }
        return fail_code;
    }
    if !yes {
        if !stdin_is_tty() {
            eprintln!(
                "error: purge deletes {} stored originals ({}); pass --yes to confirm (nothing was deleted)",
                cands.len(),
                mb(total)
            );
            return exit::USAGE;
        }
        eprint!(
            "Delete {} backups ({})? Their originals can no longer be restored. [y/N] ",
            cands.len(),
            mb(total)
        );
        let _ = std::io::stderr().flush();
        let mut answer = String::new();
        let _ = std::io::stdin().lock().read_line(&mut answer);
        if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
            eprintln!("nothing was deleted");
            return exit::OK;
        }
    }
    let mut deleted = 0usize;
    let mut failed = 0usize;
    for c in &cands {
        if env.cancel.is_cancelled() {
            break;
        }
        if store.remove(&c.id) {
            deleted += 1;
        } else {
            failed += 1;
            eprintln!("error: could not delete backup {}", c.id);
        }
    }
    if g.json {
        out_json(
            &serde_json::json!({"schema": "auto-crop/backups-purge", "v": 1, "dry_run": false, "deleted": deleted, "failed": failed, "bytes": total}),
        );
    } else if !g.quiet {
        eprintln!("deleted {deleted} backups ({})", mb(total));
    }
    if env.cancel.is_cancelled() {
        exit::CANCELLED
    } else if failed > 0 || fail_code != exit::OK {
        exit::FAILED
    } else {
        exit::OK
    }
}
