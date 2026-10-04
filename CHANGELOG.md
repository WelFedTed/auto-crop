# Changelog

## Unreleased

- Save as copy of a scan with several items no longer waits for the split to be accepted: a copy destroys nothing, so it is written at once; replacing the original stays held until the split is accepted.
- One error code, `NOT_REPLACEABLE`, for a source that is never replaced in place (multi-page TIFF, a format with no writer), on the single-item and the multi-item path; the reason is in the notice. The single-item path used to answer `UNSUPPORTED_OUTPUT`.
- A Windows package with HEIC, HEIF and AVIF input can be built in CI (`package-windows.yml`); the app and the CLI point libheif at the `libheif` folder beside the executable. HEIC and AVIF are parsed in-process (the sandboxed worker pool is not built yet).

## 0.0.1 (2026-10-02), pre-release, Windows only

The first public build: an early, owner-directed test build of the desktop app. It is **not** the
CLI-only v0.1.0 that [ROADMAP.md](ROADMAP.md) milestone M2 describes (that version number is still
unused), and none of M2's gates (G1) have been measured. Treat it as a preview to try, not as
something to trust with irreplaceable files.

### What it does

- Opens JPEG and PNG files, folders (with or without subfolders) and drag-and-drop. "Try sample
  images" creates synthetic receipts and documents with a spread of difficulty.
- Finds the page or receipt with a classical detector, shows a review grid (Needs review, All, Edited,
  Skipped, Failed, Saved) with Good, Check and Failed tiers and the reason an item was held, and lets you
  adjust the crop with corner, edge and move handles, a 0.1 degree ruler, 90 degree turns, compare,
  zoom and undo/redo.
- Saves by replacing the original **after** a verified automatic backup (the default), or as a copy in an
  `AutoCrop` folder. **Backups** restores originals, per file or per run, byte for byte, including after
  closing the app. A file edited since saving asks first (restore as copy, or replace anyway, which keeps
  the edited file).
- Everything runs locally. No network access, no telemetry, no account.

### Known limits

- JPEG and PNG only: no HEIC, TIFF, WebP or other formats. EXIF other than the orientation is dropped
  when a file is saved, and JPEG output is always re-encoded (quality 92).
- The confidence score is an **uncalibrated heuristic**. The detector is a baseline checked on synthetic
  images only; no accuracy has been measured on real photos, so expect to fix some crops by hand.
- The save path follows PLAN 2.7 in outline (temp file, verify, backup, re-stat, swap) but lacks the
  journal, crash recovery, `ReplaceFileW` and a free-space check (ROADMAP M2.83). If the app is killed in
  the middle of a save, an orphan `.autocrop-*.tmp` file can be left beside the original, which stays intact.
- No enhancement, multi-item splitting, Convert, command-line tool or touch-specific testing. Windows 10/11
  x64 only; macOS and Linux are not built or tested for this release.
- Strings are English only and not yet externalised (ROADMAP M3.51).

### Verification

- 125 automated tests pass on Windows, macOS and Ubuntu CI (core, image kernels, detector, codecs, engine
  save and restore invariants including a source changed after opening, backup byte-equality and restore
  after re-save). Interaction budgets, real-device touch behaviour and a clean-VM install were **not**
  measured.
- Built on the maintainer's machine, **unsigned** (Windows SmartScreen will warn), with no build
  attestation or SBOM. Check the download against `SHA256SUMS`.
