# Added latency of the sandboxed decode worker

Measured by `crates/worker/examples/latency.rs` in `.github/workflows/worker.yml` (job "HEIC and AVIF through the worker", step "Added latency per image"), release builds, GitHub-hosted runners. **NOISY**: shared machines, 3-4 vCPUs, other jobs on the host; read them as an order of magnitude, never as a gate. Run [38789975407](https://github.com/WelFedTed/auto-crop/actions/runs/38789975407) (commit f0cfd96). The earlier run [38711660710](https://github.com/WelFedTed/auto-crop/actions/runs/38711660710) agrees within the noise (below).

What is measured, per file (`repeat` = 10, 20 for the small file): `in-process` is the mean of in-process decodes of the same bytes (`decode_in_process`, libheif already initialised); `first` is the first decode through a fresh pool (start the worker process, confine it, load libheif and its plugins, warm up with one AVIF decode, say hello, decode); `steady` is the mean of the following decodes on the same, reused worker; `added = steady - in-process`. A pool decode also includes the header probe in the parent, the input over the pipe, the copy into shared memory and out of it, and the reply.

## 12 MP AVIF (4000 x 3000, 8-bit 4:2:0, 1.3 MB, `make_big_avif.py`)

| OS (sandbox level the worker reported) | in-process | worker, first | worker, steady | added per image | earlier run |
|---|---|---|---|---|---|
| windows-2025 (AppContainer + job) | 83.6 ms | 731.8 ms | 114.9 ms | **+31.4 ms** | +37.5 ms |
| macos-latest, arm64 (Seatbelt) | 102.2 ms | 171.0 ms | 110.9 ms | **+8.7 ms** | +15.6 ms |
| ubuntu-22.04 (rlimit + Landlock + seccomp) | 143.5 ms | 225.3 ms | 190.3 ms | **+46.9 ms** | +61.1 ms |

## 12 MP HEIC (4000 x 3000, 538 KB)

| OS | in-process | worker, first | worker, steady | added per image |
|---|---|---|---|---|
| macos-latest, arm64 (the file is made by `sips`, ImageIO) | 215.7 ms | 531.3 ms | 265.3 ms | **+49.6 ms** (earlier run +30.7 ms) |
| windows-2025, ubuntu-22.04 | not measured: the pinned libheif has no HEVC encoder (no x265, B12) and neither runner has one (Windows has no `sips`; the `heif-enc` of Ubuntu is built without an HEVC encoder) | | | |

## A tiny HEIC (`rainbow-451x461.heic`, 7 KB, HEVC through the libde265 plugin): the fixed cost of a job

| OS | in-process | worker, first | worker, steady | added per image |
|---|---|---|---|---|
| windows-2025 | 3.6 ms | 521.3 ms | 3.9 ms | +0.3 ms |
| macos-latest | 4.3 ms | 116.1 ms | 6.7 ms | +2.4 ms |
| ubuntu-22.04 | 3.2 ms | 8.7 ms | 3.3 ms | +0.1 ms |

(Linux "first" is small because the first number there was measured after the 12 MP file warmed the page cache; the worker start itself is the AVIF row's difference.)

## Reading the numbers

- The first decode on a machine pays process start, the staging check and library loading: 0.1 s (macOS) to 0.7 s (Windows, where the worker and its DLLs sit in a staged folder and starting a process under AppContainer is slow). It happens once per worker, not per image; a batch starts at most `min(cores / 2, 4)` workers and reuses them (a worker is retired after 64 jobs).
- Steady state adds roughly 10-60 ms on a 12 MP image on these runners (about the cost of moving 36 MB of pixels once more: worker memory to shared memory to the parent's buffer, plus the input over the pipe), and a fraction of a millisecond to a few milliseconds on a small file. At the budgets of PLAN (a 12 MP HEIC overlay in 700 ms) that is a single-digit percentage; a budget gate is not claimed from CI numbers.
- The in-process column is the decoder alone; the app's own work (analysis, proxies, encode) is the same in both modes. The 12 MP HEIC on Windows and Linux, a real-device HEIC corpus and the Surface are not measured.
