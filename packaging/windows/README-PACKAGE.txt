Auto Crop @VERSION@ - Windows x64 build with HEIC, HEIF and AVIF input
=====================================================================

Built by CI from commit @COMMIT@ of https://github.com/WelFedTed/auto-crop
(workflow "Package Windows"). This is an early PRE-RELEASE test build, published on the Releases page
when it carries a version tag; it has not been signed and is for people who accept that.

WHAT IS IN THE FOLDER
---------------------
AutoCrop.exe          the desktop app (Tauri 2, Svelte UI embedded)
auto-crop.exe         the command-line tool (one developer command today: dev-pipeline)
heif.dll              libheif 1.23.5, decode only (LGPL-3.0-or-later)
dav1d.dll             dav1d 1.5.4, the AV1 decoder used for AVIF (BSD-2-Clause)
libde265.dll          libde265 1.1.3, the HEVC decoder used for HEIC (LGPL-3.0-or-later)
libheif\              plugin folder; heif-libde265.dll in it lets libheif decode HEVC (HEIC)
LICENSE-MIT, LICENSE-APACHE, THIRD_PARTY_NOTICES.md, licenses\   licences and notices
README-PACKAGE.txt    this file

The three DLLs are separate, dynamically linked and replaceable: you may swap in your own
build of the same libraries (the LGPL libraries are not part of the executables). Their source
is at https://github.com/strukturag/libheif (tag v1.23.5), https://github.com/strukturag/libde265
(tag v1.1.3) and https://code.videolan.org/videolan/dav1d (tag 1.5.4); the exact archives and
SHA-256 sums that were built are listed in native-deps.toml of the commit above.

HOW TO RUN
----------
1. Unzip the whole folder anywhere (a normal folder, for example under your user profile).
   Keep the files together: AutoCrop.exe finds the DLLs and the libheif folder NEXT TO ITSELF.
   Do not copy AutoCrop.exe alone to another place.
2. Double-click AutoCrop.exe. Windows SmartScreen may warn because the file is not signed:
   "More info", then "Run anyway" (only if you built or downloaded it from your own CI run).
3. Open JPEG, PNG, AVIF, HEIC or HEIF files, a folder, or drag them in. HEIC and AVIF
   sources are never replaced in place (nothing can write them back yet): use "Save as copy"
   (copies of HEIC become JPEG, of AVIF become PNG, in an AutoCrop folder).
4. Command line, from a terminal in this folder:
       auto-crop.exe dev-pipeline photo.heic --json
   decodes the file, runs the stand-in detector and prints the per-stage report.

Needs: Windows 10 or 11 x64, the Microsoft Edge WebView2 runtime (already present on Windows 11
and on current Windows 10), and the Microsoft Visual C++ 2015-2022 Redistributable x64 (the
libraries are built with MSVC; it is installed on most PCs and is a free download from Microsoft).

Your data stays on your PC: there is no network access and no telemetry. Settings and backups
are in %LOCALAPPDATA%\AutoCrop, or in the folder named by the environment variable AUTO_CROP_HOME if you set it.

HEIC/AVIF ARE DECODED IN A SANDBOXED WORKER PROCESS
---------------------------------------------------
The files are untrusted input parsed by C and C++ libraries (libheif, dav1d, libde265). In this
build they are decoded by a separate program, "auto-crop-worker.exe", that sits beside the application and
confines itself BEFORE it reads any image: in the best level this PC allows, tried in this order and reported: an AppContainer with no
capabilities (no network, no access to your files) plus a job object (one process, memory limit),
then a restricted token plus the job object (files blocked, the network is NOT), then the job object
alone (no new processes and a memory limit only). It has a memory limit, a
time limit and a pixel cap, and if it crashes, hangs or runs out of memory only that one file fails
("Reading this file crashed the safe reader"), the worker is replaced and the rest of the batch
goes on. Run  auto-crop.exe doctor  to see the level the worker really reached and what it saw when it tried
to open a file, connect to the network and start a program from the inside: that line is printed
by the worker itself, not assumed. Set AUTOCROP_DECODE=inproc to decode inside the app instead
(level "none": the libraries then run with your full rights), AUTOCROP_DECODE=worker to force
every format through the worker. If the worker is missing or cannot start, the app says so once
and decodes HEIC and AVIF inside its own process (level "none").

What this does NOT cover: it was proven on GitHub-hosted CI runners (see docs/testing/sandbox-worker.md
for the exact levels and results), not on a Surface or a Windows 10 PC; the confinement of a crafted file that exploits the
operating system itself is out of scope. On first use the worker and its DLLs are copied into %LOCALAPPDATA%\AutoCrop\worker-stage so that the
AppContainer may read them (a folder you unzipped into is not readable by an AppContainer). JPEG, PNG, TIFF and WebP are decoded by safe Rust
code inside the app (set AUTOCROP_DECODE=worker to send them through the worker as well).

KNOWN LIMITS OF THIS BUILD
--------------------------
- It was proven headlessly in CI: the command-line tool decodes a real AVIF and a real HEIC
  from an unzipped copy of this folder with the build tree removed and PATH cleared. The
  desktop window itself was not driven by an automated test; treat the GUI as a preview.
- THIRD_PARTY_NOTICES.md lists the Rust crates. The npm packages inside the embedded UI are not
  yet covered by the notices tool (ROADMAP M3.67, M3.68).
- Windows only, x64 only.
