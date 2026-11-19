# Packaging: one stand-alone executable per platform

Status: since decision **B22** (2026-11-19) every release is **one file per platform**, not a zip, installer or app bundle. Nothing is signed or notarised (B15: no paid signing, no Apple account). Decisions: B9 (Windows first, CI on all three OSes), B12 as amended by B22 (the decoders are linked in), ADR-0004, ADR-0005, ADR-0009, ADR-0013. The HEIC details are in [testing/heif-native.md](testing/heif-native.md).

## What a release contains

| OS | Workflow | Runner | File (release asset) |
|---|---|---|---|
| Windows x64 | `package-windows.yml` | `windows-2025` | `auto-crop.exe` |
| macOS Apple silicon | `package-macos.yml` | `macos-latest` (arm64) | `auto-crop-macos-arm64` |
| macOS Intel | `package-macos.yml` | `macos-26-intel` (best-effort) | `auto-crop-macos-x86_64` |
| Linux x86_64 | `package-linux.yml` | `ubuntu-22.04` (glibc 2.35) | `auto-crop-linux-x86_64` |

Next to the files a release carries `SHA256SUMS` (one file for all of them) and `THIRD_PARTY_NOTICES.md`.

The one file has three faces, chosen by its arguments (`entry()` in `crates/shell/src/lib.rs`):

- **no arguments**: the desktop app (`auto-crop gui` does the same);
- **`--decode-worker ...`**: the sandboxed decode worker. The app starts **its own executable** with this flag (ADR-0013); users never type it. The worker is the code of `crates/worker` (a library; the `auto-crop-worker` binary is a thin wrapper used by tests and development builds);
- **anything else**: the command-line tool of `crates/cli` (`auto-crop process ...`, `auto-crop doctor`, `auto-crop --help`; see [cli.md](cli.md)).

**Windows console note.** `auto-crop.exe` is a GUI-subsystem program (no console window when you double-click it). When it is started from a console with arguments it attaches to that console and prints there, but `cmd` does not wait for a GUI program: in a script use `start /wait auto-crop.exe process ...` (cmd) or `& .\auto-crop.exe process ... | Out-Host` (PowerShell), and the exit code is then available as usual.

## How the decoders are in the file

libheif, libde265 (HEVC) and dav1d (AV1) are built from the hash-checked, pinned sources of `native-deps.toml` as **static archives** (`cargo xtask build-native --static`, into `target/native/prefix-static`), with libde265 built into libheif (no plugin) and no encoder, then linked into the executable (`AUTOCROP_NATIVE_PREFIX=<prefix-static> AUTOCROP_HEIF_STATIC=1`, `crates/codecs/build.rs`). `cargo xtask check-native --binary <file>` fails the build when the file contains an x265, x264 or AV1-encoder symbol or imports a heif, dav1d or de265 library. Only the system's own libraries stay dynamic: the C and C++ runtime and system DLLs on Windows, libSystem and the WebKit framework on macOS, glibc, libstdc++, WebKitGTK and GTK on Linux.

**LGPL.** libheif and libde265 are LGPL-3.0-or-later. Linking them statically replaces the "separate, replaceable shared library" form of B12; the recipient's right to relink is met by the public source: the exact versions and hashes are in `native-deps.toml`, `cargo xtask build-native --static` rebuilds the archives and `cargo build --release -p auto-crop-shell --features gui,custom-protocol,heif` with the two environment variables above rebuilds the file. The licence texts are in `THIRD_PARTY_NOTICES.md` and in the repository.

## Building it

The workflows are `workflow_dispatch` only, use SHA-pinned actions and read-only permissions, and build everything on the runner, because the owner's machine has no cmake, ninja, meson or nasm: the static libraries, the UI (`npm ci`, `npm run build`), then the shell (`--features gui,custom-protocol,heif`) and `cargo xtask check-native --binary`. Third-party notices come from `cargo about`.

```
gh workflow run package-windows.yml --ref main      # also package-linux.yml, package-macos.yml
gh run list --workflow package-windows.yml --limit 1
gh run download <run-id> -n auto-crop-windows-x64-<ver>
```

By hand, on Windows (Visual Studio build tools, cmake, ninja, meson, nasm, Node 22):

```
cargo xtask build-native --static
(cd ui && npm ci --ignore-scripts && npm run build)
$env:AUTOCROP_NATIVE_PREFIX = "$PWD\target\native\prefix-static"; $env:AUTOCROP_HEIF_STATIC = "1"
cargo build --release -p auto-crop-shell --features gui,custom-protocol,heif
# the file is target\release\AutoCrop.exe; releases call it auto-crop.exe
```

macOS needs `brew install cmake meson ninja` (plus `nasm` on Intel) and `MACOSX_DEPLOYMENT_TARGET=12.0`; Linux (Ubuntu 22.04 or the devcontainer) needs `nasm meson ninja-build cmake pkg-config libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev`.

## What the smoke tests prove

`packaging/smoke-single.sh` (macOS and Linux; the Windows workflow has the same steps in PowerShell) copies the file **alone** into an empty folder, after the native build tree has been moved away and the library search variables cleared, and runs it:

1. `--version` and `doctor` start, and doctor reports libheif (linked in) with HEVC and AV1 found.
2. The `worker:` line of doctor names the executable itself; the `sandbox:` line is the level the worker reported from inside it, never `none`; from inside the worker its file access and process creation are denied.
3. `--decode-worker --version` says `escape-tests=off`, and the file contains none of the test-hook strings.
4. A committed AVIF (48x32) and the HEIC of the pinned libheif archive (451x461) decode, forced through the worker (`AUTOCROP_DECODE=worker` never falls back) and in the default mode (which must not say `Sandbox level: none`).

On top of that: `check-native --binary` (no forbidden symbols, no decoder library imported), the size and SHA-256 of what is uploaded, `codesign --verify` on macOS, and the app is started for ten seconds (macOS with `screencapture`, Linux under Xvfb with a window list and a screenshot, Windows with a process check). That last check is informational: nobody drives the window.

## Opening an unsigned file

- **Windows**: SmartScreen may warn about an unrecognised app; choose "More info" then "Run anyway".
- **macOS**: Gatekeeper blocks an unsigned download. Clear the quarantine flag once and make it executable: `xattr -d com.apple.quarantine auto-crop-macos-arm64 && chmod +x auto-crop-macos-arm64`, then run it (`./auto-crop-macos-arm64`). It is a bare executable, not an app bundle: it has no Dock icon of its own until the window opens, and double-clicking it in Finder opens it in Terminal.
- **Linux**: `chmod +x auto-crop-linux-x86_64`. WebKitGTK 4.1 and GTK 3 must be installed (`libwebkit2gtk-4.1-0 libgtk-3-0` on Debian and Ubuntu 22.04 or later, `webkit2gtk4.1 gtk3` on Fedora, `webkit2gtk-4.1 gtk3` on Arch).

## Not verified, known limits

- Everything ran on GitHub-hosted runners only; no real Mac or Linux hardware, no real touch input, no screen reader.
- Unsigned and not notarised; macOS and Linux have no `.app`, `.dmg`, AppImage, `.deb` or desktop entry any more (a desktop entry and file associations are a follow-up).
- On Windows the AppContainer worker is started from a **staged copy of the executable** (`%LOCALAPPDATA%\AutoCrop\worker-stage\<fingerprint>`, one copy per build, older ones removed), because an AppContainer cannot read the folder the file sits in.
- The worker is the whole application image (WebView and GUI code included, unused in that mode); on Linux the address-space limit of the worker has to cover the GTK and WebKit libraries the loader maps. If a platform shows that this does not work, the fix is a separate worker binary for that platform, not a change of the one-file promise.
- Linux: WebKitGTK is a host dependency. The plan's WebKitGTK caveats apply (blank window with some NVIDIA/Wayland combinations; `WEBKIT_DISABLE_DMABUF_RENDERER=1` or `GDK_BACKEND=x11` as workarounds). Touch input on Linux is best-effort (B8).
- The Intel macOS job is best-effort (`continue-on-error`); ONNX Runtime is not part of these builds.
- Still on the roadmap: updater, Homebrew cask, Flatpak, arm64 Linux, signed builds, the LGPL relink drill (M8.41).
