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
# Tool: appimagetool (MIT, https://github.com/AppImage/appimagetool), pinned release and SHA-256.
# It is a build tool only; the output contains its type 2 runtime (MIT).
#
# Usage: make-appimage.sh <package-tree> <out.AppImage> <tools-dir>
set -euo pipefail

tree=$1 out=$2 tools=$3
here=$(cd "$(dirname "$0")" && pwd)
app_id=io.github.welfedted.AutoCrop

APPIMAGETOOL_VERSION=1.9.1
APPIMAGETOOL_SHA256=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
url="https://github.com/AppImage/appimagetool/releases/download/$APPIMAGETOOL_VERSION/appimagetool-x86_64.AppImage"

mkdir -p "$tools"
tool="$tools/appimagetool-x86_64.AppImage"
if [ ! -f "$tool" ]; then
  curl --proto '=https' --tlsv1.2 -fsSL --retry 3 -o "$tool" "$url"
fi
echo "$APPIMAGETOOL_SHA256  $tool" | sha256sum -c -
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
ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$tool" --no-appstream "$appdir" "$out"
chmod +x "$out"
ls -l "$out"
