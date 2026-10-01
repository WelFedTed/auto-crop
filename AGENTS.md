# Instructions for AI coding agents

This file is for any AI coding agent working in this repository. [CLAUDE.md](CLAUDE.md) carries the same rules for Claude Code; keep them in sync.

**Status: planning stage / milestone M0.** Read [PLAN.md](PLAN.md), the [decision log](docs/plan/00-decision-log.md) (authoritative) and [ROADMAP.md](ROADMAP.md) before changing anything.

## Ticking rule (ROADMAP.md)

- Tick a box (`- [x]`) **only in the commit that implements and verifies the item** (tests or measurements pass).
- **Never tick a GATE item by inference**; link measured evidence.
- **Never renumber or delete an ID.** Retired items disappear; their IDs are never reused. Append new items with the next free ID.
- Run `cargo xtask roadmap-check` (once it exists) before committing.

## Other rules

- Decisions B1-B21 and assumptions A-1..A-12 in the decision log are binding. Raise disagreements with the owner; do not override silently.
- No AGPL, GPL or non-commercial dependencies or weights. No telemetry, no network by default.
- Sign off commits (`git commit -s`), Conventional Commits, push to `main` (the owner's workflow).
- Disclose AI assistance as described in [CONTRIBUTING.md](CONTRIBUTING.md).
