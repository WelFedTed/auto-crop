> Auto Crop design, part 3 of 8 | [PLAN.md](../../PLAN.md) | [Decision log](00-decision-log.md) | [ROADMAP.md](../../ROADMAP.md)  
> Planning draft, 2026-10-01. Numbers marked PROVISIONAL are unmeasured estimates; decisions B1-B21 and the assumptions A-1..A-12 live in the decision log and in section 1.

# 3. Image I/O, formats, HEIC and colour

This section fixes what Auto Crop reads and writes, how hostile files are decoded, how HEIC works, and how metadata and colour survive an edit. It implements decisions B2, B3, B11, B12, C1, C2 and D3-D4 and leans on sections 2.7 (commit protocol, backups, lossless path), 2.9 (worker pool), 5.9 (bilevel output), 7.7 (test pyramid), 8.1.3 (LGPL procedure), 8.6.2 (decoder isolation) and 8.7 (HEVC risk) rather than repeating them. The owner-decision assumptions A-1 to A-12 live in 1.7; the ones that touch formats (A-1 to A-6 and A-8) are pointed to where they apply. Numbers marked **PROVISIONAL** are unmeasured. A dagger (†) marks a licence or version from outside the research files, to be confirmed by `cargo-deny`, `cargo-about` or the named spike.

## 3.1 Principles and invariants

1. **Bytes, not paths.** Decoders receive a byte slice (or `Read + Seek` for very large TIFF/PNG); workers get no filesystem access; no mmap of user files (external truncation makes it unsound).
2. **Sniff, do not trust extensions.** A magic-byte sniffer identifies the format (HEIF `ftyp` brands `heic/heix/hevc/hevx/heim/heis/mif1/msf1` versus `avif/avis`; JXL codestream or container signature), and header dimensions are checked against caps before any allocation. A `.jpg` that is really a HEIC is common and is simply converted.
3. **Rust decodes everything.** The webview receives only sRGB JPEG/WebP proxies and tiles (D1).
4. **Two trust tiers (D3).** C/C++ parsers of untrusted input run in the sandboxed worker: libheif, libde265, dav1d (through libheif), later PDFium and RAW helpers. Memory-safe or heavily fuzzed decoders run in-process behind hard caps: zune-jpeg, png, tiff, image-webp, jxl-rs, turbojpeg. Encoders take our own pixels and run in-process.
5. **One decode boundary.** Every decoder returns the same `Raster`, with EXIF orientation already applied once and a colour tag attached.
6. **Nothing is written unless something changed.** An identity EditState is `Unchanged`: the first defence against generation loss under overwrite-by-default (B3).
7. **Every loss is announced** as a typed `Notice`, shown in the UI and CLI `--json` and mapped to externalised strings (B20).

```rust
struct Raster {
    w: u32, h: u32, px: Pixels,     // U8 | U16 interleaved, 1-4 channels
    alpha: AlphaMode,               // None | Straight | Premultiplied
    colour: ColourTag,              // Srgb | DisplayP3 | Icc(Arc<[u8]>) | Cicp{..} | GrayGamma(..)
    meta: MetaBlobs,                // exif (TIFF blob, prefix stripped), xmp, iptc, icc, dpi
    notices: Vec<Notice>,           // e.g. code "heic.gain_map_dropped"
}
```

## 3.2 Tiered format matrix

### 3.2.1 The matrix

R = read, W = write. "Runs in" says where untrusted bytes are parsed. jpegxl-rs and jpegxl-sys (GPL-3.0-or-later), the `heic` crate (AGPL), x265/x264 (GPL) and non-commercial code are banned outright (B2).

**Tier 1: required for v1.0**

| Format | R | W | Library (version) | Licence | Runs in |
|---|---|---|---|---|---|
| JPEG | yes | yes | zune-jpeg 0.5.15 via `image` 0.25.10 (full decode); turbojpeg 1.5.1 over libjpeg-turbo ≥ 3.1.4 (scaled decode, lossless transforms, encode); jpeg-encoder as no-C-toolchain fallback | zune-jpeg MIT OR Apache-2.0 OR Zlib; turbojpeg crate Unlicense OR MIT, C library BSD-3/IJG/zlib; jpeg-encoder (MIT OR Apache-2.0) AND IJG | in-process |
| PNG, APNG | yes | yes (APNG first frame only) | png 0.18.1 via `image` | MIT OR Apache-2.0 | in-process |
| TIFF (8/16-bit, multi-page, 1-bit) | yes | yes | tiff 0.11.3; own bilevel writer on fax 0.3.0's G4 codec | tiff MIT; fax MIT | in-process |
| WebP | yes | yes | image-webp (decode, lossless encode); libwebp lossy encode via thin FFI† | MIT OR Apache-2.0; BSD-3† | in-process |
| AVIF | yes | yes | read: libheif + dav1d†; write: ravif over rav1e | LGPL-3 + BSD-2†; ravif BSD-3-Clause, rav1e BSD-2† | read: worker; write: in-process |
| JPEG XL | yes | yes | read: jxl 0.7.4 (jxl-rs); write: libjxl 0.12.0 via our own thin FFI | BSD-3-Clause | in-process |
| HEIC, HEIF | yes | no | libheif ≥ 1.23.5 + libde265 1.1.3, our CMake build; libheif-rs 3.0.0 or own bindings (S3.2) | LGPL-3 shared libraries; wrapper MIT | worker |
| PDF | Tier 2 | yes, 1 to N pages | own streaming writer on pdf-writer 0.15.0 | permissive† | in-process |
| BMP | yes | yes | `image` | MIT OR Apache-2.0 | in-process |
| GIF, ICO | yes | no | `image` (first frame; largest entry) | MIT OR Apache-2.0 | in-process |

**Tier 1b, documented long tail (read-only, lightly tested):** TGA, PNM, QOI, farbfeld, Radiance HDR (tone-mapped to SDR), all via `image`. `image-extras` formats (DDS, ICNS, PCX) are unfuzzed and stay out.

**Tier 2, deferred to 1.x (PROVISIONAL order):**
- **1.1:** PDF page rasterisation (pdfium-render 0.9.4 + PDFium binary†, in the worker, DPI cap 300; watch hayro 0.7), because scanned PDFs are a real flatbed input; page pickers (3.8).
- **1.2:** OpenEXR (`exr`, BSD-3-Clause) with tone-map; SVG via resvg (MPL-2.0†, rasterise-only, no external references); RAW embedded-JPEG preview via rawler 0.8.0 (LGPL-2.1).
- **1.x:** Ultra HDR gain-map preservation (ultrahdr-rs†); JPEG-to-JXL lossless transcode (libjxl).

**Tier 3, best effort, no date:** full RAW development (LibRaw 0.22.2, LGPL-2.1 or CDDL-1.0), JPEG 2000 (hayro-jpeg2000 or OpenJPEG†), PSD composite (`psd` 0.3.5 is stale), HEIC write through OS APIs only (never x265), and a long tail through a user-installed libvips, never bundled.

**RAW answer.** rawler for the embedded preview, LibRaw only for a possible full develop. Both are LGPL, so both live in separate replaceable helper executables (B2). RAW is not a scanning input, so it stays off the 1.0 path.

### 3.2.2 What "all image formats" will mean in the README

> **Formats.** Auto Crop opens all common image formats: JPEG, PNG, GIF, BMP, TIFF, WebP, ICO, HEIC/HEIF, AVIF and JPEG XL, plus TGA, PNM, QOI, farbfeld and Radiance HDR. It saves JPEG, PNG, WebP, AVIF, TIFF (including 1-bit CCITT G4), JPEG XL and PDF. PDF pages, RAW, SVG and JPEG 2000 are planned, not supported yet. "All formats" is a direction, not a promise: `docs/formats.md` is the contract, and a file in an unlisted format fails with a clear message and is never modified.

### 3.2.3 Default output per source, and replace versus copy

| Source | Default output | Replaced? |
|---|---|---|
| JPEG | JPEG (lossless when eligible, else 3.9 re-encode) | yes, backup first (B3) |
| PNG | PNG | yes (1-bit PNG once its writer ships) |
| TIFF | TIFF, same compression class (1-bit stays G4) | yes (1-bit once its writer ships) |
| WebP | lossless stays lossless; lossy re-encodes with a notice | lossless yes; lossy once its writer ships |
| HEIC, HEIF | JPEG | yes, source moved to backup store (B3) |
| AVIF, JXL | same format, with a generation-loss notice (PNG offered) | once the writers ship |
| BMP, static GIF, ICO, TGA, PNM, QOI, HDR | PNG | yes (format conversion, B3) |
| Animated, multi-page, multi-image | copy only (3.8) | **never** |
| EXR, SVG, PDF, RAW, PSD, JPEG 2000 | PNG copy | **never** |

**Replaceability rule** (one rule, enforced only in `engine::FsPlan`, M2.27). A source is replaced in place only if it is a single-frame image AND this build can write its default output (table above) without dropping content.

- **Replaced with backup:** JPEG, PNG, TIFF and lossless WebP (same format); HEIC/HEIF to JPG (source to the backup store); BMP, static GIF, ICO, TGA, PNM, QOI and HDR to PNG (B3 conversion).
- **Never replaced:** animated GIF, APNG, WebP, AVIF and JXL; multi-page TIFF; HEIF with several top-level images or sequences; EXR, SVG, PDF, RAW, PSD and JPEG 2000.
- **Not replaced until the writer ships:** 1-bit TIFF and PNG (M7.28, M7.29), lossy WebP (M11.21), AVIF and JXL (M11.22, M11.25-M11.27).

Otherwise the source stays byte-identical and the item is `Skipped(NotReplaceable(reason))` in batches and in CLI default mode, with the notice `anim.first_frame_only`, `tiff.multi_page`, `heic.multi_image`, `heic.sequence` or `format.write_unavailable`. The user may opt in per file to write a copy (GUI `<folder>/AutoCrop/`; CLI `--suffix` or `--output`; PNG is the copy format for AVIF, JXL, EXR and RAW). Acceptance (M2.60): a folder with one file of each case leaves every original byte-identical under the default mode.

This is a safety rule beyond B3 (Assumption A-2, 1.7); the CLI's default of overwriting in place after a verified backup is Assumption A-1.

## 3.3 Required encoders: licence-safe routes

**JPEG.** turbojpeg encode: baseline, optimised Huffman, progressive off (Advanced toggle). Chroma `Auto`: 4:2:0 for photos, 4:4:4 when an enhancement mode is on (thin coloured text), one component for Grayscale/B&W (PROVISIONAL). The encoder emits a bare stream and our `codecs::meta` module splices JFIF density, Exif, XMP, multi-segment ICC and IPTC, so we do not depend on turbojpeg's ICC API. turbojpeg-sys 1.2.0 vendors libjpeg-turbo 3.1.0, but 3.1.4 fixed a `tj3Transform` double-free and an ICC-precedence bug ([ChangeLog](https://github.com/libjpeg-turbo/libjpeg-turbo/blob/main/ChangeLog.md)). `xtask` therefore builds libjpeg-turbo ≥ 3.1.4 (3.2.x is current), points turbojpeg-sys at it, and CI asserts the linked version. CMake and NASM are required; the devcontainer and `xtask` provide both (C7).

**PNG.** `png` 0.18.1: 8/16-bit gray, gray+alpha, RGB, RGBA; 1-bit gray for bilevel export; `iCCP`, `eXIf`, `pHYs` written.

**TIFF.** The `tiff` crate writes 8/16-bit Gray/RGB(A) with LZW (horizontal predictor for 16-bit), Deflate, PackBits or none, and multi-page through sequential images (S3.6 verifies this plus EXIF sub-IFD, XMP, ICC and IPTC tags). It cannot write CCITT, and `fax::tiff::wrap` hard-codes 200 dpi, WhiteIsZero and one strip. So **our own bilevel TIFF wrapper** (specified in 5.9) sits on fax's Group 4 row encoder:
- Tags: Compression 4 (or 1 / 32773), BitsPerSample 1, WhiteIsZero, one strip by default (best ratio), X/YResolution and ResolutionUnit **copied from the source**; with no known size, ResolutionUnit is "none", never an invented dpi. Multi-page chains IFDs with PageNumber. Classic TIFF only (a G4 file will not reach 4 GiB).
- Polarity is the classic bug: tests decode our output with three independent readers (`tiff` crate, libtiff via Pillow, ImageMagick) and require identical pixels and no inversion (5.14 owns the encoder API spike).
- 8-to-1-bit: threshold at 128 on the anti-aliased result (`bilevel_threshold`, Advanced), no dithering. The export sheet warns on large midtone regions.
- JBIG2 is not in 1.0 (rationale in 5.9).

**WebP.** image-webp gives safe decode and lossless encode. Lossy encode uses libwebp through a thin FFI (†; the `webp` crate's licence and build story are unverified), for **encode only**: CVE-2023-4863 was a libwebp decoder bug, so decode stays in image-webp. If the lossy spike fails on any OS, lossless-only WebP is the fallback, labelled "WebP (lossless)"; it narrows B11, so it is the owner's decision (A-4).

**AVIF.** ravif over rav1e. Pin bit depth, chroma model and quantiser mapping explicitly. Exif/ICC/XMP writing through ravif and avif-serialize is unverified (S3.5); the fallback is our own ISOBMFF box insertion. rav1e is the slowest writer: expect several seconds per 12 MP (PROVISIONAL), so run it on the batch pool with cancellation.

**JPEG XL (B11).** jpegxl-rs and jpegxl-sys are **GPL-3.0-or-later** and cannot be used. The route is **our own thin FFI to libjxl 0.12.0 (BSD-3)**: bindings for about a dozen `JxlEncoder*` calls (frame settings, basic info, colour encoding or ICC, `AddImageFrame`, `AddBox` for `Exif` and `xml `, lossless and distance, `ProcessOutput`), a rayon-backed parallel runner, and an encoder-only CMake build (option names to confirm for 0.12.0). Orientation is written as identity, and the `Exif` box uses the same 4-byte TIFF-offset prefix as HEIF. Bundled dependencies (highway, brotli, skcms or lcms2) are believed permissive† and audited in S3.4; `xtask` writes their notices, since cargo-about sees only Rust crates. A `JxlEncoderBackend` trait lets a permissive Rust encoder replace the FFI later (none is verified; zune-jpegxl† is lossless-only). If the FFI fails on any OS, lossless-only JXL is the fallback; it narrows B11 and is the owner's decision (A-4).

**PDF.** No mature permissive writer streams multi-page output, so we use pdf-writer's serialisers if its `Chunk` API allows per-page flushing, else a standalone ~400-line writer (S3.7).
- One image XObject per page; MediaBox = pixels / dpi × 72 pt, with nominal 300 dpi when the dpi is unknown (as in 5.9) and an Advanced "fit to A4/Letter".
- **JPEG pages** embed the encoder's bytes unchanged (`DCTDecode`), EXIF stripped, ICC moved to an `ICCBased` colour space. **Lossless pages** embed the PNG `IDAT` stream as `FlateDecode` with `Predictor 15` (the img2pdf technique), so nothing is compressed twice. **Bilevel pages** reuse the TIFF path's G4 bytes (`CCITTFaxDecode`, `K -1`).
- Pages stream to disk as produced; xref and catalogue come last, so 200 pages of 12 MP stay within the 500 MB budget (3.11). `Info` holds Producer and date only, never GPS. No text layer (B19), but the page builder leaves room for invisible text later.

**Presets (PROVISIONAL)**, mapping the UX Small/Balanced/Best. The JPEG column holds the fixed values used for every source that is not a JPEG and for Custom; a JPEG source follows the source-relative rule under the table.

| Preset | JPEG | WebP lossy | AVIF quality/speed | JXL distance/effort |
|---|---|---|---|---|
| Small | q80, 4:2:0 | q75 | 60 / 6 | 1.8 / 5 |
| Balanced (default) | q90, Auto chroma | q85 | 75 / 6 | 1.0 / 6 |
| Best | q95, 4:4:4 | q92 | 88 / 5 | 0.5 / 7 |

**JPEG sources are source-relative (PROVISIONAL, set by the preset sweep M11.63).** The in-place default (B3) is JPEG to JPEG, where the quality already in the file decides generation loss, so for a JPEG source the presets follow the estimated source quality `q_est` (3.9): Balanced = `clamp(q_est + 5, 80, 95)`, Small = `clamp(q_est - 10, 60, 85)`, Best = `clamp(q_est + 10, 90, 97)`. Subsampling and the progressive flag stay as in the source unless an enhancement mode is on (then 4:4:4, or one component for Grayscale and B&W); Huffman tables are optimised; a low-confidence `q_est` falls back to the fixed column. A lossless transform (3.9) re-encodes nothing, so no quality applies.

**Gate for the B11 encoders.** AVIF, JXL and lossy WebP stay in 1.0 (B10), so each needs, before the RC: (a) output decoded correctly by an independent permissive reference (`dwebp`, `avifdec`/dav1d, `djxl`): bit-exact if lossless, thresholded if lossy; (b) building and passing on all three OSes; (c) meeting its PROVISIONAL time cap; (d) metadata and ICC round-tripping. A failed gate follows Assumption A-4 (1.7): 1.0 is delayed, and shipping the encoder labelled Experimental or moving it to 1.x reopens B10 and is the owner's decision, put with measured numbers, never a silent cut.

## 3.4 HEIC and HEIF design (B12)

### 3.4.1 Architecture

```rust
trait HeicBackend: Send {
    fn id(&self) -> &'static str;            // "libheif" | "wic" | "imageio"
    fn caps(&self) -> HeicCaps;              // hevc, av1, thumbnails, aux_images
    fn probe(&self, bytes: &[u8], lim: &DecodeLimits) -> Result<HeifProbe, DecodeError>;
    fn thumbnail(&self, bytes: &[u8], lim: &DecodeLimits) -> Result<Option<Raster>, DecodeError>;
    fn decode_primary(&self, bytes: &[u8], want: PixelWant, lim: &DecodeLimits,
                      cancel: &CancelToken) -> Result<HeifDecoded, DecodeError>;
}
```

`LibheifBackend` is the default on all OSes. `WicBackend` (`windows` 0.62) and `ImageIoBackend` (`objc2-image-io` 0.3.2) are **optional fast paths** chosen by `heic.engine = bundled | system | auto`. The default is **bundled**: it behaves identically on all three OSes and works on Windows machines without the HEVC extension (whose price and availability are unverified; we never buy it, B15). `system` uses the OS codec where one exists. `auto` is available only after a parity gate on the real-device corpus (ΔE2000 between backends mean ≤ 0.5 and p99 ≤ 1.5, identical orientation and metadata; PROVISIONAL): it then uses an OS backend that has passed the gate and the bundled one otherwise. Whether WIC's Store-delivered codec or ImageIO loads inside our sandbox is unverified (S3.3). All backends run in the worker (2.9). We never encode HEVC.

Pure-Rust decoders (heic-rs 0.1.1 MIT OR Apache-2.0, hpvcd, heif-oxide) are weeks to months old, and the `heic` crate is AGPL. **Re-evaluate around March 2027** (bar: full corpus at ΔE parity, a fuzzing record). That would remove the LGPL binaries but not the patent question.

### 3.4.2 Our own decode-only libheif build

`xtask build-native` builds libheif ≥ 1.23.5 and libde265 with CMake (MSVC on Windows) into `deps/prefix/<target>/`. Option and API names are to be confirmed against 1.23.5 in S3.1.

- `BUILD_SHARED_LIBS=ON`, `ENABLE_PLUGIN_LOADING=ON` and `WITH_LIBDE265_PLUGIN=ON`: libde265 is a **separate shared library loaded as a libheif plugin**, as 2.9 assumes. The worker loads plugins from one absolute, application-relative directory, before the sandbox applies, and ignores `LIBHEIF_PLUGIN_PATH`. If plugin loading proves unreliable on any OS, fall back to `WITH_LIBDE265=ON` linked directly as a shared library (8.1.3).
- `WITH_DAV1D=ON` for AVIF, folded into libheif (BSD-2†). `ENABLE_PARALLEL_TILE_DECODING=ON`. Examples and tests off.
- **Every encoder OFF** (x265, x264, kvazaar, aom, rav1e, SVT, JPEG, J2K), and other decoders (VVC, AVC, JPEG, JPEG 2000 in HEIF) off, so those files fail with "unsupported HEIF codec".
- **Never `libheif-sys` embedded mode.** It links libheif statically, bundles 1.23.1 (four security releases behind under a latest-only fix policy), turns plugin loading off and force-enables x264/x265 detection ([build.rs](https://github.com/Cykooz/libheif-sys/blob/master/build.rs)). On Windows-MSVC it always uses vcpkg, whose libheif port defaults to the `hevc` feature, meaning GPL-2.0+ x265 ([vcpkg port](https://github.com/microsoft/vcpkg/blob/master/ports/libheif/vcpkg.json)); its Windows override `libheif[aom]` still inherits that default.
- libheif-rs 3.0.0 with `default-features = false`, or our own bindgen crate over about 30 functions if libheif-sys cannot be pointed at our prefix without vcpkg (S3.2).
- **CI guards:** `cargo xtask check-native` (8.1.3) compares loaded libraries and greps for `x265_`/`x264_` symbols; a runtime test asserts libheif reports no HEVC or AVC encoder; `cargo tree -e features` fails on `embedded-libheif`.

**LGPL-3.** The relink, source-archive and notice procedure is in 8.1.3. What this section adds: only the `auto-crop-worker` process links libheif, the Windows loader is restricted to the application directory and System32 (`SetDefaultDllDirectories`), and Flatpak bundles its own libheif and libde265 because the Freedesktop runtime builds libheif without libde265.

**`hevc` feature and the `no-hevc` variant.** Official builds always enable the Cargo feature `hevc` and bundle libde265 (B12); there is no automatic fallback to `no-hevc` (Assumption A-5, 3.4.4). With the feature off (8.7) and no libde265 plugin file, the `no-hevc` variant serves distro packagers or an owner-decided fallback: AVIF still decodes; an HEVC HEIC fails with `HevcDecoderMissing` and the UI points to `heic.engine = system` where the OS has the codec. Distro packagers may link a system libheif (`system-libheif` feature) at their own risk. There is no vcpkg anywhere in the build, and bundled libheif is the default on every OS.

### 3.4.3 Security-update duty and advisory tracking

libheif published 61 advisories from January to September 2026 (4 critical, 20 high), has one largely unfunded maintainer (almost no recurring funding) and patches only its latest release ([SECURITY.md](https://github.com/strukturag/libheif/blob/master/SECURITY.md)); libde265 reportedly had 13.

- **Pins and watching.** `native-deps.toml` records tag and SHA-256 for libheif (minimum 1.23.5, released 2026-09-21), libde265, dav1d, libjpeg-turbo, libjxl, libwebp and ONNX Runtime; About and `--version --verbose` print them. A scheduled workflow compares pins with upstream releases and the GitHub advisory feed and opens a `security-native` issue.
- **SLA (PROVISIONAL).** Critical and high: patched release within 7 days of the upstream fix (72 h target for critical); moderate and low: next regular release, at most 30 days. A tag-triggered rebuild-and-release path makes this feasible for one maintainer.
- **Honest load.** 24 critical or high advisories in about nine months implies a native security release every one to two weeks if all apply. Mitigations: the sandbox confines memory-safety bugs to a low-privilege worker process (3.10.2); high items may batch weekly; users can set `heic.engine = system` where the OS has the codec.
- **What the sandbox blocks.** Only AppContainer, Landlock with seccomp and `sandbox_init` also block network and filesystem access. The Windows job object with a restricted token does not block network access or reads of user files, and macOS and Linux run process-only until M8 and M9. The achieved level (`appcontainer`, `job+token`, `landlock+seccomp`, `seccomp-only`, `sandbox_init` or `process-only`) is shown in About and `doctor` (8.6.2).
- **Exit criterion for B12.** If the SLA is missed two quarters running, reopen the decision with the owner (there is no automatic `no-hevc` fallback, A-5).
- **Stale installs.** With no auto-update (B18), old builds keep old libheif: Settings shows a network-free banner once the build is over 120 days old (PROVISIONAL), and the opt-in weekly check flags security releases.

### 3.4.4 HEVC patent-risk note and legal task

HEVC is covered by third-party patents. Wikipedia reports a 2016 HEVC Advance policy exempting software distributed directly to consumers, but pools have since consolidated (trade press: Access Advance took over Via LA's HEVC pool in December 2025) and **no verified FOSS safe harbour** was found. Bundling libde265 is a distributor risk the copyright licence does not address. Designed-in mitigations: decode-only, the Cargo feature `hevc` (off gives the `no-hevc` variant, 3.4.2), `heic.engine = system` where the OS has the codec, no HEVC encoder ever, and never buying the Windows HEVC extension (B15).

**Assumption A-5:** Official builds always bundle libde265 (B12). The $0 legal read (pool policies re-read with dates, any free-clinic reply, gap disclosed) is due before the first published HEVC build if practicable, no later than 1.0; a `no-hevc`-only official release needs the owner's recorded decision. (owner may veto)

**Legal read:** a short read on distributing a decode-only HEVC decoder in a free desktop app, including Microsoft Store and macOS distribution (whether Store certification accepts a bundled HEVC decoder is unverified; the hidden Store dry run is Assumption A-6). B15 forbids paying, so the read is the $0 route of A-5 (8.11). Outcomes: proceed (B12 stands) or, if the read finds unacceptable exposure, the owner decides whether to reopen B12 (`no-hevc` as the default with libde265 as a separate download, or a Store-only `no-hevc` build). There is no automatic fallback, and nothing changes without that recorded decision. README and SECURITY wording is in 8.7.

### 3.4.5 Apple and HEIF traits: kept, dropped, and what the user sees

| Trait | Handling | User sees |
|---|---|---|
| Grid tiles (iPhone 512 px, about 48 at 12 MP), overlays | libheif composes; memory admission counts the full canvas | nothing |
| `irot`, `imir`, `clap` | applied by libheif; EXIF Orientation ignored for HEIF; output Orientation = 1 (3.6) | nothing |
| EXIF, XMP | kept per 3.6; 4-byte TIFF-offset prefix stripped | nothing |
| Display P3 | ICC (`prof`) first, then nclx/CICP; default to sRGB (3.5), toggle preserves | batch summary line |
| 10/12-bit, PQ or HLG HDR | 16-bit decode, own tone-map to SDR (3.7) | Warn: HDR converted |
| Apple gain map (auxiliary image) | SDR base kept, gain map dropped. libheif's ISO 21496-1 support is unmerged (PR #1503) and Apple's maths is undocumented, so we do not reconstruct HDR | Info: gain map dropped |
| Depth, mattes | dropped | Info |
| Live Photo | still only; paired `.MOV` (same stem or ContentIdentifier) untouched | Info: video left as is |
| Burst | separate files, ordinary items | nothing |
| Sequences (`msf1`), several top-level images | primary only; never replaced (3.2.3, 3.8), notice `heic.sequence` or `heic.multi_image` | Warn |
| Alpha | flattened on white for JPEG; kept for PNG, WebP, AVIF, TIFF, JXL | Info for JPEG |
| Embedded thumbnail | draft preview and grid only (draft ≤ 150 ms, PROVISIONAL); never copied to output | nothing |
| Unsupported codec (VVC, AVC, JPEG in HEIF, J2K) | item fails, source untouched | Issues tray |

## 3.5 HEIC to JPG conversion behaviour (B3, C2)

Pipeline: probe → decode in worker → colour transform → encode → metadata splice → the conversion protocol of 2.7 (encode and verify the temp, back up the source, no-clobber rename, unlink the source; journalled and crash-recoverable). What this section adds:

1. **Naming and times.** `IMG_1234.HEIC` becomes `IMG_1234.jpg`; a collision follows the `collision` setting of the output spec (default Rename: a numeric suffix per 2.7, never overwriting an unrelated file). The output mtime is copied from the source.
2. **Quality.** Balanced: q90 (the fixed value, since a HEIC is not a JPEG source, 3.3), optimised Huffman, chroma matching the source (iPhone HEIC is 4:2:0, so nothing more is lost). Target ΔE2000 versus a reference decode: mean ≤ 0.5, p99 ≤ 1.5, max ≤ 3 (PROVISIONAL).
3. **Colour (C2).** Default: convert Display P3 or any wide-gamut source to sRGB (relative colourimetric, clipping) and tag Exif ColorSpace = 1. HEIC/HEIF to JPG and enhanced output are the only places where sRGB is the default (3.7). Toggle **"Preserve wide gamut (Display P3)"**: pixels untouched, source ICC (or a P3 profile synthesised from nclx) embedded. **Fallback:** if the embedded profile cannot be parsed or applied, keep pixels, embed the original ICC unchanged, emit a Warn.
4. **Metadata (C2).** Keep everything except Orientation (applied once, reset), thumbnails and MPF, and the dimension fields (rewritten to the output). GPS is kept by default. A visible one-click **"Strip location"** toggle (off) sits in the export sheet; C2 makes it off by default though the UX research favoured on. When a batch has GPS-tagged files the export bar shows the chip "N files contain a location" with the toggle inline, and the choice is remembered. The sheet's metadata choice is: Keep all except orientation and thumbnail (default), Keep but strip location, or Strip all. Complete strip-location ships with the metadata carry-over in M2.23, and M6.27 keeps the HEIF parts (3.6).
5. **Notices and restore.** Each dropped trait in 3.4.5 appears in the batch summary and per-item detail, and the CLI prints the same codes. Restore original is the 2.7 flow.

## 3.6 Metadata: EXIF, XMP, IPTC, ICC, orientation

`MetaBlobs { exif, xmp, iptc, icc, dpi }`. Parsing uses kamadak-exif† (BSD-2). The write-capable EXIF ecosystem is thin and re-serialising an IFD can break MakerNote pointers, so our own `exif_patch` edits the TIFF blob **in place**.

- **Policy (C2).** Keep everything except Orientation (reset), thumbnails and MPF, and the dimension fields (rewritten). GPS is kept by default, and "Strip location" is visible, one click and off (3.5). The export sheet offers Keep all except orientation and thumbnail (default), Keep but strip location, and Strip all; Strip all removes EXIF, XMP and IPTC but keeps the ICC profile and dpi.
- **Orientation applied once, then reset.** Non-HEIF sources: orientation (1-8, including mirrored 2/4/5/7) is applied at the decode boundary, geometry lives in the oriented space, and every output writes Orientation = 1 (EXIF, XMP `tiff:Orientation`, TIFF tag 274, PNG `eXIf`, WebP EXIF, JXL basic info; AVIF and PDF emit no rotation). HEIF: libheif has already applied `irot`/`imir`, so the EXIF tag is informational and passing it through would double-rotate ([pillow-heif note](https://pillow-heif.readthedocs.io/en/latest/workaround-orientation.html)). The lossless JPEG path rotates through turbojpeg then patches the tag, never both.
- **TIFF-offset prefix.** HEIF, AVIF and JXL Exif payloads start with a 4-byte big-endian offset to the TIFF header. Skip that many bytes after the field and check `II*\0` or `MM\0*`; if wrong, scan the first 32 bytes; if still wrong, drop EXIF with a Warn. JPEG APP1 is `Exif\0\0` + TIFF blob; PNG and WebP carry a bare blob.
- **In-place edits:** Orientation, PixelX/YDimension (and TIFF dimension tags), ColorSpace, and IFD0's next-IFD pointer (detaches IFD1 and its thumbnail). Removed regions are **zero-filled**, so stripped GPS or thumbnail bytes are unrecoverable. The `Software` tag ("Auto Crop x.y", the 2.7 idempotency chip) is written only when it fits in place; otherwise the journal alone drives idempotency. Above the 65,527-byte JPEG APP1 limit, rebuild a minimal EXIF (Make, Model, dates, exposure, lens, GPS unless stripped) with a Warn.
- **Stale data.** Drop the EXIF thumbnail, Photoshop 8BIM thumbnails, `xmp:Thumbnails` and any MPF APP2 (Ultra HDR gain maps use it; offsets break after any edit).
- **Strip location.** Removes the GPS IFD, XMP `exif:GPS*` and location-shown properties, and IPTC city, province, country and sublocation. Property test: no source GPS byte pattern survives. The complete removal ships in M2.23 (JPEG, PNG, TIFF, WebP); M6.27 keeps the HEIF parts.
- **Split outputs.** `ImageUniqueID` is dropped on the outputs of a 1-to-N split: a deliberate exception to "keep everything", because several files must not claim one image identity.
- **XMP** over 65,502 bytes cannot fit a JPEG APP1 without Extended XMP: after dropping thumbnails, drop it with a Warn. **IPTC** stays in an 8BIM APP13.
- **ICC** is preserved byte-exact where the target carries it (JPEG multi-segment APP2, PNG `iCCP`, TIFF tag 34675, WebP `ICCP`, AVIF `colr`, JXL, PDF `ICCBased`), else converted to sRGB with a notice. **DPI** is copied through crop and deskew and rescaled if the output is resampled.

## 3.7 Colour management, 16-bit and HDR

**Crates.** `moxcms` 0.9.1 (BSD-3/Apache, pure Rust, already used by `image`) is the runtime CMS. `lcms2` 6.2.0 (C library, MIT†) is a **dev-only test oracle** in 1.0: parsing attacker-supplied ICC blobs in C in-process is unneeded risk until moxcms coverage proves insufficient. `qcms` is stagnant since 2024 and unused. Unusable profiles fall back to sRGB (RGB) or gamma 2.2 (gray) with a Warn; S3.8 measures how often. Pre-checks: `acsp` signature, size ≤ 16 MiB (PROVISIONAL), version and device class.

**Resolution order.** Embedded ICC, then CICP/nclx, then format defaults (JPEG Exif ColorSpace, with interop index `R03` as an Adobe RGB hint; PNG `cICP`/`sRGB`; else sRGB). H.273 codes: primaries 12 (P3-D65) and 9 (BT.2020); transfer 13 (sRGB), 16 (PQ), 18 (HLG). Sources disagree on whether iPhone stills carry ICC or nclx (libheif issue #566 has a file with no nclx), so both are read.

**Where colour converts.**
- Webview proxies and tiles are always converted to sRGB in Rust, so preview equals sRGB export (D1).
- **The sRGB default (C2) covers two cases only:** HEIC/HEIF to JPG (3.5, with the "Preserve wide gamut (Display P3)" toggle) and enhanced output (next bullet).
- **Every other source keeps its profile and pixels**, in same-format edits and in conversions alike (an Adobe RGB JPEG re-saved as JPEG, an AVIF converted to PNG). The ICC is carried byte-exact where the target carries it (3.6), else the pixels are converted to sRGB with a notice; a lossless JPEG transform cannot convert anyway. PQ and HLG sources are tone-mapped to SDR with a notice (below).
- Every enhancement mode (Auto, Grayscale, B&W) outputs sRGB or gray, because enhancement assumes sRGB luma weights.
- Resampling runs in gamma-encoded space (parity with other tools, half the cost); pic-scale's linear-light path is a perf-spike candidate. Target: matrix-shaper transform ≤ 60 ms per 12 MP (PROVISIONAL).

**16-bit and HDR.** Integer sources up to 16-bit decode to `U16`. Geometry-only edits keep 16-bit where the target supports it (PNG, TIFF, JXL); enhancement modes output 8-bit; JPEG and WebP reduce with a fixed 4×4 ordered dither (PROVISIONAL). EXR and Radiance HDR are tone-mapped at decode. PQ and HLG need a real tone-map because libheif's 8-bit conversion does not tone-map: request 16-bit output, linearise, scale 203-nit reference white (BT.2408) to 1.0, roll off highlights with a soft knee, map BT.2020 to the target gamut and apply the sRGB OETF (about 150 lines; validated in S3.9). No HDR output in 1.0. CMYK and YCCK JPEG (M6.83) and CMYK TIFF decode to sRGB with a Warn, through an ICC transform if moxcms supports CMYK LUTs (unverified), else a naive conversion, where the chosen decoder handles them (S3.10 for JPEG, S3.6 for TIFF); otherwise the read fails with a typed `UnsupportedFormat` naming the reason. CMYK is never written.

## 3.8 Animated and multi-page inputs

`SourceRef` already carries `frame: u32`, so page-as-item fits the data model later.

- **v1.0:** the first frame or page is processed. Animated GIF, APNG, WebP, AVIF sequences, JXL animations, multi-page TIFF and HEIF files with several top-level images show a Warn ("page 1 of N") and are **never replaced** (rule in 3.2.3, Assumption A-2): the result is a new file, so no frames or pages are lost even though a backup would exist. In batches and CLI default mode they are `Skipped(NotReplaceable(reason))` with the notice `anim.first_frame_only`, `tiff.multi_page`, `heic.multi_image` or `heic.sequence`, unless the user opts in per file to write a copy.
- **Tier 2 (1.1):** page picker, all-pages-as-items, multi-page write-back (TIFF, PDF). Flatbed scanners often emit multi-page TIFF, so this is a candidate for late 1.0 (local assumption A2, 3.13). Combining N results into one PDF is separate and in 1.0.

## 3.9 Lossless JPEG transforms and the recompression policy

Eligibility is defined in 2.7: JPEG to JPEG, enhancement off, no resize or perspective, `fine_deg` under 0.05, and a 90-degree step, flip or axis-aligned crop. Deskew and perspective are never lossless, so for phone photos of receipts the re-encode rules below matter most.

- **Mechanism.** turbojpeg `Transform` with markers copied, then the 3.6 patch. Crop origins snap outward to the iMCU grid (8 or 16 px by subsampling; [jpegtran man page](https://github.com/libjpeg-turbo/libjpeg-turbo/blob/main/doc/jpegtran.1)); growth is accepted up to 16 px or 0.5% of the short side (2.7), else re-encode. The planner resolves the snapped rectangle into `ResolvedGeometry` **before** rendering, so preview equals output, and the UI shows a "lossless" badge and any growth. Use `perfect`, never silent `trim`.
- **Quality estimation.** Our own JPEG marker parser (also used for metadata, scan counts and subsampling; M2.80) reads the quantisation tables, finds the IJG quality whose scaled tables match best (least squares on log ratios) and returns `(q_est, confidence)`. `q_out` follows the source-relative preset rules in 3.3 (Balanced is `clamp(q_est + 5, 80, 95)`; Small and Best shift it), PROVISIONAL, tuned for median PSNR ≥ 40 dB versus the decoded source region. The source subsampling and progressive flag are kept. Non-standard tables (cameras, Photoshop) get low confidence and fall back to the fixed preset value (q80, q90 or q95).
- **Other lossy sources** (WebP lossy, AVIF, JXL) have no cheap estimator: use the preset with a generation-loss notice.
- **Avoiding avoidable generations.** Identity edits are not written; one resample, one encode, one metadata write; the idempotency guard of 2.7 (journal hash lookup, plus the `Software` chip) prompts "already processed" instead of silently recompressing; failed and low-confidence items are never auto-written (B4; confident items save after the whole batch is triaged, Assumption A-3).

## 3.10 Decoder security

### 3.10.1 Limits

| Limit | Default | Notes |
|---|---|---|
| `max_pixels` | 100 MP | The tested class (C1, as in 2.8 and 8.6.1); above it the error names the cap and offers "allow this file". The Advanced setting or CLI `--max-pixels` raises it, up to a 500 MP hard ceiling that is also the fuzz robustness target. Hostile fixtures (a header claiming 60000×60000, a scan-bomb JPEG) are rejected within 1 s and 64 MB (7.7) |
| Decoded bytes | admission weight `pixels × 9 + 64 MiB` (2.8) | oversize job runs alone, else rejected |
| File size | 256 MiB (worker formats); 2 GiB (streamed TIFF/PNG) | PROVISIONAL |
| Timeout | worker: `10 s + 1 s/MP` (2.9), killed; in-process: soft, result discarded | PROVISIONAL |
| Metadata blobs | EXIF, XMP, ICC parse cap 16 MiB each | JPEG output caps in 3.6 |
| Frames, pages, items | probe up to 10,000; decode frame 0 | |
| JPEG progressive scans | ≤ 100 | own marker scan; libjpeg-turbo's scan-limit parameter† as second guard |
| PNG, TIFF | explicit `png` and `tiff` `Limits` | `image`'s 512 MiB alloc limit is non-strict and dimensions are unlimited by default, so it is never the only guard |
| libheif | `heif_security_limits` always set (pixels, tiles, memory block, items, colour-profile size) | never disabled for untrusted files |

Declared dimensions are checked before allocation, so a PNG or TIFF header claiming 65535×65535, a HEIC grid with excessive tiles, or a tiny palette PNG with a huge IDAT is rejected cheaply; a bomb corpus runs in CI (8.6.1). Long loops check the `CancelToken` per 64-row band.

### 3.10.2 Isolation

Worker protocol, shared-memory pixel return, pool size, per-OS sandboxes (Windows job object with a restricted token, Linux Landlock with seccomp, macOS `sandbox_init`, plus a parent RSS watchdog) and the `process-only` fallback are specified in 2.9 and 8.6.2. The worker is a low-privilege process: only AppContainer, Landlock with seccomp and `sandbox_init` also block network and filesystem access, a job object with a restricted token does not, and macOS and Linux run process-only until M8 and M9 (level strings in 3.4.3). Workers are recycled after any anomaly, after a decode above 50% of a cap and every 32 decodes. `birdcage` is unusable (GPL-3.0, archived July 2026). I/O-specific rules:

- The parent reads the file once and passes it in; the worker writes only into an output mapping sized from the parent's own probe, and any size mismatch is an error.
- A crash, timeout or limit hit fails that item only (`Failed(kind)`), leaves the source untouched, respawns the worker and marks the content hash do-not-auto-retry; a file that crashes twice is offered as "Report this file". Crash counts stay local (C4).
- **In-process decoders cannot be pre-empted.** The guarantee is bounded work through the caps above, not a kill switch (8.6.1). `decode.isolation = native-only | all` (default native-only) routes JPEG, PNG, TIFF, WebP and JXL through the worker too, and CLI `--isolate` enables it for hostile inputs. The worker protocol is codec-agnostic, so this is configuration, not new code.

### 3.10.3 Fuzzing plan

- **Targets** (cargo-fuzz on Linux and macOS, nightly Rust; not Windows): `sniff`; `jpeg_meta`; `exif_patch` (structure-aware); `icc_validate`; `heif_probe` and IPC framing; `tiff_bilevel` (arbitrary bitmaps round-trip); `pdf_write` (checked with qpdf, Apache-2.0); `decode_pipeline` (bytes → sniff → decode under limits → proxy).
- **Sanitisers.** ASan builds with the C libraries compiled under ASan for worker targets; `cargo miri` on the small shared-memory `unsafe`. libheif and libde265 have upstream fuzzing, but our glue and limits are ours to fuzz.
- **Cadence** follows 7.7: 60 s smoke on codec PRs, one hour per target nightly, crashers replayed as ordinary tests on all OSes (covering Windows). Goal: zero crashes, hangs or over-cap allocations, including 500 MP declared sizes, over at least 72 h cumulative per target before 1.0 (nightlies count).

## 3.11 Test corpus and per-format acceptance criteria

**Policy (B21; corpus strategy in 7.5).** The public repo holds only synthetic files from our generator, permissively licensed public files, and small HEIC fixtures made on the macOS CI runner with `sips`/ImageIO (no GPL encoder, no x265), at most 5 MB. Larger sets come from `cargo xtask fetch-corpus` with pinned SHA-256. Nokia HEIF conformance files have no licence, so they stay local. The private real-device set (iPhone and Android: P3, HDR, gain map, Live Photo, burst, grid; GPS kept by recorded exception) follows the private golden-set rules of Assumption A-8 (7.5): its images stay on the maintainer's disk, the HEIC corpus runs only on the maintainer's Mac, it never runs on public CI or fork PRs, and only aggregates are published. Independent checkers: `dwebp`, `avifdec`, `djxl`, libtiff tools, Pillow, qpdf, PDFium, pdf.js.

| Format | Fixtures | Acceptance |
|---|---|---|
| JPEG | baseline, progressive, all subsamplings, gray, YCbCr, orientations 1-8 | 100% decode; orientation 100%; lossless rotate bit-exact; zune-jpeg vs turbojpeg ≤ 2/255 (PROVISIONAL); ICC byte-exact; quality estimator within ±3 on IJG-table files |
| JPEG variants (M6.83) | CMYK, YCCK, arithmetic-coded, 12-bit | a typed `UnsupportedFormat` naming the reason, never a panic, source untouched, unless spike S3.10 shows a decoder handles the variant (turbojpeg with arithmetic decoding compiled in): then it decodes, CMYK and YCCK to sRGB with a Warn, and the row is tightened to 100% decode |
| PNG | PngSuite (permissive licence, verify); 1-16-bit, palette, Adam7, APNG, `iCCP`/`cICP`/`eXIf` | lossless bit-exact; corrupt files fail without panic |
| TIFF | G4, G3, none, PackBits, LZW, Deflate, JPEG; 1-bit, gray 8/16, RGB, palette, CMYK, planar, tiled, BigTIFF, multi-page, MinIsWhite | 1-bit G4 reads via `tiff` directly; our G4 decodes identically in three readers; dpi and photometric preserved 100%; no inversion in Windows Photos, Preview, Okular (manual) |
| WebP | lossy, lossless, alpha, animated | lossless bit-exact; lossy within 50 dB PSNR of `dwebp` (PROVISIONAL); animated becomes `Skipped` |
| AVIF | libavif/libheif samples (verify licences); 8/10-bit, alpha | worker decode within ΔE ≤ 0.5 of `avifdec`; our output decodes in dav1d and browsers |
| JPEG XL | libjxl testdata (verify licences); lossless, lossy, alpha, 16-bit | `djxl` decodes ours; lossless bit-exact; lossy above an SSIM floor; Exif prefix correct |
| HEIC | private corpus plus `sips` fixtures: 8 orientations × `irot`/`imir`, grid, P3 (ICC and nclx), 10-bit, gain map, depth, Live Photo, alpha | ΔE2000 mean ≤ 0.5, p99 ≤ 1.5, max ≤ 3 versus ImageIO; EXIF/ICC kept, orientation and notices 100%; grid seams ≤ 2/255; 12 MP overlay ≤ 700 ms (PROVISIONAL) |
| PDF | 1, 10 and 200 pages (200 × 12 MP for the memory test); JPEG, lossless, G4; unknown dpi | `qpdf --check` passes; PDFium renders the expected size at SSIM ≥ 0.98; opens in Chrome, Edge, Preview, Evince; 200 pages of 12 MP peak at ≤ 500 MB RSS (PROVISIONAL) |
| Metadata | GPS, MakerNotes (Apple, Canon, Nikon), IFD1 thumbnails, 64 KB+ EXIF, extended XMP | non-policy tags survive parse-write-parse; stripped GPS leaves no source bytes; APP1 limits give a Warn |
| Robustness | fuzz corpora, bombs, truncated files | zero crashes or hangs; hostile fixtures rejected within 1 s and 64 MB; every failure typed, source untouched |

## 3.12 Risks per format

| Format or area | Risk | Mitigation |
|---|---|---|
| HEIC, HEVC | vulnerability churn, patents, packaging traps (x265 via vcpkg, stale embedded libheif) | 3.4: sandbox (level shown in About), own build, SLA and exit criterion, CI guards, $0 legal read (A-5), `no-hevc` only for distro builds or an owner decision |
| AVIF, JXL, lossy WebP writers | rav1e speed and memory; JXL FFI on three OSes; metadata writing and WebP FFI unverified | time gates, own box insertion, backend trait; a lossless-only JXL or WebP fallback is the owner's decision (A-4) |
| JXL and WebP readers | jxl-rs is young (default-off in release browsers); libwebp decoder CVE history | fuzz, caps, `isolation = all`; libwebp for encode only |
| JPEG | libjpeg-turbo transform bugs before 3.1.4; two decoders differ by 1-2 levels; CMYK, YCCK, arithmetic and 12-bit files may not decode | pin ≥ 3.1.4 with CI check; parity test; typed failure unless S3.10 shows support (M6.83) |
| PNG, TIFF | panics or heavy allocation on hostile input; thin 1-bit G4, CMYK, 16-bit paths; G4 inversion and dpi loss | explicit limits, `catch_unwind`, direct `tiff` use, own G4 wrapper, three-reader test |
| PDF | memory growth, viewer compatibility | streaming writer, qpdf and PDFium checks |
| Metadata | double-rotation, wrong prefix, stale thumbnails, MakerNote breakage, location in dead bytes | in-place patcher, zero-fill, property and fuzz tests |
| Colour and HDR | undocumented gain-map maths, tone-map errors, moxcms gaps | drop with notice, ΔE tests, S3.8 |
| RAW, PDF input, 100 MP jobs | LGPL helper packaging; PDFium size; peaks near 1 GB per job | separate helpers, deferred to 1.x; byte-weighted admission |

## 3.13 Spikes and assumptions to veto

**Spikes** (each should become a roadmap item):
- **S3.1** Build libheif ≥ 1.23.5, libde265 (plugin, with the direct-link fallback) and dav1d with the 3.4.2 options on Windows (MSVC), macOS arm64 and Linux; confirm option and API names and that no HEVC or AVC encoder exists.
- **S3.2** Point libheif-sys at our prefix without vcpkg on Windows, else write the bindgen crate.
- **S3.3** Worker sandbox per OS: WIC and ImageIO inside it, shared memory under AppContainer, Landlock and seccomp allow-lists.
- **S3.4** libjxl encoder on three OSes: build, bundled-dependency licences, effort versus time at 12 MP.
- **S3.5** ravif metadata (Exif, ICC, XMP) and speed at 12 MP; libwebp lossy FFI and licence.
- **S3.6** `tiff` crate: multi-page write, 16-bit predictor, EXIF sub-IFD, XMP/ICC/IPTC tags; 1-bit G4, G3 and CMYK reads.
- **S3.7** pdf-writer streaming (`Chunk`) versus a standalone writer; PNG `IDAT` embedding in four viewers.
- **S3.8** moxcms coverage (P3, nclx-synthesised, CMYK LUT, gray, v4) against lcms2; corpus rate of unusable profiles.
- **S3.9** PQ/HLG tone-map against a dev-only reference on real Android and iPhone HDR files.
- **S3.10** Scaled turbojpeg first paint versus full decode plus fast_image_resize in batch (feeds the perf re-baseline); also whether turbojpeg (arithmetic decoding compiled in) or zune-jpeg decodes arithmetic-coded, CMYK, YCCK and 12-bit JPEG (decides M6.83).

**Assumptions to veto** (for the plan's "Assumptions, veto any" list). The five below are local to this section and written A1 to A5 without a hyphen; the owner-decision assumptions A-1 to A-12 are in 1.7, and A-1 to A-6 and A-8 are pointed to where they apply (3.2.3, 3.3, 3.4.4, 3.9 and 3.11).
- **A1** Formats we cannot write back, and multi-frame or multi-page containers, are never replaced (the same rule as Assumption A-2; see 3.2.3).
- **A2** Multi-page TIFF as separate items is Tier 2 (1.1), possibly pulled into late 1.0.
- **A3** The HEIC engine defaults to bundled libheif on all OSes; OS codecs are opt-in until the parity gate passes.
- **A4** The default pixel cap is 100 MP as elsewhere in the plan, so 108 MP phone sensors (12000×9000) need one click; raising the default to 128 MP (2^27) is the alternative.
- **A5** Strip-location stays off by default per C2, surfaced through an inline chip; this may flip after user feedback.
