#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
#
# Builds the AppImage from the assembled package tree (packaging/linux/assemble.sh).
#
# This is a THIN AppImage: it carries the app, the command-line tool, libheif, dav1d and libde265, but
# NOT WebKitGTK, GTK and the system libraries below them. Those come from the host (the same
# libwebkit2gtk-4.1 and GTK 3 the tarball and the .deb need; README-PACKAGE.txt lists the package
# names). Bundling WebKitGTK is possible (Tauri's own bundler does it with linuxdeploy and a GTK
# plugin) but it needs several unpinned downloads and patches of WebKit's helper-process paths, and
# a bundled WebKitGTK is tied to the graphics stack it was built against, which is the main way
# AppImages of WebKit apps fail on other distributions. Not done here.
#
# Tools, both MIT, both pinned by release and SHA-256 (appimagetool would otherwise fetch the moving
# "continuous" runtime by itself):
#   appimagetool  https://github.com/AppImage/appimagetool   the builder; a build tool only
#   type2-runtime https://github.com/AppImage/type2-runtime  the small program at the head of the
#                                                            AppImage that mounts it; it IS in the output
#
# Usage: make-appimage.sh <package-tree> <out.AppImage> <tools-dir>
set -euo pipefail

tree=$1 out=$2 tools=$3
here=$(cd "$(dirname "$0")" && pwd)
app_id=io.github.welfedted.AutoCrop

APPIMAGETOOL_VERSION=1.9.1
APPIMAGETOOL_SHA256=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
RUNTIME_VERSION=20251108
RUNTIME_SHA256=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d

mkdir -p "$tools"
fetch() { # url file sha256
  if [ ! -f "$2" ]; then
    curl --proto '=https' --tlsv1.2 -fsSL --retry 3 -o "$2" "$1"
  fi
  echo "$3  $2" | sha256sum -c -
}
tool="$tools/appimagetool-x86_64.AppImage"
runtime="$tools/runtime-x86_64"
fetch "https://github.com/AppImage/appimagetool/releases/download/$APPIMAGETOOL_VERSION/appimagetool-x86_64.AppImage" "$tool" "$APPIMAGETOOL_SHA256"
fetch "https://github.com/AppImage/type2-runtime/releases/download/$RUNTIME_VERSION/runtime-x86_64" "$runtime" "$RUNTIME_SHA256"
chmod +x "$tool"

appdir="$tools/AppDir"
rm -rf "$appdir"
mkdir -p "$appdir/usr"
cp -a "$tree/bin" "$tree/lib" "$tree/share" "$appdir/usr/"
mkdir -p "$appdir/usr/share/doc/auto-crop"
cp -a "$tree/licenses" "$tree/LICENSE-MIT" "$tree/LICENSE-APACHE" "$tree/README-PACKAGE.txt" "$appdir/usr/share/doc/auto-crop/"
if [ -f "$tree/THIRD_PARTY_NOTICES.md" ]; then cp "$tree/THIRD_PARTY_NOTICES.md" "$appdir/usr/share/doc/auto-crop/"; fi

cp "$here/$app_id.desktop" "$appdir/$app_id.desktop"
cp "$tree/share/icons/hicolor/32x32/apps/$app_id.png" "$appdir/$app_id.png"
cp "$appdir/$app_id.png" "$appdir/.DirIcon"

# `AppRun --cli ...` runs the command-line tool of the same image.
cat > "$appdir/AppRun" <<'APPRUN'
#!/bin/sh
here=$(dirname "$(readlink -f "$0")")
root=${APPDIR:-$here}
if [ "${1:-}" = "--cli" ]; then
  shift
  exec "$root/usr/bin/auto-crop" "$@"
fi
exec "$root/usr/bin/AutoCrop" "$@"
APPRUN
chmod +x "$appdir/AppRun"

# The tool is itself an AppImage: run it without FUSE. --no-appstream: there is no AppStream file yet.
rm -f "$out"
ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$tool" --no-appstream --runtime-file "$runtime" "$appdir" "$out"
chmod +x "$out"
ls -l "$out"
