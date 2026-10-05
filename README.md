# Auto Crop

**The offline batch fixer for scans, receipts and photos.**

Auto Crop is a free, open-source desktop app for Windows, macOS and Linux. Drop in images or a whole folder and it finds the page, receipt or photo, crops it, rotates it, removes skew and perspective distortion, and offers to whiten the paper and sharpen the text of black-and-white documents. It also converts formats, first of all HEIC/HEIF to JPG. Results land in a review grid where anything the app is unsure about comes first, and every change can be adjusted with large touch-friendly handles and undone.

Everything runs locally: no account, no upload, no telemetry.

> **Status: pre-alpha.** There is no release or download yet. A first, owner-directed slice of the desktop app can be built from source for testing (see [Try the early GUI](#try-the-early-gui)): it opens JPEG and PNG, finds the page with a classical detector, lets you adjust the crop, saves with a verified automatic backup, and restores originals. Most of the plan below is not built.

## Try the early GUI

Windows 10/11 (WebView2 is already part of Windows 11), Rust (see `rust-toolchain.toml`) and Node.js LTS are needed.

```sh
cd ui && npm ci && npm run build && cd ..
cargo run -p auto-crop-shell --release --features gui,custom-protocol
```

Click **Try sample images** to get synthetic receipts and documents with a spread of difficulty, or open your own JPG or PNG files or a folder (or drop them on the window). Nothing leaves your computer. Originals are replaced only after a verified backup; **Save as copy** (Settings or Home) leaves them alone, and **Backups** restores them. To work on the interface without the Rust side, run `npm --prefix ui run dev` in a browser: it uses a built-in mock.

Known limits of this slice: JPEG and PNG only (no HEIC yet), EXIF other than orientation is dropped on save, the confidence score is an uncalibrated heuristic, no enhancement, no multi-item splitting, Windows is the only tested platform. The detector is a simple baseline; expect to adjust some crops by hand.

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

A **pre-release** (0.0.2, Windows 10/11 x64 only) is on the [Releases page](https://github.com/WelFedTed/auto-crop/releases): download `AutoCrop-0.0.2-windows-x64.zip`, check it against `SHA256SUMS`, unzip, run `AutoCrop.exe`. It needs the Microsoft Edge WebView2 runtime (part of Windows 11 and current Windows 10). It is an early test build, see [CHANGELOG.md](CHANGELOG.md) for what it does and does not do. Planned: a Windows installer first, then macOS (Apple silicon) and Linux (AppImage, deb, rpm, Flatpak).

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
