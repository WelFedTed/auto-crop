Auto Crop @VERSION@ - Linux x86_64 build with HEIC, HEIF and AVIF input
=====================================================================

Built by CI from commit @COMMIT@ of https://github.com/WelFedTed/auto-crop
(workflow "Package Linux"). This is an early PRE-RELEASE test build for people who accept that.
It is not signed. It was built and smoke-tested on a GitHub Ubuntu 22.04 runner (glibc 2.35, the
oldest supported); nobody has used it on a real Linux desktop yet, so treat it as a preview.

THE THREE FORMS OF THIS BUILD (they hold the same programs and libraries)
------------------------------------------------------------------------
auto-crop-linux-x86_64-@VERSION@.tar.gz     a folder: unpack it anywhere and run bin/AutoCrop
AutoCrop-@VERSION@-x86_64.AppImage          one file: make it executable and run it
auto-crop_@VERSION@_amd64.deb               for Debian and Ubuntu: sudo apt install ./auto-crop_@VERSION@_amd64.deb

WHAT IS IN THE FOLDER (tar.gz; the AppImage and the .deb have the same layout under usr/)
-----------------------------------------------------------------------------------------
bin/AutoCrop          the desktop app (Tauri 2, Svelte UI embedded)
bin/auto-crop         the command-line tool (one developer command today: dev-pipeline)
lib/                  libheif 1.23.5 (decode only, LGPL-3.0-or-later), dav1d 1.5.4 (the AV1 decoder
                      for AVIF, BSD-2-Clause), libde265 1.1.3 (the HEVC decoder for HEIC,
                      LGPL-3.0-or-later): separate shared libraries, found through $ORIGIN
lib/libheif/          plugin folder; the libde265 plugin in it lets libheif decode HEVC (HEIC)
share/                desktop entry and icon (the icon is a 32 pixel placeholder)
licenses/, LICENSE-MIT, LICENSE-APACHE, THIRD_PARTY_NOTICES.md   licences and notices
README-PACKAGE.txt    this file

The three libraries are separate, dynamically linked and replaceable: you may swap in your own
build of the same libraries (they are not part of the executables; for the AppImage run it with
--appimage-extract, replace the file, and run squashfs-root/AppRun). Their source is at
https://github.com/strukturag/libheif (tag v1.23.5), https://github.com/strukturag/libde265
(tag v1.1.3) and https://code.videolan.org/videolan/dav1d (tag 1.5.4); the exact archives and
SHA-256 sums that were built are listed in licenses/native-deps.toml.

WHAT YOUR SYSTEM MUST PROVIDE
-----------------------------
The desktop window is drawn by WebKitGTK 4.1 and GTK 3, which are NOT inside this build (not even in
the AppImage): they come from your distribution.
    Debian, Ubuntu (22.04 or later):  sudo apt install libwebkit2gtk-4.1-0 libgtk-3-0
    Fedora:                           sudo dnf install webkit2gtk4.1 gtk3
    Arch:                             sudo pacman -S webkit2gtk-4.1 gtk3
    openSUSE:                         sudo zypper install libwebkit2gtk-4_1-0 gtk3
Also needed: glibc 2.35 or newer (Ubuntu 22.04, Debian 12, Fedora 36 and later), libstdc++ 11 or
newer, and for the AppImage libfuse2 (Debian and Ubuntu: sudo apt install libfuse2; Ubuntu 24.04
calls it libfuse2t64) or run it with --appimage-extract-and-run. The command-line tool needs none of
the window libraries.

HOW TO RUN
----------
tar.gz:     tar xzf auto-crop-linux-x86_64-@VERSION@.tar.gz
            auto-crop-linux-x86_64-@VERSION@/bin/AutoCrop
AppImage:   chmod +x AutoCrop-@VERSION@-x86_64.AppImage
            ./AutoCrop-@VERSION@-x86_64.AppImage                 the app
            ./AutoCrop-@VERSION@-x86_64.AppImage --cli doctor    the command-line tool of the same image
.deb:       AutoCrop (the app) and auto-crop (the command-line tool) are then on your PATH.
Open JPEG, PNG, AVIF, HEIC or HEIF files, a folder, or drag them in. HEIC and AVIF sources are never
replaced in place (nothing can write them back yet): use "Save as copy" (copies of HEIC become JPEG,
of AVIF become PNG, in an AutoCrop folder). Command line example:
    auto-crop dev-pipeline photo.heic --json
    auto-crop doctor              says which libraries were found
Keep the files of the tar.gz together: bin/AutoCrop finds the libraries and the plugin folder
relative to itself. Do not copy bin/AutoCrop alone to another place.

Your data stays on your PC: there is no network access and no telemetry. Backups are in
$XDG_DATA_HOME/auto-crop (usually ~/.local/share/auto-crop) and settings in $XDG_CONFIG_HOME/auto-crop
(usually ~/.config/auto-crop), or everything is in the folder named by the environment variable
AUTO_CROP_HOME if you set it.

IMPORTANT: HEIC/AVIF PARSING RUNS IN-PROCESS, THERE IS NO SANDBOX
-----------------------------------------------------------------
The design (ADR-0006, decision B12) decodes untrusted HEIC, HEIF and AVIF files, which are
parsed by C and C++ libraries (libheif, dav1d, libde265), in a sandboxed worker-process pool
with pixel, memory and time caps. THAT POOL DOES NOT EXIST YET (and the Linux Landlock and seccomp
profile for it is not written). In this build the libraries run inside the app's own process, with
your full rights. Size and memory limits and a decode time limit are enforced before and during
decoding, and the libraries are run against a hostile-file corpus and fuzz seeds in CI, but a crafted
file that finds a bug in one of them could run code as you. Do not hand this build to other people
and do not open files from untrusted sources with it. It is not suitable for a public release.

KNOWN LIMITS OF THIS BUILD
--------------------------
- Proven only headlessly in CI: the command-line tool decodes a real AVIF and a real HEIC from the
  unpacked tarball, from the AppImage and from the installed .deb with the build tree removed and
  LD_LIBRARY_PATH cleared, and the app starts under Xvfb and stays alive for ten seconds. The window
  was not driven by a test and no real Linux desktop or GPU was used.
- WebKitGTK caveats of the plan (PLAN section 6, risk R1): on some NVIDIA and Wayland setups the window
  can come up blank; try  WEBKIT_DISABLE_DMABUF_RENDERER=1 AutoCrop  (or  GDK_BACKEND=x11 AutoCrop
  on Wayland). Pinch zoom on a touchscreen or touchpad may zoom the whole page instead of the
  picture. Touch input is best-effort on Linux: mouse, touchpad and keyboard are the supported way
  to use it.
- THIRD_PARTY_NOTICES.md lists the Rust crates. The npm packages inside the embedded UI are not
  yet covered by the notices tool (ROADMAP M3.67, M3.68).
- x86_64 only. No arm64 build, no Flatpak, no rpm yet.
