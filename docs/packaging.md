# Packaging: Windows, macOS and Linux test builds

Status: pre-release test builds for the owner's own use. Nothing here is published (no release, no tag, no signing, no notarisation); the packages are workflow artifacts that live for 14 days. Decisions: B9 (Windows first, CI on all three OSes, 1.0 needs all three; Linux touch is best-effort), B15 ($0: no Apple Developer account, no paid signing), B12 (libheif, libde265 and dav1d are separate, replaceable shared libraries), ADR-0004, ADR-0005, ADR-0009. The HEIC details are in [testing/heif-native.md](testing/heif-native.md).

## What exists

| OS | Workflow | Runner | Package (artifact name) | Contents |
|---|---|---|---|---|
| Windows x64 | `package-windows.yml` | `windows-2025` | `auto-crop-windows-x64-<ver>` : `.zip` | `AutoCrop.exe`, `auto-crop.exe`, `auto-crop-worker.exe`, `heif.dll`, `dav1d.dll`, `libde265.dll`, `libheif/` plugin folder, licences |
| macOS Apple silicon | `package-macos.yml` | `macos-latest` (arm64) | `auto-crop-macos-arm64-<ver>` : `.zip`, `.dmg` | `AutoCrop.app` (ad-hoc signed, not notarised) with the CLI, dylibs and plugin folder inside, licences, README |
| macOS Intel | `package-macos.yml` | `macos-26-intel` (best-effort) | `auto-crop-macos-x86_64-<ver>` : `.zip`, `.dmg` | the same, x86_64 |
| Linux x86_64 | `package-linux.yml` | `ubuntu-22.04` (glibc 2.35) | `auto-crop-linux-x86_64-<ver>` : `.tar.gz`, `.AppImage`, `.deb` | `bin/AutoCrop`, `bin/auto-crop`, `lib/` with libheif, dav1d, libde265 and `lib/libheif/` plugin folder, desktop file, icon, licences |

All three workflows are `workflow_dispatch` only, use SHA-pinned actions and read-only permissions, and build everything on the runner, because the owner's machine has no cmake, ninja, meson or nasm: the pinned native libraries (`cargo xtask build-native`, then `cargo xtask check-native`), the UI (`npm ci`, `npm run build`), the Tauri shell (`--features gui,custom-protocol,heif`) and the CLI (`--features heif`), then the package. Third-party notices come from `cargo about`.

```
gh workflow run package-macos.yml --ref main      # also package-linux.yml, package-windows.yml
gh run list --workflow package-macos.yml --limit 1
gh run download <run-id> -n auto-crop-macos-arm64-<ver>      # into ./auto-crop-macos-arm64-<ver>/
```

`gh run download` unpacks the artifact wrapper; the files inside are the real packages and `SHA256SUMS` style `.sha256` / `.txt` files next to them (`shasum -a 256 -c <file>` on macOS, `sha256sum -c` on Linux, run in the folder that holds them). **A downloaded AppImage has lost its execute bit** (artifact transfer does not keep it): `chmod +x AutoCrop-<ver>-x86_64.AppImage`.

## Where the program looks for the libheif plugin folder

libheif's compiled-in plugin directory is the build machine's prefix, so every executable calls `auto_crop_engine::packaged::configure_heif_from_exe()` first (the shell in `run()`, the CLI in `main()`). It tries these folders, relative to the executable, in order, and uses the first that exists (`crates/engine/src/packaged.rs`, one unit test per layout):

| Layout | Executable | Plugin folder | How the executable finds the shared libraries |
|---|---|---|---|
| Windows zip | `<dir>/AutoCrop.exe` | `<dir>/libheif` | the DLLs beside it (default DLL search order) |
| macOS app bundle | `AutoCrop.app/Contents/MacOS/AutoCrop` (and `auto-crop`) | `Contents/Frameworks/libheif` | rpath `@executable_path/../Frameworks`; the dylibs have `@rpath/<name>` install names and rpath `@loader_path`, the plugin `@loader_path/..` |
| Linux tarball, AppDir, deb | `<root>/bin/AutoCrop` (and `auto-crop`) | `<root>/lib/libheif` | RUNPATH `$ORIGIN/../lib`; the libraries have RUNPATH `$ORIGIN`, the plugin `$ORIGIN/..` |

A development build has none of these folders and keeps libheif's built-in directory and `LIBHEIF_PLUGIN_PATH`. The macOS plugin sits in `Contents/Frameworks/libheif` (as ROADMAP M8.14 says), not in `Resources`: code signing treats `Resources` as data, and an unsigned dylib there invalidates the bundle seal. On macOS CMake names the plugin `libheif-libde265.so` (a Mach-O bundle with the module suffix `.so`, not `.dylib`); libheif finds it by that name and the assemble script keeps it.

## macOS: `AutoCrop.app`

`packaging/macos/assemble-app.sh` (run by the workflow) builds the bundle:

- `Contents/MacOS/AutoCrop` (the app) and `auto-crop` (the CLI, run it by its full path or link it onto your `PATH`), `Contents/Frameworks/libheif*.dylib`, `libdav1d*.dylib`, `libde265*.dylib` and `Contents/Frameworks/libheif/` (the libde265 plugin), `Contents/Resources/` (icon, licence texts of the three libraries, `native-deps.toml`, notices), `Info.plist` from `packaging/macos/Info.plist.in` (bundle id from `identity.toml`, minimum system version 12.0).
- `install_name_tool` rewrites every install name, dependency path and rpath so that nothing refers to the build machine, and an audit fails the build if a dependency is not `/usr/lib`, `/System`, or an `@rpath` library that is in the bundle, or if an rpath is not one of the three allowed ones. No x265, x264 or AV1 encoder symbol may be in the shipped libraries.
- Ad-hoc signature (`codesign --force --sign -`, free), inner files first, then the bundle with `--deep`; `codesign --verify --deep --strict` runs on the built bundle, after the zip round trip and on the copy taken from the DMG. `MACOSX_DEPLOYMENT_TARGET=12.0` is set for rustc, CMake, meson and the C compilers.
- The zip is made with `ditto` (it keeps symlinks and the signature); the DMG is a plain `hdiutil create -format UDZO` of the same folder (no styled background, no Applications link yet: ROADMAP M8.06).

Build it yourself on a Mac (Xcode command line tools, `brew install cmake meson ninja`, plus `nasm` on Intel, Node 22, Rust from `rust-toolchain.toml`):

```
cargo xtask build-native && cargo xtask check-native
(cd ui && npm ci --ignore-scripts && npm run build)
export AUTOCROP_NATIVE_PREFIX="$PWD/target/native/prefix" MACOSX_DEPLOYMENT_TARGET=12.0
cargo build --release -p auto-crop-shell --features gui,custom-protocol,heif
cargo build --release -p auto-crop-cli --features heif
PREFIX=$PWD/target/native/prefix RELEASE=$PWD/target/release SRC=$PWD/target/native \
  OUT=$PWD/dist VERSION=0.0.4 COMMIT=local bash packaging/macos/assemble-app.sh
```

**Opening an unsigned app** (also in the package's `README-PACKAGE.txt`): Gatekeeper blocks it because it is not notarised (B15: no Apple fee). Control-click the app and choose Open, or on macOS 15 and later open it once and use System Settings > Privacy & Security > Open Anyway, or `xattr -dr com.apple.quarantine /Applications/AutoCrop.app`. A downloaded zip or DMG carries the quarantine flag; a file copied with `scp` or from a USB stick does not.

## Linux: tar.gz, AppImage, deb

`packaging/linux/assemble.sh` builds one tree (`bin/`, `lib/`, `lib/libheif/`, `share/`, `licenses/`), with `patchelf` setting the RUNPATHs above, an audit (no RPATH pointing into the build tree, no symbol newer than GLIBC_2.35, no encoder symbol), then:

- **tar.gz**: the tree, `tar --owner=0 --group=0`.
- **AppImage** (`make-appimage.sh`): the same `bin`, `lib`, `share` under `AppDir/usr`, an `AppRun` that starts `AutoCrop` (or the CLI with `--cli`), built with **appimagetool 1.9.1** (MIT, `AppImage/appimagetool`), downloaded in CI and checked against the pinned SHA-256 `ed4ce84f...eb0` before it runs (extract-and-run, so no FUSE is needed to build). It is a **thin** AppImage: WebKitGTK 4.1 and GTK 3 are not inside it. Bundling WebKitGTK (what Tauri's bundler does with linuxdeploy and a GTK plugin) needs several unpinned downloads and patching WebKit's helper-process paths, and a bundled WebKit tied to the graphics stack it was built against is the usual way AppImages of WebKit apps break on other distributions. If the owner wants that, it is a separate task (ROADMAP M9.13).
- **deb** (`make-deb.sh`): plain `dpkg-deb`, no Tauri bundler and no download: `/usr/lib/auto-crop/{bin,lib}`, symlinks `/usr/bin/AutoCrop` and `/usr/bin/auto-crop`, desktop file and icon under `/usr/share`, `Depends: libc6 (>= 2.35), libstdc++6 (>= 11), libgcc-s1, libwebkit2gtk-4.1-0, libgtk-3-0`.

Runtime needs (also in the package README): WebKitGTK 4.1 and GTK 3 from the distribution (`libwebkit2gtk-4.1-0 libgtk-3-0` on Debian and Ubuntu 22.04 or later, `webkit2gtk4.1 gtk3` on Fedora, `webkit2gtk-4.1 gtk3` on Arch), glibc 2.35 or newer, and for the AppImage `libfuse2` (or `--appimage-extract-and-run`).

Build it yourself on Ubuntu 22.04 (or the devcontainer): `sudo apt install nasm meson ninja-build cmake patchelf pkg-config libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev`, then the same build commands as above and `PREFIX=... RELEASE=... SRC=... OUT=dist/auto-crop-linux VERSION=... COMMIT=... bash packaging/linux/assemble.sh`, `packaging/linux/make-appimage.sh <tree> <out.AppImage> <tools dir>` and `packaging/linux/make-deb.sh <tree> <version> <out.deb>`.

## What the smoke tests prove

`packaging/smoke-cli.sh` (shared by the macOS and Linux workflows; the Windows workflow has the same steps in PowerShell) runs the packaged command-line tool from an unpacked copy (every decode with `AUTOCROP_DECODE=worker`, which never falls back, so a pass means the sandboxed worker did it; doctor prints the level and the worker's own probes; with the worker renamed a forced-worker decode fails and the default mode says `sandbox: none`), after the native build tree has been moved away and `LIBHEIF_PLUGIN_PATH`, `LD_LIBRARY_PATH` and the `DYLD_*` variables were cleared:

1. `--version` and `doctor` start, and doctor reports libheif with HEVC and AV1 found.
2. A committed AVIF (`crates/codecs/tests/fixtures/heif/gradient-444-48x32.avif`) decodes (48x32) and writes a JPEG.
3. The HEIC of the pinned libheif archive (`rainbow-451x461.heic`) decodes (451x461): HEVC through the libde265 plugin of the package folder.
4. With the plugin folder renamed, doctor says `HEVC NOT FOUND`, the HEIC fails with `HevcDecoderMissing` (so the plugin really comes from the package), and the AVIF still decodes.
5. With the libheif library renamed, the tool does not start (the library is a real dependency of the package, not found elsewhere on the runner).

On top of that: macOS runs `codesign --verify --deep --strict` after the zip round trip and on the DMG copy, prints what `spctl` says about a quarantined copy and checks `lipo`/`minos`; Linux checks with `ldd` that every heif, dav1d and de265 library resolves inside the package folder, runs the AppImage through `AppRun --cli` (extract-and-run, and through FUSE where the runner allows it) and installs the `.deb` with `dpkg -i`; every workflow checks the sizes and SHA-256 sums of the files it uploads. The app is then started for ten seconds (macOS: from the unzipped app with `screencapture`; Linux: under Xvfb with a window list from `xwininfo` and a screenshot from ImageMagick `import`); the log and the screenshot are uploaded as a second artifact (`gui-check-*`). That check is informational: nobody drives the window, so it proves the process starts and stays alive and the window maps, not that the UI works.

## CI evidence (2026-10-14, commit cba1a1a and 39ee8bf)

| Package | Run | Size | Time |
|---|---|---|---|
| `auto-crop-linux-x86_64-0.0.4`: tar.gz 9.76 MB, AppImage 9.85 MB, deb 6.72 MB | [38552116089](https://github.com/WelFedTed/auto-crop/actions/runs/38552116089), again with the pinned AppImage runtime: [38561743353](https://github.com/WelFedTed/auto-crop/actions/runs/38561743353) | all smoke checks green, app window mapped under Xvfb ("Auto Crop", 1280x860), screenshot shows the start screen | about 12 minutes |
| `auto-crop-macos-arm64-0.0.4`: zip 9.1 MB, DMG 10.0 MB | [38552110560](https://github.com/WelFedTed/auto-crop/actions/runs/38552110560) | all smoke checks green, `codesign` valid, Gatekeeper `rejected` (as expected unsigned), the app stayed alive and `screencapture` showed the real window | about 80 minutes (the libraries alone take about 55 on this runner) |
| `auto-crop-macos-x86_64-0.0.4`: zip 10.6 MB, DMG 11.8 MB | [38561739199](https://github.com/WelFedTed/auto-crop/actions/runs/38561739199) | same checks green | about 25 minutes |
| Shell with the `gui` feature on Windows, macOS, Linux (ci.yml job `shell-gui`) | [38552097873](https://github.com/WelFedTed/auto-crop/actions/runs/38552097873) | green | |

The artifacts were also downloaded with `gh run download` and the `.sha256` / `SHA256SUMS` files checked against the downloaded files. Details worth knowing: on macOS the `AutoCrop` executable reports `minos 11.0` (Rust's default for Apple silicon) and the CLI `minos 12.0`; the bundle's `LSMinimumSystemVersion` is 12.0, and nothing was run on a macOS older than the runners' (15 or later). The "libheif renamed" check on Linux ends in a symbol-lookup error rather than a missing-library error because the runner has an older system `libheif.so.1`, which the loader falls back to once the package's copy is gone; with the package intact the loader takes the package copy first (`ldd` shows it).

## Not verified, known limits

- **No real Mac or Linux hardware has run these packages.** Everything above ran on GitHub-hosted runners. The owner will test on an Apple-silicon Mac, an Intel MacBook and a Linux machine. Expect first-run surprises (graphics stack on Linux, Gatekeeper wording on newer macOS).
- Unsigned: no Developer ID, no notarisation, no Windows signature. macOS needs the Open Anyway steps; the Linux packages are not signed either.
- HEIC and AVIF are decoded by the **sandboxed `auto-crop-worker`** that ships beside the executable in every package (ADR-0013); the level reached on the CI runners and what the worker saw from inside are in [testing/sandbox-worker.md](testing/sandbox-worker.md), and `auto-crop doctor` prints the level on the machine it runs on. Not verified: real Windows 10, Surface, Mac or Linux hardware, macOS 12, and any distribution kernel other than the runners'. With `AUTOCROP_DECODE=inproc`, or if the worker is missing, decoding happens in the app's own process (level `none`, said once on stderr).
- Linux: WebKitGTK is a host dependency. The plan's WebKitGTK caveats apply (blank window with some NVIDIA/Wayland combinations, pinch zoom of the whole page, DMABUF renderer; `WEBKIT_DISABLE_DMABUF_RENDERER=1` or `GDK_BACKEND=x11` as workarounds). Touch input on Linux is best-effort (B8); mouse, touchpad and keyboard are the supported way. The `.desktop` file has no `MimeType` and no `%F` yet (opening files from the file manager is ROADMAP M9.31).
- The icon is the 32 pixel placeholder from `crates/shell/icons/icon.ico`, scaled up for `.icns`.
- The Intel macOS job is best-effort (`continue-on-error`); ONNX Runtime is not part of these builds (no inference runtime is linked yet).
- Not done (still on the roadmap): styled DMG with an Applications link (M8.06), Homebrew cask (M8.09), updater, Flatpak/rpm (M9), arm64 Linux, a bundled WebKitGTK, replaceable-library instructions per OS (M6.79), the LGPL relink drill (M8.41), Mach-O audit inside `check-native` (M8.15; the assemble script has its own audit), release workflow and checksums for publication.
