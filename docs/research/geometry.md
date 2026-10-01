# Research: Auto crop, rotate, deskew and distortion correction algorithms  (key: geometry)

## Summary
Use a cheap-first, evidence-fused pipeline. A small learned corner+mask net (256x256 input, roughly 5-15 MB, about 10-25 ms on CPU) and a classical Canny/contour/LSD detector run on a proxy of at most 1024 px. Their quads are fused, refined to sub-pixel accuracy at full resolution by robust edge-line fitting, and only then used to warp the original once. Deskew, 90/180 orientation and multi-item splitting are separate stages, each with its own gate and confidence. Low confidence leaves the image unchanged and flags it for review. Prefer a pure-Rust core (imageproc or kornia-imgproc, fast_image_resize, custom warp) plus ONNX inference over OpenCV bindings, mainly because packaging is simpler. Curved-page dewarping (UVDoc, roughly 0.4-1 s per page) should be opt-in and after v1. The best public pretrained corner model (DocAligner) has no stated weight licence and its authors report weak zero-shot performance. Plan to train our own on permissively licensed data.

## Recommendation
1) Own-trained DocQuadNet-style model (MobileNetV3 or LCNet backbone, FPN, 4 corner heatmaps plus a mask head, 256x256) run through ort. Add a coarse-to-fine ROI second pass and test-time rotations. Train on UVDoc (MIT), SmartDoc (CC BY 4.0), MIDV-500/2019/2020, CORD (CC BY 4.0) and synthetic data. 2) A classical detector as second opinion and fallback. 3) Full-resolution sub-pixel refinement. 4) A Leptonica-style projection-profile deskew, applied only when a confidence ratio passes. 5) A small 4-way orientation CNN. Tesseract OSD is optional and secondary; do not use ocrs for orientation. 6) Pure-Rust geometry with a custom Lanczos warp, benchmarked against OpenCV as a test oracle in dev only. 7) Spike ort against rten in week 1, because ort is still rc.13 and has no Intel-Mac binary. 8) UVDoc dewarp in v1.1, opt-in. 9) Build the evaluation harness (synthetic ground truth plus a scene-disjoint golden set) before tuning anything.

## Key findings
- Quad detection evidence: on SmartDoc 2015 Ch.1 (25K frames, 1080p, 5 backgrounds) the best classical or hybrid entries scored 0.972 Jaccard (LRDE) and 0.955 (SmartEngines). The cluttered Bg5 is the killer: the best score there was 0.861, and SmartEngines fell to 0.688. DocAligner's heatmap nets report 0.9892 (LC100, 1.2M params, 4.9 MB fp32) to 0.9937 (FastViT_SA24, 83 MB), but they were trained on SmartDoc/MIDV/CORD, so these are near in-domain, and the authors say zero-shot is poor. Expect ML to win on clutter and low contrast, and classical to win on clean scans.
- Prior art to copy: MakeACopy (Apache-2.0 Android app) ships a 13.4 MB ORT-format model with a MobileNetV3+FPN backbone, four 64x64 corner heatmaps and a mask head at 256x256 input. Its confidence is heatmap peak times quad/mask agreement. It falls back to OpenCV contour/Hough when the ML result is unconfident. It reruns at fixed rotations because strongly tilted documents are the model's main weakness. It snaps corners to full-resolution gradients. It reports XNNPACK gave no gain and NNAPI was 12x slower on a Pixel 7a, so tiny nets are best run on CPU. The scanic-ml model card lists a 1.8 MB, 456K-param net with median corner error 2.3 px at 224 px (about 1%), so refinement is mandatory.
- Deskew: the Leptonica method (shear-and-sum of differential row sums) is BSD-licensed and simple to port. It sweeps +-7 degrees at 4x reduction, then bisects to 0.01 degrees. Its documented accuracy is about 1/width radians (roughly 0.03 degrees at 2000 px). It works with only a couple of text lines. Defaults are a 0.1 degree minimum correction and a confidence ratio of at least 3. It fails on sparse receipts, tables, graphics and handwriting. For quad-rectified pages, edge geometry already fixes rotation, so use text skew only as a verification signal. For general photos, a length-weighted LSD angle histogram is the standard cue. It is scene-dependent (no man-made lines means no-op), so suggest rather than auto-apply.
- Orientation without OCR: PP-LCNet_x1_0_doc_ori (Apache-2.0 on Hugging Face, Paddle format, needs ONNX conversion) is 7 MB, takes 3.2 ms on CPU and reports 99.06% top-1 on its authors' 1000-image self-built set. The eval set is small, so expect lower on receipts and handwriting. The 0.96 MB textline_ori net is 0/180 only. Tesseract OSD (Apache-2.0) needs osd.traineddata and explicitly skips pages with too few characters, and ignores blobs under 10 px, so it is unreliable on short receipts and is slow. ocrs is documented as an early-preview, Latin-only OCR with no orientation feature. EXIF orientation must be applied first, exactly once.
- Rectification: imageproc 0.27 (MIT) has Projection::from_control_points, but warp interpolation is only Nearest/Bilinear/Bicubic, with no Lanczos. Write a custom f64 DLT and a rayon-parallel Lanczos3 warp. fast_image_resize (Apache-2.0/MIT) does Lanczos3 16 MP to 852 px in about 13 ms with AVX2, but it does not linearise gamma by default. Use linear-light only for large downscales. Recover the aspect ratio with the Zhang-He (2007) method, assuming the principal point is at the image centre, and snap to A4/Letter/ID-1 only within about 3%. Sub-pixel corners come from robust line fits to the four edges, not from cornerSubPix.
- Dewarping: UVDoc (MIT code; MIT weights; MIT dataset of 20K images) has 8M params, takes 488x712 input, and gets CER 0.172 on DocUNet versus DocTr 0.181 and DewarpNet 0.217. PaddleX measures about 870 ms on a Xeon CPU, so it is too slow as a batch default. DocTr and DocGeoNet carry a custom non-commercial, share-alike licence (all rights reserved otherwise) and must be avoided. DewarpNet and Doc3D are MIT but older and slower. Recommendation: v1.1, opt-in per image, with the geometry model designed for either a homography or a dense grid so preview and undo work for both.
- Multi-item and trimming (classical, imageproc-feasible): estimate the bed colour from a border strip, compute Lab distance, threshold, close, then connected components. Fit minAreaRect per component and accept if rectangle fill is at least 0.9. Otherwise split by distance-transform watershed or LSD line cuts, and flag the rest. Scanner-bed shadows are soft gradients, so require crisp edge support for boundaries. Photos touching or overlapping each other are the known hard case, so route them to review. GrabCut and heavy saliency models (U2-Net 176 MB) are not worth it here.
- Confidence and failure detection: combine seven signals into a calibrated score (isotonic on held-out data). The signals are heatmap peak, mask-quad IoU, full-resolution edge support, geometry sanity (convex, angles 55-125 degrees, area 5-99.5%), ML-vs-classical IoU, test-time-augmentation corner spread, and post-warp residual skew. Optimise the cost asymmetry: a wrong crop is worse than no crop, so low confidence means leave unchanged and flag.

## Risks
- Runtime and error-in-degrees figures marked est. are my estimates, not measurements. Web search quota ran out mid-research, so items like DISEC'13 numbers, CORD's ROI field and the Zhang-He derivation rest on prior knowledge and need verification.
- ort is still 2.0.0-rc.13 (2026-07-28) after 2+ years of RCs. ONNX Runtime 1.30.0 ships no macOS x86_64 package, so Intel Macs need a source build or rten/tract fallback.
- No open, permissively licensed pretrained document-corner weights are confirmed. DocAligner weights sit on Google Drive with no stated licence, and it is unclear whether training data (MIDV, CORD, synthetic) would carry through. We must train and label our own, and DTD background licensing needs checking.
- Learned corner nets are weak on strongly tilted, partially out-of-frame and white-on-white documents. Mitigate with rotation augmentation, TTA and the classical cross-check.
- Threshold values (0.5, 0.9, 0.95, 3%, 40%) are starting points. They must be tuned on the golden set, not trusted.
- Pure-Rust means writing LSD, Lanczos warp and refinement ourselves. There is no Rust LSD crate, so budget roughly 2-3 weeks plus SIMD tuning to reach OpenCV speed.
- DIS/IS-Net weights are trained on DIS5K, which has separate terms of use; RMBG-1.4/2.0 are non-commercial. Avoid both for the bundled model.

## Options evaluated

### Own-trained corner-heatmap + mask net via ONNX (DocQuadNet/DocAligner-style) — recommended
MobileNetV3/LCNet + FPN, 256x256 input, four corner heatmaps plus a mask head, run through ort or rten; coarse-to-fine ROI pass and test-time rotation.
- licence: Our own weights (Apache-2.0/MIT) on permissively licensed data: UVDoc MIT, SmartDoc CC BY 4.0, CORD CC BY 4.0, MIDV-500 sources public domain or CC.
- status: Approach proven by MakeACopy (Apache-2.0, ships a 13.4 MB ORT-format model, updated 2026-09-30) and DocAligner v1.1.1 (2026-01-13).
- pros: Best accuracy on clutter and low contrast; 10-25 ms est. on CPU; Small: 2-15 MB; Confidence signals come for free: heatmap peak and mask agreement
- cons: We must train, label and maintain it; Weak zero-shot: DocAligner authors say fine-tuning is needed; Tilted or partial documents need TTA; Needs a training and eval pipeline outside Rust

### Classical quad detector (blur, Canny, contours, approxPoly, LSD) — recommended
Proxy-resolution edge and contour quad fitting with LSD line support; also the fallback and cross-check for the ML path.
- licence: OpenCV Apache-2.0; imageproc MIT; kornia-imgproc Apache-2.0; OpenCV's lsd.cpp carries a BSD-style header.
- status: imageproc 0.27.0 (2026-06-02), kornia-imgproc 0.2.0 (2026-09-26). No Rust LSD crate found.
- pros: No model, fully deterministic; 10-60 ms est.; Excellent on clean scans and high contrast; Independent of the ML failure modes
- cons: Struggles on clutter, low contrast and white-on-white (SmartDoc Bg5 hard even for top entries); Thresholds are brittle; LSD must be ported

### Saliency / segmentation models (U2-Net, IS-Net/DIS, BiRefNet) — fallback
General foreground segmentation, then quad fit on the mask.
- licence: U2-Net and DIS code Apache-2.0 (DIS5K has separate terms; released DIS weights are an academic version); BiRefNet MIT; RMBG-1.4/2.0 tagged 'other' (non-commercial).
- status: U2-Net 176 MB / u2netp 4.7 MB; BiRefNet repo active 2026-09.
- pros: Handles non-rectangular subjects; Usable for multi-item masks
- cons: Heavy: 176 MB, or lite variants of unknown accuracy; Overkill for quads; Licence and dataset terms are murky; Coarse edges still need refinement

### Doc orientation CNN vs Tesseract OSD — recommended
4-way doc orientation classifier (PP-LCNet_x1_0_doc_ori class, converted to ONNX) vs OCR-based OSD.
- licence: PP-LCNet weights Apache-2.0 (HF tag); Tesseract Apache-2.0; ocrs Apache-2.0/MIT.
- status: PaddleOCR repo active 2026-09-16; Tesseract active 2026-09-28; ocrs 0.13.1 (2026-09-13) is an early preview with no orientation feature documented.
- pros: CNN: 7 MB, about 3 ms, no OCR dependency; OSD: script-aware and mature, an optional tie-break
- cons: CNN accuracy claim rests on a 1000-image set and needs our own validation; OSD skips pages with too few characters and is slow; OSD needs libtesseract plus osd.traineddata, and the Rust bindings are stale (leptess last updated 2023)

### UVDoc page dewarp — viable
Neural grid-based unwarping for curved or photographed pages.
- licence: Code MIT, dataset MIT, weights in repo; a Paddle port is Apache-2.0 on Hugging Face.
- status: Repo last pushed 2024-07-28; 8M params, 31 MB ONNX (third-party conversion).
- pros: Best published accuracy/size trade-off (CER 0.172 on DocUNet); Permissive; Small enough to bundle
- cons: About 870 ms CPU on Xeon (PaddleX); 488x712 input limits detail, so the flow map must be applied to the full-resolution image; Failure modes on non-page content; Repo is quiet

### DocTr / DocGeoNet dewarp — avoid
Transformer-based dewarping, 24.8M params for DocGeoNet.
- licence: Custom: non-commercial only, share-alike, otherwise all rights reserved (Copyright Hao Feng 2024).
- status: Repo active but not OSI-open.
- pros: Strong benchmark numbers; DIR300 comes from the same group
- cons: Licence incompatible with a free-and-open-source product that anyone may use commercially; Larger than UVDoc

### opencv-rust bindings (OpenCV 4.14 / 5.0) — fallback
Use OpenCV for Canny, contours, LSD, findHomography, warpPerspective (Lanczos4), cornerSubPix.
- licence: opencv crate MIT; OpenCV Apache-2.0.
- status: opencv 0.101.0 (2026-09-26, Rust >=1.88, OpenCV 4.x/5.x); OpenCV 4.14.0 (2026-07-19) and 5.0.0 (2026-06-06).
- pros: Fastest route to proven, SIMD-optimised algorithms; Great test oracle; Big feature set
- cons: Needs libclang and a system OpenCV at build time; Static linking supported mainly on Linux; Windows and macOS packaging is painful; Distro version fragmentation; The official Windows package is 203 MB (a stripped custom build is far smaller, my est. 5-15 MB); A large surface for a 10-function need

### Pure-Rust stack: imageproc/kornia-imgproc + fast_image_resize + ort (rten fallback) — recommended
Contours, Canny, morphology, connected components, hull/minAreaRect from imageproc or kornia; SIMD resize; custom warp, LSD and refinement; ort for NN with rten as pure-Rust option.
- licence: imageproc MIT; kornia-imgproc Apache-2.0; fast_image_resize Apache-2.0/MIT; ort Apache-2.0; rten and tract MIT/Apache-2.0.
- status: ort 2.0.0-rc.13 (2026-07-28, no stable 2.0); rten 0.26.0 (2026-08-29); tract-onnx 0.23.8 (2026-09-21); ONNX Runtime 1.30.0 (2026-09-10) has no macOS x86_64 package.
- pros: Single cargo build; Small binaries; Full control of SIMD and threading; No C++ toolchain; kornia's Canny claims byte parity with cv2.Canny
- cons: We write LSD, Lanczos warp and refinement; imageproc is mostly scalar apart from rayon; ort is RC-only; Intel-Mac ONNX needs rten/tract or a source build

## Deliverable
**Design rule:** cheap-first, evidence-fused, conservative on doubt. Detect on a <=1024 px proxy, refine at full resolution, resample the original once. Times are for 12 MP on an 8-core laptop; "est." means my estimate, to be verified with criterion benches.

| Stage | Method | Time | Rust route |
|---|---|---|---|
| Decode + proxy | JPEG DCT-scaled decode (1/4), or full decode + Lanczos3 to 1024 px | 10-40 ms est.; resize about 13 ms (16 MP to 852 px, AVX2, sourced) | zune-jpeg/turbojpeg, fast_image_resize |
| Quad, ML | 256^2 heatmap+mask net, ROI 2nd pass | 8-25 ms est. | ort (or rten) |
| Quad, classical | blur, Canny, close, contours, approxPoly/minAreaRect; LSD lines | 10-60 ms est. | imageproc/kornia-imgproc; port LSD |
| Refine | sample 4 edges at full res, sub-pixel gradient peak along normal, Huber line fit, intersect | 3-10 ms est. | custom |
| Rectify | normalised DLT (f64), Zhang-He aspect, Lanczos3 warp | 30-200 ms est. (rayon) | custom |
| Orientation | 4-way CNN, 7 MB | 3-10 ms (3 ms CPU sourced) | ort |
| Deskew | differential-square-sum sweep | 5-20 ms est. | custom |
| Multi-item | Lab bg distance, components, minAreaRect/watershed | 10-40 ms est. | imageproc |
| Dewarp (opt-in) | UVDoc 488x712 | 0.4-1 s (870 ms Xeon, sourced) | ort |

Detection-only is about 30-80 ms after decode; full path with warp and JPEG encode is about 250-450 ms per 12 MP image (est.).

**Pipeline**
1. Apply EXIF once, build the proxy.
2. Candidates. ML is accepted if min corner peak >=0.5 and mask-quad IoU >=0.90. If the quad covers <25% of the frame, rerun on ROI+15%. If confidence is low, rerun at 0/90/180/270. Classical is accepted if convex, area 5-99.5%, interior angles 55-125 degrees, edge support >=0.8.
3. Fuse. If IoU(ML, classical) >=0.95, use the ML corners. Otherwise take the candidate with higher edge support. If both fail, go to step 8.
4. Refine each corner with line fits at full resolution.
5. Rectify. Output size comes from mean opposite-edge lengths. Snap to A4 1.414, Letter 1.294 or ID-1 1.586 only if the Zhang-He aspect is within 3%. Never upscale.
6. Orient with the CNN on 4 rotations. Accept if top-1 >=0.8 and rotation-consistent, else keep and flag. Tesseract OSD is an optional tie-break.
7. Deskew. Pages without a quad: sweep +-7 degrees, coarse 1 degree at 4x reduction, bisect to 0.01 degrees. Apply only if |theta| >=0.1 degrees and the peak ratio is >=3. Photos: LSD angle histogram, auto-apply only if >=40% of long-segment weight is in one mode within +-10 degrees, else suggest.
8. Fallback (no quad). Flatbed path: bed colour from border median, trim (8 px margin), split components >=1% area with rect-fill >=0.9. Else leave unchanged with a red flag.
9. Confidence is calibrated (isotonic) from: heatmap peak, mask-quad IoU, edge support, geometry sanity, ML-classical IoU, TTA corner spread <0.5% of diagonal, post-warp residual skew <0.3 degrees. Bands: >=0.9 auto, 0.6-0.9 amber review, <0.6 red and unchanged.

**Evaluation**
- Public data: SmartDoc 2015 Ch.1 (CC BY 4.0; baseline 0.972, Bg5 hardest), MIDV-500/2019/2020, CORD. Dewarp: DocUNet, DIR300, UVDoc benchmark (test-only; licences unclear).
- Synthetic ground truth: clean page renders plus backgrounds with random homography +-45 degrees, 360 degree rotation, blur, JPEG, shadows. Corners and angles are exact, so it is licence-clean and runs on every commit.
- Own golden set of 500-1000 images, scene-disjoint from training. Stratify: receipts, A4 on desk, books, IDs, whiteboards, flatbed single and multi, HEIC, low light, partial pages, no-document negatives. Label with the app's own manual-adjust UI.
- Metrics: quad IoU (SmartDoc protocol) and corner error as % of diagonal; failure rate at IoU <0.9; success at >=0.95 and >=0.98; skew error p50/p95/p99 (target p95 <=0.25 degrees on text pages); orientation confusion matrix; item precision/recall at IoU 0.9; calibration (ECE, AUROC of confidence vs failure). Headline: % of images needing review at <=1% wrong auto-accepts. Also OCR CER before and after.
- Speed budgets: detection <=100 ms and full pipeline <=500 ms at P95 on a 4-core laptop, benchmarked at 12, 24 and 50 MP. Report peak RSS and model-load cold start.

## Decision-critical claims (as researched)
- The ort crate (ONNX Runtime for Rust) has no stable 2.0 release. The latest is 2.0.0-rc.13, published 2026-07-28, preceded by rc.12 (2026-03-05) and rc.11 (2026-01-07). [https://github.com/pykeio/ort/releases]
- The official ONNX Runtime 1.30.0 release assets contain only osx-arm64 for macOS (no x86_64/universal), plus win-x64/arm64 and linux-x64/aarch64. The ort docs list prebuilt binaries for Windows x64/ARM64, macOS ARM64 and Linux x64/ARM64 only. [https://github.com/microsoft/onnxruntime/releases/tag/v1.30.0]
- DocTr and DocGeoNet are under a custom licence that allows only non-commercial use with share-alike and prohibits commercial use without written permission, so they are unusable in a free-and-open-source project that permits commercial use. [https://github.com/fh2019ustc/DocTr/blob/master/LICENSE.md]
- UVDoc has 8M parameters, 488x712 input, a 20,000-image MIT-licensed dataset, MIT code, and reaches CER 0.172 on DocUNet (DocTr 0.181, DewarpNet 0.217). PaddleX reports about 870 ms CPU inference (Xeon Gold 6271C, high-performance mode) and a 30.3 MB model. [https://arxiv.org/html/2302.02887v2]
- DocAligner's code is Apache-2.0, but its ONNX weights are downloaded from Google Drive file IDs with no stated weight licence. Its heatmap models were trained on SmartDoc, MIDV-500/2019/2020, CORD and synthetic data. The authors state it does not perform well zero-shot and needs fine-tuning. LC100 scores 0.9892 and FastViT_SA24 0.9937 on SmartDoc. [https://docsaid.org/en/docs/docaligner/benchmark]
- imageproc 0.27.0's Interpolation enum has only Nearest, Bilinear and Bicubic. There is no Lanczos in warp, so a custom Lanczos warp is required for the highest-quality output. [https://docs.rs/imageproc/0.27.0/imageproc/geometric_transformations/enum.Interpolation.html]
- SmartDoc 2015 Challenge 1 is licensed CC BY 4.0 (about 1.5 GB test set). Published results give an overall Jaccard of 0.972 for the best entry (LRDE) and 0.955 for SmartEngines, and the cluttered background 5 is the hardest (best 0.861; SmartEngines 0.688). [https://zenodo.org/records/1230217]
- Leptonica's projection-profile deskew (BSD-style licence) is documented as accurate to about the inverse image width in pixels (about 0.03 degrees at 2000 px) with a 0.1 degree minimum-correction default, a +-7 degree default sweep, a confidence ratio of at least 3, and it works with a couple of text lines. [https://raw.githubusercontent.com/DanBloomberg/leptonica/master/src/skew.c]

## Researcher questions for user
- What should 'distortion correction' cover? — Perspective rectification is cheap and fast. Curved-page dewarping (UVDoc) costs about 0.4-1 s per page and a 31 MB model. Lens distortion needs lens profiles. Scope changes the model bundle and the UI. (default: Perspective and skew in v1; opt-in UVDoc dewarp in v1.1)
- When the app is not confident, what should it do? — It sets the auto-accept threshold and the entire review UX. A wrong crop can lose content, so this is a cost-asymmetry choice. (default: Leave unchanged and flag, with a user-configurable option)
- Should 'crop' mean the paper edge or the content? — It changes the detection target and margins. Paper-edge cropping is much more reliable. Content-tight cropping needs a text/ink bounding step. (default: Paper edge plus a small margin, with a content-tight toggle)
- What installer size and offline policy do you want for the ML models? — Bundling 5-15 MB of models is trivial. Adding UVDoc (31 MB) and orientation nets, or downloading on first run, affects offline use and trust. (default: Bundle core corner+orientation models; optional UVDoc download)
- Is multi-item flatbed splitting (several photos or receipts per scan) in v1 scope? — The classical approach works for well-separated items but touching or overlapping photos need review. It adds UI for per-item results and undo. (default: Yes, classical in v1, with review for touching items)
- Must Intel Macs and other platforms without ONNX Runtime binaries be supported at launch? — ONNX Runtime 1.30 has no macOS x86_64 package. Supporting Intel Macs means adding the pure-Rust rten/tract backend or building ORT from source in CI. (default: Decide after the week-1 rten vs ort spike)

## INDEPENDENT VERIFICATION (skeptic) — overrides the researcher where they differ
- [CONFIRMED] 1. ort has no stable 2.0; latest is 2.0.0-rc.13 (2026-07-28), preceded by rc.12 (2026-03-05) and rc.11 (2026-01-07).
  CORRECTION: No correction. The crates.io API lists rc.13 (2026-07-28), rc.12 (2026-03-05) and rc.11 (2026-01-07) as the newest three, none yanked. The GitHub releases API gives the same tags and dates. I found no stable 2.0.0 tag.
- [CONFIRMED] 2. ONNX Runtime 1.30.0 ships only osx-arm64 for macOS; ort's prebuilt binaries cover Windows x64/ARM64, macOS ARM64 and Linux x64/ARM64 only.
  CORRECTION: The v1.30.0 release (published 2026-09-10) has 10 assets: linux-aarch64, linux-x64, two linux-x64-gpu (cuda12/cuda13), osx-arm64, win-arm64, win-arm64x, win-x64 and two win-x64-gpu. There is no osx-x86_64 or universal build. The ort docs pages would not render in my fetcher, so I checked ort-sys/build/download/dist.tsv instead. Its desktop targets are aarch64-apple-darwin, x86_64 and aarch64 Windows MSVC, and x86_64 and aarch64 Linux GNU. It also lists iOS and Android targets, which are irrelevant here. There is no x86_64-apple-darwin. Caveat: ort's load-dynamic feature lets Intel-Mac users supply their own older dylib, so the gap is a packaging problem rather than a hard block.
- [CONFIRMED] 3. DocTr and DocGeoNet use a custom non-commercial, share-alike licence that prohibits commercial use without written permission, so they are unusable in a FOSS project that permits commercial use.
  CORRECTION: I read LICENSE.md in the DocTr and DocGeoNet repos, and it is the same text in DocScanner and DocTr-Plus. It is 'Copyright Hao Feng 2024, All Rights Reserved'. Non-commercial use (research, personal study, non-profit) is allowed with attribution and share-alike. Commercial use is prohibited without prior written permission. GitHub reports the licence as NOASSERTION. The 'algorithm' definition includes code, docs and data. A field-of-use restriction is incompatible with the OSI and FSF definitions, so the conclusion holds. Weights and code should be treated the same way. This is an engineering reading, not legal advice.
- [PARTLY-TRUE] 4. UVDoc: 8M parameters, 488x712 input, a 20,000-image MIT-licensed dataset, MIT code, CER 0.172 on DocUNet (DocTr 0.181, DewarpNet 0.217); PaddleX reports about 870 ms CPU and 30.3 MB.
  CORRECTION: The numbers are confirmed. I checked the arXiv HTML: 8M parameters, a 488x712 input, 20,000 images, and DocUNet CER 0.172 (UVDoc), 0.181 (DocTr), 0.217 (DewarpNet), with DocGeoNet at 24.8M parameters and CER 0.190. The repo is MIT and was last pushed 2024-07-28. PaddleX lists 869.82 ms CPU in high-performance mode on a Xeon Gold 6271C and 30.3 MB, but its own CER for UVDoc is 0.179. The correction is about licensing. 'MIT dataset' only covers the repo and tooling. The document textures come from many third-party sources: ACM TOG, CVF, NeurIPS and PLOS paper pages, Project Gutenberg, and DeepFloyd IF outputs. Backgrounds are DTD, which is 'research purposes' only with no licence stated. UVDoc weights were also trained on Doc3D. So the training data is not a clean MIT dataset.
- [CONFIRMED] 5. DocAligner: Apache-2.0 code; ONNX weights fetched from Google Drive with no stated weight licence; trained on SmartDoc, MIDV-500/2019/2020, CORD and synthetic data; authors say it does not zero-shot well; LC100 0.9892, FastViT_SA24 0.9937 on SmartDoc.
  CORRECTION: All confirmed. The repo is Apache-2.0 (v1.1.1, 2026-01-13). heatmap_reg/infer.py holds three Google Drive file IDs, and ckpt/README points to a Drive folder. Neither the docs nor the repo state a weight licence. The docs' Dataset page lists SmartDoc 2015, MIDV-500/2019, MIDV-2020, CORD v0 and a synthetic set. The Discussion page says the design does not perform well zero-shot and needs fine-tuning. The scores 0.9892 and 0.9937 match the docs. Caveat: SmartDoc appears in both the training and evaluation lists, so I could not verify a clean split. Do not compare these scores with the 2015 competition figures. The 0.9937 model (FastViT_SA24) is 20.8M parameters and 83 MB, while the 4.9 MB LC100 scores 0.9892.
- [PARTLY-TRUE] 6. imageproc 0.27.0's Interpolation enum has only Nearest, Bilinear and Bicubic, so a custom Lanczos warp is required for the highest-quality output.
  CORRECTION: The enum fact is confirmed. docs.rs for 0.27.0 and the source both show only those three variants, and 0.27.0 was published on 2026-06-02. The conclusion is stale, because the plan already lists kornia-imgproc. In the v0.2.0 tag, warp_perspective takes InterpolationMode::Lanczos, implemented as a direct 6x6 Lanczos-3 sampler. Limits: it works on Image<f32, C> with an f32 3x3 matrix, and the u8 fast path (warp_perspective_u8) is bilinear only. A 12 MP RGB f32 buffer is about 144 MB, so plan strip-wise processing or a custom u8 Lanczos. Rewrite the claim as: imageproc lacks Lanczos; kornia-imgproc has an f32 Lanczos warp; a u8 Lanczos warp is still custom. Do a quick spike before committing to writing one.
- [PARTLY-TRUE] 7. SmartDoc 2015 Challenge 1 is CC BY 4.0 (about 1.5 GB test set); best entry LRDE has overall Jaccard 0.972, SmartEngines 0.955; cluttered background 5 is hardest (best 0.861; SmartEngines 0.688).
  CORRECTION: Confirmed: Zenodo record 1230217 is CC BY 4.0, titled 'Challenge 1 (original version)', with testDataset.tar.gz at 1.5 GB and a 21 MB sample set. LRDE reaching 1st place in 2015 is confirmed by the LRDE authors' ICIP paper (bg1-4: 0.987/0.977/0.989/0.984). I could not open the primary competition paper (HAL PDF blocked), so 0.972, 0.955 and 0.861 are unverified. One figure looks misattributed. In Tropin et al. (arXiv 2008.02615) Table III, 0.688 is the Bg5 cell of the 'SmartDoc (Averaged)' row, the mean of all entrants, not SmartEngines. That table lists Smart Engines with overall 0.955 (from its 2017 paper) and no Bg5 value. It also shows later methods well above the 2015 'best': JCD+CSR 0.982 overall and 0.961 on Bg5, and CS-NUST-2 0.978 and 0.948. So the 0.972 baseline and Bg5 0.861 are 2015-era, not state of the art.
- [CONFIRMED] 8. Leptonica projection-profile deskew (BSD-style) is accurate to about 1/width in pixels (about 0.03 degrees at 2000 px), min-correction 0.1 degrees, sweep +-7 degrees, confidence ratio >=3, works with a couple of text lines.
  CORRECTION: Confirmed from skew.c on master. The header is BSD-style. The comment says accuracy is about the inverse image width in pixels in radians (1/2000 rad is about 0.029 degrees). It also says the method works with as little as a couple of text lines. Constants: DefaultSweepRange 7.0, DefaultSweepDelta 1.0, DefaultMinbsDelta 0.01, DefaultSweepReduction 4, MinDeskewAngle 0.1, MinAllowedConfidence 3.0. Note that it operates on a binarised 1-bpp image (default threshold 160), so the plan needs a binarisation step before the sweep. It is designed for text pages, which matches the plan's gating.

### Other errors spotted by skeptic
- The summary says no permissive pretrained corner model exists, but MakeACopy (egdels/makeacopy) publishes 'DocQuadNet-256'. Its README labels the model Apache-2.0. The file is docquadnet256_trained_opset17.ort at 13,404,960 bytes, which matches the 13.4 MB claim. The training README documents the recipe: UVDoc pretrain, SmartDoc fine-tune with sequence-disjoint splits, DTD backgrounds and a UVDoc tail. GitHub reports the licence as NOASSERTION because LICENSE opens with an OpenCV notice, so confirm the weights' licence. It is a bootstrap or baseline candidate. MakeACopy is an Android app, not desktop, and its README warns of SmartDoc background bias.
- The '~13 ms resize (16 MP to 852 px, AVX2, sourced)' figure is from the fast_image_resize benchmark on an AMD Ryzen 9 5950X desktop, single-threaded: Lanczos3 AVX2 at 4928x3279 to 852x567 takes 13.52 ms. The deliverable's 'times are for an 8-core laptop' framing does not match that source, so expect slower on laptops.
- The decode row lists 'JPEG DCT-scaled decode (1/4)' via zune-jpeg/turbojpeg. I found no DCT-scaling in zune-jpeg (README shows only 8x8 IDCT and colour conversion). Scaled decode would need the turbojpeg crate (libjpeg-turbo, a C build), which conflicts with the 'no C toolchain, single cargo build' pitch. Confidence is moderate.
- LSD licensing needs a note. OpenCV's lsd.cpp carries the OpenCV BSD-style header (confirmed). From memory, not verified this session, the original LSD by von Gioi is AGPL. A Rust port must be derived from the BSD-licensed OpenCV version, or use a different line detector such as EDLines.
- The '5-15 MB, 10-25 ms' learned-net sizing is supported only for smaller models. DocAligner's best SmartDoc score (0.9937) is an 83 MB, 20.8M-parameter FastViT_SA24. The 4.9 MB LC100 scores 0.9892 and the 14.7 MB MBV2-140 scores 0.9909. Expect a small accuracy trade-off, and treat SmartDoc scores as in-distribution.
- The plan lists UVDoc (MIT), SmartDoc and CORD as 'licence-clean' training data. UVDoc's data uses DTD backgrounds (research-only wording), scientific-paper textures and DeepFloyd IF output. Doc3D, used to train the UVDoc weights, has textures from CVF pages, Yes! Magazine (CC) and Gutenberg. Do a data-provenance review before claiming permissive weights. CORD is CC BY 4.0 (confirmed).
- Confirmed with no change needed: imageproc 0.27.0 (2026-06-02); kornia-imgproc 0.2.0 (2026-09-26, Apache-2.0, Canny 'byte-for-byte with cv2.Canny' per its source docs); rten 0.26.0; tract-onnx 0.23.8; ocrs 0.13.1; opencv crate 0.101.0 (MSRV 1.88, OpenCV 4.x/5.x); OpenCV 4.14.0 (2026-07-19) and 5.0.0 (2026-06-06); leptess last released 2023-02; ONNX Runtime 1.30.0 (2026-09-10); Tesseract and PaddleOCR push dates; PP-LCNet_x1_0_doc_ori (99.06% on a 1000-image set, 7 MB, 3.24 ms CPU normal mode, Apache-2.0 HF tag); PaddlePaddle/UVDoc on HF tagged Apache-2.0. The OpenCV Windows package of 193 MiB is about 203 MB decimal, so the summary's '203 MB' is consistent.

## Sources
- https://crates.io/api/v1/crates/ort
- https://github.com/pykeio/ort/releases
- https://ort.pyke.io/setup/linking
- https://github.com/microsoft/onnxruntime/releases/tag/v1.30.0
- https://github.com/twistedfall/opencv-rust
- https://github.com/opencv/opencv/releases/tag/4.14.0
- https://docs.rs/imageproc/0.27.0/imageproc/
- https://docs.rs/imageproc/0.27.0/imageproc/geometric_transformations/enum.Interpolation.html
- https://docs.rs/kornia-imgproc/latest/kornia_imgproc/
- https://github.com/cykooz/fast_image_resize
- https://github.com/robertknight/rten
- https://github.com/robertknight/ocrs
- https://github.com/DocsaidLab/DocAligner
- https://raw.githubusercontent.com/DocsaidLab/DocAligner/main/docaligner/heatmap_reg/infer.py
- https://docsaid.org/en/docs/docaligner/benchmark
- https://docsaid.org/en/docs/docaligner/dataset
- https://docsaid.org/en/docs/docaligner/discussion
- https://github.com/egdels/makeacopy
- https://raw.githubusercontent.com/egdels/makeacopy/main/training/README.md
- https://cdn.jsdelivr.net/npm/scanic-ml@0.2.0/MODEL_CARD.md
- https://github.com/tanguymagne/UVDoc
- https://arxiv.org/html/2302.02887v2
- https://github.com/tanguymagne/UVDoc-Dataset
- http://www.paddleocr.ai/latest/en/version3.x/module_usage/text_image_unwarping.html
- http://www.paddleocr.ai/latest/en/version3.x/module_usage/doc_img_orientation_classification.html
- http://www.paddleocr.ai/latest/en/version3.x/module_usage/textline_orientation_classification.html
- https://huggingface.co/PaddlePaddle/PP-LCNet_x1_0_doc_ori
- https://huggingface.co/EasyImageSharp/EasyImageSharp-models
- https://github.com/fh2019ustc/DocTr/blob/master/LICENSE.md
- https://github.com/fh2019ustc/DocGeoNet
- https://github.com/cvlab-stonybrook/DewarpNet
- https://github.com/xuebinqin/DIS
- https://github.com/xuebinqin/U-2-Net
- https://huggingface.co/ZhengPeng7/BiRefNet_lite
- https://zenodo.org/records/1230217
- https://www.lrde.epita.fr/dload/papers/movn.19.icdarw.pdf
- https://arxiv.org/abs/1807.05786
- https://arxiv.org/abs/1910.04009
- https://arxiv.org/pdf/2210.08161
- https://raw.githubusercontent.com/DanBloomberg/leptonica/master/src/skew.c
- https://raw.githubusercontent.com/tesseract-ocr/tesseract/main/src/ccmain/osdetect.cpp
- https://raw.githubusercontent.com/opencv/opencv/4.x/modules/imgproc/src/lsd.cpp
- https://doi.org/10.1016/j.dsp.2006.05.006
- https://ipol.im/pub/art/2012/gjmr-lsd/article.pdf