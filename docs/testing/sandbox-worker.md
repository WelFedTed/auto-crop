# The sandboxed decode worker: what CI proved, per OS

Decision record: [ADR-0013](../adr/0013-decode-worker-pool.md) (design), [ADR-0006](../adr/0006-decode-sandbox.md) (the spike). Code: `crates/sandbox/`, `crates/worker/`. Workflow: `.github/workflows/worker.yml` (manual or on a push that touches those crates). Latency: [../perf/sandbox-worker.md](../perf/sandbox-worker.md).

## Running it yourself

```
cargo test -p auto-crop-sandbox                                   # protocol, tiers, notices, cancel scope
cargo test -p auto-crop-worker --features escape-tests -- --nocapture --test-threads=2
cargo test -p auto-crop-worker --features heif,escape-tests ...   # with libheif (after cargo xtask build-native)
auto-crop doctor                                                  # the level of the worker beside the executable
AUTOCROP_DECODE=inproc|worker|auto auto-crop ...                  # default auto
```

The test builds need `--features escape-tests`; a shipped worker is built without it. Windows runs the AppContainer for real on a developer PC (no administrator needed); the profile `autocrop.decode.worker` and `%LOCALAPPDATA%\AutoCrop\worker-stage` are created on first use.

## The tests

| File | What it proves |
|---|---|
| `crates/sandbox/src/proto.rs` (unit) | frames, `Hello`, jobs and replies round-trip; wrong magic or version, an unknown kind, a length of 2^63, short streams and every truncation of a reply are typed errors; text fields are cut, cleaned and bounded |
| `crates/worker/tests/escape.rs` | the real worker, built with `escape-tests`, attempts (after its sandbox is applied) to open a user-private file by path, connect to a listening TCP port, start a process and allocate 1 GiB, and reports each; the expectation per OS and level is asserted (table below) |
| `crates/worker/tests/pool.rs` | pixels, ICC, orientation, notices and frame counts equal the in-process decode for PNG, JPEG, WebP (and every committed AVIF fixture with `heif`); typed errors; a worker is reused, shared by 8 threads, recycled after 64 jobs; the worker proves its confinement from inside (`SANDBOX probes=`); fault injection: abort, exit, garbage on the control channel (typed `WorkerCrashed`, worker replaced, next file fine), injected panic (typed, worker survives), hang (`DecodeTimeout` at the deadline), allocation past the limit (`MemoryLimit`), cancellation (kills the worker, `Cancelled`), 240 decodes with random faults on 3 threads (no leak, no orphan) |
| `crates/worker/tests/installed.rs` | the pool installed as the `codecs` remote decoder (`AUTOCROP_DECODE=worker`): `decode_with`, `decode_scaled`, `decode_flattened_with` agree with in-process; parent-side limits refuse a 60000 x 60000 header without involving the worker; the 128-file hostile corpus; the fuzz regression files; with `heif`, 1561 truncated and byte-flipped copies of the HEIF fixtures: no `WorkerCrashed`, no `InternalPanic`, no parent crash |
| `crates/worker/tests/guard.rs` | a worker built without `escape-tests` contains none of the hook strings (`AUTOCROP_ESCAPE`, `AUTOCROP-FAULT:`, `--no-landlock`) and reports `escape-tests=off`; no other manifest enables the feature. `worker.yml` repeats the check on the **release** binary, and the package scripts and smoke tests on the **shipped** one |

## Escape-test expectations and results

Expectations (copied from the spike, ADR-0006). Results: `worker.yml`, run [38789975407](https://github.com/WelFedTed/auto-crop/actions/runs/38789975407) (commit f0cfd96), all jobs green; the same rows were green in the earlier runs [38711660710](https://github.com/WelFedTed/auto-crop/actions/runs/38711660710) and [38709310953](https://github.com/WelFedTed/auto-crop/actions/runs/38709310953). Every cell below was printed by the worker (`ESCAPE ...` lines); `blocked` is what the test asserts for a level that blocks it.

| OS and level (as reported by the worker) | file | tcp | spawn | alloc 1 GiB |
|---|---|---|---|---|
| windows-2025, `appcontainer(capabilities=0)+job(active-process=1,memory=512MiB,kill-on-close=yes)` | blocked (PermissionDenied) | blocked (TimedOut: loopback is dropped for an AppContainer) | blocked | blocked |
| windows-2025, `restricted-token+job(active-process=1,...)` (forced) | blocked | **allowed** (a restricted token does not touch the network; recorded, not claimed) | blocked | blocked |
| windows-2025, `token=unrestricted+job(active-process=1,...)` (job only, forced: the control) | **allowed** | **allowed** | blocked | blocked |
| ubuntu-22.04, `rlimit(as=512MiB,cpu=3600s,core=0)+landlock=FullyEnforced+seccomp(43 syscalls)` | blocked | blocked | blocked | blocked |
| ubuntu-22.04, `...+landlock=unavailable(disabled by the test hook)+seccomp(43 syscalls)` (Landlock absent) | blocked | blocked | blocked | blocked |
| ubuntu-24.04, ubuntu-22.04-arm, fedora (digest-pinned container) | same two rows, all blocked (runs 38709310953 and 38711660710, jobs green) | | | |
| macos-latest (macOS 26.6.2, arm64), `sandbox_init(deny default; allow sysctl-read)+rlimit(cpu=3600s,core=0)+parent-rss-watchdog` | blocked | blocked | blocked | **allowed** (the kernel does not enforce `RLIMIT_AS`; not claimed. The memory limit is the parent's resident-size watchdog, shown by `running_out_of_memory_is_a_memory_limit_error_and_the_app_goes_on` on the same runner) |

The control row proves the tests can pass when nothing blocks: job-only shows `file allowed` and `tcp allowed`.

Self-probe lines (what the worker saw from inside after applying its sandbox, `SANDBOX probes=`): Windows AppContainer `file=denied(PermissionDenied) net=timeout spawn=denied(PermissionDenied)`; Linux `file=denied(PermissionDenied) net=denied(PermissionDenied) spawn=denied(PermissionDenied)`; macOS the same as Linux. The Windows probe lists the user's profile folder (a system file like `hosts` is readable by every AppContainer and proves nothing).

## Real HEIC and AVIF through the sandbox (libheif 1.23.5, dav1d, libde265)

`worker.yml`, job "HEIC and AVIF through the worker" (native libraries from `cargo xtask build-native`, cached):

| OS | `auto-crop doctor` line printed by the CLI (the level the worker reported) | HEIF test results |
|---|---|---|
| windows-2025 | `sandbox:   appcontainer(capabilities=0)+job(active-process=1,memory=2549MiB,kill-on-close=yes) [full]`; `confinement attempt that worked: appcontainer+job`; `checked from inside the worker: file=denied(PermissionDenied) net=timeout spawn=denied(PermissionDenied)` | 1561 truncated or flipped HEIF files through the worker, 0 crashes; hostile corpus 128 files, 0 crashes, 0 timeouts (slowest 1.96 s); `dev-pipeline` decodes the committed AVIF (48x32) and `rainbow-451x461.heic` (HEVC through the libde265 plugin) |
| macos-latest (arm64) | `sandbox:   sandbox_init(deny default; allow sysctl-read)+rlimit(cpu=3600s,core=0)+parent-rss-watchdog [full]`; `checked from inside the worker: file=denied(PermissionDenied) net=denied(PermissionDenied) spawn=denied(PermissionDenied)` | same: 1561 mutations, 0 crashes; hostile corpus 0 crashes (slowest 1.29 s); the same two real files decode |
| ubuntu-22.04 (kernel 6.8, LSMs `landlock,yama,apparmor`) | `sandbox:   rlimit(as=2549MiB,cpu=3600s,core=0)+landlock=FullyEnforced+seccomp(43 syscalls) [full]`; `confinement attempt that worked: self-confining worker`; `checked from inside the worker: file=denied(PermissionDenied) net=denied(PermissionDenied) spawn=denied(PermissionDenied)` | 1561 truncated or flipped HEIF files, 0 crashes (806 through the worker, the rest refused by the parent's checks); hostile corpus 0 crashes, 0 timeouts (slowest 3.2 s); the same two real files decode (HEVC through the libde265 plugin) with Landlock and seccomp applied; `strace -f` over a 12 MP AVIF, the HEIC and an alpha AVIF: every denied call is an escape probe (`openat` of the probe file and of `/dev/null` for the spawn, `socket`) or `clone3` answered `ENOSYS`; none is a call libheif, dav1d or libde265 needed |

With `AUTOCROP_DECODE=inproc` the same `doctor` prints `sandbox:   none [none]` and `worker: ... (NOT in use ...)`: the level is not assumed.

The Windows job ran the AppContainer from a worker staged out of `D:\a\...\target\release` (an ordinary user folder an AppContainer cannot read): staging is what makes it start.

## What is not verified

- Real hardware: no Surface, Windows 10, real Mac, macOS 12, Linux desktop or distribution kernel other than the runners' (ubuntu 22.04 and 24.04 x86_64, ubuntu 22.04 arm64, fedora container) was used. The ARM Linux runner also passed with the shared allow-list.
- A restricted token leaves the network open; the job-only level leaves files and the network open. Both are fallbacks that the worker reports, never silent.
- The memory limit on macOS is a 20 ms polling watchdog of the parent; a worker can briefly exceed it. `RLIMIT_AS` is not available there.
- No libheif or dav1d vulnerability was exploited to test the sandbox itself: the escape tests show that the confinement blocks what a compromised worker would try, with the real worker binary, not that every path is closed.
- Seccomp denies with `EPERM`, not by killing the process; both stop the call (ADR-0013).
- The `AUTOCROP_DECODE=worker` route of JPEG, PNG, TIFF and WebP is tested, but only HEIC and AVIF are routed by default.
- `--require-sandbox` and a setting to refuse unsandboxed HEIC are not built (M6.54, M6.57).

## Packaged layout (Linux), run [38789999789](https://github.com/WelFedTed/auto-crop/actions/runs/38789999789)

`package-linux.yml` unpacked the tarball, the AppImage (extract-and-run) and the installed `.deb`; `packaging/smoke-cli.sh` ran `auto-crop doctor` from each, with every decode under `AUTOCROP_DECODE=worker` (never falls back): all three printed `sandbox:   rlimit(as=2549MiB,cpu=3600s,core=0)+landlock=FullyEnforced+seccomp(43 syscalls) [full]` and `checked from inside the worker: file=denied(PermissionDenied) net=denied(PermissionDenied) spawn=denied(PermissionDenied)`; the worker reports `auto-crop-worker 0.0.5 (heif=on, escape-tests=off)`; with the worker renamed a forced-worker decode fails (`AUTOCROP_DECODE=worker but no worker was found beside the executable`) and the default mode says `sandbox: none`. The Windows and macOS package workflows have the same checks (`package-windows.yml` in PowerShell); the macOS package run and a Windows package run on a green tree are listed in the final report of the task as pending or blocked (the Windows run of 65c1e97 stopped in `cargo about`, see below).
