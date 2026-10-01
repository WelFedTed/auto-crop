> Auto Crop design, part 6 of 8 | [PLAN.md](../../PLAN.md) | [Decision log](00-decision-log.md) | [ROADMAP.md](../../ROADMAP.md)  
> Planning draft, 2026-10-01. Numbers marked PROVISIONAL are unmeasured estimates; decisions B1-B21 and the assumptions A-1..A-12 live in the decision log and in section 1.

# 6. UX and touch design

This section specifies flows, screens, input handling, accessibility and the usability test plan. Wireframes, the gesture map and the accessibility model are toolkit-agnostic, so the Slint fallback (B8) would re-implement rather than redesign. Implementation notes assume Tauri 2.x with Svelte 5 (B8). A dagger (†) marks a PROVISIONAL number: an unmeasured starting value to tune in the tests of 6.14. A "run" is one batch session; "held" means not written to disk. This section owns the UI copy and the interaction timings: text for hold reasons, errors and notices is looked up by code (6.7). The owner-decision assumptions A-1 to A-12 are listed in 01 §1.7; 6.15 names the ones this section relies on.

## 6.1 Principles and the input guarantee

- **Triage first.** The app works; the person reviews what it is unsure of (B4, B5). The best vendor figure is 85% correct detection on phone captures ([Genius Scan 2024](https://blog.thegrizzlylabs.com/2024/10/document-detection.html), method undisclosed), so our 8-10% flagged target (B6) must be verified on receipts.
- **Overwrite is acceptable only because it is recoverable** (B3): a verified backup precedes every write, and Restore original is one click away and survives closing the app.
- **Auto result first, manual second.** Review mode (swipe, no handles) and Adjust mode (handles, swipe off) avoid gesture conflicts.
- **Gestures accelerate, never gate.** Every drag, pinch or twist has a button, number or key path (WCAG [2.5.1](https://www.w3.org/WAI/WCAG22/Understanding/pointer-gestures.html), [2.5.7](https://www.w3.org/WAI/WCAG22/Understanding/dragging-movements.html)). This is also what lets Linux touch degrade gracefully (6.4.3).
- **One pixel path** (D1): the webview draws the image, overlay, loupe and gesture layer; Rust renders every real result.
- **Honest confidence, no telemetry** (B18): three tiers with plain reasons, never a raw score.

| Input (B8, B9) | Windows 10/11, WebView2 | macOS 12+, WKWebView | Linux, WebKitGTK |
|---|---|---|---|
| Mouse, keyboard | Guaranteed | Guaranteed | Guaranteed |
| Trackpad pinch, rotate | Pinch as Ctrl+wheel; no rotate event | Guaranteed via `gesture*` events | Best-effort: GTK may claim pinch ([wry#544](https://github.com/tauri-apps/wry/issues/544), [tauri#13115](https://github.com/tauri-apps/tauri/issues/13115), open) |
| Touchscreen, pen | Guaranteed | Not applicable; trackpad validated instead | Best-effort, measured in the week-1 spike |

## 6.2 Primary user flows

The flow is Home, processing, batch review (flagged first), Review or Adjust per image, Save all, then the saved summary with Backups and Restore behind it. **Convert only** goes from Home straight to the Convert dialog. Nothing is written for an item until it is Good and auto-saved (after the whole batch is triaged, 6.2.4), or the person resolves it:

| Item state | Written to disk? |
|---|---|
| Good, unedited | Yes: auto-saved after analysis (default), or on Save all |
| Check or Failed, unreviewed | No (held); original untouched |
| Check accepted or edited; Failed with crop drawn | Yes, on Save all |
| Skipped | No; can be un-skipped |
| Not replaceable (6.2.6) | No; source byte-identical; a copy only if the person opts in per file |

### 6.2.1 First run and empty state

No wizard, account, permission prompt or network call. Home (wireframe A) always shows three safety elements: a **Saving** row ("Replace originals, backup kept 30 days") with a **Save as copy** switch, off by default (B3); a **Backups** button; and a **How saving works** card, expanded on first run. The switch is one global setting mirrored on Home, the review bar, the Convert dialog and Settings, so what is shown is what happens. The card reads: "Auto Crop replaces your originals with the corrected version. Each original is copied to Backups first and kept for 30 days; you can restore it even after closing the app." Actions: **Got it**, **Save copies instead**, **Where are backups?**.

Cards get dismissed unread, so a **first-write confirmation sheet** appears once, right before the first real write, showing file count, mode and backup size against free space, with **Replace originals** or **Save copies instead**. Afterwards the review header carries "Originals backed up · Restore anytime".

**Where backups live.** In the one per-user store that the GUI and the CLI share (02 §2.7): `%LOCALAPPDATA%\AutoCrop\backups\<ulid>\` on Windows, `~/Library/Application Support/AutoCrop` on macOS and `$XDG_DATA_HOME/auto-crop` on Linux, with one `<ulid>` folder per backed-up file and runs grouped only in the Backups panel. The shell passes the engine's `AppPaths`, so there is no second, GUI-only store. The Store build cannot keep backups or the library in virtualised package data, which uninstalling deletes, so it uses a real folder in the user's profile fixed at first run (02 §2.7). Hardlink or reflink backups only work on the same volume (02 §2.7); otherwise a backup is a full copy (300 JPEGs of 4 MB take about 1.2 GB), so the UI shows size estimates.

Also on Home: **Try sample images** (synthetic per B21, copied to a temp folder, with one deliberately hard image so a Check tile appears; this replaces a tutorial), **Resume previous session?**, and a line saying the app never connects to the internet unless asked (builds from the Store, Flatpak, winget or a distro contain no updater and update through their own channel). A remembered **preset** sets defaults only: *Documents and receipts* (default), *Photos*, *Scans with several items*, *Convert only* (the CLI's `--preset` names are `receipt` and `document`, `photo`, `flatbed` and `convert-only`).

### 6.2.2 Open, drop, folders

Entry points: drop on the window or taskbar/Dock icon, **Open files**, **Open folder** (big buttons, since drag-drop is impractical on touch), **Paste image**, and OS "Open with" (6.11). A dropped folder shows a one-line strip, "312 images in 4 folders · Include subfolders [on]", plus edge cases: symlink loops skipped and counted; online-only cloud placeholders counted apart ("22 online-only [Download] [Skip]", never downloaded silently); bad files sent to the Issues tray (6.7). Paths inside a detected sync root (OneDrive, iCloud Drive, Dropbox) add "This folder syncs to the cloud. Replacing or converting files changes the cloud copy too. [Save copies instead] [Continue]"; the default stays Replace (B3).

### 6.2.3 Auto-process

Processing starts at once, visible items first, thumbnails streaming in, on the fixed proxy pyramid of D1. HEIC shows its embedded thumbnail as a draft first. One progress bar shows count, measured rate and ETA, with Pause and Cancel. The throughput targets (CLI at least 4 images/s on 6 cores, GUI batch at least 3†) are PROVISIONAL, so the UI reports measured speed and promises no duration.

### 6.2.4 Batch review, flagged first (B4, B5)

The grid (wireframe B) shows processed results, sorted **Failed, then Check (lowest confidence first), then Good**, and opens on **Needs review**, or on **All** with "All 312 look good" when nothing is flagged.

- **Filters:** Needs review, All, Edited, Skipped, Failed, Saved. **Sort:** Confidence (default), Name, Modified.
- **Tile:** cropped thumbnail; icon-plus-word badge; one reason line for Check and Failed (6.7); dots for Edited, Skipped, Saved. Failed tiles show the *original* under an amber **Draw crop** banner.
- **Selection** follows the gesture map (6.4.1). Bulk actions: Accept, Skip, Rotate 90, Apply enhancement, Reset to auto, each with the scope This image, Selected or All. **Accept with scope All covers Good items only.** Accepting Check items needs an explicit selection and a confirmation that names the count ("Accept 23 items you have not reviewed?"); they are recorded as accepted by the person, never as auto-approved.
- **Finding silent failures.** At B6's 1% bar, a 300-image run auto-saves about 3 bad results. The **Saved** and **All** filters therefore sort lowest-confidence-first, and every saved tile has **Restore original**. Test T8 seeds bad auto-accepts. A local counter of auto-saved items that were later restored (Advanced > Diagnostics; nothing is sent, M5.80) is the only field signal, since there is no telemetry.

**Bands (04 §4.9).** Good is a calibrated score at or above the mode's cutoff t; Check is 0.60 up to t; Failed is below 0.60 in every mode, or any hard gate (an implausible quad is always Failed). Good is auto-saved (A-3); Check is held and listed first, with the original untouched; Failed leaves the original untouched under the **Draw crop** banner. The UI shows icon plus word, never a raw score. Until `calibration.json` exists, t is 0.95 (Strict), 0.90 (Balanced) and 0.80 (Aggressive); these are interim values (0.9 is a research start value, not the Balanced cutoff), replaced by cutoffs derived from the risk-coverage curve on held-out data.

**Strictness (B6, A-3):** Strict, Balanced, Aggressive (caption: "More results are saved without review."). It moves t and re-buckets instantly from stored calibrated scores. PROVISIONAL targets: Strict at most 0.3% silent failures with about 15-20% flagged, Balanced at most 1% with about 8-10% flagged (B6), Aggressive at most 3% with about 3-5%. **Balanced is the unlabelled default only if the locked golden set shows a silent-failure point estimate of at most 1.0% with a one-sided 95% upper bound of at most 2.0% (gate G2, 07 §7.4); until then previews default to Strict and label Balanced "experimental" (a tag on the control in the review bar and in Settings).** Only silent-failure evidence demotes the default; a Balanced flag rate above 10% (15% at v0.3.0) is a stage-gate note, not a demotion. Captions quote only measured numbers (M5.34, M5.63). Once auto-save has written a run, a change affects only unwritten items, and the bar says so.

**When results save (Assumption A-3, for veto).** The engine's `CommitMode` has three values. **`AfterAnalysis`** (default) waits until every item is triaged, then writes all Good results in one background pass, so hold rules that need the whole batch can run and strictness can still change before any file is touched; review opens when analysis completes, so the grid does not reorder under the pointer. **`Manual`** ("Review before saving") writes nothing until **Save all**; the review bar's **Review before saving** switch and Settings > Review show the same setting. **`Streaming`** (opt-in, Settings > Review) writes Good items as they finish; the grid then keeps a frozen sort order and shows an "N new flagged" pill for arrivals, and the batch-outlier rule is off because it needs the whole batch. Held items are never written in any mode. **Save all** commits whatever the person has resolved; with flagged items unresolved it says "Will save 289. 23 flagged stay untouched. [Review flagged first] [Save 289]". Enhancement is not part of auto-save (B13: suggest, never force), so confident results are saved crop-only; applying the batch suggestion later re-renders from the pristine backup and re-saves. That double write costs time, never quality.

### 6.2.5 Editing one image

Tap a tile or press Enter to open **Review**: full-screen result, chevrons, swipe (at fit zoom only), **Accept & next** (Enter), **Skip** (X), **Adjust** (E). "Review flagged" walks the flagged queue, then offers the rest or Save all. **Adjust** (wireframe C) shows the EXIF-oriented source with an editable quad. It auto-zooms to the crop bounds on entry, so narrow receipts get usable handles, and prefetches full-resolution tiles around all four corners for the loupe.

- **Handles.** Visible dot 16 px with a dual white/dark stroke for 3:1 non-text contrast on any photo ([WCAG 1.4.11](https://www.w3.org/WAI/WCAG22/Understanding/non-text-contrast.html)). Hit area 48 px, never below 44 px. Priority is corner, edge midpoint, then body; a 32 px gutter keeps handles off the viewport edge. If adjacent handles would sit under 96 px† apart on screen, edge handles hide. Body drag pans; a centre grip moves the whole quad.
- **Loupe and auto-pan.** During a finger drag, a 4x† circular loupe appears up and left of the finger (mirrored in left-handed mode, flipped near edges) with a crosshair on the exact point. It samples full-resolution tiles, showing the upscaled proxy on a miss. On by default for touch and pen, off for mouse. Within 48 px† of the viewport edge, after 250 ms† dwell, the view auto-pans with ramping speed.
- **Drag preview (D1).** During a drag the webview applies a geometry-only CSS/canvas matrix to the loaded proxy, with no IPC, at a frame time of at most 17 ms (median) and 20 ms (95th percentile)†. On release Rust renders the real result (release to final image: median at most 150 ms, 95th percentile at most 250 ms†) and it cross-fades in over about 150 ms† (instant under reduced motion); a thin progress line shows if the render exceeds about 250 ms†. This is a second (TypeScript) four-point homography, the drift risk D1 acknowledges. Mitigation: it is used only between press and release, always replaced by Rust's render, and a CI property test compares both on shared test vectors and random quads (fail above 0.05 px on the 2-4 MP display proxy, the bound of 02 §2.5).
- **Rotation dial.** A ruler in 0.1° steps, ±45° per quarter turn, plus **90 L / 90 R**; it edits `quarter_turns` and `fine_deg`. Detents at 0, ±90 and 180 capture within 1.0°† and release beyond 1.5°†. A marker shows the auto-detected angle. The farther the finger from the ruler, the finer the ratio (0.1× beyond 60 px†), and ±0.1° buttons and a numeric field give exactness. Two-finger twist arms after 8°† of rotation so a pinch does not rotate. With Perspective off, the quad becomes a rotated rectangle and the dial rotates it.
- **Several items:** chips (Item 1, 2, 3, **Add item**), one quad each. **Curved pages:** dewarp stays in v1.0 (B10) as an opt-in **Flatten curved page** toggle, offered by a chip when a page looks curved (`CURVED_PAGE_SUSPECTED`, 6.7) and never applied silently; it is automatic only (no editable mesh) and held in Check until its gate metrics justify Good.

**Compare (before/after).** Long-press the canvas body for 400 ms† (under 10 px† movement, off handles, not within 24 px of a screen edge) or hold **Compare** to show the untouched source; a draggable split is the precise alternative. The canvas suppresses `contextmenu`, since Windows treats press-and-hold as a context menu ([Microsoft](https://learn.microsoft.com/en-us/windows/apps/develop/input/touch-interactions)); thumbnails keep it.

### 6.2.6 Save all and the saved summary

Every write runs the commit protocol of 02 §2.7: encode to a temp file in the same folder, verify it (hash check, then re-decode), back up the original, then swap atomically; a backup is never deleted before the new file is saved. The summary bar persists (no auto-dismissing toast): "Saved 305 · Skipped 4 · Failed 1 · [Restore all originals] [Open Backups] [Retry failed]". Files that are not replaced (below) count as Skipped. Quitting with held items says they stay untouched and can be resumed from Recent.

**Output mapping (Assumption A-2, for veto).** *One source to one output* keeps the stem and replaces the source after a verified backup (B3); a conversion such as HEIC to JPG changes the extension and moves the source to Backups. *1-to-N (splitting a multi-item scan)* follows B3 too: the N outputs, named `{name}_{n}` (`scan_01.jpg`, `scan_02.jpg`, ...), take the scan's place and the scan moves to Backups, where it can be restored (6.2.7). A one-time sheet before the first split says so: "1 scan becomes 4 files (scan_01 ...). The scan is moved to Backups for 30 days and can be restored. [Replace scan (recommended)] [Keep the scan and save items as new files]". The setting **Keep the scan** (Settings > Saving, off by default) leaves the scan where it is and writes the items beside it as new files, and **Save as copy** always keeps the source. *N-to-1 (images combined into a PDF or a multi-page TIFF)* writes one new file and leaves the sources untouched unless the run's **Move sources to Backups** box is ticked.

**Never replaced (one rule, enforced only in `engine::FsPlan`, 02 §2.7).** A source is replaced in place only if it is a single-frame image *and* this build can write its default output without dropping content (03 §3.2.3). Replaced after a backup: JPEG, PNG, TIFF and lossless WebP (same format); HEIC and HEIF (to JPG, source moved to Backups); BMP, static GIF, ICO, TGA, PNM, QOI and HDR (to PNG). Never replaced: animated GIF, APNG, WebP, AVIF and JXL; multi-page TIFF; HEIF with several top-level images or sequences; EXR, SVG, PDF, RAW, PSD and JPEG 2000. Not replaced until the writer ships: 1-bit TIFF and PNG, lossy WebP, AVIF and JXL. Such a file stays byte-identical and shows "Not replaced" with its notice (`anim.first_frame_only`, `tiff.multi_page`, `heic.multi_image`, `heic.sequence` or `format.write_unavailable`, 6.7); the person can opt in per file to write a copy (`<source folder>/AutoCrop/`; PNG is the copy format for AVIF, JXL, EXR and RAW).

A name collision with an *unrelated* file keeps both by default (numeric suffix) and is announced; **If exists** (6.9) can choose Skip or Replace instead, and Replace backs the existing file up first.

### 6.2.7 Restore original

Available per image (Adjust "More" menu, tile context menu, Backups panel), per run ("Restore all from this run") and from the summary bar. It runs from the journal and on-disk backup manifests, not memory, so it works after saving and closing (B3); the panel must be able to rebuild from the manifests if the journal is lost (a storage requirement). Guards:

- **The pristine original is never replaced.** Re-saving updates the output only; later edits re-render from the backup, so there is no generation loss. Adjust says "Editing from the original".
- **Changed since saved?** The current file's hash is compared with the recorded output hash; if it differs, offer **Restore as copy** or **Replace anyway**.
- **Restore is reversible** (proposal): the file it replaces is kept as "Result replaced by restore" until retention expires. For HEIC to JPG, the `.heic` returns and the `.jpg` moves to the store. If the original path is gone, offer **Restore as copy…** or **Choose folder**.
- **A split scan (1-to-N).** Restoring it brings the scan back to its original path and asks about the files made from it: "Restore scan.jpg. 4 files were made from it. ( ) Keep them ( ) Remove them (moved to Backups, reversible)". Derived files that changed since saving are listed and preselected as Keep, and moving one needs a second confirmation. Remove never deletes: the files go into a pre-restore backup entry. If another file now occupies the scan's path, the scan is restored as `name (restored).ext`.
- **Retention:** 30 days by default, configurable (7, 30, 90, 365 days or Never), with **Keep** to pin a run. Expired backups are purged at app start, every 24 hours while the app runs and on **Purge now**; no service is installed. No size cap by default; a warning appears at 10 GB† used or below 5 GB† free on the backup volume.
- **No backup, no overwrite.** If a verified backup cannot be made (disk full, unwritable), the run pauses: "Can't back up originals. [Free space] [Change location] [Save copies for the rest]".

### 6.2.8 Convert and export

**Convert only** skips detection and opens the dialog in 6.9 after import; from a normal run, **Save as…** opens it.

## 6.3 Screens and wireframes

Wide desktop layout (1008 epx and up) unless noted; sizes are CSS px in touch density.

**A. Home / drop target**

```
+--------------------------------------------------------------+
| Auto Crop                            [Backups]  [Settings]   |
+--------------------------------------------------------------+
|  +- - - - - - - - - - - - - - - - - - - - - - - - - - - +    |
|  :   Drop photos, scans or a folder here (tap to browse) :   |
|  :   JPG PNG HEIC HEIF WebP TIFF AVIF ...                :   |
|  +- - - - - - - - - - - - - - - - - - - - - - - - - - - +    |
|  [ Open files ] [ Open folder ] [ Paste image ]   (48 px)    |
|  What are you processing?                                    |
|  (o) Documents & receipts  ( ) Photos                        |
|  ( ) Scans with several items  ( ) Convert only              |
|  Saving  Replace originals - backup kept 30 days             |
|          Save as copy [ off ]         [How saving works]     |
|  Recent  Sep-receipts, 312 [Resume]        [Try samples]     |
+--------------------------------------------------------------+
```

**B. Batch review grid**

```
+------------------------------------------------------------------+
| [<] Sep-receipts (Undo)(Redo)(History)  [Backups]  [Save all]    |
| Originals backed up - Restore anytime                            |
| 23 need review | 289 confident  Strictness [Strict|Balanced|Aggr]|
| Review before saving [off]   Enhance: 187 look like receipts     |
|                              [Preview 3] [Apply to all]          |
| [Needs review 23][All 312][Edited 4][Skipped 2][Failed 1][Saved] |
| Sort: Confidence v                Size -o---+       [Select]     |
| +-------+ +-------+ +-------+ +-------+ +-------+ +-------+      |
| |ORIGINL| | result| | result| | result| | result| | result|      |
| |[X]Fail| |[!]Chck| |[!]Chck| |[v]Good| |[v]Good| |[v]Good|      |
| |Draw   | |Right  | |Two    | |       | |       | |       |      |
| |crop   | |edge ? | |outline| |       | |       | |       |      |
| +-------+ +-------+ +-------+ +-------+ +-------+ +-------+      |
| 23 held for review                    [Review flagged >]         |
+------------------------------------------------------------------+
```

**C. Editor (Adjust)**

```
+----------------------------------------------------------------------+
| [< Grid] IMG_07  7 of 23  [!] Check: right edge unclear              |
|                 (Undo)(Redo)(History)    [Compare]    [Accept >]     |
+------+------------------------------------+--------------------------+
|strip |  o-----------o-----------o         | CROP                Auto |
|96 px |  |    source image        |        | Target (o)Paper+margin   |
|      |  o     [+] centre grip    o        |        ( )Content-tight  |
|      |  |                        |        | Aspect Free v            |
|      |  o-----------o-----------o         | Perspective Auto v       |
|      | o = 16 px dot, 44-48 px hit area   | [ ] Flatten curved page  |
|      | loupe 4x shown while dragging      | [Try outline 2 of 3]     |
|      |                   [result inset]   | ENHANCE                  |
|      +------------------------------------+ Looks like a receipt     |
|      | ruler |..|..|.. 0 ..|..|..|        | [Whiten paper?]          |
|      | 0.1 deg steps, snaps at 0/90       | Orig|Auto|Gray|B&W       |
|      | [-0.1][+0.1] -0.4 deg [90L][90R]   | Strength ---o---         |
|      |                          [Auto]    | > Fine-tune              |
|      | Compare: hold, or drag split       | Apply: This|Sel|All      |
+------+------------------------------------+ > Advanced               |
| [Skip X]  [Reset to auto]  [More v]         [Accept & next >]        |
+----------------------------------------------------------------------+
Portrait: canvas, ruler, [Crop][Rotate][Enhance][More] tabs, then
[Skip] (Undo)(Redo) [Accept >]; the inspector becomes a bottom sheet.
```

**D. Convert / Save as** (a sheet, not a page)

```
+-- Save / Convert 312 images --------------------------------------+
| Format [JPG v]   Quality (Small|Balanced|Best) ~41 MB, 12 lossless|
| Resize (o)Original ( )Longest edge [2000] px ( )Percent           |
| Metadata (o)Keep all except orientation and thumbnail             |
|          ( )Keep, strip location  ( )Strip all                    |
|          37 files contain a location   [Strip location]           |
| Colour [x] sRGB (HEIC, enhanced)  [ ] Preserve wide gamut (P3)    |
| Save (o) Replace originals - originals go to Backups (3.4 GB)     |
|      ( ) Save as copy -> <source folder>/AutoCrop/   [Choose..]   |
| Name [{name}]  IMG_0012.jpg IMG_0013.jpg ...                      |
| If exists (o)Keep both ( )Skip ( )Replace                         |
| (i) 41 HEIC: depth maps, HDR gain maps and Live Photo video are   |
|     not kept; 23 HDR photos tone-mapped to SDR; paired .MOV       |
|     files untouched.                          [Details]           |
| > Advanced                                                        |
| [Cancel]                                    [ Convert 312 ]       |
+-------------------------------------------------------------------+
```

**E. Backups / Restore**

```
+-- Backups ------------------------------------------------ [x] ---+
| Originals are kept here before Auto Crop replaces them.           |
| Location [%LOCALAPPDATA%\AutoCrop\backups]       [Open] [Change]  |
| Keep for [30 days v]   Using 3.4 GB - 212 GB free   [Purge now]   |
| v Sep-receipts 2026-09-30 14:02  312 files 1.1 GB  expires 30 Oct |
|   [Restore all from this run] [Keep]                              |
|   [thumb] IMG_0007.jpg  4.1 MB -> 1.3 MB  changed since: no       |
|           C:\...\Receipts\IMG_0007.jpg [Compare][Restore][Reveal] |
| > Flatbed 2026-09-29  scan.jpg to 4 files  expires 29 Oct         |
|   [Restore...] [Keep]                                             |
| > Vacation HEIC 2026-09-28  88 files 0.9 GB  expires 28 Oct       |
| Empty: "No backups yet."          [Delete backup now] (confirm)   |
+-------------------------------------------------------------------+
```

**Keep for** offers 7, 30, 90 or 365 days or Never (default 30); **Keep** pins a run, and **Purge now** removes expired backups at once. The only user-initiated permanent deletion in the app is **Delete backup now**, reachable only here and confirmed; backups past their retention are purged automatically (6.2.7). Uninstalling a direct build keeps the store and says where it is, and `brew uninstall --zap` never touches it; the Store build keeps backups in a real user folder, never in package data that uninstalling removes (02 §2.7).

**F. Settings**

```
+-- Settings --------------------------------------------------------+
| Saving     Save as copy [off] (off = replace, backed up)           |
|            Keep the scan when splitting [off]                      |
|            Backups keep [30 days v]  [Location..] [Open Backups]   |
| Review     Default strictness [Strict v]  (Balanced: experimental) |
|            Save confident results [After analysis v]               |
|            Open on flagged [x]                                     |
| Appearance Theme [System v] Density [Auto v] UI scale [100% v]     |
|            Controls side [Right v] Reduce motion [System v]        |
|            Language [English v]                                    |
| Input      [x] Loupe [x] Auto-pan [x] Long-press [ ] Tap undo      |
| Shell      [x] Open with  [x] File-manager action                  |
| Privacy    [Clear session history and thumbnails]                  |
| Updates*   [Check now] [ ] Weekly notify (off)  No telemetry       |
|            [Open crash log] [Report issue]                         |
| > Advanced                                                         |
+--------------------------------------------------------------------+
```

*Saving:* **Backups keep** offers 7, 30, 90 or 365 days or Never (default 30); **Keep the scan when splitting** is the 1-to-N setting of 6.2.6. *Review:* the default strictness is Strict until the locked golden set passes the Balanced gate, after which Balanced becomes the default (6.2.4); **Save confident results** offers After analysis (default), As they finish (opt-in) and Review before saving, the `CommitMode` values of 6.2.4. *Privacy:* clearing session history and thumbnails never touches originals or backups. **\*Updates** appears only in builds that contain the updater: NSIS installs and AppImages can **Install** on an explicit click after a check (a macOS DMG only if it keeps in-place install), while the portable zip and our own deb and rpm packages only notify and open the release page. Store, Flatpak, winget-managed, distro and no-hevc builds compile the updater out, show no Updates row and update through their own channel. The check reads a static `latest.json` from github.com, the weekly check is opt-in and notify-only, and 0.x previews count as ordinary releases (B18).

## 6.4 Gesture and input model

### 6.4.1 Gesture and input map

Ctrl is Cmd on macOS.

| Action | Touch | Mouse | Trackpad | Keyboard |
|---|---|---|---|---|
| Zoom | Pinch; +/- buttons | Ctrl+wheel; buttons | Pinch | `+` `-`, `0` fit, `1` 100% |
| Pan | Drag background (Adjust), or two fingers | Space+drag, middle-drag | Two-finger scroll | Arrows (no handle focused) |
| Fit / 100% | Double-tap at a point | Double-click | Double-tap | `Z` |
| Move corner or edge | Drag handle, loupe, edge auto-pan | Drag | Drag | Tab to handle, arrows 1 px, Shift 10 px, number fields |
| Move whole quad | Centre grip | Drag grip | Drag grip | Ctrl+arrows |
| Fine rotate | Ruler (farther = finer); twist after 8°†; ±0.1° buttons | Ruler; wheel 0.1° per notch | macOS rotate gesture | Dial: arrows 0.1°, Shift 1°, PgUp/PgDn 5°, Home reset |
| Quarter turn, snap | 90 L / 90 R; detent within 1.0°† of 0/90/180/270 | Same; Alt disables snap | Same | `R`, `Shift+R`; exact steps |
| Compare | Long-press 400 ms†, or hold Compare | Hold Compare or `B`; drag split | Same | Hold `B` |
| Next / previous | Swipe (Review, fit zoom); chevrons | Filmstrip; chevrons | Two-finger swipe (Review) | Left/Right (mirrored in RTL), PgUp/PgDn |
| Undo / redo | Buttons; opt-in two-finger tap, three-finger tap | Buttons | Buttons | Ctrl+Z; Ctrl+Shift+Z or Ctrl+Y |
| Multi-select | Long-press, then tap | Ctrl/Shift-click, rubber-band | Cmd-click | Space, Shift+arrows, Ctrl+A |
| Context menu | Press-and-hold thumbnail | Right-click | Two-finger click | Shift+F10 |

- **Commit on release** (WCAG [2.5.2](https://www.w3.org/WAI/WCAG22/Understanding/pointer-cancellation.html)): a handle drag commits one history entry on `pointerup`; `pointercancel` (an OS interruption) reverts it, and Esc cancels.
- **No custom edge gestures, no shake.** Windows reserves edge swipes and three- and four-finger swipes, and Apple asks apps not to redefine the standard three-finger-swipe and shake undo gestures ([HIG](https://developer.apple.com/design/human-interface-guidelines/undo-and-redo)).
- **Tap undo and redo follow Procreate's convention** ([handbook](https://help.procreate.com/procreate/handbook/interface-gestures/gestures)) but are opt-in: a rushed pinch could false-trigger them, and Windows delivery of three-finger taps to the webview is unverified (spike item).

### 6.4.2 The webview input layer

- **`touch-action`:** `none` on viewport and overlay; `manipulation` on buttons; `pan-y` on grid and panels; `overscroll-behavior: none` on the body to suppress overscroll and swipe-navigation effects.
- **Pointer Events** with `setPointerCapture` on handles. A pointer `Map` feeds a pure recogniser `(state, event) -> (state, effects)` with states IDLE, HANDLE_DRAG, PAN, TWO_POINTER, unit-tested on recorded sequences. Thresholds: slop 8 px†, double-tap 300 ms and 30 px†, two-finger tap under 250 ms and 10 px†, long-press 400 ms†.
- **One `ViewTransform` reducer** takes every source: two pointers (touchscreen); `wheel` with `ctrlKey`, registered non-passive (Windows and Chromium trackpad pinch); on macOS the non-standard, WebKit-only `gesturestart/change/end` with `scale` and `rotation` ([MDN](https://developer.mozilla.org/en-US/docs/Web/API/GestureEvent)), with `preventDefault`. Windows and Linux trackpad rotate has no browser event, so the dial and buttons are the path.
- **Zoom shims.** wry already disables WebView2 pinch page-zoom (`zoom_hotkeys_enabled` defaults false), so Windows needs none; that flag is unsupported on macOS and Linux ([WebView2](https://learn.microsoft.com/en-us/microsoft-edge/webview2/reference/win32/icorewebview2settings5)). Browser zoom hotkeys are off, so UI scale is our own setting (6.5).

macOS "touch" means trackpad gestures; there is no Mac touchscreen path we can validate. Pen input uses mouse density, with the loupe on by default as for touch (6.2.5).

### 6.4.3 Linux: best-effort stance (B8) and graceful degradation

Mouse, trackpad and keyboard are guaranteed. Touch is best-effort, with documented limitations and never labelled "unsupported": the week-1 spike reports on Ubuntu 24.04 and Fedora, Wayland and X11, NVIDIA and Intel, with a real touchscreen, and the Linux device rows (Ubuntu 24.04 and 26.04 (verify), Fedora; Wayland and X11; Intel and NVIDIA; x86_64 only) record works, partial or fails against a pointer bar of median frame at most 22 ms and 95th percentile at most 33 ms†, without ever blocking a release (M9.36). WebKitGTK's GTK layer handles pinch itself, and Tauri documents blank-window and silent software-fallback cases ([docs](https://v2.tauri.app/develop/debug/linux-graphics/)). Whether Pointer Events reach the page intact is unmeasured. Degradation, in order:

1. **Capability probe** at the first two-pointer sequence: do both pointers keep moving without `pointercancel`, and did `visualViewport.scale` or the device pixel ratio change? The result is stored.
2. **Pinch unreliable:** hide "pinch to zoom" hints and promote **+/- buttons and a zoom slider**. Handle drags, dial, loupe and buttons still work.
3. **Multi-touch unusable:** run as single-pointer; touch density still applies when `pointerType` is `touch`.
4. **Zoom shims to test (none verified):** reset the WebKitGTK zoom level to 1.0 on every change, or intercept the GTK gesture through `with_webview` (unsafe).

**If the Slint fallback triggers.** The gate is exactly B8's, evaluated once before any UI is written (M0.70): Linux touch cannot hold about 60 fps on the 24 MP proxy (pass bar: median frame at most 17 ms and 95th percentile at most 20; the trigger is a median above 22 ms after the shims in item 4, Assumption A-10) *and* Slint passes the folder-drop test. The gesture layer would then be rebuilt on `ScaleRotateGestureHandler` (touchscreens on all platforms, trackpad only on macOS and iOS); OS folder drop exists only in the Qt backend; and layout mirroring is unsupported ([slint#2294](https://github.com/slint-ui/slint/issues/2294) open), so the fallback cannot meet B20's RTL-capable layout. Switching therefore needs the owner's explicit waiver of B20's RTL layout, recorded in the M0 ADR, and a licence decision. If Tauri fails on Windows or macOS instead, no UI code is written until the owner decides.

## 6.5 Responsive layout, thumb reach and density

Breakpoints are Windows' epx classes, equal to CSS px: small up to 640, medium 641-1007, large 1008 and up ([Microsoft](https://learn.microsoft.com/en-us/windows/apps/design/layout/screen-sizes-and-breakpoints-for-responsive-design)), via container queries. **Large:** filmstrip left, canvas, 320 px inspector right, Accept/Skip bottom right. **Medium or portrait:** canvas on top, ruler, bottom tab bar (Crop, Rotate, Enhance, More), inspector as a bottom sheet. **Small or snapped:** single pane, grid via Back.

**Thumb reach.** Tablets are mostly held at the sides, so frequent actions (Accept, Skip, Undo, 90° rotate, ruler) sit along bottom corners and edges, and rare or consequential ones (Settings, Backups, Save all) sit top-right. A **Controls side** setting mirrors this and the loupe offset. This is a heuristic the tests must confirm (6.14). Keep controls 24 px† from a maximised window's edge, where Windows edge swipes start.

**Density follows the last-used pointer, not an OS mode.** Windows 11 removed Tablet mode, and the remaining posture signals are OEM-set device-type values ([Microsoft](https://learn.microsoft.com/en-us/windows-hardware/customize/desktop/settings-for-better-tablet-experiences)); macOS and Linux give none. The root carries `data-input="touch|pen|mouse"`, updated on `pointerdown` only, with `(any-pointer: coarse)` as a start-up hint. Changes apply only when no pointer is active, and a Settings override (Auto, Always touch, Always compact) covers 2-in-1 flip-flopping.

- **Touch density:** controls 48 px, minimum 44 px, at least 8 px† apart. **Mouse density:** 32 px, never under WCAG [2.5.8](https://www.w3.org/WAI/WCAG22/Understanding/target-size-minimum.html)'s 24 px. **Handle hit areas stay 44-48 px in both**, since narrow receipts collide otherwise.
- **Guidance, cited accurately.** Microsoft: 40 epx touchable minimum, 44 epx touch-optimised with at least 4 epx spacing. Material: 48 dp with about 8 dp spacing ([Google](https://support.google.com/accessibility/android/answer/7101858)). Apple's 44 x 44 pt is the **iOS/iPadOS default** (minimum 28); the macOS default is 28 x 28 pt (minimum 20) ([HIG](https://developer.apple.com/design/human-interface-guidelines/accessibility)). So "44-48 px" is a deliberate touch-mode choice, not an Apple desktop requirement.
- **UI scale** (90-200%) is our setting and must reflow without horizontal scroll (cf. WCAG 1.4.10).

## 6.6 Undo, redo and their relation to Restore original

Edits are parameters, so history is cheap: a cursor over `Arc<EditState>` snapshots with gesture coalescing, plus command-and-memento for session-level batch actions (architecture).

- **Per image (Adjust):** one entry per completed drag, slider release or nudge burst within 500 ms†; capped at 200 entries per image†.
- **Session (grid):** one named entry per action ("Apply enhancement to 41 images").
- **File level:** Save is not a history entry; it is reversible only through Restore original.

**Affordances.** Always-visible Undo and Redo buttons (48 px touch, 32 px mouse) naming the operation in tooltip and menu ("Undo Move corner", as Apple's HIG asks); Ctrl/Cmd+Z, Ctrl+Shift+Z or Ctrl+Y; a native Edit menu on macOS (key handling checks focus so text fields keep text undo); a **History drawer** with lazily rendered thumbnails, tap to jump; opt-in tap gestures (6.4.1). Sessions autosave, so a crash loses nothing.

**Undo versus Restore.** Confusing the two is a real risk (usability task T2 tests it), so the UI separates them:

- **Undo** takes back an edit. After saving it still works: the image shows "changed since saved" and the file on disk is unchanged until saved again. After closing, the session resumes with capped history. **Reset to auto** is an undoable step.
- **Restore original** brings the original file back on disk, per image or per run. It works after saving and after closing, until the backup expires.

Pressing Undo on a saved image toasts "Edit undone. The file on disk is unchanged until you save. [Restore original file]". The History footer links "Need the original file back? Restore original".

## 6.7 Low-confidence and error UI

**Three tiers, icon plus word:** Good (check-circle), Check (triangle), Failed (x-circle); shape and word differ, so colour is never the only cue, and a raw score is never shown (bands: 6.2.4). **Every reason line is looked up by code.** The engine returns a `HoldReason` code plus parameters, never English; one registry (02 §2.8) holds all codes, and each code owns the Fluent keys `hold.<code>.title`, `.cause` and `.action` (`<code>` in lower case). This table owns the English wording: the **title** is the tile and chip line, the **cause** appears under a **Why?** link (which also opens the matching entry of the "Why was this held?" guide, M5.77), and the **action** is the button offered. A tile shows the title of the first code returned, and the editor header lists every code as a chip. Info chips never change a band, and notices (below) never hold an item.

| Code | Effect | Title (tile and chip) | Cause (under Why?) | Action |
|---|---|---|---|---|
| `NO_QUAD` | Failed | "Couldn't find the edges" | No page outline was found with enough confidence, so nothing was changed. | Draw crop; Try another outline (only if the detector gave more than one candidate); Skip |
| `DETECTORS_DISAGREE` | Check | "Two possible outlines" | The two detection methods found different edges. | Try outline 2 of 3 |
| `WEAK_EDGE` | Check | "Edge unclear on the {side} side" | That edge blends into the background or is partly hidden. | Adjust |
| `PARTIAL_FRAME` | Check | "Content may be cut off" (on a scan item: "Cut off by the scan edge") | The page runs past the edge of the picture. | Adjust |
| `ORIENT_UNSURE` | Check | "Check orientation" | It is unclear which way up the page is. | Rotate 90 L and R |
| `RESIDUAL_SKEW` | Check | "Text still looks tilted" | After straightening, the lines of text still slope slightly. | Rotation dial |
| `NO_DOCUMENT` | Check | "No document found" (on a scan: "No items found") | It looks like an ordinary photo or a blank scan. | Keep original (Skip); Draw crop |
| `ML_UNAVAILABLE` | Check | "Detection model not available" | Only the basic method ran, so every result is held for a look. | Adjust |
| `ANALYSIS_LIMIT` | Check | "Analysis took too long" | The time limit was reached before a reliable result was ready. | Adjust |
| `BATCH_OUTLIER` | Check | "Differs from the rest of the batch" | Its crop size, shape or tilt is unlike most images in this batch. | Adjust; Accept |
| `TOUCHING_ITEMS` | Check | "Touching. Check the split." | Two items touch, so the cut between them may be in the wrong place. | Try suggested split; Merge; Cut |
| `ITEMS_TOO_CLOSE` | Check | "Items very close" | The gap between items is under 1.5% of the scan's shorter side. | Try suggested split; Merge; Cut |
| `OVERLAPPING_ITEMS` | Check | "Items overlap. Check the split." | Items lie partly on top of each other and cannot be separated reliably. | Try suggested split; Merge; Cut |
| `SPLIT_UNSTABLE` | Check | "Split changes with small settings changes" | Slightly different settings give a different number of items. | Try suggested split; Merge; Cut |
| `LOW_CONTRAST_EDGE` | Check | "Edge hard to see" | The item is close in colour to the scanner bed. | Adjust |
| `ODD_ASPECT` | Check | "Unusual shape for a photo" | The item is much longer than a photo; it may be a receipt or a false find. | Adjust; Receipts profile |
| `TOO_MANY_ITEMS` | Check | "Too many items, showing the largest 32" | More than 32 items were found; the smaller ones are ignored. | Add item |
| `BED_UNCERTAIN` | Check | "Scan background unclear. Check the items." | The scanner bed colour could not be measured reliably. | Treat as one item |
| `FADED_PRINT` | Check, when enhancement applies | "Faint print detected" | The print is very light against the paper, so B&W may drop small marks. Check amounts. | Use Auto (colour); Try Faded preset; Show original |
| `INK_COVERAGE` | Check, when enhancement applies | "Text may look too light or too heavy" | The amount of ink after B&W differs a lot from a plain reference conversion. | Adjust Text darkness; Show original |
| `LOST_MARKS` | Check, when enhancement applies | "Small marks may be missing" | Small dark marks, such as decimal points, are absent from the result. Check amounts. | Use Auto (colour); Show original |
| `ESTIMATE_UNRELIABLE` | Check, when enhancement applies | "Paper could not be measured reliably" | Most of the page was too dark or too busy to estimate the paper tone. | Use Original; Show original |
| `CURVED_PAGE_SUSPECTED` | Info | "This page looks curved. Flatten it?" | The page edge or the text lines bend. | Flatten; Not now |
| `DEWARP_UNCERTAIN` | Check (an applied flattening is held) | "Flattening is uncertain" | The flattened result could not be fully verified. | Compare; Unflatten |
| `DEWARP_REJECTED` | Info | "Not applied: {why}" (for example "this looks folded") | The flattened result failed its checks, so the crop-only result stands. | none |
| `DEWARP_NEAR_FLAT` | Info | "Not applied: near flat" | The page is already almost flat. | none |
| `DEWARP_NOT_APPLICABLE` | Info | "Not applied: {why}" | Flattening does not suit this item (for example a cut-off page edge, unsure orientation or a very small item). | none |
| `DEWARP_MODEL_MISSING` | Info | "Flattening is not installed" | The flattening model is missing, so pages cannot be flattened. | Settings > Models |

An implausible quad (non-convex, extreme angles or an implausible area) is Failed whatever its score. Suggested outlines and splits are cycled by the **Try** buttons as one undoable step, and nothing is written for a held item.

**Failed** leaves the original untouched. Tile and Review show the original with a banner: "Couldn't find the edges. The original is untouched. [Draw crop] [Try another outline] [Skip]". **Draw crop** places an editable quad inset about 5%† from the frame and enters Adjust.

**Notices** are Info or Warn chips, looked up by code the same way; they never hold an item or change its band. Examples: `heic.gain_map_dropped` ("HDR gain map not kept") and the not-replaced notices of 6.2.6, each with **Save a copy**: `anim.first_frame_only` ("Animated image: first frame only. Not replaced."), `tiff.multi_page` ("Multi-page TIFF: page 1 only. Not replaced."), `heic.multi_image` ("HEIC with several images: main image only. Not replaced."), `heic.sequence` ("HEIC sequence: first image only. Not replaced.") and `format.write_unavailable` ("This build can't write this format back. Not replaced."). The other HEIC notices (03 §3.4.5) appear in the dialog of 6.9.

**File errors** go to a non-modal **Issues tray**, never a modal per file, each with **Retry**, **Show in folder**, **Copy details**; the row text is looked up by the stable error code (`ErrKind`, 02 §2.10). Reasons: corrupt (`Corrupt`); unsupported (`UnsupportedFormat`); HEIC with unsupported features; HEIC without an HEVC decoder (`HevcDecoderMissing`); over the pixel limit (`TooLarge`: 100 MP by default, C1; the row offers **Allow this file**, and Advanced raises the ceiling to 500 MP); online-only placeholder (`CloudNotLocal`); permission denied; file in use (`FileInUse`; Windows sharing violations are retried first); disk full (`DiskFull`); read-only (`ReadOnly`; suggest Save as copy). Save failures pause and resume and end in the summary with **Retry failed**. A verify-before-replace failure (`VerifyFailed`) reads "Couldn't verify the new file. The original is unchanged." If the file changes while it is being saved (`SourceChanged`), nothing is replaced: "This file changed while Auto Crop was working, so it was left as it is. Its backup is kept. [Retry]".

**Report issue** (B18) first shows a preview of exactly what will be shared. **Copy and open** puts redacted diagnostics on the clipboard (at most about 4 KB†: the last redacted log lines and any backtrace, the version, OS, webview or backend, channel and error code, plus local counts such as auto-saved items later restored; no image content, file names or raw paths) and opens the new-issue page on github.com/WelFedTed/auto-crop with only the version, OS, webview or backend, channel and error code in the URL. The person pastes the text by hand; the app uploads nothing itself. After a crash, the next launch offers the same button, reading the crash file `crash/<ts>-<version>.txt`.

## 6.8 Enhancement UI (B13)

**Suggested, never forced.** A chip near the canvas reads "Looks like a receipt. Whiten paper?", shown only when a cheap heuristic on the analysis proxy sees paper-like content (dominant light region, low chroma, text-like local contrast). The heuristic is to be defined and gated on the golden set, and the chip never appears under the Photos preset (general photo editing is a non-goal, B19). One tap applies **Auto** and opens a live before/after split (drag the divider); **Revert** is one tap, and it is an ordinary undoable edit. Batch form: "Enhance: 187 look like receipts [Preview 3 samples] [Apply to all 187]", previewing three before/after pairs; the batch default is Off or Suggest, never Auto, and applying is a visible, undoable action.

**Modes:** Original, **Auto** (colour kept, paper whitened), **Grayscale**, **B&W** (anti-aliased 8-bit, with a **Faded print** preset for thermal receipts). Grayscale and B&W keep the source format (a JPEG stays a gray JPEG at quality 90 or higher); for a lossy source in B&W the Save as sheet (6.9) suggests PNG but never switches the format on its own. 1-bit is an export option only (6.9). Visible: mode, one **Strength** slider, and scope (This image, Selected, All). **Fine-tune** reveals five sliders, each neutral at 0: Brightness, Contrast, Text darkness, Clean-up (colour and noise only) and Sharpness. **Despeckle** is a separate switch under **Advanced** (6.12), off by default in the Receipt look and on in the Document look, captioned "Can erase decimal points and faint print."; it runs only at full resolution (100% zoom tiles and the saved file), never in the proxy preview. After an update that changes the enhancement algorithm (`algo_ver`), an image edited earlier is re-estimated when it is opened and shows an Info chip, "Result changed after update"; files already saved are not touched.

**Preview budget (PROVISIONAL, D5).** Parameters are cached so a slider tick recomputes only cheap stages: at most 16 ms† on the 2 MP display proxy, about 50 ms† round trip, with one request in flight and a generation counter dropping stale results. The analysis itself runs once on a proxy of about 1.5 MP by area (long edge at most 3072 px). Full resolution renders only in 100% zoom tiles and on save.

## 6.9 Convert and export dialog

Wireframe D. Fields:

- **Format:** *Same as original*, JPG, PNG, WebP, AVIF, TIFF, PDF, JPEG XL (all v1.0, B11). Each 0.x preview lists only the formats it contains. The button reads **Save N** if the format is unchanged, else **Convert N**. Files that cannot be replaced (6.2.6) are listed as "Not replaced" with a **Save a copy** choice.
- **Quality:** Small, Balanced, Best, with a size estimate from encoding a sample of up to five images. JPEGs needing only a 90° turn or MCU-aligned crop transform losslessly ("12 lossless"); any perspective warp or other rotation resamples, so few phone photos of receipts qualify. Re-encoding is one generation from the original.
- **Resize:** Original, longest edge, percent; never upscale by default.
- **Metadata:** *Keep all except orientation and thumbnail* (default, C2), *Keep, strip location* or *Strip all*. Orientation is applied once and reset, and the embedded thumbnail is dropped; GPS stays unless the person strips it. A visible one-click **Strip location** toggle (off by default) sits in the sheet, and when the batch holds GPS-tagged files a chip reads "N files contain a location [Strip location]"; the choice is remembered.
- **Colour:** HEIC and HEIF to JPG, and every enhanced output, default to sRGB, with a visible **Preserve wide gamut (Display P3)** toggle for HEIC (C2); every other source keeps its profile and pixels (the ICC profile byte-exact where the output format can carry it, otherwise converted to sRGB with a notice). PQ and HLG HDR is tone-mapped to SDR and gain maps are dropped, each with a notice.
- **Save:** *Replace originals* (default; originals go to Backups, with size and a free-space check) or *Save as copy* (`<source folder>/AutoCrop/`, or a chosen folder). The name template takes `{name}`, `{n}`, `{date}` and `{ext}` with a live preview of three names; the default is `{name}` (one file to one file, so replaced files keep their names) and `{name}_{n}` for a split scan, and the field is enabled for Save as copy or an explicit rename. One grammar applies everywhere: illegal characters are replaced, and names that differ only by letter case or Unicode form count as the same name. **If exists:** Keep both (default), Skip, Replace (the existing file is backed up first).
- **HEIC notices** (B12, C2), inline when relevant: depth maps, gain maps and Live Photo video not carried into JPG (with counts); HDR photos tone-mapped; the paired `.MOV` left untouched and orphaned when the `.heic` is replaced; unsupported files sent to the Issues tray. In a `no-hevc` distro build: "This build cannot decode HEVC-based HEIC."
- **Advanced:** chroma subsampling, progressive JPEG, compression, ICC, DPI, PDF page size and single versus combined pages, 1-bit output (TIFF CCITT G4, PNG, PDF), AVIF and JPEG XL effort.

## 6.10 Accessibility, theming, i18n and RTL

Target WCAG 2.2 AA for the webview UI (a design target, not a certification claim).

**Accessible parallel model for the canvas.** A canvas is opaque to screen readers, so the crop tool is real DOM under the drawing:

```
<main aria-label="Editor: IMG_07, 7 of 23, needs review">
  <div role="img" aria-label="IMG_07 with crop outline"> canvas + SVG </div>
  <div role="group" aria-label="Crop outline">
    <button aria-roledescription="crop handle"
            aria-label="Top-left corner, x 12.1%, y 8.0%"> x4, edges if shown
  <div role="status" aria-live="polite"> announcements </div>
Inspector: labelled number inputs per corner (x, y in %), angle, zoom
```

- **The 48 px hit areas are the focusable elements**, so touch targets, focus rings and the accessibility tree cannot drift. SVG draws the 16 px dot inside.
- Arrows nudge (1 screen px, Shift 10); values are announced via the live region, debounced 150 ms†. The inspector's number fields are the WCAG 2.5.7 alternative *and* the screen-reader path. Rotation is `role="slider"` with `aria-valuetext` ("minus 0.4 degrees") following the [ARIA slider pattern](https://www.w3.org/WAI/ARIA/apg/patterns/slider/), except Home, which resets (stated in its `aria-describedby` text).
- The grid uses roving tabindex; tile names carry status ("IMG_07, needs review, edge unclear on the right, not saved"). Progress is a polite live region throttled to about every 5 s†. Focus moves to the editor heading on entry and back to the tile on exit.
- WebView2 exposes UIA and WKWebView feeds VoiceOver, but Orca and AT-SPI coverage in webviews is unverified, so Linux is best-effort here too: Orca results are reported and do not gate 1.0 (A-11). The gating screen-reader checks are NVDA or Narrator on Windows and VoiceOver on macOS.

**Keyboard-only editing** is complete. Character-key shortcuts fire only outside text fields and can be turned off or rebound (WCAG 2.1.4). Focus is always visible and never hidden by sticky bars (2.4.11).

**Contrast, motion, themes.** Honour Windows Contrast Themes via `forced-colors: active` (overlay uses `Highlight` and `CanvasText`) and `prefers-contrast: more`. Text 4.5:1, graphics 3:1, status never colour-only, canvas a neutral mid-grey in every theme so white paper edges stay visible. `prefers-reduced-motion` (plus an in-app override) removes cross-fades, inertial pan, slides and animated zoom. Themes (Light, Dark, System, High contrast) are CSS custom-property tokens.

**i18n (B20).** English at launch, strings externalised from day one. Recommendation, to confirm in the spike: **Project Fluent**, with `fluent-rs` in Rust so CLI and GUI share a catalog; the core returns typed error codes plus parameters, never English. A **pseudo-locale** (accents, about 40%† expansion, bracket markers) runs in CI with truncation screenshots.

**RTL (B20).** CSS logical properties only, enforced by a stylelint rule; a pseudo-RTL locale in CI; one real RTL locale is a stretch goal needing a volunteer reviewer. **Mirrored:** layout, filmstrip side, Previous/Next arrows and Left/Right keys, progress bars. **Not mirrored:** the image, the quad, the rotation dial and rotation-icon direction. Budget 30-40%† text expansion.

## 6.11 OS shell integration UX (B19)

- **Open with and file associations** for the supported types, always as an alternative handler, opt-in, and never changing the default app or the user's own choice. *Windows:* not Tauri's `bundle.fileAssociations` for JPG, PNG, TIFF and WebP (its NSIS template may claim the class default; to verify); the NSIS `installerHooks` (an opt-in checkbox) write `Applications\AutoCrop.exe` and `OpenWithProgids` under HKCU, leave the default and UserChoice alone, and the uninstaller removes them; the portable zip writes the same keys from a Settings toggle. *macOS:* document types with `LSHandlerRank = Alternate`. *Linux:* the desktop entry's `MimeType=`, with `Exec=AutoCrop %F`. One extension list feeds the installer, Settings and the shell verb (M3.19, M6.85, M13.20).
- **Single instance.** A second launch forwards its paths to the running window; launches within about 300 ms† merge into one run, because Explorer can start one process per selected file. A toast says "Added 12 images to the current run".
- **Windows 11.** The classic verb "Crop and straighten with Auto Crop" satisfies B19 and sits under "Show more options"; a top-level entry needs an `IExplorerCommand` handler with package identity (the Store MSIX) and is an optional stretch (M13.22, M13.23).
- **macOS.** A Quick Action workflow runs `open -b io.github.welfedted.AutoCrop`; Settings offers "Add to Finder Quick Actions", which copies it to `~/Library/Services`. An `NSServices` handler is not attempted, and a Finder Sync extension is rejected because it needs signing that B15 rules out.
- **Linux.** A Dolphin service menu and a Nemo action ship in the deb and rpm packages, and Settings > Integration offers an opt-in Nautilus script and Thunar action (M9.39 to M9.42); elsewhere "Open With" is the path.
- **One verb for 1.0:** "Crop and straighten with Auto Crop", the same label on every OS, opens the selection and starts the run; "Convert to JPG" is a candidate.
- **Flatpak and overwrite.** Overwrite needs a temp file beside the original, which a portal-only sandbox may forbid. Mitigation: the own-remote manifest requests access to user folders; if not granted, fall back to Save as copy with a notice (unverified spike item).

## 6.12 What lives behind Advanced

Nothing behind Advanced is needed to complete a task. Sections remember their open state, and every item has a reset plus "Reset all to defaults".

- **Detection:** numeric cut-offs, maximum skew, perspective on/off, crop margin (B14; set per route, 04 §4.6), content-tight default, numeric confidence diagnostics.
- **Enhancement:** **Despeckle** (off in the Receipt look, on in the Document look, with the erase caption of 6.8), binarisation method (Sauvola or NICK), window size, shadow strength, CLAHE, paper tint, colour marks.
- **Output:** DPI, chroma subsampling, ICC, gain maps, 1-bit modes, naming tokens, conflict policy, PDF options.
- **System and input:** threads, memory budget, pixel limit (100 MP by default, 500 MP ceiling), tile cache, undo depth, logs, **Diagnostics** (local counts only, such as auto-saved items later restored; nothing is sent), usability recording, loupe magnification, long-press time, tap-undo, shortcut rebinding, density override.
- **Not Advanced:** Save as copy, Keep the scan, backup retention and location, and when results save stay visible in Settings or the review bar, because B3 makes them safety-relevant.

## 6.13 Keyboard shortcuts

Ctrl is Cmd on macOS. All are rebindable. Letters are preferred over punctuation because punctuation position varies across layouts.

| Scope | Shortcut | Action |
|---|---|---|
| Global | Ctrl+O, Ctrl+Shift+O, Ctrl+V, Ctrl+S, Ctrl+E | Open files, open folder, paste, Save all, Save as / Convert |
| Global | Ctrl+Z, Ctrl+Shift+Z or Ctrl+Y | Undo, redo |
| Global | Ctrl+, / Ctrl+Shift+B / `?` | Settings / Backups / shortcut sheet |
| Grid | Arrows, Enter, Space, Ctrl+A, X | Move, open, select, select all, skip |
| Review | Enter, X, E, Left/Right (mirrored in RTL), PgUp/PgDn | Accept, skip, Adjust, previous/next |
| Adjust | Tab, arrows, Shift+arrows, Ctrl+arrows | Focus handle, nudge 1 px, 10 px, move outline |
| Adjust | `R` / `Shift+R` (`]` / `[`), `+` `-` `0` `1` `Z` | Rotate 90° right / left; zoom in, out, fit, 100%, toggle |
| Adjust | Hold `B`, `N`, `G`, Ctrl+Shift+A, Esc | Compare, cycle enhancement, grid overlay, reset to auto, cancel or leave |

Restore original deliberately has no shortcut (a file-level action with a confirmation), and Delete is unbound.

## 6.14 UX test plan

Task-based moderated tests on real touch hardware, in rounds tied to milestones. The 0.x previews are Windows-only (B9), so macOS and Linux rounds follow before 1.0.

**Hardware:** a Windows 2-in-1 or Surface-class device (touch, pen, touchpad; the primary touch target and density switching); a Windows desktop (mouse, keyboard-only); a Windows tablet in portrait (thumb reach); a MacBook (trackpad `gesture*` events, VoiceOver); a Linux touchscreen laptop on Ubuntu 24.04, Wayland and X11 (best-effort touch, probe and degradation).

**Rounds:** R0 week-1 spike (gesture feel and frame rate on a clickable Svelte prototype); R1 clickable flows with synthetic data (M3.82, tasks T1 and T4); R2 alpha with the real engine on Windows; R3 pre-1.0 on all three OSes plus an accessibility pass. Recruit 5-8† per round per device class, mixing receipt scanners and phone-photo converters. Use think-aloud, screen and touch-overlay recording, and a System Usability Scale target of 75 or more† (no baseline yet).

| ID | Task | Success criterion (all PROVISIONAL†) |
|---|---|---|
| T1 | Drop a 20-image folder on first run | Each states, unprompted, that originals are replaced and where backups go |
| T2 | Restore an image after closing and reopening | Unaided; chooses Restore, not Undo |
| T3 | Fix 3 flagged items in a 30-image batch; separately, review 300 images with about 25 flagged | Flagged found first; median under 90 s; the 300-image case about 2 min (research target, unmeasured) |
| T4 | Move a corner on a narrow receipt by touch | At least 90% of drags start on the intended handle |
| T5 | Straighten with the dial, snap at 0° then 90° | Within 0.3° unaided |
| T6 | Compare, apply and revert enhancement | Understands it is optional; reverts in one tap |
| T7 | Convert HEIC to JPG with location stripped (M6.84) | Finds the Strip location toggle unaided; reads the notices |
| T8 | Find 2 seeded bad auto-accepts in 100 images | Both found within 3 min |
| T9 | Adjust a crop by keyboard only | Completes; focus never lost |
| T10 | Adjust a crop with NVDA or Narrator, and with VoiceOver | Completes via handle names and number fields; Orca is best-effort and does not gate |

Feature-specific tasks follow the same method: MI-1 and MI-2 for multi-item scans (fix a wrong merge, a missed item and a dust item; restore a split scan and choose Keep or Remove correctly, M10.46) and the flattening round (M12.50).

**Other checks.** Automated axe-core in Playwright on the DOM (not the canvas), plus pseudo-locale and pseudo-RTL screenshot runs. Manual passes for keyboard-only, Windows Contrast Themes, 200% scale, reduced motion and colour-blindness. Playwright's Chromium approximates WebView2 touch but not WKWebView or WebKitGTK, and Tauri's WebDriver has excluded macOS (verify), so macOS gestures rely on manual tests. **Instrumentation without telemetry (B18):** an opt-in local recording (Advanced) writes a JSON event log that the tester exports by hand.

**Exit criteria.** T1-T8 pass on Windows, and on macOS via trackpad, before each platform's release. Linux touch results are recorded with known issues listed and do not gate release (B8). T9 and T10 gate 1.0.

## 6.15 Assumptions in this section to veto

The owner-decision assumptions A-1 to A-12 are listed in 01 §1.7. This section relies on A-3 and A-2 (items 1 and 2, in their canonical wording), on A-10 for the Slint gate and its B20 RTL waiver (6.4.3) and on A-11 for Orca (6.10).

1. **Assumption A-3:** Confident results are saved after the whole batch is triaged (streaming is opt-in). Previews default to Strict and label Balanced experimental until the golden gate passes; B6 stays the target. (owner may veto) B4 and B5 are reconciled as `CommitMode` (6.2.4), and Accept with scope All covers Good items only.
2. **Assumption A-2:** A 1-to-N split follows B3: outputs take the scan's place and the scan moves to Backups ("Keep the scan" is a setting). Multi-frame sources and formats this build cannot write back are never replaced; results are new files. (owner may veto) N-to-1 results (images combined into a PDF) are new files and leave the sources alone unless "Move sources to Backups" is ticked (6.2.6).
3. Unrelated name collisions keep both files by default; If exists can choose Skip or Replace, and Replace backs the existing file up first.
4. Restore is itself reversible, **Keep** pins a run, and restoring a split scan offers Keep or Remove for the files made from it (6.2.7).
5. Confident results are saved crop-only; enhancement is applied afterwards on request.
6. Backups default to the shared per-user store (02 §2.7) with a warning at 10 GB used or below 5 GB free and no size cap.
7. Fluent as the message format, pending the spike.

## 6.16 Visual design reference (design canvas)

A clickable visual design of the screens in 6.3 was built on 2026-10-01 as a design canvas (a private artifact, owner access only): <https://claude.ai/artifact/3qmimSAb6smSL5LtfQTCKg>. It is a **reference for look, layout and copy, not a specification**: the decision log and sections 6.1 to 6.15 win wherever they differ. It uses synthetic data, a placeholder 24-image batch, and no engine. Because the link is private, the tokens below are the durable part.

**Boards.** A Home (and A2 dark), B Batch review (clickable: filters, sort, size, strictness, selection, Accept with the unreviewed-items confirmation, Skip, Undo, Save all with its confirmation and summary; B2 dark), C Adjust (clickable: draggable corner and edge handles with a loupe, keyboard nudging, scrolling angle ruler, outline cycling, corner number fields, enhancement modes, Compare, zoom, undo and redo; C4 dark), C2 Review of a Failed item, C3 Adjust on a portrait tablet, D Convert / Save as, E Backups and Restore (restore, restore as copy, replace anyway, undo restore), F Settings, G first-write sheet and saved summary, H and H2 a multi-item scan (touching items, suggested split, merge and cut, first-split sheet, restoring a split scan).

**Tokens (starting values, to be verified against the M3.48 contrast checklist).**

| Role | Light | Dark |
|---|---|---|
| Window and surface | `#F2F3F5`, `#FFFFFF` | `#14171C`, `#1D2128` |
| Line | `#DDE0E6` | `#2F3541` |
| Text, secondary, tertiary | `#15181E`, `#4A5160`, `#5F6675` | `#ECEEF2`, `#B4BAC6`, `#9199A8` |
| Accent (fill, tint) | `#2B4FD8`, `#E8EDFC` | `#3E63E6`, `#1B2438` (link and focus `#9DB4FF`) |
| Good (text on tint) | `#0B5B33` on `#E3F4EA` | `#7FE0A8` on `#12301F` |
| Check (text on tint) | `#7A4200` on `#FDF0D5` | `#F5C56B` on `#3A2A0A` |
| Failed (text on tint) | `#A3201A` on `#FCE7E5` | `#FF9C94` on `#3F1613` |

Type is IBM Plex Sans (400, 500, 600) with IBM Plex Mono for file names and numeric readouts (both OFL-1.1, allowed by the M3.67 font rule); the canvas loads them from Google Fonts, **the app must bundle them** (B18: no network). Radii are 8 px for controls, 10 to 12 px for cards and 16 px for dialogs; touch controls are 48 px and mouse controls 32 to 36 px (6.5); the three tiers always pair an icon with a word (6.7).

**Deviations and additions to resolve before M3 builds on it.**

1. The Adjust canvas is a dark neutral grey (`#2A2D34`) in both themes. 6.10 asks for a *mid*-grey so white paper edges stay visible; tune it on real receipts.
2. New copy that needs `hold.*` or UI keys and an owner read: "Select shown", "Use this outline", "Try the other outline", "Treat as several items", "Accept split", "Cut between them", "Treat as one item", "Purge expired now" (6.2.7 says "Purge now"), and the Settings section name "System integration" (6.3 wireframe F says "Shell").
3. The enhance bar shows only "Apply to all N" and "Undo"; the "Preview 3 samples" button of 6.8 is not drawn.
4. Counts, scores and the Strict, Balanced and Aggressive cut-offs in the clickable grid are placeholders (0.95, 0.90, 0.80, the interim values of 6.2.4), not measurements.
5. A split scan shows a number badge and colour per item; colour must never be the only cue, so the number stays.
6. Not drawn: the Issues tray, the History drawer, the "Already processed" state, the Updates settings row, RTL and high-contrast themes, and the Windows 11 context-menu entry.
