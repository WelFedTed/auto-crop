# Instruction-count gate (M1.58, X.02)

Wall-clock benchmarks on shared runners are noisy, so only instruction counts gate a change. The
gate runs [gungraun](https://crates.io/crates/gungraun) 0.20.0 (Valgrind Callgrind; Linux only;
the runner binary `gungraun-runner` must have the same version) over small fixed inputs, builds the
kernels once from the base commit and once from the head commit **in one job**, and compares the
`Ir` (instructions executed) of every benchmark. Worse by **more than 5% fails**, **more than 2%
warns**; a benchmark that disappears fails. Code: `crates/imgproc-bench/benches/gungraun.rs`,
`tools/perf_gate/compare.py` (unit-tested), `.github/workflows/perf-gate.yml`.

## What is measured

| Benchmark | Input |
|---|---|
| `decode/jpeg_256x192`, `decode/png_256x192` | `auto_crop_codecs::decode` on a generated 256 x 192 file |
| `resize/area_512x384_to_128x96` | `imgproc::scale::resize_area` |
| `warp/plain_512x384_to_256x352`, `warp_guarded/guarded_1024x768_to_256x192` | Lanczos3 warp, plain and with the minification guard |
| `otsu`, `sauvola_w31`, `nick_w31` (256 x 192 grey) | `imgproc::threshold` |
| `canary/histogram_x100` | the histogram kernel, 100 repetitions (+N% with `AUTO_CROP_GATE_CANARY_PCT=N`) |

Inputs come from fixed seeds in an unmeasured setup function. Kernels run on the calling thread
(rayon's global pool is built with one thread that is the current thread), so there is no
work-stealing noise. The same binary measured twice differs by about 300 instructions in 14 million
(0.002%), and decode by about 100 in 2.3 million; the 2% warning level is therefore far above the
noise floor.

## SIMD levels

The kernels have no hand-written SIMD and no runtime dispatch: they rely on autovectorisation (see
[kernels.md](kernels.md)), so there is no "scalar / SSE4.1 / AVX2" switch to flip in the code. A
level is the compile-time target feature set the autovectoriser may use, one matrix job each:

| Level | `RUSTFLAGS` |
|---|---|
| `baseline` (x86-64, SSE2; the scalar-equivalent floor, a truly scalar x86-64 build does not exist) | none |
| `sse4.1` | `-C target-feature=+sse4.1` |
| `avx2` | `-C target-feature=+avx2` |

When M7 adds hand-written kernels with runtime dispatch (M7.64), each dispatch level gets its own
benchmark ids and this table is replaced by the real levels. Instruction counts per level (head,
GitHub-hosted `ubuntu-24.04`, Valgrind from the Ubuntu archive, run 37117866434):

| Benchmark | baseline | sse4.1 | avx2 |
|---|---:|---:|---:|
| decode jpeg 256x192 | 2,325,252 | 2,327,905 | 2,325,640 |
| decode png 256x192 | 5,760,855 | 5,760,823 | 5,594,749 |
| resize area 512x384 to 128x96 | 10,298,182 | 10,033,854 | 9,635,547 |
| warp plain 512x384 to 256x352 | 65,048,886 | 56,487,806 | 52,503,686 |
| warp guarded 1024x768 to 256x192 | 45,994,708 | 36,602,401 | 33,484,216 |
| otsu + binarize 256x192 | 695,286 | 695,286 | 692,383 |
| sauvola w31 256x192 | 7,326,194 | 7,138,461 | 6,741,309 |
| nick w31 256x192 | 4,623,768 | 4,434,788 | 3,987,857 |

## Triggers

* `pull_request`: base = the PR's base commit, head = the PR merge commit. The job decides inside
  itself whether anything relevant changed (`imgproc`, `imgproc-bench`, `codecs`, `core`,
  lockfile, toolchain, the gate itself); the CI guard forbids a `paths:` filter on `pull_request`
  because a required check that never reports blocks docs-only PRs.
* `push` to `main`: base = the commit before the push. The owner pushes straight to `main`, so this
  is the gate that actually sees the owner's changes.
* `workflow_dispatch`: inputs `base` (a ref, default the parent) and `canary_pct`.

The benchmark harness (`crates/imgproc-bench`) is always taken from the head commit and copied over
the base checkout, so both builds run identical benchmark code against different kernels. If the
base does not compile against that harness the job fails loudly rather than skipping.

## Acceptance evidence (M1.58: an injected 10% regression fails, a no-op passes)

The owner forbids scratch branches and PRs, so the "plant a slowdown on a throwaway PR" proof is
replaced by two real runs on `main` through `workflow_dispatch` plus a unit-tested comparison
script:

| Evidence | Result |
|---|---|
| No-op: base = head = `3b5a861`, `canary_pct=0` ([run 37117866434](https://github.com/WelFedTed/auto-crop/actions/runs/37117866434)) | all three levels: 9 benchmarks, 0 failing, 0 warning; worst difference +0.002% (the canary, +301 instructions) |
| Injected +10%: same commits, `canary_pct=10` ([run 37117868259](https://github.com/WelFedTed/auto-crop/actions/runs/37117868259)) | all three levels: `canary/histogram_x100` 13,775,498 to 15,153,308 (+10.00%) FAIL; the run is green because `--expect-fail` inverts the verdict ("the injected slowdown was caught") |
| Gate logic | `python3 -m unittest discover -s tools/perf_gate`: exact thresholds (+2% and +5% boundaries do not trip, one instruction more does), +10% on one or all benchmarks fails, +3% warns and passes, improvements pass, a removed benchmark fails, a new one does not, empty or malformed input is a usage error; the same checks replay a real recorded gungraun run (`fixtures/recorded_base.jsonl`) including a derived +10% case |
| The PR path | the workflow ran on real Dependabot PRs (for example [run 37116989312](https://github.com/WelFedTed/auto-crop/actions/runs/37116989312): `bff246379` base vs `08181646d` head, 9 benchmarks, 0 failing); a Dependabot PR that broke the build of `auto-crop-codecs` failed the job as it should ([run 37117175554](https://github.com/WelFedTed/auto-crop/actions/runs/37117175554)) |

### Limits of this evidence (written down on purpose)

* The canary is a benchmark whose work is scaled by an environment variable, not a real kernel
  that got slower in a diff. It proves the comparison, the workflow plumbing, the exit status and
  the `--expect-fail` inversion end to end, and that +10% in one benchmark of nine is detected. It
  does **not** prove that a code change in `imgproc` reaches the head build through the checkout
  (the head and base trees are separate checkouts, and the Dependabot runs show the base and head
  SHAs differ and both build), nor that a PR author cannot weaken the gate by editing
  `compare.py` in the same PR (the script is taken from the head checkout; a required-check setup
  must pin it, and branch protection is the owner's decision).
* "Required check" status is not set: the owner pushes to `main`, so no PR blocks on this job
  today; a regression on `main` shows as a red Perf gate run.
* Valgrind counts instructions, not time: a change that trades instructions for cache misses or
  memory stalls is not seen. Wall-clock stays in the nightly (M1.59) and the criterion benches.
