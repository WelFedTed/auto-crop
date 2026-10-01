> Auto Crop design, part 5 of 8 | [PLAN.md](../../PLAN.md) | [Decision log](00-decision-log.md) | [ROADMAP.md](../../ROADMAP.md)  
> Planning draft, 2026-10-01. Numbers marked PROVISIONAL are unmeasured estimates; decisions B1-B21 and the assumptions A-1..A-12 live in the decision log and in section 1.

# 5. Contrast and readability enhancement

Enhancement is a classical, CPU-only, pure-Rust stage that runs after geometry (crop, perspective, optional dewarp) and before encoding. It flattens uneven lighting, whitens paper, sets the tone curve and, on request, binarises. The rule: analyse once on a ~1.5 MP proxy, cache the maps, re-run only cheap stages for slider preview, and run the full-resolution pass once on save.

**Evidence status.** Nothing here is validated on receipts. The closest published method, [Bako et al. (ACCV 2016)](https://web.ece.ucsb.edu/~psen/Papers/ACCV16_RemovingShadows.pdf), was tested on 81 DSLR photos of 11 documents plus 16 Flickr images, with no receipt credited among them. DIBCO is historical manuscripts. Every number below is a literature-informed starting value, PROVISIONAL until the 5.10 harness says otherwise.

## 5.1 Decisions implemented

- **B13.** Enhancement is suggested with a one-tap live before/after, never forced: Original is the default state, a batch defaults to Off or Suggest, the CLI defaults to off, and an item that trips a readability-risk signal is held (5.3.3, 5.11). B&W is anti-aliased 8-bit, 1-bit is an export option, and despeckle is a separate Advanced switch that is off for receipts.
- **B7.** Hybrid ML covers detection only: the ~12-25 MB bundle is the corner/quad net and the 4-way orientation net. Enhancement ships no model in v1.0, only a dormant slot (5.8).
- **B3, B4, B6.** Overwrite-by-default means a lossy enhanced result replaces the original on save, so 5.11 adds guardrails. Readability-risk signals feed Good / Check / Failed triage: a batch item that trips one is held for review.
- **B10, B11.** Multi-item scans are enhanced per item after the split; dewarped pages use the dewarp page mask as validity mask. 1-bit PNG, TIFF G4 and PDF/G4 ship in v1.0; JBIG2 does not.
- **B2, B21.** Own kernels, permissive only. DocEnTr (CC BY-NC) and DE-GAN (GPL-3, academic-only note) are excluded; Leptonica, Doxa and OpenCV are dev-time oracles, never shipped. The receipt evaluation slice is private.

## 5.2 Position in the pipeline

```
 warped raster (one Lanczos resample, from geometry) + validity mask
 [P] proxy: area-average to ~1.5 MP
 [A] analysis: text height -> block grid -> percentile map -> reject
     -> inpaint -> smooth -> coarse RGB gain grid; b, w, gamma
 [F] flatten: fused per-pixel gain (bilinear from the coarse grid)
      +-> tonal: levels LUT -> chroma clean-up -> unsharp     => Auto, Grayscale
      +-> B&W:   luma -> (unsharp) -> Sauvola/NICK -> ramp    => 8-bit anti-aliased
 [D] optional colour-mark composite, despeckle; export: 1-bit cut at 128, encode
```

- The enhancer sees the geometry output. With the default crop margin (B14, set per route in 04 §4.6), pixels outside the paper quad are excluded from every statistic by the validity mask. In B&W they are filled white; in Auto and Grayscale they keep the nearest valid gain, so table edges stay natural.
- Original bypasses the enhancer, so the lossless JPEG fast path (90-degree turns, MCU-aligned crops) still applies. Every other mode costs one re-encode generation, stated in the export dialog.
- `EditState.enhance` is the default for every item, and `Item.enhance_override` replaces it for one item (02 §2.3). It holds `mode`, `look` (Receipt or Document), five `i8` slider offsets, B&W parameters, the Advanced overrides (despeckle is one of them, a separate switch, 5.5), and `est`: stored estimates (`algo_ver`, `text_h`, `b`, `w`, gamma, paper colour). It holds no bit depth: 1-bit is an export choice (5.9), not an edit. Storing `est` keeps output a pure function of (source, EditState) and makes export reuse the preview's estimates. The illumination grid is recomputed deterministically, never stored. Sliders are offsets from `est`, so they survive re-estimation after a geometry edit. An `algo_ver` bump (an algorithm update) re-estimates: `est` is recomputed instead of replayed, mode, look and slider offsets are kept, and the item shows an Info chip, "result changed after update".

## 5.3 Default pipelines

Lengths are in "300 dpi-equivalent" (scaled) units, multiplied by `dpi_eq / 300`. For photos `dpi_eq = 300 x text_h_px / 26`, where 26 px (median ink-component height of 10 pt text at 300 dpi) is PROVISIONAL, calibrated in spike E1. Scans use DPI metadata when present. Other values are in 5.12.

### 5.3.1 Phone photo of a receipt (look = Receipt)

1. **Proxy.** Area-average downscale (fast_image_resize) to ~1.5 MP by area, capped at a 3072 px long edge, never upsampled. A long-edge rule fails on receipts: at 1536 px an 8:1 strip is only ~0.3 MP.
2. **Text scale.** Rough local-contrast ink mask, connected components, median height of the dominant size mode gives `text_h`. Under ~30 components: fall back to short side / 40 and mark the look low-confidence.
3. **Illumination map.** Blocks of `1.75 x text_h` (16-64 px on the proxy), half-block stride. Per block, take the mean RGB of pixels at or above the 88th luma percentile (sweep 85-90; luma-selected rather than per-channel percentiles is our choice, A/B in the harness). Reject a block with:
   - percentile luma under 0.60 x the median of valid blocks (black bands, reverse-video headers);
   - over 35% ink coverage (dense barcodes, QR codes);
   - chroma over 12 Cb/Cr units from the dominant paper chroma (coloured headers, stamps), so coloured print is not whitened;
   - a position outside the validity mask.

   Inpaint rejected cells by push-pull diffusion on the coarse grid (~10k cells), then a 3x3 median and a Gaussian of one block. Upsampling is bilinear inside the apply kernel, so no full-resolution map exists.
4. **Gain.** `L_eff = P + s (L - P)`, with `P` the per-channel 90th percentile of valid cells and `s` the shadow strength. Per channel `g = clamp(255 / L_eff, 1, 3.0)`. This removes soft shadows and white-balances the paper in one step. "Keep paper tint" swaps in a luma-only gain.
5. **Tone.** On flattened luma: black point `b` at the 0.75th percentile (capped at 0.35 w); white point `w = 242` (95% of 255), so paper texture clips to white. Gamma puts the median ink-candidate pixel (below 0.75 w) near 0.32 of range, clamped to 0.8-1.0, with `out = x^(1/gamma)` (0.8 darkens midtones). One 256-entry LUT per channel. Guard: if the uncapped 0.75th percentile is above `w - 60` (almost no ink), skip the stretch and raise `low_contrast`.
6. **Clean-up.** Blur Cb/Cr with sigma 2 px, then chroma coring (smoothstep from 4 to 12 units), which removes residual paper tint but keeps saturated stamps and logos. Luma denoise (3x3 median or Gaussian sigma 0.7) runs only if the post-gain noise estimate (MAD on flat paper) exceeds 4 levels, since gain up to 3x amplifies shadow noise.
7. **Sharpen.** Unsharp on luma only: sigma 1.0 px, amount 0.5, threshold 3/255.
8. **Modes.** Auto is steps 1-7 with colour kept. Grayscale is Auto's luma. B&W is in 5.4.

### 5.3.2 Flatbed text scan (look = Document)

1. Measure illumination variation `V = (p95 - p5) / P` on the smoothed map. If `V < 3%` (PROVISIONAL), skip flattening; levels and gamma collapse into one LUT (~4 ms at 12 MP). Otherwise apply a mild large-block flatten for lid shading or binding shadow: blocks of 4-6 x `text_h` so text cannot leak into the map, `s = 0.6`, gain clamp 1.5.
2. Show-through: raise white point or gamma; B&W removes it. Denoise only if noisy.
3. B&W is Sauvola on flattened luma, as for receipts. Flatten-then-global-Otsu (the [Leptonica approach](https://tpgit.github.io/UnOfficialLeptDocs/leptonica/binarization.html)) is a harness candidate for clean pages, decided by the sweep. Despeckle is on by default in this look only (5.5).

### 5.3.3 Look selection and the suggestion

Look defaults to Document for scan-like sources (DPI metadata, `V < 3%`, near-paper aspect), otherwise Receipt; both are overridable. The suggestion chip ("Looks like a receipt or document: enhance?") appears when the analysis proxy shows paper fraction at least 55%, ink fraction 1-25%, and a visible Auto effect (mean delta-E above ~4, or `V` above 8%, or a paper cast). Thresholds are PROVISIONAL, tuned for precision.

**Never forced.** One tap on the chip applies Auto as an undoable edit with a live before/after. A batch defaults to Off or Suggest, never Auto: Suggest shows a banner with the count of qualifying images and a three-sample before/after preview (06 §6.8) and applies nothing until the user taps Apply. Auto, Grayscale and B&W therefore reach an item only as a visible, user-chosen, undoable value. The CLI defaults to `--enhance off` (`off|auto|gray|bw`, 02 §2.12) and a preset does not turn it on; `analyze` reports the suggestion and writes nothing. In a batch or CLI run, an item that trips a readability-risk signal (5.11) is held: nothing is written, the original stays untouched, it lists first in the review grid, and a CLI run counts it as held (02 §2.12).

## 5.4 Modes

- **Original:** geometry output untouched, enhancer bypassed; the default state, one tap away.
- **Auto:** RGB, colour kept (chroma is cleaned, never dropped), paper whitened (steps 1-7 of 5.3.1). The enhancer input is the sRGB raster of 03 §3.7: a non-sRGB profile is converted first (ICC read before nclx), so gains apply to sRGB data and Auto outputs sRGB, the default for enhanced output. If Preserve wide gamut is on when an enhancement mode is applied, the export sheet notes that the result is sRGB (M7.26 records the rule). A linear-light gain variant remains a harness A/B on sRGB data (5.14).
- **Grayscale:** 8-bit gray, Rec. 709 luma of Auto (correct because the input is sRGB). Keeps the source format (5.9).
- **B&W:** 8-bit anti-aliased gray (flatten, Sauvola on flattened luma, soft ramp). Keeps the source format (5.9); 1-bit only at export.

**B&W.**

- Sauvola: `T = m (1 + k (s/R - 1))`, local mean `m`, local standard deviation `s`, `R = 128`, `k = 0.25`. Window and `k` come from the 5.10 sweep.
- Clamp `T` to `[64, 208]`. This stops the classic failure where the interior of a large black area (reverse-video header, bold glyph) whitens because `s` is near zero, and stops paper noise from inking.
- Ramp: `out = 255 x smoothstep((Y_f - (T - rho)) / (2 rho))`, `rho = 6`: nearly bilevel, with edge softness from the source. Larger `rho` (24-32) keeps more gray edge detail and is swept.
- **Faded preset** ([NICK, Khurshid et al. 2009](https://helios2.mi.parisdescartes.fr/~vincent/articles/DRR_nick_binarization_09.pdf)): `T = m + k sqrt((sum p^2 - m^2) / NP)`, `k = -0.10`, `T_hi = 232`. In low-contrast windows NICK sits nearer the mean than Sauvola, so faint strokes survive, at the cost of paper-noise sensitivity, so the preset forces light denoise. It is offered as a chip when the faded-print detector (5.11) trips, never applied silently.

## 5.5 Sliders and parameter mapping

Five sliders, each -100..100, where 0 means the value in `est`. The UX section decides how many show by default; the engine exposes exactly five. Slopes are PROVISIONAL.

| Slider | Auto / Grayscale | B&W |
|---|---|---|
| Brightness | White point: +1 lowers `w` by 0.6 (to ~182); -1 raises by 0.13 (to 255) | `T` shifts by -0.3 levels per unit (brighter, less ink) |
| Contrast | Black point +0.3 per unit (cap 0.35 w); gamma x `(1 - 0.002 C)` | Ramp `rho`: 16 at -100, 6 at 0, 2 at +100 |
| Text darkness | Midtone gamma x `(1 - 0.003 D)` | Sauvola `k = 0.25 - 0.002 D`; NICK `k = -0.10 + 0.0005 D` |
| Clean-up | Chroma blur, coring, luma denoise | Denoise before thresholding |
| Sharpness | Unsharp amount `0.5 + 0.005 S` | Same, before Sauvola (B&W default 0.3) |

**Advanced** (collapsed, remembered): the 5.12 parameters, method choice, keep paper tint, keep colour marks, despeckle (a separate switch, below) and CLAHE (luma only, clip 2, 8x8, off). Output bit depth is not an enhancement setting and not in `EditState`: 1-bit is chosen in the export dialog (5.9).

**Despeckle** is its own Advanced switch, not a slider and not part of Clean-up, which is colour and noise only and never touches it. It is off for the Receipt look (B13) and on by default only in the Document look; the switch carries the erase caption of 06 §6.8, because it can delete decimal points and faint print. A thermal decimal point is a 2x2-dot cluster, ~0.25 mm on a typical 203 dpi head: ~20 px^2 where an 80 mm receipt fills 1500 px of a phone photo (an estimate), but 1-3 px^2 at proxy scale, like a speck. So despeckle never runs on proxy renders (only in full-resolution tiles at 100% zoom and at export), and it is context-aware: a component is deleted only if it is under the area limit (4 px^2 at 300 dpi, scaled by `(dpi/300)^2`) and no other component lies within 1.0 x `text_h` horizontally in the same baseline band, so decimal points, commas and colons beside digits survive. The synthetic price-column test (5.13) gates this code.

## 5.6 Stamps, logos and coloured paper

- **Auto** keeps colour by construction: gain is a slow multiplier, coring touches only near-neutral chroma, and chroma-rejected blocks stop coloured print being whitened. A stamp-colour test bounds delta-E (5.13).
- **Coloured paper** (pink, yellow) is whitened, which is Auto's intent; "Keep paper tint" preserves it. A tinted header on white paper is chroma-rejected and keeps its colour.
- **Grayscale and B&W** turn stamps gray or black, and yellow highlighter vanishes. **Keep colour marks** (Advanced, off) composites colour into B&W: a chroma mask after white balance (chroma at least 24 units, area at least 150 px^2 scaled) keeps flattened RGB for stamps and signatures. Output then has three channels, so 1-bit and G4 export are disabled.

## 5.7 Parameter caching and preview

Each stage caches its output keyed by a hash of only the `EditState` fields it reads, so a slider re-runs the shortest suffix (tonal sliders touch T; in B&W, Text darkness, Brightness and Contrast touch only R). Costs are scaled by pixel count from the 12 MP measurements below and are PROVISIONAL.

| Stage | Key (what invalidates it) | Cost at 2 MP |
|---|---|---|
| P proxy | geometry edits | 2-4 ms |
| A analysis + F flatten | P + block factor, percentile, clamp, shadow strength, tint | 12-35 ms |
| T tonal | F + `b`, `w`, gamma, clean-up, sharpness | LUT under 1 ms; chroma 2-4; unsharp 3-6 |
| S stats (`m`, `s`, one window) | luma of F + sharpness + window | 8-12 ms |
| R threshold + ramp | S + `k`, offset, `rho`, clamp | 1-2 ms |
| E export | all, full-res, in strips | 60-250 ms at 12 MP (Auto, Grayscale: 120 target; B&W, Faded: up to 250) |

- **Budgets (PROVISIONAL).** Slider: at most 16 ms Rust compute on the 2 MP display proxy on the preview pool, ~50 ms end to end through IPC. Full-resolution `enhance` at 12 MP is Table A line 7 of 07 §7.1: 120 ms wall for Auto and Grayscale, inside the 700 ms p50 end-to-end target (crop-only is 580 ms). B&W and Faded add the Sauvola or NICK window statistics (Sauvola alone measured 60-74 ms, naive) and may take up to 250 ms, so their end-to-end total is at most 830 ms; 07 §7.1 carries that as a B&W/Faded variant row, and the 700 ms line is not claimed for them. E3 re-baselines both lines. The earlier 40 ms line in the performance research is dropped. Enhancement re-estimates on geometry release.
- **Measurement base.** Naive std-only Rust, synthetic 12 MP, i7-8700K: luma 4 ms, LUT 4 ms, Sauvola 31x31 ~60-74 ms, proxy flatten ~55 ms. No Tier-M, Apple silicon or ARM figure exists yet.
- **Scale invariance.** Sizes are in scaled units, so tonal modes agree between proxy and full-res to within rounding. B&W does not agree pixel for pixel on thin strokes, so at 100% zoom or more the visible region (and the before/after compare) is rendered from full-res tiles with a halo of `r_window + r_unsharp`; a tile must equal the same crop of a whole-image render (5.13).

## 5.8 Kernel implementation notes

- **Crates.** `fast_image_resize` 6.1.0 for proxy and final resizing. `rayon` with `par_iter` and `scope` only (panics in `spawn` abort), per-item `catch_unwind`, separate preview and batch pools. `wide` or `pulp` for stable SIMD with runtime dispatch (AVX2 floor per C1, NEON, scalar fallback). `imageproc` 0.27 is only a test reference: its `adaptive_threshold` is a box-mean (Bradley-style) threshold, its u32 integrals overflow (sums above ~16.8 MP, squared sums above ~66k pixels), and it has no Sauvola, NICK or CLAHE.
- **Sauvola with u64 strip integrals** ([technique](https://www-live.dfki.de/fileadmin/user_upload/import/2676_FsDkTmbEfficientImplSpie2008.pdf)). Strips of ~256 output rows plus an `r_window` halo each side; two u64 tables (`sum p`, `sum p^2`) per strip, so scratch per thread is `W x (256 + 2r) x 16 B`, ~23 MB for a 4000 px image at `r = 50`. Full-image u64 pairs would cost ~192 MB at 12 MP and ~770 MB at 48 MP. The exact variance numerator `N x Q - S^2` fits u64 for windows up to ~1000 x 1000; f32 SIMD then does the square root and threshold. Spike E3 benchmarks sliding column sums and wrapping-u32 accumulators (valid up to ~256 x 256 windows) as alternatives.
- **Fixed point and memory.** Gain is Q8.8 in u16; LUTs are u8. Integer stages are bit-exact across AVX2, NEON and scalar; coarse-map stages use f32 without fused multiply-add and match within 1 level. Enhancement runs in place on the warped RGB8 raster in strips, adding one luma plane and per-thread scratch, within the per-image peak budget (3x decoded RGB8 plus 64 MB) even at 100 MP.
- **No wgpu in v1.0.** A LUT pass costs ~4 ms at 12 MP on the CPU, and GPU upload cost, driver variance over three OSes and a second copy of every kernel (which drifts) are not worth it. Revisit only if Tier-M benchmarks miss the slider budget.
- **No ML in v1.0 except a dormant model slot**: a trait plus loader for an ONNX model on the `ort` runtime that detection already ships. A model may only propose a low-frequency gain map (output/input on a ~768 x 1024 proxy, low-passed and clamped to the coarse grid) replacing stage A's estimator, so it can never synthesise or alter glyphs. No default model ships: DocShadow-SD7K is MIT code but archived, its ONNX exports are ~114 MiB each, and its weight and training-data licences are unstated, so it fails the OSI-only weights policy until audited. In 1.0 the slot is empty and loads nothing: it accepts only a file pinned in `models.lock` (04 §4.10), read into memory and hash-verified before the session is created, and there is no in-app model download (B18). User-supplied or downloaded models are post-1.0 (roadmap B.13) and would need a pinned SHA-256, an OSI-licensed provenance row (08 §8.1.5) and loading inside the sandboxed worker. ML binarisation is not planned.

## 5.9 Output encodings

B&W and Grayscale keep the output format that 03 §3.2.3 gives the source: JPEG stays JPEG, PNG stays PNG, TIFF stays TIFF, HEIC becomes JPG, BMP becomes PNG. An enhancement mode never changes the format silently. For a lossy source, the export sheet suggests "Save as PNG" when the mode is B&W, because PNG is lossless and keeps faint marks, and the first-save notice (5.11 item 5) says so. Bit depth is an export choice, never part of `EditState` (5.2).

| Output | Encoder | Notes |
|---|---|---|
| 8-bit gray JPEG | `turbojpeg`, gray subsampling, libjpeg-turbo 3.1.4 or later | Default for B&W and Grayscale when the source's output format is JPEG. Single component, quality never below 90 (q90 default, PROVISIONAL); low quality rings around text. |
| 8-bit gray PNG | `png` 0.18 | Default when the source's output format is PNG or the user picks it; the "Save as PNG" suggestion for B&W. Lossless; keeps faint marks. |
| 8-bit gray, other formats | encoders of 03 §3.3 | TIFF stays TIFF; every other source follows 03 §3.2.3. |
| 1-bit PNG | `png`, bit depth 1 | Export option (`--bits 1 --bilevel png`). Cut at 128 from the anti-aliased result, no dithering. |
| TIFF CCITT G4 | `fax` 0.3.0 (MIT) inside our own TIFF wrapper | Export option (`--bits 1 --bilevel tiff-g4`). Below. |
| PDF/G4 | `pdf-writer` 0.15 | `CCITTFaxDecode` image XObject; the general PDF writer is in 03 §3.3. |

**Own TIFF wrapper.** `fax::tiff::wrap` hardcodes 200 dpi, WhiteIsZero and one strip, and the `tiff` crate (0.11.3) decodes Fax4 but cannot encode it. Our wrapper writes Compression 4, T6Options 0, BitsPerSample 1, WhiteIsZero, real X/Y resolution with ResolutionUnit, and a configurable `RowsPerStrip` (default one strip). Multi-page uses an IFD chain. With no known physical size, ResolutionUnit is "none" rather than an invented dpi. Interop is tested by round trip through `tiff`, libtiff, ImageMagick and Leptonica (dev-only oracles); spike E2 verifies the fax encoder's polarity and strip API.

**PDF/G4.** `/CCITTFaxDecode` with `/K -1`, `/DeviceGray`, 1 bit per component. `BlackIs1` polarity is the classic bug, so a render-and-compare test runs through pdfium and pdf.js as dev oracles. Page size derives from DPI (nominal 300 when unknown).

**Sizes, without over-promising.** A raw 1-bit A4 page at 300 dpi is ~1.09 MB. A synthetic dense text page measured ~30 KB in G4 (an earlier "9 KB" figure is unverified); speckle inflates G4 many-fold, so the dialog shows the real size. **JBIG2 is not in v1.0.** The "3-5x smaller than G4" claim traces to a committee press release, and jbig2enc's own numbers show lossless generic-region output about the size of lossless symbol mode. Lossy symbol matching caused the 2013 Xerox digit-substitution bug, which on receipts would silently change amounts. Any later JBIG2 work must first measure generic-region coding against G4 on our corpus and never use lossy matching.

## 5.10 Evaluation

**Reading the ML numbers correctly.** The often-quoted DocEnTr figures (Otsu 77.7, Sauvola 77.1, DocEnTr-Base 92.5 F-measure) belong to DIBCO 2017. On H-DIBCO 2018 the same paper gives 51.5, 67.8 and 90.6, and a cGAN method (92.4) ranks above DocEnTr ([arXiv 2201.10252](https://arxiv.org/pdf/2201.10252)). The baselines are untuned and none of it covers receipts.

**Datasets.** Public data is fetched by script with checksums, never vendored.

- DIBCO and H-DIBCO for binariser regression (F-measure, pseudo-F, PSNR, DRD), with [Doxa](https://github.com/brandonmpetty/Doxa) BinBench (CC0) as the reference.
- [SmartDoc-QA](https://zenodo.org/records/5293201) (CC BY 4.0, one ~13 GB zip, receipts are one of three document types, has transcriptions) and [CORD](https://github.com/clovaai/cord) (CC BY 4.0, 1,000 receipt photos) for CER. SROIE is excluded (licence unconfirmed).
- Synthetic receipts (public, exact ground truth): 203 dpi dot fonts, price columns, barcodes, reverse-video headers, degraded with Augraphy and OpenCV (shadow, fade ladder, blur, noise, JPEG), sharing no code with the app.
- **Receipt slice of the private golden set (B21).** No receipt binarisation benchmark exists, so we build one: 80-150 real receipts stratified by thermal fade, coloured paper, hard shadow, crumple, flash glare, reverse-video headers, barcodes, stamps, decimal-dense price columns, small text and aspect above 4:1. Ground truth is a human transcription of item lines and amount tokens, plus a paired 300 dpi flatbed scan where possible. It runs only through the private-repo workflow of 07 §7.5 (never on public CI or fork PRs) and reports aggregates only. A tuning split and a locked release split limit overfitting.

**Metrics.**

- CER from pinned Tesseract 5.5.x with fixed page-segmentation mode, language and input scale, run with `thresholding_method` 0 (Otsu) and 2 (Sauvola; window 0.33 x dpi, ~100 px at 300 dpi, `k` 0.34). Per the [Tesseract docs](https://tesseract-ocr.github.io/tessdoc/ImproveQuality.html) it binarises internally, so the setting is fixed. It is a dev-only proxy (OCR is a v1.0 non-goal) that favours hard binarisation and cannot see ghosting.
- **Amount-token exact match**: per receipt, the share of money tokens (`\d+[.,]\d{2}`) transcribed exactly, compared paired against the same receipt's original. This is the receipt-critical metric and drives the damage gate below.
- Artefact detectors (ghost-patch fraction, gain-map seam strength, small-mark loss) plus a generated contact sheet per slice for human review before each release. Contact sheets of the private slice never leave the maintainer's machine (07 §7.5); public sheets use synthetic data.

**Sweeps (spike E1).** The 31-51 px window in the research is the least-supported number here, since Tesseract's own is ~100 px.

| Parameter | Sweep |
|---|---|
| Sauvola window at 300 dpi | 31, 51, 75, 101, 151, 201 px, plus Tesseract's (~100 px, `k` 0.34) as a baseline arm |
| Sauvola `k` / NICK `k` | 0.1, 0.2, 0.25, 0.34, 0.5 / -0.05 to -0.3 |
| Others | Flatten then global Otsu; percentile 0.80-0.95; block factor 1.25-3.0; `rho` 2-24; threshold clamp |

Choose by the worst slice, not the mean: coarse pass on DIBCO plus synthetic, then the receipt tuning split, confirmed once on the locked split.

**Release gates** (PROVISIONAL, from the performance research).

| Gate | Target |
|---|---|
| Median delta-CER, enhanced vs original | at most 0 |
| Low-contrast slice | at least 20% relative CER reduction |
| Images worse by more than 1 CER point | at most 2% |
| Amount-token exact match (enhancement damage gate, below) | zero lost tokens on the synthetic suite; one-sided 95% bound of the mean paired difference no worse than -0.5 pt on private slices |
| DIBCO F-measure; ghost and seam detectors | within 1 point of the Sauvola baseline; zero failures on the synthetic suite |

**Enhancement damage gate.** The test is paired per receipt: each receipt's exact-match rate of amount tokens after enhancement is compared with the same receipt's original, per mode.

- On the synthetic thermal-fade and price-column suite (exact ground truth) the target is zero lost amount tokens, and a lost token blocks that mode as a default.
- On each private slice the lower one-sided 95% confidence bound of the mean paired difference (enhanced minus original, in points of exact-match rate) must be no worse than -0.5 pt, and every receipt that lost a token is listed for human review on the maintainer's machine.
- A slice with n < 30 is advisory only and is not published (07 §7.5).

07 §7.4 and G3 quote this rule.

## 5.11 Risks and guardrails

| Risk | Consequence | Mitigation |
|---|---|---|
| Over-binarisation (lost decimal points, commas, thin digits) | Wrong amount saved silently | 8-bit default, threshold clamp, despeckle off, small-mark check, 100% tile preview, amount-token gate |
| Faded thermal paper | Sauvola under-inks | Faded-print detector, NICK preset, Auto alternative, warning. Ink absent from the photo cannot be recovered. |
| Flatten artefacts, hard shadows, flash glare; no receipt validation | Ghost patches, seams, unknown failure modes | Block rejection, inpaint, clamps, seam and ghost detectors, receipt slice, suggestion-only |
| Coloured paper or headers; shadow noise amplification | Bako's stated limit; grainy dark areas | Chroma-based block rejection, Keep paper tint; post-gain noise estimate triggers denoise, gain capped at 3.0 |
| Preview differs from export; tuning overfits the private set | Surprise output; poor field accuracy | Shared estimates, scale-invariant parameters, full-res tiles at 100%; tuning and locked splits |

Overwrite-by-default (B3) compounds all of these, so backup and Restore original are a precondition. Guardrails:

1. **Original is one tap away**, with press-and-hold compare and undo.
2. **Readability-risk signals** (hold codes in the single registry of 04 §4.9), computed on the analysis proxy and the result:
   - faded print: paper-minus-ink contrast under a threshold;
   - over- or under-inking: B&W ink coverage differs from a flattened-Otsu reference by over ~25% (PROVISIONAL);
   - lost marks: over ~5% of small dark components (2-30 px^2 scaled) in flattened luma are absent from the output;
   - unreliable estimate: over 40% of blocks rejected, or `low_contrast`.
3. **A visible warning.** A tripped signal puts a badge on the thumbnail and an amber banner in the editor ("Faint print detected. B&W may drop small marks. Check amounts.") with "Use Auto (colour)", "Try Faded preset" and "Show original". In a batch the item is held: it moves to Check and lists first in the review grid, nothing is written and the original stays untouched (B4). A CLI run holds it the same way (5.3.3).
4. **No content inpainting.** Inpainting happens only on the coarse illumination grid, and a model slot can influence only that grid.
5. **First-save notice.** The first B&W or Faded save over an original shows a one-time dialog: the original is backed up (kept 30 days by default) and restorable from the Backups panel, and for a lossy source it offers Save as PNG, which is lossless and keeps faint marks.

## 5.12 Parameter defaults

All values are PROVISIONAL until spike E1. Where two values are shown, the first is Receipt and the second Document. Search ranges are in the 5.10 sweep table.

| Parameter | Default |
|---|---|
| Proxy area | 1.5 MP, long edge at most 3072 px |
| Block factor (x `text_h`) | 1.75 (16-64 px) / 5.0 |
| Block percentile | 0.88 (0.80-0.95) |
| Block reject: luma / coverage / chroma | 0.60 x median / 35% / 12 units |
| Shadow strength `s`; gain clamp | 1.0 / 0.6; 3.0 / 1.5 |
| Black point / white point | 0.75th percentile (cap 0.35 w) / 242 |
| Gamma (`out = x^(1/gamma)`) | auto, clamped 0.8-1.0 |
| Chroma blur / coring / denoise trigger | 2 px scaled / 4-12 units / noise above 4 levels |
| Unsharp: sigma, amount, threshold | 1.0 px scaled, 0.5, 3/255 (B&W amount 0.3) / 0.8 px |
| Sauvola window at 300 dpi; `k`, `R` | 51 px initial, expected to move up; 0.25, 128 |
| Faded (NICK) `k`, `T_hi` | -0.10, 232 |
| Threshold clamp; ramp `rho` | [64, 208]; 6 |
| Despeckle; CLAHE; colour marks; paper tint | off / on (up to 4 px^2 scaled); off; off; off |

## 5.13 Test plan

- **Kernel correctness.** Integral-image Sauvola and NICK match a naive reference within 1 level on random images and windows (including windows larger than the image, 1xN and Nx1). Output is independent of strip height and thread count, with no overflow on a synthetic 100 MP image. Integer stages are bit-exact across AVX2, NEON and scalar. A full-res tile equals the same crop of a whole-image render. Dev-only oracles, never shipped: OpenCV `ximgproc`, Leptonica and Doxa, within 1-2 levels.
- **Synthetic artefact cases (public).** Reverse-video header and black band stay black. Price-column decimal points survive at 150, 200 and 300 dpi-equivalent, with Sauvola, NICK, and despeckle on and off. A fade ladder (ink contrast 10-60%) behaves monotonically and the faded detector trips below its threshold. Also: shadow gradients, barcode block, coloured paper, stamp colour retention, 8:1 strip, tiny image, all-black input. Blank pages get no noise amplification and set `low_contrast`; Auto on a clean scan moves mean luma by at most ~2 levels.
- **Quality gates.** The 5.10 gates run per PR on a 200-image public smoke set, nightly on the full public suite, and on the private set only through the private-repo workflow of 07 §7.5 (the maintainer's machine, never per PR or on forks, aggregates only).
- **Performance.** `criterion` for trends and `gungraun` instruction counts (Linux) as the CI gate (regression over 5% fails) on downscale, percentile grid, gain apply, LUT, unsharp and the Sauvola strip. Nightly wall-clock on 12, 48 and 100 MP files on Tier-M and Apple silicon (12 MP `enhance` against the 120 ms line for Auto and Grayscale and the 250 ms line for B&W and Faded), a peak-RSS test on 48 and 100 MP B&W, and a slider test (p95 compute at most 16 ms at 2 MP).
- **Defaults and contracts.** Tests assert that mode is Original until a user or batch action chooses; the batch default is Off or Suggest and the CLI default is off; an item that trips a readability-risk signal is held and not written; B&W and Grayscale keep the source format (gray JPEG at q90 or above); `EditState` holds no bit depth; despeckle is off in the Receipt look and independent of Clean-up; and an `algo_ver` bump re-estimates and raises the "result changed after update" chip with no stored grid.
- **Encoders and robustness.** TIFF G4 and PDF/G4 round-trip pixel-exact through independent readers (polarity, multi-strip, multi-page). `cargo-fuzz` on the kernels.

## 5.14 Spikes and roadmap hand-off

- **E1, parameter sweep and flatten variants:** window, `k`, ramp, percentile, flatten-then-Otsu, luma-selected vs per-channel percentiles, sRGB vs linear-light gain, on DIBCO, synthetic and the receipt slice; calibrate `dpi_eq`.
- **E2, G4 and PDF interop:** own TIFF wrapper, fax polarity and strip API, PDF `BlackIs1`.
- **E3, kernel benchmarks:** u64 strips vs sliding sums vs wrapping u32 at 12, 48 and 100 MP on Tier-M and Apple silicon; re-baseline every PROVISIONAL time here, including the 120 ms and 250 ms `enhance` lines.
- **E4, signal thresholds:** faded-print, over-inking, lost-mark and suggestion thresholds against the golden set.
