# Research: Image format support, HEIC/HEIF conversion, metadata and colour  (key: formats)

## Summary
"All image formats" is unbounded, so define it as tiers. A Rust core on the `image` 0.25 ecosystem (zune-jpeg, png, tiff, image-webp, gif, exr, ravif/dav1d) plus jxl-rs covers nearly everything users meet, memory-safe and permissively licensed. HEIC/HEIF is the exception. As of 2026-09-30 no production-grade, permissively licensed pure-Rust HEIC decoder exists, so it needs libheif + libde265 (LGPL C/C++) or an OS codec. That stack is the security hotspot: libheif published 61 advisories in 2026 (4 critical, 20 high), has one part-time maintainer, and patches only its latest release. The HEVC patent position for a free decoder is unresolved. So decode everything out-of-process in a sandboxed helper with hard limits, hide HEIC behind a swappable backend trait, and keep the HEVC decoder out of the main binary.

## Recommendation
Rust is the right choice for orchestration and safe decoders, but expect C/C++ for HEIC, PDF and RAW. (1) Core: `image` 0.25.x with its decoder hooks, per the Tier 1 list. (2) HEIC/AVIF: a HeicBackend trait. macOS uses ImageIO. Windows uses WIC when the HEVC codec is present. Everything else uses libheif, dynamically linked, decode-only, from our own CMake build, with libde265 as a separate optional plugin. Never use the vcpkg default `hevc` feature (pulls GPL x265). (3) Never encode HEVC; offer AVIF/JXL instead. (4) Run all decoding in a sandboxed helper process with pixel, memory and time caps; upgrade libheif within days of advisories. (5) Transcode metadata explicitly: apply irot/imir then set Orientation=1, fix dimensions, drop stale thumbnails, keep ICC. (6) Use lossless JPEG (turbojpeg) only for 90-degree turns and iMCU-aligned crops; deskew always re-encodes. (7) Test against a real-device corpus.

## Key findings
- `image` 0.25.10 default formats are AVIF (encode only, via ravif), BMP, EXR, farbfeld, GIF, HDR, ICO, JPEG (zune-jpeg), PNG, PNM, QOI, TGA, TIFF and WebP. AVIF decode needs the optional `avif-native` feature (dav1d, C). It has no HEIC, JXL, RAW, PDF, JP2, SVG or PSD. Since 0.25.7 external decoders register via hooks (libheif-rs and jxl-image-rs-integration provide them). Its own notes say many decoders panic on malicious input; default max_alloc is 512 MiB.
- Pure-Rust HEIC: `heic` 0.1.6 (imazen) is AGPL-3.0 or commercial, decodes 118 of 162 test files, and its README says not all code was manually reviewed. ente's heic-decoder is AGPL. hpvcd (BSD-3/Apache) was created June 2026. gamut-heic is a placeholder. None is a safe permissive v1 dependency; keep the backend swappable.
- libheif 1.23.5 (2026-09-21, LGPL-3) and libde265 1.1.3 (LGPL) are the practical engine. libheif had 61 advisories in 2026 (4 critical, 20 high, including an RCE-class one in Aug 2026), libde265 had 13. There is one maintainer, sponsorship is about $41/month, and only the latest release gets fixes.
- Packaging traps: the vcpkg libheif port's default `hevc` feature pulls x265 (GPL-2.0+), and libheif-sys's vcpkg metadata requests x264/x265. libheif-sys `embedded-libheif` builds statically but does not link libde265 statically, force-enables x264/x265 detection, and turns plugin loading off. The Freedesktop SDK builds libheif with libde265 OFF, so Flatpaks must bundle their own HEVC decoder.
- Windows WIC decodes HEIC only with the Store HEIF extension plus an HEVC codec (HEVC Video Extensions is $0.99 retail). WIC exposes depth and gain planes and can rotate/crop HEIF without recompression. macOS ImageIO reads HEIC natively.
- HEIF orientation: libheif applies irot/imir at decode and the EXIF orientation tag is informational, so writing JPEG without resetting Orientation double-rotates. The HEIF Exif blob starts with a 4-byte TIFF-header offset that must be stripped before building a JPEG APP1.
- Lossless JPEG (jpegtran/turbojpeg) covers only 90-degree steps and flips. A crop's top-left snaps to the iMCU grid (8 or 16 px) and partial edge blocks are imperfect, so deskew and perspective correction can never be lossless.
- JXL decode: jxl-rs 0.7.4 (BSD-3, pure Rust, used in Chrome and Firefox) or jxl-oxide (MIT/Apache). The libjxl Rust wrapper jpegxl-rs is GPL-3.0. Colour: moxcms 0.9.1 (BSD-3/Apache, pure Rust, used by `image`), lcms2 6.2.0 (C), qcms stagnant since 2024. The tiff crate reads Fax4 (scanner G4) but cannot write it.

## Risks
- libheif/libde265 vulnerability churn with a single unfunded maintainer; only the latest release is patched, so distro-shipped versions may lag.
- HEVC patent exposure is unresolved. Wikipedia reports a 2016 HEVC Advance policy exempting software distributed directly to consumers, but pools have since consolidated (trade press says Access Advance took over Via LA's HEVC pool in Dec 2025) and Avanci and Sisvel exist. No verified FOSS safe harbour; take legal advice.
- Licence contamination: GPL x265/x264 via vcpkg or embedded builds, GPL-3 jpegxl-rs, AGPL imazen crates, and the LGPL relink obligation when statically linking libheif/libde265.
- Rust decoders can panic or exhaust memory on hostile input (`image` known issue). image-extras is explicitly unfuzzed. Young crates (hpvcd, mozjpeg-rs, libjpeg-turbo-rs) have vendor-only benchmarks.
- HDR: PQ/HLG camera HEIF needs real tone mapping (libheif's 8-bit conversion does not tone-map). Apple's gain-map maths is undocumented, and Apple's APIs drop ISO 21496-1 gain maps on HEIC to JPEG.
- Metadata corruption: orientation, EXIF offset prefix, stale thumbnails and dimensions, and Ultra HDR/MPF offsets that break after any JPEG edit.
- Scanner-style 1-bit G4 TIFF is unverified through the `image` wrapper, and CMYK/16-bit paths are thin. libvips is the fallback.
- A 48 MP HEIC is about 146 MB as RGB8. Batch parallelism times libheif threads can OOM without per-process memory caps.

## Options evaluated

### `image` 0.25.x + native Rust codecs — recommended
Core decode/encode for JPEG, PNG, TIFF, WebP, GIF, BMP, EXR, ICO, TGA, PNM, QOI, HDR and AVIF (encode).
- licence: MIT OR Apache-2.0 (image, png, tiff, image-webp, zune-jpeg)
- status: image 0.25.10 (2026-03-10), zune-jpeg 0.5.15 (2026-09), tiff 0.11.3 with 0.12 in progress, all actively maintained
- pros: Memory-safe, permissive, one API; Hooks let libheif/JXL plug in; 16-bit and f32 buffers, ICC/EXIF/XMP/IPTC extraction
- cons: Decoders can panic on bad input; No HEIC/JXL/RAW/PDF built in; Colour-space info is not carried cleanly through pixels

### libheif + libde265 via libheif-rs 3.0.0 / libheif-sys 5.3.1 — recommended
HEIC/HEIF/AVIF decode (plus JPEG-in-HEIF, JP2, unci). Supports grids, aux/alpha/depth, gain maps, sequences and configurable security limits.
- licence: libheif and libde265 LGPL; wrapper crates MIT; x265 is GPL and must be excluded
- status: libheif 1.23.5 (2026-09-21); libde265 1.1.3 (2026-09-14); libheif-rs 3.0.0 (2026-08-18)
- pros: Only mature HEIC engine; Plugin build lets HEVC ship separately; Same code path on all three OSes
- cons: 61 advisories in 2026; Single maintainer, latest-only fixes; libheif-sys embedded mode is not turnkey (no libde265, no plugins, may pick up x264/x265)

### OS codecs: WIC (windows crate 0.62) and ImageIO (objc2-image-io 0.3.2) — viable
Platform-licensed HEIC/AVIF/JXL/RAW decode.
- licence: MIT/Apache bindings; codecs are OS components
- status: Bindings current; Windows needs Store extensions; macOS built in
- pros: No patent or CVE-patching burden for us; macOS is zero-config; WIC can do lossless HEIF rotate/crop
- cons: Windows needs a paid or OEM HEVC codec; Behaviour differs per OS and version; No Linux equivalent

### Pure-Rust HEIC (`heic`, hpvcd, ente heic-decoder) — avoid
Rust HEIF parser plus intra-only HEVC decoder.
- licence: heic and heic-decoder AGPL-3.0 (heic also commercial); hpvcd BSD-3/Apache
- status: heic 0.1.6 (2026-05); hpvcd 0.3.2 (2026-07); all under 9 months old
- pros: Memory-safe; No LGPL or C build; Could replace libheif later
- cons: Immature (118/162 corpus); AGPL licence problem; Patent question unchanged

### JPEG XL: jxl-rs / jxl-oxide decode, libjxl encode — recommended
Pure-Rust JXL decode; C++ libjxl for encode.
- licence: jxl-rs BSD-3; jxl-oxide MIT/Apache; libjxl BSD-3; jpegxl-rs wrapper GPL-3.0
- status: jxl 0.7.4 (2026-09-17); libjxl 0.12.0 (2026-07-01)
- pros: Decode is safe and fast; Ships in Chrome and Firefox
- cons: No permissively licensed encode wrapper (zune-jpegxl is lossless only); Encode needs custom FFI

### libvips / ImageMagick / FFmpeg as out-of-process fallback engine — fallback
Long-tail format coverage when the helper cannot decode a file.
- licence: libvips LGPL-2.1+; ImageMagick Apache-style; FFmpeg LGPL/GPL by build
- status: libvips 8.18.7 (2026-09-26); ImageMagick 7.1.2-32 (2026-09-27)
- pros: Huge format list; Actively maintained
- cons: Large attack surface; Heavy to bundle; Inconsistent colour handling

### PDF rasterisation: pdfium-render 0.9.4 vs hayro 0.7 — viable
Render PDF pages to bitmaps for cropping.
- licence: pdfium-render MIT/Apache with PDFium BSD/Apache binary; hayro Apache/MIT
- status: pdfium-render 2026-09-06; PDFium binaries current (chromium/8076)
- pros: PDFium is battle-tested and binds at runtime; hayro is pure Rust
- cons: PDFium must be bundled and sandboxed; hayro is experimental and slow

### RAW: rawler 0.8.0 / rawloader 0.37.2 / LibRaw 0.22.2 — fallback
Camera RAW parsing; rawler and rawloader give sensor data and metadata but no demosaic.
- licence: rawler, rawloader LGPL-2.1; LibRaw LGPL-2.1 or CDDL-1.0
- status: All maintained (Aug-Sep 2026)
- pros: Embedded JPEG preview extraction is fast and safe enough; LibRaw gives full development
- cons: Not useful for document scans; Full RAW pipeline is large scope

## Deliverable
## Tiered support matrix (R = read, W = write)

**Tier 1, must-have for v1.0**

| Format | R/W | Library and notes |
|---|---|---|
| JPEG | R/W | zune-jpeg (via `image`); write with jpeg-encoder, mozjpeg opt-in. Lossless 90-degree/iMCU-aligned ops via turbojpeg |
| PNG / APNG | R/W | png (APNG: first frame); oxipng optional |
| TIFF (multi-page, 8/16-bit) | R/W | tiff: write LZW/Deflate/PackBits; read Fax4/JPEG/ZSTD. Verify 1-bit G4 |
| WebP | R/W | image-webp (lossless write only) |
| BMP, GIF, ICO, TGA, PNM, QOI, HDR | R (BMP W) | `image`; GIF first frame plus warning |
| HEIC/HEIF | R, export to JPG | HeicBackend: ImageIO (mac), WIC (Win), libheif+libde265 elsewhere |
| AVIF | R | libheif (dav1d) or `image` avif-native |
| JPEG XL | R | jxl-rs via jxl-image-rs-integration |

**Tier 2, later:** AVIF W (ravif/rav1e); JXL W (libjxl, custom FFI); WebP lossy W (libwebp); PDF pages R (pdfium-render, helper process; hayro to watch); per-frame GIF/APNG/WebP; EXR R; SVG R (resvg, no external refs); RAW embedded preview (rawler); Ultra HDR/gain-map preservation (ultrahdr-rs) or dropped with a notice.

**Tier 3, best effort:** full RAW develop (LibRaw out-of-process); JPEG 2000 (hayro-jpeg2000/openjp2); PSD composite (psd 0.3.5, stale); DDS/ICNS/PCX via image-extras (unfuzzed); HEIC write via ImageIO/WIC only, never x265; long tail via libvips/ImageMagick out-of-process with a restrictive policy.

## Pipeline rules
- Sniff magic bytes (not extensions) and probe header dimensions before allocating.
- Detect on a downscaled proxy; apply the final transform at full resolution, keeping 16-bit for 16-bit sources. JPEG output is 8-bit.
- Colour: iPhone HEIC uses an nclx Display P3 tag with no ICC. Embed a P3 ICC or convert to sRGB with moxcms; lcms2 only for CMYK/LAB oddities.
- Apple HEIC: use the primary image (picker for multi-image files); libheif handles 512-px grids. Gain map, depth and mattes are dropped on JPG export; the app notes this. Live Photo: the still only; the paired MOV is untouched.
- Metadata: copy EXIF/XMP/IPTC/ICC via little_exif or img-parts; set Orientation=1 after transforms, fix pixel dimensions, drop the EXIF thumbnail, and offer "strip GPS".
- Never re-encode JPEG when only rotating 90 degrees or cropping on the iMCU grid.

## Security assessment
- **Attack surface, highest first:** HEIC (libheif, libde265, aom/dav1d); PDFium; LibRaw; libwebp/libjxl (libwebp had CVE-2023-4863); ImageMagick; then the pure-Rust decoders, which are memory-safe but can panic or exhaust memory.
- **Isolation:** decode in a per-image helper process and return pixels over shared memory or pipes. Sandbox per OS: Linux landlock + seccompiler; Windows job object (memory cap, kill-on-close) plus restricted token or AppContainer; macOS sandbox_init/App Sandbox. birdcage was archived July 2026, so do not depend on it. Glycin shows the design, but its sandbox is Linux-only.
- **Decompression bombs:** cap pixels (for example 100 MP), file size, page/frame counts, and wall-clock time. Set `heif_security_limits`, `image::Limits`, and cap PDF DPI. Never disable libheif limits for untrusted files.
- **Supply chain:** pin the native library baseline, run cargo-deny/audit, subscribe to libheif and libde265 advisories, and ship security updates fast. Use `#![forbid(unsafe_code)]` in our crates and cargo-fuzz the glue code.

## Decision-critical claims (as researched)
- libheif published 61 security advisories in Jan-Sep 2026 (4 critical, 20 high), is maintained by one largely unpaid developer (about $41/month sponsorship), and only the latest release receives security fixes. [https://github.com/strukturag/libheif/blob/master/SECURITY.md]
- The vcpkg libheif port lists `hevc` (x265, GPL-2.0-or-later) as a default feature, and libheif-sys's vcpkg metadata requests libheif[hevc,aom,x264,h264-decoder,jpeg], so the stock recipe yields GPL-encumbered builds. [https://github.com/microsoft/vcpkg/blob/master/ports/libheif/vcpkg.json]
- libheif-sys `embedded-libheif` links libheif statically but not libde265/libaom, and its build.rs sets ENABLE_PLUGIN_LOADING OFF and every WITH_*_PLUGIN OFF while turning X265/X264 ON. [https://github.com/Cykooz/libheif-sys/blob/master/build.rs]
- No production-ready permissively licensed pure-Rust HEIC decoder exists: `heic` is AGPL-3.0 or commercial and decodes 118/162 test files; gamut-heic is an unimplemented placeholder; hpvcd was created in June 2026. [https://github.com/imazen/heic]
- `image` default formats do not include HEIC, JPEG XL or RAW; the `avif` feature is encode-only (ravif) and decoding needs `avif-native` (dav1d); many decoders panic on malicious input; default allocation limit is 512 MiB. [https://github.com/image-rs/image/blob/main/CHANGES.md]
- Lossless JPEG crop requires the region's top-left corner to sit on an iMCU boundary (silently moved up/left otherwise), and partial edge iMCUs make some transforms imperfect unless -trim/-perfect are used. [https://github.com/libjpeg-turbo/libjpeg-turbo/blob/main/doc/jpegtran.1]
- jxl-rs is a pure-Rust BSD-3 JPEG XL decoder used in Chrome and Firefox, while the libjxl wrapper crate jpegxl-rs is GPL-3.0-or-later. [https://github.com/libjxl/jxl-rs]
- The WIC HEIF codec depends on the Store HEIF extension and HEVC codec (which may be absent on some PCs), exposes depth/gain pixel formats, and can rotate/crop HEIF without recompression. [https://learn.microsoft.com/en-us/windows/win32/wic/heif-codec]

## Researcher questions for user
- Which licence should Auto Crop itself use? — It decides whether GPL/AGPL dependencies (jpegxl-rs, imazen's AGPL crates, x265) are allowed and how simple LGPL compliance is for libheif and libde265. (default: MIT OR Apache-2.0, with libheif/libde265 dynamically linked so the LGPL relink terms are trivially met.)
- What is your policy on shipping an HEVC decoder (libde265) in official binaries? — HEVC patent licensing for a free decoder is unresolved. The choice trades HEIC-to-JPG convenience for legal and CVE-maintenance exposure. (default: OS codec first, with the HEVC plugin shipped separately from the main binary.)
- Which output formats must v1.0 write? — Each extra encoder adds dependencies, licence questions (libjxl wrapper is GPL) and tuning work. (default: JPEG, PNG, TIFF, WebP lossless in v1.0; AVIF then JXL in v1.x.)
- What should the metadata default be on export and conversion? — Photos and receipts often carry GPS and device IDs, and privacy expectations differ between a converter and a scanner-cleanup tool. (default: Keep everything except orientation and thumbnail, with a visible one-click 'strip location' toggle.)
- How should animated and multi-page inputs be handled in v1.0? — Per-frame and per-page processing changes the UI, undo model and memory use. (default: First frame/page with a warning in v1.0; page picker and batch pages in Tier 2.)
- How should HDR camera HEIF (10-bit PQ/HLG) and Apple gain-map photos be treated? — libheif does not tone-map, and a naive 8-bit conversion looks wrong; proper handling costs effort that scanning use cases rarely need. (default: Tone-map to SDR with a visible notice; keep the gain map only in a later HEIC-to-HEIC path.)

## INDEPENDENT VERIFICATION (skeptic) — overrides the researcher where they differ
- [PARTLY-TRUE] 1. libheif published 61 advisories Jan-Sep 2026 (4 critical, 20 high), one largely unpaid maintainer (~$41/month sponsorship), latest release only gets security fixes.
  CORRECTION: Confirmed: SECURITY.md and README both state 61 advisories, 4 critical, 20 high, a single largely unpaid developer, and latest-release-only fixes. I also counted GitHub's advisory list: 61 published in 2026 (4 Critical, 20 High, 29 Moderate, 8 Low) plus 1 from Dec 2025. NOT supported: the '$41/month' figure. The sponsors page shows 3 current sponsors, 41 PAST sponsors, and '1% towards $5,000/month' (about $50/month). $41 looks like a misread of '41 past sponsors'. Say 'almost no recurring funding' instead.
- [CONFIRMED] 2. vcpkg libheif port lists `hevc` (x265, GPL-2.0-or-later) as a default feature; libheif-sys vcpkg metadata requests libheif[hevc,aom,x264,h264-decoder,jpeg], so the stock recipe yields GPL-encumbered builds.
  CORRECTION: Verified on vcpkg master (port at libheif 1.23.5): default-features is ["hevc"], described as 'HEVC encoding via x265', licence GPL-2.0-or-later; x264 is also GPL-2.0-or-later. libde265 (LGPL) is a core dependency. libheif-sys 5.3.1 Cargo.toml lists exactly libheif[hevc,aom,x264,h264-decoder,jpeg] (vcpkg tag 2026.07.29). Extra: its x86_64-pc-windows-msvc override is `libheif[aom]`, which still inherits the default `hevc` feature, so the Windows build also pulls x265 unless default features are disabled.
- [CONFIRMED] 3. libheif-sys `embedded-libheif` links libheif statically but not libde265/libaom; build.rs sets ENABLE_PLUGIN_LOADING OFF and every WITH_*_PLUGIN OFF while turning X265/X264 ON.
  CORRECTION: build.rs confirmed line by line: ENABLE_PLUGIN_LOADING=OFF, BUILD_SHARED_LIBS=OFF, every WITH_<codec>_PLUGIN=OFF, and every codec ON (X265, X264, LIBDE265, AOM, DAV1D, RAV1E, and so on) except FFMPEG_DECODER. The README warns the static libheif does not statically link libde265, libaom and the like. Important extras: (a) on windows-msvc, build.rs skips the embedded build entirely and always uses vcpkg; (b) the crate embeds libheif 1.23.1 (README says 1.23.0) while 1.23.5 is current, so it is four security releases behind under a latest-only policy; (c) WITH_*=ON only takes effect if the codec is found on the system.
- [PARTLY-TRUE] 4. No production-ready permissively licensed pure-Rust HEIC decoder exists: `heic` is AGPL-3.0 or commercial and decodes 118/162 test files; gamut-heic is an unimplemented placeholder; hpvcd was created in June 2026.
  CORRECTION: Confirmed: heic is AGPL-3.0-only OR commercial, 118/162 corpus (plus 49/49 ITU-T vectors), last publish 0.1.6 on 2026-05-19; hpvcd (BSD-3/Apache) was created 2026-06-07. But: (1) heic's own README labels its pure-Rust backend 'Production', so 'not production-grade' is your judgement. (2) gamut-heic is not a bare placeholder: 0.2.2 (2026-07-21) is a working container parser with a pluggable HEVC hook, but no pixel decode. (3) The report OMITS heic-rs 0.1.1 (MIT OR Apache-2.0, published 2026-09-12, self-declared production-ready, validated against Apple's decoder, no AVIF/overlay/sequences) and heif-oxide (Apache/MIT, GitHub-only, 44/63 Nokia files). All are weeks to months old and unproven, so the conclusion stands but the 'none exist' wording is too strong.
- [CONFIRMED] 5. `image` default formats exclude HEIC, JPEG XL, RAW; `avif` feature is encode-only (ravif), decoding needs `avif-native` (dav1d); many decoders panic on malicious input; default allocation limit 512 MiB.
  CORRECTION: Verified in Cargo.toml (default-formats = avif, bmp, exr, ff, gif, hdr, ico, jpeg, png, pnm, qoi, tga, tiff, webp), the lib.rs format table ('AVIF decoding requires avif-native, uses libdav1d C library'), CHANGES.md known issues ('many decoders will panic'), and limits.rs (512MiB). Nuance for the security design: limits.rs says the alloc limit is non-strict by default and some decoders may ignore it, and max width/height default to no limit. So the helper process must enforce its own caps. image 0.25.10 (2026-03-10) is the latest on crates.io.
- [CONFIRMED] 6. Lossless JPEG crop needs the region's top-left on an iMCU boundary (silently moved up/left otherwise); partial edge iMCUs make some transforms imperfect unless -trim/-perfect are used.
  CORRECTION: Matches the jpegtran man page. Precision: the output covers at least the requested region and may cover more (lower-right corner unchanged); an 'f' modifier on W/H disables the size adjustment, not the corner snap. Imperfection from partial edge iMCUs is documented for flips, rotations and for crops that expand past the edge. By default jpegtran preserves the non-transformable edge blocks; -trim drops them and -perfect makes it fail with an error. For the UI: snap crop origins to 8/16 px based on chroma subsampling and tell the user when a lossless crop grows.
- [PARTLY-TRUE] 7. jxl-rs is a pure-Rust BSD-3 JPEG XL decoder used in Chrome and Firefox, while the libjxl wrapper crate jpegxl-rs is GPL-3.0-or-later.
  CORRECTION: Licences confirmed on crates.io: jxl 0.7.4 (2026-09-17) and jxl-image-rs-integration are BSD-3-Clause; jpegxl-rs 0.15.0+libjxl-0.12.0 and jpegxl-sys are GPL-3.0-or-later. The README does say jxl-rs is used in Chrome/Chromium and Firefox. However caniuse data shows JXL is still disabled by default in release builds: Chrome behind chrome://flags, Firefox behind about:config (default-on only in Nightly), with default-on projected for Chrome 155 and Firefox 158. So 'used in' means integrated but not yet shipping enabled to most users, which weakens the 'battle-tested' implication. Moderate confidence (caniuse can lag).
- [CONFIRMED] 8. The WIC HEIF codec depends on the Store HEIF extension and HEVC codec (may be absent), exposes depth/gain pixel formats, and can rotate/crop HEIF without recompression.
  CORRECTION: All confirmed on the Microsoft Learn page: the extension comes from the Store; HEVC and AV1 codecs 'might not be available on all PCs'; GUID_WICPixelFormat8bppDepth and 8bppGain exist; HEIF rotate/crop is attempted without recompression. Caveats: it is 'attempted', and with WICHeifCompressionNone the Commit fails with WINCODEC_ERR_UNSUPPORTEDOPERATION if recompression would be needed. Gain data is documented only for recent Apple iOS files. The page is flagged as partly prerelease and was last updated 2025-03, so re-test on current Windows 11.

### Other errors spotted by skeptic
- Summary says iPhone HEIC uses 'an nclx Display P3 tag with no ICC'. This is likely wrong for typical iPhone stills: several sources say iPhone photos embed a Display P3 ICC profile, and libheif issue #566 reports an iPhone 12 file with no NCLX profile. I could not confirm this from a primary spec or file dump. Read the ICC ('prof') first, fall back to nclx/CICP (newer HDR files may use nclx).
- Options list says libheif supports 'gain maps'. libheif's ISO 21496-1 tmap/gain-map PR #1503 is still open and unmerged (experimental flag only). Apple-style gain maps are reachable only as generic auxiliary images.
- 'zune-jpeg 0.5.15 (2026-09)' is stale. 0.5.15 was published 2026-03-26; the September release is the pre-release 0.5.16-rc2.
- Licence line 'MIT OR Apache-2.0 (image, png, tiff, image-webp, zune-jpeg)' is inaccurate. tiff is MIT only; zune-jpeg is MIT OR Apache-2.0 OR Zlib; ravif is BSD-3-Clause; exr is BSD-3-Clause; jpeg-encoder is (MIT OR Apache-2.0) AND IJG (needs attribution).
- The tiff crate README table lists Gray only at 8/16/32/64 bits, with no 1-bit entry. The 'verify 1-bit G4' note is warranted; Fax4 decode is supported, but 1-bit bilevel output through `image` must be tested, and TIFF encode is limited to LZW/Deflate/PackBits/none.
- 'Core is memory-safe' overlooks that `avif-native` uses the C dav1d library, and libheif-rs/turbojpeg/pdfium are C FFI. `#![forbid(unsafe_code)]` cannot apply to any crate that wraps them.
- 'Windows needs a paid or OEM HEVC codec' is unverified: the Store page could not be read. Re-check the current price and availability of HEVC Video Extensions.
- birdcage 'archived July 2026' is correct (archived 2026-07-06) and moot: crates.io lists it as GPL-3.0-or-later, so it was unusable for a permissive app anyway.

## Sources
- https://github.com/strukturag/libheif
- https://github.com/strukturag/libheif/blob/master/SECURITY.md
- https://github.com/strukturag/libde265
- https://github.com/microsoft/vcpkg/blob/master/ports/libheif/vcpkg.json
- https://github.com/Cykooz/libheif-sys
- https://github.com/Cykooz/libheif-rs
- https://github.com/imazen/heic
- https://github.com/awxkee/hpvcd
- https://github.com/justin13888/gamut
- https://github.com/image-rs/image
- https://github.com/image-rs/image/blob/main/CHANGES.md
- https://github.com/image-rs/image-extras
- https://github.com/image-rs/image-tiff
- https://github.com/libjxl/jxl-rs
- https://github.com/tirr-c/jxl-oxide
- https://crates.io/crates/jpegxl-rs
- https://github.com/libjpeg-turbo/libjpeg-turbo/blob/main/doc/jpegtran.1
- https://github.com/honzasp/rust-turbojpeg
- https://learn.microsoft.com/en-us/windows/win32/wic/heif-codec
- https://windowslatest.com/2025/07/16/can-you-get-hevc-codec-for-free-on-windows-11
- https://en.wikipedia.org/wiki/High_Efficiency_Video_Coding
- https://en.wikipedia.org/wiki/High_Efficiency_Image_File_Format
- https://gitlab.com/freedesktop-sdk/freedesktop-sdk/-/raw/master/elements/components/libheif.bst
- https://pillow-heif.readthedocs.io/en/latest/workaround-orientation.html
- https://developer.apple.com/forums/thread/791283
- https://github.com/m13253/heif-hdrgainmap-decode
- https://gitlab.gnome.org/GNOME/glycin
- https://github.com/phylum-dev/birdcage
- https://github.com/awxkee/moxcms
- https://github.com/TechnikTobi/little_exif
- https://github.com/ajrcarey/pdfium-render
- https://github.com/LaurenzV/hayro
- https://github.com/dnglab/dnglab
- https://github.com/LibRaw/LibRaw
- https://github.com/image-rs/image-webp
- https://github.com/imazen/mozjpeg-rs