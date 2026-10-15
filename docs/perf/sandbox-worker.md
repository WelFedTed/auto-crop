# Added latency of the sandboxed decode worker

Measured by `crates/worker/examples/latency.rs` in `.github/workflows/worker.yml` (job "HEIC and AVIF through the worker", step "Added latency per image"), release builds, GitHub-hosted runners. **NOISY**: shared machines, 3-4 vCPUs, other jobs on the host; read them as an order of magnitude, never as a gate. Run [38711660710](https://github.com/WelFedTed/auto-crop/actions/runs/38711660710) (commit 2c063c8).

What is measured, per file (`repeat` = 10): `inproc` is the mean of in-process decodes of the same bytes (`decode_in_process`, libheif already initialised); `first` is the first decode through a fresh pool (start the worker process, confine it, load libheif and its plugins, warm up with one AVIF decode, say hello, decode); `steady` is the mean of the following decodes on the same, reused worker; `added = steady - inproc`. A pool decode also includes the header probe in the parent, the input over the pipe, the copy into shared memory and out of it, and the reply.

## 12 MP AVIF (4000 x 3000, 8-bit 4:2:0, 1.3 MB, `make_big_avif.py`)

| OS (sandbox level the worker reported) | in-process | worker, first | worker, steady | added per image |
|---|---|---|---|---|
| windows-2025 (AppContainer + job) | 138.8 ms | 770.7 ms | 176.4 ms | +37.5 ms |
| macos-latest, arm64 (Seatbelt) | 91.2 ms | 196.4 ms | 106.8 ms | +15.6 ms |
| ubuntu-22.04 (rlimit + Landlock + seccomp) | 185.1 ms | 299.1 ms | 246.2 ms | +61.1 ms |

## 12 MP HEIC (4000 x 3000, 538 KB)

| OS | in-process | worker, first | worker, steady | added per image |
|---|---|---|---|---|
| macos-latest, arm64 (the file is made by `sips`, ImageIO) | 145.5 ms | 219.1 ms | 176.2 ms | +30.7 ms |
| windows-2025, ubuntu-22.04 | not measured: the pinned libheif has no HEVC encoder (no x265, B12) and the runners have none: Windows has no `sips`, and `heif-enc` of Ubuntu is built without an HEVC encoder | | | |

The HEVC decode path through the sandbox is exercised on every OS with `rainbow-451x461.heic` (see [testing/sandbox-worker.md](../testing/sandbox-worker.md)); the fixed per-job cost on a tiny HEIC is in the last table.

## Reading the numbers

- The first decode on a machine pays process start, the AppContainer staging check and library loading: 0.1 s (macOS) to 0.8 s (Windows, where the worker and its DLLs are in a staged folder and a new process under AppContainer is slow to start). It happens once per worker, not per image; a batch starts at most `min(cores / 2, 4)` workers and reuses them (a worker is retired after 64 jobs).
- Steady state adds 15-60 ms on a 12 MP image on these runners, i.e. roughly the cost of moving 36 MB of pixels once more (worker memory to shared memory to the parent's buffer) plus the 1.3 MB input over the pipe. At the budgets of PLAN (a 12 MP HEIC overlay in 700 ms) that is a single-digit percentage; a budget gate is not claimed from CI numbers.
- The in-process column here is the decoder alone; the app's own work (analysis, proxies, encode) is the same in both modes.
