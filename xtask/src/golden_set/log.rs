// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The append-only evaluation log (ROADMAP M1.44): every evaluation of the golden set is one JSON
//! line in `eval-log.jsonl`, chained to the line before it by SHA-256, with a small side file
//! (`eval-log.head`: entry count and the hash of the last line). Editing, removing or re-ordering
//! a line breaks the chain; cutting lines off the end breaks the head. This makes tampering
//! visible, not impossible: someone who rewrites both files consistently is not stopped, and the
//! log lives on the owner's own disk, so the point is to make repeated peeking at the locked set
//! impossible to forget, not to defend against the owner.

use super::common::{Paths, sha256_bytes, write_atomic};
use serde::{Deserialize, Serialize};
use std::io::Write;

pub const ZERO: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogEntry {
    pub seq: u64,
    /// SHA-256 of the previous line's text (all zeros for the first).
    pub prev: String,
    pub at: String,
    pub who: String,
    pub commit: String,
    pub dirty: bool,
    /// `dev` or `locked`.
    pub set: String,
    pub predictor: String,
    pub n: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub lock_sha256: String,
    /// SHA-256 of the aggregate metrics file the run produced.
    pub aggregate_sha256: String,
}

#[derive(Debug, Clone, Default)]
pub struct LogState {
    pub entries: Vec<LogEntry>,
    pub tip: String,
}

impl LogState {
    /// Evaluations of the locked set so far.
    pub fn locked_evaluations(&self) -> usize {
        self.entries.iter().filter(|e| e.set == "locked").count()
    }
}

/// Reads and verifies the log against its head file.
pub fn verify(p: &Paths) -> Result<LogState, String> {
    let (log, head) = (p.eval_log(), p.eval_head());
    let text = match std::fs::read_to_string(&log) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("cannot read {}: {e}", log.display())),
    };
    let head_text = std::fs::read_to_string(&head).ok();
    let Some(text) = text else {
        return if head_text.is_some() {
            Err(format!(
                "{} is missing but {} exists: the evaluation log was deleted",
                log.display(),
                head.display()
            ))
        } else {
            Ok(LogState {
                entries: Vec::new(),
                tip: ZERO.to_owned(),
            })
        };
    };
    let mut state = LogState {
        entries: Vec::new(),
        tip: ZERO.to_owned(),
    };
    for (i, line) in text.lines().filter(|l| !l.trim().is_empty()).enumerate() {
        let e: LogEntry = serde_json::from_str(line)
            .map_err(|er| format!("eval-log.jsonl line {}: {er}", i + 1))?;
        if e.seq != i as u64 + 1 {
            return Err(format!(
                "eval-log.jsonl line {}: sequence number {} where {} was expected (a line was removed or re-ordered)",
                i + 1,
                e.seq,
                i + 1
            ));
        }
        if e.prev != state.tip {
            return Err(format!(
                "eval-log.jsonl line {}: the chain is broken (an earlier line was edited, removed or inserted)",
                i + 1
            ));
        }
        state.tip = sha256_bytes(line.as_bytes());
        state.entries.push(e);
    }
    let expected = format!("{} {}", state.entries.len(), state.tip);
    match head_text {
        Some(h) if h.trim() == expected => Ok(state),
        Some(_) => Err(
            "eval-log.jsonl does not match eval-log.head: the log was rewritten or cut short"
                .to_owned(),
        ),
        None if state.entries.is_empty() => Ok(state),
        None => Err("eval-log.head is missing: the log cannot be verified".to_owned()),
    }
}

/// Appends one entry (filling `seq` and `prev`), after verifying the log is intact.
pub fn append(p: &Paths, mut entry: LogEntry) -> Result<LogEntry, String> {
    let state = verify(p)?;
    entry.seq = state.entries.len() as u64 + 1;
    entry.prev = state.tip;
    let line = serde_json::to_string(&entry).map_err(|e| e.to_string())?;
    if let Some(dir) = p.eval_log().parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(p.eval_log())
        .map_err(|e| format!("cannot open the evaluation log: {e}"))?;
    f.write_all(format!("{line}\n").as_bytes())
        .and_then(|()| f.sync_all())
        .map_err(|e| format!("cannot append to the evaluation log: {e}"))?;
    write_atomic(
        &p.eval_head(),
        format!("{} {}\n", entry.seq, sha256_bytes(line.as_bytes())).as_bytes(),
    )?;
    Ok(entry)
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn paths(dir: &std::path::Path) -> Paths {
        let golden = dir.join("golden");
        Paths {
            data: dir.to_owned(),
            images: dir.to_owned(),
            labels: golden.join("labels"),
            golden,
        }
    }

    pub fn entry(set: &str) -> LogEntry {
        LogEntry {
            seq: 0,
            prev: String::new(),
            at: "2026-10-04T00:00:00Z".to_owned(),
            who: "tester".to_owned(),
            commit: "abc".to_owned(),
            dirty: false,
            set: set.to_owned(),
            predictor: "detector".to_owned(),
            n: 10,
            reason: None,
            lock_sha256: "l".repeat(64),
            aggregate_sha256: "a".repeat(64),
        }
    }

    #[test]
    fn an_empty_log_verifies_and_appends_chain() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = paths(dir.path());
        assert!(verify(&p).expect("empty ok").entries.is_empty());
        let a = append(&p, entry("dev")).expect("append");
        let b = append(&p, entry("locked")).expect("append");
        assert_eq!((a.seq, b.seq), (1, 2));
        assert_eq!(
            b.prev,
            sha256_bytes(serde_json::to_string(&a).expect("json").as_bytes())
        );
        let s = verify(&p).expect("verifies");
        assert_eq!(s.entries.len(), 2);
        assert_eq!(s.locked_evaluations(), 1);
    }

    #[test]
    fn every_kind_of_rewrite_is_detected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = paths(dir.path());
        for set in ["dev", "locked", "locked"] {
            append(&p, entry(set)).expect("append");
        }
        let original = std::fs::read_to_string(p.eval_log()).expect("read");
        let lines: Vec<&str> = original.lines().collect();
        let put = |text: &str| std::fs::write(p.eval_log(), text).expect("write");
        // Editing the first line breaks the chain at the second.
        put(&original.replacen("tester", "someone", 1));
        assert!(verify(&p).expect_err("edited").contains("chain is broken"));
        // Editing the last line is caught by the head.
        let last_edited = format!(
            "{}\n{}\n{}\n",
            lines[0],
            lines[1],
            lines[2].replace("tester", "x")
        );
        put(&last_edited);
        assert!(verify(&p).expect_err("edited tip").contains("rewritten"));
        // Dropping the last line (peeking and erasing the evidence).
        put(&format!("{}\n{}\n", lines[0], lines[1]));
        assert!(
            verify(&p)
                .expect_err("cut")
                .contains("rewritten or cut short")
        );
        // Dropping a middle line.
        put(&format!("{}\n{}\n", lines[0], lines[2]));
        assert!(verify(&p).is_err());
        // Swapping two lines.
        put(&format!("{}\n{}\n{}\n", lines[0], lines[2], lines[1]));
        assert!(verify(&p).is_err());
        // Deleting the log but leaving the head.
        std::fs::remove_file(p.eval_log()).expect("remove");
        assert!(verify(&p).expect_err("deleted").contains("deleted"));
        // Appending refuses to extend a broken log.
        assert!(append(&p, entry("dev")).is_err());
        // Restoring the original makes everything verify again.
        put(&original);
        assert_eq!(verify(&p).expect("restored").entries.len(), 3);
        // A log without a head cannot be verified.
        std::fs::remove_file(p.eval_head()).expect("remove");
        assert!(verify(&p).expect_err("no head").contains("head"));
    }
}
