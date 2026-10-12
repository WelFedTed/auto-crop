# Enhancement: measured (M7 engine slice)

Roadmap items touched: M7.02 to M7.12, M7.14 (partly), M7.15, M7.19, M7.50, M7.55 (synthetic part), M7.60, M7.61, M7.63 (partly), M7.67, M7.71 (partly); see the end for what is **not** measured. Code: `crates/imgproc/src/enhance`, harness `tools/ocr_oracle` and `.github/workflows/ocr-oracle.yml`, benches `crates/imgproc/tests/enhance_memory.rs`. Design and PROVISIONAL numbers: [PLAN 5](../plan/05-enhancement.md). API: [dev/enhance-engine.md](../dev/enhance-engine.md).

## Read this first

| | |
|---|---|
| Inputs | **Synthetic**, never photographs: the pages of `imgproc::enhance::testpages` (a 5 x 7 bitmap font at integer scale, blurred like a camera, price columns, barcode, black band, coloured stamp; lit by a known field plus noise) and, for the OCR matrix, `tools/ocr_oracle` receipts (DejaVu Sans Mono) and prose documents (DejaVu Sans) degraded by recipes written independently of the enhancer (Pillow only). They are stand-ins for the private receipt slice (B21, M7.54), which is not used here and does not exist yet in this repository. **No real receipt has been seen.** |
| OCR | Tesseract 5.5.1 from `ppa:alex-p/tesseract-ocr5` on `ubuntu-22.04`, `eng`, `--oem 1 --psm 6 --dpi 300`, one OpenMP thread, `thresholding_method` 0 (Otsu, the acceptance setting) and 2 (Sauvola), `eng.traineddata` sha256 `7d4322bd...170b2`. Tesseract is **not installed on the dev machine**: every OCR number below was read from a GitHub Actions log. It is a dev-only proxy (OCR is a non-goal, B19) that favours hard binarisation and cannot see ghosting. |
| Timing host | **LUNCHBOX, desktop, indicative** ([hardware.md](hardware.md)): i7-8700K (6C/12T, AVX2), Windows 11, release build, **NOISY**: other agents were compiling (`rustc`) and the owner's media jobs run on this machine; the global CPU counter read 56% to 100% during the runs, so every millisecond is pessimistic by an unknown, varying factor. Nothing here claims a hardware-tier gate (M1.61 is not claimed). |
| What to trust | Heap peaks (exact), the stage ratios of one run, the OCR tables (deterministic), the determinism and oracle results. Not the absolute milliseconds. |

## Timing, 12 MP (2928 x 4099, shaded text page with a barcode, band and stamp), all 12 threads, NOISY

`cargo test --release -p auto-crop-imgproc --test enhance_memory -- --ignored --nocapture`, best of 5 (the stage columns are of the last run, so they do not add to the best). Text height 63 px, threshold window 151 px.

| Mode | best of 5 | analysis | apply | proxy | text scale | illumination | 2nd round | tone | flatten pass | threshold pass |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Clean | 72 ms | 60 | 18 | 15 | 11 | 18 | 16 | 0.7 | 18 | - |
| Document | 139 ms | 75 | 88 | 33 | 12 | 15 | 13 | 0.9 | 22 | 66 |
| Receipt | 120 ms | 52 | 69 | 13 | 10 | 14 | 13 | 1.0 | 23 | 47 |

Against the plan: `enhance` at 12 MP is budgeted at **120 ms** (Auto, Grayscale: Clean here) and **up to 250 ms** (B&W, Faded: Document and Receipt here). Clean sits at 0.6x, Document at 1.2x and Receipt at 1.0x of the 120 ms line and all three under the 250 ms line **on a loaded machine, NOISY**; the Document row of an earlier run on the same machine read 188 ms (before the text-scale shortcut). The plan's **15 ms** for the percentile grid on the 1.5 MP proxy is **not met as a single stage**: the illumination estimate takes 14 to 18 ms (first round) and the second round about the same, plus 10 to 12 ms for the text scale and 13 to 33 ms for the area-average proxy from 12 MP (`scale::resize_area`, scalar `f32`). Everything is scalar Rust (no SIMD, M7.13 is not built), so these numbers have headroom that has not been taken. The 16 ms slider tick on a 2 MP proxy is **not met**: a preview request re-runs the analysis (about 30 to 60 ms at 2 MP here); the `analyse` / `apply` split exists so a stage cache (M7.02) can be added.

On 1.4 MP pages (the size of the preview) one `enhance` call takes 25 to 45 ms in the same noisy conditions.

## Memory (exact, load-independent)

12 MP, 8 threads: extra peak heap **16.7 MB for Clean and 16.7 MB for Document** (the proxy, its working copies and the grids; the flattened luma plane and the result plane are allocated after the input RGB buffer is freed, so they do not raise the peak above the analysis's), **24.8 MB for Receipt at strength 0.5** (the input stays alive to blend). A full-frame `f32` copy of the RGB image would be 144 MB. One thread and eight threads peak within 13 KB of each other (`the_peak_does_not_grow_with_the_thread_count_beyond_the_band_scratch`). `Engine::save_weight` adds `2 B/px` of output for the grey modes plus the proxy to the admission weight.

## Determinism and oracles (all in `cargo test`)

* Bytes identical at 1 and 8 threads for Clean, Document and Receipt, at strength 1 and 0.6 (`the_bytes_do_not_depend_on_the_thread_count`), and the strip kernel's own tests (`threshold.rs`).
* Residual shading after Clean (worst 32 x 32 cell of the paper, spike definition), synthetic 1000 x 1400 page: tilt 55% **1.0%**, vignette 45% **0.4%**, soft shadow 50% over 150 px **0.5%**, hard-edged shadow 40% over 60 px **under 8%** (PROVISIONAL bar of the spike: 3%; sweep in `sweep_flatten`: block factor 1.25, percentile 0.88, 2 rounds).
* No ghost beside a barcode, a black band or a stamp: the paper within 40 px of them stays within **0.6%** of the page level (bar 2%).
* Text strokes: ink recall at least **0.90** and paper cleanliness at least **0.995** in Document and Receipt on shadowed pages (measured 1.000 on every page tried); decimal points of price columns survive at glyph heights 14, 21, 28 and 42 px (150 to 300 dpi-equivalent) in both modes, **0 lost**.
* Text height found within 15% from 100 to 400 dpi-equivalent (glyph heights 14 to 56 px: +14%, +5%, +4%, 0%, +2%).
* Validity mask: a page on a dark table gets a paper estimate within 1.5 levels of the same page cropped tight; the pixels outside are white in a thresholded mode; engine test: a full-frame crop turned 6 degrees pulls black in from outside the source and the result's corners are white, not black ink.
* Degenerate inputs (0 x 0, 1 x 1, 3 x 200, blank, all black, noise, an 8:1 strip, an all-zero mask, strengths NaN, infinite, negative) never panic and keep the size.
* Cancellation: a token set before the call, and tokens that fire after 3, 10 and 40 polls, return `Cancelled` with no output.
* Scale: Clean at full size and at half size agree within a mean absolute difference of 4 levels; the ink coverage of Document differs by 21% (thin strokes do not agree between resolutions: the plan's limit, no tile renderer yet).
* A blank page is not amplified (mean luma moves by 5 levels from 250 to 255: the white point), a clean flat scan is flattened with a maximum gain of 1.06.
* Suggestion (`the_suggestion_is_precise_on_pages_and_silent_on_photographs`): **dev set 28 of 28 pages suggested, 0 of 36 other images** (8 procedural photograph kinds x 4 seeds, plus a flat crisp scan x 4); **held-out set (other seeds) 21 of 21 and 0 of 27**. The photographs are procedural (value-noise landscapes, foliage, interiors, brick walls, a poster wall, a night city, a portrait, tiles), not photographs.

## The OCR matrix: does enhancement help?

`.github/workflows/ocr-oracle.yml`, job `enhance`: 8 synthetic receipts (monospace, 32 columns, 34 px) and 6 prose documents (proportional font, 150 dpi), each degraded by 7 recipes (`tools/ocr_oracle/degrade.py`; every slice but `clean` is JPEG q85): `clean` (the control), `shading` (tilt and vignette, sigma-4 noise), `shadow` (a soft cast shadow over half the page), `fade` (thermal fade: ink at 30% of its contrast), `fade_shading`, `dim_noisy` (0.55 light, sigma-10 noise), `warm_lamp` (colour cast and tilt). Each image is enhanced by the pure-Rust enhancer and read by the same Tesseract; CER is the Levenshtein distance over the transcript after the oracle's normalisation, mean over the images of a slice. Cells are `mean CER % (change against original) w<images worse by more than 1 point> a<amount tokens the original read that were lost>`. A mode that makes a slice worse shows as a positive change.

### First run, `adedc85` (before the fixes below), Tesseract thresholding_method=0 (Otsu)

| slice | original | clean | document | receipt |
|---|---:|---:|---:|---:|
| doc-clean | 0.00 | 0.00 | 0.00 | 0.00 |
| doc-dim_noisy | 0.00 | 0.00 | 0.00 | 0.47 (+0.47) |
| doc-fade | 0.00 | 0.00 | **100.00 (+100.00) w6 a182** | 0.01 |
| doc-fade_shading | 33.59 | 0.00 (-33.59) | 0.01 | 0.00 |
| doc-shading | 19.70 | 0.00 (-19.70) | 0.00 | 0.01 |
| doc-shadow | 35.82 | 0.00 (-35.82) | 0.00 | 0.04 |
| doc-warm_lamp | 1.25 | 0.00 (-1.25) | 0.00 | 0.02 |
| receipt-clean | 0.24 | 0.09 (-0.15) | 0.00 (-0.24) | 0.04 (-0.20) |
| receipt-dim_noisy | 1.62 | 0.09 (-1.53) | 0.35 (-1.27) | 0.77 (-0.85) |
| receipt-fade | 3.72 | 0.00 (-3.72) | 1.96 (-1.76) w1 a2 | 0.08 (-3.64) |
| receipt-fade_shading | 29.35 | 0.00 (-29.35) | 0.17 (-29.18) | 0.27 (-29.08) |
| receipt-shading | 25.27 | 0.00 (-25.27) | 0.40 (-24.87) | 0.58 (-24.69) |
| receipt-shadow | 32.68 | 0.00 (-32.68) | 0.26 (-32.42) | 0.30 (-32.39) |
| receipt-warm_lamp | 2.51 | 0.14 (-2.37) | 0.09 (-2.42) | 0.38 (-2.13) |
| **mean over slices** | **13.27** | **0.02** | **7.37** (1 slice worse, 7 images worse) | **0.21** (1 slice worse, 0 images worse) |

With Tesseract's own Sauvola (method 2) the means were original 15.51, clean 0.06, document 7.39, receipt 0.32; the doc-fade row reads 100% for the original, 0.54% for clean, 100% for document and 0.02% for receipt.

What it says, and what it does not:

* **Enhancement helps a great deal on the synthetic conditions it is for.** Uneven light is the failure of Tesseract's global Otsu (original CER 20% to 36% on shaded and shadowed slices); flattening takes it to about 0%. Thermal fade makes the original unreadable to Tesseract's Sauvola (100%); every enhanced mode but one reads it. The clean synthetic slices are the control: nothing is made worse by Clean on them.
* **Clean is the winner: it is never worse than the original on any slice and its mean CER is 0.02%.** Document and Receipt, which binarise, are close behind on most slices but made some worse (Receipt on dim noisy documents: +0.47 at method 0, +1.48 with 5 images worse at method 2; Document on thermal-faded print: catastrophic, below). This is why the engine's suggestion is **Clean** and the black-and-white looks are the person's choice, and why `process --enhance auto` applies only Clean.
* **A failure the matrix found:** Document (Sauvola, k 0.25) lost faded thermal print entirely (doc-fade 100%, 182 amount tokens lost, receipt-fade 1.96% with 2 lost): the faded ink (182 at a paper of 250) sits above the Sauvola threshold (about 188). Receipt's contrast stretch before NICK reads it (0.01%). **Fix in the same commit:** Document takes the levels stretch when the analysis finds the print faded (ink contrast under 100 levels), and the "almost no ink" test is the noise floor rather than the plan's `white - 60` (which also skipped the stretch for faded print).
* **The blend (`strength` 0.5) is not an OCR input**: half-way images read worse than either end (Document at 0.5: mean CER 1.66%); strength is for the eye.
* The synthetic recipes are not photographs: no lens blur, glare, perspective, crumple, paper texture, reverse-video, real thermal fonts or JPEG chroma subsampling of a phone. The numbers say the algorithms do what they are built for on a controlled model of the conditions; they say nothing about the field accuracy (PLAN 5.10 asks for the private receipt slice and CORD/SmartDoc, which are not used here).

### Parameter sweep, same run, thresholding 0, mean CER % over the 14 slices (original 13.27)

| variant | mean CER | slices worse | images worse |
|---|---:|---:|---:|
| doc: window 2.0 / 3.5 x text height | 7.40 / 7.05 | 1 / 1 | 8 / 7 |
| doc: Sauvola k 0.34 | 14.58 | 2 | 14 |
| doc: NICK instead of Sauvola | 0.14 | 0 | 1 |
| doc: hard edges / ramp rho 16 | 7.62 / 0.34 | 2 / 1 | 12 / 6 |
| doc: strength 0.5 | 1.66 | 2 | 13 |
| receipt: window 2.0 / 3.5 | 0.17 / 0.21 | 1 / 1 | 0 / 1 |
| receipt: Sauvola instead of NICK | 0.12 | 0 | 0 |
| receipt: NICK k -0.2 | 0.06 | 0 | 0 |
| receipt: hard edges | 0.24 | 1 | 0 |
| clean: one flatten round instead of two | 0.06 | 0 | 0 |

(The document rows are dominated by the thermal-fade failure that was fixed afterwards; the rows that do not touch it are comparable.) Chosen from this: NICK k **-0.2** (PLAN: -0.1), soft ramp rho 8 (hard edges are worse), two flatten rounds, window 2.5 x text height. The second run after the fixes is the next section.

### After the fixes

(See the run linked from the commit that fills this section.)
