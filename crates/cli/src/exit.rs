// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Exit codes (PLAN 2.12, ROADMAP M2.45). One table, used by every command and by `docs/cli.md`.

/// Everything written, or skipped by choice (already processed, a format that is never replaced).
pub const OK: u8 = 0;
/// An internal error: a bug, not something the user can fix.
pub const INTERNAL: u8 = 1;
/// A usage error: the command line was wrong. Nothing was touched.
pub const USAGE: u8 = 2;
/// Some items failed; the others may have succeeded.
pub const FAILED: u8 = 3;
/// No failures, but some items were held for review (or the detection failed). Nothing was
/// written for them. `--hold-exit-zero` maps this to 0.
pub const HELD: u8 = 4;
/// No supported input was found.
pub const NO_INPUT: u8 = 5;
/// A precondition failed before anything was written (the CPU floor, an unwritable backup store,
/// a confirmation that cannot be asked).
pub const PRECONDITION: u8 = 6;
/// Interrupted (Ctrl+C or a termination signal) after a clean stop: the file in progress was
/// finished or left untouched, never half written.
pub const CANCELLED: u8 = 130;

/// The stable name of a code, as written to the manifest.
pub fn name(code: u8) -> &'static str {
    match code {
        OK => "ok",
        INTERNAL => "internal",
        USAGE => "usage",
        FAILED => "failed",
        HELD => "held",
        NO_INPUT => "no_input",
        PRECONDITION => "precondition",
        CANCELLED => "cancelled",
        _ => "unknown",
    }
}

/// Every code with its meaning, in table order (the docs test checks `docs/cli.md` against it).
#[cfg(test)]
pub const TABLE: [(u8, &str); 8] = [
    (OK, "all written, or skipped by choice"),
    (INTERNAL, "internal error"),
    (USAGE, "usage error"),
    (FAILED, "some items failed"),
    (HELD, "items held for review, none failed"),
    (NO_INPUT, "no supported input"),
    (PRECONDITION, "a precondition failed before any write"),
    (CANCELLED, "interrupted after a clean stop"),
];

/// What a batch ended with, for [`batch_exit`].
#[derive(Debug, Clone, Copy, Default)]
pub struct Outcome {
    pub failed: usize,
    pub held: usize,
    /// Items that were candidates (supported files found), before any skip or failure.
    pub candidates: usize,
    pub cancelled: bool,
    pub hold_exit_zero: bool,
}

/// The exit code of a finished batch. Precedence: a cancel, then failures (3), then held items
/// (4), then "no supported input" (5).
pub fn batch_exit(o: Outcome) -> u8 {
    if o.cancelled {
        CANCELLED
    } else if o.failed > 0 {
        FAILED
    } else if o.held > 0 && !o.hold_exit_zero {
        HELD
    } else if o.candidates == 0 {
        NO_INPUT
    } else {
        OK
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precedence_is_cancel_failed_held_no_input() {
        let base = Outcome::default();
        assert_eq!(batch_exit(base), NO_INPUT);
        let ok = Outcome {
            candidates: 3,
            ..base
        };
        assert_eq!(batch_exit(ok), OK);
        let held = Outcome { held: 1, ..ok };
        assert_eq!(batch_exit(held), HELD);
        assert_eq!(
            batch_exit(Outcome {
                hold_exit_zero: true,
                ..held
            }),
            OK
        );
        let failed = Outcome { failed: 1, ..held };
        assert_eq!(batch_exit(failed), FAILED);
        assert_eq!(
            batch_exit(Outcome {
                cancelled: true,
                ..failed
            }),
            CANCELLED
        );
    }

    #[test]
    fn every_code_has_a_name() {
        for (code, _) in TABLE {
            assert_ne!(name(code), "unknown");
        }
    }
}
