> Auto Crop design, part 2 of 8 | [PLAN.md](../../PLAN.md) | [Decision log](00-decision-log.md) | [ROADMAP.md](../../ROADMAP.md)  
> Planning draft, 2026-10-01. Numbers marked PROVISIONAL are unmeasured estimates; decisions B1-B21 and the assumptions A-1..A-12 live in the decision log and in section 1.

# 2. Architecture and engine design

This section fixes the structure the roadmap builds on: crates, data model, pipeline, the Tauri preview path, the write-safety design that overwrite-by-default (B3) demands, the batch engine, C-parser isolation, the CLI, and OS and filesystem mechanics. Numbers marked PROVISIONAL are unmeasured and get re-baselined by the benchmark harness and the Tier-M laptop spike. Where a decision overrides research, the text says so and names the mitigation.

## 2.1 Architectural invariants

Each rule below is testable.

1. **Purity.** Result pixels are a pure function of (original source pixels, `EditState`, engine version). File bytes also depend on `OutputSpec` and the encoder version.
2. **The original is never modified in place.** A user path changes only by atomic swap of a verified temp file, after a verified backup exists (B3). No `OutputSpec` value, setting or CLI flag can skip the backup or the verify step of an in-place write (property test over every `OutputSpec` field).
3. **One pixel path.** Rust renders every pixel the user judges; the webview only transforms a loaded proxy during a drag.
4. **Untrusted C parsers stay out of the GUI process** (B12, D3): libheif, libde265, dav1d and later PDFium and LibRaw run only in the worker, and libjpeg-turbo is the single accepted in-process C parser, behind pixel, allocation and time caps. Test: `cargo tree -i libheif-sys` lists only `worker`.
5. **Low confidence never writes** (B4); a failed detection leaves the original untouched.
6. **The core has zero UI dependencies** (B8), and nothing below `shell` touches the network (B18).

## 2.2 Workspace layout

```
auto-crop/                       github.com/WelFedTed/auto-crop  (MIT OR Apache-2.0)
├─ Cargo.toml                    workspace; edition 2024; resolver 3; panic = "unwind" everywhere
├─ rust-toolchain.toml  deny.toml  about.toml  REUSE.toml  native-deps.toml  .devcontainer/
├─ crates/
│  ├─ core/     model + ports, no I/O, no threads: EditState, History, JobState, HoldReason, ErrKind,
│  │            CancelToken, traits (Decoder, Encoder, Analyser, Renderer, HeicBackend, InferenceBackend)
│  ├─ imgproc/  pure pixel functions: proxies, warp, enhance, refinement, tiles          -> core
│  ├─ codecs/   codec impls: image, turbojpeg, jxl, tiff + G4 wrapper, encoders         -> core
│  ├─ worker/   bin auto-crop-worker: sandboxed decode helper; the only crate that links
│  │            libheif, and home of the OS HEIC backends                                -> core, codecs
│  ├─ engine/   Session, PreviewService, BatchScheduler, FsPlan, CommitWriter, BackupStore,
│  │            Store (rusqlite), Settings, AppPaths, WorkerPool         -> core, imgproc, codecs
│  ├─ cli/      bin auto-crop (clap)                                                    -> engine
│  ├─ eval/     bin auto-crop-eval: accuracy and timing harness (public)                -> engine
│  └─ shell/    bin AutoCrop: Tauri 2.x glue only                                       -> engine
├─ xtask/       dev tasks: build-native, check-native, check-models, roadmap-check, bench, eval,
│               i18n, vectors (not shipped)
├─ ui/          Svelte 5 + TypeScript (Vite)
└─ fuzz/  benches/  testdata/   (synthetic data and public-set fetch scripts only, B21)
separate repo: WelFedTed/auto-crop-models   training code, provenance log, ONNX + manifest
```

**Dependency rules**, enforced in CI:

- `core` depends on `serde` and `thiserror` only: no `image`, no `rayon`, no OS APIs.
- Only `shell` may have `tauri`, `wry`, `tao`, `gtk`, `webkit2gtk` or `slint` in its `cargo tree`. A CI script greps; `cargo-deny` bans them elsewhere.
- `imgproc` entry points take `Parallelism { Strips, Sequential }` (carried by `RenderReq` and `AnalyzeOptions`). `Strips` runs on rayon's ambient pool, so the caller's `pool.install()` picks preview or batch; `Sequential` runs on the calling thread. The scheduler chooses the level per job (2.8), so the two levels never nest.
- `shell` is thin: each `#[tauri::command]` forwards in one line. Anything the CLI could want belongs in `engine`. Its `updater` feature is compiled out of Store MSIX, Flatpak, winget-managed NSIS, distro and `no-hevc` builds (B18, 8.5).
- `worker` stays minimal (no `rusqlite`, Tauri or `ort`) to shrink the sandboxed surface. Its Cargo feature `hevc` is on in every official build (B12); off is the `no-hevc` variant for distro packagers or an owner-decided fallback, never an automatic one (2.9).
- `unsafe` is forbidden in `core` and confined to SIMD, `ffi/` and `isolation/` modules elsewhere.

**Edition and panic policy.** Edition 2024. No MSRV promise: `rust-toolchain.toml` pins the build toolchain and `rust-version` tracks it (8.2.3). `panic = "unwind"` in every profile, and CI fails on any `panic = "abort"`. Tauri's size guidance commonly suggests abort; we do not adopt it, because rayon aborts on panics it cannot propagate from `spawn` ([docs](https://docs.rs/rayon/latest/rayon/struct.ThreadPoolBuilder.html)) and per-item isolation relies on `catch_unwind`.

## 2.3 Data model

Edits are parametric; history holds no pixels. Pseudocode, not final signatures.

```rust
struct SourceRef {
    id: SourceId,                  // blake3 of file bytes
    path: PathBuf, size: u64, mtime: SystemTime, file_id: Option<FileId>,
    format: Format, dims: (u32, u32), exif_orientation: u8, icc: Option<Arc<[u8]>>, frame: u32,
    original: OriginalHandle,      // Path(p) until first save, then Backup(BackupId)   (see 2.7)
}
const EDIT_SCHEMA: u32 = 1;        // serde(default) everywhere; migrations are pure fns v(n) -> v(n+1)
struct EditState {
    schema: u32,
    orientation: Orient,           // quarter-turns + mirror of the WHOLE image, on top of EXIF (EXIF applied once,
                                   //   at decode); used only for Identity geometry or convert-only jobs.
                                   //   Per-item turns live in the item's geometry (QuadWarp, GridWarp)
    items: Vec<Item>,              // 1 normally; N for multi-item scans (B1); [] = whole image (convert-only)
    margin: MarginPolicy,
    enhance: Enhance,              // default for all items
}
struct Item {                      // M10 adds order and split as additive fields
    id: ItemId,                    // stable across edits and undo
    include: bool,                 // drop a false detection without deleting it
    geometry: Geometry,
    enhance_override: Option<Enhance>,
    origin: Origin,                // Auto{pipeline_ver, detector_a: ModelRef, detector_b: ClassicalVer}
                                   //   | Manual | AutoThenEdited        (provenance; render ignores it)
    confidence: Option<Confidence>,   // score, band Good|Check|Failed, reasons: Vec<HoldReason>, signal vector,
                                      //   calibration_ver (provenance; render ignores it)
}
enum Geometry {
    Identity,                      // convert-only, or keep whole image
    Quad(QuadWarp),                // homography from 4 normalised corners in EXIF-oriented source space
    Grid(Arc<GridWarp>),           // dense dewarp (B10): outline + cols x rows source-space nodes + ModelRef;
                                   //   carries its own quarter_turns and mirror like QuadWarp
}
struct QuadWarp {
    corners: [Pt; 4], quarter_turns: u8, mirror: bool,    // per item: orientation runs on each rectified item (4.2 S7)
    fine_deg: f32, aspect: Recover|Keep|Fixed|Free, size: Auto|LongEdge|Dpi,
}
enum MarginPolicy {
    PaperEdge { margin: f32 },     // B14 default, resolved per route (Assumption A-11, 4.6): quad 0 plus a 1 px
                                   //   inward guard, bed +0.5% of the shorter side (at most 12 px at 12 MP),
                                   //   border trim 0; PROVISIONAL
    ContentTight { bbox: Rect, pad: f32 },   // ink bbox computed once on the rectified proxy, then stored
    None,
}
struct Enhance {                   // B13; fields and semantics are defined in 5.2 and 5.5
    mode: Original | Auto | Gray | BW,        // default Original: enhancement is suggested, never forced
    look: Receipt | Document,
    offsets: [i8; 5],              // bright, contrast, darkness, cleanup, sharp; -100..100, 0 = the value in `est`
    bw: Sauvola | Nick,            // Normal or Faded preset
    despeckle: bool,               // separate Advanced switch: off for receipts, on by default only in the Document
                                   //   look; Clean-up (colour and noise) never touches it
    advanced: Overrides,           // the 5.12 parameters, keep paper tint, keep colour marks, CLAHE
    est: Option<Estimates>,        // stored per item: algo_ver, text_h, black point, white point, gamma, paper colour
}                                  // no bit depth here: 1-bit is an OutputSpec export option
fn render(src: &Raster, st: &EditState, req: &RenderReq) -> Vec<Raster>;   // pure; one Raster per included item
impl EditState { fn render_hash(&self) -> u64 }        // render-relevant fields only

struct OutputSpec {                // per batch, overridable per image; not part of EditState
    format: Jpeg | Png | Webp | Avif | Tiff | Pdf | Jxl | KeepSource,   // KeepSource resolves per 3.2.3; FsPlan decides replaceability (2.7)
    quality: MatchSource{floor, cap} | Fixed(u8) | Lossless,
    bits: Eight | One{ bilevel: Png | TiffG4 },             // 1-bit is an export option (B13), never part of EditState
    colour: Srgb | PreserveWideGamut,                       // C2; sRGB for HEIC/HEIF to JPG and enhanced output,
                                                            //   every other source keeps its profile and pixels (3.7)
    metadata: Keep{ strip_location: bool } | Strip,
    target: InPlace | Copy{ dir, template },                // B3: default InPlace; dir = `<source folder>/AutoCrop/`
                                                            //   or a Rust-picked folder id (2.5), never a webview path
    collision: Rename | Skip | Replace,                     // UI "Keep both" = Rename; Replace sends the existing
                                                            //   unrelated file through the same backup-then-swap
    verify: Full | Fast,                                    // no Off; not a setting (2.7)
    lossless_jpeg: Auto | Never,  keep_mtime: bool,
}
```

- **N quads from day one.** Multi-item splitting (B1, B10) is an ordinary edit. A multi-item scan renders N rasters, named by the template `{name}_{n}` (`{name}` for one-to-one) and written as a group commit (2.7). Each item carries its own quarter turn and mirror, so two receipts lying at different angles on one scan are each made upright (4.7). Outputs are a list from the first release (`Saved.outputs`, the `backup_outputs` table and `outputs[]` in `manifest.json` of 2.7, with N = 1 for ordinary rows), so 1-to-N later needs only an additive migration of the safety-critical store.
- **Dewarp needs no schema break.** `Grid` holds tens of nodes per side (exact size unverified, about 10 KB, shared via `Arc`), cached by (source id, model version, outline hash) so a crop change re-renders without re-inference. Dragging a grid item's outline previews as a homography approximation; Rust re-renders on release with a progress indicator (inference about 0.4 to 1 s, PROVISIONAL).
- **Render never re-analyses.** Analyser output that render consumes (content bbox, dewarp grid, the per-item enhancement estimates `est`) is stored in `EditState`, so output stays a pure function of (source, `EditState`, engine version) and export reuses the preview's estimates. The enhancement illumination grid is recomputed deterministically and never stored; a bump of `est.algo_ver` re-estimates and the item shows "result changed after update". Analysis caches are disposable.
- **The source is always the original.** After the first in-place save the file at `path` is the output, so `original` flips to `Backup(id)`; otherwise re-editing would apply the crop twice. If that backup has expired or been purged, the item opens read-only ("original expired") and re-save is refused, because re-editing the processed output would compound the crop.

### Job state machine

```rust
enum JobState {
    Queued, Probing, Decoding, Analysing, Triage,                 // transient; journalled in batches
    HeldForReview { reason: HoldReason },                         // nothing written; the registry is in 2.8
    DetectionFailed { reason: Option<HoldReason> },               // original untouched; "Draw crop" banner (B4)
    Approved { by: Auto | User },
    Rendering, Encoding, Verifying, BackingUp, Committing,        // journalled durably
    Saved { outputs: Vec<PathBuf>, backup: Option<BackupId> },
    Skipped { why: User | AlreadyProcessed | NotReplaceable(NoticeCode) },   // source untouched (2.7)
    Failed { kind: ErrKind }, Cancelled,
}
```

```
Triage -> Approved(Auto)        band Good (score >= t(mode)) and no hold reason
Triage -> HeldForReview         band Check (0.60 <= score < t(mode)), or a hold reason fires
Triage -> DetectionFailed       band Failed (score < 0.60 or a hard gate such as an implausible quad), or no quad
HeldForReview -> Approved(User) user accepts (optionally after adjusting)
DetectionFailed -> Approved(User)  user draws a crop;   either held state -> Skipped on skip
Probing -> Skipped(NotReplaceable) FsPlan will not replace this source (2.7); the user may opt in to a copy
Skipped(User) -> HeldForReview | DetectionFailed   un-skip returns to the state the item left
Approved -> Rendering -> Encoding -> Verifying -> BackingUp -> Committing -> Saved
Saved -> Approved(User)         a later edit re-renders from Backup(id) and commits again through 2.7
any state before Committing -> Failed | Cancelled      (original provably untouched)
```

Convert-only jobs skip `Analysing` and `Triage`. Restore original is a separate operation on the backup record (2.7), not a job state.

## 2.4 Pipeline stages

Five stages, each behind a `core` trait, so the CLI, GUI and benchmark harness run the same code.

| Stage | Contract | Notes |
|---|---|---|
| Decode | bytes to `Raster` (Full or Scaled) plus `Probe` (dims, orientation, ICC/nclx) | Bytes only, never paths. Header probe and pixel cap precede any allocation. EXIF/`irot` applied once. |
| Analyse | proxies to suggestion (items, confidence, orientation, content bbox) | Classical plus ONNX nets on the 1024 px proxy, then full-resolution sub-pixel corner refinement. Refinement replaces the first overlay unless the user already touched a handle. |
| Edit | suggestion plus gestures to `EditState` | Pure data, no pixels. |
| Render | (source, `EditState`, region, scale) to `Raster` | One resample: orientation, homography or grid, and margin composite into a single warp from source pixels; enhancement follows. |
| Encode | `Raster` plus `OutputSpec` to bytes | Metadata transcoded explicitly: Orientation reset, HEIF Exif TIFF-offset prefix stripped, stale thumbnail dropped, ICC kept. The lossless JPEG path bypasses Render and Encode. |

Encoders take only our own pixels, so they run in-process; only decoders of untrusted bytes are isolated. Metadata blobs from the source (ICC, EXIF, XMP) reach an encoder only after our own validation and the 3.7 ICC pre-checks, or they are dropped with a Warn; a C encoder never receives raw source bytes.

**Budgets (PROVISIONAL, D5; 7.1 owns the numbers and Table A the per-stage split).** One 12 MP image that exits at tier T1 takes at most 700 ms p50 end to end including verification (crop-only 580 ms; B&W or Faded enhancement costs up to 250 ms instead of the 120 ms of Auto and Grayscale, at most 830 ms in total). Proxy analysis takes at most 40 ms p95 over T1 exits (over a mixed set, p50 at most 40 ms and p95 about 290 ms); the first overlay at most 150 ms (a HEIC at most 700 ms at 12 MP). Throughput: the CLI at least 4 images per second on 6 cores (`cores - 1` workers), a GUI batch at least 3, HEIC to JPG at least 2.5 (5 is a non-gating stretch). The research's stage figures summed to about 300 ms before verification and are not used.

## 2.5 Preview architecture (Tauri)

Rust decodes everything; HEIC and RAW never reach the webview, and full-resolution pixels are never sent to it (D1).

| Level | Size | Built from | Use |
|---|---|---|---|
| thumb | 256 px | embedded thumbnail, else scaled decode | grid, filmstrip; persisted in SQLite as small JPEG |
| detect | 1024 px | first decode | nets and classical detector; dropped after analysis |
| analysis | about 1.5 MP by area, long edge at most 3072 px | first decode | illumination map, enhancement parameters; LRU |
| display | 2 to 4 MP | first decode | main viewer, served to the webview |
| tiles | 512 px | lazy full decode | zoom past the display proxy to 1:1; byte-weighted `quick_cache`, PROVISIONAL 256 MB |

All levels come from one decode via `fast_image_resize`, converted to sRGB once. An interactive JPEG open paints first from `turbojpeg` DCT-scaled decode while the full decode continues at low priority; that first overlay is a draft. The score used for triage always comes from the one canonical proxy derivation shared by GUI, CLI and batch, which replaces the draft when ready, so a file cannot be Good in one front end and Check in another.

**Custom URI scheme.** The shell registers an asynchronous handler. Tauri serves it as `acimg://localhost/...` on macOS and Linux and `http://acimg.localhost/...` on Windows ([docs](https://docs.rs/tauri/latest/tauri/struct.Builder.html)); one UI helper builds URLs: `acimg://localhost/<launch-token>/<item-id>/<level>[/<z>/<x>/<y>]?g=<gen>` (Windows form `http://acimg.localhost/...`). The launch token is 128 random bits per launch. The handler parses strictly, maps no URL to a filesystem path and serves only engine caches by opaque id, so the webview has no file-read primitive (headers and limits in 8.6.4). Default payload is JPEG q about 88, 4:4:4; the 500-image tile spike decides between JPEG, WebP and raw RGBA over an IPC `Channel`.

**Drag versus release (D1).** This replaces the perf report's GPU homography drag preview, which would need a second implementation of the warp that drifts from Rust.

```
pointerdown  ui holds the SOURCE layer (oriented, uncropped display proxy), already loaded
pointermove  ui recomputes a 3x3 homography from the 4 handles, sets CSS matrix3d on the source layer;
             SVG overlay updates in the same frame; NO IPC
pointerup    set_edit{image, base_gen, patch, gesture, phase:"end"} -> History.commit (coalesced),
             gen += 1, RenderReq(Final, gen) on the preview pool -> event render-ready{image, gen, url}
ui           img.decode(); if gen is current, cross-fade the RESULT layer over the source (~150 ms); else drop
sliders      set_edit{phase:"update"}, one request in flight, latest wins; Rust reruns cheap stages on cached
             maps. PROVISIONAL: slider tick <= 16 ms compute on the 2 MP display proxy + ~50 ms round trip
```

The TypeScript homography is the one duplicated piece of geometry. It drives only the transient drag view and is checked against Rust by shared vectors (`xtask vectors`; both sides tested to 0.05 px on the 2 to 4 MP display proxy), and the cross-fade hides residual differences. There is no GLSL preview and no CSS colour approximation, so previews cannot drift from exports. Interactive budgets (7.1, PROVISIONAL): drag frame median at most 17 ms and p95 at most 20 ms; release to final render p50 at most 150 ms and p95 at most 250 ms.

**Generations and cancellation.** Each committed change bumps a per-image `gen`. Requests carry `gen` and a `CancelToken`; workers check `cancel || latest != gen` every 64 rows. A new request for the same (image, level) replaces a queued one, results dedupe by `render_hash`, and the UI drops results older than current.

**Gestures.** Pointer Events with `touch-action: none` and a small two-pointer pan, pinch and rotate recogniser. The SVG quad overlay draws 16 px handles with a 48 px nominal hit area that is never below 44 px; other controls are at least 24 px. macOS adds WebKit `gesture*` events. wry already disables WebView2 pinch page-zoom, so Windows needs nothing more. Linux pinch stays with GTK (wry#544, tauri#13115, both open), so Linux touch is best-effort with documented limitations (B8), never "unsupported": a `with_webview` shim, spike measurements, and the documented NVIDIA and DMABUF workarounds ([docs](https://v2.tauri.app/develop/debug/linux-graphics/)) behind `--safe-graphics`. Native drag-drop must stay on to receive paths, which disables HTML5 drag-and-drop on Windows, so thumbnail reordering uses pointer events.

**Commands** (small JSON in, no pixels out; every file is an opaque id registered by Rust): `pick_files`, `pick_folder` and `pick_save_location` (Rust-side dialogs), `get_image`, `set_edit(id, base_gen, patch, gesture, phase)`, `undo`, `redo`, `session_undo`, `session_redo`, `apply_to`, `redetect`, `start_batch`, `pause_batch`, `resume_batch`, `cancel_batch`, `set_preset`, `accept`, `skip`, `save_all`, `list_backups`, `restore`, `delete_backup`, `reveal_in_folder`, `get_settings`, `set_settings`, `diagnostics_report`, `open_project_link`, and `check_for_updates` (updater builds only). Dropped files (`WindowEvent::DragDrop`) and OS open-requests (2.13) register their paths in Rust and reach the UI as ids through `image-added`. A raw-path `open_paths` exists only behind an `e2e` cargo feature, for the scripted GUI tests, and is absent from release builds.

**Events** (`emit` at low rate; `tauri::ipc::Channel` for progress, coalesced to 10 Hz): `scan-progress`, `image-added`, `analysis-ready`, `render-ready`, `job-state` (with notices), `batch-progress`, `history-changed`, `backup-changed`, `notice`, `error`, `open-request` (2.13).

**Hardening (8.6.4).** The webview holds no plugin permission: no `fs`, `shell`, `http`, `process`, `opener` or `dialog`. The pickers, the save dialog, drag-drop and open-requests run in Rust, register the paths and return opaque ids; every command takes ids and range-checked types and validates ids against the journal. A compromised webview (hostile EXIF text is a listed threat) therefore cannot name a file, cannot call `delete_backup` or `set_settings` with a path of its choosing, and cannot aim overwrite-by-default at an arbitrary file. `Copy{dir}` is a Rust-picked folder id or `<source folder>/AutoCrop/`; `set_settings` validates the backup location in Rust; one Rust command, `open_project_link`, opens links and accepts only the project's GitHub URL prefix. The CSP is the full object in 8.6.4 (`default-src 'none'`, no remote origin). Negative IPC tests (absolute paths, `..`, foreign ids, forged `delete_backup` and `set_settings` calls) belong to the hardening suite.

## 2.6 Undo and redo

Undo is a cursor over immutable snapshots: `History { entries: Vec<Entry{ state: Arc<EditState>, label, gesture }>, cursor }`, capped at 200 entries per image (PROVISIONAL). A snapshot is about 1 KB, so undo costs a cursor move plus a re-render. It is about 150 lines of our own code; the `undo` and `imbl` crates add nothing here.

- **Coalescing.** `commit(state, label, gesture)` replaces the top entry if `gesture` matches, else truncates the redo tail and pushes. A gesture ends on `phase:"end"` or after 500 ms idle for sliders and keyboard nudges (PROVISIONAL). Commits with an unchanged `render_hash` are discarded.
- **Baselines and memory.** Entry 0 is the analyser's result; "Reset to auto" and "Reset to original" are ordinary, undoable commits. Idle histories flush to SQLite.
- **Session actions** (apply to selection, remove, change enhancement for all) use command plus memento, restoring every affected cursor in one step.
- **Scope, stated in the UI.** Ctrl/Cmd+Z covers edits only. File writes are reversed by **Restore original** (2.7), a separately labelled control. After "Save all", a persistent toast offers "Restore all originals from this batch".
- **Persistence.** Written behind to SQLite (`history_entries`) on gesture end; unfinished batches and held items trigger "Resume previous session?" on launch.

## 2.7 Overwrite-by-default safety design (B3)

The research default was "save a copy"; B3 overrides it. Two properties hold at every instant: the path holds either the original bytes or a fully verified output, and a verified, durable backup of the original exists before any swap. This section is the single authoritative statement of the commit protocol; 8.6.3 cites it and adds nothing to it.

### Commit protocol (per file, in place)

| # | Step | Journal | If the process dies here |
|---|---|---|---|
| 1 | **Plan**: names collision-checked across the whole batch; free space checked (2x headroom); source identity snapshotted (size, mtime, file id, blake3); read-only, symlink, placeholder and sync-root status checked (a sync root warns and recommends Save as copy) | Planned | nothing changed |
| 2 | **Encode** to `.autocrop-<ulid>.tmp` **in the target directory** (stray `.autocrop-*.tmp` files are swept); apply mtime, mode and xattrs; `sync_all` | Writing | orphan temp, swept at start |
| 3 | **Verify the temp**: re-read it and compare its blake3 with the encoder output (catches truncation and bit flips); then re-decode and check dimensions, Orientation and ICC; exact pixel hash for lossless formats, else a 64x64 luma fingerprint with error at most 3/255 (PROVISIONAL) | Verified | same |
| 4 | **Back up**: reflink, else hardlink, else copy (re-hash, then `sync_all`); write and fsync `manifest.json`; fsync the backup directory and `backups/` (Windows: `FlushFileBuffers` on the files; directories rely on NTFS journalling); insert the `backups` row on a `synchronous=FULL` connection; only then journal BackedUp | BackedUp | harmless extra backup |
| 5 | Journal `Committing{temp, target, expected hashes, source fingerprint}` | Committing | roll forward (Crash recovery, below) |
| 6 | **Re-stat the source**: if size, mtime or file id changed, abort with `SourceChanged`, keep the backup and write no copy. Otherwise **swap**: `rename(2)` on Unix, `ReplaceFileW` (fallback `rename`) on Windows, with a retry ladder of 10 to 640 ms and then a retryable `FileInUse`. On errors 1175 to 1177, re-stat the target: missing means rename the verified temp into place; still the source means retry | | target is original or output |
| 7 | Unix: fsync the parent directory. Journal `Saved` with `output_hash`. No backup is deleted before `Saved` | Saved | |

**Ordering and durability.** Everything recovery relies on is durable before the swap: the verified temp (step 2), the backup data, `manifest.json`, the directory entries of the backup folder and of `backups/`, and the `backups` row (step 4), then the `Committing` row (step 5). Every journal and `backups` write uses a `synchronous=FULL` connection, and a state is journalled only after the work it names has completed. A copy made by the copy method (forced on other volumes, FAT, exFAT, network shares and after any link failure) is `sync_all`-ed, because a re-hash alone does not make it durable. Reflinks and hardlinks have no data blocks of their own to flush, so their durability rests on the directory fsync and the filesystem's journal ordering, which the plumbing spike verifies per filesystem. Windows has no directory fsync: directory entries rely on NTFS journalling plus the start-up reconcile.

**Verification modes.** `verify` is `Full` or `Fast`. There is no `Off`, it is not a setting, and no `OutputSpec` value, CLI flag or environment variable can skip verification or the backup for an in-place write (2.1). Both modes do the blake3 re-read and every check of step 3 that the format allows. `Full` re-decodes at full size; `Fast` re-decodes at reduced scale where the codec offers it (JPEG at 1/8 through turbojpeg, which still runs the full Huffman stream, so the whole entropy stream is still checked; cost to be measured) and checks dimensions, Orientation, ICC and the luma fingerprint. For lossless formats the two are identical. Which one is the default follows the verify-stage cost measured in the benchmark baseline (2.17).

**If the source changed (TOCTOU).** The step-6 re-stat aborts with `SourceChanged`: the temp is deleted, the original stays as it is, the backup is kept and no copy is written. The re-stat narrows the window but cannot close it, because no portable compare-and-swap exists; an edit landing between the re-stat and the swap is overwritten, and Restore returns the version that was backed up.

**Why not `persist()` alone.** `NamedTempFile::persist` replaces atomically but syncs neither contents nor the parent directory ([docs](https://docs.rs/tempfile/latest/tempfile/struct.NamedTempFile.html)), so `tempfile` only creates the file. `atomic-write-file` is rejected: it preserves no timestamps, ACLs or xattrs.

**Windows swap.** `ReplaceFileW` preserves creation time, DACLs and named streams (including `Zone.Identifier`), requires one volume, and can fail part-way with errors 1175 to 1177 ([docs](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-replacefilew)); step 6 says what happens then, since the target may be missing or still the source and the backup already exists, so nothing is lost. `std::fs::rename` gets POSIX semantics on Windows 10 1607+ since Rust 1.85 (PR [#131072](https://github.com/rust-lang/rust/pull/131072)) but keeps neither DACL nor creation time. The plumbing spike compares both on NTFS, ReFS, exFAT and a network share; until then `ReplaceFileW` on NTFS and ReFS, `rename` plus explicit metadata copy elsewhere. Sharing violations from antivirus, indexers and thumbnail caches retry at 10, 20, 40, 80, 160, 320 and 640 ms (PROVISIONAL), then fail as retryable `FileInUse`.

**Metadata.** mtime is kept by default (file managers sort by it); Unix mode and xattrs (Finder tags included) copy via `filetime` and `xattr` (Unix-only); birth time is best-effort. A read-only source is held for a user choice.

**Crash recovery, at start.** Rows in `Planned` to `BackedUp`: delete the temp and re-queue; the original is untouched. Rows in `Committing`: if the temp has the expected hash and the target is the source or missing, roll forward; if the target equals the output, mark `Saved`; otherwise mark `Failed(SourceChanged)` and keep the backup, deleting nothing of the user's. The CLI never re-processes earlier items (it only reconciles them). A 1-to-N group rolls forward when every output verifies against the journalled hashes; otherwise it rolls back to the original plus its backup, never leaving a partial set, and the source is re-stat'ed right before it is unlinked. Temp sweeping covers only directories named in the journal or currently open. A fault-injection suite asserts the invariant that the target is always the original or a verified output: `fail` failpoints at every step, `kill -9` and `TerminateProcess` runs, the `FaultFs` power-loss model (it drops data not yet `sync_all`-ed and un-fsynced directory entries, the backup copy, `manifest.json` and backup directories included) and an injected error 1176 (target missing, verified temp present).

### Backup store

```
<app-data>/AutoCrop/      Windows %LOCALAPPDATA%\AutoCrop; macOS ~/Library/Application Support/AutoCrop;
  library.db  logs/  crash/         Linux $XDG_DATA_HOME/auto-crop (settings.toml in $XDG_CONFIG_HOME)
  backups/<ulid>/         time-sortable id
      original.<ext>      reflink, hardlink, copy, or moved-in source
      thumb.jpg  manifest.json   manifest: original path, hashes, times, mode, edit state, engine version, outputs[]
```

The CLI and the GUI share this one store: `shell` passes the engine's `AppPaths` and neither resolves paths itself, so a backup made by one restores from the other. `library.db` is an index; each directory is self-describing, so the store survives database loss. The `backups` table holds id, created_at, kind (`OneToOne`, `OneToMany` or `ManyToOne`), original path (raw bytes) with size, mtime and blake3, attributes, method, batch id, edit state, restored_at and purge_after. Outputs are a list from day one: a `backup_outputs` table (backup id, path, blake3) mirrors `outputs[]` in `manifest.json`, with N = 1 for ordinary rows. The store is user-only (a user-only ACL on Windows, mode 0700 on Unix), both files are schema-versioned (a newer schema is refused), and every release must open, migrate and restore from stores written by any earlier release.

- **Methods.** Reflink (`reflink-copy`) beats hardlink beats copy. Links work only on the same volume and the store defaults to app data, so other drives and network shares always copy. Hardlinks are safe because we never write through an existing inode.
- **Location.** Configurable; never defaults into a cloud-synced folder.
- **Store (MSIX) builds.** *Assumption A-6: The Store build is verified early by a hidden dry run (M6.81) and keeps backups outside virtualised package data; a Store-only `no-hevc` build or any fee needs the owner's decision (B12, B15). (owner may veto)* A packaged app's writes under `%LOCALAPPDATA%` are redirected into the package's private data, which uninstalling deletes; keeping `backups/` or `library.db` there would silently defeat "Restore original works after closing the app". The Store build therefore uses a real user-folder default chosen at first run, or the `unvirtualizedResources` capability if certification allows it. This is resolved early, in the plumbing spike (2.17), by sideloading a throwaway MSIX, uninstalling it and observing what survives; each package variant sets its own backups directory default, and `doctor` prints the real paths. On every channel uninstall, reinstall and Restore must work: Homebrew's `zap` never touches the store, and uninstallers ask before removing app data and default to keeping it.
- **Retention.** 30 days default (B3); options 7, 30, 90 and 365 days or Never, plus a **Keep** pin per run. A purge runs at start, every 24 h and on **Purge now**, and deletes only files inside the store (canonicalised prefix check; symlinks and junctions in the store are refused). The Backups panel shows usage, warns at 10 GB used or under 5 GB free on the store volume (PROVISIONAL) and flags entries expiring within 7 days.

### Restore original

Works after saving and after closing the app: per image, per batch, from the Backups panel, or from the CLI. Find the backup by id or current path and compare each file at a recorded output path with its blake3 in `backup_outputs`. If equal, replace without a prompt. If it changed since we saved, offer: restore anyway (the current version goes into a new pre-restore backup), restore as a copy, or cancel. The restore itself writes a temp, verifies the original's recorded blake3 and swaps by the same protocol, so it is atomic and reversible, and it restores mtime, mode and attributes. For a conversion, the original returns to its path and the converted file moves into a pre-restore backup. For a 1-to-N split the scan returns to its path and Restore offers **Keep** or **Remove** for the derived files; Remove moves them into a pre-restore backup, so it is reversible, and files that changed since we wrote them default to Keep. The file an N-to-1 merge created is handled the same way.

### What may be replaced (one rule, `engine::FsPlan`)

A source is replaced in place only if it is a single-frame image **and** this build can write its default output (3.2.3) without dropping content. `engine::FsPlan` is the only place that decides this; the GUI, the CLI and every batch path ask it.

- **Replaced, with backup:** JPEG, PNG, TIFF and lossless WebP (same format); HEIC and HEIF to JPG (the source goes to the backup store); BMP, static GIF, ICO, TGA, PNM, QOI and HDR to PNG (the B3 conversion).
- **Never replaced:** animated GIF, APNG, animated WebP, AVIF and JXL; multi-page TIFF; HEIF with several top-level images or sequences; EXR, SVG, PDF, RAW, PSD and JPEG 2000.
- **Not replaced until the writer ships:** 1-bit TIFF and PNG, lossy WebP, AVIF and JXL.

Otherwise the source stays byte-identical and the item is `Skipped(NotReplaceable(reason))` in batches and in the CLI default mode, with the notice `anim.first_frame_only`, `tiff.multi_page`, `heic.multi_image`, `heic.sequence` or `format.write_unavailable`. The user may opt in per file to write a copy (GUI: `<source folder>/AutoCrop/`; CLI: `--suffix` or `--output`); PNG is the copy format for AVIF, JXL, EXR and RAW. Test: a folder holding one file of each case leaves every original byte-identical under the default mode.

### Conversion replaces the source (B3, B12)

`IMG_0001.HEIC` becomes `IMG_0001.jpg`. Order: encode and verify the temp; back up the source (link or copy, so it stays put); commit the output by **no-clobber** rename; only then unlink the source path. Each step is journalled; a crash between the last two leaves both files, and recovery finishes the unlink after re-checking the backup hash. If another file already holds the target name (an Apple "keep originals" pair, say), the collision policy renames to `IMG_0001 (2).jpg`. A Live Photo's paired `.MOV` is never touched, and the UI and CLI say so (B12).

- **1-to-N (a multi-item split).** *Assumption A-2: A 1-to-N split follows B3: outputs take the scan's place and the scan moves to Backups ("Keep the scan" is a setting). Multi-frame sources and formats this build cannot write back are never replaced; results are new files. (owner may veto)* It is the same flow as a group: verify all temps, back up the scan, N no-clobber renames, then unlink the source; the group's recovery rule is in Crash recovery above. "Keep the scan" is off by default and leaves the scan in place beside the outputs.
- **N-to-1** (images combined into one PDF, say) writes a new file and leaves the sources untouched unless the run's "Move sources to Backups" box is ticked; the `backups` kind is `ManyToOne`.

### Lossless JPEG path (avoids generation loss)

`lossless_plan(&EditState, &SourceInfo)` applies when source and output are JPEG, the enhancement mode is Original, there is no resize or perspective, `fine_deg` is under 0.05 (PROVISIONAL), and the geometry is a 90-degree step, a flip or an axis-aligned crop. It runs `turbojpeg::Transform` with no re-encode.

- Crop origins snap **outward** to the iMCU grid (8 or 16 px), so no content is lost but the output may grow ([jpegtran](https://github.com/libjpeg-turbo/libjpeg-turbo/blob/main/doc/jpegtran.1)). Accept growth up to 16 px or 0.5% of the short side (PROVISIONAL), else re-encode.
- Use `perfect`: if dimensions are not iMCU-aligned, re-encode at quality matched to the source (estimated from its quantisation tables) rather than silently trimming.
- Rewrite the EXIF Orientation tag to 1 in place after a physical rotation. Everything else re-encodes; the UI shows a "lossless" badge when the path applies.
- The vendored libjpeg-turbo 3.1.0 has a `tj3Transform` double-free fixed in 3.1.4, so we override it and CI asserts the linked version.

### Save as copy, first-run explainer, idempotency

- **Save as copy** is a Settings toggle and a per-batch choice. GUI copies go to `<source folder>/AutoCrop/` (or a Rust-picked folder id, 2.5). Template tokens `{name}`, `{ext}`, `{n}`, `{date}`, with the defaults `{name}` (1-to-1) and `{name}_{n}` (1-to-N); one grammar, one sanitiser (illegal characters are replaced) and one collision key (NFC plus case-fold on every OS) live in `FsPlan`; collision `Rename`. Same temp, verify and atomic rename; no backup is needed unless `collision = Replace` overwrites an existing file, which then takes the full backup-then-swap path.
- **First-run explainer**, not a blocking dialog: an expanded "How saving works" card on Home (6.2.1) plus a one-time first-write sheet, shown again (with the file count and total backup size) the first time a batch would overwrite: "Auto Crop saves changes over your original files. Before it does, it keeps a copy of each original for 30 days in `<path>`. You can restore any file from Backups at any time." Buttons: "Replace originals" and "Save copies instead".
- **Idempotency guard.** Re-running a batch on a processed folder would crop cropped files, so each file's blake3 is looked up in `backup_outputs.blake3`; a hit yields "Already processed" with Skip (default), Re-process from original, and Restore. An EXIF `Software` tag ("Auto Crop x.y") only raises a chip, since the database may be lost or elsewhere.
- **Preflight.** Backup copies (except same-volume links) plus estimated outputs need 2x headroom (PROVISIONAL); auto-pause below 500 MB free.
- **No backup, no overwrite.** If a verified backup cannot be made (disk full, store unwritable), nothing is written: a GUI run pauses and offers to free space, change the location or save copies for the rest; the CLI exits 6 before any write (2.12).

## 2.8 Batch engine

**Pools.** Two rayon pools when a GUI exists. Preview: `clamp(cores/4, 2, 3)` threads at normal priority. Batch: `max(1, physical_cores - 1)` workers in the CLI and headless builds, and `max(1, physical_cores - preview)` in the GUI, where the preview pool takes its share (so the GUI batch floor is lower than the CLI's, 2.4). Batch threads run at below-normal OS priority (`thread-priority`). The dispatcher uses `pool.spawn`, so each job body runs in `catch_unwind` (mapped to `Failed(InternalPanic)`) and the pool has a `panic_handler`; `spawn` aborts without one, and nothing here covers C segfaults (hence 2.9). ONNX intra-op threads are set to 1 or 2 to avoid oversubscription.

**Parallelism.** Every dispatched job carries a `Parallelism` (2.2). While the queued jobs number at least the workers, jobs run `Sequential`: one image per worker, single-threaded stages. A lone job (a single-file CLI run, the last images of a batch) runs `Strips`, strip-parallel on the batch pool; interactive edits run `Strips` on the preview pool. The two levels are never nested. The wall-clock figures of Table A (7.1) assume `Strips` for a lone image; batch throughput assumes `Sequential`.

**Byte-weighted memory budget.** A weighted semaphore with `cap = min(25% RAM, 4 GiB, 50% of free RAM)` (PROVISIONAL). Job weight is `pixels x 9 + 64 MiB` (about 3x decoded RGB8 plus overhead): about 175 MB at 12 MP, about 1 GB at 100 MP. A job heavier than the cap **waits until nothing else runs and is then admitted alone**, so one big file cannot deadlock the queue. Above `max_pixels` (default 100 MP, C1) a job fails fast with a clear message naming the size and the setting; the Advanced setting, `--max-pixels` or "Allow this file" raises the cap to at most 500 MP, which the worker refuses to exceed regardless. Hostile headers are rejected within 1 s and 64 MB.

**Bands and hold-for-review gating (B4, B5, B6).**

- **Bands.** Good = calibrated score at least t(mode); Check = 0.60 up to t(mode); Failed = below 0.60 or a hard gate. An implausible quad (non-convex, an interior angle outside 55 to 125 degrees, or an area outside 5 to 99.5% of the frame; thresholds PROVISIONAL) is Failed, never a hold. Good is auto-saved (as the commit mode below allows); Check is held, listed first, with the original untouched; Failed leaves the original untouched and shows the "Draw crop" banner. The UI shows icon plus word, never a raw score.
- `Approved(Auto)` needs band Good and no hold reason; a hold reason caps an item at Check whatever its score. **`HoldReason` is one registry** (next table). Notices (Live Photo video not converted, gain map or depth dropped, HDR tone-mapped, GIF first frame only) are Info or Warn chips and never hold an item: nothing waits for an acknowledgement, since that would stall HEIC batches (B12).

| Group | Codes | Effect |
|---|---|---|
| Detection (4.9) | `DETECTORS_DISAGREE`, `WEAK_EDGE`, `PARTIAL_FRAME`, `ORIENT_UNSURE`, `NO_DOCUMENT`, `RESIDUAL_SKEW` (post-warp skew above its threshold) | Check |
| Detection failure | `NO_QUAD` | explains a `DetectionFailed` item |
| Runtime | `ML_UNAVAILABLE` (classical-only mode caps every result), `ANALYSIS_LIMIT` (analysis time cap hit) | Check |
| Batch (2.8) | `BATCH_OUTLIER` (armed at 20 or more items; off in `Streaming`) | Check |
| Multi-item (4.7) | `TOUCHING_ITEMS`, `OVERLAPPING_ITEMS`, `SPLIT_UNSTABLE`, `ITEMS_TOO_CLOSE` (clear gap under 1.5% of the shorter side), `LOW_CONTRAST_EDGE`, `ODD_ASPECT`, `TOO_MANY_ITEMS`, `BED_UNCERTAIN` | Check |
| Readability (5.11) | `FADED_PRINT`, `INK_COVERAGE`, `LOST_MARKS`, `ESTIMATE_UNRELIABLE` (the four risk signals) | Check |
| Dewarp (4.8) | `DEWARP_UNCERTAIN` | Check |
| Dewarp chips (4.8) | `DEWARP_REJECTED`, `DEWARP_NEAR_FLAT`, `DEWARP_NOT_APPLICABLE`, `DEWARP_MODEL_MISSING` | Info chip: the homography-only result stands |
| Curvature (4.8) | `CURVED_PAGE_SUSPECTED` | Info chip, never changes the band |

UI and CLI copy is looked up by code and never built from English in `core`: each code owns the Fluent keys `hold.<code>.title`, `.cause` and `.action` (`<code>` in lower case; 6.7 owns the wording), and a CI test fails on a code without keys or without a "Why was this held?" entry.

- **Presets move only the Good cutoff.** Strict, Balanced and Aggressive choose t(mode); the Failed floor is 0.60 in every mode. The cutoffs below are interim starting values, used until `calibration.json` exists; it ships with the models, so recalibration needs no code change, and is fitted on the unlocked real dev tier plus the public SmartDoc, MIDV and CORD sets, never on synthetic data. The locked golden set only confirms it, once per release candidate (7.6). 0.90 is a research starting value, not the Balanced cutoff: Balanced is the lowest cutoff whose estimated silent failures are at most 1% (4.9).

| Preset | Good from (interim) | Failed below | Target silent failures | Flagged (an outcome, not a control) |
|---|---|---|---|---|
| Strict | 0.95 | 0.60 | at most 0.3% | about 15 to 20% |
| Balanced | 0.90 | 0.60 | at most 1% (B6) | about 8 to 10% (B6) |
| Aggressive | 0.80 | 0.60 | at most 3% | about 3 to 5% |

- **Default strictness.** *Assumption A-3: Confident results are saved after the whole batch is triaged (streaming is opt-in). Previews default to Strict and label Balanced experimental until the golden gate passes; B6 stays the target. (owner may veto)* Balanced becomes the unlabelled default only if the locked golden set shows a silent-failure point estimate of at most 1.0% with a one-sided 95% Clopper-Pearson bound of at most 2.0% (gate G2); otherwise Strict is the default and Balanced is labelled experimental. Only silent-failure evidence demotes the default: a Balanced flag rate above 10% (15% at v0.3.0) is a stage-gate note.
- Changing the preset re-triages stored scores for unwritten items; written items are unaffected.
- **Commit mode** (`CommitMode`). `AfterAnalysis` is the default: confident results are saved in one background pass once every item is triaged, because hold rules such as `BATCH_OUTLIER` need the whole batch. `Manual` ("Review before saving") writes nothing until **Save all** (B5). `Streaming` is opt-in: it writes confident items as they finish and switches `BATCH_OUTLIER` off. In every mode held items are written only after acceptance; **Accept all** takes Good items only, and accepting Check items needs an explicit selection and a confirmation that shows the count.
- **Review order:** `DetectionFailed`, then `HeldForReview` by ascending confidence, then the rest. Held state survives restarts.

**Cancel, pause, isolation.** `CancelToken` is hierarchical (batch, job, stage) and checked every 64 rows. Cancel never interrupts `Committing`, a shielded section. Pause stops admission and lets in-flight jobs reach a state boundary. Each job returns its own `Result`; one failure never stops the batch, and the summary offers Retry failed.

**Crash-resume.** `batches` persists the spec (roots, preset, thresholds, commit mode, `OutputSpec`, engine version). On restart, `Saved` items are skipped, in-flight items reconcile (2.7), and the rest re-queue with the recorded parameters.

## 2.9 Worker-process pool and sandboxing (B12, D3)

The arch report deferred a worker mode to a later release; D3 overrides that, so the pool ships in v1 because `catch_unwind` cannot stop a segfault in C. **Isolated in the `worker` pool:** libheif and libde265, AVIF via libheif with dav1d, the WIC and ImageIO HEIC backends, and later PDFium and LibRaw. **In-process, behind hard limits:** zune-jpeg, png, tiff, image-webp, jxl-rs and turbojpeg. In-process decoders get a header probe and pixel cap before allocation, explicit `image::Limits` (the default `max_alloc` is 512 MiB and non-strict), per-item `catch_unwind` and a job timeout. libjpeg-turbo is C running in-process, which D3 accepts; mitigations are the version pin, fuzzing and our dimension check. AVIF goes through the worker rather than `avif-native`, so every C parser of untrusted bytes stays isolated.

**Pool.** Persistent workers, `ceil(batch_threads / 2)` (PROVISIONAL), spawned lazily and respawned on death. Control uses length-prefixed `postcard` frames over pipes. Pixels return through shared memory: the parent reads file bytes once with `std::fs` (never a path) into an input segment and allocates an output segment from the probed dimensions, capped by `max_pixels`. When the worker signals done, the parent seals or remaps the output read-only (`memfd` with `F_SEAL_WRITE` on Linux, a read-only duplicated handle on Windows, the equivalent for macOS chosen in the sandbox spike) and wraps it as an `Arc<SharedRaster>` without copying. It treats every reply field as untrusted (the returned dimensions must equal the probe) and never reuses a segment while its worker lives. The mechanism (`shared_memory`, or `memmap2` over `memfd_create`, `shm_open` or `CreateFileMappingW`) is chosen in the sandbox spike.

**Recycling.** Overwrite is the default, so a worker exploited by one hostile HEIC could hand back altered pixels for later files that are then written over originals, and the step-3 verify of 2.7 decodes our own output, so it cannot notice. A worker is therefore recycled after any anomaly (crash, timeout, cap hit, protocol mismatch), after any decode that used more than 50% of a cap, and every 32 decodes (PROVISIONAL).

**Limits.** libheif security limits are always set. A parent watchdog kills the worker after `10 s + 1 s per MP` (PROVISIONAL). A crash or timeout fails that item only (`DecoderCrashed`, `DecodeTimeout`), respawns the worker and marks the file hash do-not-auto-retry. Plugins load before the sandbox applies.

**Native libraries and the `hevc` feature.** libheif and libde265 ship as **separate shared libraries** (B12), built by our own decode-only CMake build: never vcpkg, never `libheif-sys` embedded mode, and bundled libheif is the default on every OS, so the paid Windows HEVC extension is never needed or bought (B15). libde265 is a libheif plugin bundled in all official builds and gated by the `hevc` Cargo feature. Turning the feature off gives the `no-hevc` variant, which omits the plugin file: a HEVC HEIC then fails with `HevcDecoderMissing` (AVIF and JPEG-in-HEIF still decode, and `heic.engine = system` uses the OS codec where one exists). There is no automatic fallback to `no-hevc`; the variant serves distro packagers or an owner-decided fallback. *Assumption A-5: Official builds always bundle libde265 (B12). The $0 legal read (pool policies re-read with dates, any free-clinic reply, gap disclosed) is due before the first published HEVC build if practicable, no later than 1.0; a `no-hevc`-only official release needs the owner's recorded decision. (owner may veto)* libheif is tracked at 1.23.5 or later within days of advisories (pins in `native-deps.toml`, SLA in 3.4.3; [SECURITY.md](https://github.com/strukturag/libheif/blob/master/SECURITY.md)).

| OS | Mechanism | Level string | Notes |
|---|---|---|---|
| Linux | `landlock` (MIT OR Apache-2.0, kernel 5.13+, best-effort, [docs](https://docs.rs/landlock/latest/landlock/)) with an empty filesystem ruleset; `seccompiler` allow-list; `NO_NEW_PRIVS`; `RLIMIT_AS`, `RLIMIT_CPU` | `landlock+seccomp`; without Landlock `seccomp-only` (rlimits and a syscall allow-list); `process-only` until the Linux preview delivers the sandbox | `doctor` and About print the level string |
| Windows | Job Object (memory cap, kill-on-close, one active process, UI limits), restricted token at low integrity, mitigation policies (no win32k, no remote image loads) | `job+token`; `appcontainer` is a stretch, since portable installs outside `Program Files` need ACL grants | the first Windows preview ships `job+token` |
| macOS | `sandbox_init` deny-default profile applied after libraries load; `RLIMIT_CPU`; parent RSS watchdog, since `RLIMIT_AS` is unreliable | `sandbox_init`; `process-only` until the macOS preview delivers it | `sandbox_init` is deprecated but widely used; ImageIO under it needs testing |

**What a level promises.** The worker is always a separate low-privilege process inside a caps-and-watchdog envelope, and no path is ever passed to it. Only `appcontainer`, `landlock+seccomp` and `sandbox_init` also block network and filesystem access. `job+token` (a Windows job object plus restricted token) does not: it caps memory, process count and UI access, but files and the network stay reachable. `seccomp-only` and `process-only` are weaker still, and macOS and Linux run `process-only` until their previews deliver their sandboxes. `doctor` and About report exactly one of the level strings `appcontainer`, `job+token`, `landlock+seccomp`, `seccomp-only`, `sandbox_init` or `process-only`; `--require-sandbox` refuses to run at `process-only`, and a setting can refuse HEIC decoding there (8.6.2).

Per D4, the default HEIC backend is the bundled libheif on every OS (`heic.engine = bundled`: identical behaviour everywhere); WIC and ImageIO are opt-in fast paths selected by `heic.engine = system` (or `auto` once the parity gate of 3.4.1 passes), also run in the worker, until the OS-decoder benchmark exists.

## 2.10 Settings, logging, crash log, Report issue, errors

- **Settings.** `settings.toml` (`serde` plus `toml`), schema-versioned, written through the atomic writer. Layering: defaults, file, `AUTOCROP_*` environment, CLI flags. Keys cover saving mode, retention, backup location, preset, commit mode, quality, HEIC colour and strip-location (C2), "Keep the scan" (off), `heic.engine`, `max_pixels`, `notice_ack` (the CLI's one-time notice) and locale. There is deliberately no telemetry key and no verification key: `verify` is not a setting (2.7). `set_settings` validates paths such as the backup location in Rust (2.5).
- **Logging.** `tracing` with `tracing-subscriber` and a daily `tracing-appender` file (7 days, 20 MB cap, PROVISIONAL). Paths are redacted above `debug` to `blake3(path)[..8]` plus extension. Spans per job and stage; `stage_done{stage, ms, px}` also feeds `--timings` and the benchmark harness.
- **Crash log.** A panic hook writes `crash/<ts>-<version>.txt`: version, git SHA, OS, architecture, webview version, panic location, backtrace, last 200 redacted log lines, active stage and memory stats. No image content or raw paths. A `session.lock` sentinel, written at start and removed on clean exit, detects native crashes on the next launch and offers Report issue. The parent records worker exit status or signal.
- **Report issue (B18).** Opens a pre-filled `github.com/WelFedTed/auto-crop/issues/new` URL that carries only the version, OS, webview or backend, release channel and error code (well under 1 KB; a test asserts it holds no file name or path). The redacted diagnostic text (last log lines, backtrace; at most 4 KB, PROVISIONAL) goes to the clipboard after a preview dialog that shows exactly what will be copied, and the user pastes it. The app uploads nothing itself, and nothing sensitive ever rides in a URL.
- **Error taxonomy.** Errors are stable codes plus parameters, never English text; UI and CLI localise them.

| Class | Examples | Retryable | Original touched |
|---|---|---|---|
| Input | `UnsupportedFormat`, `Corrupt`, `TooLarge`, `ReadOnly`, `CloudNotLocal`, `HevcDecoderMissing` | some | no |
| Decode | `DecoderCrashed`, `DecodeTimeout`, `MemoryLimit` | timeout | no |
| Analyse, Render | `ModelLoadFailed`, `OutOfMemory`, `InternalPanic` | yes | no |
| Encode | `EncodeFailed`, `UnsupportedOutput` | no | no |
| Write | `DiskFull`, `FileInUse`, `VerifyFailed`, `SourceChanged`, `BackupFailed` | mostly | no; reconciled if in `Committing` |

Detection failure is a state, not an error, and a hold is a state with a `HoldReason` (2.8). **Notices** are non-fatal Info or Warn item chips and never hold an item: Live Photo video not converted, gain map or depth dropped, HDR tone-mapped, GIF first frame only (B12). `ErrKind` is the enum behind this table; later milestones extend it with new codes.

## 2.11 i18n plumbing (B20)

Rust emits codes and parameters only (`ErrKind`, `HoldReason` and notice codes); no English literal reaches the UI. The leading candidate is one Fluent (`.ftl`) catalogue read by `@fluent/bundle` in Svelte and `fluent-bundle` in the CLI (plurals live in the format; Weblate's Fluent support to be confirmed); the alternative is ICU MessageFormat (Paraglide or `svelte-i18n`). A one-day spike decides on plural support, runtime locale switching and a missing-key lint. Numbers, dates and sizes use `Intl`. `xtask i18n pseudo` generates an accented, roughly 30% longer locale and a mirrored RTL pseudo-locale; CI drives the UI with mocked IPC and fails on untranslated, clipped or unmirrored output. Layout uses CSS logical properties, arrow shortcuts mirror in RTL, and `aria-label` and SVG text are translated too. A real RTL locale in CI is a stretch goal.

## 2.12 Headless CLI

The CLI is a console-subsystem binary, `auto-crop`, separate from the GUI because a Windows GUI-subsystem executable cannot write to the console; the console `auto-crop.exe` ships beside the GUI exe in every Windows package. It links `engine` only, so a headless Linux build has no webview dependency. Stack: `clap` 4 derive, `clap_complete`, `clap_mangen`. Besides the file paths of "Open with" (2.13), the GUI binary takes only `--safe-graphics` (2.5) and a hidden `--smoke-test <dir>` (a packaged-artifact check that opens a bundled image, saves a copy into `<dir>` and writes a JSON verdict); everything else is the CLI's job.

| Subcommand | Purpose |
|---|---|
| `process <inputs...>` | detect, correct, enhance (off by default), write |
| `convert <inputs...> --format jpg` | convert-only preset (HEIC to JPG) |
| `analyze <inputs...>` | detection only; prints `EditState` and confidence; `--emit-edit <dir>`; writes no images |
| `render --edit state.json <input> -o out` | applies a saved `EditState`; reproduces a GUI result |
| `restore`, `backups list \| show \| purge` | same store as the GUI |
| `doctor` | library and model versions and hashes, sandbox level string, threads, memory, store usage and real store paths; `--self-test` (model hashes, worker spawn, a tiny HEIC decode, one synthetic image) |

**Output modes.** *Assumption A-1: The CLI follows B3: with no output flag, `process` overwrites in place after a verified backup; `--output <dir>` and `--suffix <s>` write copies; `--in-place` is an explicit alias. The backup store, `restore`, `--dry-run` and a one-time stderr notice cover script accidents. (owner may veto: require `--in-place` or `--output`)* At most one of `--in-place` (an alias for the default), `--output <dir>` (mirrors the relative tree) and `--suffix <s>` (writes beside the source) may be given. There is no `--copy`, no `--output-dir` and no confirmation prompt. Copies need no backup unless `--if-exists replace` overwrites an existing file, which then takes the full backup-then-swap path (2.7). Sources that cannot be replaced (2.7) are skipped and reported, unless `--suffix` or `--output` writes a copy. The first time `process` would overwrite, it prints one notice to stderr (originals are overwritten after a verified backup; where the backups live and for how long; how to `restore`; the `--output` and `--suffix` alternatives), recorded as `notice_ack`; it never blocks a non-interactive run and never goes to stdout. If a verified backup cannot be made before the first write (store unwritable, no space), `process` exits 6 and writes nothing; a later per-item backup failure fails that item (`BackupFailed`, exit 3) and leaves its original untouched.

**Flags** include `-r`, `--include/--exclude`, `--preset receipt|document|photo|flatbed|convert-only` (B1; `flatbed` is the preset for scans with several items), `--enhance off|auto|gray|bw` (default `off`: enhancement is never forced), `--margin | --content-tight`, `--triage strict|balanced|aggressive` with a numeric `--min-confidence` (below the 0.60 Failed floor is a usage error), `--format`, `--quality`, `--bits 8|1` with `--bilevel png|tiff-g4`, `--dpi`, `--name-template`, `--if-exists keep-both|skip|replace` (the `collision` of 2.3), `--colour srgb|preserve`, `--strip-location`, `--jobs`, `--mem-limit`, `--max-pixels`, `--require-sandbox` (exit 6 at level `process-only`), `--dry-run`, `--deterministic` (fixed threads, for golden tests), `--timings`, `--no-config`. Later features add flags on the same names (`--split`, `--dewarp`, the enhancement sliders). The CLI reads `settings.toml` only for store location, retention, language and `notice_ack`, so a run reproduces from its command line and a GUI "Save as copy" toggle never changes what a script does. The start-up purge of expired backups (2.7) runs from `process` and `restore`; `--dry-run`, `doctor` and the `backups list | show` listings never purge and never write. Held items are never written and no flag forces them; a script must `analyze` and decide.

**Output.** stdout is machine output; logs and progress go to stderr (`indicatif` on a TTY). `--json` prints one final document; `--ndjson` streams one versioned event per line, for example `{"v":1,"t":"item","state":"held_for_review","score":0.74,"ms":{"decode":81}}`.

| Exit | Meaning |
|---|---|
| 0 | all written, or skipped by choice (for example already processed) |
| 1 / 2 | internal error / usage error |
| 3 | some items failed; others may have succeeded |
| 4 | no failures, but items held or detection failed (`--hold-exit-zero` maps to 0) |
| 5 | no supported input |
| 6 | precondition failed before any write (space, store unwritable, no verified backup possible, required sandbox missing) |
| 130 | interrupted after a clean cancel; journal consistent |

Precedence 1, 6, 3, 4. **Benchmark harness.** `analyze --ndjson --timings` and `render` are the building blocks of `xtask bench` (criterion for trends, gungraun instruction counts as the CI gate, nightly wall-clock) and `xtask eval`, which runs the public `auto-crop-eval` harness (IoU, corner error, skew, orientation, CER). They run the same engine as the GUI, and the harness lands before any GUI work.

## 2.13 OS shell integration (B19)

Everything is opt-in (Settings or an installer checkbox). We register as an **alternative** handler and never change the default app.

| | Windows | macOS | Linux |
|---|---|---|---|
| Open with | Not Tauri's `bundle.fileAssociations`: its NSIS template claims the extension's default class (to be verified in the plumbing spike). The NSIS `installerHooks` (opt-in) write `Applications\AutoCrop.exe` and `OpenWithProgids` under HKCU, leave the extension default and UserChoice (hash-protected) alone, and are removed on uninstall; the portable zip writes the same keys from Settings | `CFBundleDocumentTypes` with `LSHandlerRank = Alternate` | `.desktop` `MimeType=` for the supported image types; `Exec=AutoCrop %F` |
| Delivery | command-line args | Apple Events, surfaced as `RunEvent::Opened{urls}` (confirm in spike) | `%F` paths or `%U` URIs (`Url::to_file_path`) |
| Context action | classic verb "Crop and straighten with Auto Crop" under `SystemFileAssociations\image\shell` plus explicit extensions such as `.heic`; it sits under "Show more options" on Windows 11, which satisfies B19; the modern menu needs an `IExplorerCommand` handler with package identity, so MSIX only (an optional stretch) | Quick Action workflow in the bundle; "Add to Finder Quick Actions" copies it to `~/Library/Services` and it runs `open -b io.github.welfedted.AutoCrop`, by bundle id because "Auto Crop" is also a Mac App Store title. A Finder Sync extension is rejected: it needs real signing (B15) | Dolphin service menu and Nemo action via deb/rpm; Thunar action on request; GNOME Files has no declarative menu, so "Open With" |

**One extension list.** The installer hooks, the portable-zip registration, `CFBundleDocumentTypes`, `MimeType=` and the context verb all read one extension list, so associations cannot drift from what the engine decodes, and a new format is added in one place.

**Multi-selection.** Explorer's classic verbs can launch one process per file. The shell uses [`tauri-plugin-single-instance`](https://v2.tauri.app/plugin/single-instance/), whose callback receives args and cwd (DBus on Linux). The second process forwards its paths and exits; the running instance gathers `open-request` events for a 300 ms merge window (PROVISIONAL) into one session. The Flatpak manifest needs the DBus permissions.

**Flatpak caveat (unverified; packaging spike).** In-place replace needs write access to the containing directory, and a document-portal grant to one file does not allow a sibling temp file. The manifest therefore requests home or Pictures access (fine for our own remote, scrutinised on Flathub, B17); without it the app falls back to Save as copy and says so.

## 2.14 Filesystem behaviour

| Topic | Rule |
|---|---|
| Folder open | `walkdir`, streamed with lazy header probes. Extension prefilter, then magic-byte sniff; explicitly dropped files are sniffed regardless of extension. Skip hidden and system entries and `.autocrop-*.tmp`; do not cross mount points; depth cap 64; warn above 50,000 files (PROVISIONAL). |
| Symlinks | Not followed by default. When enabled: loop detection plus a visited set of file ids (`dev,ino`; volume serial and index), so each file is processed once even across hardlinks and junctions (junction behaviour tested in the plumbing spike). A symlinked file is replaced at its resolved target and the link stays valid. |
| Natural sort | `alphanumeric-sort` or `natord` on an NFC, case-folded key, digit runs numeric, raw-byte tie-break. Display only. |
| Cloud placeholders | Windows: `RECALL_ON_DATA_ACCESS` (0x400000), `RECALL_ON_OPEN`, `OFFLINE`. macOS: `SF_DATALESS`. Never hydrate during scan or thumbnailing. Preflight offers "Download and process" or "Skip online-only"; 2 to 4 concurrent downloads, 60 s timeout (PROVISIONAL). |
| Windows paths | `std` adds `\\?\` from 248 characters (not 260), and for many short relative paths. `dunce` for display, verbatim internally. One sanitiser (in `FsPlan`) replaces illegal characters in template output and handles reserved names, trailing dots and spaces, and 255-unit components. Workers never get paths. |
| Unicode | Dedupe and collision keys are NFC plus case-fold on every OS (conservative). Output names append to the directory entry's raw `OsString`, so no lookalike sibling appears. Linux names may be invalid UTF-8: `OsString` throughout, raw bytes in SQLite. |
| Timestamps, permissions | mtime, mode, xattrs and (Windows) attributes and `Zone.Identifier` carry over as in 2.7; birth time is best-effort; a read-only source is held for a user choice. |
| Volumes, shares | Temp file in the target directory. Cross-volume backups copy, re-hash and `sync_all`; FAT, exFAT and network shares have no hardlinks or reflinks, so backups copy. |

## 2.15 UI-agnostic core and the Slint fallback (B8)

The `engine` contract is a command and query API plus an `EventSink` trait; `shell` implements the sink with Tauri events, and nothing below it mentions a webview (2.2). The frontend keeps only view state and the drag-time homography.

**The gate, exactly as decided in B8.** Switch to Slint **only if** Linux touch cannot hold about 60 fps pan and zoom on a 24 MP proxy **and** Slint passes the folder-drop test, evaluated once in the week-1 spike, before any UI is written. The pass bar for "holds about 60 fps" is a median frame time of at most 17 ms and a p95 of at most 20 ms; the Linux trigger is a median above 22 ms on the 24 MP proxy after the shims. *Assumption A-10: For B8, "cannot hold about 60 fps" on Linux means median frame > 22 ms after shims on the 24 MP proxy; Linux touch stays best-effort; switching to Slint needs the owner's waiver of B20 RTL layout and a licence decision. (owner may veto)* The spike measures both stacks on a Surface, a MacBook, and Ubuntu 24.04 plus Fedora (Wayland and X11, NVIDIA and Intel, real touchscreen), including a dropped folder with symlink loops and OneDrive placeholders. It also includes an unconditional Slint smoke of about one day, so the fallback's folder-drop clause rests on a measurement even if the gate never fires.

What the gate must reckon with:

- Slint's default winit backend has no OS file or folder drop in 1.18.1; only the Qt backend does, and the winit `DroppedFile` workaround is unverified on Wayland. Hence the folder-drop clause.
- Slint has no RTL mirroring (issue #2294), which conflicts with B20; its gesture handler needs 1.16, file paths need 1.18, and Skia is opt-in ([changelog](https://raw.githubusercontent.com/slint-ui/slint/master/CHANGELOG.md)). Its royalty-free 2.0 licence is non-OSI, so recheck it against B2 and SignPath's rules first. A switch therefore needs the owner's waiver of the B20 RTL layout and a licence decision (A-10).
- If Tauri fails the touch clause and Slint fails the drop clause, the gate does not fire: Tauri ships with Linux touch best-effort and its limitations documented, never "unsupported" (B8 allows this). Tauri failing on Windows or macOS, where touch is guaranteed, is not a Slint trigger either: no UI code is written until the owner decides. Qt via `cxx-qt` and Tauri's alpha CEF runtime are unagreed alternatives needing a new decision.
- The gate is evaluated once, before any UI is written. After that, a switch is a new user decision, and later Linux measurements never re-trigger it.
- **After the gate.** The Linux preview re-measures on real GPU rows (Ubuntu 24.04 and 26.04 (verify), Fedora; Wayland and X11; Intel and NVIDIA; x86_64 only) against a pointer gate of median at most 22 ms and p95 at most 33 ms. Its touch protocol records works, partial or fails per row and never blocks a release.

A switch means a `shell-slint` crate against the same `engine` contract (`SharedPixelBuffer` viewer, custom quad editor and loupe, `rfd` dialogs) and nothing else. The spike report estimates effort.

## 2.16 Dependencies and maturity

Versions are as verified on 2026-09-30 and pinned in `Cargo.lock`. Licences are given where research established them; `cargo-deny` gates the rest.

| Component | Role | Version, licence | Maturity notes |
|---|---|---|---|
| `image` + `zune-jpeg` | PNG, TIFF, WebP, BMP, GIF; JPEG fallback | 0.25.10 (MIT OR Apache-2.0); zune-jpeg 0.5.15 | non-strict 512 MiB alloc limit; decoders can panic; no DCT scaling; let `image` pin zune-jpeg (0.5.16-rc2 is a prerelease) |
| `turbojpeg` | scaled decode, lossless transform, encode | 1.5.1, `-sys` 1.2.0 (Unlicense OR MIT) | needs CMake and NASM; vendored libjpeg-turbo 3.1.0 is stale, override to 3.1.4+ |
| `fast_image_resize` | proxies, export resize | 6.1.0 (MIT OR Apache-2.0) | published benchmarks are Lanczos3, desktop CPU, on 6.0.1 |
| `jxl` (jxl-rs) | JXL decode | 0.7.4 (BSD-3) | young; JXL encode needs our own FFI to libjxl (BSD-3) |
| `tiff`, `moxcms` | TIFF; colour management | tiff 0.11.x (MIT); moxcms 0.9.1 (BSD-3 OR Apache-2.0) | tiff cannot write Fax4 and `fax::tiff::wrap` hardcodes 200 dpi and WhiteIsZero, so we write our own G4 wrapper; `lcms2` is a dev-only test oracle and never ships (CMYK goes through `moxcms` or the 3.7 fallback), and `deny.toml` allows it only as a dev-dependency |
| libheif, libde265 | HEIC, HEIF, AVIF | 1.23.5, 1.1.3 (LGPL) | own decode-only CMake build, shared libs, bundled on every OS; libde265 sits behind the `hevc` feature (2.9); one largely unfunded maintainer |
| `libheif-rs`, `-sys` | binding, non-embedded | 3.0.0, 5.3.1 (MIT) | `embedded-libheif` rejected (static, no plugins, four releases behind). No vcpkg: its libheif port's default `hevc` feature pulls GPL x265 ([port](https://github.com/microsoft/vcpkg/blob/master/ports/libheif/vcpkg.json)), so the binding is pointed at our own CMake prefix with `default-features = false`. Fallback: own bindgen crate |
| `ort` (or `rten`, `tract`) | ONNX inference | 2.0.0-rc.13 (ONNX Runtime 1.28) | still an RC; no macOS x86_64 binary; behind `InferenceBackend` |
| `rayon`, `crossbeam-channel`, `rusqlite` | parallelism, events, journal | 1.12.0, 0.5.17, 0.40.2 | no tokio in core; rusqlite compiles C (`redb` is the pure-Rust alternative) |
| `tempfile`, `filetime`, `xattr`, `reflink-copy`, `blake3` | write path | 3.27.0, 0.2.29, 1.6.1 | `persist` does not fsync; xattr is Unix-only |
| `tauri` and plugins | shell | 2.12.0; dialog, opener, single-instance; updater 2.13.1 | wry 0.57 is still GTK3 WebKitGTK; stay off Tauri 3 alpha; dialog and opener are called from Rust commands only, the webview holds no plugin permission (2.5) |
| `landlock`, `seccompiler`, `windows`, `objc2` | sandbox, OS APIs | landlock (MIT OR Apache-2.0); windows 0.62 | `birdcage` is archived and GPL |
| Test stack | quality | criterion 0.8.2, gungraun 0.20.0, proptest 1.11.0, insta 1.48.0, `image-compare` 0.5.0, `cargo-fuzz` 0.13.2 | gungraun needs Valgrind; cargo-fuzz is Unix-only; `opencv-rust` only as a dev oracle |

Routine crates (`walkdir`, `dunce`, `unicode-normalization`, `directories`, `clap`, `serde`, `postcard`, `tracing`, `toml`, `sysinfo`, `quick_cache`, `fail`) are omitted.

**Banned** by `deny.toml` and the model policy: `heic` and `dssim-core` (AGPL; even as a dev-dependency), `jpegxl-rs` and `jpegxl-sys` (GPL), x265 and x264 (GPL), `birdcage`, AlbumentationsX (AGPL), and DocTr, DocGeoNet and DocEnTr code or weights (non-commercial). LGPL native libraries are not Cargo dependencies; the third-party notices list them with relink instructions. Pure-Rust `heic-rs` (MIT OR Apache-2.0) is re-evaluated in about six months.

## 2.17 Architecture spikes that gate this section

Each becomes a roadmap item with a measurable exit.

- **Week-1 GUI spike** (2.15): 24 MP pinch, pan and rotate near 60 fps (median at most 17 ms, p95 at most 20 ms); folder-drop edge cases; 500-image tile serving; IPC latency; JPEG, WebP or raw RGBA payloads; the unconditional one-day Slint smoke.
- **Plumbing spike:** `ReplaceFileW` versus `rename` per filesystem, including the 1175 to 1177 failure cases; sharing-violation retries; parent-directory fsync and `sync_all` on macOS (`F_FULLFSYNC`, to confirm); link durability and journal ordering per filesystem; same-volume links; the commit-protocol fault-injection suite with the `FaultFs` power-loss model; a throwaway MSIX sideloaded, uninstalled and observed for virtualised app data (2.7); whether Tauri's NSIS `fileAssociations` claim the default class (2.13).
- **Sandbox spike:** shared-memory mechanism per OS and the read-only seal; Landlock plus seccomp; Windows job object plus restricted token (and AppContainer as a stretch); `sandbox_init` with libheif and ImageIO; `libheif-rs` pointed at our CMake prefix without vcpkg, else bindgen.
- **Inference spike:** `ort` versus `rten` versus `tract`; session pooling; int8 accuracy per platform.
- **Benchmark baseline:** real 12, 48 and 100 MP files on Tier-M and Apple silicon; verify-stage cost, which also picks the default `verify` mode; scaled-then-full versus full-then-resize decode in batch (full-then-resize is expected to win; unmeasured).
- **Legal read** on HEVC patents (B12), due before the first published HEVC build if practicable and no later than 1.0 (Assumption A-5, 2.9).
