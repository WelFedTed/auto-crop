# Auto Crop: plan

**Auto Crop: the offline batch fixer for scans, receipts and photos.** A free, open-source (MIT OR Apache-2.0) desktop app for Windows, macOS and Linux that automatically crops, rotates, deskews and perspective-corrects images, offers to whiten paper and sharpen text on black-and-white documents, and converts formats (first of all HEIC/HEIF to JPG).

**Status (2026-10-01): planning complete, no code written yet.** Repository to be created as `WelFedTed/auto-crop` (public). This file is the entry point; the reasoning lives in [docs/plan/](docs/plan/), the to-do list in [ROADMAP.md](ROADMAP.md).

## Start here

| Read this | For |
|---|---|
| **This file** | The whole plan in about ten minutes: scope, decisions, architecture, milestones, risks, what needs your decision |
| [ROADMAP.md](ROADMAP.md) | The living checklist: every feature and task as a checkbox, grouped by milestone and release. Tick boxes as work completes |
| [docs/plan/00-decision-log.md](docs/plan/00-decision-log.md) | The authoritative decisions B1-B21 from the interview, plus the interpretations A-1..A-12 you may veto |
| [docs/plan/](docs/plan/) | Eight design documents (vision, architecture, formats, detection, enhancement, UX, performance/quality, release/security/risks) |
| [docs/research/](docs/research/README.md) | The research snapshots and fact-checks behind the decisions |
| [CLAUDE.md](CLAUDE.md) | Instructions for AI coding sessions working in this repo (including the roadmap ticking rule) |

In the design documents, `PLAN 2.7` means section 7 of [02-architecture.md](docs/plan/02-architecture.md).

## What it is, in your words

Every line of your brief maps to a decision and to a place in the plan:

| Your requirement | What the plan does | Where |
|---|---|---|
| Open files and folders, drag and drop | Native picker and OS drop in Rust (opaque IDs to the UI), recursive folder open, OS "Open with" integration | PLAN 2.13, 6.2; M3, M5 |
| Automate crop, rotate, skew and distortion correction | Hybrid detection: classical plus small bundled ONNX models, sub-pixel refinement, perspective rectification, orientation, deskew, multi-item split, curved-page dewarp | PLAN 4; M2, M4, M10, M12 |
| Improve contrast and readability of B&W scans, e.g. receipts | Suggested, never forced: flatten the background, whiten the paper, optional B&W. Colour kept by default; despeckle off for receipts | PLAN 5; M7 |
| Very fast, very accurate | Measured, not claimed: a benchmark and accuracy harness built before any GUI; gates on measured numbers; calibrated confidence; "unsure" means held for review | PLAN 7; M1, every gate |
| Previews with manual adjustment | Auto result shown immediately, draggable quad handles with a loupe, before/after, per-image or apply-to-all | PLAN 6; M3, M5 |
| Undo | Edits are parameters, so undo/redo is cheap; "Restore original" survives saving and restarts | PLAN 2.5-2.7; M2, M3, M5 |
| Free and open source, public repo `auto-crop` | MIT OR Apache-2.0, DCO, public repo `WelFedTed/auto-crop`, AI assistance disclosed | PLAN 8; M0 |
| Windows, macOS, Linux | CI on all three from day one; Windows previews first; 1.0 needs all three | B9; M8, M9 |
| Rust backend? | Yes for the core, with honest caveats (HEIC, PDF and ML runtimes are C/C++) | PLAN 1.8 |
| Intuitive, touch-first GUI | 48 px handles, loupe, pinch/rotate, always-visible buttons, non-drag alternatives, keyboard and screen-reader support | PLAN 6; M3 |
| All image formats | A defined support matrix ("all common formats" plus a documented long tail), not a literal promise | PLAN 3; M6, M11 |
| Automated conversion, HEIC/HEIF to JPG | Bundled decode-only libheif + libde265, sandboxed; sRGB by default with a wide-gamut toggle | PLAN 3.4; M6 |
| ROADMAP.md living checklist | Done: a checklist per milestone with stable IDs, gates and a progress table | [ROADMAP.md](ROADMAP.md) |

## Decisions that shape everything

Full table with rationale and the research that was overridden: [PLAN 1.6](docs/plan/01-vision-decisions.md).

- **Inputs:** phone photos of documents/receipts, flatbed scans, multi-photo flatbed scans, general photos (B1).
- **Licence:** MIT OR Apache-2.0. No AGPL/GPL/non-commercial dependencies or model weights; LGPL only as separate, replaceable libraries (B2).
- **Originals are overwritten by default, safely:** verified output, automatic backup first, atomic replace, "Restore original" for about 30 days, in-session undo (B3). Low-confidence results are **held, never written**, and listed first in the review grid (B4). Batch flow: auto-process everything, review flagged items first, then "Save all" (B5).
- **Accuracy bar:** Balanced means at most 1% silent failures with about 8-10% flagged; Strict and Aggressive per batch (B6).
- **Detection:** hybrid classical plus a small bundled local ML model, fully offline (B7).
- **GUI:** Tauri 2.x with Svelte 5 and TypeScript; Rust owns every pixel. Linux touch is best-effort; Slint is the gated fallback (B8).
- **Platforms:** Windows 10/11 x64 first; all three OSes in CI from day one; 1.0 requires all three (B9).
- **1.0 scope:** everything you selected, delivered as incremental 0.x previews: B&W enhancement, multi-photo split, headless CLI, curved-page dewarp (B10); exports JPEG, PNG, WebP, AVIF, TIFF (incl. 1-bit G4), PDF, JPEG XL (B11).
- **HEIC:** libde265 with a decode-only libheif is bundled in all official builds, as separate dynamically linked libraries in a sandboxed worker pool (B12).
- **Budget $0:** SignPath Foundation, Store MSIX, own Homebrew tap, unsigned macOS DMG, own Flatpak remote (B15). Repo `WelFedTed/auto-crop`, app ID `io.github.welfedted.AutoCrop` (B16).
- **Offline and private:** no telemetry; manual update check plus an opt-in weekly notify-only check (B18).
- **Not in 1.0:** OCR, scanner capture, watch folders, mobile, general photo editing; radial lens correction (B19, A-11). **In 1.0:** OS shell integration.
- **Private golden test set:** your real labelled photos never enter the public repo or public CI (B21).

## Architecture at a glance

```
   Tauri 2 shell (thin) + Svelte 5 / TypeScript UI
   draws proxies and tiles, SVG quad overlay, loupe, gesture layer, review grid
        |  opaque-ID commands and events          ^  tiles over a custom URI scheme
        v                                         |
   engine crate: job queue, batch + review states, EditState history (undo/redo),
                 safe-write pipeline + backup store (SQLite journal), CLI entry points
        |                     |                        |
   imgproc (Rust)        codecs (Rust)           worker-process pool (sandboxed)
   detect, warp,         JPEG, PNG, TIFF,        C parsers of untrusted input:
   enhance, encode       WebP, ... + caps        libheif + libde265 (and PDFium if ever)
```

- **Workspace:** `core` (EditState, history, errors), `codecs`, `imgproc`, `engine`, `cli`, `shell` (the only crate that knows Tauri), `xtask`. Models live in a separate repo; the private golden set in a private repo. Binaries: `AutoCrop` (GUI), `auto-crop` (CLI), `auto-crop-worker`.
- **Edits are parameters, not pixels.** Output is a pure function of (source, EditState), with N quads per image (multi-item) and a geometry that can be a homography or a dense grid (dewarp). Undo is a cursor over cheap snapshots.
- **Preview path:** Rust decodes everything and renders every real result; the webview only draws a 2-4 MP proxy plus 512 px tiles. During a drag it applies a geometry-only transform with no IPC; on release Rust renders and the UI cross-fades.
- **Detection:** fast classical detector plus a small learned corner net, fused and refined at full resolution, then a single high-quality resample. Calibrated confidence maps each result to Good (auto-saved), Check (held) or Failed (left untouched, "Draw crop" banner).
- **Enhancement:** classical flatten-then-threshold on a downscaled proxy, five sliders, colour kept by default.
- **Rust verdict:** right for the core, but mostly for memory-safe parsing, parallelism and packaging, not raw speed; HEIC, PDF and ONNX Runtime remain C/C++, and the GUI/webview, not the language, is the real risk (PLAN 1.8).

## Milestones

Sizes are rough part-time weeks (S 1-2, M 3-4, L 5-8, XL 9-16) and are unmeasured. Each milestone has an exit gate and a release; 0.x previews are public.

| Milestone | Title | Release | Size | Goal |
|---|---|---|---|---|
| M0 | Foundations and week-1 spikes | none | XL | Stand up the public repo, licence and supply-chain tooling and 3-OS CI, and retire the biggest unknowns with time-boxed spikes recorded as ADRs in `docs/adr/`. |
| M1 | Measurement harness and core engine skeleton | none | XL | Build the benchmark and accuracy harness, the synthetic corpus generator, the first private golden set (v0) and the UI-agnostic core data model and kernels, so every... |
| M2 | Classical pipeline, safe writes and CLI | v0.1.0 | L | Ship the first public preview: a headless CLI with a classical detect, crop, rotate, deskew and perspective pipeline and the complete overwrite-with-backup safety stack... |
| M3 | GUI alpha: single-image editor | v0.2.0 | XL | Ship the touch-first Windows desktop shell (Tauri 2.x + Svelte 5): open files and folders, show the M2 classical auto result, correct it with a quad editor and angle... |
| M4 | ML detection, orientation and calibrated confidence | v0.3.0 | XL | Deliver the accuracy promise: a hybrid learned plus classical detector with sub-pixel refinement, a 4-way orientation net and gated deskew, and a calibrated Good / Check... |
| M5 | Batch review workflow, backups panel and trust features | v0.4.0 | L | Deliver the batch-first workflow (B4, B5, B6): auto-process in the background, hold low-confidence items for review (flagged first), then Save all, backed by a... |
| M6 | HEIC/HEIF, sandboxed decoding and format read breadth | v0.5.0 | XL | Deliver HEIC/HEIF to JPG conversion with bundled libheif and libde265 (separate dynamic libraries) in a sandboxed worker pool, the Convert-only preset and Tier 1 read... |
| M7 | Enhancement and bilevel output | v0.6.0 | XL | Deliver the readability promise: a classical, CPU-only, pure-Rust flatten-then-threshold pipeline with a one-tap live before/after suggestion, four modes (Original,... |
| M8 | macOS preview | v0.7.0 | L | Ship the full app on macOS for Apple silicon: an ad-hoc-signed, unnotarised DMG and own Homebrew tap, bundled libheif and libde265 dylibs with an optional ImageIO HEIC... |
| M9 | Linux preview | v0.8.0 | XL | Bring Auto Crop to Linux with the WebKitGTK workarounds proven on a recorded distro, session and GPU matrix, a Landlock and seccomp sandbox around the decode worker, and... |
| M10 | Multi-item splitting | v0.9.0 | XL | Detect and split several photos or receipts lying on one flatbed scan into separate, straightened outputs: classical detection, every accepted item a quad that runs the... |
| M11 | Output formats, PDF and metadata policy | v0.10.0 | XL | Write every 1.0 export format (JPEG, PNG, WebP lossy and lossless, AVIF, JPEG XL, TIFF incl. multi-page and 1-bit CCITT G4, PDF single and multi-page) through one export... |
| M12 | Curved-page dewarp | v0.11.0 | XL | Deliver opt-in curved-page dewarping (B10): a UVDoc-class dense-grid model whose flow map is applied to the full-resolution image in one resample, suggested when a page... |
| M13 | Hardening, accessibility, i18n, docs, legal and 1.0 release | v1.0.0 | XL | Turn the feature-complete previews into a trustworthy 1.0: accessibility, i18n, security and fuzz hardening, docs, legal closure, verified shell integration and... |

**Honest estimate (PROVISIONAL, unmeasured).** Adding the size bands gives roughly 114-200 part-time weeks for everything through 1.0, which is **a multi-year project** (about 2.2-3.8 years at one part-time week per calendar week). The first Windows GUI alpha (M0-M3) is roughly 32-56 part-time weeks. AI assistance may shorten this, and the measured M0-M3 pace will replace the guess. This is why 1.0 is gated on measured numbers and why the cut order in A-4 exists.

Why this order: risk first (spikes M0, harness M1 before any GUI); a CLI-only Windows v0.1.0 ships early with the complete safe-write stack, because originals are overwritten by default; ML detection and calibrated confidence (M4) come before batch review and enhancement because confidence is what hold-for-review is built on; macOS and Linux previews sit mid-project so platform problems surface early; multi-item, output formats and dewarp come last because they are the largest and riskiest. More in [ROADMAP.md](ROADMAP.md#why-this-order).

## Main risks

Full register with likelihood, impact and owner milestone: [PLAN 8](docs/plan/08-oss-release-security-risks.md).

1. **Scope against one part-time maintainer.** Everything selected is in 1.0 (B10). Mitigation: gates on measured numbers, incremental previews, and the gate-miss rule A-4: a missed gate delays 1.0, and shipping a feature as experimental or moving it to 1.x is **your** decision, made with numbers. Suggested cut order if needed: dewarp, AVIF/JXL encoders, multi-item.
2. **Linux touch (WebKitGTK).** Known GPU and pinch problems. Mitigation: a week-1 spike on real hardware, env shims, best-effort stance, a UI-agnostic core and a Slint fallback with a precise trigger (B8, A-10).
3. **HEIC: patents and security.** Bundling libde265 carries HEVC patent exposure and a steady stream of libheif advisories. Mitigation: sandboxed worker pool, dynamic linking, advisory tracking within days, a $0 legal read before 1.0 (B12, A-5).
4. **Accuracy on real receipts and the golden set.** No receipt benchmark exists, no cleared pretrained weights exist, and labelling about 1,100 images is a large chore (A-7, A-8). Mitigation: fuse two detectors, calibrate, hold uncertain results, build the harness first.
5. **Data safety with overwrite by default.** Mitigation: the commit protocol (verify, back up, fsync, atomic replace, recovery journal), crash-injection tests, Store-build backups kept outside virtualised storage (PLAN 2.7).
6. **Unmeasured performance budgets.** All numbers are PROVISIONAL until the harness measures them on real laptops.
7. **Licence and provenance contamination** (GPL/AGPL crates, non-commercial weights). Mitigation: cargo-deny, a provenance log, an OSI-only weights policy.
8. **Distribution friction at $0.** Unsigned previews trigger SmartScreen and Gatekeeper warnings; Flathub's AI policy has flipped repeatedly. Mitigation: SignPath, own channels, honest instructions, Flathub only after 1.0.
9. **Hardware and testers.** Gates need a Mac, a Windows touch device and a flatbed scanner; they stay open until measured there (A-9).

## Decisions and vetoes for you

**Update 2026-10-01: the owner accepted all twelve as written** (A-1 CLI overwrites by default, A-3 Strict until proven, A-4 delay 1.0 and the owner decides were explicitly confirmed; see [decision log section I](docs/plan/00-decision-log.md)). They stay listed here for reference.

These were needed to resolve conflicts between documents without reopening B1-B21. **Each is an assumption the plan follows until you say otherwise** (wording in [decision log section H](docs/plan/00-decision-log.md) and [PLAN 1.7](docs/plan/01-vision-decisions.md)).

| ID | Assumption (default the plan follows) |
|---|---|
| A-1 | The CLI follows B3: it overwrites in place after a verified backup unless `--output`/`--suffix` is given; one-time notice on stderr |
| A-2 | A 1-to-N split replaces the scan (the scan moves to Backups); multi-frame sources and formats this build cannot write back are never replaced |
| A-3 | Confident results save after the whole batch is triaged; previews default to Strict until the golden gate shows Balanced meets its 1% target |
| A-4 | A missed exit gate delays 1.0; experimental or 1.x needs your decision with measured numbers |
| A-5 | Official builds always bundle libde265; a $0 legal read is due before the first published HEVC build if practicable, no later than 1.0 |
| A-6 | The Store MSIX build is verified early by a hidden dry run; any fee or Store-only no-HEVC build needs your decision |
| A-7 | Only OSI-licensed model lineage ships; ImageNet-initialised, DocQuadNet-derived or UVDoc weights need your exception |
| A-8 | The golden set grows to 800 locked images (plus a ~300 dev tier) before 1.0 and never leaves your encrypted disk |
| A-9 | Gates that need a Mac, a Windows touch device or a flatbed scanner stay open until measured on that hardware |
| A-10 | "Cannot hold 60 fps" on Linux means median frame above 22 ms after shims; switching to Slint needs your waiver of RTL layout and a licence decision |
| A-11 | No radial lens correction in 1.0; Orca is best-effort; crop margin is per detection route |
| A-12 | 1.0 needs two 14-day release-candidate windows; `auto-crop` is reserved on crates.io at the first release |

Also unasked and open to veto (defaults C1-C7 and P1-P3 in PLAN 1.7): hardware floor (x86-64 with AVX2, 8 GB, images up to 100 MP), HEIC to JPG colour and metadata defaults, desktop-only, English-first with i18n.

## What happens next

1. **You confirm the milestone order** and say "start M0" (ROADMAP item P.05). A-1..A-12 are already accepted.
2. **Then M0 starts:** create the public repo `WelFedTed/auto-crop` with licence, CI on all three OSes and dependency policy, and run the week-1 spikes (GUI stack, inference runtime, warp kernels, HEIC on real files, sandboxing). Nothing is built before you say so.

## How this plan was made

Requirements interview (21 decisions) followed by nine research studies, each independently fact-checked by a second pass; eight design documents and fourteen milestone checklists drafted in parallel, then reviewed by twelve reviewers (512 findings) and reconciled into one set of canonical values before the final edits. The process is AI-assisted throughout, and the repository will say so openly (B17). Research is a snapshot from 2026-09-30: versions, licences, prices and policies must be re-verified before they are relied on.
