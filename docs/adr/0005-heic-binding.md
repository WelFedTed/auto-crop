# 0005 - HEIC binding: own thin libheif FFI, libde265 plugin, HeicBackend

- **Status:** accepted
- **Date:** 2026-10-01
- **Roadmap items:** M0.43 (feeds M6.01-M6.03)
- **Decision log links:** B12, D4, A-5
- **Time box:** 4 days (PROVISIONAL, shared with ADR-0004); actual a few hours. Code: `spikes/heic/` (throwaway, outside the workspace).

## Context

M0 must show that a real HEVC HEIC decodes to RGB8 from bytes through our own decode-only libheif with libde265 as a separate plugin on all three OSes, and decide how Rust binds libheif.

## Options considered

| Option | Notes |
|---|---|
| `libheif-rs` 3.0.0 on `libheif-sys` | Mature wrapper, but `libheif-sys` finds libheif through pkg-config or vcpkg (no pkg-config on Windows; vcpkg is excluded by B12 because its default `hevc` feature pulls GPL x265), and its embedded mode is stale and forbidden (D4). Not evaluated further |
| bindgen-generated own crate | Needs libclang in every contributor and CI environment for a surface of about ten functions |
| **hand-written own FFI** | About ten functions, no extra build dependencies, reviewed against the pinned headers, easy to put behind a `HeicBackend` trait |

## Results

`spikes/heic/` links the pinned libheif 1.23.5 (the build script refuses anything older) and exposes `decode_rgb8(bytes, load_plugins)`. The test input is `conformance_window_padding.heic` (656 bytes, real HEVC) taken from the **pinned libheif source tree** after the SHA-256 check, so no third-party file is committed.

| Case | Result (identical on windows-2025, macos-latest, ubuntu-22.04, ubuntu-24.04, ubuntu-22.04-arm, windows-11-arm) |
|---|---|
| plugin present (`heif_init`, plugin dir via `LIBHEIF_PLUGIN_PATH`) | `OK 1x1 bytes=3 nonzero=true libheif=1.23.5` |
| plugin directory removed (the `no-hevc` situation) | error code 11, subcode 6003: "No decoding plugin installed for this compression format: HEVC (a suitable decoder plugin is libde265)" |
| garbage input; truncated input | clean errors, no crash (unit tests) |

Fedora is a best-effort cell (see ADR-0004).

## Decision

**GO:** bind libheif with our **own hand-written FFI** inside the codecs crate (or a small `heic` module behind the `HeicBackend` trait), loading libde265 as a **plugin** (plugin vs direct link: plugin on every OS, recorded in ADR-0004).

- **`HeicBackend` trait** (ImageIO on macOS and WIC on Windows stay optional fast paths, `heic.engine = system`, opt-in until a parity gate passes): `decode(bytes, limits) -> Result<Rgb8 + metadata, HeicError>`.
- **`no-hevc` hook:** the `hevc` Cargo feature ships or omits the libde265 plugin. Without it the engine maps libheif error 11/6003 (and any "no decoding plugin ... HEVC" message) to the typed error `HevcDecoderMissing`, which the UI and CLI present with the `heic.engine = system` hint. Official builds always ship the plugin (A-5).
- **LGPL layout:** libheif and libde265 are separate shared libraries (and the plugin) next to the executable with their licence texts and source offers in the notices; replacing them requires no relinking of Auto Crop.
- **Sandbox:** the spike runs in-process; in the product this code runs in the sandboxed worker-process pool (D3), with `heif_security_limits` set from the pixel and memory caps.

## Consequences

- M6 builds the worker routes, metadata (EXIF, ICC, `irot`/`imir`) and colour handling on this binding, and a Windows packaging item decides the plugin directory for relocated installs.
- The 1x1 fixture proves the pipeline but not colour correctness; the real-device corpus (M0.49, M6) covers that.
- **Revisit trigger:** a maintained permissive pure-Rust HEVC/HEIC decoder (D4, about six months), or a libheif API change that breaks the ten bound functions.
