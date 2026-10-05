# Measured stage budgets (M1.54, M1.60, M1.63)

Roadmap items: M1.54 (pipeline skeleton, `auto-crop dev-pipeline`), M1.60 (memory profile, batch throughput), M1.63 (this table). Budgets: [PLAN 7.1](../plan/07-performance-accuracy-quality.md) Table A and B, all PROVISIONAL. Code: `crates/engine/src/skeleton.rs`, harness `xtask/src/perf/`. Other measurements: [kernels.md](kernels.md), [hardware.md](hardware.md).

## Read this first: every number below is NOISY

| | |
|---|---|
| Host | **LUNCHBOX, desktop, indicative** ([hardware.md](hardware.md)): Intel Core i7-8700K (6 cores / 12 threads, AVX2), 31.9 GB, Windows 11 Pro 10.0.26200, no battery (mains), power plan *Ultimate Performance* |
| Build | `cargo build --release` profile of this repository; the tables below were measured at `74a7dce` (the earlier classical detector), the addendum after Table A at `bc93fba` (the faster line-based detector of `3937492` and `15fd429`); rustc 1.98.1; **LTO off, 16 codegen units, no `target-cpu`** (the "shipped release profile" of PLAN 7.2 is not defined yet) |
| Load | **Not idle for the whole session.** An unrelated `ffmpeg` (libx265, `-preset slow`, about 9 of the 12 threads) plus one Python process kept the machine at 98-100% CPU before every run (the harness prints the global counter; the Windows `Processor(_Total)` counter agreed). During the runs the other processes used 45-90% of all CPU, and the share fell as the number of benchmark threads grew (the scheduler gives more to more threads), so every multi-thread number below is pessimistic by an unknown and *varying* factor. Same binary, same image, 12 MP, all threads, minutes apart: total p50 **579 ms** in the final run and **1069 ms** in the first |
| What to trust | Single-thread columns (p50 within 5% of the minimum, but still sharing cores with the SMT siblings of a busy machine), heap peaks (exact, load-independent), the stage-sum check (a ratio), ratios between stages of the same run. Not the absolute milliseconds, not the scaling efficiency, not the img/s |
| Label | Every verdict is printed with `[NOISY: re-measure idle]` by the harness. Nothing here claims a hardware-tier gate (**M1.61 is not claimed**: it needs an idle Tier-M run) or the 90 ms warp gate (M2.15) |
| Inputs | **Synthetic stand-ins**, never committed: 4:3 page-on-a-desk photos from `engine::skeleton::bench_images` (index-seeded, q92). A 12 MP file is **1.8 MB (0.15 bytes per pixel)**; a real photo is 3-6 MB, so `decode` and `read_probe` are optimistic. The file cache is warm (no cold NVMe reads). The real corpus is M0.48/M1.53 |

Reproduce (release build, otherwise refused):

```
cargo run --release -p xtask -- perf host
cargo run --release -p xtask -- perf stages --mp 12 --runs 30 --threads all     # or --threads 1, --enhance sauvola
cargo run --release -p xtask -- perf stages --mp 48,100 --runs 10
cargo run --release -p xtask -- perf memory --mp 12,48,100
cargo run --release -p xtask -- perf batch --mp 12 --count 200 --workers 1,2,3,4,5,6,7,8
cargo run --release -p xtask -- perf batch --pass-images 40 --rounds 5      # interleaved rounds
cargo run --release -p auto-crop-cli -- dev-pipeline photo.jpg --timings    # one image, one table
```

## What is measured

`engine::skeleton` chains `read_probe`, `decode`, `proxy`, `analyse`, `rectify`, `enhance` and `encode` as `tracing` spans named like Table A, each emitting `stage_done{stage, ms, px}`. Stand-ins: **`analyse` is the existing classical detector on the 1024 px proxy** (not the corner net, orientation or fusion of M2/M4), **`enhance` is a prototype** (luma plus Otsu, or Sauvola window 51 for the B&W variant), **`encode` is the `image` crate JPEG encoder at q90**. Table A lines **not run**: `refine` (10 ms), `commit` (100 ms, the safe-write path of B3) and the 75 ms slack, so the seven stages are compared with **515 ms**, not 700 ms. Tests (`cargo test -p auto-crop-engine skeleton`, `-p auto-crop-cli`) prove the span order, that every stage reports, that the spans sum to the total, the same bytes at 1, 3 and 8 threads, cancellation after every stage, a deadline and the error paths.

## Table A, 12 MP, measured (all NOISY)

30 runs after 3 warm-ups. **Lone job, all 12 threads** is what Table A budgets (PLAN 7.3: a lone job runs strip-parallel). Output 2663 x 2434 (the page covers about half the frame). Ratio is p50 over budget; **> 1.5x is flagged "REDESIGN" as the plan requires**, with the caveat that these runs were NOISY.

| Stage | Budget ms | min | p50 | p95 | p50 / budget | Verdict |
|---|---:|---:|---:|---:|---:|---|
| `read_probe` | 10 | 1.9 | 2.2 | 3.2 | 0.22 | within (warm cache, small file) |
| `decode` | 120 | 100.6 | 105.2 | 109.8 | 0.88 | within (optimistic input, see above) |
| `proxy` | 25 | 21.9 | 26.8 | 41.0 | 1.07 | over budget, within 1.5x |
| `analyse` (STAND-IN) | 40 | 105.7 | 112.8 | 122.1 | **2.82** | **REDESIGN** (stand-in only; p95 ceiling 40 ms also missed: 122 ms) |
| `rectify` | 90 | 72.3 | 80.1 | 90.7 | 0.89 | within on this noisy host; the M2.15 gate is **not** claimed |
| `enhance` (PROTOTYPE, Otsu) | 120 | 6.4 | 8.1 | 15.1 | 0.07 | not representative (the Auto/Grayscale flatten is M7) |
| `enhance` (PROTOTYPE, Sauvola, B&W variant) | 250 | 27.3 | 36.1 | 62.1 | 0.14 | not representative |
| `encode` | 110 | 211.1 | 232.7 | 271.4 | **2.12** | **REDESIGN** (single-thread `image` encoder; see below) |
| **sum of the seven stages** | **515** | | 575.1 | | 1.12 | over, within 1.5x |
| **total wall** | | | 578.5 | 612.8 | | |

- **Stage-sum check: PASS.** Total minus the sum of the stage spans is 2.8-4.7 ms at the median at 12 MP (and 9 and 19 ms at 48 and 100 MP; 0.2-0.7% of the total in every configuration, 36 ms worst), so the spans account for the run and nothing hides between them.
- **Table A projected, not measured:** 579 ms measured for the seven stages plus the 185 ms of unmeasured budget (`refine`, `commit`, slack) is about 764 ms against 700 ms (1.09x). That is arithmetic, not a measurement, and the Table A total stays PROVISIONAL.
- **Single thread (batch mode: one image per worker)**, same run protocol: `read_probe` 2.3, `decode` 105.4, `proxy` **210.6**, `analyse` 120.4, `rectify` **616.4**, `enhance` 29.1, `encode` 215.8; sum 1299 ms, total p50 1302 ms, p95 1321 ms. These have no Table A budget (Table A is the all-thread case); they are the CPU cost of one image, compared with the plan's batch breakdown below.

### Addendum: `analyse` after the detector rewrite (`bc93fba`, same host, NOISY)

The upstream detector commits vectorised the blur and morphology and added a line-based page finder, which changes the stand-in row. Re-measured, 30 runs: lone job (all threads) `analyse` min 45.0, **p50 53.4**, p95 58.2 ms, **1.34x** the 40 ms budget (within 1.5x, no redesign flag; the p95 ceiling of 40 ms is still missed at 1.46x); single thread p50 64.4 ms (1.61x). The other stages did not move (decode 101, proxy 24, rectify 72, encode 214 ms all threads; total p50 **478 ms**, p95 503 ms against the 515 ms chained budget, 0.93x; single-thread total 1217 ms). The tables above and below (Table A, the CPU table, Table B, the batch run) were all taken with the earlier detector, so their `analyse` and total figures are about 60 ms too high; nothing else in them depends on it.

### Analysis stand-ins (M1.55): STAND-IN, NOISY

Equivalent analysis work timed on the same 1024 px detection proxy (786,432 px) of a synthetic 12 MP page-on-a-desk photo, **every row a STAND-IN until M2 (classical analysis) and M4 (the real corner net)**: no accuracy or M4 claim is made. Code: `engine::skeleton::standin`, `crates/infer`, harness `perf standin` ([ADR-0007](../adr/0007-inference-backend.md) update 2026-10-05).

```
cargo xtask fetch-ort                     # pinned ONNX Runtime 1.28.2, hash-checked, no compiler needed
cargo xtask make-standin-net              # random-weight 256x256 MobileNetV3-class net, ~2.7 MB, never committed
cargo run --release -p xtask --features standin-ort,standin-rten,standin-canny -- perf standin --runs 40 --warmup 8
cargo run --release -p auto-crop-cli --features standin-ort,standin-rten,standin-canny -- dev-pipeline photo.jpg --timings --analyse standin-net
```

Host and conditions: LUNCHBOX (i7-8700K, 6C/12T, Windows 11), release profile (LTO off, no `target-cpu`), 40 runs after 8 warm-ups, commit `0f9e8d6`. **NOISY**: other agents were compiling and a browser was open; the harness measured other processes at **47.6% of all CPU before and 63.9% during** (an earlier run the same day at 83-100% load gave 1.5-3x worse numbers). Net rows: the net sits behind the `InferenceBackend` trait; `inference only` is `run` on a prepared tensor, `analysis stage` adds the squash to 256x256 (area average), ImageNet normalisation and corner decode (`StandinNet::analyse`, what `analyse` runs). Threads are the rayon pool size for the kernels and the intra-op thread count of the backend.

| What (STAND-IN) | Backend | Threads | min ms | p50 ms | p95 ms | p95 / 40 ms |
|---|---|---:|---:|---:|---:|---:|
| classical detector | - | 1 | 47.2 | 65.8 | 90.5 | 2.26 |
| classical detector | - | 4 | 41.7 | 46.8 | 55.1 | 1.38 |
| Canny + contours (imageproc 0.27) | - | 1 | 75.0 | 91.1 | 130.4 | 3.26 |
| Canny + contours (imageproc 0.27) | - | 4 | 58.9 | 68.7 | 95.3 | 2.38 |
| **ONNX net, inference only** | ort 1.28.2 | 1 | 6.1 | **9.8** | **12.4** | 0.31 |
| **ONNX net, inference only** | ort 1.28.2 | 4 | 2.4 | **3.3** | **4.3** | 0.11 |
| ONNX net, analysis stage | ort | 1 | 10.8 | 14.4 | 16.2 | 0.40 |
| ONNX net, analysis stage | ort | 4 | 3.8 | 5.1 | 6.1 | 0.15 |
| same net, inference only | rten 0.26.0 | 1 | 25.4 | 37.8 | 44.9 | 1.12 |
| same net, inference only | rten | 4 | 6.7 | 10.0 | 11.9 | 0.30 |
| same net, analysis stage | rten | 1 | 26.8 | 34.7 | 44.4 | 1.11 |
| same net, analysis stage | rten | 4 | 8.2 | 11.6 | 14.4 | 0.36 |

- **The `analyse` budget row (40 ms p50 and p95) is met by the net stand-in with ort** (p95 6 ms analysis stage at 4 threads, 16 ms at 1 thread) and by rten at 4 threads (14 ms); rten at one thread is 1.1x over. That is a stand-in with random weights at 256x256: it says what a MobileNetV3-class pass costs, nothing about the real corner net (M4), the orientation net, fusion or refinement. Table A's analysis line stays PROVISIONAL.
- ort and rten agree on the net output to **3.9e-7** (bar 1e-3; ADR-0007 measured 5e-7); the output is identical at 1 and 4 threads on both. Tests: `cargo test -p auto-crop-infer --features ort,rten`.
- Canny + contours is the slowest stand-in (2.4-3.3x of the p95 ceiling on this loaded host; not profiled); it is a crude page finder and is not a candidate for the shipped analysis, only a cost reference for edge-plus-contour work.
- Full pipeline, 12 MP, all threads, 20 runs (NOISY, 28-35% other load): `--analyse standin-net` (ort, 4 threads) `analyse` p50 **4.5** ms, p95 5.9 ms; `--analyse standin-canny` p50 **61.9** ms, p95 70.6 ms (1.55x: flagged REDESIGN by the harness's rule, for a stand-in that is not shipped). With the net stand-in the corners are noise, the full frame is rectified (4000x3000 output) and the downstream rows are not comparable with a cropped run: only the `analyse` row is meaningful there. Stage-sum check passes (0.8-0.9% glue).
- Not measured here: Linux and macOS numbers (the `Analysis stand-ins` workflow prints the same table on all three OSes into its step summary, NOISY shared runners), the int8 net, GPU providers, a loaded-machine-free Tier-M run (M1.61).

### CPU per image against the plan's batch breakdown (single thread, NOISY)

PLAN 7.1 assumes about 760 CPU-ms per 12 MP image (decode 120, proxies 40, analysis 60, refine 10, warp 200, enhance 200, encode 110, commit 20) and tolerates 875 ms for the CLI floor.

| Stage | Plan CPU ms | Measured CPU ms (1 thread) | Ratio |
|---|---:|---:|---:|
| decode | 120 | 105-115 | 0.9 |
| proxies | 40 | 211-230 | **5.3-5.8** |
| analysis | 60 | 120-133 | **2.0-2.2** |
| warp | 200 | 616-655 | **3.1-3.3** |
| enhance | 200 | 29-32 (prototype) | n/a |
| encode | 110 | 216-246 | **2.0-2.2** |
| refine, commit | 30 | not run | n/a |
| **Sum of what ran** | **730** | **1299-1413** | **1.8-1.9** (against the plan's full 760 ms: 1.7-1.9x) |

All of these are SMT-inflated and NOISY, but they are consistent with [kernels.md](kernels.md) (single-thread warp 815 ms for 8.7 MP on the same host) and they point at a plan inconsistency that load does not explain: **a `rectify` that takes 90 ms wall on 8 threads costs about 700 CPU-ms**, so the 200 CPU-ms warp line of the batch breakdown cannot hold unless the single-thread kernel gets about 3-4x faster. The **batch CPU cost is therefore flagged as needing a redesign or a budget change** (owner decision; this document does not relax anything): candidates are a SIMD proxy resize (`fast_image_resize`, recommended in kernels.md, 4-11x faster per thread), an AVX2 warp (kernels.md next step), a turbojpeg encoder (M1.21/M1.57) and a cheaper stand-in-free analysis.

## Table B, size scaling (all threads, 10 runs, NOISY)

Rectified output is 25.9 MP (48 MP source) and 53.9 MP (100 MP source). Table B states total budgets only, so there are no per-stage verdicts at these sizes.

| Source | read_probe | decode | proxy | analyse (STAND-IN) | rectify | enhance (Otsu) | encode | total p50 | total p95 | Table B p50 | Ratio |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 12 MP | 2.2 | 105 | 27 | 113 | 80 | 8 | 233 | 579 | 613 | 700 | 0.83 |
| 48 MP | 7.3 | 403 | 44 | 114 | 288 | 22 | 866 | 1740 | 1846 | 2500 | 0.70 |
| 100 MP | 12.2 | 640 | 86 | 69 | 581 | 43 | 1148 | 2698 | 3782 | 5000 | 0.54 |

The Table B totals also hold `refine`, `commit` and slack (not run here), so a ratio below 1 is not yet a pass. The analysis row does not grow with the megapixel count (it works on the 1024 px proxy: 786,432 px at every size; the differences between 113, 114 and 69 ms are load). `encode` is the largest stage at every size and grows with the output pixels; at 100 MP its p95 (1.9 s) shows how noisy a long single-threaded stage is.

## Memory profile (M1.60): exact, load-independent

Peak heap above the heap in use when the run starts, from the counting allocator (`xtask/src/alloc_count.rs`), per stage, against the job weight the `MemoryBudget` admits a job with: `pixels * 9 + 64 MiB` (3 x decoded RGB8 + 64 MB: 175 / 499 / 967 MB, PROVISIONAL). "Fills frame" is the worst case: the page covers the frame, so the rectified output is about source-sized. The same figures are asserted by `crates/engine/tests/skeleton_memory.rs` (12 MP by default, 48 and 100 MP with `--ignored`, using the safe `peak_alloc` crate).

| MP | Page | Threads | Peak heap MB | Budget MB | Ratio | Verdict | Peak stage |
|---:|---|---:|---:|---:|---:|---|---|
| 12 | on a desk | 1 / all | 58.9 / 59.0 | 175.1 | 0.34 | within | `analyse` |
| 12 | fills frame | 1 / all | 66.9 / 67.6 | 175.1 | 0.38-0.39 | within | `rectify` |
| 48 | on a desk | 1 / all | 221.8 / 222.7 | 499.1 | 0.44-0.45 | within | `rectify` |
| 48 | fills frame | 1 / all | 267.4 / 268.7 | 499.1 | 0.54 | within | `rectify` |
| 100 | on a desk | 1 / all | 461.9 / 463.3 | 967.1 | 0.48 | within | `rectify` |
| 100 | fills frame | 1 / all | 557.1 / 559.0 | 967.1 | 0.58 | within | `rectify` |

Per-stage peaks (MB, 12 MP fills frame, 1 thread): `read_probe` 1.8, `decode` 39.8, `proxy` 52.1, `analyse` 58.9, `rectify` 66.9, `enhance` 66.9, `encode` 31.5; the 100 MP row is 13, 327, 316, 323, 557, 557, 262. **Finding:** the measured peak is 0.34-0.58 of the weight at every size, so `pixels * 9 + 64 MiB` over-reserves by about 1.7-3x; the admission is safe but conservative, and the plan's "<= 175 MB / 500 MB / 1.0 GB per in-flight image" holds with room. The extra heap is dominated by `decode` (one RGB8 raster) plus the rectified output; threads add nothing measurable (under 2 MB).

## Batch throughput (M1.60), 200 x 12 MP, NOISY and not a scaling measurement

200 deterministic synthetic JPEGs (4000 x 3000, 365 MB in total, never committed), N workers, one image per worker with every kernel on a private one-thread pool (no nesting), jobs admitted by a `MemoryBudget` (cap 3.5 GiB here, 166 MiB per job), no `commit`. Efficiency is images/s over N times the one-worker images/s; the plan assumes 70%. The output digest was **identical at every worker count and round**, and no image failed.

| Workers | images/s | Efficiency | Peak RSS MB | Budget peak in use MB |
|---:|---:|---:|---:|---:|
| 1 | 0.70 | 100% | 79 | 175 |
| 2 | 1.37 | 98% | 137 | 350 |
| 3 | 2.06 | 98% | 195 | 525 |
| 4 | 2.57 | 92% | 250 | 700 |
| 5 | 3.53 | 101% | 309 | 876 |
| 6 | 3.45 | 82% | 360 | 1051 |
| 7 | 3.58 | 73% | 414 | 1226 |
| 8 | 4.36 | 78% | 460 | 1401 |

A second protocol is less sensitive to drift: 40 images per pass, 5 rounds interleaved over the worker counts (images/s median / best): 1 worker 0.74 / 0.76, 2: 1.32 / 1.52, 3: 1.74 / 2.16, 4: 2.66 / 2.94, 5: 3.46 / 3.60, 6: 4.05 / 4.13, 7: 4.73 / 4.82, 8: 4.77 / 5.46; peak RSS 82 MB at one worker growing by about 55 MB per worker to 486 MB at eight.

- **The 70% efficiency is NOT established and NOT refuted.** Efficiencies of 73-101% look better than the plan's assumption, but they are an artefact of the load: the machine was saturated by `ffmpeg`, and every extra benchmark worker took CPU share from it (the other-process load fell from 89% at one worker to 45-60% at eight). That measures scheduler fair share, not core scaling. The measurement must be repeated on an idle machine (M1.61).
- **Floors, for the record (not verdicts):** CLI, 5 workers on 6 cores: **3.5 images/s** (median 3.46, best 3.60) against **>= 4**, which is 0.86-0.90x of the floor, i.e. below it even though this run took CPU from a competitor; GUI, 4 workers: **2.57-2.94** against **>= 3**. Neither is more than 1.5x off, so neither trips the redesign rule on this evidence, but both are at risk, and the 1.7-1.9x CPU-per-image finding above says why: the plan's 4 images/s needs at most 875 CPU-ms per image and the measurement is 1300-1400.
- **Peak RSS** is reliable (it does not depend on CPU share): about 80 MB for one worker and 55 MB per extra worker, 460-486 MB at eight, i.e. well inside the 166 MiB per-job weight the budget reserves (1.4 GB admitted at eight workers).

## Verdict summary

| Finding | Evidence | Status |
|---|---|---|
| `analyse` 2.8x over its 40 ms budget at `74a7dce` (p95 122 ms); 1.34x (p95 58 ms) at `bc93fba` | classical detector stand-in, NOISY | was **REDESIGN flagged**, now within 1.5x; still a stand-in (the M2/M4 analysis lines are unmeasured) |
| `encode` 2.1x over 110 ms | `image` JPEG encoder, single-threaded, NOISY | **REDESIGN flagged** (turbojpeg encoder, M1.21/M1.57) |
| Batch CPU per image 1.7-1.9x of 760 ms (proxies 5x, warp 3x) | single-thread stage sums, SMT-inflated | **REDESIGN or budget change flagged** (owner) |
| `proxy` 1.07x with all threads, 8x single-threaded | own area resize (kernels.md) | within 1.5x only because of 12 threads; SIMD resize recommended |
| `rectify` 0.89x with all threads, 6.8x single-threaded | warp kernel, M2.15 gate not claimed | needs the idle-machine run |
| Memory 0.34-0.58 of `pixels * 9 + 64 MiB` | exact | **within budget** at 12, 48 and 100 MP |
| Stage-sum check | 0.2-0.7% glue | **PASS** |
| >= 4 img/s CLI, >= 3 GUI, 70% efficiency | NOISY | **inconclusive, at risk** |

## Not measured (stay PROVISIONAL)

`refine` (10 ms), `commit` (100 ms), scheduling slack (75 ms), the real analysis lines (corner net 10-25 ms, orientation, fusion), the enhancement flatten (M7), first paint and first overlay (<= 150 ms p95: needs a scaled JPEG decode, which zune-jpeg lacks, and Tauri IPC), HEIC, the GUI batch with its preview pool, T2 escalation, Tier-L (budgets x2), cold-cache reads, real photographs (0.15 versus 0.4-0.5 bytes per pixel), the Apple-silicon and Tier-M baselines (M1.61, M1.62), and anything at 24 MP. None of the PLAN 7.1 numbers has been replaced by these measurements: they are bounds from a loaded desktop.
