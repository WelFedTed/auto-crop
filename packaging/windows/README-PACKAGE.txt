Auto Crop @VERSION@ - Windows x64 build with HEIC, HEIF and AVIF input
=====================================================================

Built by CI from commit @COMMIT@ of https://github.com/WelFedTed/auto-crop
(workflow "Package Windows"). This is a PRIVATE TEST BUILD for the owner's own use.
It is not a release and has not been signed.

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

IMPORTANT: HEIC/AVIF PARSING RUNS IN-PROCESS
--------------------------------------------
The design (ADR-0006, decision B12) decodes untrusted HEIC, HEIF and AVIF files, which are
parsed by C and C++ libraries (libheif, dav1d, libde265), in a sandboxed worker-process pool
with pixel, memory and time caps. THAT POOL DOES NOT EXIST YET. In this build the libraries run
inside the app's own process, with the user's full rights. Size and memory limits and a decode
time limit are enforced before and during decoding, and the libraries are run against a
hostile-file corpus and fuzz seeds in CI, but a crafted file that finds a bug in one of them could run code as
you. This was decided acceptable for the owner's own use with files the owner trusts. Do not
hand this build to other people and do not open files from untrusted sources with it. It is not
suitable for a public release.

KNOWN LIMITS OF THIS BUILD
--------------------------
- It was proven headlessly in CI: the command-line tool decodes a real AVIF and a real HEIC
  from an unzipped copy of this folder with the build tree removed and PATH cleared. The
  desktop window itself was not driven by an automated test; treat the GUI as a preview.
- THIRD_PARTY_NOTICES.md lists the Rust crates. The npm packages inside the embedded UI are not
  yet covered by the notices tool (ROADMAP M3.67, M3.68).
- Windows only, x64 only.
