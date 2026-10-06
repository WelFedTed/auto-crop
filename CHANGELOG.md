# Changelog

## Unreleased

- The package README no longer calls itself a private build once it is published as a pre-release.
- **The `auto-crop` command line** (not in a release package yet; build it from source). `process` crops and
  straightens images and, with no output option, replaces the originals after a verified backup; `--output`,
  `--suffix` and `--copy` write new files instead, `--dry-run` writes nothing, `--manifest` and `--json` give a
  versioned JSON run manifest. Results below the cut-off (`--triage strict|balanced|aggressive`, default strict),
  failed detections and scans with several items that were not accepted (`--accept-splits`) are held: not written,
  exit code 4. `analyze` reports the detection and the confidence without writing, `render` writes one crop to a
  file without touching the source, `restore` (by file, backup id or run) and `backups list | show | purge`
  work on the same backup store as the app, and `doctor` checks the machine (AVX2 floor, memory, decoders, the
  store). Ctrl+C stops cleanly (exit 130). See [docs/cli.md](docs/cli.md). The engine gains a few small additive
  calls for it (explicit settings that are never persisted, an engine without start-up housekeeping, saves
  grouped into a named run, a JPEG quality and a crop margin per run, removing one backup).

- **Saving one image now follows the full safe-write protocol** (it was a shorter early version in 0.0.1 and
  0.0.2). The save writes a journal first, backs the original up (a reflink, a hardlink or a re-checked copy),
  writes the new file beside the old one, re-reads and re-decodes it, then swaps it in (`ReplaceFileW` on
  Windows, retried when an antivirus or indexer holds the file). If the program is killed at any moment, the next
  start finishes the save or puts the original back, and removes stray temporary files. The free-space check
  runs before anything is written, a read-only file or a cloud placeholder is refused without being read or
  changed, and a file another program edited meanwhile is never overwritten. A kill-at-random-instants test
  (500 kills on every run, 1,000 nightly) and a restore matrix check this. See
  [docs/safety-threat-model.md](docs/safety-threat-model.md).
- **JPEG output is written by the codecs crate**, not the stand-in encoder: the quality follows the source's
  own (Balanced is the source's quality plus 5, between 80 and 95), EXIF is kept (Orientation is set to 1, the
  size updated, the thumbnail removed), the ICC profile and pixel density are kept byte for byte, and
  `strip_location` removes GPS, XMP and IPTC. A crop that falls on the JPEG block grid, and turns and flips, are
  done **without re-encoding** (the lossless path, with the safe-Rust transform; libjpeg-turbo with the
  `turbojpeg` feature); the picture then does not lose a generation, and may be up to 16 pixels larger than
  asked.
- **BMP files open**, and `Engine::convert_items` replaces a BMP by its PNG (a HEIC by its JPEG with the
  `heif` build) after a verified backup; Restore returns the original and moves the converted file into the
  backup.
- Known gaps in this part: there is no `library.db` (the backup manifests are the index), file identity and
  extended attributes are not carried over, and power loss (data never written to disk) is not modelled.

## 0.0.2 (2026-10-05), pre-release, Windows only

A second early test build. Like 0.0.1 it is **not** the CLI-only v0.1.0 of [ROADMAP.md](ROADMAP.md)
milestone M2, none of the accuracy gates has been measured on real photos, and the confidence score is
still an uncalibrated heuristic. Treat it as a preview, not as something to trust with irreplaceable files.

### What changed

- **A better page finder.** It now handles low-contrast pages far better and finds long, thin receipts from
  their two long edges. On synthetic test images the failure rate fell from about 30% to under 2% for the
  earlier test set, but on a second, independent set of synthetic scenes it still fails about 40% of the
  time (long and strip receipts, partial frames, white desks). On a small hand-labelled set of real photos,
  5 of 15 crops are within 0.9 IoU. Hand-held receipts and some long receipts are still held with a wrong
  crop. A wrongly confident "Good" was not seen, but that is a small sample.
- **More input formats.** WebP and TIFF open in the standard build. The separate package (below) also opens
  **HEIC, HEIF and AVIF**. These formats are never replaced in place, because there is no writer for them
  yet: use "Save as copy" (written as PNG, or JPEG for HEIC) with the colour profile kept.
- **Scans with several photos or receipts** are detected by the engine, and are **held for review**; nothing
  is written until you accept the split. Save as copy writes one file per item (`name_01`, `name_02`, ...)
  at once, because a copy destroys nothing. The screens for choosing, editing and accepting items are
  **not built yet**: the current window shows only the first item of such a scan, and an in-place save of a
  held scan is refused with a generic message. All items are written or none, with crash recovery, and
  Restore original returns the scan byte for byte.
- One error code, `NOT_REPLACEABLE`, for a source that is never replaced in place (multi-page TIFF, a format
  with no writer), with the reason in the notice.
- **Stricter, safer decoding.** Fuzzing found and fixed: a crash on a rare kind of JPEG (4:2:0 with one scan
  per colour component is now refused as unsupported), an edit state that could be written but not read
  back, a size overflow in a HEIF header and a colour profile that could exceed its size limit. Truncated
  JPEGs and files smaller than their header claims are now refused as corrupt instead of being decoded
  with grey fill.
- Saved edits use a new, versioned format (v2) that old edits migrate into without loss. A build older than
  0.0.2 cannot read edits saved by this one.

### The package with HEIC, HEIF and AVIF

`AutoCrop-0.0.2-windows-x64.zip` contains the app, the command-line tool, three DLLs and a `libheif` plugin
folder. HEIC and AVIF files are parsed **inside the app**, because the sandboxed helper process of the plan
does not exist yet: use this build for your own files only. It is unsigned (Windows SmartScreen will warn).

### Known limits

- No screens for multi-item scans, no enhancement, no Convert, no command-line batch mode, no
  touch-specific testing. Windows 10/11 x64 only; macOS and Linux are not built for this release.
- Real-photo accuracy has not been measured beyond the small indicative set above.
- Strings are English only and not yet externalised.

### Verification

- Automated tests run on Windows, macOS and Ubuntu CI (core, image kernels, detector, codecs, engine save and
  restore invariants including fault injection at every step of the multi-item save, fuzz regressions).
  Interaction budgets, real-device touch behaviour and a clean-VM install were **not** measured, and the
  window itself was only checked to start.
- Built in GitHub Actions, **unsigned**, with no build attestation or SBOM. Check the download against
  `SHA256SUMS`.

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
