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
