# Auto Crop

**The offline batch fixer for scans, receipts and photos.**

Auto Crop is a free, open-source desktop app for Windows, macOS and Linux. Drop in images or a whole folder and it finds the page, receipt or photo, crops it, rotates it, removes skew and perspective distortion, and offers to whiten the paper and sharpen the text of black-and-white documents. It also converts formats, first of all HEIC/HEIF to JPG. Results land in a review grid where anything the app is unsure about comes first, and every change can be adjusted with large touch-friendly handles and undone.

Everything runs locally: no account, no upload, no telemetry.

> **Status: planning stage. There is no code and no download yet.** The plan is complete and the first milestone (M0) has not started. Watch or star the repo to follow progress.

## Planned features

- **Auto crop, rotate, deskew and perspective correction** for phone photos of documents and receipts, flatbed scans, multi-photo scans (split into separate files) and ordinary photos; curved-page dewarping.
- **Readability enhancement** for black-and-white documents: whiter paper, crisper text, optional black and white. Suggested with a live before/after preview, never forced.
- **Honest confidence:** results the app is unsure about are held for your review instead of being written.
- **Preview and manual adjustment:** draggable corner handles with a magnifier, rotation dial, before/after, undo and redo, designed for touch screens as well as mouse and keyboard.
- **Safe overwrite by default:** originals are replaced only after a verified automatic backup, and "Restore original" works even after saving or restarting.
- **Format conversion**, especially HEIC/HEIF to JPG, and exports to JPEG, PNG, WebP, AVIF, TIFF (including 1-bit CCITT G4), PDF and JPEG XL.
- **Headless command-line tool** that shares the same engine, for scripting and batch jobs.
- **Fast:** speed and accuracy targets are measured by a benchmark harness before any claim is made.

"All image formats" means a documented support matrix (all common formats plus a long tail), not a literal promise.

## Project documents

| Document | What it is |
|---|---|
| [PLAN.md](PLAN.md) | The plan in about ten minutes: scope, decisions, architecture, milestones, risks |
| [ROADMAP.md](ROADMAP.md) | The living checklist: every task, grouped by milestone and release |
| [docs/plan/](docs/plan/) | Decision log and eight design documents |
| [docs/research/](docs/research/README.md) | Research and fact-check snapshots behind the decisions |

## Install

Nothing to install yet. Planned: a Windows installer and portable zip first (the first public previews are Windows-only), then macOS (Apple silicon) and Linux (AppImage, deb, rpm, Flatpak). Releases will appear on the [Releases page](https://github.com/WelFedTed/auto-crop/releases).

Early builds will not be code-signed (the project has a $0 budget), so Windows SmartScreen and macOS Gatekeeper will show warnings. Step-by-step instructions and checksums will accompany every release.

## Privacy

Auto Crop makes no network requests by default. There is no telemetry. A manual "Check for updates" and an opt-in weekly update notification are the only planned network uses. Your images never leave your machine.

## HEIC and patents

Reading HEIC/HEIF files requires an HEVC decoder. Official builds are planned to bundle libde265 (with a decode-only libheif) as separate, replaceable libraries. HEVC is covered by patents in some jurisdictions, and this is not legal advice; a legal read is planned before 1.0. Distro packagers can build without HEVC support.

## AI assistance

This project is planned and written with AI assistance (Claude). All output is reviewed by the maintainer. No code is knowingly copied from GPL, AGPL or non-commercial projects; algorithms derived from such work are reimplemented clean-room from papers.

## Contributing

The project is in the planning stage and is not yet ready for code contributions. Feedback on the [plan](PLAN.md) is welcome through issues. When development starts, contributions will use the Developer Certificate of Origin (`git commit -s`); a full CONTRIBUTING guide will follow in milestone M0.

## Licence

Dual-licensed under either of

- [MIT licence](LICENSE-MIT)
- [Apache License, Version 2.0](LICENSE-APACHE)

at your option. Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this project, as defined in the Apache-2.0 licence, is dual-licensed as above, without any additional terms or conditions.
