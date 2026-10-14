#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
#
# Builds a .deb from the assembled package tree (packaging/linux/assemble.sh) with plain dpkg-deb (no
# download, no Tauri bundler). The tree goes to /usr/lib/auto-crop (bin/ and lib/ stay siblings, so
# the RUNPATHs and the plugin folder lookup keep working), with symlinks in /usr/bin. The package
# depends on the host WebKitGTK 4.1 and GTK 3.
#
# Usage: make-deb.sh <package-tree> <version> <out.deb>
set -euo pipefail

tree=$1 version=$2 out=$3
app_id=io.github.welfedted.AutoCrop
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT

lib="$stage/usr/lib/auto-crop"
mkdir -p "$lib" "$stage/usr/bin" "$stage/usr/share/doc/auto-crop" "$stage/DEBIAN"
cp -a "$tree/bin" "$tree/lib" "$lib/"
ln -s ../lib/auto-crop/bin/AutoCrop "$stage/usr/bin/AutoCrop"
ln -s ../lib/auto-crop/bin/auto-crop "$stage/usr/bin/auto-crop"
cp -a "$tree/share" "$stage/usr/"
cp -a "$tree/licenses" "$tree/LICENSE-MIT" "$tree/LICENSE-APACHE" "$tree/README-PACKAGE.txt" "$stage/usr/share/doc/auto-crop/"
if [ -f "$tree/THIRD_PARTY_NOTICES.md" ]; then cp "$tree/THIRD_PARTY_NOTICES.md" "$stage/usr/share/doc/auto-crop/"; fi
cat > "$stage/usr/share/doc/auto-crop/copyright" <<'COPYRIGHT'
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: auto-crop
Source: https://github.com/WelFedTed/auto-crop

Files: *
Copyright: 2026 Auto Crop contributors
License: MIT or Apache-2.0
 See LICENSE-MIT and LICENSE-APACHE in this directory.

Files: usr/lib/auto-crop/lib/*
Copyright: the libheif, libde265 and dav1d authors
License: LGPL-3.0-or-later (libheif, libde265) and BSD-2-Clause (dav1d)
 Separate, dynamically linked, replaceable libraries; the licence texts are in the
 licenses directory of this package, the pinned sources are listed in licenses/native-deps.toml.
COPYRIGHT
find "$stage/usr/share/doc/auto-crop" -type f -exec chmod 644 {} +
find "$stage" -type d -exec chmod 755 {} +

size=$(du -sk "$stage/usr" | cut -f1)
cat > "$stage/DEBIAN/control" <<CONTROL
Package: auto-crop
Version: $version
Section: graphics
Priority: optional
Architecture: amd64
Installed-Size: $size
Depends: libc6 (>= 2.35), libstdc++6 (>= 11), libgcc-s1, libwebkit2gtk-4.1-0, libgtk-3-0
Maintainer: Auto Crop contributors <WelFedTed@users.noreply.github.com>
Homepage: https://github.com/WelFedTed/auto-crop
Description: crops, straightens and cleans up scans and photos
 Auto Crop finds the page in a photo or scan, rotates and crops it, and converts
 formats (HEIC, HEIF and AVIF are read through libheif, dav1d and libde265).
 This is an early pre-release test build; see /usr/share/doc/auto-crop/README-PACKAGE.txt.
CONTROL

rm -f "$out"
dpkg-deb --root-owner-group --build "$stage" "$out"
dpkg-deb -I "$out"
ls -l "$out"
