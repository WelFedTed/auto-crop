# Auto Crop - decision log (authoritative)

This is the authoritative decision log. Where any other document disagrees with it, the decision log wins; to change a decision, the owner records the change here first. Dated 2026-09-30 (interview); reconciled 2026-10-01 after review. Research snapshots are in [../research/](../research/README.md).

## A. Original brief (the owner's requirements)
Desktop app "Auto Crop"; PLANNING ONLY for now (no code yet).
1. Open image files/folders; accept drag-and-drop.
2. Automate cropping, rotating, correcting skews/distortions.
3. Offer to improve contrast + readability of black-and-white document scans (e.g. a photo of a receipt).
4. Very fast, very accurate.
5. Previews of the changes with option for manual adjustment.
6. Undo support.
7. Free and open source, pushed to a NEW PUBLIC GitHub repo named `auto-crop`.
8. Cross-platform: Windows, macOS, Linux.
9. Rust presumed backend; the user wants an honest opinion.
10. GUI intuitive and easy, designed with TOUCH SCREEN controls in mind.
11. Support "all image formats".
12. Automated image conversion (format to format), particularly HEIC + HEIF -> JPG.
13. ROADMAP.md is a LIVING document: a full-feature checklist (markdown checkboxes) grouped by version release/milestone; it doubles as the effective to-do list; boxes are ticked as tasks/features are completed.

## B. Decisions made by the owner in the interview (final unless the owner reopens them)
| # | Topic | Decision |
|---|---|---|
| B1 | Input types that matter | ALL FOUR: phone photos of docs/receipts; flatbed scans of documents; multi-photo flatbed scans (several items per scan -> split); general photos (horizon/border crop, EXIF rotate, HEIC->JPG) |
| B2 | Licence | MIT OR Apache-2.0 (permissive, Rust-standard dual licence). Consequence: hard ban on AGPL, GPL, and non-commercial-licensed dependencies/weights; LGPL only as separately shipped, dynamically linked, replaceable libraries |
| B3 | Originals & undo | OVERWRITE ORIGINALS BY DEFAULT, with: automatic backup of the original before overwrite + "Restore original" that works even after saving/closing the app (backup kept ~30 days by default, configurable); undo/redo during the session. Format conversion (e.g. HEIC->JPG) REPLACES the source file (source moved to the backup store, reversible via Restore original). NOTE the research default was "save a copy"; the plan must implement overwrite-by-default safely (atomic temp+rename, verify-before-replace, lossless JPEG transforms where possible to avoid generation loss, first-run explanation, a "Save as copy" setting, a Backups panel) |
| B4 | Low-confidence results | HOLD UNTIL REVIEWED: confident results are auto-saved in a batch; low-confidence results are NOT written and appear FIRST in the review grid. Failed detections leave the original untouched and show a "Draw crop" banner |
| B5 | Batch workflow | Auto-process all in background, review flagged items first, then "Save all". (Live preview grid and per-image accept/adjust/skip are part of that flow) |
| B6 | Auto-accept bar | BALANCED: target <=1% silent failures (bad result auto-accepted), ~8-10% flagged for review; adjustable per batch (Strict/Balanced/Aggressive) |
| B7 | ML | HYBRID: fast classical detection + small bundled local ONNX model(s) (~12-25 MB total: corner/quad net + 4-way orientation net) as cross-check/refiner; 100% offline |
| B8 | GUI stack | Rust core + web UI: TAURI 2.x + Svelte 5 + TypeScript (stay on stable 2.x, not Tauri 3 alpha). Tauri-specific code isolated in a thin shell crate; core is UI-agnostic. LINUX TOUCH IS BEST-EFFORT (mouse/trackpad/keyboard guaranteed everywhere; touch guaranteed on Windows and macOS; Linux touch validated in a week-1 spike). Pre-agreed fallback: Slint (only if the spike shows Linux touch cannot hold ~60 fps pan/zoom on a 24 MP proxy AND it passes a folder-drop test) |
| B9 | Platforms | FIRST public release = Windows 10/11 x64 only. CI builds Windows+macOS+Linux from day one so nothing rots. v1.0 REQUIRES ALL THREE OSes to be solid. Early 0.x previews are Windows-only; macOS and Linux previews follow before 1.0 |
| B10 | v1.0 scope | EVERYTHING the user selected is in v1.0: B&W/contrast enhancement; multi-photo scan splitting; headless CLI; curved-page dewarping. Delivered as INCREMENTAL public 0.x previews (one per milestone) toward 1.0; 1.0 = full scope stable. (Research recommended cutting dewarp/multi-item/AVIF/JXL to 1.x — the plan must keep them in v1.0 but gate each behind accuracy/quality exit criteria and call out schedule risk honestly) |
| B11 | Export formats | JPEG, PNG (assumed) + WebP, AVIF, TIFF (incl. 1-bit CCITT G4), PDF (single + multi-page), JPEG XL — ALL in v1.0 |
| B12 | HEIC/HEVC | BUNDLE libde265 (with libheif, decode-only build) IN ALL OFFICIAL BUILDS. Mitigations to design in: libheif/libde265 shipped as separate DYNAMICALLY LINKED shared libraries (LGPL-3 relinkability), run in a sandboxed worker-process pool with pixel/memory/time caps; no x265/x264; libheif tracked >=1.23.5 within days of security advisories; HeicBackend trait allowing OS codecs (WIC/ImageIO) as optional fast path; a build flag/"no-hevc" variant for distro packagers; a roadmap item for a short legal read on HEVC patents before 1.0 and an explicit patent-risk note in README/SECURITY; UI/CLI message when a HEIC uses unsupported features (gain map, depth, Live Photo video dropped with notice) |
| B13 | Enhance default | On a coloured receipt/document: SUGGEST enhancement with a live before/after preview (one tap) — colour kept, paper whitened; never forced. Modes: Original / Auto (colour, paper whitened) / Grayscale / B&W. B&W output is anti-aliased 8-bit; 1-bit is an EXPORT option; despeckle OFF by default for receipts (avoid erasing decimal points/faint thermal print) |
| B14 | Crop target | Paper/photo edge + small margin (default), with a "content-tight" toggle |
| B15 | Signing/budget | $0: NEVER PAY. Free channels only: SignPath Foundation free OSS signing for Windows (once eligible after first public release), Microsoft Store MSIX (verify fee status), own Homebrew tap, unsigned macOS DMG with "Open Anyway" instructions (ad-hoc signed as required for Apple Silicon), Linux AppImage/deb/rpm/own Flatpak remote. Windows previews are unsigned (SmartScreen warning) until SignPath approves |
| B16 | Repo owner / IDs | Personal account WelFedTed; repo `WelFedTed/auto-crop`; app ID `io.github.welfedted.AutoCrop`. Name "Auto Crop" is generic (collision risk noted) -> always pair with tagline |
| B17 | AI disclosure / Flathub | Disclose AI assistance openly in README/CONTRIBUTING. Ship own Flatpak remote + AppImage + deb/rpm first; hand-write the Flathub manifest and submit AFTER 1.0 with human-authored submission |
| B18 | Network & updates | Fully offline by default. Manual "Check for updates" + OPT-IN weekly notify-only check. NO telemetry. Local crash log + "Report issue" button opening a pre-filled GitHub issue. Updater compiled out of Store/Flatpak/distro builds. No silent auto-update (tool overwrites user files) |
| B19 | Extra features | IN v1.0: OS shell integration ("Open with Auto Crop", file associations, Explorer/Finder/Linux file-manager context action). EXPLICIT NON-GOALS for 1.0: OCR / searchable-PDF text layer, scanner/camera capture (TWAIN/WIA/SANE/webcam), watch-folder auto-processing, mobile app, general photo editing (filters/retouching). Keep data model/architecture from painting us into a corner for these |
| B20 | Languages | English at launch, i18n-ready from day one (externalised strings, pseudo-locale in CI; RTL-capable layout; one RTL locale in CI is a stretch goal); community translations later |
| B21 | Test corpus | The REAL hand-labelled golden set (user's own specimen/redacted docs, receipts, etc.) stays PRIVATE. Public repo holds only synthetic + public-dataset (permissively licensed) data. CI runs the private set only via a private repo/secret-gated workflow (never on fork PRs), and reports aggregate metrics publicly |

## C. Assumptions not explicitly asked (defaults; the owner may veto any)
- C1 Hardware floor: x86-64 with AVX2 on Windows 10 22H2+/11; 8 GB RAM, 4 cores; images up to ~100 MP supported, graceful clear error above; ARM64 (Windows/Linux) and Intel Macs are best-effort until 1.0 (ONNX Runtime ships no macOS x86_64 binary -> rten/tract/load-dynamic for Intel Macs); macOS 12+, Ubuntu 22.04+.
- C2 HEIC->JPG colour/metadata: default sRGB with a visible "preserve wide gamut (Display P3)" toggle; keep metadata except orientation (applied once and reset) and embedded thumbnail; a visible one-click "strip location" toggle; HDR/gain-map -> SDR tone-map with a notice.
- C3 Desktop only (no mobile), but core stays UI-agnostic so mobile remains possible.
- C4 Telemetry none; privacy-first; all processing local.
- C5 DCO sign-off (no CLA); SPDX/REUSE headers; cargo-deny + cargo-about in CI.
- C6 Conventional commits, semantic versioning, release-plz, GitHub artifact attestations + SHA256SUMS.
- C7 Contributor tooling: devcontainer + `xtask` for native toolchain (CMake/NASM etc.).

## D. Resolutions of conflicts between research topics (owner decisions applied)
1. GUI: Tauri primary (B8); one authoritative pixel path in Rust: during a drag the webview applies a geometry-only CSS/canvas matrix to the already-loaded proxy (no IPC); on release Rust renders the real result and cross-fades. Fixed pyramid: 256 px thumbnail, 1024 px detection proxy, ~1.5k analysis proxy, 2-4 MP display proxy + 512 px tiles served via a custom URI scheme. Never send full-res to the webview. Rust decodes everything (HEIC/RAW never reach the webview).
2. Licence: MIT OR Apache-2.0 (B2). Rule out AGPL (heic crate, dssim-core if linked), GPL (jpegxl-rs, x265/x264) and non-commercial (DocTr, DocGeoNet, DocEnTr) code/weights. Reimplement GPL ideas clean-room from papers. Enforce with cargo-deny.
3. Decode isolation: persistent worker-process pool (shared-memory pixel return) from v1 for C parsers of untrusted input (libheif, libde265, PDFium, LibRaw if ever); zune-jpeg/png/tiff/turbojpeg in-process behind hard limits (pixel cap, alloc cap, timeout), per-item catch_unwind, panic=unwind. Enforce own caps (image's default alloc limit is non-strict).
4. HEIC engine: HeicBackend trait; own decode-only libheif (>=1.23.5) CMake build with libde265 (B12) — NOT libheif-sys embedded mode; never x265/x264; OS codecs (WIC/ImageIO) optional. Re-evaluate MIT/Apache pure-Rust heic-rs in ~6 months.
5. Budgets are PROVISIONAL, all unmeasured: <=700 ms single image at 12 MP end-to-end (auto-process), >=4-5 images/s on 6 cores, <=40 ms proxy analysis, <=150 ms first overlay. Re-baseline after the Tier-M spike. Lossless JPEG fast path for 90-degree rotates and MCU-aligned crops.
6. Pure-Rust kernel gaps: imageproc lacks Lanczos warp/Sauvola/CLAHE (and its u32 integral overflows above ~16.8 MP) -> spike kornia-imgproc Lanczos warp first, then own strip-wise u8/u16 Lanczos warp; Sauvola with u64 strip integrals; LSD only ported from OpenCV's BSD-style version or use EDLines; opencv-rust only as a DEV-ONLY test oracle, never shipped; turbojpeg needs CMake+NASM (so "no C toolchain" is false) and must pin libjpeg-turbo >=3.1.4; write own TIFF G4 wrapper (fax::tiff::wrap hardcodes 200 dpi/WhiteIsZero).
7. Model supply: bootstrap corner net from MakeACopy DocQuadNet-256 (13.4 MB, Apache-2.0 label; GitHub reports NOASSERTION -> verify) ; own training pipeline in a separate repo; OSI-only model-weights policy + data-provenance log. UVDoc-class dewarp weights need a provenance audit (textures/backgrounds third-party); avoid DocTr/DocGeoNet (non-commercial).
8. Tauri facts: updater supports deb/rpm/AppImage/NSIS/MSI (plugin 2.10+); bundler does NOT produce Flatpak (manual flatpak-builder); wry disables WebView2 pinch page-zoom by default so only Linux pinch (wry#544, tauri#13115) is the real problem; wry 0.57 still GTK3 WebKitGTK.
9. Slint fallback corrections: gesture handler in 1.16, file paths in 1.18, OS drops only in Qt backend, Skia opt-in, no RTL mirroring.

## E. Research follow-ups that must become roadmap items (spikes and gates)
- Week-1 GUI spike: Tauri vs Slint on Windows (Surface), macOS, Linux (Ubuntu 24.04 + Fedora; Wayland+X11; NVIDIA+Intel; real touchscreen): folder-drop (incl. symlink loops, OneDrive placeholders), 24 MP proxy pinch/pan/rotate ~60 fps, HiDPI, dark mode, 500-image folder tile serving through custom URI scheme, IPC latency.
- Benchmark + accuracy harness BEFORE any GUI work (criterion for trends, gungraun instruction counts as CI gate, nightly wall-clock); real 12/48/100 MP files; Tier-M laptop + Apple silicon baselines.
- ort vs rten vs tract spike (ort is 2.0.0-rc.13 wrapping ONNX Runtime 1.28); int8 accuracy check per platform.
- Real-device HEIC corpus (iPhone, Android; Display P3, HDR/gain map, Live Photo, burst, grid); OS decoder benchmarks.
- Receipt-specific stratified golden set (long narrow receipts >4:1, thermal fade, partial frames, touching items) — private per B21; test 256x256 heatmap net on 8:1 aspect early.
- Legal: short HEVC patent read before 1.0 (B12); trademark-ish check on "Auto Crop" (USPTO/EUIPO) as a low-priority item.
- Data provenance audit for any trained/bootstrapped weights.
- Plumbing tests: tile serving 500-image folder, folder-drop edge cases, memory budgeting for HEIC batches, per-OS sandbox of decode helper (Windows job objects/AppContainer, Landlock+seccomp, sandbox_init), Windows retry-on-sharing-violation, fsync of parent dir after rename, backups hardlink/reflink only same-volume.

## H. Interpretations made during plan review (owner may veto any)

These readings were needed to resolve conflicts between documents without changing decisions B1-B21. Each is an assumption the owner can veto; until then they are what the plan assumes.

- **A-1** (Should the CLI overwrite by default?) The CLI follows B3: with no output flag, `process` overwrites in place after a verified backup; `--output <dir>` and `--suffix <s>` write copies; `--in-place` is an explicit alias. The backup store, `restore`, `--dry-run` and a one-time stderr notice cover script accidents.
- **A-2** (Split and never-replaced sources?) A 1-to-N split follows B3: outputs take the scan's place and the scan moves to Backups ("Keep the scan" is a setting). Multi-frame sources and formats this build cannot write back are never replaced; results are new files.
- **A-3** (When do results save, which default strictness?) Confident results are saved after the whole batch is triaged (streaming is opt-in). Previews default to Strict and label Balanced experimental until the golden gate passes; B6 stays the target.
- **A-4** (Gate-miss rule (08 §8.11 Q4)?) A missed exit gate delays 1.0. Shipping the feature labelled Experimental or moving it to 1.x reopens B10 and is the owner's decision, put with measured numbers; nothing is lowered, cut or disabled silently. Proposed cut order if needed: dewarp, AVIF/JXL encoders, multi-item.
- **A-5** (Is a $0 HEVC read enough?) Official builds always bundle libde265 (B12). The $0 legal read (pool policies re-read with dates, any free-clinic reply, gap disclosed) is due before the first published HEVC build if practicable, no later than 1.0; a `no-hevc`-only official release needs the owner's recorded decision.
- **A-6** (Store MSIX with a bundled decoder (Q2)?) The Store build is verified early by a hidden dry run (M6.81) and keeps backups outside virtualised package data; a Store-only `no-hevc` build or any fee needs the owner's decision (B12, B15).
- **A-7** (Weights exceptions (Q5)?) Only OSI-licensed lineage ships. ImageNet-initialised, DocQuadNet-derived or UVDoc weights need an owner-granted exception (ADR, status `exception`); otherwise from-scratch or Track B weights ship.
- **A-8** (Labelling budget and hosting?) The golden set reaches v2 (>= 800 locked, >= 80 per gated slice, plus a ~300 dev tier) before 1.0, and its images never leave the maintainer's encrypted disk; a hosted variant needs the owner's approval.
- **A-9** (Hardware and testers?) Gates needing a Mac, a Windows touch device or a flatbed scanner stay open until measured; publishing past one needs the owner's recorded delta (M13.42), never "CI-validated only".
- **A-10** (Slint fallback thresholds and waivers?) For B8, "cannot hold about 60 fps" on Linux means median frame > 22 ms after shims on the 24 MP proxy; Linux touch stays best-effort; switching to Slint needs the owner's waiver of B20 RTL layout and a licence decision.
- **A-11** (Scope readings?) Radial lens correction is not in 1.0 (perspective and curved-page dewarp deliver "distortion"); Orca results are best-effort and do not gate 1.0; the default crop margin is per route (04 §4.6).
- **A-12** (Release process?) 1.0 needs two RC windows of 14 days (3 days if the RC differs only by native-library bumps); 0.x previews are ordinary releases; `auto-crop` is reserved on crates.io at the first release, no `auto-crop-core`.

## I. Owner confirmations (2026-10-01)

The owner reviewed section H and answered as follows. These are now decisions, not assumptions.

- **A-1 confirmed:** the CLI overwrites in place after a verified backup when no output flag is given (follows B3).
- **A-3 confirmed:** early previews default to Strict; Balanced is labelled experimental until the private golden set shows it meets its 1% silent-failure target.
- **A-4 confirmed:** a missed exit gate delays 1.0; shipping a feature as Experimental or moving it to 1.x is the owner's decision, made with measured numbers. Nothing is cut silently.
- **A-2, A-5, A-6, A-7, A-8, A-9, A-10, A-11, A-12 accepted as written.** A-8: the owner will build the private golden set over time (v0 about 150 images in M1, 500 before the ML gate, 800 before 1.0).
- **Hardware available (A-9):** a Windows touchscreen device, an Apple silicon Mac, an Intel MacBook, a Linux machine and a flatbed scanner. Intel Mac results can therefore be measured (still best-effort until 1.0, C1); the Linux machine has **no touchscreen**, so Linux touch gestures cannot be measured on real touch hardware: those cells stay UNMEASURED (A-9) and Linux touch remains best-effort (B8); mouse, trackpad and keyboard on Linux are still tested.
- **Copyright holder:** `WelFedTed` (LICENSE-MIT, LICENSE-APACHE; see B2).
- **Milestone order confirmed (2026-10-01):** M0-M13 as in ROADMAP.md.
- **Still open:** the owner's go-ahead to start M0 (ROADMAP item P.05).
- **Push to `main` (2026-10-01):** the owner pushes directly to `main` (no feature branches or PRs for the maintainer). ROADMAP item M0.07 was changed accordingly: the `main` ruleset blocks force-push and deletion only; CI runs on every push and failures are fixed forward. External contributors still use forks and pull requests.
