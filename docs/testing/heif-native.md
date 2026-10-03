# HEIC, HEIF and AVIF: building, running and testing the libheif path

Decision record: [ADR-0009](../adr/0009-heif-decode-backend.md). Design: [PLAN 3.4](../plan/03-image-io-formats.md). Code: `crates/codecs/src/heif/` (feature `heif`), `crates/codecs/src/parse/heif.rs` (header walk, always built), `xtask build-native`, `.github/workflows/heif.yml`.

## What is built

`cargo xtask build-native` builds, from the SHA-256-pinned archives of `native-deps.toml`, into `target/native/prefix`:

| Library | Build | Role |
|---|---|---|
| dav1d 1.5.4 (BSD-2) | meson, shared | AV1 decoder, linked into libheif (AVIF) |
| libde265 1.1.3 (LGPL) | cmake, shared | HEVC decoder, loaded by libheif as a **plugin** (HEIC) |
| libheif 1.23.5 (LGPL) | cmake, shared, **decode only** | container, grids, `clap`/`irot`/`imir`, colour conversion |
| libjpeg-turbo 3.2.0 | cmake, shared | the `turbojpeg` feature |

Every encoder is off (no x265, x264, aom, rav1e, SVT-AV1, kvazaar). `cargo xtask check-native` fails on `x265_`, `x264_` and AV1 encoder symbols, on a dependency that is not on `packaging/allowed-libs.txt` (dav1d is), and on a libheif that does not link dav1d.

## Local requirements

| OS | Needed on `PATH` |
|---|---|
| Windows 10/11 | Visual Studio 2022 Build Tools (C++ workload), CMake, **Meson and Ninja** (`pip install meson ninja`), **NASM** (dav1d and libjpeg-turbo assembly; a portable `nasm.exe` is enough). Meson finds the Visual Studio environment itself when `cl` is not on `PATH` |
| Linux | `gcc g++ make cmake nasm meson ninja-build curl tar xz` (Ubuntu 22.04: `sudo apt-get install nasm meson ninja-build cmake build-essential`) |
| macOS (Apple silicon) | Xcode command line tools, `brew install cmake meson ninja` (no NASM on arm64) |

The default build (`cargo build`, `cargo test --workspace`) needs none of this: the `heif` feature is off, nothing native is searched for, and HEIC and AVIF are *probed* (size, depth, orientation, ICC, item count; safe Rust) but `decode` answers `avif files cannot be decoded in this build`.

## Build, check, test

```
cargo xtask build-native            # about 20 minutes cold on Windows (libheif and dav1d dominate); cached archives are reused
cargo xtask check-native
```

The libraries must be found at run time: on Windows put `target/native/prefix/bin` on `PATH`; on Linux and macOS the test binaries carry an rpath to `target/native/prefix/lib` for libheif itself, but libheif finds dav1d and libde265 through the loader path, so also set `LD_LIBRARY_PATH` / `DYLD_LIBRARY_PATH` to `target/native/prefix/lib` (CI does).

```
cargo test -p auto-crop-codecs --features heif          # real AVIF and HEIC files, limits, orientation, hostile input
cargo test -p auto-crop-engine --features heif          # open AVIF, never replace it, copy it as PNG
cargo run -p xtask --features heif -- make-hostile      # the hostile corpus through libheif and dav1d, each file in its own process
cargo run --release -p auto-crop-codecs --features heif --example heif_probe -- --repeat 10 file.avif
```

`AUTOCROP_NATIVE_PREFIX` points the build script at another prefix. `AUTOCROP_REQUIRE_HEIF_SAMPLES=1` turns "the libheif source archive is not extracted" from a skip into a failure (CI sets it). The tests read these files in place from the extracted, hash-checked libheif archive (`target/native/libheif-<ver>/src/...`) and never copy them: `tests/data/rainbow-451x461.heic`, `with-alpha-512x512.heic`, `clap_cropped*.heic|avif`, `clap_oversized_ispe_*.avif`, `examples/example.avif`, the `mif3` mini-layout files and four codec samples of `fuzzing/data/corpus` (AVC, JPEG, JPEG 2000, VVC: must be a typed error).

The committed fixtures (`crates/codecs/tests/fixtures/heif/`, 15 AVIF files and one ICC profile, 18 KB) are procedural pictures made by `make_heif_fixtures.py` (Pillow 12.3.0; ImageMagick for the 10-bit file); `make_big_avif.py` writes the 12 MP timing file (never committed).

## What the plugin directory means (HEVC)

libheif loads `heif-libde265` (Windows: `heif-libde265.dll`; Linux and macOS: `libheif-libde265.so|dylib`) from its compiled-in plugin directory, which is `<prefix>/lib/libheif` of the build, and from `LIBHEIF_PLUGIN_PATH` (**when that variable is set it replaces the compiled-in directory**, it does not add to it). Without the plugin an HEVC file fails with `CodecError::HevcDecoderMissing` (the `no-hevc` variant of ADR-0005; AVIF still decodes), mapped to `ErrKind::HevcDecoderMissing` by the engine. CI proves it by moving the directory away (`heif_probe` prints `ERR code=hevc_decoder_missing`).

## What the shell needs (packaging, a later task)

Not done here; this is the list for whoever owns packaging (ADR-0004, ADR-0005, PLAN 3.4.2):

1. Build the shell with `--features heif` (`auto-crop-shell/heif` forwards to `auto-crop-codecs/heif`), after `cargo xtask build-native`.
2. Ship next to the executable: `heif.dll`, `dav1d.dll`, `libde265.dll` (Windows; `libheif.so.1`, `libdav1d.so.7`, `libde265.so.0` on Linux; the `.dylib`s on macOS) and the plugin directory `libheif/` containing `heif-libde265.dll`. They are separate, replaceable shared libraries (LGPL, B12) with their licence texts and source offer in the notices.
3. Before the first decode call `auto_crop_codecs::heif::configure(Some(<exe dir>/libheif))` (or set `LIBHEIF_PLUGIN_PATH`): the compiled-in plugin directory is the *build machine's* prefix and does not exist on a user's machine. `configure` must come before any decode or `heif::init()`; it returns false afterwards.
4. Windows: the DLLs must be found next to the executable (default search order does that); PLAN 3.4.2 wants the loader restricted to the application directory and System32 (`SetDefaultDllDirectories`).
5. Linux and macOS: give the executable an rpath to the library directory (`$ORIGIN/lib`, `@executable_path/../Frameworks`); libheif's own dependencies (dav1d) are resolved from libheif's directory, so install them together or set the rpath of libheif itself.
6. Decode threads: `heif::set_codec_threads(n)` bounds the threads dav1d and libde265 use per decode (default 0 = every core); lower it while the engine decodes several files at once.
7. dav1d and libheif print diagnostics (`Error parsing OBU data`) on stderr for damaged files; they are not errors of the app and cannot be silenced through the libheif API.
8. The decode must run in the sandboxed worker pool (ADR-0006): this crate decodes in-process; libheif and its plugins are loaded before the sandbox is entered.
