Auto Crop @VERSION@ - macOS @ARCH@ build with HEIC, HEIF and AVIF input
===================================================================

Built by CI from commit @COMMIT@ of https://github.com/WelFedTed/auto-crop
(workflow "Package macOS"). This is an early PRE-RELEASE test build for people who accept that.
It is NOT notarised and NOT signed with an Apple Developer ID (the project pays for no Apple
account, decision B15): it carries only the free "ad-hoc" signature that Apple silicon requires of
every program. It was built and smoke-tested on a GitHub macOS runner; nobody has used it on a real
Mac yet, so treat it as a preview.

WHAT IS IN THE FOLDER
---------------------
AutoCrop.app                         the desktop app (Tauri 2, Svelte UI embedded)
  Contents/MacOS/AutoCrop            the app itself
  Contents/MacOS/auto-crop           the command-line tool (one developer command today: dev-pipeline)
  Contents/Frameworks/               libheif 1.23.5 (decode only, LGPL-3.0-or-later), dav1d 1.5.4
                                     (the AV1 decoder for AVIF, BSD-2-Clause), libde265 1.1.3
                                     (the HEVC decoder for HEIC, LGPL-3.0-or-later) as .dylib files
  Contents/Frameworks/libheif/       plugin folder; the libde265 plugin in it lets libheif decode HEVC (HEIC)
  Contents/Resources/                licence texts of the libraries, THIRD_PARTY_NOTICES.md, native-deps.toml
LICENSE-MIT, LICENSE-APACHE, THIRD_PARTY_NOTICES.md   licences and notices
README-PACKAGE.txt                   this file

The three libraries are separate, dynamically linked and replaceable: you may swap in your own
build of the same libraries (they are not part of the executables; macOS requires that you sign
the replaced file again: codesign --force --sign - <file>). Their source is at
https://github.com/strukturag/libheif (tag v1.23.5), https://github.com/strukturag/libde265
(tag v1.1.3) and https://code.videolan.org/videolan/dav1d (tag 1.5.4); the exact archives and
SHA-256 sums that were built are listed in native-deps.toml of the commit above.

HOW TO OPEN AN UNSIGNED APP
---------------------------
1. Unzip (or open the .dmg) and drag AutoCrop.app to the Applications folder (or anywhere else
   you can write to; keep the .app in one piece).
2. macOS Gatekeeper blocks apps that are not notarised. The first time:
   - Control-click (right-click) AutoCrop.app, choose Open, then click Open in the dialog. On
     macOS 15 and later that menu path may not offer Open: try to open the app once, then go to
     System Settings > Privacy & Security, scroll down and click "Open Anyway" for Auto Crop.
   - or, in Terminal (only for a download you built or fetched from your own CI run):
         xattr -dr com.apple.quarantine /Applications/AutoCrop.app
   After that it opens normally. Files you copy to the Mac with a tool that sets no quarantine flag
   (for example scp or a USB stick) need none of this.
3. Open JPEG, PNG, AVIF, HEIC or HEIF files, a folder, or drag them in. HEIC and AVIF
   sources are never replaced in place (nothing can write them back yet): use "Save as copy"
   (copies of HEIC become JPEG, of AVIF become PNG, in an AutoCrop folder).
4. Command line, from Terminal:
       /Applications/AutoCrop.app/Contents/MacOS/auto-crop dev-pipeline photo.heic --json
   decodes the file, runs the stand-in detector and prints the per-stage report.
       /Applications/AutoCrop.app/Contents/MacOS/auto-crop doctor
   says which libraries were found.

Needs: macOS 12 Monterey or later, @ARCH@ (@ARCH_NOTE@).

Your data stays on your Mac: there is no network access and no telemetry. Settings and backups are
in ~/Library/Application Support/AutoCrop (or the folder named by the environment variable
AUTO_CROP_HOME if you set it).

HEIC/AVIF ARE DECODED IN A SANDBOXED WORKER PROCESS
---------------------------------------------------
The files are untrusted input parsed by C and C++ libraries (libheif, dav1d, libde265). In this
build they are decoded by a separate program, "auto-crop-worker", that sits beside the application and
confines itself BEFORE it reads any image: it enters a Seatbelt sandbox with everything denied (no files, no network, no new processes)
after its libraries and the libde265 plugin are loaded. macOS does not enforce an address-space
limit, so the memory limit is the app watching the worker's size and stopping it. It has a memory limit, a
time limit and a pixel cap, and if it crashes, hangs or runs out of memory only that one file fails
("Reading this file crashed the safe reader"), the worker is replaced and the rest of the batch
goes on. Run  Contents/MacOS/auto-crop doctor  to see the level the worker really reached and what it saw when it tried
to open a file, connect to the network and start a program from the inside: that line is printed
by the worker itself, not assumed. Set AUTOCROP_DECODE=inproc to decode inside the app instead
(level "none": the libraries then run with your full rights), AUTOCROP_DECODE=worker to force
every format through the worker. If the worker is missing or cannot start, the app says so once
and decodes HEIC and AVIF inside its own process (level "none").

What this does NOT cover: it was proven on GitHub-hosted CI runners (see docs/testing/sandbox-worker.md
for the exact levels and results), not on a real Mac (the Seatbelt call is deprecated by Apple but is what browsers use); the confinement of a crafted file that exploits the
operating system itself is out of scope. JPEG, PNG, TIFF and WebP are decoded by safe Rust
code inside the app (set AUTOCROP_DECODE=worker to send them through the worker as well).

KNOWN LIMITS OF THIS BUILD
--------------------------
- Not notarised, not signed with a Developer ID: see "How to open an unsigned app". Updates are
  manual (there is no updater).
- Proven only headlessly in CI: the command-line tool inside the app decodes a real AVIF and a real
  HEIC from an unpacked copy with the build tree removed and the library search variables cleared,
  and the app starts and stays alive for ten seconds. The window was not driven by a test and no
  real Mac was used. The window icon is a 32 pixel placeholder, scaled up.
- THIRD_PARTY_NOTICES.md lists the Rust crates. The npm packages inside the embedded UI are not
  yet covered by the notices tool (ROADMAP M3.67, M3.68).
- Apple silicon (arm64) is the main target; the Intel (x86_64) build is best-effort. The macOS
  specific parts of the plan (native menu bar, trackpad gestures, Finder integration, APFS
  clone backups) are not built yet.
