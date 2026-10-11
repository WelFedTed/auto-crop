# 0010 - Learned page-corner detector: feasibility study

- **Status:** accepted as a study result (not a shipping decision); the owner may veto
- **Date:** 2026-10-05
- **Roadmap items:** M4.12 (generator), M4.14 (training script), M4.15 (from-scratch time box), M4.16 (ONNX export), early input to M4.49 and M4.42; the ticks of those items are NOT changed here (see "Consequences")
- **Decision log links:** B7 (small bundled ONNX models), B2 (licences), B21 (private data), M4.05 (from-scratch default), ADR 0003 and ADR 0007
- **Time box:** a few days of wall time with several interruptions by machine crashes (PROVISIONAL budget); about 56 minutes of GPU training in total. Code: `tools/train/` and the `--train` mode of `tools/synth/`. Nothing in the application depends on it.

## Context

The classical detector fails 40.5% of the 5,200-image public synthetic suite at the IoU < 0.90 bar (`docs/perf/detector-baseline.md`, re-measured below), with no answer at all on 11.8% of it, and ADR 0003 showed long receipts and cluttered or low-contrast desks are where it breaks. B7 plans a small learned corner detector for M4. No cleared pretrained weights exist (M4.05), so the question was: **can a small net trained from scratch, for about an hour on one consumer GPU, on our own procedural data, beat the classical detector on held-out synthetic scenes, and does anything we can measure suggest it transfers to real photographs?**

What was built (all under `tools/`, never shipped, no weights, datasets or checkpoints committed, model-weights policy rules 3 and 5):

- `tools/synth --train`: failure-biased quotas (long and strip receipts, textured desks, partial frames, curl, any rotation), four extra desk textures, and a hand holding the page that can hide an edge or a corner. Truth is the page's true corners. Training seeds are refused if they equal an evaluation seed.
- `tools/train`: a MobileNetV3-small-layout backbone with a 64-channel FPN and one stride-4 head (class-agnostic corner heatmap, page mask, page-centre heatmap, centre-to-corner offsets), 0.94 M parameters, random initialisation, 256 px letterbox input. Decoding snaps regressed corners to heatmap peaks. A logistic confidence is fitted on synthetic validation data only.
- Training data: 20,000 generated images (four 5,000-image sets, generated from seeds different from the 1,000-image validation set and from every evaluation suite). Total training: 5,200 steps (30 min), then a continued run of 5,500 steps (26 min), batch 64, on one RTX 3060. A third continuation was started and lost to a crash; it is not used.

## Options considered

| Option | Notes |
|---|---|
| A. Classical detector only (status quo) | Shipping today; known weak on long receipts, tiles, white desks, partial frames. |
| B. Learned detector alone | This study. |
| C. Learned plus classical, offline hybrids in `tools/train/hybrid.py` | `fallback` (classical if it says good, else learned), `snap` (learned quad replaced by the classical candidate it agrees with at IoU >= 0.8, so the classical line fit supplies the sub-pixel corners), `snap_solo` (as snap, plus a learned quad no candidate agrees with may still be good), `agree` (good only when both agree at IoU >= 0.9). |
| D. Pretrained or ImageNet-initialised backbone | Not tried: needs an owner-granted A-7 exception (M4.05). Open lever if B and C plateau. |

## Results

Protocol: the repository's own harness (`auto-crop-eval run`, canonical-warp IoU, failure = IoU < 0.90, no quad counts as failure), the learned model scored through `--predictor jsonl:`, the classical detector scored by `--predictor detector` at the same commit. The evaluation suites are the default generator's (`tools/train/gen_eval.sh`) and were never seen in training; one checkpoint (`best.pt` of the continued run, chosen on the synthetic validation set only) and one confidence threshold (fitted on the synthetic validation set, 0.914) were used for everything below. Windows x64, Python 3.14, torch 2.14.1 (CUDA) for prediction.

### Held-out synthetic, overall

| Suite (n) | Predictor | Mean IoU (95% CI) | Failure % | No quad | Auto-accepted, silent failures (one-sided 95% bound) | Flag rate |
|---|---|---|---|---|---|---|
| full (5,200) | classical | 0.6965 (0.685-0.708) | 40.5 | 616 | 1,468, 31 (2.84%) | 71.8% |
| | learned | 0.9240 (0.922-0.926) | 23.0 | 0 | 1,873, 10 (0.90%) | 64.0% |
| | `snap` hybrid | 0.9380 | 20.4 | 0 | 1,776, 28 (2.16%) | 65.9% |
| | `agree` hybrid | 0.9284 | 22.9 | 0 | 1,370, 1 (0.35%) | 73.7% |
| | `fallback` hybrid | 0.9263 | 22.4 | 0 | 2,369, 40 (2.19%) | 54.4% |
| mid-d (1,000) | classical / learned | 0.689 / 0.923 | 43.4 / 24.0 | 123 / 0 | 290, 7 / 341, 0 | 71.0% / 65.9% |
| smoke, smoke-b, smoke-c (200 each) | classical / learned | 0.696, 0.726, 0.705 / 0.916, 0.923, 0.922 | 41.5, 40.5, 42.5 / 26.0, 26.0, 26.0 | | | |
| test-train-mode (1,000; the harder training-mode distribution with hands) | classical / learned | 0.572 / 0.886 | 56.5 / 38.0 | 170 / 0 | 169, 9 / 198, 1 | |

The learned detector ECE is 0.053 and AUROC 0.942 on `full` (classical: 0.204 and 0.879), but the confidence is fitted on the same synthetic distribution it is scored on, so these are in-distribution numbers.

### Held-out synthetic, per slice (`full`, 5,200; failure % and mean IoU)

| Slice (n) | classical | learned | `snap` |
|---|---|---|---|
| aspect=document (2,082) | 28.5, 0.793 | 18.7, 0.938 | 14.5, 0.955 |
| aspect=receipt (1,560) | 37.2, 0.724 | 19.9, 0.931 | 18.8, 0.944 |
| aspect=long (1,090) | 55.5, 0.582 | 25.2, 0.912 | 25.4, 0.922 |
| aspect=strip (468) | 69.2, 0.440 | 47.2, 0.864 | 40.4, 0.881 |
| framing=full (4,576) | 32.5, 0.757 | 14.2, 0.942 | 11.0, 0.959 |
| framing=partial (624) | 98.9, 0.249 | 87.2, 0.792 | 89.4, 0.785 |
| background=tile (624) | 66.2, 0.539 | 23.9, 0.920 | 22.3, 0.930 |
| background=white-desk (831) | 56.0, 0.569 | 31.8, 0.907 | 29.7, 0.914 |
| background=wood (831) | 27.9, 0.793 | 19.0, 0.932 | 16.4, 0.950 |

The other backgrounds (dark-mat, fabric, plain, stone) and `mid-d` show the same ordering. The learned net is better than the classical detector on every slice of every suite measured, by 5 to 40 points of failure rate; the largest gains are the ones the classical detector is worst at (long receipts, tile, white desk). It is not good enough yet on strips (47% failure) and partial frames (87% failure), where the page extends past the picture and the 0.90 bar is hard for any detector. Overall 23% failure (20% for `snap`) is still far above the M4 targets.

### Real photographs (evaluation only, aggregates only)

The owner's private images with approximate hand labels (`_data/labels.jsonl`, 15 images, labels good to about 1-2% of image size, never used for training, tuning or checkpoint choice; the clipping convention (`--clamp`) was fixed before this run). 15 images with approximate labels cannot support a conclusion; they are a smoke test of the synthetic-to-real gap.

| Predictor | Mean IoU | Failure (IoU < 0.90) | Corner error p50, % of diagonal | Auto-accepted (silent failures) |
|---|---|---|---|---|
| classical (14 of 15 answered) | 0.650 | 10 of 15 | 4.6 | 3 (1) |
| learned, `--clamp` | 0.745 | 8 of 15 | 46 (not meaningful: probably a vertex-order artefact of the clipped quad; IoU is order-independent; not investigated) | 1 (0) |
| learned, no clamp | 0.691 | 12 of 15 | 8.6 | 1 (0) |

The synthetic-to-real gap is real: failure on these photos (53%) is more than twice the synthetic rate, and the learned detector flags 93% of them for review (the confidence threshold was fitted on synthetic data). It still scores higher mean IoU than the classical detector on the same 15, but the difference is far inside the noise of n = 15 and approximate labels. **This is the key unmeasured quantity.** No other real labels were available.

### Cost and size

| Quantity | Value |
|---|---|
| Parameters / ONNX size (fp32, opset 17, static 1x3x256x256) | 0.94 M / 3.77 MB (bar: <= 15 MB) |
| Operators | Add, Conv, GlobalAveragePool, HardSigmoid, HardSwish, Mul, Relu, Resize (the set ADR 0007 covered) |
| PyTorch vs ONNX Runtime CPU, max abs difference | 7.8e-5 over 5 random inputs (bar 1e-4 over 50 images: only 5 random inputs checked) |
| ONNX Runtime 1.30.0 CPU, network only, 256x256 | 4.1 ms median, 6.9 ms p95 at 4 threads; 12.3 ms median at 1 thread; 50 ms cold load (12 logical CPUs, Python, 100 timed runs after warm-up) |
| Training | 56 min on one RTX 3060, about 10,700 steps; validation mean IoU still rising (0.879 at step 4,141 to 0.888 at step 4,848 of the second run) |
| M4.14 overfit check | 36 images, 700 steps (4.4 min): mean corner error 2.7 px (median 1.7) at 256 px, mean IoU 0.93. The bar is < 1 px, **not met** at this budget (loss was still falling). |

## Not shown (UNMEASURED, not passes)

- Accuracy on real photographs beyond the 15-image smoke test above. The M4.46 golden set, M4.49 (30 real long receipts) and the M4.42 calibration on real data do not exist yet. Every confidence and risk figure above is synthetic and in-distribution.
- Whether more training helps: validation was still improving when the budget ended, and no larger data set, longer schedule, wider net, stride-2 head or higher resolution was tried. The overfit bar (< 1 px) was not met.
- Pretrained or ImageNet initialisation (option D): not tried, needs an A-7 exception.
- Inference in Rust: no rten or `ort` run of this net, no int8 version, no Windows-ARM64, Linux or macOS timing, and no timing of the Rust pre- and post-processing. The 4 ms figure is the network alone in Python on the owner's Windows machine. ADR 0007 measured rten at 2-4x ONNX Runtime on a net of the same layout.
- Behaviour on images with no page, several pages, or a page held by a real hand (the synthetic hands are capsules, not skin); on HEIC or other decoded inputs; and the orientation (the net predicts corners only).
- Run-to-run variance: one training seed, one checkpoint. The three 200-image smoke suites agree to within 1 point, which says nothing about seed variance.
- Hybrids on real photographs, and the `agree` hybrid's 0.07% silent-failure risk outside the synthetic distribution.
- Reproducibility of the exact checkpoint: training is not bit-deterministic (GPU, data-loader workers); the data generator is deterministic and `tools/train/README.md` gives the commands. The checkpoint itself is not archived (policy: nothing committed).
- The training-mode generator was verified against the unchanged one: the default `smoke` and `smoke-c` suites regenerated by this commit's generator are byte-identical to those from the generator at the parent commit (same manifest hash, same image hashes; only the wall-clock `seconds` field in `suite.json` differs), and a unit test checks that the default (no-hands) path is untouched.

## Decision

On the evidence we have, a from-scratch 0.94 M-parameter corner net is **feasible and much stronger than the classical detector on synthetic data** (mean IoU 0.92 against 0.70; failure 23% against 40%; no case where the classical detector wins a slice), costs 3.8 MB and about 4 ms of CPU per image, and a conservative combination (`agree`: both detectors must concur before a result is called good) reached 1 silent failure in 1,370 auto-accepted synthetic images. The evidence that this carries over to real photographs is one 15-image smoke test that points the same way but with a visible gap and no statistical power. The recommended path is therefore conditional, and it does **not** change what the next release ships: the classical detector remains the shipping detector until the condition below is met.

**GO-IF:** continue the learned detector as the primary M4 candidate, provided that (1) on at least 30 real labelled receipts and documents (M4.49 and the dev tier of M4.46, with exact labels and a published interval) its failure rate at IoU < 0.90 is lower than the classical detector's with the intervals separated, (2) a longer or larger training run (more steps, the open lever of an ImageNet exception, a real-photo fine-tune set) brings the synthetic overfit check under 1 px and the real-photo gap down, and (3) an inference run of the exported net in the pinned Rust backend (ort and rten) matches PyTorch within the M4.16 bar. If (1) fails after one more training cycle, record NO-GO for the learned detector and keep the classical detector with the existing candidate-ranking work.

## Consequences

- No change to PLAN.md, the decision log or what ships in the next release: the application and every crate are untouched; there is no model in the repository and none in a release.
- ROADMAP: M4.12, M4.14 and M4.16 are NOT ticked. Their acceptance lines are not met as written (M4.12 asks for corner error < 0.5 px, M4.14 for an overfit below 1 px, M4.16 for parity on 50 images in models-repo CI). The code exists and is the starting point; the numbers in this ADR are the evidence to cite. Roadmap item M4.83 records the study.
- `tools/synth` gains `--train` (default behaviour byte-identical, see above); `tools/train` and its four Python pins are new developer tooling and are listed in `docs/provenance.md`.
- The weights and datasets live under `target/` on the owner's machine only (one repo, no weights in git).
- Revisit trigger: the golden-set dev tier (M4.46) or any set of at least 30 real labelled long receipts is available, or a second training cycle with a materially different recipe finishes.

## Update 2026-10-12: second cycle, the owner's real labels, the Rust path

Status of the recommendation: **still GO-IF, with narrower conditions (below). The shipping detector stays classical.** Code: `tools/train/`, the training mode of `tools/synth/`, `crates/infer` (feature `learned`, off by default). `crates/imgproc/src/detect.rs` was not touched here. **The classical detector changed under this study:** another change (`af872cd`, "hold what colour says is not the page", see `docs/perf/detector-baseline.md`) landed on `main` while it ran. Every classical number below is therefore given for both `main` at `1bb9c50` (before) and `main` at `0d03275` (after, the harness records my rebased HEAD `316ffb0`, which differs from `0d03275` only in tools, docs and the infer crate); the learned predictions were produced once and are unchanged, only the hybrids and the paired statistics were recomputed against the new classical answers. The earlier checkpoints and venv were lost in a crash; the environment was recreated from the hash-locked `tools/train/requirements.lock` (torch 2.14.1+cu130) and the net retrained.

### What changed since the first cycle

- **Generator, training mode only** (`python -m synth --train`; each feature draws from a random stream of its own; the default and evaluation suites are byte-identical: `smoke` and `smoke-c` regenerated with the changed generator have the same manifest hash and the same 200 image hashes as before): 30% partial frames that may cut away up to 60% of the page (was 20% and 45%), "tight" pages that fill the picture (25% of full-frame pictures), white-on-white desks (20%), photo prints with a white mat (15% of document scenes; the truth is the whole print, an assumption about how the owner labels prints) and 50% hands (was 40%). Unit tests cover the identity of the default path and the truth of the new modes.
- **Net and training** (the same 0.94 M-parameter from-scratch net, no pretrained weights, no downloads): 384 px input (was 256; heatmap 96 x 96); 25% of samples zoomed so the page fills the canvas; 15% with a random finger-like blob over the picture; a page whose centre is outside the frame (a receipt cut by the picture) gets its centre target on the nearest border cell, so the decoder always finds a centre. 20,000 generated pictures (four 5,000-image sets, seeds disjoint from every evaluation suite) and a new 1,000-image validation set. Three consecutive runs, each continuing the EMA weights of the previous: A 6,150 steps (55.5 min), B 5,746 steps (55.6 min), C 6,397 steps (40.3 min); batch 64, AdamW, a cosine schedule per run. About 2.5 GPU-hours of training plus about 0.4 h of overfit checks on one RTX 3060 (CPU-bound data loading kept the GPU about half busy, and the machine was shared with other work, so the step counts are lower than the minutes would allow).
- **Selection:** checkpoints by the synthetic validation score only (`best.pt`: B at step 4,917, C at its last step); confidence threshold fitted on the synthetic validation set (B 0.927, C 0.934). The owner's photographs were never used for training, tuning, thresholds or checkpoint choice. They were evaluated **twice for the learned net (candidate B once, candidate C once)** and three times for the classical detector (no tuning). Quads are answered as predicted, not clipped to the frame (the golden labels may lie outside it).

### Held-out synthetic (default generator, never seen in training)

| Suite (n) | Predictor | Mean IoU (95% CI) | Failure % | No quad | Auto-accepted, silent failures (one-sided 95% bound) | Flag rate |
|---|---|---|---|---|---|---|
| full (5,200) | classical, `1bb9c50` | 0.6965 (0.685-0.708) | 40.5 | 616 | 1,468, 31 (2.84%) | 71.8% |
| | classical, `0d03275` | 0.6979 (0.686-0.710) | 40.1 | 623 | 1,561, 32 (2.74%) | 70.0% |
| | learned B | 0.9281 (0.926-0.931) | 22.3 | 0 | 2,488, 13 (0.83%) | 52.2% |
| | **learned C** | **0.9380 (0.936-0.940)** | **18.2** | 0 | 2,603, 9 (0.60%) | 49.9% |
| | `snap` hybrid (C, new classical) | 0.9450 (0.943-0.947) | 18.1 | 0 | 2,390, 61 (3.15%) | 54.0% |
| | `agree` hybrid (C, new classical) | 0.9409 (0.939-0.943) | 18.2 | 0 | 1,496, 1 (0.32%) | 71.2% |
| | `agree_l` hybrid (C; the learned quad when both agree) | 0.9380 (0.936-0.940) | 18.2 | 0 | 1,496, 3 (0.52%) | 71.2% |
| | `fallback` hybrid (C, new classical) | 0.9376 (0.935-0.940) | 18.2 | 0 | 2,931, 41 (1.81%) | 43.6% |
| | first cycle (table above, 256 px) | 0.9240 (0.922-0.926) | 23.0 | 0 | 1,873, 10 (0.90%) | 64.0% |
| mid-d (1,000) | classical `1bb9c50` / `0d03275` / B / C | 0.689 / 0.690 / 0.925 / 0.934 | 43.4 / 43.0 / 23.5 / 20.1 | 122 / 122 / 0 / 0 | 290, 7 / 307, 5 / 453, 3 / 480, 3 | 71.0 / 69.3 / 54.7 / 52.0% |
| smoke, smoke-b, smoke-c (200 each) | classical `0d03275` | 0.700, 0.726, 0.710 | 41.5, 40.5, 41.5 | | | |
| | C | 0.929, 0.937, 0.932 | 24.0, 19.0, 20.0 | 0 | 87, 2 / 96, 0 / 99, 0 | |
| test-train-mode (1,000; the **new** harder training-mode generator: hands, mats, tight and white-on-white pages) | classical `0d03275` / C | 0.500 / 0.874 | 65.4 / 41.5 | 223 / 0 | 105, 11 / 242, 7 | 89.5 / 75.8% |

(The `test-train-mode` of the first cycle came from the older generator and is not comparable.) Confidence quality of C on `full`: ECE 0.039, AUROC 0.956 (classical 0.204 and 0.879), in distribution.

Per slice of `full` (failure %, mean IoU); the two slices that moved most against the first cycle are in bold:

| Slice (n) | classical | first cycle | B | C | `snap` (C) |
|---|---|---|---|---|---|
| aspect=document (2,082) | 28.5, 0.793 | 18.7, 0.938 | 20.1, 0.941 | 15.7, 0.951 | 13.3, 0.961 |
| aspect=receipt (1,560) | 37.2, 0.724 | 19.9, 0.931 | 19.4, 0.938 | 15.8, 0.948 | 16.5, 0.952 |
| aspect=long (1,090) | 55.5, 0.582 | 25.2, 0.912 | 24.8, 0.915 | 19.4, 0.924 | 22.2, 0.928 |
| aspect=strip (468) | 69.2, 0.440 | 47.2, 0.864 | 36.3, 0.868 | **35.3, 0.882** | 35.0, 0.891 |
| framing=full (4,576) | 32.5, 0.757 | 14.2, 0.942 | 13.3, 0.947 | 9.6, 0.955 | 9.1, 0.964 |
| framing=partial (624) | 98.9, 0.249 | 87.2, 0.792 | 88.6, 0.786 | **81.4, 0.816** | 83.7, 0.808 |
| background=tile (624) | 66.2, 0.539 | 23.9, 0.920 | 22.8, 0.924 | 17.8, 0.938 | 19.9, 0.940 |
| background=white-desk (831) | 56.0, 0.569 | 31.8, 0.907 | 29.8, 0.916 | 23.6, 0.929 | 25.8, 0.927 |
| background=wood (831) | 27.9, 0.793 | 19.0, 0.932 | 19.4, 0.936 | 16.6, 0.944 | 15.6, 0.955 |

(The classical column is `1bb9c50`; at `0d03275` no slice moves by more than 2 points, for example tile 64.4%, 0.544 and dark-mat 38.1%, 0.707.) The ordering is the same as before on every slice (learned far above classical). The two weak spots of the first cycle improved but did not close: strips 47% to 35% failure (the larger input helps; the 0.90 bar on a 25 px wide strip is a sub-pixel bar) and partial frames 87% to 81% (the truth lies beyond the frame, so part of the answer is an extrapolation; training on pages cut by up to 60% helped little).

### The owner's real photographs (evaluation only; aggregates; n = 17)

The 17 single-item, not-assisted blank-quad labels in `_data/golden/labels` (12 `receipt-long`, 3 `flatbed-single`, 2 `general-photo`; 7 flagged `partial_frame`, 1 `curved`). The other 13 labels are multi-item (10) or assisted (3), which a single-quad predictor cannot be scored on, and are left out. Exact labels, scored by the harness (canonical-warp IoU, failure = IoU < 0.90, no quad = failure). Percentile bootstrap over images, 10,000 resamples (one image per scene, so images are the resampling unit); exact Clopper-Pearson interval on the failure rate.

| Predictor | Mean IoU (95% bootstrap) | Failure (IoU < 0.90), exact 95% | Auto-accepted (silent failures) | Images with IoU >= .95 / .90-.95 / .80-.90 / .50-.80 / < .50 |
|---|---|---|---|---|
| classical, `1bb9c50` (1 of 17 gave no quad) | 0.624 (0.43-0.80) | 11 of 17 = 64.7% (38-86%) | 4 (3) | 6 / 0 / 2 / 3 / 6 |
| classical, `0d03275` (1 of 17 no quad; the comparison below) | 0.686 (0.50-0.85) | 9 of 17 = 52.9% (28-77%) | 2 (0) | 8 / 0 / 1 / 3 / 5 |
| learned B | 0.797 (0.69-0.88) | 10 of 17 = 58.8% (33-82%) | 0 (0) | 4 / 3 / 4 / 4 / 2 |
| learned C | 0.783 (0.68-0.87) | 12 of 17 = 70.6% (44-90%) | 1 (0) | 4 / 1 / 5 / 5 / 2 |
| `snap` hybrid B / C (new classical) | 0.811 (0.70-0.90) / 0.804 (0.69-0.90) | 9 / 10 of 17 = 52.9% / 58.8% | 0 (0) / 1 (0) | C: 6 / 1 / 4 / 4 / 2 |
| `agree` and `agree_l` hybrids B / C (new classical) | as the learned net | as the learned net | 1 (0) / 1 (0) | |
| `fallback` hybrid B / C (new classical) | 0.843 (0.77-0.91) / 0.832 (0.76-0.90) | 9 / 11 of 17 = 52.9% / 64.7% | 2 (0) / 2 (0) | |

Paired differences against the **new** classical (the same images resampled): mean IoU B +0.111 (-0.096 to +0.324), C +0.097 (-0.107 to +0.309), `snap` B +0.125 (-0.078 to +0.335), `snap` C +0.118 (-0.086 to +0.325), `fallback` B +0.157 (-0.020 to +0.346), C +0.146 (-0.023 to +0.331); failure rate B +5.9 points (-29.4 to +41.2), C +17.6 (-11.8 to +47.1), `snap` B 0.0 (-29.4 to +29.4), `snap` C +5.9 (-17.6 to +29.4). Against the old classical the mean IoU differences were +0.17 and +0.16 and the failure rates -5.9 and +5.9 points (every interval also included zero). **Every interval includes zero.** Part of what the learned net seemed to gain on these photographs is now also gained by the classical detector's own fixes (mean IoU 0.624 to 0.686, silent failures 3 to 0).

Descriptive slices of the same 17 (too small to test; looked at after the fact and used for no decision), mean IoU and failure rate: `receipt-long` (12): classical `0d03275` 0.585 and 67% (old 0.497, 83%), B 0.887 and 42%, C 0.861 and 67%; `partial_frame` (7): classical 0.476 and 71% (old 0.338, 86%), B 0.847 and 57%, C 0.849 and 86%; `flatbed-single` (3): classical 0.900 and 33%, B 0.508 and 100%, C 0.511 and 100% (`snap` C 0.578).

**What can be concluded.** On 17 exactly labelled real photographs the learned net's quads are, on average, closer to the truth than the (new) classical detector's (mean IoU 0.78 to 0.80 against 0.69) and it has fewer gross misses (2 images below IoU 0.5 against 5; 6 against the old classical), most clearly on long receipts and pages cut by the frame. That is the same direction as every synthetic slice. **What cannot be concluded:** that the learned net has a lower failure rate at the 0.90 bar (10 or 12 of 17 against 9 of 17: not separated, and the direction is the opposite one; C is better than B on every synthetic measure and worse here by two images, within noise); that more synthetic training transfers; anything about calibration (the synthetic-fitted threshold auto-accepted 0 and 1 of 17 images, so the confidence does not carry over and no silent-failure claim is possible; the new classical accepts 2 of 17, both right); anything about flat documents (3 images, where the learned net loses to the classical detector). The learned errors on real photographs are mostly near misses (IoU .80 to .95 for 6 or 7 of 17) rather than gross misses, which points at precision, not detection, as the next lever. With n = 17 a failure-rate difference needs a true effect of about 30 points to separate; the gate of the first GO-IF (at least 30 labels, separated intervals) is **not met**.

### Rust path (candidate C, the exported net, 100 held-out `mid-d` images)

ONNX opset 17, fp32, static 1 x 3 x 384 x 384, 3.77 MB, operators Add, Conv, GlobalAveragePool, HardSigmoid, HardSwish, Mul, Relu, Resize (the same set as before). Run through `crates/infer` (`ort =2.0.0-rc.13` load-dynamic with the SHA-256-pinned ONNX Runtime 1.28.2, fetched with `cargo xtask fetch-ort` for this check, and `rten =0.26.0`; example `net_parity`), compared with PyTorch fp32 on CPU, then decoded with the Python decoder and with the Rust decoder (`crates/infer/src/corner.rs`):

| Check (100 images) | ort 1.28.2 | rten 0.26.0 |
|---|---|---|
| max abs difference, raw logits | 2.3e-3 | 2.0e-4 |
| max abs difference, heatmap, mask and centre as probabilities (M4.16 bar 1e-4) | **1.6e-4** (candidate B: 3.9e-5) | 1.2e-5 (B: 9.7e-6) |
| max offset difference | 0.06 px | 0.006 px |
| decoded corners, max difference | 1.7e-4 cells = 6.9e-4 px (bar: < 0.1 cell) | 2.0e-5 cells = 7.8e-5 px |
| images that decode to a different peak | 0 | 0 |

The corner bar passes on both by three orders of magnitude on 100 images (the first cycle checked 5 random inputs). The M4.16 heatmap-probability bar of 1e-4 passes for rten and for candidate B on ort and **is missed by 0.6e-4 for candidate C on ort** (a logit difference in a flat region); it moves no decoded corner. The Rust decoder agrees with the Python decoder to 7e-4 px on the same outputs. Timing (12 logical CPUs, 4 threads, network only, 100 images x 3 runs, quiet machine): ort 8.5 ms median (p95 11.1), load 188 ms; rten 19.7 ms median (p95 25.6), load 5 ms (ADR 0007 measured rten 2 to 4 times ort; here 2.3 times at 384 px). Pre- and post-processing in Rust, one thread: letterbox of a 12 MP (4032 x 3024) RGB picture to 384 px 29.5 ms, of a 2 MP picture 8.0 ms (exact area average; JPEG decode not included), decoder 0.13 ms. That is about 40 ms per 12 MP picture with ort, most of it in the letterbox, which can be parallelised or replaced by the codec's scaled decode. The Rust letterbox resamples slightly differently from OpenCV's (area average, bilinear for enlarging, no pre-shrink warp); the effect on accuracy was not measured. Not measured: int8, other operating systems, the Rust path on real pictures.

### Overfit check (M4.14: 36 images, no augmentation, 384 px)

3,000 steps (13 min wall on the shared machine): mean corner error 1.52 px (median 1.01) at the 384 px input, mean IoU 0.964, 1 of 36 below 0.90; a second run of 3,094 steps gave 1.49 px (median 1.04). In 256 px units that is 1.0 px (median 0.67), against 2.7 px (1.7) after 700 steps in the first cycle. **The bar of < 1 px mean is not met** at this budget (the loss was still falling slowly); the mean is within about 0.5 px of it and the median is at it.

### Not shown (UNMEASURED, not passes)

- Any claim on real photographs beyond 17 images, on flat documents (3 images), on several items per picture (13 labels left out; they need a multi-quad predictor) or on HEIC and other decoded inputs.
- Confidence calibration on real data (the synthetic-fitted threshold accepted almost nothing), so any silent-failure risk on real photographs.
- Run-to-run variance: one training seed per run; B and C are one continued chain, not independent runs.
- ImageNet or other pretrained initialisation (needs an owner-granted A-7 exception, not tried), a wider net, a stride-2 head, int8, and a real-photo fine-tune set (none exists; the private labels are evaluation only).
- Whether the white-mat prints and white-on-white pages the generator now draws match the owner's real prints (the truth convention for a mat is an assumption), and whether the finger blobs help on real hands (the 7 `partial_frame` images are the only related ones).
- The Rust `LearnedDetector` has no real-picture test, no multi-threaded letterbox, no mask-agreement feature (the Python confidence uses it) and no engine hook; it is a prototype behind a feature that is off by default.
- Classical numbers on the 17 images are from commit `1bb9c50`; another agent was changing `detect.rs` in parallel, so they may have moved by the time this is read.

### Revised recommendation: GO-IF (conditions narrowed)

Keep the learned detector alive as the candidate for M4, not as the shipping detector. For it: strong on synthetic data (failure 18% against 40%, no slice where it loses), directionally positive on real photographs (mean IoU +0.10 to +0.11 over the current classical detector, +0.16 to +0.17 over the one it replaced; fewer gross misses; long receipts and cut frames much better), and the Rust path works (parity, 8.5 ms). Against promoting it: the failure rate at the 0.90 bar did not separate from the classical detector on 17 photographs (and is numerically higher than the current classical detector's), the classical detector is itself improving (its fixes of 2026-10-12 closed about 35 to 40% of the mean-IoU gap on these photographs), more synthetic training did not carry over (C against B), the confidence does not transfer, and flat documents are worse. The remaining conditions replace those of the first GO-IF:

1. **Evidence.** At least 30 (better 50, golden v1) exactly labelled single-item receipts and documents on which the learned net, or a hybrid, has a lower failure rate than the then-current classical detector with a paired-bootstrap interval that excludes zero, fewer than half as many gross misses (IoU < 0.5), and is no worse on flat documents. The owner has 17 (plus 10 multi-item labels that need a multi-quad scorer); 13 more single-item labels reach 30 (about a minute each in the labeller). Effort: one to two hours of owner time, 1 day to run.
2. **Precision.** Most real misses are at IoU .80 to .95. A refinement stage (move each learned edge to the strongest edge within a small band by a local line fit, as `detect/strip.rs` does for the classical detector; the existing `snap` hybrid does this only when a classical candidate agrees, and helped on synthetic data and a little on real) is the cheapest lever. Effort: 3 to 5 days with tests. Exit: median corner error on the real set under 1.5% of the diagonal (classical 1.9% on the 16 images it answers, learned 2.7% to 4.4% now).
3. **Domain gap.** Either a real-photo fine-tune set (another 50 to 100 photographs, separate from the evaluation labels; owner decision, B21 applies) or an owner-granted A-7 exception to try a pretrained backbone (option D, untried). Without one of these, more synthetic training is not expected to move the real failure rate (B to C). Effort: 1 day of training per variant; the decision is the owner's.
4. **Confidence on real data (M4.42).** The auto-accept threshold must be fitted on real labels; until then the learned detector is Check-only. The `agree` hybrid is the only combination with a synthetic silent-failure bound (1 in 1,405 accepted, 0.34% one-sided 95% bound) and on real photographs it accepted nothing. Effort: part of M4.42; needs about 100 labels.
5. **Rust.** A parallel letterbox or scaled decode, the mask feature, a real-picture test and the engine hook; the ort heatmap-probability bar of 1e-4 either relaxed to 2e-4 by the owner (the decoded corners differ by under 0.001 px) or met some other way. Effort: 3 to 4 days.

If condition 1 fails after conditions 2 and 3 were tried once each, record NO-GO for the learned detector as the primary and keep the classical detector with the candidate-ranking work. Nothing in this update changes PLAN.md, the decision log, ROADMAP.md or what ships; no weights, datasets or checkpoints are committed (they live under `target/` on the owner's machine; the pinned ONNX Runtime fetched for the parity check is under `target/native/`).
