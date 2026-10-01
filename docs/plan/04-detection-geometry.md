> Auto Crop design, part 4 of 8 | [PLAN.md](../../PLAN.md) | [Decision log](00-decision-log.md) | [ROADMAP.md](../../ROADMAP.md)  
> Planning draft, 2026-10-01. Numbers marked PROVISIONAL are unmeasured estimates; decisions B1-B21 and the assumptions A-1..A-12 live in the decision log and in section 1.

# 4. Detection, geometry and ML

This section specifies how the engine finds the paper (or photos) in an image, straightens it, and decides whether to trust its own answer. Detection is a hybrid (B7): an own-trained ONNX corner net and a classical detector run independently and are fused, and a calibrated confidence score decides whether a result is saved, held or left untouched (B4, B6). Every duration is an unmeasured estimate and every threshold a starting value to tune on the dev split (07 §7.6), never on the locked golden set, which only confirms the result; both are marked PROVISIONAL and are not promises until the harness exists.

## 4.1 Constraints from the decision log

| Decision | Consequence here |
|---|---|
| B1 four input types | One engine, four routes (phone photo, flatbed single, flatbed multi-item, general photo), chosen by cheap scene triage with a per-batch content hint override. |
| B3 overwrite by default | A wrong crop replaces the original (recoverable only from the backup store), so the auto-accept bar comes from the harness. Geometry is parametric; output is one resample of the source. |
| B4, B5, B6 | Good is auto-saved, Check is held and listed first, Failed leaves the file untouched with a "Draw crop" banner. Strict, Balanced and Aggressive are per-batch cutoffs on one calibrated score (4.9). |
| B2, B21 | OSI-only weights (an exception needs an owner-granted ADR, Assumption A-7) and a provenance log (4.11). The golden set stays private; public repos hold synthetic and permissive data plus the small calibration artefact. |
| C1 | AVX2 x86-64 baseline. Apple silicon (macOS arm64) is first-class; ARM64 on Windows and Linux and Intel Macs are best-effort until 1.0 (drives the ort/rten spike, 4.10). |

## 4.2 End-to-end pipeline

Design rule: cheap-first, evidence-fused, conservative on doubt. Cheap stages run first and may end the pipeline; two independent detectors work on a small proxy and must agree, or the item is held; only surviving candidates touch full-resolution pixels, and the source is resampled once.

```
 probe -> decode (scaled / thumbnail-first) -> EXIF/HEIF rotate+mirror applied ONCE
 S0  pyramid: 256 px thumb | 1024 px detection proxy | ~1.5 MP analysis proxy | full-res (lazy)
 S1  scene triage: bed-like? blank? already tight? content hint
       '-- blank / tight / clean-border photo --> early exit (NoChange / TrimOnly)
 +-- concurrent (rayon::join) ----------------------------------------------------+
 | S2a Detector A  256^2 corner-heatmap + mask net (ONNX)                         |
 | S2b Detector B  Canny + contours + LSD line support -> best convex quad        |
 | S2c Bed path    (bed-like only) Lab distance -> components -> N rectangles     |
 +--------------------------------------------------------------------------------+
 S3  fuse: agree -> A corners | disagree -> higher edge support | none -> S2c or Failed
 S4  escalate only if unsure: ROI +15%, rotated re-runs (TTA) 0/90/180/270, 384-512 px
 S5  full-res sub-pixel refinement of kept candidates (edge strips, Huber line fit)
 S6  homography (f64) + Zhang-He aspect recovery -> output size, never upscale
 S7  orientation: 4-way CNN on rectified proxy -> quarter_turns
 S8  gated deskew (binarise, shear-sum) / photo horizon (suggest)
 S9  dewarp, opt-in per image: dense grid composed with the homography
 S10 calibrated confidence + reason codes -> Good | Check | Failed (per item, then file)
 EditState{items[]} -> render: ONE strip-wise Lanczos3 resample (or lossless JPEG transform)
```

| Stage | Dominant work | Est. at 12 MP (PROVISIONAL) |
|---|---|---|
| S0 | Scaled decode or full decode, Lanczos3 to 1024 px | 10-40 ms (resize about 13 ms) |
| S1 | Border-strip statistics | 2-5 ms |
| S2a | 256x256 net inference | 8-25 ms |
| S2b | Canny, contours, LSD | 10-60 ms |
| S2c | Lab distance, components | 10-40 ms |
| S4 | Re-runs of S2a | 20-100 ms, low confidence only |
| S5 | Gradient profiles, line fits | 3-10 ms |
| S6 | Homography solve | under 1 ms (the warp is render cost, 30-200 ms) |
| S7 | 4-way CNN | 3-10 ms per pass |
| S8 | 1-bpp shear-sum sweep | 5-20 ms |
| S9 | UVDoc-class net at 488x712 | 0.4-1 s |

The resize figure is a single-threaded Ryzen 9 5950X result from a [fast_image_resize benchmark](https://github.com/Cykooz/fast_image_resize/blob/main/benchmarks-x86_64.md) stamped with version 6.0.1; expect worse on laptops. Serial detection stages sum to roughly 30-125 ms, so the 40 ms proxy-analysis budget needs S2a and S2b to run concurrently and exit early. The pyramid levels are the 02 §2.5 set; the analysis proxy is about 1.5 MP by area with a long edge of at most 3072 px. The decision-log budgets (D5) stand and are unproven: at most 40 ms proxy analysis, 150 ms to first overlay, 700 ms end-to-end for one 12 MP image, at least 4-5 images per second on 6 cores; 07 §7.1 owns the exact definitions (percentiles, which exits count, CLI versus GUI throughput). Re-baseline after the Tier-M spike. The fast path (S1-S3 plus proxy-level S5) is expected to settle 85-90% of images (an estimate); S4 and the review flag handle the rest.

## 4.3 Quad detection: two detectors and a fuse step

### 4.3.1 Detector A: learned corner and mask net

**Design intent.** A MobileNetV3- or LCNet-class backbone with an FPN neck, aspect-preserving letterbox input at 256x256, and stride-4 heads: four corner heatmaps and a document mask, as in [MakeACopy](https://github.com/egdels/makeacopy)'s DocQuadNet-256 (13.4 MB). Corner slots are ordered in image space (clockwise from the corner nearest the frame's top-left), so the net never needs to know which way up the page is; orientation is stage S7. Decoding is peak plus soft-argmax over a 5x5 window; peak values and mask-quad IoU become confidence signals.

**Refinement is mandatory.** Small nets are accurate to about 1% of the input (the scanic-ml model card reports a 2.3 px median corner error at 224 px), which is tens of pixels at 12 MP. DocAligner's SmartDoc scores (0.9892 for a 4.9 MB model) are near in-domain: SmartDoc is in both its training and evaluation lists, and its authors say [zero-shot use is weak](https://docsaid.org/en/docs/docaligner/benchmark). Its weights are distributed from a Google Drive link with no stated weight licence, so they are at most a comparison baseline and never shipped (4.11).

**Escalation (S4).** If the quad covers under 25% of the frame, re-run on the ROI plus 15%. If confidence is low, re-run at 0, 90, 180 and 270 degrees and take the consensus (strong tilt is this family's known weak spot). The same weights run at 384-512 px input rather than a second model (fully convolutional; train multi-scale). MakeACopy found NNAPI 12x slower and XNNPACK no faster on a Pixel 7a, so the default is plain CPU (4.10).

**Long strips (untested risk).** At stride 4 the two corners on the short end of an 8:1 receipt are a few heatmap cells apart at 256 px and their peaks can merge. Mitigation: rotate the mask's minimum-area rectangle plus 15% to horizontal and re-run at an elongated input such as 512x128 (a spike parameter). First spike: train and test on 8:1 synthetic strips.

**Bootstrap and licence.** DocQuadNet-derived weights ship only under an owner-granted exception (Assumption A-7, 4.11), and only if the licence checks out:
1. Its README labels the model Apache-2.0, but GitHub reports NOASSERTION because the LICENSE file opens with an OpenCV notice. Ask the author for an explicit weights licence.
2. Its recipe is a UVDoc pretrain, SmartDoc fine-tune and DTD backgrounds. DTD is research-only and UVDoc textures are third-party (4.8), so a permissive label does not settle data provenance.
3. The shipped file is an inference-only `.ort` artefact; fine-tuning needs the author's checkpoint or a re-run of the recipe (unverified).

Uses: architecture and recipe reuse on our own cleared data, which is what ships unless the owner grants an exception; checkpoint reuse only if 1 and 2 pass and the owner grants an exception (ADR, provenance status `exception`); a comparison baseline that is never shipped. Training from scratch changes no other stage.

### 4.3.2 Detector B: classical quad

1. Grayscale on the 1024 px proxy, blur (sigma about 1.5 px), Canny with thresholds from the median gradient, closing; repeat on Lab chroma or saturation so a white receipt on a coloured desk is not gray-only.
2. Contours, polygon approximation (epsilon 1-3% of perimeter), keep convex 4-gons, score by area, edge support and squareness. If a corner is occluded (finger, clip), fit the quad to four line groups from the line detector instead.
3. Accept (PROVISIONAL): convex, area 5-99.5% of frame, interior angles 55-125 degrees, edge support at least 0.8.

**Edge support** is the fraction of sample points along a quad edge with a gradient peak within a small perpendicular tolerance, gradient direction aligned to the edge normal and consistent polarity. Record the mean and the minimum over the four edges: one unsupported edge is a failed document.

**Line detector.** No Rust LSD crate exists. Port OpenCV's [`lsd.cpp`](https://raw.githubusercontent.com/opencv/opencv/4.5.4/modules/imgproc/src/lsd.cpp) only from a tag at 4.5.4 or later, never from the moving `4.x` branch. OpenCV's own note on `LineSegmentDetector` ([`imgproc.hpp`](https://raw.githubusercontent.com/opencv/opencv/4.x/modules/imgproc/include/opencv2/imgproc.hpp)) says the implementation was removed from 3.4.6-3.4.15 and 4.1.0-4.5.3 because of an original-code licence conflict, and restored once NFA-computation code published under the MIT licence was available. So never port from 3.4.x, 4.0.x or 4.1-4.5.3 sources, distro packages, Python wrappers or the original LSD implementation, and do not treat the BSD-style header as provenance: on its own it proves nothing. The port records the tag, commit, file SHA-256 and that note in the LSD ADR (M1.28), the port commit and the source header. Fallbacks: EDLines, or a clean-room region-growing detector from the open IPOL paper. Research estimate: 2-3 weeks plus SIMD tuning.

### 4.3.3 Fuse and disagreement handling

| Situation | Action | Interim rule until the calibrator exists (PROVISIONAL) |
|---|---|---|
| A and B accepted, IoU at least 0.95 | Use A's corners, refine (S5) | Eligible for Good |
| Both accepted, IoU below 0.95 | Refine both, keep the higher full-res edge support, store the other as an alternative (one tap in review) | Cap at Check |
| Only one accepted | Use it | Cap at Check |
| Neither | Bed or border trim from S2c if present, else Failed | Failed |

The 0.95 here is the A-versus-B agreement bar, not the silent-failure line (0.90, 4.9). Once the calibrated model (4.9) exists the caps are dropped and the situation becomes features (a fired-indicator per detector plus their IoU). ML should win on clutter and low contrast, classical on clean scans. SmartDoc's cluttered background 5 is the hard slice; later methods reach about 0.96 Jaccard on it against about 0.98 overall ([Tropin et al.](https://arxiv.org/abs/2008.02615)), so the 2015 entrants are not state of the art.

## 4.4 Refinement, homography and the single resample

**Sub-pixel refinement (S5), coarse to fine**, because the ML error is around 1% of the frame. At the proxy: sample 24-64 points per edge (10-90% of its length), take the perpendicular gradient profile within about 2% of the long side, find the peak by parabolic fit, fit a line by Huber IRLS. Then repeat at full resolution within about 8 px of that line. Corners are intersections of adjacent lines in f64. A sample is supported if its gradient peak exceeds about 3x the local noise MAD (PROVISIONAL; matters for white-on-white); if under 60% of an edge's samples are supported, keep the proxy edge and lower edge support. This replaces `cornerSubPix`.

**Partial frames.** If a fitted corner falls outside the image beyond a tolerance, set `partial_frame`, clamp that side to the image border, skip paper snapping and cap at Check (`PARTIAL_FRAME`); the item is offered as a crop to the visible area, since a missing edge cannot be reconstructed reliably.

**Homography and aspect.** Solve the 4-point homography in f64 with Hartley normalisation. Recover the aspect with the [Zhang-He method](https://doi.org/10.1016/j.dsp.2006.05.006), assuming the principal point is at the image centre. That assumption fails on cropped or zoomed inputs, and the estimate is ill-conditioned for near-fronto-parallel quads and narrow strips; then use mean opposite-edge lengths and do not snap. Snap to A4 (1.414), Letter (1.294) or ID-1 (1.586) only within 3% and with all four corners inside the frame. Output size is the mean of opposite edge lengths, never upscaled.

**Single resample.** No intermediate image is resampled. Render is inverse mapping (homography, or homography composed with a dense grid, 4.8), sampled once with a Lanczos3 window on u8 or u16 data, clamped against ringing. Kernel design (4.13): process about 64-row output strips, compute each strip's source bounding box, and where the local downscale exceeds about 1.5 pre-shrink that region with the separable resizer so minification does not alias; memory scales with a strip, not the image. For a pure 90-degree rotation or MCU-aligned crop of a JPEG with no enhancement, the engine instead emits a `lossless_transform` hint (from `axis_aligned` and `residual_perspective`) and the codec layer uses the libjpeg-turbo transform with no re-encode.

## 4.5 Orientation and deskew

### 4.5.1 Orientation

- **EXIF and HEIF first, exactly once.** Decode applies all eight cases (EXIF 1-8; HEIF `irot` and `imir`) and resets Orientation on write, so detection coordinates live in display-upright space. A double-apply is a release blocker, checked on a HEIC and EXIF corpus.
- **4-way CNN** on the rectified proxy (224-256 px input, about 7 MB, 3-10 ms). Reference: PaddleOCR's [PP-LCNet_x1_0_doc_ori](https://huggingface.co/PaddlePaddle/PP-LCNet_x1_0_doc_ori) (Apache-2.0 tag, 7 MB, 3.24 ms on a Xeon Gold 6271C). Its 99.06% top-1 comes from the authors' own 1,000-image ID and document test set, its training data is undisclosed and the ONNX conversion is unverified, so it is a comparison baseline. The shipped net is our own, trained on free labels (rotate any upright page image; the label is the rotation) unless the provenance audit (4.11) clears the Paddle weights.
- **Decision rule (PROVISIONAL).** Run four rotations. Accept if top-1 is at least 0.8 and predictions are rotation-consistent (input turned by k gives the class shifted by k); otherwise keep the orientation and flag `ORIENT_UNSURE` (a Check). For long receipts average logits over three overlapping square crops along the long axis, since a 224 px squash destroys text. Tesseract OSD is not used (it skips pages with few characters and adds libtesseract plus a traineddata file). Photos and blank pages get EXIF orientation only.
- **Flips.** The CNN reports rotation only. Mirrored text (some front-camera captures) is not auto-detected in v1.0; users get a manual Flip. Quarter turns and `mirror` live in the item's `QuadWarp` or `GridWarp` (02 §2.3), so a flip is one undoable edit; `EditState.orientation` is used only for `Identity` geometry or convert-only jobs.

### 4.5.2 Gated small-angle deskew

Order: orientation first, then deskew.

- **Quad-rectified pages.** Edges already fix rotation. Text skew is only a verification signal (post-warp residual under 0.3 degrees feeds confidence) and is applied only on request.
- **No quad (flatbed page).** Rotation beyond the sweep range comes from the component's minimum-area rectangle; the text sweep handles the residual.
- **Method.** Leptonica's projection-profile method ([`skew.c`](https://raw.githubusercontent.com/DanBloomberg/leptonica/master/src/skew.c), BSD-style): sweep plus or minus 7 degrees in 1 degree steps at 4x reduction, bisect to 0.01 degrees, score by the sum of squared differences between adjacent row sums (Leptonica's differential square sum). Documented accuracy is about 1/width radians (about 0.03 degrees at 2000 px) from as little as a couple of text lines. Apply only if the angle is at least 0.1 degrees and the confidence ratio at least 3 (Leptonica's defaults; port the confidence definition exactly).
- **It runs on a binarised 1-bpp image** (default threshold 160). Uneven light breaks a global threshold on photos, so the stage binarises the ~1.5 MP analysis proxy itself (illumination flatten, then Otsu), sharing the flatten kernel with Enhancement but independent of the user's enhancement mode. It fails on sparse receipts, tables, graphics and handwriting, and then returns no-op, never a guess.
- **General photos.** Length-weighted LSD angle histogram. Auto-apply only if at least 40% of long-segment weight sits in one mode within plus or minus 10 degrees; otherwise show "Straighten by X degrees" as a suggestion.
- **Targets:** skew error on text pages median at most 0.1 degrees and p95 at most 0.25 degrees; on photos p95 at most 0.5 degrees. An auto-accepted result left with residual skew over 1.0 degree is a silent failure (4.9), so a post-warp residual above that line raises `RESIDUAL_SKEW` (Check).

## 4.6 Auto-crop, bed edges and crop semantics

**Borders and uniform background (general photos, scanned prints).** Per side, advance inward while rows or columns match the border colour (strip median) within a small Lab distance and have low variance. Never trim more than 40% from a side (PROVISIONAL). Ignore the outermost 2 px at proxy scale (scanner edge lines).

**Scanner-bed edges (flatbed).** Estimate bed colour from a border strip, take Lab distance to it, threshold (Otsu with a floor), close, take connected components. Soft shadows are gradients, so a boundary needs crisp edge support. "Bed-like" (S1) means all four image corners and most of the border agree on one colour.

**Crop semantics (B14, Assumption A-11).** Default: the paper or photo edge, with a content-tight toggle. B14's "small margin" is per route, not one number (Assumption A-11: the default crop margin is per route; the owner may veto, 4.16), because the right margin sign differs by route:

| Route | Boundary | Default margin (PROVISIONAL) | Reason |
|---|---|---|---|
| Quad (phone photo of a document) | Refined quad via homography | 0, plus 1 px inward guard | A desk sliver on a warped page looks like a failure; 1-2 px of blank paper costs nothing. |
| Bed (flatbed documents, multi-item) | Component's minimum-area rectangle | Outward 0.5% of shorter side, at most 12 px at 12 MP, clamped to the frame | Bed edges are soft; photos have no blank margin to sacrifice. |
| Border trim (general photo) | First non-uniform row and column | 0 | The border is what the user wants gone. |

The preset table stores the three values (M2.01) and the transform composer applies them (M2.02); the route's value reaches `EditState` as `MarginPolicy::PaperEdge` (02 §2.3). Tune on content clipping (at most 0.5%) and the rate of visible background; the user-facing control is one "Margin" setting, which starts at the route default.

**Content-tight toggle.** Ink mask on the flattened analysis proxy (adaptive, NICK-like so faint print survives), drop components touching the boundary (shadows) and specks, take the bounding box, pad 2% of the shorter side (PROVISIONAL). If ink covers under 0.5% of the area (thermal fade, blank page), fall back to the paper edge and flag. For photos it means border trim only.

## 4.7 Multi-item splitting (v1.0, B10)

**Data model.** This section defines no type of its own; the model is 02 §2.3's. `EditState.items: Vec<Item>` exists from day one (N = 1 otherwise). Each item carries its own `Geometry` (`Quad(QuadWarp)`, or `Grid(Arc<GridWarp>)` after a dewarp) with its own quarter turns and mirror, plus `include`, `enhance_override`, `origin` and `confidence`; the item id is stable per source and never reused. M10.17 adds `Item.order` and `EditState.split: SplitState` (policy `Auto | Always | Never`, profile `Photos | Receipts`, order mode).

**Algorithm (classical).**
1. Bed model and Lab distance map (4.6); Otsu with floor; close with a kernel of about 1% of the long side; open to drop dust.
2. Connected components on the proxy; discard those under 1% of the image (PROVISIONAL).
3. Minimum-area rectangle per component; accept if fill (component area over rectangle area) is at least 0.9. Otherwise try distance-transform watershed or LSD line cuts and re-test; else flag.
4. Run Detector A on each item's ROI plus 15% and fuse with the component rectangle (4.3.3); refine each item at full resolution.
5. Repeat step 1 at 0.7x the threshold; if the item count changes, flag `SPLIT_UNSTABLE`. Count stability is a confidence signal.

**Touching or overlapping items go to review, not to a solver.** This is the known hard case: Photoshop's Crop and Straighten is reported to fail on closely packed prints, textured backgrounds and scanner edge lines. GrabCut and heavy saliency nets (U2-Net is 176 MB) are not worth it.

**Router.** The ML net returns one quad, so S2c runs whenever S1 says bed-like. N = 1 is compared with A and B as usual; N of 2 or more needs every item to pass its own gate. A per-batch `split: Auto | Always | Never` applies. Splitting replaces one file with N, so `Auto` auto-saves only when gaps are clear (minimum gap about 1.5% of the shorter side, PROVISIONAL; a smaller gap raises `ITEMS_TOO_CLOSE`, a Check); a scan is written only when every included item is accepted, and its outputs commit as one group, never a partial set (02 §2.7, M10.23).

**Originals, naming and restore (Assumption A-2).** Assumption A-2: A 1-to-N split follows B3: outputs take the scan's place and the scan moves to Backups ("Keep the scan" is a setting). Multi-frame sources and formats this build cannot write back are never replaced; results are new files. (owner may veto) "Keep the scan" is off by default; with it on, the outputs are new files and the scan stays. The write path and the replaceability rule belong to 02 §2.7; this section fixes the split semantics. Outputs are named `{name}_{n}`, with `{n}` the 1-based rank in reading order (rows clustered by centroid, then left to right; zero-padded, M10.21), so numbering is deterministic across re-detection, and one grammar, sanitiser and collision key serve every output (M2.27). `ExportRecord.outputs` is a list from day one (M2.33), so a split needs only the additive M10.25 migration; its kind is `OneToN`. "Restore original" puts the scan back and offers Keep or Remove for the derived files: Remove moves them to the backup store (reversible, never a hard delete), and a derived file changed since the save defaults to Keep. The opposite direction, N-to-1 (images combined into one PDF or multi-page TIFF, M11.11), writes a new file and leaves its sources untouched unless "Move sources to Backups" is ticked; its kind is `ManyToOne`.

## 4.8 Curved-page dewarp (v1.0, B10)

**Model class and cost.** UVDoc-class grid dewarping. [UVDoc](https://arxiv.org/abs/2302.02887) has 8M parameters, takes 488x712 input and scores CER 0.172 on DocUNet against 0.181 for DocTr and 0.217 for DewarpNet; PaddleX measures about 870 ms on a Xeon Gold 6271C for a 30.3 MB model. Planning cost: **0.4-1 s per page (PROVISIONAL, unmeasured on Tier-M) and about 31 MB fp32**, too slow for a batch default, hence opt-in per image.

**Licence position.** DocTr and DocGeoNet carry a custom non-commercial licence ([confirmed](https://github.com/fh2019ustc/DocTr/blob/master/LICENSE.md)) and are excluded, code and weights; DocAligner weights are excluded too (no stated weight licence, 4.11). UVDoc's code and weights are MIT, but "MIT dataset" covers only the repo tooling: textures come from third-party paper pages, Project Gutenberg and DeepFloyd IF output, backgrounds are DTD (research-only wording), and the weights also saw Doc3D. So the MIT label alone does not clear UVDoc-derived weights (Assumption A-7, 4.11), and an audit is a hard gate before bundling:
- **Track A:** audit the [UVDoc dataset](https://github.com/tanguymagne/UVDoc-Dataset) and weights lineage and ask the authors for a data statement (M4.81; M12.01 and M12.02 read the outcome). The weights count as cleared only if every training source is OSI-compatible or the authors confirm so in writing. Otherwise UVDoc-derived weights ship only under an owner-granted exception (ADR, provenance status `exception`); the maintainer cannot waive the policy, and any deviation from B2 is the owner's decision.
- **Track B (fallback):** train a small net on synthetic parametric surfaces (cylinder, cone, waves, curl) with exact ground-truth flow, permissive textures and no Blender (GPL). High effort; start only if Track A fails. **Decision point: before the first 0.x preview that advertises dewarp.**

If neither track clears in time, the gate-miss rule applies (4.16, Assumption A-4): the owner decides, and dewarp is never silently disabled or cut.

**Bundle size (assumption, veto-able).** B7's 12-25 MB covers the corner and orientation nets only. Ship dewarp as a separately versioned, fp16-stored file (about 15 MB; needs an accuracy check on ort and rten) inside the standard installers if the total stays under about 40 MB (PROVISIONAL); otherwise as a hash-pinned release asset (the Dewarp pack) that the user downloads in a browser and installs with "Install from file..." (M12.69, M12.71). The app never fetches it (B18), and the loader checks it against its pinned hash like any bundled model (4.10).

**Dense-grid warp.** The net predicts a coarse flow grid (believed 45x31 in the reference code; verify). The flow is composed with the quad homography so the source is still resampled once:
1. Rectify a 5% padded ROI of the quad, upright it using S7, and feed the net (rectified versus raw-crop input and background masking are a spike item).
2. Upsample the grid bilinearly, compose it with the inverse homography into a source-space map at output resolution, and resample with the same strip-wise Lanczos kernel, driven by a per-pixel map instead of a matrix.
3. Output size comes from grid arc lengths, never upscaled.

The item's geometry becomes `Geometry::Grid(Arc<GridWarp>)` (02 §2.3: the outline, columns x rows of source-space nodes and the model id), so undo, redo and re-render work as for a homography, and re-detection never discards an applied dewarp.

**Automatic versus offered.** Never silent in v1.0.
- *Detecting need (cheap):* page edges show curvature (contour deviates from its quad chord by over about 0.5% of edge length, PROVISIONAL) or LSD text-line segments bend consistently. The item gets `CURVED_PAGE_SUSPECTED`, an info chip that never changes the band (4.9), and a "Flatten page" action in review.
- *Modes:* `dewarp: Off | Suggest | Auto`, default `Suggest`. `Auto` is opt-in, labelled experimental (a label on this batch mode only; whether the feature itself ships as experimental is the owner's decision, 4.16), and keeps a result only if it verifies: positive Jacobian everywhere (no fold-over), local scale within 0.5x-2x, page-edge curvature improved, aspect sane. A grid that is near-identity after the homography (mean residual under about 0.3% of the diagonal) is skipped.
- *Cost:* 0.4-1 s per image on demand; batch `Auto` is a background job that never blocks "Save all".

**Evaluation.** DocUNet, DIR300 and the UVDoc benchmark are test-only (licences unclear; never vendored), plus a curved-page slice in the private golden set. Metrics: Tesseract 5 CER before and after (evaluation-only), our own line-straightness residual, and above all the do-no-harm rate (4.15).

## 4.9 Confidence calibration and triage

**Signals.** (1) minimum corner peak, (2) mask-quad IoU, (3) full-resolution edge support (mean and minimum), (4) geometry sanity, (5) A-versus-B IoU, (6) TTA corner spread (under 0.5% of the diagonal is good), (7) post-warp residual skew (under 0.3 degrees), plus orientation margin and split stability. Signal 4 (convex, angles 55-125 degrees, area 5-99.5%) is a hard gate: an implausible quad is Failed regardless of score. A small monotone model (logistic regression) maps features to a raw score and isotonic regression on the real calibration split (below) calibrates it to a probability of success. Curvature suspicion is not a score signal: it is an info chip (registry below).

**Reason-code registry (`HoldReason`).** Each Check carries reason codes shown as chips. This table is the one registry: the core emits exactly these codes, and the UI, CLI, JSON and docs key on them. Other files reference it rather than restate it (02 §2.3 `HoldReason`, 06 §6.7 copy, M4.43, M10.15, M12.34).

| Code | Fires when (PROVISIONAL) | Effect | English chip (06 §6.7 owns the text) |
|---|---|---|---|
| `DETECTORS_DISAGREE` | both detectors accepted, IoU below 0.95 (4.3.3) | Check | "Two possible outlines" (offers "Try outline 2 of 3") |
| `WEAK_EDGE` | an edge is weakly supported at full resolution (4.4) | Check | "Edge unclear on the right side" (the side is a parameter) |
| `PARTIAL_FRAME` | a fitted corner falls outside the frame (4.4) | Check | "Content may be cut off" |
| `ORIENT_UNSURE` | top-1 below 0.8 or not rotation-consistent (4.5.1) | Check | "Check orientation" (with 90 degree buttons) |
| `RESIDUAL_SKEW` | post-warp text skew over 1.0 degree at a confidence ratio of at least 3 (4.5.2) | Check | "Text still looks tilted" |
| `NO_DOCUMENT` | low document likelihood and no quad, hint `Auto` (below) | Check | "No document found. The image is left as it is." |
| `NO_QUAD` | no accepted quad and no bed or border route (4.3.3) | Failed | "Couldn't find the edges. The original is untouched." |
| `ML_UNAVAILABLE` | the ML model failed to load or verify; classical-only result (4.10, M4.22) | Check | "Detection is running without the ML model; results are held for review" |
| `BATCH_OUTLIER` | crop-area ratio, aspect or residual skew is an outlier against the batch median; arms at 20 or more items; off in Streaming commit mode (Assumption A-3, M5.11) | Check | "Differs from the rest of the batch" |
| `TOUCHING_ITEMS`, `OVERLAPPING_ITEMS`, `ITEMS_TOO_CLOSE`, `SPLIT_UNSTABLE`, `LOW_CONTRAST_EDGE`, `ODD_ASPECT`, `TOO_MANY_ITEMS`, `BED_UNCERTAIN`, `ANALYSIS_LIMIT` | multi-item rules (4.7, M10.15): an unsupported cut, an overlapping cluster, a clear gap under 1.5% of the shorter side, a count that changes at 0.7x threshold, a low-contrast edge, an implausible aspect for the profile, more than 32 items, a forced split on a non-bed image, the analysis time cap (M10.67) | Check, whatever the score | M10.41 |
| `READABILITY_RISK{faded_print, ink_coverage, lost_marks, unreliable_estimate}` | the four readability signals (05 §5.11, M7.24) | Check | 05 §5.11 |
| `CURVED_PAGE_SUSPECTED` | page edges bow or text lines bend (4.8) | Info chip; never changes the band | the chip plus the "Flatten page" action (M12) |
| `DEWARP_UNCERTAIN`, `DEWARP_REJECTED{why}`, `DEWARP_NEAR_FLAT`, `DEWARP_NOT_APPLICABLE{why}`, `DEWARP_MODEL_MISSING` | dewarp outcomes (4.8, M12.34) | Info chips; an applied dewarp is capped at Check (M12.26) | M12.34 |

A Check-effect code caps the item at Check whatever its score, and a file's band is the worst band among its items. Implausible geometry is not a hold reason: it is the signal-4 hard gate and gives Failed. Notices (dropped HEIC traits, GIF first frame only, format fallbacks; 02 §2.10) are Info or Warn chips and never hold an item. UI and CLI text is looked up by code, one message per code, and is never English built in core (B20). A new code needs a row here, a message, a fixture that triggers it and a table-driven test (M4.43).

**Silent failure, defined (07 §7.4 owns the measurement).** An auto-accepted result with any of: quad IoU with the hand label below **0.90** after canonical warp (both quads projected into a reference frame, the SmartDoc Jaccard protocol); ink or text clipped by more than 2 px; not upright, or residual skew over 1.0 degree; a no-document image cropped; or a wrong item count or an item IoU below 0.90. 0.90 is the project's own line: SmartDoc's widely quoted 0.945 success threshold could not be sourced and is not used. The 0.95 and 0.98 IoU levels are reported success levels only, and 0.95 is also the A-versus-B agreement bar (4.3.3); neither defines a silent failure (PROVISIONAL).

**Bands (B4, B6).** Good is auto-saved once the whole batch is triaged (Assumption A-3); Check is held and listed first; Failed leaves the original untouched. The UI shows icon plus word, never a raw score.

| Band | Calibrated score s | UI | Action |
|---|---|---|---|
| Good | s at least t(mode) | Check-circle icon and the word "Good" (green) | Auto-saved once the batch is triaged |
| Check | 0.60 up to t(mode) | Triangle icon and the word "Check" (amber) | Held, listed first in the review grid, original untouched |
| Failed | s under 0.60, or any hard gate | X-circle icon and the word "Failed" (red) | Original untouched, "Draw crop" banner |

The Failed floor is 0.60 in every mode (PROVISIONAL).

**Operating points.** The three modes are cutoffs t(mode) on the one calibrated score, each with a target for silent failures among auto-accepted results. The flagged share is an outcome to report, not a control, and the shares below are guesses.

| Mode | Silent failures (target) | Flagged (expected) |
|---|---|---|
| Strict | at most 0.3% | about 15-20% |
| Balanced | at most 1% (B6) | about 8-10% (B6) |
| Aggressive | at most 3% | about 3-5% |

B6 fixes only Balanced; the Strict and Aggressive targets are PROVISIONAL proposals. Until `calibration.json` exists (M2.18, M2.43) the interim cutoffs are t = 0.95 (Strict), 0.90 (Balanced) and 0.80 (Aggressive) on the uncalibrated v0 score.

**Why 0.9 is not the Balanced cutoff.** 0.9 is a research start value. A calibrated 0.9 means about one in ten such results fails, so a 0.9 cutoff does not by itself deliver at most 1% silent failures; that holds only if most accepted results sit well above it. Once calibrated, the cutoffs are derived from the risk-coverage curve on held-out data, and **Balanced** is the lowest t with estimated silent failures among accepted results at most 1%. Which mode is the unlabelled default is Assumption A-3 (01 §1.7, 07 §7.4): Balanced only if the locked golden set shows a point estimate at most 1.0% and a bound at most 2.0% (G2), otherwise Strict is the default and Balanced is labelled experimental. Only silent-failure evidence demotes it; a flagged share above 10% (15% at v0.3.0) is a stage-gate note, not a demotion.

**Calibration data.** The calibrator and the cutoffs are fitted on the unlocked real dev tier plus SmartDoc, MIDV and CORD (07 §7.6, M4.42), never on synthetic data and never on the locked golden set. The locked set only confirms the cutoffs once per release candidate, with an append-only log (M4.47). The shipped artefact is `calibration.json` (isotonic knots, the three cutoffs, versions); only curves and cutoffs are published (B21).

**Statistical power (arithmetic, not from the research).** Showing at most 1% at 95% one-sided confidence needs zero failures in about 300 accepted images (rule of three). With 450 accepted images the upper bound is 0.66% at zero failures and about 1.05% at one, so a 500-image golden set cannot prove "at most 1%" unless nearly clean. The gate in 07 §7.4 is therefore a point estimate of at most 1.0% plus a one-sided 95% Clopper-Pearson bound of at most 2.0%, with at most one silent failure in any slice of 80 or more images; smaller slices are advisory, and slices under 30 images are suppressed from every public artefact. Report the interval and keep growing the set (v2 is at least 800 locked images, 07 §7.5). The 5k synthetic set supports regression tests, not calibration and not the claim.

**No-document images and splits.** A photo with no document is normal, not a failure. Outcomes are `Crop | TrimOnly | NoChange | Split(n) | Failed`. With a `Photo` hint, or when S1's document likelihood (peak mask activation plus best edge support) is low and borders are clean, the result is `NoChange` at Good; under `Auto`, a low-likelihood image with no quad becomes Check with `NO_DOCUMENT`. A file's confidence is the minimum over its items and split stability. The per-batch hint `Auto | Document | Photo` goes beyond the decision log; the user may veto it.

## 4.10 ML runtime

- **Default.** `ort` 2.0.0-rc.13 (published 2026-07-28, no stable 2.0; [releases](https://github.com/pykeio/ort/releases)), wrapping ONNX Runtime **1.28** (the 1.30 badge on ort's master README is unreleased). Pin exactly. CPU execution provider; 1-2 intra-op threads per session in batch mode, 2-3 interactively. Check the `Session::run` receiver (`&self` or `&mut self`) in rc.13 before designing session sharing. All ML calls go through a `trait InferenceBackend`, so ort, rten and tract are swappable; canonical artefacts are `.onnx`, with `.ort` or `.rten` derived by `xtask`.
- **Week-1 spike: ort vs rten vs tract** (rten 0.26.0, tract-onnx 0.23.8). Pass criteria (PROVISIONAL): operator coverage for both nets (hard-swish, squeeze-excite, FPN resizes; fp32 and int8); median 256x256 latency at most 25 ms on Tier-M with 4 threads; corners within 0.1 px of ORT's; binary-size delta; CI friction on three OSes (ort's default build downloads prebuilt binaries).
- **Intel Macs.** ONNX Runtime 1.30.0's [release assets](https://github.com/microsoft/onnxruntime/releases/tag/v1.30.0) have no macOS x86_64 or universal build; ort's prebuilt list is Windows x64/ARM64, macOS ARM64, Linux x64/ARM64. Options: (1) an rten backend for Intel Macs (and everywhere if it matches ORT within tolerance); (2) ort `load-dynamic` with a self-built dylib (a CI source build is heavy, unmeasured); (3) a documented classical-only degraded mode with more items in review. C1 keeps Intel Macs best-effort until 1.0, so the spike sets the 1.0 promise.
- **Models and int8.** Corner net about 13 MB plus orientation net about 7 MB, about 20 MB fp32, inside B7's range. Ship fp32 by default; quantise (static QDQ, about 500 calibration images from the training split, never the golden set) only if the spike shows a real speed or size win, and check every platform: non-VNNI x86 U8S8 can saturate and sub-pixel heatmap heads are sensitive, so keep heads at higher precision if needed. Gate (PROVISIONAL): int8 ships only if, against fp32 on that platform, the p95 corner error rises by at most 0.05% of the diagonal and the mean IoU changes by at most 0.3 pt; the corner gate is a fraction of the diagonal, not a pixel shift.
- **GPU and NPU providers (CoreML, DirectML, CUDA, WebGPU).** Opt-in only, after per-model benchmarks: DirectML is in sustained engineering, ORT calls WebGPU experimental, and small nets rarely gain. Only dewarp might benefit, and any provider must reproduce the CPU grid within tolerance.
- **Trust and startup.** The 1.0 loader accepts only models pinned in `models.lock` and checks their hash before building a session; none is user-supplied in v1.0 (user-supplied models are B.13, after 1.0). Bundled models are fetched at build time by `xtask fetch-models`, never at run time, and the app has no in-app model download (B18): the Dewarp pack, if it ships as an asset, is a hash-pinned file installed from disk (4.8). Sessions are created lazily on a background thread.

## 4.11 Model training pipeline and provenance

**Separate repository.** `WelFedTed/auto-crop-models` holds PyTorch training, ONNX export, evaluation and data-generation scripts, and model cards, keeping datasets, GPU dependencies and licence-sensitive data out of the app repo. The app pins model versions by SHA-256 in `models.lock`; binaries stay out of the app repo (no Git LFS: its free bandwidth quota is a CI risk, unverified).

**Who trains (open).** The maintainer, on their own GPU or a free-tier notebook (B15 rules out paid compute; free quotas are unverified). The nets are small (roughly 1-5M parameters), so one consumer GPU should train them in hours (estimate). Recipe: Gaussian-target heatmap loss plus BCE and Dice on the mask; augmentation with 360-degree rotation, perspective to 45 degrees, partial frames, occlusion, blur, JPEG 40-95, shadows, white-on-white backgrounds, 4:1 to 8:1 strips and multi-scale input.

**Synthetic data, independent of the app.** Corners and angles are exact by construction, so it is licence-clean and cheap. Generate in Python with OpenCV (Apache-2.0) `warpPerspective` and `remap` plus Augraphy (MIT) for paper and print degradation, never with the app's Rust warp, so generator and engine cannot share failure modes. Augraphy's last PyPI release is from 2023-12-31, a maintenance risk; Albumentations (archived) and AlbumentationsX (AGPL-3.0) are not used. Text and receipts come from OFL fonts and public-domain or generated text; backgrounds from procedural textures, CC0 texture libraries (check each asset at ingest) and the maintainer's own CC0 photos. Every image records its seed and parameters; about 5k feed the CI accuracy suite.

**Public data.** SmartDoc 2015 Ch.1 ([CC BY 4.0](https://zenodo.org/records/1230217); cite the paper), MIDV-500 (public-domain or open sources) and CORD (CC BY 4.0), fetched by script with checksums, never vendored, with our own sequence-disjoint splits. Avoid the aggregated DocCornerDataset (research-only for parts) and DIS or RMBG-family weights.

**OSI-only weights policy (Assumption A-7).** Released weights are MIT OR Apache-2.0, and any third-party weight in the lineage needs an OSI licence. Research-only, non-commercial or unknown-terms data is prohibited: DocTr, DocGeoNet and DocEnTr, DocAligner weights (no stated weight licence), DIS and RMBG-family weights, the aggregated DocCornerDataset and AlbumentationsX are excluded (M4.04 holds the ban list). CC BY needs attribution in the model card; share-alike data is avoided; the private golden set is never trained on. Exceptions belong to the owner alone. Assumption A-7: Only OSI-licensed lineage ships. ImageNet-initialised, DocQuadNet-derived or UVDoc weights need an owner-granted exception (ADR, status `exception`); otherwise from-scratch or Track B weights ship. (owner may veto) A gap the research did not cover is ImageNet itself: pretrained backbones (MobileNetV3, LCNet) inherit ImageNet's non-commercial-research access terms, and the effect on derived weights is unsettled. Log every initialisation source and run one from-scratch control (M4.05) so the owner can see what declining the exception costs; the from-scratch weights ship unless the owner grants it.

**Provenance log.** The source of truth is a JSON-lines log in the models repo, one record per asset (id, source URL, licence SPDX, retrieval date, SHA-256, split, allowed use, status); the CSV and Markdown views are generated from it and never edited. Status is `cleared`, `pending`, `blocked`, `banned` or `exception` (granted by the owner only, with its ADR linked; A-7). A `MODEL_CARD.md` per release records the data-manifest hash, code commit, metrics, licence and known failures. `xtask check-models` and the models-repo CI reject an unlogged asset and any bundled weight whose lineage includes an asset that is not `cleared`; an `exception` row passes only with its ADR.

## 4.12 Hard cases

Each row gets its own golden-set slice (`aspect>4:1`, `faded`, `partial`, `low_contrast`, `clutter`, `curved`), gated on the worst slice.

| Case | Failure mode | Mitigation |
|---|---|---|
| Long narrow receipts (over 4:1) | Corner peaks merge at stride 4; aspect recovery ill-conditioned; orientation squash | Elongated ROI pass (4.3.1); edge-length aspect, no snap; multi-crop orientation; early 8:1 spike |
| Thermal fade | Faint text defeats binarisation, so deskew and orientation are unreliable; content-tight would clip | NICK-like ink mask; deskew no-ops below ratio 3; orientation flags; content-tight falls back to paper edge |
| Partial frames, hand over a corner | Missing corner; wrong aspect | Huber fits tolerate partial edge occlusion; `partial_frame`; Check; no snap |
| White on white | Almost no gradient at the edge | Local-contrast boost before edges; chroma channels; low-contrast training samples; support relative to noise MAD; low support means Check |
| Clutter | Competing rectangles; B fooled | A leads; nested strong candidates go to review with the alternative |
| Lens distortion | Bowed edges | Out of scope for 1.0 (below) |

**Lens distortion is out of scope.** Assumption A-11: radial lens correction is not in 1.0 (perspective and curved-page dewarp deliver "distortion"); the owner may veto. The interview selected perspective, skew and curved-page dewarp, not lens correction. Correcting a lens needs a per-lens model (database or calibration step) that the app cannot know from a shared file, and it is a different problem from paper curl. Strong lens bow lowers edge support because the boundary stops fitting a straight line, so such images land in Check, not in silent failure. A radial-distortion suggestion is a 1.x candidate.

## 4.13 Rust implementation notes

Kernels to write, in priority order:
1. **Strip-wise u8 and u16 Lanczos3 warp** with a homography or dense-map coordinate provider. imageproc 0.27 warps only with Nearest, Bilinear or Bicubic. kornia-imgproc 0.2.0 has a [direct 6x6 Lanczos-3 sampler](https://docs.rs/kornia-imgproc/latest/kornia_imgproc/) but only for `f32` images (its `u8` path is bilinear, and a 12 MP RGB f32 buffer is about 144 MB). **Spike first:** drive kornia's kernel per strip with f32 conversion and adopt it if it meets speed and memory budgets against the oracle; otherwise write ours with fixed-point weights from a lookup table (`wide` or `pulp` for SIMD; `std::simd` is not stable).
2. **LSD port** (4.3.2).
3. **u64 strip integrals** for Sauvola and box statistics. A u32 plain-sum integral of u8 data overflows above about 16.8 MP and a squared-sum integral above about 66k pixels, so u64 is needed even on 1 MP proxies. Share with Enhancement.
4. **Small pieces:** f64 homography solve, Zhang-He, Huber IRLS, profile sampling, minimum-area rectangle (check imageproc's geometry module first), distance transform with marker-based watershed, bit-packed shear-sum, Lab conversion. Take `fast_image_resize` for proxies and imageproc or kornia-imgproc for Canny, contours, morphology and components (kornia claims byte parity with `cv2.Canny`; verify).

**opencv-rust as a dev-only oracle.** opencv 0.101.0 needs libclang and a system OpenCV, so it lives in a Linux-only `oracle` crate outside default workspace members, is banned from the shipped graph by `cargo-deny`, and is never packaged. It cross-checks homography, Canny, rectangles, LSD segments and `warpPerspective`; OpenCV's Lanczos4 is 8x8 against our 6x6, so warps are compared by PSNR (floor 45 dB, 07 §7.7) or SSIM tolerance.

## 4.14 Public engine API contract

The shell and CLI call the same functions. Types are `serde`-serialisable and versioned; the CLI's `--json` output is the serialised `Analysis`. Inputs are a decoded `Pyramid`, never a path, so decode, sandboxing and caps stay in `codecs`.

```rust
pub struct AnalyzeOptions {
    mode: TriageMode,           // Strict | Balanced | Aggressive
    content_hint: ContentHint,  // Auto | Document | Photo
    split: SplitPolicy,         // Auto | Always | Never (stored as EditState.split.policy, M10.17)
    dewarp: DewarpPolicy,       // Off | Suggest | Auto
    deskew: DeskewPolicy,       // Off | Auto | Suggest
    crop: CropSpec,             // the request: PaperEdge{margin} | ContentTight{pad}; resolved to MarginPolicy (02 §2.3)
    detectors: DetectorSet,     // Both (default) | MlOnly | ClassicalOnly
}
pub fn analyze(p: &Pyramid, o: &AnalyzeOptions, c: &CancelToken) -> Result<Analysis, ErrKind>;
pub fn refine_full_res(a: &mut Analysis, full: &Raster, c: &CancelToken) -> Result<(), ErrKind>;
pub fn suggest_dewarp(p: &Pyramid, item: &Item, c: &CancelToken) -> Result<DewarpResult, ErrKind>; // carries the GridWarp, verdict, confidence (M12.34)
pub fn snap_corner(full: &Raster, quad: &[Pt; 4], idx: usize, hint: Pt) -> Pt; // manual-adjust magnet (M4.80)
pub fn triage(a: &Analysis, mode: TriageMode) -> Triage;                      // pure, from calibration.json
pub fn to_edit_state(a: &Analysis) -> EditState;                              // the AutoSuggestion
```

| `Analysis` field | Meaning |
|---|---|
| `schema_version`, `algo_version`, `models[] {id, sha256}` | Reproducibility; copied into `EditState.origin`. |
| `outcome` | `Crop`, `TrimOnly`, `NoChange`, `Split(n)` or `Failed`. |
| `items[]` | `id`, `quad` and `quad_px`, `out_size`, `aspect {recovered, snapped_to}`, `quarter_turns`, `fine_deg` and source (`Edges`, `Text`, `Horizon`), `partial_frame`, `axis_aligned`, `residual_perspective`, `curvature_score`, `dewarp_suggested`, `candidates[] {source, quad, edge_support}`. |
| `confidence` | Per item and per file: `score` (calibrated, 0-1), `band`, `reasons[]` (registry codes, 4.9), `signals {peak_min, mask_iou, edge_support_mean, edge_support_min, ab_iou, tta_spread, residual_skew, orient_margin, split_stable}`. |
| `triage` | `AutoAccept`, `Review(reasons)` or `Failed`, for the batch's mode. |
| `timings`, `diagnostics` | Per-stage milliseconds, tier reached, detectors fired. |

Rules: (1) Check and Failed never produce a write; the batch scheduler enforces this, not the shell. (2) Results are deterministic per platform, model hash and runtime version but not bit-identical across CPUs, so snapshot tests use tolerances. (3) `analyze` does no I/O. (4) A manual edit is never overwritten: the shell stores the user's `Item` with `origin: Manual` and re-runs `analyze` only on request. (5) The CLI mirrors B4 by default (held items are reported, not written); its flags belong to the CLI section. (6) `to_edit_state` fills 02 §2.3's types and adds none: per item a `QuadWarp` (corners, quarter turns, `fine_deg`) or, after an applied dewarp, a `GridWarp`, the `MarginPolicy` resolved from `crop`, and the `confidence` with its `reasons[]`.

## 4.15 Evaluation and exit gates

All numbers are PROVISIONAL targets from the perf and geometry research, measured per golden-set slice and gated on the worst slice. The statistics are 07 §7.4's: a one-sided 95% Clopper-Pearson bound, a slice gates only from n = 80 (smaller slices are advisory), and slices under 30 images are suppressed from every public artefact. Nothing is ticked in the roadmap unless its gate passes.

| Feature | Metric | Release target |
|---|---|---|
| Quad | Mean IoU after canonical warp; failures (IoU under 0.90, the silent-failure line in 4.9) | Mean at least 0.985; failures at most 2% realistic, 6% hard slice |
| Corners | Error over image diagonal (golden set, before refinement); pixels after refinement | Before: median at most 0.15%, p95 at most 0.5%. After, at 12 MP: median at most 2 px, p95 at most 5 px, synthetic only (annotator noise may exceed 2 px) |
| Skew | Absolute angle error | Text pages: median at most 0.1 degrees, p95 at most 0.25 degrees; photos: p95 at most 0.5 degrees |
| Orientation | Top-1 | At least 99%; EXIF and HEIF rotation 100% (no double-apply) |
| Crop | IoU vs hand box; clipping | Mean at least 0.97, p5 at least 0.92; clipping at most 0.5% |
| Multi-item (v1.0 gate, G4) | Exact item count; precision and recall at IoU 0.9 on separated items; touching or overlapping scans | Count at least 97%, recall at least 95%, precision at least 97%; at least 90% of touching or overlapping scans routed to review |
| Dewarp (v1.0 gate, G5) | CER vs perspective-only; do-no-harm; flat-page skip | Median relative CER cut at least 20% on the curved slice; worse by over 1 point on at most 2%; at least 95% of flat pages skipped, rendering byte-identical to the perspective-only output |
| Confidence | ECE; risk-coverage | ECE at most 0.05 pooled and at most 0.10 in the worst slice (n at least 30); Balanced silent failures: point at most 1.0%, bound at most 2.0% (G2) |

Harness: a 200-image deterministic smoke set per pull request (block on a 0.3-point mean IoU drop or 0.5-point failure-rate rise), the full suite nightly, OCR before and after as a secondary metric. The locked golden set is staged (v0 at least 150 images, v1 at least 500, v2 at least 800 before 1.0; two annotators on 20% for the noise floor; 07 §7.5) and stays private: its images never leave the maintainer's encrypted disk (Assumption A-8). It runs only from the private `auto-crop-golden` workflow on the maintainer's machine, once per release candidate and never on a fork PR or a push (B21), and only aggregates are published.

## 4.16 Schedule risk, spikes and open items

**Schedule risk (B10).** Research advised cutting dewarp and multi-item from v1.0. They stay, gated by 4.15, and are the items most likely to move the 1.0 date: one maintainer also writes the LSD port, warp kernel, training pipeline and provenance audits (only the LSD-and-warp figure of 2-3 weeks has a research estimate). Suggested order: harness, classical Detector B, refinement and warp; the ML net; orientation and deskew; multi-item; dewarp last.

**Gate-miss rule (Assumption A-4).** A missed exit gate delays 1.0. Shipping the feature labelled Experimental (still opt-in) or moving it to 1.x reopens B10 and is the owner's decision, put to the user with the measured numbers; nothing is lowered, cut, relabelled or disabled silently. Proposed cut order if needed: dewarp, AVIF/JXL encoders, multi-item. (owner may veto) The rule covers dewarp, multi-item and the AVIF and JXL encoders. It is asked at the M0 wrap-up and stored as an ADR before the M10 gates (X.41); the decision itself runs through M12.68 (dewarp) and M13.42 (release).

**Spikes for the roadmap:** (1) kornia-imgproc per strip vs own u8 Lanczos; (2) ort vs rten vs tract, with Intel Mac and int8; (3) 8:1 receipt test and elongated ROI pass; (4) DocQuadNet licence query, UVDoc audit and author outreach (M4.81), Track A or B; (5) LSD port from OpenCV 4.5.4 or later with recorded provenance, or EDLines (the LSD ADR, M1.28); (6) public-data fetch scripts, synthetic generator, first calibrator fit on the real dev split; (7) own orientation net vs PP-LCNet baseline.

**Assumptions the user may veto:** the per-batch content hint; the Strict (0.3%) and Aggressive (3%) targets; dewarp weights bundled under about 40 MB total, else the hash-pinned Dewarp pack installed from a file (the app never downloads it); per-route margin defaults (Assumption A-11); `Suggest` as the dewarp default. The owner-decision assumptions that touch this section are A-2 (4.7), A-3 and A-8 (4.9, 4.15), A-4 (above), A-7 (4.3.1, 4.8, 4.11) and A-11 (4.6, 4.12); the full list is 01 §1.7.
