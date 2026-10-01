> Auto Crop design, part 7 of 8 | [PLAN.md](../../PLAN.md) | [Decision log](00-decision-log.md) | [ROADMAP.md](../../ROADMAP.md)  
> Planning draft, 2026-10-01. Numbers marked PROVISIONAL are unmeasured estimates; decisions B1-B21 and the assumptions A-1..A-12 live in the decision log and in section 1.

# 7. Performance, accuracy and quality engineering

"Very fast, very accurate" (brief item 4) is only enforceable as numbers a machine checks on every change. This section defines those numbers, the harness that measures them, the corpus behind it, and the gates that block a merge or a release. **Every unmeasured figure is PROVISIONAL**: the inputs are extrapolations from desktop CPUs (Ryzen 9 5950X/7950X, older i7s) or design targets, and nothing has been measured on a laptop, on Apple silicon or through Tauri IPC. Budgets are p50 unless a percentile is named. The harness exists before any GUI work (decision E).

**Reference hardware.** *Tier-M*: 6C/12T x86-64 AVX2 laptop (Ryzen 5 7640U or Core i5-1335U class) or Apple M1, 16 GB, NVMe, on AC power. *Tier-L*: the C1 floor (4 cores, 8 GB), with budgets 2x Tier-M. Windows Tier-M is the primary baseline because the first release is Windows-only (B9).

## 7.1 Performance budgets (all PROVISIONAL)

The research proposed 400 ms and 10 images/s, but its own parts (decode ~85 ms, encode ~100 ms, geometry 250-450 ms, enhancement 60-250 ms) already exceed 400 ms. Per decision D5, refined by worker count, the targets are **<= 700 ms p50 for one 12 MP image that exits at tier T1, end to end; >= 4 images/s from the CLI and >= 3 images/s in a GUI batch on 6 cores (worker counts below); `analyse` <= 40 ms p95 over T1 exits; and <= 150 ms p95 to first overlay.** Each figure names its population or worker count, because one pooled number cannot hold: T2 escalation (section 7.9) puts 10-15% of images in the tail.

**Table A. 12 MP JPEG, auto-process to JPEG at a fixed q90 (a benchmark setting; the shipped JPEG-source quality rule is in 3.3), overwrite with backup (B3), Tier-M, p50 for images that exit at tier T1 (section 7.9). All PROVISIONAL.**

| # | Stage (tracing span) | ms | Basis (unverified) |
|---|---|---:|---|
| 1 | `read_probe`: read, sniff magic bytes, probe dimensions, EXIF, ICC | 10 | NVMe assumed |
| 2 | `decode`: full JPEG decode | 120 | ~85 ms from an unsourced 6 MP figure, x1.4 for a laptop |
| 3 | `proxy`: 1024 px detection proxy and ~1.5 MP analysis proxy (by area, long edge <= 3072 px) | 25 | [fast_image_resize](https://github.com/Cykooz/fast_image_resize/blob/main/benchmarks-x86_64.md) 3.5-13.5 ms per 16 MP on a 5950X, x2 |
| 4 | `analyse`: corner net, classical cross-check, fuse, orientation, confidence | 40 | net 10-25 ms, orientation 3-10 ms, concurrent (estimates); 40 is also the p95 ceiling over T1 exits (G2) |
| 5 | `refine`: full-resolution sub-pixel line fits on four edges | 10 | estimate 3-10 ms |
| 6 | `rectify`: one composed homography, u8 Lanczos3 warp on rayon | 90 | estimate 30-200 ms; kernel gate <= 90 ms wall at 8 threads (7.7) |
| 7 | `enhance` (Auto or Grayscale): proxy illumination map, full-resolution gain, levels, unsharp | 120 | estimate 60-250 ms; B&W and Faded are the last variant below |
| 8 | `encode`: JPEG q90 | 110 | ~100 ms |
| 9 | `commit`: backup (hardlink, reflink or copy), temp write, fsync, Fast verify, rename | 100 | B3 path; excludes antivirus stalls |
| | scheduling, model warm-up, allocator, IPC jitter | 75 | slack |
| | **Total** (Auto or Grayscale enhancement) | **700** | |
| | Variant: crop-only (no line 7) | 580 | enhancement off, or suggested and not applied (B13) |
| | Variant: B&W or Faded print (line 7 up to 250) | <= 830 | Sauvola path (5.7); 700 - 120 + 250 |

- **Variants.** The 700 ms ceiling covers Auto or Grayscale enhancement; crop-only output (enhancement off, or suggested but not applied, B13) drops line 7 and takes 580 ms; B&W or Faded print, where line 7 runs up to 250 ms, is budgeted separately at 830 ms. The lossless fast path (90-degree turn or MCU-aligned crop, nothing else) is ~150-200 ms. Escalated images (tier T2, 10-15% of images) pay up to 250 ms more, so their p95 budget is 950 ms (1,080 ms for B&W or Faded).
- **Batch throughput.** CPU time per image is ~760 ms (decode 120, proxies 40, analysis 60, refine 10, warp 200, enhance 200, encode 110, commit 20); `commit` is I/O latency on its own threads. Throughput is workers x 70% (the research's assumed scaling efficiency) / CPU seconds, so every floor names its worker count. The CLI runs cores - 1 workers and a GUI batch leaves the preview pool its 2 to 3 threads (2 on 6 cores; pool sizes: section 2.8). On 6 cores the CLI has 5 workers, 3.5 effective cores and ~4.6 images/s, so its floor is **>= 4 images/s** (it tolerates ~875 CPU-ms per image); a GUI batch has 4 workers, 2.8 effective cores and ~3.7 images/s, so its floor is **>= 3 images/s** (~930 CPU-ms). The research's 5/s target is adopted for neither. While the queue holds at least as many jobs as workers, each job runs single-threaded; a lone job runs strip-parallel. The floors are measured on Auto output; a B&W or Faded batch is benchmarked on its own line.
- **HEIC to JPG.** The research target of >= 5 images/s is not adopted as a floor. The only 12 MP figure is for another decoder ([imazen/heic](https://github.com/imazen/heic): 451 ms sequential, 180 ms tile-parallel on a 7950X). Two to three times that on a laptop plus encode is ~1.1-1.5 CPU-seconds, which is 2.3-3.2 images/s on the CLI's 3.5 effective cores, so **>= 2.5 images/s** (CLI, 6 cores) is the floor, which sits inside the estimate and is therefore at risk, and 5 images/s is a non-gating stretch. The split between workers and libheif threads is grid-searched, and a measured shortfall is recorded, never hidden; libheif, WIC and ImageIO are unmeasured.

**Table B. Scaling with image size (JPEG, Tier-M).**

| Metric | 12 MP | 48 MP | 100 MP |
|---|---:|---:|---:|
| First paint (scaled decode 1/4-1/8) | <= 60 ms | <= 120 ms | <= 250 ms |
| First overlay (p95) | <= 150 ms | <= 250 ms | <= 450 ms |
| Full auto-process, p50 (Auto or crop-only; B&W or Faded adds up to 130 ms at 12 MP) | <= 700 ms | <= 2.5 s | <= 5.0 s |
| Batch throughput, CLI (5 workers on 6 cores) | >= 4 /s | >= 1.0 /s | >= 0.45 /s (memory-bound) |
| Batch throughput, GUI (4 workers beside the preview pool) | >= 3 /s | >= 0.75 /s | >= 0.35 /s (memory-bound) |
| Peak RSS per in-flight image (3x RGB8 + 64 MB) | <= 175 MB | <= 500 MB | <= 1.0 GB |
| HEIC draft (thumbnail) / final overlay | <= 150 ms / <= 700 ms | <= 400 ms / <= 3 s | not budgeted |

The 48 and 100 MP latency rows scale the size-dependent stages (~575 ms at 12 MP) by 4x and 8.3x and keep the ~125 ms fixed part; the throughput rows scale the 12 MP floors by the same pixel ratios. The 12 MP first overlay is read 10 + 1/4-scale decode 35 + proxy 5 + analysis 40 + webview delivery 25 (a placeholder until the spike measures Tauri) = 115 ms, plus 35 ms slack. It is the T1 result shown progressively; T2 and full-resolution refinement replace it later (2.4).

**Interactive budgets (Tier-M).** Cold start to usable UI <= 1.0 s (models load lazily). Corner drag, pinch and rotate: frame time median <= 17 ms and p95 <= 20 ms (about 60 fps); this replaces the research's 8 ms GPU homography, because the webview applies a geometry-only matrix to the loaded proxy with no IPC (D1). Release to final render: p50 <= 150 ms and p95 <= 250 ms, then a cross-fade of about 150 ms. Enhance slider tick <= 16 ms compute on the 2 MP display proxy, ~50 ms round trip. Undo and redo <= 50 ms. Handles are 48 px nominal and never below 44 px; every other control is >= 24 px. Tier-L doubles every time figure. Nothing goes in the README before Table A is measured.

## 7.2 Benchmark and accuracy harness (built before any GUI work)

Build order: `core` and `cli`, then the harness, then the Tauri shell. The harness is a `benches/` directory, an `auto-crop-eval` crate that runs the engine over a manifest and writes metrics JSON, a seeded Python generator (`tools/synth/`), and `xtask` commands (`fetch-corpus`, `synth`, `make-bench-images`, `eval`, `calibrate`).

| Tool | Role | Caveats |
|---|---|---|
| criterion 0.8.2 | wall-clock trends | too noisy on shared runners to gate |
| [gungraun](https://github.com/gungraun/gungraun) 0.20.0 (successor to iai-callgrind) | instruction counts under Callgrind: **the CI gate** | Linux only (not Windows; Valgrind does not work on current Apple silicon). Single-threaded, so it misses memory-bandwidth and threading effects. Benchmark each SIMD level (scalar, SSE4.1, AVX2). Running it under CodSpeed is unverified |
| hyperfine | end-to-end CLI timing (`--warmup 3 --runs 20`, JSON), cold and warm, on 12/48/100 MP sets | flags illustrative |
| Nightly wall-clock | p50/p95 per stage | the maintainer's Tier-M laptop as a self-hosted runner of the **private** repo only (on the public repo it would run fork PR code), running only binaries built on a hosted runner (the two-stage rule of 7.5); AC power, >= 20 samples, discard runs with variation above 5%; $0 (B15) |
| Profilers | diagnosis | Windows: samply, Windows Performance Recorder. macOS: Instruments. Linux: perf, cargo-flamegraph, heaptrack, DHAT. Tracy via `tracing` |

Rules: `tracing` span names equal Table A's, and every eval result carries per-stage timings; benchmarks use the shipped release profile (LTO, one codegen unit, no `target-cpu=native`); fixed inputs (12/24/48/100 MP JPEG at q85-95, 4:2:0 and 4:4:4, baseline and progressive, plus 12 and 48 MP HEIC) come from `xtask make-bench-images` because decode time depends on entropy; outputs are byte-identical at 1 and 8 threads.

**Gate G0 (M1, section 7.11)** needs Table A spans measured on Windows Tier-M (the analysis line on labelled stand-ins until the real detectors land), Tier-M and Apple-silicon baselines recorded, the public suite in CI, the dashboard live, and private golden v0 evaluated once. A Table A line more than 1.5x over budget forces a redesign or a documented budget change here.

**Tier-M spike: what each measurement replaces.** Full decode (turbojpeg against zune-jpeg); fast_image_resize on a laptop; ort against rten and tract; libheif, WIC and ImageIO on real iPhone and Android files; Tauri IPC and custom-URI latency on a 500-image folder; kornia-imgproc's f32 Lanczos warp against our strip-wise u8 warp; Sauvola with u64 strip integrals.

## 7.3 Speed techniques, ranked by expected payoff

The ranking is a judgement against Table A and is itself unmeasured; re-rank after the spike.

| Rank | Technique | Expected payoff and caveats |
|---:|---|---|
| 1 | **Proxy-first analysis:** detect on <= 1024 px, illumination on ~1.5 MP, full-resolution work only along four edges | analysis independent of megapixel count (12x fewer pixels at 12 MP, ~100x at 100 MP) |
| 2 | **Workload-matched parallelism:** batch = one image per worker, single-threaded stages while queued jobs >= workers; a lone job and interactive renders = strip-parallel; preview pool (2-3 threads) and batch pool (GUI: physical cores minus the preview pool; CLI: cores - 1); fsync on I/O threads | ~3.5 effective cores from 5 CLI workers and ~2.8 from 4 GUI workers (70% efficiency assumed); never nest both levels |
| 3 | **Lossless JPEG path** (turbojpeg `Transform`: 90-degree turns, flips, MCU-aligned crops) | ~150-200 ms and no generation loss, which matters because B3 overwrites. Crop origin snaps to the 8/16 px grid and may grow, so the UI says so ([jpegtran](https://github.com/libjpeg-turbo/libjpeg-turbo/blob/main/doc/jpegtran.1)). Pin libjpeg-turbo >= 3.1.4 (a `tj3Transform` double-free fix; vendored 3.1.0 is stale); CI checks it |
| 4 | **SIMD resampling** (fast_image_resize 6.1) | 16 MP to 852 px in 3.55 ms bilinear, 13.5 ms Lanczos3, against 80 and 176 ms for `image` (~13x); single thread, 5950X, stamped 6.0.1. Benchmark pic-scale too |
| 5 | **Scaled JPEG decode** (turbojpeg 1/2 to 1/8) for first paint, thumbnails, proxies | 3-5x, not 8x, because Huffman decoding remains. zune-jpeg has no DCT scaling. In batch, one full decode plus SIMD resize probably beats scaled plus full decode; the spike decides |
| 6 | **Fewer passes and copies:** one composed homography means one Lanczos pass; RGB8 not RGBA8; in-place LUT and gain; scratch pool; no mmap; per-strip warp, enhance and encode where the encoder takes rows | 10-25% on memory-bound stages (a guess); holds RSS at the 3x ceiling |
| 7 | **Thumbnail-first HEIC:** embedded thumbnail for the T0 draft, full decode in a background worker, libheif built with parallel tile decoding (CI asserts the flag) | perceived latency only; thumbnails may not match the primary image, so they never feed detection. Benchmark OS decoders behind `HeicBackend` |
| 8 | **Warm inference:** ONNX session created off the critical path, 1-2 threads in batch, int8 only after a per-platform accuracy check (gate in 7.7) | avoids a model load per batch |

Not doing in v1: wgpu compute, OpenCV in shipped builds, mmap, `image`'s resizer.

## 7.4 Accuracy metrics and acceptance thresholds

### Silent failure: definition and measurement

B6 targets at most 1% silent failures (a bad result auto-accepted) with about 8-10% flagged. An auto-accepted result is a **silent failure** if, against ground truth:

- quad IoU after canonical warp is below **0.90** (the silent-failure line; the SmartDoc "success >= 0.945" figure has no primary source and is not used). IoU >= 0.95 and >= 0.98 are reported as success levels, not failure lines, and 0.95 is also the A-versus-B detector agreement bar (4.3.3);
- ink or text is clipped by more than 2 px;
- the output is not upright, or residual skew exceeds 1.0 degrees;
- a no-document image was cropped, or an item split had the wrong count or an item with IoU below 0.9.

Enhancement is suggested, not forced (B13), so it has its own damage metric below.

**Measurement.** The eval binary records state (Good, Check, Failed), confidence and outcome per image at each operating point. **Risk**, the headline, is silent failures divided by auto-accepted images. Also reported: silent failures over all images (always lower, so the gate satisfies both readings of B6), the flag rate, and the share of flagged images that were fine. A Failed or flagged image must leave its original byte-identical (B4), checked by hash and mtime.

**Statistics.** A golden set cannot prove 1% exactly, so gates use the point estimate and a Clopper-Pearson one-sided 95% upper bound: 6 failures among 741 auto-accepted images is 0.81% with a bound of ~1.6%. The gate is point <= 1.0% and bound <= 2.0%. At ~740 accepted images that tolerates at most 7 failures (0.95%, bound ~1.8%); at ~450 (golden v1) it tolerates at most 3, because the bound binds (4 failures give 2.02%); even with zero failures the bound reaches 2.0% only from 149 accepted images, so v0 (~135 accepted) can only measure. Per slice with n >= 80 images, **at most 1 silent failure**; slices with 30 <= n < 80 are advisory; slices with n < 30 are suppressed from every public artefact. Synthetic data only detects regressions and never satisfies B6.

**Operating points and bands.** Strict, Balanced and Aggressive thresholds are *derived from the measured risk-coverage curve*, not fixed. Starting targets (PROVISIONAL): **Strict** <= 0.3% (~15-20% flagged); **Balanced** <= 1% with ~8-10% flagged (B6); **Aggressive** <= 3% (~3-5% flagged). Every mode uses the same three bands: Good is a calibrated score s >= t(mode); Check is 0.60 <= s < t(mode); Failed is s < 0.60 (the floor in every mode) or a hard gate such as implausible geometry. Good is auto-saved after the whole batch is triaged (Assumption A-3); Check is held, listed first, original untouched; Failed leaves the original untouched with the "Draw crop" banner. The UI shows an icon plus a word, never a raw score. The cutoffs t = 0.95 / 0.90 / 0.80 (Strict / Balanced / Aggressive) are interim values only until `calibration.json` exists (M2.18, M2.43). 0.9 is a research start value, not the Balanced cutoff: a calibrated 0.9 means a 10% chance of being wrong, so a mean risk <= 1% usually needs a higher threshold. `calibration.json` is fitted on the unlocked real dev tier plus SmartDoc, MIDV and CORD, never on synthetic data, and the locked golden set confirms it once per release candidate.

**Default strictness (Assumption A-3, section 1.7).** Previews default to Strict and label Balanced experimental until the golden gate passes; B6 stays the target. Balanced is the unlabelled default only if the locked golden set shows point <= 1.0% and bound <= 2.0% (G2); otherwise Strict is the default and Balanced is labelled experimental. Only silent-failure evidence demotes Balanced; a Balanced flag rate above 10% (15% at v0.3.0) is a stage-gate note, not a demotion. The bar is never lowered to make Balanced pass.

### Per-sub-task thresholds (all PROVISIONAL)

| Sub-task | Metric | Acceptance |
|---|---|---|
| Quad | SmartDoc protocol: both quads projected into a reference frame, Jaccard | mean >= 0.985; failure (< 0.90) <= 2% on realistic slices, <= 6% on the hardest |
| Corners | error as % of image diagonal; pixels after refine | golden set, before refine: median <= 0.15%, p95 <= 0.5% of the diagonal. After refine, at 12 MP: median <= 2 px, p95 <= 5 px (synthetic only; annotator noise may exceed 2 px) |
| Skew | absolute angle error; false-apply rate | text pages: median <= 0.1 deg, p95 <= 0.25 deg; photos: p95 <= 0.5 deg; applied when true angle < 0.1 deg: <= 1% |
| Orientation | top-1 over 0/90/180/270; selective accuracy | >= 99%; >= 99.5% at 95% coverage. EXIF/HEIF turn exact for 8 orientations x JPEG, TIFF, HEIC (no double-apply). PP-LCNet's 99.06% (1,000 ID/document images) is not evidence here |
| Crop (B14) | IoU against ground-truth box; clipping rate | mean >= 0.97, p5 >= 0.92; clipping <= 0.5% of images |
| Multi-item split | count exact match; precision and recall at IoU >= 0.9 | separated: count >= 97%, recall >= 95%, precision >= 97%; touching or overlapping: >= 90% routed to review |
| Dewarp (B10) | Tesseract CER against the perspective-only render of the same page; flat-page do-no-harm | median relative CER cut >= 20% on the curved slice; worse by more than 1 CER point on <= 2% of images (reported with n and a Clopper-Pearson interval); >= 95% of flat pages skipped, skipped pages byte-identical to the perspective-only output, and no non-skipped flat page more than 1 pt worse. UVDoc's published 0.172 on DocUNet comes from another OCR setup and is context only |
| Enhancement damage | Tesseract 5 CER before vs after; amount-token exact match (amounts with decimal points), paired per image | median CER change <= 0; > 1 pt worse on <= 2% of images; >= 20% relative CER cut on the low-contrast slice; amount tokens: zero lost on the synthetic fade and price-column suite (a loss blocks that mode as a default), and on private slices the one-sided 95% bound of the mean paired difference is no worse than -0.5 pt with every lost-token receipt listed (n < 30 is advisory) |
| Binarisation | F-measure, pseudo-F, PSNR, DRD via [Doxa BinBench](https://github.com/brandonmpetty/Doxa) (CC0); synthetic renders | no regression > 1 pt against the previous release; not below plain Otsu and Sauvola |
| HEIC colour | CIEDE2000 against an independent decode | conversion stage: mean <= 0.2, max <= 1.0. Final JPEG: mean <= 0.5, p99 <= 1.5, max <= 3; out-of-gamut pixels reported apart. EXIF/ICC kept except orientation (reset) and thumbnail (dropped) |
| Export (B11) | round trips, PSNR/SSIM floors, validity | PNG, TIFF, lossless WebP bit-exact. TIFF G4 bitwise, dpi and photometric tags verified by libtiff (our own wrapper: `fax::tiff::wrap` hardcodes 200 dpi and WhiteIsZero). PDF passes `qpdf --check` and renders in pdfium and pdf.js; 200 pages of 12 MP stay <= 500 MB peak RSS. Lossy floors set at first baseline |
| Idempotence | second run on its own output | >= 99% classified no-op; a no-op never rewrites the file (B3 would compound generation loss) |

Confidence gates are in 7.6, robustness gates in 7.7. The HEIC reference is not libheif (that would test only our glue): use macOS ImageIO output and independently encoded colour charts, reading the ICC profile first, then nclx/CICP. HDR tone-mapping and dropped gain maps are checked by visual reference images and the user notice.

## 7.5 Corpus strategy (decision B21)

| Layer | Contents | Lives in and runs |
|---|---|---|
| Public smoke | <= 5 MB: 200 synthetic images, hostile-file fixtures, small reference outputs | public repo; every PR, forks included |
| Public suites | ~5k synthetic images with exact ground truth; public datasets fetched by script with pinned SHA-256, never vendored | generated or downloaded; nightly, calibration, contributor signal |
| Private golden | real hand-labelled photos and scans | images only on the maintainer's encrypted disk (never Git LFS); the private repo `auto-crop-golden` holds workflows, labels and hashes; B6 and per-slice gates |

**Synthetic generator.** `tools/synth` is Python (OpenCV plus [Augraphy](https://github.com/sparkfish/augraphy), MIT) and shares no code with the app's warp, so the two cannot share a failure mode. It renders known pages and receipts (OFL fonts, public-domain text, generated line items, barcodes) under random homographies (tilt to 45 degrees, any rotation, aspect to 8:1), backgrounds, shadows, blur, noise, JPEG q40-95, thermal fade, curl, partial framing and multi-item layouts. Ground truth per image: exact corners, rotation, orientation, crop box, item count, a clean binary render and the transcript. Augraphy 8.2.6 dates from 2023-12-31, so pin it and keep it replaceable; avoid AlbumentationsX (AGPL-3.0). Backgrounds and base documents never cross the train and evaluation split.

**Public datasets.**

| Dataset | Use | Licence and caveats |
|---|---|---|
| [SmartDoc 2015 Ch.1](https://zenodo.org/records/1230217) | quad IoU; A4 on 5 backgrounds, ~24k frames; 1.5 GB test set, 21 MB sample | CC BY 4.0, cite the paper and email the organisers. Use the Zenodo original, not the HF DocCornerDataset aggregate (its card says research-only). 2015 baselines are not state of the art; background 5 is hardest |
| SmartDoc-QA, [CORD](https://github.com/clovaai/cord) | OCR CER before and after; receipts (CORD: 1,000 photos with text) | CC BY 4.0. SmartDoc-QA is one ~13 GB zip, so filter and cache. Both are thin on long narrow receipts, thermal fade, partial frames, touching items |
| MIDV-500, -2019 | ID-card quads under clutter | sources public domain or open (MIDV-500 paper); check each release |
| DIBCO/H-DIBCO, DocUNet/DIR300/UVDoc benchmark, Nokia HEIF conformance | binarisation and dewarp regressions, HEIC coverage | licences unverified or absent: fetch-only, never redistributed |

### Private golden set

The only guard against the synthetic-to-real gap, and never tuned against. In this plan "golden" means this private set only; checked-in regression outputs are called reference images.

- **Slices** (B1, B21, research gaps; slices overlap, so one image can count in several; >= 80 images each before its slice gate blocks): (1) receipts, long and narrow (aspect above 4:1); (2) thermal fade and low contrast; (3) partial frames; (4) touching or overlapping items; (5) phone photos of documents (clutter, white-on-white, low light, tilt above 30 degrees); (6) flatbed single scans; (7) flatbed multi-photo scans; (8) general photos (horizon and border crop, EXIF rotation); (9) HEIC/HEIF device files (iPhone and Android, Display P3, HDR or gain map, Live Photo, burst, grid); (10) no-document negatives.
- **Staging for a solo maintainer** (Assumption A-8, section 1.7: the golden set reaches v2 (>= 800 locked, >= 80 per gated slice, plus a ~300 dev tier) before 1.0, and its images never leave the maintainer's encrypted disk; a hosted variant needs the owner's approval). v0: >= 150 images, >= 25 per slice, at M1 for the first gate. v1: >= 500, >= 50 per slice, before G2 (M4) for hybrid detection. v2: >= 800 locked images, >= 80 per gated slice, before 1.0 (M13). Because slices overlap (a long thermal-fade receipt counts in two), ten slices of 25 fit in 150 images. The ~300-image dev tier is extra, so v2 means about 1,100 labelled images plus transcripts for ~100 receipts. A guess of 1-3 minutes per image and ~10 minutes per transcript gives roughly 40-80 hours, which the maintainer must approve; M1.42 replaces the guess with a measured rate.
- **Three tiers:** *dev* (unlocked, ~300 images beyond the golden counts, for thresholds and calibration); *golden* (locked, confirms once per release candidate, append-only evaluation log); a 20% subset double-labelled by two annotators, which sets a noise floor no target may undercut.
- **Scene-disjoint** across training, dev and golden through a `scene_id` manifest check. If golden results ever guide a change, retire and replace those images.
- **Label without the app's suggestion.** The research proposed labelling in the app's own adjust UI, which anchors annotators to the model's quad. Label in Label Studio or the blank-quad labeller `tools/labeler/`, never from a suggestion.
- **Consent.** The maintainer's own documents, redacted specimens, or images with the owner's consent; encrypted at rest. The public policy, without images, is `docs/testing/golden-set.md`.
- **Publishing.** Only aggregates leave (counts, means, percentiles, per-slice numbers, calibration bins): no per-image rows, paths, thumbnails or OCR text; only slices with n >= 30 are published, and contact sheets for human review never leave the machine. Aggregates come from the nightly dev-tier run and from release candidates, never per push, PR or fork, since a fine-grained series could reveal single-image outcomes.

**Running it safely (Assumption A-8, section 1.7; workflow details in 8.3.5).**

- **The private repo `auto-crop-golden`** holds workflows, labels and hashes: no images and no Git LFS. The images stay on the maintainer's encrypted disk and are read by a self-hosted runner attached to that repo only, so they never travel. (`auto-crop-eval` is the public crate and binary, not a repo.)
- **Two stages.** A GitHub-hosted runner builds the public repo at a SHA and attests the binary; it sees no golden path and no token. The self-hosted runner then runs only that built `auto-crop-eval` in a network-less container with the set mounted read-only. No third-party code (actions, build scripts, proc macros, npm scripts, dependency updates) runs beside the data. A small reviewed publisher checks the output against the `PublishableMetrics` allow-list (M1.52) and pushes only aggregates to the public `metrics` branch.
- **Triggers:** nightly (dev tier), `workflow_dispatch` (locked set, once per release candidate) and a release `repository_dispatch` from the public release workflow, whose fine-grained token can only dispatch to the private repo; the public repo holds no other golden-set secret. Never per push, pull request or fork.
- **Where it runs.** Overlap and scene-disjoint checks run in the private repo. The private HEIC corpus (GPS kept by a recorded exception to the ingest strip rule) runs only on the maintainer's Mac. The private set never runs on public CI or fork PRs, and it is kept outside any directory an AI coding tool can read (B17). There is no public-CI fallback; a hosted variant needs the owner's approval. If the runner is down, the maintainer runs `cargo xtask eval --golden` locally before the release candidate and commits the aggregate JSON.
- **Contributor PR code never touches the golden set.** Only merged `main` SHAs and release-candidate tags are built for it. zizmor lints workflows for `pull_request_target` mistakes.

**Signal without the private set.** `cargo xtask eval --suite smoke` (minutes) and `--suite public` run locally and on fork PRs with no secrets and print a delta against `main`; the private delta comes from the nightly dev-tier run after the merge. A failing image may be attached to an issue and, with consent, added to the private dev set. The public repo takes no real photos (B21).

## 7.6 Confidence calibration evaluation

Triage (B4, B5) only works if confidence is calibrated. The score combines heatmap peak, mask-quad agreement, edge support, geometry sanity, ML-versus-classical IoU, test-time-augmentation spread and residual skew; the label is "acceptable" by the silent-failure definition.

- **Fit** an isotonic calibrator (temperature scaling if data is scarce) on the *real* dev split, about 1,000 samples, so pool SmartDoc, MIDV and CORD with the unlocked private dev tier. Synthetic data must not calibrate the shipped score.
- **Evaluate** on held-out data: reliability diagram (10 equal-mass bins, Wilson 95% intervals), ECE, Brier score, AUROC of confidence against failure, risk-coverage curve. Gates: **ECE <= 0.05 pooled and <= 0.10 in the worst slice with n >= 30** (a pooled 0.03 can hide receipts at 0.15).
- **Derive** thresholds from the risk-coverage curve and confirm once per release candidate on the locked golden set; the shipped artefact is `calibration.json` (isotonic knots, three cutoffs, versions). Version the calibrator with `algo_ver`; any detector, model or threshold change forces recalibration.
- **Sanity tests without ground truth** (synthetic, every PR): confidence falls with blur, occlusion and tilt; moving one classical corner by 5% drops a Good result out of Good; rotating, mirroring or halving the input keeps corners consistent.

There is no telemetry (B18, C4), so field accuracy is invisible. A local Diagnostics panel counting "restored original after auto-save" gives users a silent-failure proxy to paste into an issue; nothing is sent.

## 7.7 Test pyramid

| Layer | Covers, tools and gates |
|---|---|
| Unit (every PR, 3 OSes) | DLT homography, line fit, u64 strip integrals against a naive Sauvola, Lanczos weights, EXIF orientation, collision planner, history coalescing, our TIFF G4 wrapper. cargo-nextest |
| Property (every PR) | undo(redo(x)) == x; homography round trip; `EditState` serde and migrations (insta snapshots); lossless transform equals decode-then-transform at non-MCU-aligned sizes; name planner never collides across NFC and case-fold variants. proptest 1.11 |
| Kernel oracles | our u8 Lanczos warp, Canny and LSD port against OpenCV, dev-only and never shipped: Python `cv2` writes checked-in fixtures, so contributors need no libclang. Warp PSNR floor 45 dB and warp <= 90 ms wall at 8 threads on 12 MP (both PROVISIONAL). int8 model variants ship only if, per platform, the p95 corner-error rise is <= 0.05% of the diagonal and the mean IoU delta is <= 0.3 pt (pixel-shift thresholds are not used). Nightly overflow guards: constant-255 images above 16.8 MP (u32 integral) and 66k pixels (u32 squares) |
| Reference-image / perceptual (every PR) | SSIM against checked-in reference outputs through `image-compare` 0.5 (MIT), **never dssim-core (AGPL)**: >= 0.99 same platform, >= 0.98 across platforms (SIMD paths differ by 1 LSB). ML outputs compare corners within +-0.5 px, not pixels |
| Data safety (B3; every PR; **blocks every release**) | atomic replace; crash between temp write and rename (original intact, temp swept at start); Windows sharing-violation retry; full disk; Unix parent-directory fsync; same-volume and cross-volume backups; **Restore original after app restart**; failed items byte-identical; never-replaced sources (animated, multi-page and other files this build cannot write back) byte-identical under the default mode. Fail-points |
| Fuzz (PR smoke; nightly, Linux and macOS) | cargo-fuzz (Unix only): header probe; in-process decoders behind our limits; EXIF and ICC transcoding; `HeicBackend` glue (stride, plane, bit depth); worker IPC frames (the worker is untrusted); `EditState` and journal JSON; `analyse()` on random rasters. ASan on FFI targets; 60 s PR smoke on codec and core crates, 1 h per target nightly, >= 72 h cumulative per target before 1.0 (nightlies count); crashers replayed as plain tests on Windows. Gate: 0 crashes or hangs |
| Hostile files (every PR) | tiny file claiming 60000x60000, scan-bomb JPEG, TIFF with many IFDs, huge HEIC grid, symlink loops, OneDrive placeholders: rejected within 1 s and 64 MB. A worker killed mid-decode fails its item and respawns |
| Panic and sandbox | per-item `catch_unwind`; CI asserts no profile sets `panic = "abort"`. The worker (`auto-crop-worker`) runs at the level each OS allows, and `doctor` and About print the achieved level: `appcontainer`, `job+token`, `landlock+seccomp`, `seccomp-only`, `sandbox_init` or `process-only`. The conformance test is keyed to that level, because only some levels can pass it. At `appcontainer`, `landlock+seccomp` and `sandbox_init` it asserts the worker cannot open a socket or read outside its input; at `seccomp-only` it asserts no sockets and reports "filesystem: not isolated"; at `job+token` (a Windows job object with a restricted token blocks neither the network nor user-file reads) and at `process-only` (macOS and Linux until M8 and M9) it asserts only the memory cap, kill-on-close and no child process (on Windows also no win32k) and reports "network: not isolated" until the AppContainer spike (M6.07) lands. At every level the worker dies at its memory cap, and a test asserts it recycles after any anomaly, after a decode above 50% of a cap and every 32 decodes |
| End-to-end UI (post-merge; Windows first) | Playwright (Chromium, WebKit) on the Svelte UI with `mockIPC`: grid logic, handles, keyboard alternatives, pseudo-locale, RTL, forced-colours, reduced-motion. The built app through `tauri-driver` (Windows; Linux under xvfb): open folder, auto-process, review flagged, save all, undo, restore original. No WKWebView driver is known, so macOS is manual plus CLI (confirm in the spike) |
| Accessibility (every PR) | axe-core (dev-only), zero serious or critical findings; Tab reaches every handle; arrow nudge 1 px, Shift 10 px; **handles 48 px nominal and never below 44 px, every other control >= 24 px** (WCAG [2.5.8](https://www.w3.org/WAI/WCAG22/Understanding/target-size-minimum.html)); a non-drag alternative for every drag (2.5.1, 2.5.7); handle stroke >= 3:1 over 200 photo patches. Manual screen-reader pass per release: NVDA and Narrator on Windows, VoiceOver on macOS; Orca is best-effort and does not gate |

Export round trips (B11), the HEIC device corpus (the private files only on the maintainer's Mac, 7.5) and the installed artifact's `auto-crop doctor --self-test` (model hashes, worker spawn, a tiny HEIC decode; GUI builds add `--smoke-test <dir>`) run at release.

**Touch protocol** (per release candidate, ~30 minutes per device). A Windows touchscreen (a Surface) and a Mac trackpad are guaranteed (B8); a Linux touchscreen is best-effort, reported and not gating. Check 48 px hit areas around 16 px handles, loupe placement, pinch, pan and rotate on a 24 MP proxy at 60 fps (frame times from a debug overlay), edge keep-out and press-and-hold against OS gestures, palm rejection, and the on-screen keyboard reaching numeric fields.

**CI matrix** (free runners; 20 concurrent jobs, 5 macOS): ubuntu-22.04 (glibc floor), ubuntu-22.04-arm, windows-2025, windows-11-arm and macos-26-intel (the ARM64 rows and the Intel row are best-effort until 1.0, and the Intel retirement date is unverified), macos-latest (arm64, first-class). All three OSes build and run unit tests from day one (B9); Windows is the required check for the first release.

- **Pull-request lane:** fmt, clippy `-D warnings`, cargo-deny, zizmor, nextest, reference-image tests, smoke accuracy, the 60 s fuzz smoke and gungraun on Ubuntu; build plus unit tests on Windows and macOS. **Post-merge:** full matrix, sandbox conformance, UI E2E. **Nightly:** fuzz, full public suite, wall-clock; the private dev-tier evaluation runs on the private runner (7.5).
- Numeric thresholds are enforced on Linux, where results are deterministic; other OSes assert parity within 0.2 pt mean IoU to catch SIMD and ONNX-provider differences.

## 7.8 Regression gating and accuracy dashboard

**Pull request, blocking:**

- the 200-image smoke: mean IoU down 0.3 pt or failure rate up 0.5 pt blocks. The comparison is paired against `main` and lists images that changed state (one image is 0.5 pt at n = 200). A justified trade-off needs an `accuracy-waiver` label and a reason;
- gungraun counts: more than 5% worse on a tracked kernel (decode glue, resize, warp, threshold, integral, fuse) blocks, 2% warns. `main` and the PR head are measured in the same job so runner and Valgrind drift cancel;
- unit, property, reference-image, data-safety, deny and workflow lint.

**Nightly, non-blocking (opens an issue):** p50 wall-clock or peak RSS more than 10% worse on any stage; fuzz; full public suite; private dev-tier evaluation. **Release candidates:** section 7.11.

**Dashboard.** Each run writes one JSON file per commit and suite to an orphan `metrics` branch. A static GitHub Pages page (uPlot or Chart.js) shows per-slice trends, a latency-versus-risk Pareto scatter, the latest reliability diagram and risk-coverage curve, a stage-time chart against Table A, and a gate table, each point linking to its commit.

```jsonc
{ "schema": "auto-crop-metrics/1", "commit": "abc123", "suite": "golden", "host": {"tier": "M"},
  "operating_point": "balanced", "algo_ver": "det-0.4", "n": 812, "coverage": 0.913,
  "silent_failure": {"k": 6, "n": 741, "rate": 0.0081, "ub95": 0.0159},
  "slices": {"receipt_long": {"n": 84, "iou_mean": 0.981, "flag_rate": 0.13, "silent_k": 1}},
  "calibration": {"ece": 0.031}, "latency_ms": {"e2e_p50": 640, "stages": {"decode": 118}} }
```

## 7.9 Speed-accuracy tiering

Time is spent only where confidence is low.

| Tier | Work | Time | Exit |
|---|---|---|---|
| T0 draft | embedded thumbnail or 1/8 scaled decode | <= 100 ms | display only, never a decision |
| T1 fast path | 256x256 net, geometry sanity, edge support, orientation net, calibrated confidence | <= 40 ms on the proxy | Good: proceed; expected for ~85-90% of images (PROVISIONAL) |
| T2 refine | 512x512 pass, flip and rotate test-time augmentation, classical detector, ROI second pass if the quad covers < 25% of the frame, full-resolution refine | <= 250 ms extra | Check-band confidence or disagreeing signals |
| T3 flag | none | none | below the lower threshold: original untouched, "Draw crop" banner, first in review (B4) |

The harness decides one open question: is the classical cross-check always on, or only on escalation? It cuts silent failures through independent failure modes but costs an estimated 10-60 ms. Run both over the public and dev sets and keep the cheaper one that holds <= 1% risk at <= 10% flagged; net input size (256, 384, 512), test-time augmentation and refine are tuned on the same latency-versus-risk Pareto. Expected analysis time at a 12% escalation rate is 40 + 0.12 x 250 = 70 ms, which is why Table A's 700 ms is stated for T1 exits. For the same reason the 40 ms `analyse` p95 budget covers T1 exits only: once more than 5% of images escalate, the 95th percentile of the whole population falls inside the escalated group. The mixed-population targets are p50 <= 40 ms, mean ~70 ms and p95 ~290 ms (40 + 250). The first overlay is always the T1 result, shown progressively, with the T2 and full-resolution refinements replacing it later (2.4), so its 150 ms p95 is measured over all images, not only T1 exits.

## 7.10 Memory ceilings and behaviour above ~100 MP

- **Per image:** <= 3x decoded RGB8 + 64 MB: 175 MB at 12 MP, 500 MB at 48 MP, 1.0 GB at 100 MP.
- **Batch admission:** a byte-weighted budget of min(25% of RAM, 4 GiB, 50% of free RAM) (2.8); each job declares an estimate from probed dimensions (pixels x 9 + 64 MiB, so a 100 MP job weighs ~0.9 GiB), and an oversize job **runs alone** instead of deadlocking the queue. At the 8 GB floor the 25% term is 2 GiB, so two 100 MP jobs just fit; on a typical 8 GB machine the 50%-of-free-RAM term serialises them. Decode workers run under per-process memory limits (D3) counted against it.
- **Known traps:** two u64 integral images at 12 MP cost ~192 MB, so Sauvola is strip-wise; kornia's f32 Lanczos warp needs ~144 MB at 12 MP RGB, so any use is strip-wise; imageproc's u32 integral overflows above ~16.8 MP; the `image` crate's 512 MiB allocation limit is non-strict, so we enforce our own caps.
- **Above ~100 MP (C1).** The default hard cap is **100 MP**; larger files get a clear error naming the size and the setting (a file just above 100 MP needs one "allow this file" click). Advanced settings, `--max-pixels` or "allow this file" can raise the cap up to a ceiling of 500 MP: the job then runs alone with a warning, analyses from a 1/8-scaled decode and processes strip-wise with per-64-row cancellation. The worker refuses anything over 500 MP regardless. Hostile fixtures are rejected within 1 s and 64 MB, and the worker watchdog is 10 s + 1 s per MP (PROVISIONAL). Strip-wise JPEG decode needs libjpeg-turbo's scanline crop and skip; whether the `turbojpeg` crate exposes it is unchecked. A nightly 100 MP job under an RSS cap guards this.

## 7.11 Release quality gates

Gates are keyed by capability so they survive milestone reordering; the Milestone column is the current mapping, and every GATE line in ROADMAP.md cites its G-ID (single items carry `_(gate: ...)_` markers for their metric). Thresholds are PROVISIONAL until baselined, and every gate also requires all earlier checks to stay green. Previews publish their numbers even where a gate is advisory, and say so in the release notes. A gate that needs hardware (a Mac, a Windows touch device, a flatbed scanner) stays open until measured on it; publishing past one needs the owner's recorded delta (M13.42), never "CI-validated only" (Assumption A-9, section 1.7).

**Gate-miss rule (Assumption A-4, section 1.7).** A missed exit gate delays 1.0. Shipping the feature labelled Experimental or moving it to 1.x reopens B10 and is the owner's decision, put with measured numbers; nothing is lowered, cut or disabled silently. The rule covers G4 (multi-item), G5 (dewarp) and G11 (AVIF and JXL). The owner is asked at the M0 wrap-up and the answer is stored as an ADR before the M10 gates (protocol X.41; M12.68 is its dewarp instance).

| Gate | Milestone | Speed and memory | Accuracy and safety |
|---|---|---|---|
| **G0** Harness ready (before any GUI work) | M1 | Table A spans measured on Windows Tier-M (the analysis line on labelled stand-ins until the detectors land); Apple-silicon baseline; budgets re-baselined | public suites in CI; dashboard live; golden v0 evaluated once; calibration and ECE code validated on synthetic predictors (no detector with a confidence exists yet) |
| **G1** First Windows preview (unsigned; B9, B15) | M2 | measured numbers published; no line more than 2x over budget; CLI batch >= 4 images/s on 6 cores (5 workers), within Table B | data-safety suite green (Restore original after restart, idempotence, never-replaced sources byte-identical); fuzz clean; hostile-file caps work; CLI JSON schema and exit codes tested, including exit 6 when no backup can be made; silent-failure rate measured on golden v0 and printed in the release notes; default Strict (A-3; the v0 bound cannot pass); cargo-deny green |
| **G2** Hybrid detection (B7) | M4 | `analyse` <= 40 ms p95 over T1 exits (mixed population: p50 <= 40 ms, p95 ~290 ms); first overlay (the T1 result, refined later) <= 150 ms p95 | golden v1 (>= 500 images, >= 50 per slice): the B6 gate (point <= 1.0%, Clopper-Pearson bound <= 2.0%) decides the default under A-3 (pass: Balanced is the unlabelled default; miss: Strict is the default and Balanced is labelled experimental; the bar is never lowered, and a miss that persists to G7 is a gate miss under A-4); a Balanced flag rate above 10% (15% at v0.3.0) is a stage-gate note; ECE gates; quad, corner, skew, orientation thresholds; int8 ships only if its gate in 7.7 holds; model provenance log complete (`xtask check-models`) |
| **G3** Enhancement (B13) | M7 | slider <= 16 ms compute on the 2 MP display proxy; `enhance` line within Table A (Auto and Grayscale 120 ms; B&W and Faded <= 250 ms) | CER and amount-token gates (7.4); DIBCO no regression; despeckle default off; TIFF G4 tags verified |
| **G4** Multi-item (B10) | M10 | single-item throughput regresses by <= 5% against the previous release; split analysis <= 40 ms p50 on the 1024 px proxy; a 48 MP scan with 8 items within the RSS budget | count, recall, precision gates; touching items routed to review; 1-to-N leaves no partial set and no lost original under fault injection; a miss follows A-4 |
| **G5** Dewarp (B10) | M12 | <= 1 s per 12 MP page on Windows Tier-M (PROVISIONAL), cost stated; opt-in: Suggest is the default and Auto is chosen per batch or image | median relative CER cut >= 20% on the curved slice against the perspective-only render; worse by more than 1 CER point on <= 2% of images; >= 95% of flat pages skipped and byte-identical, and no non-skipped flat page more than 1 pt worse; weights licence and data-provenance audit signed off. A missed provenance, quality or speed gate is a gate miss under A-4 (M12.68): 1.0 is delayed, or the owner decides, with the measured numbers, between shipping dewarp labelled Experimental (this reopens B10) and moving it to 1.x; it is never silently disabled, cut or relabelled |
| **G6** macOS and Linux previews (B9) | M8, M9 | Apple-silicon Tier-M within budget x1.25; Linux pointer pan and zoom on the 24 MP image: median frame <= 22 ms, p95 <= 33 ms on the GPU rows (x86_64) | cross-OS parity <= 0.2 pt; per-OS sandbox conformance at the achieved level (7.7); touch protocol run (Linux touch recorded as works, partial or fails; never gating, B8); int8 delta per platform |
| **G7** 1.0 release candidate, all three OSes | M13 | all Tier-M budgets met, Tier-L within x2, cold start <= 1.0 s | golden v2 (>= 800 locked images, >= 80 per gated slice; the ~300-image dev tier is extra): B6 bounds, <= 1 silent failure per slice with n >= 80, worst-slice ECE; fuzz clean over >= 72 h cumulative per target, nightlies count (PROVISIONAL); axe clean and a screen-reader pass (NVDA and Narrator on Windows, VoiceOver on macOS; Orca best-effort, recorded, not gating); HEVC legal read done, or the owner's recorded decision for a `no-hevc`-only release (B12, A-5); clean-VM installer smoke test |
| **G8** Interactive editing (GUI alpha; B8) | M3 | drag, pinch and rotate on a 24 MP image (2-4 MP display proxy): frame median <= 17 ms, p95 <= 20 ms, no IPC during the drag; release-to-final p50 <= 150 ms, p95 <= 250 ms; slider tick <= 16 ms compute on the 2 MP display proxy; undo and redo <= 50 ms; cold start <= 1.0 s | TypeScript and Rust homographies agree within 0.05 px on the 2-4 MP display proxy; handles 48 px nominal and never below 44 px, other controls >= 24 px; a non-drag alternative for every drag; axe clean; negative IPC tests pass (the webview holds no plugin permission); touch protocol run on a Windows touchscreen and a Mac trackpad |
| **G9** Batch review (B4, B5) | M5 | GUI batch >= 3 images/s at 12 MP on 6 cores (4 workers) with the preview live; peak RSS within the admission budget on the 300-image mixed corpus and on a 48 MP batch; a 100 MP file is admitted alone | crash-kill at every journal state: resume reconciles and no original is lost or modified; Check and Failed items never written and byte-identical; "All" accepts Good only; run-level Restore original works after a restart |
| **G10** HEIC (B12) | M6 | HEIC to JPG >= 2.5 images/s at 12 MP on 6 cores (5 is a non-gating stretch); final overlay <= 700 ms and embedded-thumbnail draft <= 150 ms at 12 MP; a 48 MP HEIC batch within the admission budget | colour dE gates (7.4); orientation applied exactly once (8 orientations x HEIC, no double rotation); device corpus decodes with 0 crashes or hangs; notices fire for a dropped gain map, depth and Live Photo video; sandbox conformance at the achieved level; fuzz clean |
| **G11** Exports (B11) | M11 | each encoder within its PROVISIONAL time cap (section 3.3); a PDF of 200 x 12 MP pages <= 500 MB peak RSS; cancel returns promptly | round-trip matrix 100% in two independent readers per output; PNG, TIFF and lossless WebP bit-exact; TIFF G4 tags verified; PDF `qpdf --check`, pdfium and pdf.js; GPS-leak suite passes (no location bytes after the strip modes); replacements only through a verified backup; a missed AVIF or JXL gate follows A-4 |

**Milestone sizes.** A roadmap Size is part-time weeks for the solo maintainer: S 1-2, M 3-4, L 5-8, XL 9-16 (PROVISIONAL; re-estimated at every retrospective, X.40). M0, M1, M10 and M11 are XL. M0 is about 23 spike days plus about 20 set-up days (8-10 weeks at half time): the repository, 3-OS CI and licence gates and the GUI spike come first, and only M3 waits for the GUI-stack decision (M0.70).

## 7.12 Open items and unverified inputs

- Every latency in 7.1 is an extrapolation. The decode figure (~85 ms per 6 MP) has no source; fast_image_resize numbers are single-threaded desktop results stamped 6.0.1; the 70% scaling efficiency behind the throughput floors is an assumption; libheif, WIC and ImageIO decode times and the T1 and T2 rates are unmeasured.
- Unverified: gungraun under CodSpeed, Callgrind coverage of the SIMD paths users run, Tauri WebDriver coverage, the libheif parallel-tile flag in our build, and licences for DIBCO years, MIDV-2019/2020 and SmartDoc-QA parts.
- The >= 99% orientation target and int8 accuracy per platform have no supporting measurement. Tolerances (PSNR floors, SSIM 0.99/0.98, sample sizes, the 0.90 IoU line) are chosen starting values.
