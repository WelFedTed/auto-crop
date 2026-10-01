# Research: Contrast and readability enhancement for B&W document scans  (key: enhance)

## Summary
Recommendation: a classical, pure-Rust, CPU-only pipeline. Estimate the illumination on a downscaled proxy, divide it out (this flattens shadows and whitens the paper), then apply a tone curve and gentle cleanup. B&W mode adds a soft Sauvola threshold (NICK for faded print) on top. This is the approach Leptonica documents, and Bako et al. validated the same idea on receipts and menus. Phase 1 should ship no ML. The strongest models have licence problems: DocEnTr is CC BY-NC and DE-GAN is GPL-3 with an academic-only note. DocShadow is MIT but about 120 MB per model. A quick benchmark I ran in scratch space (naive std-only Rust, i7-8700K) put full-res Sauvola on 12 MP at about 60-70 ms and proxy-based flattening at about 55 ms. That is comfortably fast enough without wgpu, so slider previews can run on the CPU. The main gap is evidence: no receipt-specific benchmark exists, so every default below is a literature-informed starting value that needs tuning on an evaluation harness.

## Recommendation
Build a pure-Rust enhancement crate using rayon, fast_image_resize, and hand-written kernels. Do not use OpenCV. The default pipeline is: proxy illumination map, per-pixel gain, white-balance, levels/gamma, light cleanup, then unsharp. B&W mode is Sauvola on the flattened luma, with NICK as a "Faded" preset. Output B&W as anti-aliased 8-bit by default and offer 1-bit as an export option. Parameters and maps are estimated once on a roughly 1-1.5 MP proxy and cached, so sliders only re-run a cheap final stage at preview size. Full resolution runs once, on export. Skip ML in v1 but design a model-download slot (ONNX, on demand). Skip wgpu in v1. Write bilevel outputs as 1-bit PNG, TIFF G4 (via the fax crate) and PDF/G4. Allow JBIG2 only as lossless generic-region, never symbol mode. Build an evaluation harness (DIBCO metrics, SmartDoc-QA receipts and CORD with Tesseract CER) before freezing defaults.

## Key findings
- Flatten-then-threshold is the strongest classical recipe. Leptonica normalises the local background to a constant (about 200) and then applies a global threshold, with Sauvola as the alternative. Bako et al. do per-block background estimation and a per-pixel gain map, and report it works on receipts and menus.
- Rust gap: imageproc 0.27 has only a box-mean adaptive_threshold (integral image), Otsu, equalize_histogram and stretch_contrast. It has no Sauvola, Wolf, NICK or CLAHE, so we write a few hundred lines ourselves. opencv-rust needs a system OpenCV and libclang, which hurts three-OS packaging.
- Measured in scratch space (naive std-only Rust, i7-8700K 6C/12T, synthetic 12 MP): RGB to luma 4 ms; 256-entry LUT on RGB 4 ms; Sauvola 31x31 full-res about 40 ms integral (1 thread) plus 18-30 ms threshold (12 threads), or 110-170 ms threshold on 1 thread; proxy flatten of RGB about 55 ms. GPU compute is not needed for slider preview.
- Integral-image Sauvola gives the same thresholds as the naive version at about 5x (15x15 window) to 20x (40x40) speed-up. It needs 64-bit sums for squared intensities, because 32-bit overflows. Memory is roughly 12 bytes per pixel, or about 144 MB at 12 MP, so use strips or sliding sums, or run on the proxy.
- Local method choice: Sauvola under-inks faded, low-contrast print, and Wolf depends on global min and max-std, which a dark border breaks. NICK shifts the threshold down for light, low-contrast pages (k about -0.1 to -0.2) and suits thermal receipts, though the paper's OCR test was small. Keep Sauvola as the default and NICK as a preset until the harness decides.
- ML wins on hard historical documents (H-DIBCO 2018 F-measure about 92.5 for DocEnTr-Base against about 77 for plain Otsu or Sauvola). But those baselines lack illumination normalisation, the data is manuscripts rather than receipts, and DocEnTr-Base has 68M parameters run on 256x256 patches. Licences also block it: DocEnTr is CC BY-NC 4.0 and DE-GAN is GPL-3 with an academic-only note.
- Permissive ML options exist but are heavy. DocRes (MIT), DocDiff (MIT, diffusion) and DocShadow-SD7K (MIT, repository archived, ONNX about 120 MB per model) all have public code. Weights are trained on datasets whose licences are unclear. If used, run at 768x1024 and apply the output/input ratio as a gain map at full resolution.
- Bilevel output: a clean 300 dpi A4 page is about 9 KB in G4 versus 1.09 MB raw 1-bit, but noise inflates G4 many-fold. JBIG2 lossless is 3-5x smaller than G4, but symbol mode caused the 2013 Xerox digit-substitution bug. The tiff crate cannot write CCITT; the fax crate (MIT) has a G4 encoder.

## Risks
- No receipt-specific benchmark is known. DIBCO is historical manuscripts, so all defaults are starting values and Sauvola versus NICK versus flatten-then-Otsu is untested on thermal receipts. Mitigate with the harness.
- Flattening artefacts: large black regions (reverse-video headers, barcodes) can become ghost-white patches; hard shadow edges leave seams; coloured or non-uniform paper breaks the global reference (Bako et al.'s stated limits). Use block rejection, inpainting, a gain clamp and a Keep paper tint option.
- Over-aggressive B&W and despeckle can erase decimal points, commas and faint thermal print, which for receipts means changed amounts. Keep despeckle off by default for receipts, use a soft 8-bit default and keep undo.
- JBIG2 symbol mode can silently swap similar characters (Xerox 2013). Offer generic-region lossless only.
- Benchmark caveat: synthetic image, naive code, one 2017 desktop CPU with no ARM or Apple silicon numbers. Real speeds will differ; re-measure on macOS and Windows on ARM.
- ML licence and provenance: CC BY-NC and GPL-3 plus academic-only are incompatible with a free, open-source release; weights trained on datasets with unstated licences may carry restrictions.
- Rust ecosystem gaps (no Sauvola, NICK or CLAHE; tiff cannot write CCITT; immature JBIG2 encoder; ort is still an RC) mean more code to write and maintain.
- OCR CER proxy pitfalls: Tesseract 5 does its own Otsu or Sauvola thresholding, so results depend on that setting; SROIE's licence was not confirmed.

## Options evaluated

### Classical flatten-then-threshold pipeline (illumination gain map + levels + soft Sauvola/NICK) — recommended
Estimate the illumination on a proxy, divide it out, apply a tone curve, then threshold locally for B&W.
- licence: Own code (MIT/Apache-2.0 target)
- status: Pattern documented by Leptonica (BSD-2-Clause, pushed Sept 2026) and Bako et al. (ACCV 2016). imageproc 0.27 (MIT) provides only helpers.
- pros: Fast: about 60-130 ms per 12 MP in a naive benchmark; Deterministic and explainable, with no model download; Permissive licensing (own code, with Leptonica and Doxa as references); Slider parameters map directly onto pipeline constants
- cons: Struggles with hard shadows, wavy paper and coloured paper; Needs its own implementation in Rust; No receipt-specific validation exists

### Local adaptive binarisation alone (Sauvola, Wolf, NICK, Bradley) — viable
A per-pixel threshold from local mean and standard deviation, computed with integral images or sliding sums.
- licence: Algorithms are public; Doxa is CC0
- status: Mature. Doxa was last pushed July 2026.
- pros: Cheap and O(1) per pixel; The B&W stage in Scan Tailor, Tesseract and Leptonica; Reference C++ implementations exist (Doxa, CC0, 18 algorithms)
- cons: Alone it does not white-balance colour or preserve stamps; Parameter sensitivity (window, k); 32-bit integral images overflow

### CLAHE / local histogram equalisation — fallback
Tile-based contrast-limited histogram equalisation.
- licence: Own implementation
- status: Absent from imageproc 0.27
- pros: Helps faded photos; Well known
- cons: Amplifies paper texture and JPEG noise; Can halo; No Rust crate found in this session; Not in imageproc

### ML enhancement / binarisation (DocEnTr, DE-GAN, DocDiff, DocRes) — avoid
Learned document restoration and binarisation networks.
- licence: CC BY-NC 4.0 (DocEnTr), GPL-3.0 (DE-GAN), MIT (DocDiff, DocRes)
- status: DocEnTr pushed Jan 2025, DocRes Aug 2025, DocDiff Aug 2024, DE-GAN Mar 2023
- pros: Large F-measure gains on degraded historical scans (about 92.5 vs about 77); DocRes and DocDiff are MIT
- cons: DocEnTr is CC BY-NC and DE-GAN is GPL-3 plus academic-only; Heavy and slow at 12 MP; No ONNX export documented; Trained on historical documents rather than receipts

### ML shadow removal (DocShadow-SD7K, LP-IOANet, others) via ONNX — viable
An optional learned deshadowing stage for hard shadows, used as a gain map.
- licence: MIT (DocShadow code); weights and dataset licences unclear
- status: DocShadow-SD7K archived; last push June 2024
- pros: DocShadow code is MIT and ONNX exports exist; Handles hard shadows that block methods cannot
- cons: About 120 MB model per dataset variant; The DocShadow repository is archived; LP-IOANet has no code release; ort is still 2.0.0-rc.13; tract-onnx 0.23.8 may lack ops

### Bindings to OpenCV (opencv-rust) or Leptonica (FFI) — fallback
Use existing C/C++ CLAHE, ximgproc Sauvola/Wolf/NICK and Leptonica background normalisation.
- licence: OpenCV/Leptonica permissive; opencv-rust MIT
- status: Both active in 2026
- pros: Mature, tested algorithms; Leptonica is BSD-2-Clause and has background-norm and Sauvola; opencv-rust is MIT and active (Sept 2026)
- cons: Native dependency plus libclang and bindgen complicate three-OS packaging; Slows CI and release builds; Harder to SIMD-tune and stream by strips

### wgpu compute for real-time slider preview — avoid
Run the tone, threshold and gain stages as GPU compute shaders.
- licence: MIT OR Apache-2.0
- status: Reported version 30.0.1, Aug 2026 (crates.io API, second-hand)
- pros: Scales to very large images; wgpu is active (v30.x line reported Aug 2026, MIT/Apache-2.0)
- cons: Upload cost and driver variance across Windows, macOS and Linux; Duplicate CPU and GPU code paths; The CPU proxy path already takes about 4 ms per LUT pass at 12 MP

### Bilevel export codecs (PNG 1-bit, TIFF G4 via fax, PDF/G4, JBIG2) — viable
Encoders for 1-bit output.
- licence: png/fax/pdf-writer permissive; jbig2enc Apache-2.0
- status: fax 0.3.0 (July 2026), png 0.18.1, jbig2enc pushed Sept 2026
- pros: G4 is tiny for clean text; fax crate is MIT with an encoder; pdf-writer 0.15 can embed CCITT
- cons: tiff crate cannot write CCITT; Rust JBIG2 encoders are immature (jbig2enc-rust had 357 downloads); jbig2enc (C++) needs Leptonica; JBIG2 symbol mode can swap characters

## Deliverable
**Design rule.** Analyse on a ~1-1.5 MP proxy, cache the maps (illumination gain, levels, threshold surface), and re-run only cheap per-pixel stages at preview size. Full-res runs once, on export. CPU only: rayon + fast_image_resize + own kernels. The enhancer receives the crop quad as a validity mask so table and background pixels are ignored. All numbers below are starting values, not validated on receipts.

**Measured (scratch, naive std-only Rust, i7-8700K, synthetic 12 MP).** RGB to luma 4 ms; LUT on RGB 4 ms; Sauvola 31x31 full-res ~40 ms integral (1 thread) + 18-30 ms threshold (12 threads); proxy flatten ~55 ms. Budget: <=250 ms per 12 MP end-to-end, <=16 ms per slider tick.

**(a) Receipt photo, after crop/perspective**
1. Proxy: box-downscale to ~1200-1600 px long edge (~3 ms).
2. Illumination: per block (~1.5-2x text height, half-block stride) take the 85-90th-percentile luminance. Drop blocks under ~60% of the median (barcodes, black bands) and inpaint. Then 3x3 median, Gaussian sigma ~1 block, bilinear upsample. Per-channel gain = paper reference / map, clamped to <=3x. This removes soft shadows and white-balances the paper.
3. Tone: black point at the 0.5-1st percentile of flattened luma; white point at ~95% of paper level so paper clips to 255; auto gamma 0.8-1.0 (stronger for faded thermal).
4. Cleanup: Cb/Cr blur sigma ~2 px; luma 3x3 median or sigma-0.7 Gaussian only if the noise estimate is high. No NLM or bilateral by default.
5. Sharpen: unsharp sigma ~1 px at ~300 dpi scale, amount 0.5, threshold 3/255.
6. Modes: **Original**; **Auto** = steps 1-5 (colour kept); **Grayscale** = luma of Auto; **B&W** = Sauvola on flattened luma (window 31-51 px at 300 dpi, k~0.25, R=128, floor on std) then a +/-6-level soft ramp to anti-aliased 8-bit. "Faded" preset = NICK (k~-0.1). Optional keep colour marks: a chroma mask after white balance, with a minimum area, keeps stamps and logos.

**(b) Flatbed text scan**
1. Measure illumination-map variation. If under ~3% of paper level, skip flattening and apply histogram levels + gamma as one LUT (~4 ms).
2. Otherwise a mild large-block flatten for lid shading or binding shadow.
3. Show-through: raise the white point or gamma; B&W removes it.
4. Unsharp sigma ~0.8; denoise only if noisy.
5. B&W: Otsu on the flattened image (Leptonica's approach), or Sauvola if local variation remains. Despeckle only components <=3-4 px^2 at 300 dpi, and keep it off for receipts so decimal points survive. Preserve dpi metadata.

**Sliders (5):** Brightness (white point), Contrast (black point/gamma), Text darkness (Sauvola k or threshold bias), Clean-up (denoise + despeckle), Sharpness.
**Advanced:** shadow strength (block size, gain clamp), window size, CLAHE (luma only, clip ~2, 8x8, off by default), keep paper tint, keep colour marks, output bit depth.

**Output.** Default export is 8-bit (PNG or JPEG). 1-bit is opt-in: PNG 1-bit via the png crate; TIFF G4 via the fax encoder (tiff crate cannot write CCITT); PDF via pdf-writer + CCITTFaxDecode. JBIG2 is generic-region lossless only, never symbol mode. Sizes at A4 300 dpi: raw 1-bit 1.09 MB; G4 ~9 KB for a clean page but many times more if speckled; JBIG2 lossless 3-5x smaller than G4; 8-bit grey uncompressed ~8 MB.

**Optional or later:** ML shadow removal on a 768x1024 proxy, using output/input as a full-res gain map; NLM; CLAHE; Wolf and Su variants.

**Evaluation.** DIBCO/H-DIBCO via Doxa BinBench (F-measure, pseudo-F, PSNR, DRD); SmartDoc-QA receipts and CORD with Tesseract CER (thresholding fixed); SD7K, Jung and Kligler for shadows. Gate: enhanced CER no worse than the original on >=95% of images, and no drop in decimal-point accuracy.

## Decision-critical claims (as researched)
- imageproc 0.27's contrast module offers only adaptive_threshold, Otsu, equalize_histogram, stretch_contrast, threshold, kapur_level and match_histogram, and adaptive_threshold is a box-mean threshold over an integral image. There is no Sauvola, NICK or CLAHE, so those must be written. [https://docs.rs/imageproc/latest/imageproc/contrast/index.html]
- Integral-image Sauvola computes the same thresholds as the naive algorithm at about 5x (15x15 window) to 20x (40x40 window) speed-up. Squared-intensity integral images overflow 32-bit integers, so 64-bit is required. [https://www-live.dfki.de/fileadmin/user_upload/import/2676_FsDkTmbEfficientImplSpie2008.pdf]
- Leptonica's recommended approach for uneven backgrounds is locally adaptive background normalisation (background mapped to a constant such as 200) followed by a global threshold, with Sauvola as a working alternative. Leptonica is BSD 2-Clause. [https://tpgit.github.io/UnOfficialLeptDocs/leptonica/binarization.html]
- Bako et al. estimate local background per block, form a per-pixel gain against a global reference on a stride-subsampled map, smooth and upsample it. They report it working on receipts and menus, and state limitations for varying paper colour and hard shadow edges. [https://web.ece.ucsb.edu/~psen/Papers/ACCV16_RemovingShadows.pdf]
- DocEnTr's LICENSE is Creative Commons Attribution-NonCommercial 4.0; DE-GAN is GPL-3.0 with a note limiting use to academic research; DocDiff, DocRes and DocShadow-SD7K are MIT. [https://raw.githubusercontent.com/dali92002/DocEnTR/main/LICENSE]
- On H-DIBCO 2018 plain Otsu gets F-measure 77.73 and Sauvola 77.11, versus 92.53 for DocEnTr-Base (68M parameters, 256x256 patches). This is historical manuscript data, not receipts. [https://arxiv.org/pdf/2201.10252]
- JBIG2 lossless is typically 3-5x smaller than G4, and JBIG2 symbol-mode pattern matching caused the 2013 Xerox character-substitution bug. The tiff crate lists no CCITT encoder; the fax crate (MIT) encodes and decodes Group 4. [https://en.wikipedia.org/wiki/JBIG2]
- Tesseract 5 has built-in adaptive Otsu and Sauvola thresholding, with global Otsu as the default. OCR-CER evaluation must therefore fix the thresholding setting. [https://tesseract-ocr.github.io/tessdoc/ImproveQuality.html]

## Researcher questions for user
- What should the default output of 'Auto' be for a coloured document such as a receipt with a logo or stamp? — It sets the main pipeline branch and how many people see 'washed-out' versus faithful results. (default: Colour-preserving, paper whitened; B&W is one tap away)
- Should 'Black & white' output be hard 1-bit or anti-aliased 8-bit by default? — 1-bit is smaller but jagged and can lose faint print; 8-bit anti-aliased looks better and preserves small marks like decimal points. (default: Anti-aliased 8-bit, with 1-bit as an export option)
- Is ML-based enhancement in scope for the first releases? — It affects packaging size (about 120 MB or more per model), the ONNX runtime dependency, and licence choices. (default: No ML in v1; design an on-demand model slot)
- Should the app export multi-page PDFs and bilevel scan formats (TIFF G4, PDF/G4, JBIG2), or only images? — It determines whether PDF and CCITT/JBIG2 encoders are on the roadmap. (default: Images plus TIFF G4 and PDF/G4 later; JBIG2 optional)
- Should enhancement be applied automatically on import, or only when the user picks a mode? — It shapes the non-destructive edit model, preview speed, and how much the app can change scans without the user noticing. (default: Auto-suggest with live preview; user confirms; originals never modified)

## INDEPENDENT VERIFICATION (skeptic) — overrides the researcher where they differ
- [CONFIRMED] 1. imageproc 0.27 contrast module has only adaptive_threshold, Otsu, equalize_histogram, stretch_contrast, threshold, kapur_level, match_histogram. adaptive_threshold is a box-mean threshold over an integral image. No Sauvola, NICK or CLAHE.
  CORRECTION: Function list matches docs.rs for v0.27.0, which is the newest release on crates.io (2 June 2026, MIT). adaptive_threshold builds a u32 integral image and compares each pixel with the block mean minus a delta parameter, so it is a Bradley-style mean threshold. Nuances: (a) imageproc has integral_image, integral_squared_image and a variance helper, but variance and sum_image_pixels accept only u32 images. u32 squared sums overflow above about 66k pixels, and u32 plain sums overflow above about 16.8 MP (2^32/255), so 48 MP and larger phone photos break. Sauvola needs your own u64 code. (b) A third-party MIT crate `clahe` 0.1.3 exists (see other errors), so 'must be written' is not strictly true for CLAHE.
- [CONFIRMED] 2. Integral-image Sauvola gives the same thresholds as the naive algorithm at about 5x (15x15 window) to 20x (40x40 window) speed-up. Squared-intensity integral images overflow 32-bit integers, so 64-bit is required.
  CORRECTION: I read the PDF text. It reports the same thresholds as Sauvola. Naive took 12.6 s at 15x15 and 65.5 s at 40x40 against 2.8 s for the integral version, on 2530x3300 UW-1 pages, 2.4 GHz Opteron, 2008. So 5x and 20x hold, but they are against naive Sauvola on old hardware. Minor overstatement: the paper says overflow 'might occur' with 32-bit and does not say 64-bit is required. Modular u32 arithmetic can work when each window sum fits in 32 bits, and a 51x51 window of squares is about 1.7e8. u64 is still the safe choice. Squared u32 integrals overflow beyond about 66k pixels. I re-ran a std-only Rust 12 MP Sauvola on this machine and got about 74 ms total, close to the researcher's figure.
- [CONFIRMED] 3. Leptonica recommends locally adaptive background normalisation (background mapped to a constant such as 200) then a global threshold, with Sauvola as a working alternative. Leptonica is BSD 2-Clause.
  CORRECTION: The docs say the normalised background is put at a constant like 150 or 200, and a global threshold is then set below it. They name pixOtsuThreshOnBackgroundNorm, pixMaskedThreshOnBackgroundNorm and pixBackgroundNormFlex. Sauvola 'does quite well' and is tiled for efficiency. The cited page is an unofficial mirror of Bloomberg's documentation under CC BY 3.0. The code licence is BSD 2-Clause: leptonica-license.txt has exactly two conditions. GitHub's API shows NOASSERTION only because the licence is a custom file. Last push 2 Sept 2026, not archived.
- [PARTLY-TRUE] 4. Bako et al. estimate local background per block, form a per-pixel gain against a global reference on a stride-subsampled map, then smooth and upsample it. They report it working on receipts and menus and state limitations for varying paper colour and hard shadow edges.
  CORRECTION: Mechanism and limitations are correct. Details: the local background is the higher-mean cluster of a 3-mean GMM/EM fit on 150 sampled pixels per 21x21 block, not a percentile. The stride is 20. Smoothing is a 3x3 median then Gaussian sigma 2.5, upsampled with an 8x8 Lanczos filter. The global reference comes from whole-image clustering. Limitations confirmed: changing background colour, intense hard-shadow boundaries, and bias from figures or pictures on the page. Overstated: 'receipts' appear only in the introduction and conclusion as target content. Evaluation used 81 controlled DSLR images of 11 documents plus 16 Flickr images, including a menu. The credits list no receipt. Receipt validation is therefore unshown, and the summary's 'validated on receipts' is too strong.
- [CONFIRMED] 5. DocEnTr licence is CC BY-NC 4.0; DE-GAN is GPL-3.0 with an academic-only note; DocDiff, DocRes and DocShadow-SD7K are MIT.
  CORRECTION: Verified from the LICENSE files and READMEs. DocEnTr is CC BY-NC 4.0. DE-GAN has GPL-3.0 text, and its README says academic research use only, with commercial use by contacting the author. DocDiff, DocRes and DocShadow-SD7K are MIT. Push dates also match: DocEnTr 2025-01-17, DocRes 2025-08-03, DocDiff 2024-08-22, DE-GAN 2023-03-24. DocShadow-SD7K is archived, last push 2024-06-18. MIT covers code only. The repositories state nothing about weights or training-data licences, so treat pretrained weights as unlicensed until checked.
- [REFUTED] 6. On H-DIBCO 2018 plain Otsu gets F-measure 77.73 and Sauvola 77.11, versus 92.53 for DocEnTr-Base (68M parameters, 256x256 patches). Historical manuscript data, not receipts.
  CORRECTION: The numbers are real but belong to Table VI, DIBCO 2017, not H-DIBCO 2018. Table VII (H-DIBCO 2018) gives Otsu 51.45, Sauvola 67.81, DocEnTr-Base{8} 90.59, Base{16} 89.97 and Large{16} 89.21. The best method there is Jemni et al. (cGAN) at 92.41, and the paper says DocEnTr ranks only second. The 68M parameters and 256x256 patch size are correct. The direction of the conclusion (ML far ahead of classical thresholding on degraded historical pages) still holds, and is stronger on 2018. The classical baselines are untuned, and the models train on the other DIBCO years, so the gap says little about tuned pipelines on receipts. Fix the dataset label in the writeup.
- [PARTLY-TRUE] 7. JBIG2 lossless is typically 3-5x smaller than G4, and symbol-mode pattern matching caused the 2013 Xerox bug. The tiff crate lists no CCITT encoder; the fax crate (MIT) encodes and decodes Group 4.
  CORRECTION: Confirmed: the Xerox defect (2013) was lossy pattern matching, and Xerox patched it in August 2013 by disabling it. The tiff README lists Fax4 as decode-only ('not yet' for encode; latest release 0.11.3). The fax crate is MIT, version 0.3.0 (13 July 2026), with a Group 4 encoder and decoder. Doubtful: the '3-5x' figure is a JBIG committee press release quoted on Wikipedia, not independent data. jbig2enc's own numbers for 90 book pages show generic-region coding at 3.44 MB, lossy symbol coding at 1.08 MB, and lossless symbol coding with refinement at 3.38 MB, about the same as generic. So the recommended generic-region-only mode has no evidence of 3-5x over G4. I found no primary source for its true ratio. Measure it before quoting any figure.
- [CONFIRMED] 8. Tesseract 5 has built-in adaptive Otsu and Sauvola thresholding, with global Otsu as default. OCR-CER evaluation must therefore fix the thresholding setting.
  CORRECTION: Tesseract 5.0.0 added Leptonica-based adaptive Otsu and Sauvola. In the source, thresholding_method is 0 = Otsu (default), 1 = LeptonicaOtsu, 2 = Sauvola, with Sauvola window 0.33 x DPI and k 0.34. The latest release is 5.5.3 (24 July 2026). Nuance: LSTM line recognition takes BestPix (original or grey), not the binarised image. The setting mainly affects layout analysis and segmentation, but fixing it is still right. The Tesseract Sauvola window (about 100 px at 300 dpi) is far larger than the researcher's 31-51 px, so window choice deserves an explicit sweep.

### Other errors spotted by skeptic
- The CLAHE row says 'No Rust crate found'. That is stale: crate `clahe` 0.1.3 (MIT, micahcc/clahe-rs, pure Rust, updated July 2026, about 5k downloads) exists. It is immature but usable as a reference. `purecv` (pure-Rust OpenCV port) is LGPL-2.1-or-later, so avoid statically linking it into a permissive app. `bgustreadimg` (Sauvola for OCR) has 157 downloads and a non-standard licence.
- fax::tiff::wrap hardcodes 200 dpi X/Y resolution, PhotometricInterpretation WhiteIsZero and a single strip. The plan to write TIFF G4 via fax while 'preserving dpi metadata' therefore needs your own small TIFF writer or an upstream patch.
- 'G4 ~9 KB for a clean A4 300 dpi page' looks optimistic. A synthetic dense text page (2481x3507, ImageMagick) encoded to G4 at about 30 KB. Sparse receipts will be much smaller. The figure is unverified either way; use a range, not 9 KB.
- The deliverable is internally inconsistent: it quotes 'JBIG2 lossless 3-5x smaller than G4' but recommends generic-region-only JBIG2. Per jbig2enc's own numbers, the 3x gains come from lossy symbol mode.
- Memory note on the fast Sauvola: two u64 integral images at 12 MP take about 192 MB, and 48 MP+ inputs about 770 MB. Plan strip-based or tiled integrals, or wrapping u32 per strip, rather than a full-image u64 pair. imageproc's u32 helpers cannot serve full-res Sauvola.
- Benchmark reproduced within tolerance: my std-only Rust run at 12 MP, 31x31 window, took 53 ms for the integral (1 thread) plus 21 ms for the threshold (12 threads). This CPU is not the researcher's i7-8700K, and the image is synthetic, so treat the figure as order-of-magnitude.
- SmartDoc-QA does include receipts, but only as one of three document types (modern documents, old administrative letters, receipts). It has text-transcription ground truth for CER, is CC-BY-4.0 on Zenodo, and is a single roughly 13 GB zip, so plan for filtering. CORD is CC-BY-4.0 (Hugging Face).
- Minor: LP-IOANet has no official code, but an unlicensed third-party reimplementation exists (PranavAga/LP-IOANet, 2024). The DocShadow ONNX exports are 114 MiB each (matches 'about 120 MB'). The exporting repo (fabio-sim, MIT) has been stale since Sept 2023. Version claims checked and correct: wgpu 30.0.1 (22 Aug 2026), ort 2.0.0-rc.13, tract-onnx 0.23.8, opencv-rust 0.101.0 (MIT, 26 Sept 2026), png 0.18.1, pdf-writer 0.15.0, Doxa CC0 (pushed 17 July 2026), jbig2enc pushed 1 Sept 2026 (Apache-2.0).

## Sources
- https://docs.rs/imageproc/latest/imageproc/contrast/index.html
- https://www-live.dfki.de/fileadmin/user_upload/import/2676_FsDkTmbEfficientImplSpie2008.pdf
- https://tpgit.github.io/UnOfficialLeptDocs/leptonica/binarization.html
- https://raw.githubusercontent.com/DanBloomberg/leptonica/master/src/adaptmap.c
- https://web.ece.ucsb.edu/~psen/Papers/ACCV16_RemovingShadows.pdf
- https://helios2.mi.parisdescartes.fr/~vincent/articles/DRR_nick_binarization_09.pdf
- https://arxiv.org/pdf/2201.10252
- https://github.com/brandonmpetty/Doxa
- https://en.wikipedia.org/wiki/JBIG2
- https://tesseract-ocr.github.io/tessdoc/ImproveQuality.html
- https://github.com/fabio-sim/DocShadow-ONNX-TensorRT
- https://zenodo.org/records/5293201