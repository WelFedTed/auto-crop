# Changelog

## 0.0.4 (2026-10-14), pre-release, Windows only

A fourth early test build. It is still **not** the CLI-only v0.1.0 of [ROADMAP.md](ROADMAP.md) milestone M2,
the accuracy gates have not been measured on a proper real-photo set, and the confidence score is an
uncalibrated heuristic. Treat it as a preview, not as something to trust with irreplaceable files.

### What changed

- **Convert formats.** A new **Convert** screen on Home (and `auto-crop convert --to jpg|png` on the command
  line): drop files or folders, choose JPG or PNG, the quality and the colour handling, and choose whether the
  originals are replaced (after a verified backup, as for crops), kept, or the results go to a folder. A
  preview says what will happen to each file, then progress and a per-file result list; Restore returns the
  original and handles the converted file. Anything the build can decode converts: HEIC, HEIF and AVIF with the
  package below, plus TIFF, WebP, BMP, PNG and JPEG. A source with its own colour profile is converted to sRGB by
  default (tested with Display P3; or kept as it is on request), EXIF is kept with the orientation applied once, GPS can be removed,
  alpha is flattened on white for JPEG, and a file already in the target format is skipped. `AutoCrop.exe
  --convert <paths>` opens the app on the Convert list.
- **Enhancement for documents and receipts.** An Enhance panel in the editor with Original, Clean, Document
  (black on white) and Receipt (faded thermal paper) and a strength slider, with a live preview, a hold-to-compare
  and a split view. The app may *suggest* Clean for pages that look like documents (a gentle banner with Preview,
  Apply and Not now); nothing is ever applied on its own in the app. For multi-item scans you can set it for one
  item or the whole scan, and the grid has "Enhance all suggested" as one undo. On synthetic receipts and
  prose pages, measured with Tesseract, the average reading error fell from about 13-16% to under 0.1% (shaded
  and shadowed pages from 20-36% to about 0). Clean never read worse; the black-and-white modes can lose faint
  marks, so keep Original if unsure. **No real photograph was used for this evidence.** PNG is recommended for
  Document and Receipt output (they write a single grey channel and drop the colour profile, with a notice).
  Command line: `process --enhance original|clean|document|receipt|auto` (`auto` applies only Clean, and only
  where the page looks like a document) and `--enhance-strength`.
- **Suggest curves.** In Curved mode a **Suggest curves** button fits the four edges of a page to the visible
  paper edge, shows the result as a preview, and applies it only when you say so (one undo step; the page stays
  held for review until you accept it). On synthetic pages the fitted edges were 1.5 px from the truth on
  average against 2.0 px for the straight quad (strongly bent pages 2.1 against 4.3 px); on nearly straight
  pages it is slightly worse, and on the one real curved receipt we have, the fit looked close to the paper edge by eye
  (there is no measurement beyond that). There is no automatic "this page looks curved" badge: the flag was not reliable on real
  receipts. A first-use sheet explains what Curved mode does and does not fix.
- **A more honest page finder.** Tested against hand-drawn labels on 17 real pictures, the previous build
  accepted 4 results as Good and 3 of them were wrong. Now a small box inside a larger page of the same colour
  (a barcode on a receipt that fills the frame) is never accepted, a page that fills the whole frame comes back
  as a held full-frame candidate, a quad whose edges have the same colour on both sides gets no credit (wood
  grain), and a picture sitting inside a border of the page's own colour is held (`DETECTORS_DISAGREE`). On
  those 17 pictures: mean IoU 0.62 to 0.69, silent failures 3 to 0, held 14 of 17. This is a small sample, the
  rules were developed on synthetic reproductions, and hand-held receipts and some long receipts are still
  wrong (held, not written). A print with a white mat is held: the finder keeps the paper edge, while the
  owner's label is the picture inside.
- Edits are saved in format **v4** (v1, v2 and v3 edits migrate without loss). A build older than 0.0.4 cannot
  read edits saved by this one.

### The package with HEIC, HEIF and AVIF

`AutoCrop-0.0.4-windows-x64.zip` (the only download) contains the app, the command-line tool, three DLLs and a
`libheif` plugin folder. HEIC and AVIF files are parsed **inside the app** (the sandboxed helper process of the
plan does not exist yet), so use this build for your own files only. It is unsigned (Windows SmartScreen will
warn). HEIC and AVIF files are open-only for crops: use Save as copy, or Convert to JPG or PNG.

### Known limits

- Real touch input, screen readers, the native file dialog and drag-and-drop were not tested for the new
  screens; the window was driven through WebView2 remote debugging on synthetic images only.
- There is no library.db (the backup manifests are the index), file identity and extended attributes are not
  carried over, and power loss is not modelled.
- The classical detector fails about 40% of the time on a second, independent set of synthetic scenes and gets
  hand-held receipts wrong. A learned detector was studied (small net, GPU-trained on synthetic data) and is
  **not** in this build: it beat the classical one on synthetic data and was only slightly better, not
  significantly, on the 17 real pictures.
- Curves are bent by hand or by the on-demand suggestion; wrinkles inside a page are not flattened.
- No `--preset`, no 1-bit output, English only, Windows 10/11 x64 only.

### Verification

- Automated tests run on Windows, macOS and Ubuntu CI (core, kernels, detector, codecs, engine save and restore
  invariants including fault injection and the kill loop, curved-page and enhancement oracles, 228 UI tests,
  fuzz regressions, OCR checks for enhancement).
- Built in GitHub Actions, **unsigned**, with no build attestation or SBOM. Check the download against
  `SHA256SUMS`.

## 0.0.3 (2026-10-11), pre-release, Windows only

A third early test build. It is still **not** the CLI-only v0.1.0 of [ROADMAP.md](ROADMAP.md) milestone M2,
the accuracy gates have not been measured on real photos, and the confidence score is an uncalibrated
heuristic. Treat it as a preview, not as something to trust with irreplaceable files.

### What changed

- **Curved pages.** A crop can be switched from Straight to **Curved**: each edge gets a handle you drag to bend it
  (double-click or press-and-hold adds a point, Delete removes one, arrow keys nudge), and the page is flattened
  from the four edge curves with one resample, with a live preview and a Flattened / Straight compare toggle. It
  fixes bowed edges and perspective; wrinkles inside the page stay. A curved page is held for review until you
  accept it (Save as copy works at once, Replace original only after Accept) and never takes the lossless JPEG
  path. "Try sample images" has a curved receipt. On synthetic pages the flattened result matched the flat
  original at SSIM 0.97 to 0.99. Corners, merge and cut are locked while a page is curved (use "Back to
  straight" first). Nothing proposes curves by itself yet.
- **Screens for scans with several photos or receipts.** An outline and an order chip for every item, a chip bar
  (merge, cut, reorder by drag, Alt+Arrow or buttons, remove and restore), "Add as item", a "found N items,
  held for review" banner with Accept split, split settings (Auto / Always / Never, Photos / Receipts, and an
  Experimental auto-save toggle), a Backups view that lists derived files with Keep / Remove on restore, and
  plain-language wording for all 34 error codes. Replace original needs the split to be accepted first; Save
  as copy writes one file per item at once.
- **Saving a single image follows the full safe-write protocol** (it was a shorter early version before). The
  save writes a journal first, backs the original up (a reflink, a hardlink or a re-checked copy), writes the new
  file beside the old one, re-reads and re-decodes it, then swaps it in (`ReplaceFileW` on Windows, retried when
  an antivirus or indexer holds the file). If the program is killed at any moment, the next start finishes the
  save or puts the original back, and removes stray temporary files. The free-space check runs before anything
  is written, a read-only file or a cloud placeholder is refused without being read or changed, and a file
  another program edited meanwhile is never overwritten. A kill-at-random-instants test (500 kills on every run,
  1,000 nightly) and a restore matrix check this. See [docs/safety-threat-model.md](docs/safety-threat-model.md).
- **A source that cannot be held whole in 8-bit RGB is never replaced in place** (16-bit, alpha, CMYK, animated
  PNG): it opens, and "Save as copy" works. Before, an in-place save could have changed such a file's content
  silently.
- **JPEG output is written by the codecs crate**, not the stand-in encoder: the quality follows the source's own
  (Balanced is the source's quality plus 5, between 80 and 95), EXIF is kept (Orientation is set to 1, the size
  updated, the thumbnail removed), the ICC profile and pixel density are kept byte for byte, and
  `strip_location` removes GPS, XMP and IPTC. A crop that falls on the JPEG block grid, and turns and flips, are
  done **without re-encoding** (safe-Rust transform; libjpeg-turbo with the `turbojpeg` feature); the picture
  then does not lose a generation, and may be up to 16 pixels larger than asked.
- **BMP files open.** The engine can replace a BMP by its PNG (a HEIC by its JPEG in the HEIC build) after a
  verified backup, but no screen or command offers that yet.
- **The `auto-crop` command line** is in the package: `process` crops and straightens images and, with no output
  option, replaces the originals after a verified backup; `--output`, `--suffix` and `--copy` write new files
  instead, `--dry-run` writes nothing, `--manifest` and `--json` give a versioned JSON run manifest. Results below
  the cut-off (`--triage strict|balanced|aggressive`, default strict), failed detections and scans with several
  items that were not accepted (`--accept-splits`) are held: not written, exit code 4. `analyze` reports the
  detection without writing, `render` writes one crop without touching the source, `restore` and `backups list |
  show | purge` use the same backup store as the app, and `doctor` checks the machine. Ctrl+C stops between files
  (exit 130). See [docs/cli.md](docs/cli.md).
- Edits are saved in format **v3** (v1 and v2 edits migrate without loss). A build older than 0.0.3 cannot read
  edits saved by this one.
- Decoding fixes found by fuzzing: a colour profile above its size limit, an unreadable saved state (an absurdly
  large angle), and a size overflow in a HEIF header.

### The package with HEIC, HEIF and AVIF

As in 0.0.2, `AutoCrop-0.0.3-windows-x64.zip` contains the app, the command-line tool, three DLLs and a
`libheif` plugin folder. HEIC and AVIF files are parsed **inside the app** (the sandboxed helper process of
the plan does not exist yet), so use this build for your own files only. It is unsigned (Windows SmartScreen
will warn). HEIC and AVIF files are open-only: Save as copy writes PNG (JPEG for HEIC).

### Known limits

- There is no library.db (the backup manifests are the index), file identity and extended attributes are not
  carried over, and power loss (data never written to disk) is not modelled.
- The classical detector still gets hand-held receipts and some long receipts wrong (they are held, not written);
  on a second, independent set of synthetic scenes it fails about 40% of the time. Real-photo accuracy has not been
  measured beyond a small indicative set. Curved edges are bent by hand.
- Real touch input, screen readers, the native file dialog and drag-and-drop were not tested for the new screens;
  the window was driven through WebView2 remote debugging on synthetic images.
- No enhancement, no Convert command, no `--preset`. Windows 10/11 x64 only. English only.

### Verification

- Automated tests run on Windows, macOS and Ubuntu CI (core, kernels, detector, codecs, engine save and restore
  invariants including fault injection and the kill loop, curved-page oracles, 147 UI tests, fuzz regressions).
- Built in GitHub Actions, **unsigned**, with no build attestation or SBOM. Check the download against
  `SHA256SUMS`.

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
