# Auto Crop: instructions for Claude Code

Auto Crop is a free, open-source (MIT OR Apache-2.0) cross-platform desktop app that automatically crops, rotates, deskews and perspective-corrects images, enhances black-and-white document scans and receipts, and converts formats (especially HEIC to JPG). Rust core, Tauri 2.x + Svelte 5 UI.

**Status: PLANNING ONLY.** No source code exists yet. Do not start implementing until the owner explicitly says so (ROADMAP.md item P.05, then milestone M0).

## Source-of-truth documents

| Document | Role |
|---|---|
| [PLAN.md](PLAN.md) | Entry point: scope, architecture at a glance, milestone overview, risks, assumptions the owner may veto |
| [docs/plan/00-decision-log.md](docs/plan/00-decision-log.md) | **Authoritative decisions (B1-B21) and assumptions.** If anything disagrees with it, the decision log wins. If you think a decision is wrong, raise it with the owner; never override it silently. |
| [docs/plan/](docs/plan/) | Detailed design, one file per area (01 vision ... 08 release/security/risks). `PLAN 2.7` means section 7 of `02-architecture.md` |
| [ROADMAP.md](ROADMAP.md) | **Living checklist**, grouped by milestone/release, with stable item IDs, exit gates and a progress table |
| [docs/research/](docs/research/README.md) | Research snapshots from 2026-09-30 (fact-checked once; re-verify versions, licences and policies before relying on them) |

## ROADMAP.md is a living document (rules)

1. **Tick a box in the same commit/PR that completes the work.** Change `- [ ]` to `- [x]` only when the item is implemented AND verified (tests or measurements pass). Never tick speculatively.
2. **IDs are permanent.** Never renumber and never reuse an ID. A retired item simply disappears from the list.
3. **New work discovered along the way:** append an item under the right milestone and area with the next free ID (for example `M4.91`), written as one line with its acceptance criterion. Design reasoning belongs in the matching PLAN section, not in the checklist.
4. **GATE items** (`GATE`) are ticked only with measured evidence linked from the commit or PR description. A missed gate is never quietly relaxed; it goes back to the owner (assumption A-4).
5. **Keep the progress table at the top of ROADMAP.md accurate** (regenerate it with `cargo xtask roadmap-check --write` once item M0.78 exists).
6. **Size guard:** keep ROADMAP.md under 450 KB (GitHub stops rendering Markdown near 512 KB). When a milestone is completely ticked, move its section to `docs/roadmap/archive/Mx.md` and leave a one-line pointer and its final progress count.

## Ground rules taken from the decisions

- Originals are **overwritten by default**, so every write must go through the safe-write path: verified temp output, automatic backup, atomic replace, "Restore original" (B3; commit protocol in PLAN 2.7). Low-confidence results are held for review, never written (B4).
- Licence policy (B2): never add AGPL, GPL or non-commercial dependencies or model weights. LGPL only as separate, dynamically linked, replaceable libraries (libheif/libde265, B12). `cargo-deny` enforces this once it exists.
- Offline and private by default (B18): no telemetry, no network calls unless the user opts in.
- Untrusted image parsers written in C (libheif, libde265, PDFium) run in the sandboxed worker-process pool with pixel, memory and time caps (B12).
- Non-goals for 1.0 (B19): OCR, scanner/camera capture, watch-folder processing, mobile, general photo editing.
- Numbers marked PROVISIONAL in the docs are unmeasured estimates; replace them with measured values as the benchmark harness comes online.

## Working conventions

- Conventional Commits; DCO sign-off (`git commit -s`); semantic versioning; releases via release-plz.
- The `core` crates must stay UI-agnostic (no Tauri types); Tauri-specific code lives only in the thin shell crate (B8).
- The private golden test set never enters the public repo or public CI logs (B21).
- AI assistance is disclosed openly in README and CONTRIBUTING (B17).
