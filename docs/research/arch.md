# Research: Core architecture, edit model, undo, batch engine and testing  (key: arch)

## Summary
Rust is the right backend, and the design below is GUI-agnostic. A Cargo workspace keeps decode, analyse, render and encode as separate stages behind traits, so both the CLI and the GUI drive one `engine` crate.

Edits are non-destructive. The original file is never mutated, and the output is a pure function of (source, EditState). Because EditState is a small serialisable struct (about 1 KB), undo can be a plain snapshot stack with a cursor. That needs no command pattern for pixels and no persistent data structures. Session-level operations such as "apply to all" use command plus memento.

Previews use layered proxies. JPEGs get DCT-scaled decode through libjpeg-turbo. Renders are cancellable by generation counter.

Batches run on rayon under a byte-weighted memory budget. Each item is wrapped in catch_unwind, because rayon aborts the process on unhandled panics in spawned tasks. Progress is journalled in SQLite for crash-resume.

The default is never to overwrite. In-place mode makes a backup first, then does an atomic temp-file plus rename.

The v1 build makes no network calls.

## Recommendation
Adopt Rust with a hexagonal workspace: `core` (model and ports), `codecs`, `imgproc`, `engine`, `cli` and `gui`.

- **Concurrency:** use rayon plus crossbeam-channel. Keep tokio out of the core.
- **JPEG decode:** use turbojpeg (bundled libjpeg-turbo 3.1.0) for scaled decode and lossless crop/rotate. Use the `image` crate (zune-jpeg) as the pure-Rust fallback and for other formats.
- **HEIC:** libheif-rs, feature-gated and dynamically linked. Do not adopt the pure-Rust HEIC crates yet.
- **Persistence:** rusqlite (bundled) for a session journal, per-image edit history, batch jobs and backup records. No sidecars by default.
- **Overwrite policy:** default to "save as copy". In-place mode takes a hardlink/reflink backup, then does an atomic replace, and offers restore.
- **Testing:** a synthetic ground-truth generator, proptest, golden images with SSIM (`image-compare`), criterion plus gungraun benchmarks, cargo-fuzz on Linux and macOS, and a 3-OS CI matrix including arm64.
- **Telemetry:** no telemetry. Write local panic logs and let the user file an issue by hand.

Decide the project licence before dependencies are locked in. Two crates are AGPL: dssim-core and heic.

## Key findings
- Rust fits: SIMD, controllable memory, safe parsing of untrusted files, and easy data-parallelism. The decisive libraries are still C: libjpeg-turbo (scaled decode, lossless transforms) and libheif. A C++/Qt/OpenCV stack would ease the vision work but loses memory safety on arbitrary input. Keep the vision layer behind an `imgproc` boundary so OpenCV (opencv-rust 0.101, Sep 2026) can be swapped in if the algorithms team needs it.
- The `image` crate (0.25.10, Mar 2026) is full-image decode only, with no region or tile API. Its default Limits.max_alloc is 512 MiB, so you must raise it explicitly for 100+ MP files, and you should keep a hard ceiling against decompression bombs. Use turbojpeg's set_scaling_factor for 1/2, 1/4 and 1/8 DCT-scaled previews, and its Transform with crop for lossless MCU-aligned crop and 90-degree rotation.
- Undo design: per-image undo is snapshots of Arc<EditState>. That is O(1), tiny, serialisable and trivially persistent, and it matches how darktable keeps a history stack alongside its database and XMP. The `undo` crate (last release Mar 2025, about 7k recent downloads) and `imbl` (MPL-2.0) add nothing here. Export and overwrite are not history entries. They are reversible only through a recorded backup.
- Scaled decode wins for browse, preview and first paint. My estimate, not measured and needing a benchmark spike, is that in batch mode one full decode plus a SIMD resize (fast_image_resize 6.1) beats scaled decode followed by a second full decode.
- Never hand paths to C decoders. Core reads bytes with std::fs and passes slices or readers, which sidesteps Windows ANSI and long-path problems. Avoid mmap because external truncation makes it unsound.
- Filesystem hazards to design for: (1) atomic-write-file does not preserve timestamps, ACLs, xattrs or symlink identity, so preserve them explicitly with filetime, xattr and canonicalisation. (2) Windows cloud placeholders (OneDrive) hydrate on access, so do not prefetch or batch them silently. (3) On Windows, AV and indexers cause transient sharing violations, so retry rename with backoff. (4) APFS preserves but compares filenames normalisation-insensitively, so key on NFC plus case-fold. (5) Two-phase planning of output names detects collisions inside the batch before any write.
- Licence traps: dssim-core is AGPL-3.0, the pure-Rust `heic` crate is AGPL-3.0-only OR commercial, and libheif is LGPL. Run cargo-deny in CI from day one.
- Testing: build a synthetic ground-truth generator (known quad, homography, blur, noise and lighting) to measure corner error and IoU for the accuracy claims. Add golden-image SSIM, and fuzz our own parsers and the whole decode-to-analyse pipeline. cargo-fuzz is Unix-only (Linux and macOS), so run it in the Linux CI job.

## Risks
- Untrusted-image decoders, especially C ones (libheif), can segfault and are not stopped by catch_unwind. Mitigate with strict Limits and cargo-fuzz now. Defer a subprocess worker mode (`auto-crop-cli --worker`) to a later release.
- Memory: 100 MP RGB8 is about 300 MB and RGBA8 about 400 MB. With decode, warp and output buffers a job can peak near 1 GB. The byte-weighted budget must admit oversize jobs alone, or it can deadlock.
- Build and packaging: turbojpeg-sys needs CMake, and NASM for the SIMD code. HEIC needs libheif on every OS (vcpkg on Windows, Homebrew on macOS, apt on Linux). Keep zune-jpeg and a heic-off build as fallbacks so CI stays green.
- Panic policy: rayon aborts the process on unhandled panics in spawned tasks. Requires panic=unwind in release profiles, a panic_handler, and per-item catch_unwind. A GUI toolkit that forces panic=abort would break this design.
- In-place overwrite via rename loses birth time, xattrs, Finder tags and ACLs unless copied. Users will blame the app for lost metadata. Hardlink and reflink backups fail on FAT/exFAT and network shares, so a plain copy fallback is needed.
- Undo-scope confusion: pixel edits are undoable, but file writes are not. The UI must make 'Restore original' distinct from Ctrl+Z.
- Pure-Rust HEIC crates are under 6 months old (`heic` 0.1.6 decodes 118/162 test files, heic-rs 0.1.1 created Sep 2026, heif-oxide 0.1.0), and the libheif README reportedly notes thin funding and many security reports in 2026. Re-evaluate HEIC in about 6 months.
- The performance figures in the deliverable are proposed budgets, not measurements. A spike with criterion on real 12 MP and 100 MP files must confirm them before they become promises.

## Options evaluated

### Snapshot history of Arc<EditState> (custom, ~150 LOC) — recommended
Per-image undo/redo as a cursor over immutable parameter snapshots, with gesture coalescing; command+memento only for session-level ops.
- licence: n/a (own code)
- status: n/a. Alternatives checked: `undo` 0.52.0 (Mar 2025, about 7k recent downloads), `imbl` 7.0.2 (Sep 2026, MPL-2.0).
- pros: O(1) undo, about 1 KB per step, unlimited depth; Serialises directly to SQLite or JSON; No pixel data in history because render is a pure function; Simple to test with proptest (undo(redo(x)) == x)
- cons: Coalescing and gesture logic must be hand-written; Session-level ops such as apply-to-all still need memento commands

### rayon + crossbeam-channel (no tokio in core) — recommended
Work-stealing data-parallelism for CPU-bound stages, with bounded channels for events to the UI.
- licence: MIT OR Apache-2.0
- status: rayon 1.12.0 (Apr 2026, MSRV 1.80); crossbeam-channel 0.5.17 (Sep 2026); flume 0.12.0 (Dec 2025) is an equivalent alternative.
- pros: Fits CPU-bound image work and nested parallelism; No async colouring of the core API; Separate ThreadPools give preview/batch isolation
- cons: No built-in priorities, so use separate pools plus OS thread priority; Unhandled panics in spawned tasks abort the process, so it needs catch_unwind and a panic_handler; Cancellation is cooperative only

### rusqlite (bundled) for the store — recommended
Single library.db in the app data dir holding the session, edit history, batch journal and export/backup records.
- licence: MIT (SQLite is public domain)
- status: 0.40.2 (Aug 2026); redb 4.3.0 (Sep 2026, pure Rust, MIT/Apache) is the alternative.
- pros: Crash-safe with WAL; Good for per-item batch status updates across thousands of files; Ad hoc query and inspect
- cons: Compiles C (needs the cc crate); Adds schema-migration work; Edits are not portable with the image unless opt-in sidecars are added later

### turbojpeg + image (zune-jpeg) two-path decode — recommended
turbojpeg for JPEG DCT-scaled decode and lossless transforms. The `image` crate for other formats and as the pure-Rust JPEG fallback.
- licence: turbojpeg: Unlicense OR MIT (bundles BSD/IJG libjpeg-turbo); image: MIT OR Apache-2.0; zune-jpeg: MIT OR Apache-2.0 OR Zlib
- status: turbojpeg 1.5.1 (Jul 2026, sys 1.2.0 bundles libjpeg-turbo 3.1.0); image 0.25.10 (Mar 2026; next release needs MSRV 1.88); zune-jpeg 0.5.16-rc2 (Sep 2026).
- pros: Scaled decode gives a big first-paint win; Lossless crop and rotate with no re-encode; zune-jpeg is pure Rust and fuzzed
- cons: Build needs CMake and NASM; Two decoders means two behaviours to test; The image default max_alloc of 512 MiB needs overriding; image has no region decode

### libheif-rs (dynamic, feature-gated) for HEIC/HEIF — recommended
Safe wrapper over libheif for HEIC to JPG. Bytes are passed in memory, never paths.
- licence: libheif-rs/-sys: MIT; libheif: LGPL (dynamic link keeps a permissive app licence clean)
- status: libheif-rs 3.0.0 (Aug 2026); libheif-sys 5.3.1+1.23.1 with an `embedded-libheif` feature.
- pros: Only mature HEIC route today; Handles grids, ICC and 10-bit files; Feature flag keeps it out of minimal builds
- cons: Requires a native library per OS and the HEVC decoder (libde265) and its licensing/patent posture; C code parsing untrusted input is a crash risk; Distribution needs LGPL compliance

### Pure-Rust HEIC decoders (heic, heic-rs, heif-oxide) — avoid
No-C HEIC decoders. Watch, do not adopt.
- licence: heic: AGPL-3.0-only OR commercial; heic-rs and heif-oxide: MIT OR Apache-2.0
- status: heic 0.1.6 (May 2026, created Mar 2026); heic-rs 0.1.1 (created Sep 2026); heif-oxide 0.1.0 (Jul 2026).
- pros: Memory-safe; No native dependency; WASM-friendly
- cons: Under 6 months old; heic decodes 118/162 test files; heic's AGPL would force the whole app to AGPL; heic's README says not all code is manually reviewed

### tempfile + own durable-replace logic (vs atomic-write-file) — recommended
NamedTempFile in the destination directory, sync_all, persist (atomic replace) or persist_noclobber. Explicit metadata preservation.
- licence: tempfile: MIT OR Apache-2.0; atomic-write-file: BSD-3-Clause
- status: tempfile 3.27.0 (Mar 2026); atomic-write-file 0.3.1 (Aug 2026); filetime 0.2.29; xattr 1.6.1; trash 5.2.9 (Sep 2026).
- pros: persist replaces atomically on Windows and Unix; persist_noclobber gives safe no-overwrite; Own control of retry and fsync policy
- cons: Temp files linger after a crash, so a startup sweep is needed; Timestamps, ACLs and xattrs are not preserved (true of atomic-write-file too), so we copy them ourselves

### Test stack: criterion, gungraun, proptest, insta, image-compare, cargo-fuzz — recommended
Wall-clock benchmarks plus instruction-count benchmarks for CI gating, property tests, snapshot tests of serialised EditState, SSIM golden images and fuzzing.
- licence: criterion, proptest, gungraun, cargo-fuzz: MIT/Apache; insta: Apache-2.0; image-compare: MIT. Avoid dssim-core (AGPL-3.0).
- status: criterion 0.8.2 (Feb 2026); gungraun 0.20.0 (Sep 2026, successor to iai-callgrind, which is stuck at 0.16.1, Jul 2025); proptest 1.11.0; insta 1.48.0; image-compare 0.5.0 (Aug 2025); cargo-fuzz 0.13.2 (Unix-only, nightly); cargo-nextest 0.9.146.
- pros: Instruction-count benchmarks are stable on noisy shared runners; Snapshot tests catch schema drift; proptest can drive homography and undo/redo properties
- cons: gungraun needs Valgrind, so Linux only; cargo-fuzz does not run on Windows; criterion timings on shared runners are noisy, so gate on gungraun and track criterion as trend only

## Deliverable
**Workspace** (edition 2024, resolver 3, MSRV about stable-2, `panic = "unwind"`):

```
auto-crop/
 crates/
  core     model+ports: EditState, History, Decoder/Encoder/Renderer traits, errors, CancelToken (serde, thiserror only)
  codecs   impls: image(0.25), turbojpeg[feat], libheif-rs[feat heic], tiff  -> core
  imgproc  pure fns: analyse(), render(), enhance(), tiles; SIMD; no IO       -> core
  engine   Session, PreviewService, BatchScheduler, FsPlan, Store(rusqlite), Settings -> core,codecs,imgproc
  cli      auto-crop-cli (clap)  -> engine
  gui      shell TBD             -> engine
 fuzz/ benches/ xtask/ testdata/

 gui/cli -> engine -> codecs -> core
                  \-> imgproc -> core
```

**Flow:** `open -> probe(header: dims, EXIF orientation, ICC) -> decode(Scaled|Full) -> analyse(proxy) -> AutoSuggestion -> EditState -> render(src, state, region, scale) -> display: cms->sRGB | export: encode -> temp -> fsync -> rename`.

**Data model (pseudocode):**
```rust
struct SourceRef { id: SourceId /*blake3(head+tail+size)*/, path, size, mtime, format,
                   dims, orientation: Exif, icc: Option<Arc<[u8]>>, frame: u32 }
// geometry is in EXIF-oriented source space, normalised 0..1 (resolution independent)
struct EditState { v: u32, geometry: Geometry, enhance: Enhance }  // serde(default) + migrations
struct Geometry { quad: [Pt;4], quarter_turns: u8, fine_deg: f32, origin: Auto{algo_ver}|Manual,
                  perspective: Auto|Off|Manual, aspect: Free|Doc|Fixed, out: Keep|Fit(px)|Dpi(n) }
struct Enhance { look: Off|Auto|Document|Receipt|Whiteboard, colour: Original|Gray|BW,
                 flatten_bg: f32, contrast: f32, sharpen: f32, threshold: Adaptive{method,k} }
struct OutputSpec { format, quality, meta: Keep|Strip, icc: Keep|Srgb, template: String,
                    collision: Skip|Rename|Overwrite|Ask, target: Copy|InPlace{backup} }
struct History { entries: Vec<Entry>, cursor: usize }  // cap ~1000
struct Entry { state: Arc<EditState>, label: Cow<str>, gesture: Option<GestureId> }
//  commit(): same gesture replaces top entry; truncates redo. undo/redo = cursor +/- 1
enum SessionCmd { Remove(..), Reorder(..), ApplyToAll{ before: Vec<(ImageId, cursor)> } } // command+memento
struct ExportRecord { src, out, backup: Option<PathBuf>, method: Hardlink|Reflink|Copy, at }
struct RenderReq { image, state: Arc<EditState>, gen: u64, region: Rect, scale: f32,
                   quality: Draft|Final, cancel: CancelToken }
trait Decoder { fn probe(&[u8])->Probe; fn decode(&[u8], Want::Full|Scaled{min_edge}, &Limits, &CancelToken)->Raster }
struct JobItem { id, src, out: Option<PathBuf> /*reserved before start*/, est_bytes,
                 status: Pending|Running|Done|Failed(ErrKind)|Skipped|Cancelled }
struct MemoryBudget { cap: u64 /*min(25% RAM, 4 GiB)*/ }  // weighted semaphore; oversize job runs alone
```

**Rules:**
- **Concurrency:** UI thread owns state and sends immutable snapshots to workers. There are two rayon pools: a small preview pool (2 to 3 threads) and a batch pool (cores minus 1, lowered via thread-priority). Stale results are dropped by generation counter. Cancel is checked per 64-row band. Drag gives a 1/4-res Draft, then a Final on release after a 50 to 100 ms debounce.
- **Preview pyramid:** thumbnail (about 256 px), analysis proxy (about 1500 px), display proxy (2x viewport), then a quick_cache tile LRU (512 px tiles). Full-res is decoded lazily on zoom past the proxy or on export.
- **Undo persistence:** Store keeps session, edits and history in SQLite (WAL). Prompt "Resume previous session?" on launch.
- **Batch:** stream the folder walk (walkdir) and probe headers lazily. Two-phase plan/validate/execute for names. Journal in SQLite for crash-resume. On startup, sweep `.autocrop-*.tmp`.
- **Writes:** tempfile in the destination dir, sync_all, then persist. Use `persist_noclobber` for no-overwrite. Retry on Windows sharing violations. In-place: hardlink or reflink backup into the app data dir, then replace.
- **Lossless fast path:** JPEG to JPEG with only MCU-aligned crop or 90-degree rotation goes through turbojpeg Transform (no re-encode).
- **Logging:** tracing plus tracing-appender (rolling); redact paths above debug.
- **Proposed budgets (unmeasured):** first paint under 100 ms for a 12 MP JPEG, analysis under 150 ms on the proxy, full export of 12 MP under 300 ms per core.

## Decision-critical claims (as researched)
- The `image` crate (0.25.10, Mar 2026, MIT OR Apache-2.0) applies a default Limits.max_alloc of 512 MiB and exposes only whole-image decoding, with no region or tile decode in the ImageDecoder trait. Callers must raise the limit for 100+ MP images and cannot decode regions with it. [https://docs.rs/image/latest/image/struct.Limits.html]
- The `turbojpeg` crate (1.5.1, Jul 2026) Decompressor exposes set_scaling_factor for DCT-scaled decode, and its Transform struct supports lossless crop, with `perfect` and `trim` flags for MCU alignment. turbojpeg-sys 1.2.0 vendors libjpeg-turbo 3.1.0 via CMake and needs NASM for the SIMD code. [https://docs.rs/turbojpeg/latest/turbojpeg/struct.Transform.html]
- Rayon's default behaviour is to abort the process when a panic occurs in a worker in contexts where it cannot propagate (for example spawned tasks) and no panic_handler is set. Per-item catch_unwind and panic=unwind are therefore required for per-file error isolation. [https://docs.rs/rayon/latest/rayon/struct.ThreadPoolBuilder.html]
- atomic-write-file (0.3.1) does fsync plus rename but preserves neither timestamps, ACLs, xattrs nor SELinux contexts, replaces symlinks rather than writing through them, and on non-Unix preserves no permissions. tempfile's NamedTempFile::persist atomically replaces an existing file on Windows and Unix, and persist_noclobber refuses to overwrite. [https://docs.rs/atomic-write-file/latest/atomic_write_file/]
- Since Rust 1.85 (PR #134631, merged Dec 2024), std::fs::rename on Windows 10 1607+ uses POSIX rename semantics when the filesystem supports FileRenameInfoEx. Rust std also automatically applies the \\?\ verbatim prefix for fs operations on paths of 260 UTF-16 units or longer, but rename still fails across mount points. [https://github.com/rust-lang/rust/pull/134631]
- Licence traps: dssim-core (3.5.1) is AGPL-3.0, and the pure-Rust `heic` crate (0.1.6) is AGPL-3.0-only OR a commercial licence. libheif itself is LGPL. A permissively licensed app must not link the first two, and must dynamically link libheif. [https://crates.io/api/v1/crates/heic]
- libheif-rs 3.0.0 (Aug 2026) wraps libheif via libheif-sys 5.3.1+1.23.1, which offers an `embedded-libheif` CMake build feature. Pure-Rust HEIC alternatives are all under about 6 months old, with heic decoding 118 of 162 HEIF test files. [https://crates.io/api/v1/crates/libheif-rs]
- cargo-fuzz works only on x86-64/aarch64 Unix-like systems (not Windows) and needs nightly Rust. GitHub-hosted runners free for public repos include windows-11-arm and ubuntu-24.04-arm (arm64), macos-latest (arm64, 3 CPU/7 GB) and macos-15-intel/macos-26-intel. [https://docs.github.com/en/actions/reference/runners/github-hosted-runners]

## Researcher questions for user
- How much should the app remember across restarts? — It decides whether we need SQLite, sidecar files or nothing. It also decides whether undo history survives quitting, and whether edits can follow a moved image. (default: Session restore, with SQLite. No sidecar files by default, since they clutter folders and cloud-synced directories. Add opt-in sidecars later.)
- What should the default save behaviour and safety net be? — It shapes the export pipeline. Overwriting in place needs backup, atomic replace, metadata copying and a restore UI. Copies do not. (default: Save a copy by default. In-place is opt-in and takes a hardlink/reflink backup in the app data dir (30-day retention, user-adjustable), and offers a Restore original button. Trash only as a fallback.)
- Which licence should the project use? — It gates dependency choice. AGPL crates (dssim-core, heic) are ruled out under MIT/Apache. libheif's LGPL requires dynamic linking or a relink offer. A GPL/AGPL choice would open more dependencies but limit reuse. (default: MIT OR Apache-2.0, with libheif dynamically linked and cargo-deny enforcing licence rules in CI.)
- Is any crash reporting or telemetry acceptable? — It sets the privacy stance for the whole app. Scans and receipts are sensitive, so the answer affects whether the app ever needs a network permission or a consent dialog. (default: Local crash log plus a manual 'Report issue' button. No network calls in v1.)
- What baseline hardware, image size and OS floor should the app support? — It sets the default memory budget and concurrency, whether 200 MP files must work, and which OS APIs and CI runners we target. (default: 8 GB RAM, 4 cores, up to about 100 MP, with graceful failure and a clear message beyond that.)

## INDEPENDENT VERIFICATION (skeptic) — overrides the researcher where they differ
- [PARTLY-TRUE] 1. image 0.25.10 (MIT OR Apache-2.0, Mar 2026): default Limits.max_alloc 512 MiB; ImageDecoder trait has no region/tile decode
  CORRECTION: Version, date, licence and the 512 MiB default (width/height unlimited by default) are all confirmed. The ImageDecoder trait has no region method (only dimensions, color_type, read_image, read_image_boxed plus metadata and set_limits). But 0.25.10 also ships a separate ImageDecoderRect trait with read_rect, implemented only by BmpDecoder and FarbfeldDecoder. The upstream changelog lists it as removed in the unreleased next version, which also raises MSRV to 1.88. So 'cannot decode regions' is false in the strict sense, but it does not matter for JPEG, PNG, TIFF or WebP, so the design decision stands.
- [CONFIRMED] 2. turbojpeg 1.5.1 (Jul 2026): Decompressor::set_scaling_factor, Transform crop with perfect/trim; turbojpeg-sys 1.2.0 vendors libjpeg-turbo 3.1.0 via CMake and needs NASM
  CORRECTION: Confirmed: turbojpeg 1.5.1 was released 25 Jul 2026 and turbojpeg-sys 1.2.0 on 27 May 2026. Decompressor has set_scaling_factor and supported_scaling_factors. Transform has crop (TransformCrop, with x/y that must be MCU-aligned), perfect, trim, gray, progressive, optimize and copy_none. The sys README says the vendored source is TurboJPEG 3.1.0. Two refinements. NASM is required only because the default require-simd feature is on, and only on x86/x86-64. pkg-config to a system library is also a default feature. The vendored 3.1.0 is stale (see other errors).
- [CONFIRMED] 3. Rayon aborts the process on a panic that cannot propagate (spawned tasks) when no panic_handler is set, so per-item catch_unwind and panic=unwind are needed for per-file isolation
  CORRECTION: The ThreadPoolBuilder docs say a panic that cannot be propagated goes to the panic handler, and with no handler the default is to abort. Two nuances. (a) Abort applies to spawn and spawn_fifo. In par_iter, join and scope the panic propagates to the caller, so the whole batch call panics instead of one item failing. Per-item catch_unwind is still the right isolation, but for a different reason. (b) For fire-and-forget spawn, a pool panic_handler is an alternative to catch_unwind. Neither helps with segfaults or aborts inside C code (libheif, libjpeg-turbo), which need a subprocess or sandbox. panic=abort in any profile would defeat all of this.
- [CONFIRMED] 4. atomic-write-file 0.3.1 does fsync+rename but preserves no timestamps/ACLs/xattrs/SELinux, replaces symlinks, no permissions on non-Unix; tempfile persist atomically replaces on Windows and Unix, persist_noclobber refuses to overwrite
  CORRECTION: Confirmed. atomic-write-file 0.3.1 (BSD-3-Clause, 11 Aug 2026) documents no support for timestamps, ACLs, xattrs or SELinux. A symlink at the path is replaced, not followed. Permissions are preserved on Unix only. NamedTempFile::persist says it atomically replaces an existing target. Caveats: persist_noclobber is documented as not guaranteed atomic on every platform (it may briefly leave two hard links). Neither persist nor persist_noclobber fsyncs contents or the parent directory. The plan's sync_all covers the file, but a durable replace on Unix also needs a parent-directory fsync, which atomic-write-file does. Temp files cannot be persisted across filesystems.
- [PARTLY-TRUE] 5. Since Rust 1.85 (PR #134631, Dec 2024) fs::rename on Windows 10 1607+ uses POSIX rename semantics when FileRenameInfoEx is supported; std auto-applies the \\?\ prefix for paths of 260+ UTF-16 units; rename fails across mount points
  CORRECTION: Behaviour, Rust 1.85 milestone and merge on 22 Dec 2024 are confirmed. std docs say Windows 10 v1607+ behaves like Unix when the filesystem supports FileRenameInfoEx. However #134631 is a rollup of five PRs. The actual change is #131072, 'Win: Use POSIX rename semantics for std::fs::rename if available', so cite that. The verbatim-prefix threshold is wrong. std::sys::path::windows sets LEGACY_MAX_PATH = 248, not 260. fs calls go through maybe_verbatim with prefer_verbatim=true, so any path that is not a plain short drive-absolute or UNC path (relative paths, paths containing / or ..) is made absolute and given \\?\ even when short. Cross-filesystem rename failure is confirmed by the std docs, and there is no copy fallback.
- [PARTLY-TRUE] 6. Licence traps: dssim-core 3.5.1 AGPL-3.0; heic 0.1.6 AGPL-3.0-only OR commercial; libheif LGPL; a permissive app must not link the first two and must dynamically link libheif
  CORRECTION: Licences confirmed: dssim-core 3.5.1 (29 Aug 2026, AGPL-3.0); heic 0.1.6 (19 May 2026, AGPL-3.0-only OR LicenseRef-Imazen-Commercial); libheif library is LGPL, its sample apps MIT. Two overstatements. (1) LGPL does not require dynamic linking. Static linking is permitted if the user can relink (ship object files or source plus build instructions). Dynamic linking is only the simplest compliance route. It also conflicts with the plan's own use of libheif-sys `embedded-libheif`, which statically links libheif. (2) dssim-core is a problem only if it ends up in shipped binaries. As a dev-dependency used only in CI tests it would not taint the distributed app, though keeping it out entirely, as the plan does, is safest. The real gating question is the app's own licence choice.
- [CONFIRMED] 7. libheif-rs 3.0.0 (Aug 2026) wraps libheif via libheif-sys 5.3.1+1.23.1 with an `embedded-libheif` CMake feature; pure-Rust HEIC alternatives are all under ~6 months old; heic decodes 118/162 HEIF test files
  CORRECTION: Confirmed. libheif-rs 3.0.0 was published 18 Aug 2026 (MIT). libheif-sys 5.3.1+1.23.1 was published 13 Aug 2026 and has an `embedded-libheif` feature with an optional cmake build-dependency. Its README says it builds libheif 1.23.0 statically. Codec dependencies such as libde265 and libaom stay dynamic. On Windows the sys crate expects vcpkg. Pure-Rust decoders found: heic (created 29 Mar 2026), gamut-heic (1 Jun 2026), heif-oxide (27 Jul 2026), heic-rs (12 Sep 2026) and oxideav-heif (18 Sep 2026). An older crate named `heif` (Sep 2025) is an undocumented placeholder. The heic README states 118/162 test files decode with the av1 and unci features enabled, plus a note that not all code is manually reviewed. heif-oxide's 249k downloads after two months look inflated.
- [CONFIRMED] 8. cargo-fuzz needs x86-64/aarch64 Unix-like and nightly (no Windows); free public-repo runners include windows-11-arm, ubuntu-24.04-arm, macos-latest (arm64, 3 CPU/7 GB), macos-15-intel/macos-26-intel
  CORRECTION: The cargo-fuzz README says it works on x86-64 and Aarch64, only on Unix-like systems (not Windows), and needs nightly. Latest is 0.13.2 (9 Jun 2026). The GitHub Docs runner table lists windows-11-arm and ubuntu-24.04-arm (4 CPU/16 GB, arm64), macos-latest (3 CPU/7 GB, arm64, M1) and macos-15-intel/macos-26-intel (4 CPU/14 GB), all free for public repos. Extra runners now exist: ubuntu-26.04, ubuntu-slim and windows-2025-vs2026.

### Other errors spotted by skeptic
- Stale dependency: turbojpeg-sys 1.2.0 vendors libjpeg-turbo 3.1.0, but the upstream ChangeLog lists 3.1.1, 3.1.2, 3.1.3, 3.1.4, 3.1.4.1, 3.2.0 and 3.2.1 as newer. 3.1.4 fixes a tj3Transform double-free/leak with pre-allocated destination buffers, a lossless-scaling overrun in the 2.x wrapper, and an ICC-profile precedence bug in tj3Transform. Since the lossless-transform path is the reason to use turbojpeg, the plan should pin or override the vendored source, or link a current system libjpeg-turbo, and add a version check to CI.
- zune-jpeg 0.5.16-rc2 (8 Sep 2026) is a prerelease. The latest stable is 0.5.15 (26 Mar 2026). The plan should name the stable version and let `image` pin its own zune-jpeg.
- The `xattr` crate (1.6.1, Sep 2025) is Unix-only. The metadata-preserving copy logic needs cfg gates, and Windows needs a separate approach for ACLs and alternate data streams. The plan lists it without saying this.
- The atomic-replace flow (tempfile, sync_all, persist) omits fsync of the parent directory after rename on Unix. Without it the rename is not crash-durable, unlike atomic-write-file, which does sync the directory.
- Hardlink or reflink backups into the app data dir only work on the same volume (reflink only on APFS, Btrfs, XFS or ReFS). Cross-drive or external-drive sources will always fall back to a full copy. This is a performance and disk-space consequence, not a correctness bug, and the plan already lists Copy as a method.
- Verified current, no error: rayon 1.12.0 (14 Apr 2026, MSRV 1.80), crossbeam-channel 0.5.17 (5 Sep 2026), flume 0.12.0 (8 Dec 2025), rusqlite 0.40.2 (8 Aug 2026), redb 4.3.0 (15 Sep 2026), undo 0.52.0 (8 Mar 2025, ~7.1k recent downloads), imbl 7.0.2 (9 Sep 2026, MPL-2.0+), gungraun 0.20.0 (26 Sep 2026), iai-callgrind 0.16.1 (30 Jul 2025), criterion 0.8.2, image-compare 0.5.0 (18 Aug 2025, MIT), tempfile 3.27.0, trash 5.2.9, proptest 1.11.0, insta 1.48.0, filetime 0.2.29, cargo-nextest 0.9.146, quick_cache 0.7.0.
- heic 'created Mar 2026' is exactly 6 months old on 2026-09-30, so 'under about 6 months' is borderline. The conclusion (do not adopt) is unaffected. The web search budget ran out during checking, so everything above rests on WebFetch of primary sources (crates.io API, docs.rs, GitHub, upstream changelogs), not on search results.

## Sources
- https://crates.io/api/v1/crates/image
- https://docs.rs/image/latest/image/struct.Limits.html
- https://docs.rs/image/latest/image/trait.ImageDecoder.html
- https://raw.githubusercontent.com/image-rs/image/main/CHANGES.md
- https://crates.io/api/v1/crates/zune-jpeg
- https://raw.githubusercontent.com/etemesi254/zune-image/dev/README.md
- https://crates.io/api/v1/crates/turbojpeg
- https://docs.rs/turbojpeg/latest/turbojpeg/struct.Decompressor.html
- https://docs.rs/turbojpeg/latest/turbojpeg/struct.Transform.html
- https://docs.rs/crate/turbojpeg-sys/latest
- https://crates.io/api/v1/crates/rayon
- https://docs.rs/rayon/latest/rayon/struct.ThreadPoolBuilder.html
- https://crates.io/api/v1/crates/crossbeam-channel
- https://crates.io/api/v1/crates/flume
- https://crates.io/api/v1/crates/rusqlite
- https://crates.io/api/v1/crates/redb
- https://crates.io/api/v1/crates/undo
- https://crates.io/api/v1/crates/imbl
- https://docs.darktable.org/usermanual/development/en/overview/sidecar-files/sidecar/
- https://crates.io/api/v1/crates/tempfile
- https://docs.rs/tempfile/latest/tempfile/struct.NamedTempFile.html
- https://crates.io/api/v1/crates/atomic-write-file
- https://docs.rs/atomic-write-file/latest/atomic_write_file/
- https://doc.rust-lang.org/std/fs/fn.rename.html
- https://github.com/rust-lang/rust/pull/134631
- https://rust.googlesource.com/rust/+/HEAD/library/std/src/sys/path/windows.rs
- https://crates.io/api/v1/crates/trash
- https://docs.rs/trash/latest/trash/
- https://crates.io/api/v1/crates/filetime
- https://crates.io/api/v1/crates/xattr
- https://crates.io/api/v1/crates/reflink-copy
- https://crates.io/api/v1/crates/dunce
- https://crates.io/api/v1/crates/walkdir
- https://crates.io/api/v1/crates/jwalk
- https://crates.io/api/v1/crates/alphanumeric-sort
- https://crates.io/api/v1/crates/natord
- https://crates.io/api/v1/crates/unicode-normalization
- https://crates.io/api/v1/crates/directories
- https://crates.io/api/v1/crates/etcetera
- https://crates.io/api/v1/crates/toml
- https://crates.io/api/v1/crates/tracing
- https://crates.io/api/v1/crates/tracing-subscriber
- https://crates.io/api/v1/crates/tracing-appender
- https://crates.io/api/v1/crates/human-panic
- https://crates.io/api/v1/crates/moxcms
- https://github.com/awxkee/moxcms
- https://crates.io/api/v1/crates/lcms2
- https://crates.io/api/v1/crates/fast_image_resize
- https://crates.io/api/v1/crates/libheif-rs
- https://crates.io/api/v1/crates/libheif-sys
- https://github.com/strukturag/libheif
- https://crates.io/api/v1/crates/heic
- https://lib.rs/crates/heic
- https://crates.io/api/v1/crates/heic-rs
- https://crates.io/api/v1/crates/heif-oxide
- https://crates.io/api/v1/crates/criterion
- https://crates.io/api/v1/crates/gungraun
- https://crates.io/api/v1/crates/iai-callgrind
- https://crates.io/api/v1/crates/proptest
- https://crates.io/api/v1/crates/insta
- https://crates.io/api/v1/crates/image-compare
- https://crates.io/api/v1/crates/dssim-core
- https://crates.io/api/v1/crates/cargo-fuzz
- https://raw.githubusercontent.com/rust-fuzz/cargo-fuzz/main/README.md
- https://crates.io/api/v1/crates/cargo-nextest
- https://docs.github.com/en/actions/reference/runners/github-hosted-runners
- https://learn.microsoft.com/en-us/windows/win32/cfapi/build-a-cloud-file-sync-engine
- https://eclecticlight.co/2017/04/06/apfs-is-currently-unusable-with-most-non-english-languages/
- https://v2.tauri.app/concept/inter-process-communication/
- https://crates.io/api/v1/crates/opencv
- https://crates.io/api/v1/crates/sysinfo
- https://crates.io/api/v1/crates/thread-priority
- https://raw.githubusercontent.com/rust-lang/rust/master/RELEASES.md