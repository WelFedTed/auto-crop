# Research: Prior art, competitors and differentiation  (key: priorart)

## Summary
No maintained tool combines automatic crop, deskew, perspective correction, B&W document clean-up, a review UI, undo and HEIC conversion in one cross-platform app. The open-source side is fragmented: ScanTailor is a GPL "fork of forks" (the 4lex4 fork has been dormant since Sept 2023, though other forks were active in 2026), unpaper is CLI-only and last released in 2022, and NAPS2 has no auto-crop, perspective or auto-straighten (open issues). Microsoft Lens for phones retired on 2026-03-09, and mobile scanner apps carry ads, watermarks and subscriptions. Photoshop's Crop and Straighten fails on messy backgrounds. HEIC converters are Windows/Mac-only or capped. A Rust document-scanning ecosystem barely exists (top hits are about 1 star), so the niche is open. The catch is that the best algorithms are GPL, so licence choice determines what can be reused. NAPS2 has an open community PR for simple bounds-only auto-crop, so the lasting edge must be quad detection, multi-item split, review UX and speed.

## Recommendation
Position Auto Crop as the fast, free, offline "batch fixer for existing images": detect, crop, deskew, perspective-correct and B&W-enhance receipts, documents and multi-photo scans, with a confidence-ranked review queue, touch-friendly corner handles and non-destructive undo. Build the core on permissively licensed pieces (OpenCV 5 via the opencv crate or pure-Rust imageproc, Leptonica-style deskew, libheif for HEIC). Reimplement ideas from GPL tools (ScanTailor, unpaper) from papers rather than copying code. Choose Apache-2.0 or MIT unless you deliberately want copyleft. Before writing the app, build a public benchmark harness (SmartDoc/MIDV/CORD/UVDoc plus a self-made receipt set) and compare against ImageMagick -deskew, unpaper, ScanTailor and an OpenCV-Python baseline, so "very fast, very accurate" is measurable. Rust is a reasonable choice for the backend, but the image-processing core will likely call C/C++ libraries (OpenCV, libheif) either way.

## Key findings
- Whitespace: no cross-platform GUI does auto-crop + perspective + B&W enhance + preview/undo + HEIC. NAPS2 (the most active open-source scan app) lacks auto-crop, perspective and orientation detection (issues #226, #34, #108 open); its maintainer says auto-crop is not a NAPS2 feature yet, and a community PR #830 (June 2026) proposes bounds-only auto-crop.
- ScanTailor lineage is fragmented: the original was archived in 2020; 4lex4's Advanced fork was last pushed 2023-09-13 (issue #170 'Is it abandoned?' has 20 comments, and a commenter suggests a Rust port); forks still active in 2026 are ScanTailor-Advanced org v1.2.1 (2026-05-24), Universal (commits to 2026-04) and Experimental (2025-12). All are GPL-3.0 C++/Qt Widgets.
- Photoshop 'Crop and Straighten Photos' needs a clean, uniform background with gaps between prints. Forum users report over-cropping, thin white borders and failures on textured backgrounds, stains, closely packed photos and scanner-added edge lines. VueScan Multi Crop (paid) is the usual workaround.
- ImageMagick -deskew is reported to work only for small angles (about 5 degrees or less), to be threshold-sensitive, to fail on colour photo scans, and to be much slower than Leptonica (forum anecdotes, not verified benchmarks).
- Microsoft Lens (iOS/Android) was retired: no new scans after 2026-03-09, and Microsoft points to OneDrive, which cannot save scans locally. CamScanner is widely criticised for ads, watermarks, sync loss and privacy. These are phone tools; desktop batch users are underserved.
- HEIC conversion landscape: iMazing Converter is free for Mac and Windows only (no Linux), and CopyTrans HEIC is Windows-only with the free tier limited to JPEG and 100 images per batch. Windows needs Store codec extensions to view HEIC. libheif (LGPL, MIT tools) decodes HEVC via libde265, and HEVC patent exposure is real.
- Rust ecosystem is thin but healthy at the library level: opencv crate 0.101.0 (2026-09-26), imageproc 0.27.0 (2026-06), libheif-rs 3.0.0 (2026-08). Rust document-crop projects (kropp, autocrop-rs, croppy) are at most a few stars. The name 'autocrop' is dominated by face-cropping and video projects, so use a descriptive tagline and repo topics for discoverability.
- Licensing trap: ScanTailor (GPL-3), unpaper (GPL-2 mixed), NAPS2 (GPL-2+, with LGPL parts), darktable/RawTherapee (GPL-3) and z80z80z80/autocrop (GPL-3) cannot be copied into MIT/Apache code. Reusable: OpenCV (Apache-2.0), Leptonica (BSD-2), imageproc (MIT), jscanify (MIT), OSS-DocumentScanner (MIT), sbrunner/deskew (MIT), UVDoc code (MIT).

## Risks
- Licence contamination: copying GPL ScanTailor, unpaper or NAPS2 code into an MIT/Apache project would force relicensing; use clean-room reimplementation from papers.
- HEVC patent exposure when bundling libde265 for HEIC: it is a distributor risk that needs a legal check (flagged, not verified here); OS codecs or an opt-in plugin are mitigations.
- NAPS2 (or others) could add basic auto-crop soon (PR #830 open); a bounds-only crop is not a lasting differentiator.
- No public benchmark exists for the 'photo of a receipt on a table' case: SmartDoc and MIDV are video-frame or ID-card datasets, so a self-built corpus is required and adds work.
- Classical detectors fail on low-contrast or cluttered backgrounds; accuracy targets may force an optional ML model, which adds size and complexity.
- Search budget was exhausted mid-research, so Reddit-specific complaints and HEIC HDR/gain-map behaviour were not verified; complaint evidence comes from Adobe forums, GitHub issues and review summaries.
- Search discoverability: 'autocrop' is crowded (leblancfg/autocrop 679 stars, face cropping).

## Options evaluated

### OpenCV 5 (via opencv-rust bindings) — recommended
Canny/contours/approxPoly, Hough, warpPerspective, adaptive threshold, CLAHE; the de-facto base of document scanners
- licence: Apache-2.0 (bindings MIT)
- status: OpenCV 5.0.0 (2026-06-06); actively maintained
- pros: Apache-2.0; 5.0.0 released 2026-06-06; opencv crate 0.101.0 updated 2026-09-26; Fastest path to accuracy
- cons: Heavy C++ dependency to bundle on 3 OSes; Bindings and build friction; Classical detectors fail on low-contrast backgrounds

### Leptonica — viable
Bloomberg-style skew detection, background normalisation and dewarp routines (used by Tesseract)
- licence: BSD-2-Clause style
- status: v1.87.0 (2025-12-24); pushes through 2026-09
- pros: BSD-2; Fast and proven; Small footprint
- cons: C API; Less convenient for quad detection; No Rust-native bindings I verified

### imageproc (pure Rust) — viable
Hough lines, contours, projective warps, morphology in pure Rust
- licence: MIT
- status: 0.27.0 (2026-06-02)
- pros: MIT; No native dependency; Easy cross-compile
- cons: Less optimised and complete than OpenCV; Needs custom tuning for documents

### Permissive reference implementations (jscanify, OSS-DocumentScanner, sbrunner/deskew) — viable
OpenCV-based document quad detection, perspective and deskew recipes
- licence: MIT
- status: jscanify 1.8k stars, pushed 2026-07; OSS-DocumentScanner 2.5k stars, pushed 2026-09
- pros: MIT; Recently active (2026); Good test oracles
- cons: Not Rust; Mobile or web oriented; Recipe quality, not benchmarked

### UVDoc-style neural dewarping — fallback
Small neural grid model for bent or curled pages
- licence: MIT code, CC BY 4.0 data
- status: Repo last pushed 2024-07
- pros: Code MIT, dataset CC BY 4.0; Handles curvature classical methods cannot
- cons: Needs an ONNX runtime and model files; Slower, possibly GPU-hungry; Overkill for flat receipts

### ScanTailor family / unpaper / NAPS2 code — avoid
Mature page-processing pipelines
- licence: GPL-3.0 / GPL-2 / GPL-2+ with LGPL parts
- status: Mixed; see summary
- pros: Battle-tested; Rich UX ideas (split, margins, zones)
- cons: GPL-3 / GPL-2 / GPL-2+ block reuse in permissive code; Qt or C# and C stacks; Fragmented forks

## Deliverable
## Competitor comparison (verified 2026-09-30 unless noted)

| Tool | Status today | Strong at | Weak at | Licence / reuse |
|---|---|---|---|---|
| ScanTailor family | Original archived 2020; 4lex4 Advanced last push 2023-09; live forks: ScanTailor-Advanced org v1.2.1 (2026-05), Universal (commits to 2026-04), Experimental (2025-12) | Book/page pipeline, dewarp, Sauvola/Wolf binarisation | Forks of forks; Qt mouse UI; no HEIC; no doc-quad or perspective | GPL-3: study only |
| unpaper | v7.0.0 2022-04; last push 2024-07 | Border, noise and deskew filters | CLI; FFmpeg-coupled builds; admits failures | GPL-2 (mixed): no |
| NAPS2 | v8.3.2 2026-07-22; healthy | Scanner acquisition, PDF/OCR, 3 OSes | No auto-crop or perspective; deskew is a manual menu | GPL-2+, SDK LGPL (C#) |
| OCRmyPDF | v17.13.0 2026-09-28 | PDF OCR; --deskew (Tesseract), --clean (unpaper) | CLI, PDF-centric; docs warn of artifacts | MPL-2.0 |
| ImageMagick | 7.1.2-32 2026-09-27 | Ubiquitous; -deskew/-trim/-fuzz | Small-angle, threshold-sensitive, weak on colour photos, slow | Apache-style; prior art only |
| XnConvert | 1.116.0 | 500+ formats, 80+ batch actions | Closed; free for private/edu only (EUR 15 business) | No |
| darktable / RawTherapee / digiKam 9.0 | Active (digiKam 9.0.0 2026-03) | darktable auto-perspective via LSD lines; digiKam Auto Crop trims uniform borders | RAW/photo tools; no document detection | GPL |
| Photoshop / Lightroom Classic | Current; about US$9.99/mo plan | Multi-photo split; Auto/Upright straighten | Needs clean gaps; over/under-crop; no Linux | Proprietary |
| Mobile scanners (Adobe Scan, Genius, CamScanner, Scanbot SDK) | MS Lens retired 2026-03-09 | Best live edge detection and enhance | Phone-first; ads/watermarks/subscription; Scanbot from about $2.5k/yr | Proprietary |
| ScanSnap Home / VueScan | Active; VueScan on 3 OSes | Multi-crop and deskew tied to scanners | Paid (VueScan about US$100-200, regional); scanner-tied; ScanSnap no Linux | Proprietary |
| iMazing / CopyTrans HEIC | Free | HEIC to JPG/PNG, EXIF kept | iMazing no Linux; CopyTrans Windows-only, JPEG-only free, 100/batch | Proprietary |
| libheif | v1.23.5 2026-09-21 | Reference HEIC/AVIF decode | HEVC patents; no cropping | LGPL lib, MIT tools: linkable |
| GitHub small projects | z80z80z80/autocrop (GPL, 128 stars), sbrunner/deskew (MIT), jscanify (MIT), kropp (Rust, 1 star) | Recipes | Scripts; no preview or undo | MIT ones reusable |

Reuse verdict: only OpenCV, Leptonica, imageproc and the MIT projects can seed a permissive codebase.

## Differentiation statement

Auto Crop is the free, offline batch fixer for the images you already have: it finds the page, receipt or each of several photos on a scan, straightens and de-skews them, cleans up black-and-white text, and shows a confidence-ranked review queue with big touch handles and undo, in a native Rust app on Windows, macOS and Linux.

Credible differentiators:
1. Confidence scoring plus a "needs review" queue instead of silent bad crops.
2. Full pipeline in one non-destructive edit stack: detect, crop, deskew, perspective, B&W enhance, convert, with undo.
3. Touch-first correction UI (corner handles with magnifier, pinch/rotate); I did not verify each rival's touch support, but none advertises it.
4. Published, reproducible speed and accuracy benchmarks.
5. Multi-item split (several receipts or photos per scan), the top Photoshop complaint and a paid VueScan feature.
6. Works on any imported image or folder, including HEIC, with no scanner driver or codec purchase.
7. No ads, watermark, account or subscription, plus real Linux support.
8. HEIC done properly: preserve EXIF, ICC and orientation (HDR gain-map handling is an unverified opportunity).

## Benchmarks and datasets

- SmartDoc 2015 Ch.1: 150 clips, about 24k frames, quadrilateral ground truth, CC-BY-4.0. Metric: Jaccard/IoU.
- SmartDoc Ch.2: 12,100 photos of 50 documents, CC-BY-4.0, for downstream OCR.
- MIDV-500 / MIDV-2019: ID documents in video with quadrangle ground truth; sources public domain or open licences.
- CORD (CC-BY-4.0): photographed receipts, for OCR before/after. SROIE licence unclear.
- UVDoc dataset (CC BY 4.0) and DocUNet benchmark for dewarp (MS-SSIM, line straightness).
- DIBCO/H-DIBCO for binarisation (F-measure, PSNR); DISEC 2013 for skew angle error; SD7K for shadow removal (dataset licence not verified).
- HEIF: nokiatech/heif_conformance files plus libheif test images for format coverage.
- Speed: own harness measuring MP/s, p50/p95 latency and peak RSS against magick -deskew, unpaper, ScanTailor, Leptonica and an OpenCV-Python script.
- Gap: no public "phone photo of a receipt on a table" set, so build and publish one under CC-BY.

## Top recurring complaints to fix

Silent over/under-crop with no easy override; fails on textured or non-white backgrounds; no perspective correction on desktop (NAPS2 #34: users detour through phone apps); deskew limited to small angles; abandoned or hard-to-build tools (unpaper Win64 request open since 2017, ScanTailor forks); ads, watermarks and subscriptions in phone apps; HEIC converters gated by platform or batch caps.

## Decision-critical claims (as researched)
- Microsoft Lens for iOS/Android was retired: no new scans after 2026-03-09, and Microsoft recommends OneDrive (no local scan storage). [https://support.microsoft.com/en-us/lens/]
- 4lex4/scantailor-advanced is GPL-3.0 and was last pushed 2023-09-13; the ScanTailor-Advanced org fork published v1.2.1 on 2026-05-24; Universal's latest release tag is 0.2.14 with commits into 2026-04. [https://github.com/4lex4/scantailor-advanced]
- NAPS2 v8.3.2 (2026-07-22) has no auto-crop: the maintainer says it is not a NAPS2 feature yet (#806, closed as a duplicate of open #226); perspective correction (#34) and auto-straighten (#757) requests are open; community PR #830 for bounds-only auto-crop is open. [https://github.com/cyanfish/naps2/issues/226]
- OpenCV 5.0.0 was released 2026-06-06 under Apache-2.0, and the Rust opencv crate is at 0.101.0 (2026-09-26, MIT). [https://github.com/opencv/opencv/releases]
- libheif is LGPL (MIT sample tools), decodes HEVC through libde265, and HEVC-based HEIC decoding is patent-encumbered, which is why some Linux distributions ship it in separate repositories. [https://github.com/strukturag/libheif]
- iMazing Converter is free for Mac and PC only (batch, EXIF option); CopyTrans HEIC is Windows-only, and its free tier converts to JPEG only, up to 100 images per batch. [https://imazing.com/converter]
- unpaper's latest release is 7.0.0 (2022-04-20), licensed GPL-2 with some MIT/Apache files, and depends on FFmpeg for I/O; ScanTailor and NAPS2 are also GPL, so their code cannot be copied into MIT/Apache projects. [https://github.com/unpaper/unpaper]
- The SmartDoc 2015 Challenge 1 dataset (about 24,000 frames with per-frame quadrilateral ground truth) is licensed CC-BY-4.0, as is the UVDoc dataset. [https://github.com/jchazalon/smartdoc15-ch1-dataset]

## Researcher questions for user
- Which licence should Auto Crop use? — GPL-3 lets us borrow ScanTailor, unpaper and NAPS2 algorithms directly but forces derivatives to be GPL. A permissive licence maximises adoption and matches OpenCV, imageproc and libheif-rs, but requires clean-room reimplementation of GPL ideas. (default: Apache-2.0 (or MIT OR Apache-2.0))
- Which primary use case should v1 optimise for? — Each has different algorithms, datasets and rivals: Photoshop and VueScan for multi-photo, ScanTailor for books, mobile apps for receipts. Focus determines benchmark design and which complaints we solve first. (default: Receipts and documents first, multi-item split in v1.x)
- How should HEIC decoding be shipped? — The promise of one-click HEIC to JPG on Windows without Store codecs needs a bundled decoder, but that carries HEVC patent and distribution risk. (default: OS decoders first, bundled libde265 fallback (pending a legal check on HEVC patents))
- Should v1 include optional ML models for corner detection or dewarping? — ML can raise accuracy on cluttered backgrounds and curled pages but adds model files, runtime dependencies, and speed and size costs against the goal of very fast. (default: Classical CV only for v1; add an optional model if the benchmark shows a gap)

## INDEPENDENT VERIFICATION (skeptic) — overrides the researcher where they differ
- [CONFIRMED] 1. Microsoft Lens for iOS/Android retired: no new scans after 2026-03-09; Microsoft recommends OneDrive (no local scan storage).
  CORRECTION: Microsoft's page matches. It says retirement begins 2026-01-09, the app is pulled from the App Store and Google Play on 2026-02-09, and new scans stop on 2026-03-09. It points users to OneDrive scanning and states OneDrive cannot save scans locally. Existing scans stay viewable while the app remains installed and signed in.
- [PARTLY-TRUE] 2. 4lex4/scantailor-advanced is GPL-3.0, last pushed 2023-09-13; ScanTailor-Advanced org fork v1.2.1 on 2026-05-24; Universal's latest tag is 0.2.14 with commits into 2026-04.
  CORRECTION: Every stated fact checks out via the GitHub API. But 'dormant since Sept 2023' overstates recent activity. The default-branch (master) head of 4lex4 is dated 2020-05-31; the 2023-09-13 pushed_at comes from some other ref. Its newest release tag is the 2019.8.16_EA pre-release. Universal (trufanov-nok/scantailor): tag 0.2.14 was published 2023-08-19; the last commit is 2026-04-07. The org fork's v1.2.1 is dated 2026-05-24; its release commit mentions deskew and oblique regression fixes.
- [PARTLY-TRUE] 3. NAPS2 v8.3.2 (2026-07-22) has no auto-crop: #806 closed as duplicate of open #226; #34 and #757 are open; community PR #830 for bounds-only auto-crop is open.
  CORRECTION: Confirmed: v8.3.2 is dated 2026-07-22. #226 (opened by the maintainer himself) is open. #34 and #757 are open. #806 was closed on 2026-07-26 after the maintainer wrote 'See #226 ... not a NAPS2 feature (yet)'; GitHub's close reason is 'completed', not a formal duplicate. PR #830 (ArneNostitz, 29 files) is open and not a draft. It adds an AutoCropper that finds content bounds, plus an ADF batch-feeder change, so 'bounds-only' is fair. Correction: NAPS2 does have auto-deskew for scanned pages ('Deskew scanned pages' in the profile advanced settings, and CLI --deskew since v7.3.0), plus 'Crop to page size'. The deliverable's 'deskew is a manual menu' is wrong for scans.
- [CONFIRMED] 4. OpenCV 5.0.0 released 2026-06-06 under Apache-2.0; Rust opencv crate at 0.101.0 (2026-09-26, MIT).
  CORRECTION: All confirmed. GitHub shows OpenCV 5.0.0 published 2026-06-06 and the repo licence is Apache-2.0. crates.io shows opencv 0.101.0 published 2026-09-26 under MIT. The opencv-rust README says it supports 4.x and 5.x (MSRV Rust 1.88). Context the research omitted: OpenCV 4.14.0 was released later, on 2026-07-19, so 4.x is still actively maintained. System packages on the three OSes may lag on 5.x, which matters for bundling.
- [CONFIRMED] 5. libheif is LGPL (MIT sample tools), decodes HEVC through libde265, and HEVC HEIC decoding is patent-encumbered, so some Linux distros ship it in separate repos.
  CORRECTION: The libheif README says the library is LGPL and the sample apps are MIT. It says libde265 (LGPL) is the default HEVC decoder; an ffmpeg decoder is an alternative. The README does not discuss patents. RPM Fusion's libheif-freeworld spec (v1.23.5) is a separate add-on package that supplies H.264/HEVC/VVC support, which matches the distro-split claim. Missed risk: the README's September 2026 status note says libheif and libde265 have a single maintainer, 61 security advisories in 2026, and about $41/month sponsorship. This is a security and supply-chain concern for a HEIC-heavy app.
- [CONFIRMED] 6. iMazing Converter is free for Mac and PC only (batch, EXIF option); CopyTrans HEIC is Windows-only, free tier JPEG-only, up to 100 images per batch.
  CORRECTION: iMazing's page: 'free app for Mac and PC', drag-and-drop files or folders, and a preserve/remove EXIF option; it also converts to PNG and HEVC video. It does not list Linux. CopyTrans HEIC lists Windows 7 to 11 only; it advertises up to 100 images in one go and lists PNG/PDF output as a Pro feature marked 'New'; JPEG-only for the free tier holds. The page's marketing text is a little ambiguous about free-tier output formats, so treat that detail as moderate confidence.
- [CONFIRMED] 7. unpaper latest release 7.0.0 (2022-04-20), GPL-2 with some MIT/Apache files, FFmpeg dependency; ScanTailor and NAPS2 are GPL so code cannot be copied into MIT/Apache projects.
  CORRECTION: unpaper-7.0.0 was published 2022-04-20; the last repo push was 2024-07-11. The README says the project is GPL-2.0-only with some individual files under MIT or Apache-2.0, and that ffmpeg is the only hard dependency. NAPS2 is GPL-2.0-or-later; its Sdk, Images, Escl and Internals projects are LGPL-2.1+ (linkable, not copyable into MIT). ScanTailor forks are GPL-3.0. The legal inference is sound: you cannot relicense GPL code as MIT/Apache, though you could copy it if Auto Crop itself were GPL. Apache-2.0 and GPL-2-only are also incompatible.
- [PARTLY-TRUE] 8. SmartDoc 2015 Challenge 1 dataset (~24,000 frames, per-frame quadrilateral GT) is CC-BY-4.0, as is the UVDoc dataset.
  CORRECTION: SmartDoc is confirmed. The README says 150 clips of about 24,000 frames (24,889 in its table), quadrilateral corner ground truth per frame, and CC BY 4.0. The UVDoc half is not supported. The UVDoc code repo and the UVDoc-Dataset repo are both MIT. Neither README, the ETH project page, nor the arXiv abstract states a dataset licence; only the arXiv paper itself is CC BY 4.0, which the researcher likely conflated. The dataset's textures come from third-party sources (DTD, Project Gutenberg, DeepFloyd IF outputs, paper figures), so treat its redistribution and licence terms as unverified.

### Other errors spotted by skeptic
- digiKam: the deliverable says 'digiKam 9.0.0 2026-03', but 9.0.0 was released 2026-03-08 and the latest is 9.1.0 (2026-06-07) per digikam.org news.
- ScanTailor Experimental: listed as '(2025-12)', but ImageProcessing-ElectronicPublications/scantailor-experimental was last pushed 2026-08-08 (148 stars), so it is more active than stated.
- Leptonica option says 'No Rust-native bindings I verified'. Bindings exist on crates.io: leptonica-sys 0.4.9 (2024-11-26) and leptonica-plumbing 1.4.0 (2024-04-12), each with about 500k+ downloads but stale. libheif-rs 3.0.0 (2026-08-18, MIT) and libheif-sys 5.3.1+1.23.1 also exist and are actively maintained; the option table does not mention them.
- libheif row says 'no cropping', which is misleading. The libheif README lists crop, mirror and rotate image transformations (HEIF clap/irot properties), so orientation and crop metadata are handled on decode. It is only not an auto-crop tool.
- The unpaper row says 'last push 2024-07', which is right (2024-07-11). The deliverable's NAPS2 row 'deskew is a manual menu' is stale/wrong: an auto-deskew option exists for scanned pages, as noted under claim 3.
- Scanbot: the pricing FAQ says quotes 'generally start at $2,500' for simple use cases. The retrieved text did not state a per-year period, so '$2.5k/yr' is unverified.
- Adobe Photography plan '~US$9.99/mo': the Adobe page served Korean pricing, so the US price could not be verified. Also unverified: VueScan price, SmartDoc Ch.2 (12,100 photos, CC-BY-4.0) and MIDV licensing.
- OpenCV 5 'recommended' framing should note that 4.14.0 (2026-07-19) postdates 5.0.0, so 4.x is a live branch. Minor: 4lex4's latest release tag is a 2019 pre-release.
- Confirmed as stated: OCRmyPDF v17.13.0 (2026-09-28, MPL-2.0), ImageMagick 7.1.2-32 (2026-09-27), libheif v1.23.5 (2026-09-21), Leptonica 1.87.0 (2025-12-24, pushed 2026-09-02), imageproc 0.27.0 (2026-06-02, MIT), XnConvert 1.116.0 (500+ formats, 80+ actions, EUR 15 business), jscanify (puffinsoft, 1.8k stars, pushed 2026-07-20), OSS-DocumentScanner (2.5k stars, pushed 2026-09-28), z80z80z80/autocrop (GPL-3.0, 128 stars), sbrunner/deskew (MIT), kropp (1 star), CORD (CC-BY-4.0). Note: my web-search budget ran out mid-task, so I verified through the GitHub API, crates.io and direct page fetches instead.

## Sources
- https://support.microsoft.com/en-us/lens/
- https://github.com/4lex4/scantailor-advanced
- https://github.com/ScanTailor-Advanced/scantailor-advanced/releases/tag/v1.2.1
- https://github.com/trufanov-nok/scantailor-universal
- https://github.com/4lex4/scantailor-advanced/issues/170
- https://github.com/unpaper/unpaper
- https://github.com/cyanfish/naps2
- https://github.com/cyanfish/naps2/issues/226
- https://github.com/cyanfish/naps2/issues/806
- https://github.com/cyanfish/naps2/issues/34
- https://github.com/cyanfish/naps2/issues/757
- https://github.com/cyanfish/naps2/pull/830
- https://github.com/ocrmypdf/OCRmyPDF
- https://ocrmypdf.readthedocs.io/en/latest/cookbook.html
- https://github.com/ImageMagick/ImageMagick
- https://www.xnview.com/en/xnconvert/
- https://docs.digikam.org/en/image_editor/transform_tools.html
- https://www.digikam.org/news/2026-03-08-9.0.0_release_announcement/
- https://raw.githubusercontent.com/darktable-org/darktable/master/src/iop/ashift.c
- https://community.adobe.com/questions-712/batch-auto-straighten-crop-scans-1175816
- https://www.hamrick.com/reg.html
- https://scanbot.io/pricing/
- https://www.camscanner.com/
- https://imazing.com/converter
- https://www.copytrans.net/copytransheic/
- https://github.com/strukturag/libheif
- https://github.com/opencv/opencv
- https://crates.io/crates/opencv
- https://github.com/image-rs/imageproc
- https://github.com/DanBloomberg/leptonica
- https://github.com/puffinsoft/jscanify
- https://github.com/ossappscollective/OSS-DocumentScanner
- https://github.com/sbrunner/deskew
- https://github.com/z80z80z80/autocrop
- https://github.com/leblancfg/autocrop
- https://github.com/jchazalon/smartdoc15-ch1-dataset
- https://zenodo.org/records/1230217
- https://zenodo.org/record/2572929
- https://arxiv.org/pdf/1807.05786
- https://github.com/tanguymagne/UVDoc
- https://github.com/clovaai/cord
- https://github.com/nokiatech/heif_conformance
- https://www.neowin.net/news/microsoft-lens-has-been-retired/
- https://petapixel.com/how-much-is-photoshop/