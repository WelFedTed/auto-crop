# 0011 - CLI argument parser: hand-written, table-driven

- **Status:** accepted
- **Date:** 2026-10-06
- **Roadmap items:** M2.39, M2.43, M2.67
- **Decision log links:** B2 (dependency policy), B18 (no network), A-1
- **Time box:** none (not a spike)

## Context

The command-line tool needs a parser for six commands with about forty options. PLAN 2.12 and M2.39 name `clap` 4
(derive, with `clap_complete` and `clap_mangen`). The task that built the tool allowed either, provided the choice
is recorded and the dependency and network guards stay green.

## Options considered

| Option | Result (numbers) | Notes |
|---|---|---|
| `clap` 4 with `derive` | A scratch crate depending on it resolves 20 packages; 9 of them are not in this workspace's `Cargo.lock` (`anstream`, `anstyle-parse`, `anstyle-query`, `anstyle-wincon`, `clap_derive`, `colorchoice`, `is_terminal_polyfill`, `once_cell_polyfill`, `utf8parse`), and `clap_derive` is a proc macro. `cargo tree -p auto-crop-cli -i clap` finds none today: `clap` is in the lock file for developer tools only. | Free help, completions and man pages. Licences are all MIT or Apache-2.0 (checked by `cargo deny` when added); none is a network crate. |
| Hand-written, one flag table per command | 0 new packages; `crates/cli/src/args.rs` is about 1,200 lines including the help text, typed options, its unit tests and a 20,000-case arbitrary-input test (all of `auto-crop-cli`'s unit tests run in about 0.4 s). | The same tables generate `--help` and are checked against `docs/cli.md` by a test (every option, exit code and code must be in the page). No completions or man pages. |

## Results

Both would pass `cargo deny check` and the network guard; the difference is dependency count (+9 packages against 0),
build time of the shipped binary, and who owns the behaviour. The hand-written parser handles `--flag=value`,
clustered short flags, attached short values, `--`, suggestions for mistyped options and the usage
errors, and the CLI surface is small and fixed by the plan.

## Decision

Hand-written, table-driven, in `crates/cli/src/args.rs`. Completions and man pages (M2.67) are not provided yet; if
they are wanted before 1.0, generate them from the flag tables (they are plain data) or switch to `clap` behind the
same typed `Cli` structure, which is the only thing the rest of the crate sees.

**GO / NO-GO:** GO, no `clap` dependency for now; revisit when M2.67 needs completions and man pages.

## Consequences

- `docs/cli.md` is the reference and is kept in step by tests, not generated.
- The parser's behaviour (value flags take the next argument whatever it looks like; `-x` clusters; unknown flags
  suggest the nearest name) is ours to keep stable, and is covered by `args::tests`.
- The CLI keeps linking `auto-crop-engine`, `-core`, `-codecs` and `-imgproc` directly (the output writer of the
  copy modes uses the decode, render and encode functions of the engine's own save path), `ctrlc` for Ctrl+C, and
  `sysinfo` (already in the engine) for `doctor`. No HTTP, TLS or socket crate (`cargo xtask ci-guards`).
