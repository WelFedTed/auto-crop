// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Everything a person reads on stderr while a batch runs (stdout is for machines). Quiet prints
//! failures only; the default also prints held and skipped items and one summary; verbose prints
//! a line for every item. A progress line is shown on a terminal (or with `--progress always`)
//! and is erased before any other line, so output stays readable when redirected.

use crate::args::{Global, Progress};
use crate::manifest::{ItemRecord, Status, Summary};
use std::io::{IsTerminal, Write};

pub fn stderr_is_tty() -> bool {
    std::io::stderr().is_terminal()
}

pub fn stdin_is_tty() -> bool {
    std::io::stdin().is_terminal()
}

pub struct Reporter {
    quiet: bool,
    verbose: bool,
    progress: bool,
    dry_run: bool,
    total: usize,
    done: usize,
    shown: usize,
}

impl Reporter {
    pub fn new(g: &Global, progress: Progress, total: usize, dry_run: bool) -> Self {
        let wanted = match progress {
            Progress::Always => true,
            Progress::Never => false,
            Progress::Auto => stderr_is_tty(),
        };
        Self {
            quiet: g.quiet,
            verbose: g.verbose,
            // Machine output modes keep stderr quiet of progress too.
            progress: wanted && !g.quiet,
            dry_run,
            total,
            done: 0,
            shown: 0,
        }
    }

    fn erase(&mut self) {
        if self.shown > 0 {
            let mut e = std::io::stderr().lock();
            let _ = write!(e, "\r{}\r", " ".repeat(self.shown));
            self.shown = 0;
        }
    }

    /// A line of text on stderr (a warning, a note), above the progress line.
    pub fn line(&mut self, text: &str) {
        self.erase();
        let _ = writeln!(std::io::stderr().lock(), "{text}");
    }

    /// Printed unless `--quiet`.
    pub fn info(&mut self, text: &str) {
        if !self.quiet {
            self.line(text);
        }
    }

    pub fn item(&mut self, rec: &ItemRecord) {
        self.done += 1;
        let show = match rec.status {
            Status::Failed => true,
            Status::Held | Status::Skipped => !self.quiet,
            Status::Saved => self.verbose,
        };
        if show {
            let text = describe(rec, self.dry_run);
            self.line(&text);
        }
        if self.progress && self.done < self.total {
            self.erase();
            let name: String = rec
                .input
                .chars()
                .rev()
                .take(48)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            let text = format!("[{}/{}] {name}", self.done, self.total);
            let _ = write!(std::io::stderr().lock(), "{text}");
            let _ = std::io::stderr().flush();
            self.shown = text.chars().count();
        }
    }

    pub fn summary(&mut self, s: &Summary, secs: f64, cancelled: bool) {
        self.erase();
        if self.quiet {
            return;
        }
        let save = if self.dry_run {
            "would be saved"
        } else {
            "saved"
        };
        let mut parts = vec![format!("{} {save}", s.saved)];
        if s.held > 0 {
            parts.push(format!("{} held for review (not written)", s.held));
        }
        if s.failed > 0 {
            parts.push(format!("{} failed", s.failed));
        }
        if s.skipped > 0 {
            parts.push(format!("{} skipped", s.skipped));
        }
        let mut text = format!(
            "{}{}: {} of {} in {secs:.1} s",
            if self.dry_run {
                "dry run: nothing was written; "
            } else {
                ""
            },
            if cancelled { "interrupted" } else { "done" },
            parts.join(", "),
            s.items
        );
        let mut notes: Vec<String> = Vec::new();
        if s.ignored_non_image > 0 {
            notes.push(format!(
                "{} other files in the folders are not images",
                s.ignored_non_image
            ));
        }
        if s.links_skipped > 0 {
            notes.push(format!("{} links not followed", s.links_skipped));
        }
        if s.hidden_skipped > 0 {
            notes.push(format!(
                "{} hidden or system entries left out",
                s.hidden_skipped
            ));
        }
        if s.truncated {
            notes.push("stopped at --max-files".to_owned());
        }
        if !notes.is_empty() {
            text.push_str(&format!("\n  ({})", notes.join("; ")));
        }
        let _ = writeln!(std::io::stderr().lock(), "{text}");
    }
}

/// One line about one item.
pub fn describe(rec: &ItemRecord, dry_run: bool) -> String {
    let word = match (rec.status, dry_run) {
        (Status::Saved, true) => "would save",
        (s, _) => s.word(),
    };
    let mut line = format!("{word:<10} {}", rec.input);
    if rec.status == Status::Saved {
        match rec.outputs.as_slice() {
            [] => {}
            [one] if one.path == rec.input => {}
            [one] => line.push_str(&format!(" -> {}", one.path)),
            many => line.push_str(&format!(" -> {} files, first {}", many.len(), many[0].path)),
        }
    }
    if let Some(code) = &rec.code {
        line.push_str(&format!("  {code}"));
    }
    if !rec.reasons.is_empty() {
        line.push_str(&format!(" [{}]", rec.reasons.join(", ")));
    }
    if let Some(c) = &rec.confidence
        && rec.status == Status::Held
    {
        line.push_str(&format!(" (score {:.2}, {})", c.score, c.band));
    }
    if let Some(d) = &rec.detail {
        line.push_str(&format!(": {d}"));
    }
    line
}
