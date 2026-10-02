# CI policy guards

Roadmap: M1.72 (`cargo xtask ci-guards`), M1.73 (licence gates), M1.80 (workflow hardening); extends M0.31 (`check-profiles` owns the `panic = "abort"` ban). Decisions: B2 (licences), B12 (no GPL encoders, sandboxed C parsers), B18 and C4 (offline, no telemetry). Code: `xtask/src/ci_guards/`.

`cargo xtask ci-guards` runs in the `repo checks` job of `ci.yml`; `cargo xtask ci-guards --selftest` runs next to it. A guard that is not proven red is not a guard, so every rule below has planted violations (the `CASES` tables in each module) that must be detected, plus controls that must pass. The same cases run under `cargo test -p xtask`.

## 1. `unsafe` placement

- `unsafe` code is allowed only inside a module **directory** named `ffi` or `simd` (`crates/<crate>/src/**/ffi/**`, `.../simd/**`). A file called `ffi.rs` does not count.
- Every `unsafe` block, `unsafe impl` and `unsafe extern` block carries a `// SAFETY:` comment directly above it (attributes in between are fine, a blank line is not) or on the same line. An `unsafe fn` or `unsafe trait` declaration may carry a `# Safety` doc section instead. An `unsafe fn` inside an `unsafe impl` is covered by the impl's comment.
- The lint escape hatch follows the same rule: `#[allow(unsafe_code)]` outside `ffi/` and `simd/`, or `unsafe_code = "allow"` in any manifest, fails. The workspace default stays `unsafe_code = "deny"`, and the shipped crates `forbid` it today.
- Scanned: `crates/**`, `xtask/src/**`, `xtask/tests/**`. Not scanned: `spikes/` (throwaway code outside the workspace, never shipped) and third-party crates (cargo-deny and cargo-audit cover those).

### Allow-list (explicit, in `unsafe_guard::ALLOWED_FILES`)

| File | Why |
|---|---|
| `xtask/src/alloc_count.rs` | A counting `GlobalAlloc` that forwards to `System`; `hostile-run` uses it to measure how much heap a hostile file makes a decoder allocate, the same way on every OS (M1.69). `GlobalAlloc` is an unsafe trait, so there is no safe version. `xtask` is a developer tool and is never shipped. Its `unsafe` still needs `// SAFETY:` comments. |

Anything else needs the owner's agreement and a new row here.

## 2. No network crates in shipped code

Two layers, both in `network_guard.rs`:

1. **Manifests.** No first-party manifest (`crates/*/Cargo.toml`, `[workspace.dependencies]`) may name a banned or restricted crate, in any dependency kind or target table, including through a `package = "..."` rename.
2. **The shipped dependency graph.** `cargo metadata` is resolved for `x86_64-pc-windows-msvc`, `aarch64-apple-darwin` and `x86_64-unknown-linux-gnu` with all workspace features. From `auto-crop-cli` and `auto-crop-shell` only normal edges are followed (dev and build dependencies do not ship; proc-macro crates run at build time).
   - **Banned** anywhere in that closure, even through Tauri: HTTP clients and servers (`reqwest`, `hyper`, `h2`, `ureq`, `curl`, ...), TLS stacks (`rustls`, `openssl`, `native-tls`, `schannel`, ...), QUIC and WebSocket crates.
   - **Restricted**: `tokio`, `mio`, `socket2`, `http`, `soup3` and similar low-level crates. Allowed only when reachable solely through the GUI framework stack (`tauri*`, `wry`, `tao`, `rfd`, `webkit2gtk*`, ...), which needs them internally (Tauri's async runtime; `soup3` is WebKitGTK's own binding). First-party code may not use them.

Finding recorded when the guard was written (2026-10-02): with the `gui` feature, `tauri` brings `tokio`, `mio`, `socket2` and `http` on every desktop target, and `soup3` on Linux. They are in the framework allow-list above. Tauri's `reqwest` dependency is gated to Android and iOS and is absent from every desktop target, which is why only desktop triples are resolved. This guard bounds the dependency graph; B18's behavioural promise (no network calls without opt-in) is a separate runtime matter (Tauri capabilities and the CSP).

## 3. Workflow policy

A line-based backstop in `workflow_guard.rs`; `zizmor` (the `workflow lint` job, version pinned, online audits on) stays the deep audit.

- Every `uses:` is pinned to a full 40-hex commit SHA; every container image to an `@sha256:` digest.
- A top-level `permissions:` block exists and is read-only; write scopes go on the job that needs them; no `write-all`.
- No `pull_request_target` and no `workflow_run` trigger.
- A `pull_request` trigger has no `paths:` or `paths-ignore:` filter. A required check skipped by a path filter never reports, and a docs-only pull request would wait forever. The filter is a step inside the job (see `accuracy-smoke.yml`, `devcontainer.yml`).
- No tag trigger, no `release` trigger, no `release*.yml` and no publishing step (`cargo publish`, `gh release create`, release-plz) until the release milestone (M2.78): pre-1.0 a test tag creates nothing. The explicit allow-list is `workflow_guard::RELEASE_ALLOWED` (empty).
- `ci.yml` cancels superseded runs on pull requests only, so every push to `main` keeps its own result.

## 4. Licence gates (M1.73)

`cargo deny check` (licences, bans, sources, advisories) runs in `ci.yml`. `cargo xtask deny-selftest` plants one fixture crate for **every entry of the `[bans] deny` list in `deny.toml`** (so a ban added later is tested automatically), plus the AGPL, GPL and non-commercial licence fixtures, and a clean control. Each must fail for the right reason. Confirmed bans: `dssim-core`, `heic`, `heic-decoder`, `jpegxl-rs`, `jpegxl-sys`, `x264`, `x265` (and their `-sys` crates), `opencv`, `purecv`, `birdcage`.
