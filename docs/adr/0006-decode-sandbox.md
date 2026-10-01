# 0006 - Decode sandbox: level per OS, shared-memory mechanism, build order

- **Status:** accepted
- **Date:** 2026-10-01
- **Roadmap items:** M0.21, M0.22, M0.23 (feeds M6.07, S.15, and the M8/M9 per-OS work)
- **Decision log links:** D3 (worker-process pool), B12, A-9
- **Time box:** 5 days (PROVISIONAL); actual about two days. Code: `spikes/sandbox/` (throwaway, outside the workspace). CI: `.github/workflows/sandbox.yml`.

## Context

Decoders (libheif, libde265, libjpeg-turbo, libjxl, ...) parse untrusted files. D3 puts them in a worker-process pool. M0 must show, per OS, how far a worker can be confined and how pixels get back, with escape tests that prove the confinement is real (a `none` control mode proves each test can pass when nothing is blocked).

The spike worker receives its input as an **inherited handle** (stdin, never a path), applies a mode, tries four escapes (open a file by path, connect to a TCP port, spawn a process, allocate 1 GiB) and then does its real work: it writes 4096 bytes of "pixels" into shared memory that the parent compares byte for byte.

## Results (all PASS in CI unless stated)

### Linux (ubuntu-22.04, ubuntu-24.04, ubuntu-22.04-arm, Fedora digest-pinned container)

| Mode | file | tcp | spawn | alloc |
|---|---|---|---|---|
| none (control) | allowed | allowed | allowed | allowed |
| rlimit (AS 512 MB, CPU 20 s) | allowed | allowed | allowed | blocked |
| landlock (ABI v4, FullyEnforced on all four) | blocked | blocked | blocked | allowed |
| seccomp allow-list (32 syscalls) | blocked | blocked | blocked | allowed |
| full (rlimit + landlock + seccomp) | blocked | blocked | blocked | blocked |

- Kernels seen: 6.8 and 6.17 (x86_64), aarch64 on the ARM runner.
- **Landlock absent** is exercised with `AUTOCROP_SANDBOX_NO_LANDLOCK=1`: the level string becomes `landlock=unavailable(...)`, the Landlock mode no longer claims blocking, and `full` still blocks everything through seccomp. The level is always reported, never silent. A kernel older than 5.13, or an LSM stack that refuses Landlock, therefore degrades to seccomp-only, which already blocks all four escapes.
- Pixels: `memfd_create` shared memory inherited as fd 3, `mmap`ed by the worker.

### macOS (macos-latest)

`sandbox_init` with `(version 1)(deny default)` blocks file, tcp and spawn. `RLIMIT_AS` is not enforced by the macOS kernel, so the allocation cap is not available and is not claimed; memory is bounded by the pool's size limits and a watchdog instead (M9). Pixels: an unlinked temp file inherited as fd 3, `mmap`ed. macOS 12 and a real Mac are **UNMEASURED** (A-9); `sandbox_init` is deprecated by Apple but remains what Chromium and Firefox use.

### Windows (windows-2025, local Windows 11)

| Mode | file | tcp | spawn | alloc |
|---|---|---|---|---|
| none (control) | allowed | allowed | allowed | allowed |
| job object (1 active process, 512 MB) | allowed | allowed | blocked | blocked |
| restricted token + job | blocked | allowed | blocked | blocked |
| AppContainer (no capabilities) + job | blocked | blocked | blocked | blocked |

- The file escape uses a per-user temp file whose ACL excludes everyone else; a world-readable file such as `win.ini` proves nothing (an early version of the test did exactly that and "failed" for that reason).
- The restricted token needs restricting SIDs `Everyone`, `Users` and `RESTRICTED`. Only `RESTRICTED` made the process fail to start (0xC0000135, loader cannot open DLLs).
- The worker executable is copied to a private directory with explicit ACL grants so that AppContainer can run it.
- Handles are passed with `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` (an explicit whitelist: input file and the shared-memory section), not blanket inheritance.
- A restricted token still permits TCP on loopback and outbound; only AppContainer blocks the network. Pixels: an unnamed file mapping inherited by handle.
- The Surface (the touch device) is **UNMEASURED** and is re-run in M8.

## Decision

**GO** on a per-OS sandbox for the decode helper, with the level reported in the app's diagnostics (and in `--version --verbose` in the CLI):

| OS | Target level | Fallback (reported) |
|---|---|---|
| Linux | rlimit + Landlock (fs + TCP) + seccomp allow-list | seccomp-only, then rlimit-only (`process-only`) |
| macOS | `sandbox_init` deny-default + pool size/time limits | process-only if the call fails (reported) |
| Windows | AppContainer + job object | restricted token + job, then job-only if profile creation fails (reported) |

- **Shared memory, not pipes, for pixels**, with the input passed as an inherited handle so the worker never needs filesystem access.
- The seccomp allow-list is a starting point: real decoders need more syscalls (`openat` is NOT among them on purpose; plugins and libraries are loaded before the sandbox is entered). M6 widens the list by test, never by switching the filter off.
- **`birdcage` is ruled out** (GPL-3.0, incompatible with B2). `landlock` and `seccompiler` (Apache-2.0 / MIT) and `windows-sys` are used directly.
- M6.07 (AppContainer decision) can start from these numbers: the open questions are portable-layout ACLs, WIC codec loading inside an AppContainer, and Windows 10 behaviour.

## Consequences and open items

- **M6/M8/M9:** per-OS implementation behind a `Sandbox` trait; loading libheif and its plugins must happen before confinement on Linux and macOS, and from a readable install directory on Windows.
- **Not shown by this spike:** a real decoder inside the sandbox (libheif needs to be loaded first), Windows 10, macOS 12, kernels older than 6.8 for Landlock ABI differences beyond the fallback path, and Surface/Mac hardware. All are UNMEASURED, not passes.
- The `none` control passes on every OS, which shows the escape tests detect an unconfined worker.
