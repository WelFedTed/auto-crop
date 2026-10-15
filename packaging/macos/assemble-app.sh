#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
#
# Assembles AutoCrop.app (macOS, unsigned apart from the free ad-hoc signature) from the release
# build and the native prefix of `cargo xtask build-native`. Run by .github/workflows/package-macos.yml
# on a macOS runner; it needs otool, install_name_tool, codesign and iconutil (all part of macOS and
# the Xcode command line tools) and a python3.
#
# Layout (the engine finds the plugin folder through `packaged::plugin_dir_candidates`):
#   AutoCrop.app/Contents/Info.plist
#   AutoCrop.app/Contents/MacOS/AutoCrop            the desktop app
#   AutoCrop.app/Contents/MacOS/auto-crop           the command-line tool
#   AutoCrop.app/Contents/MacOS/auto-crop-worker    the sandboxed decode worker (ADR-0013), beside the executables
#   AutoCrop.app/Contents/Frameworks/               libheif, dav1d, libde265 dylibs (@rpath)
#   AutoCrop.app/Contents/Frameworks/libheif/       the libde265 plugin of libheif (HEVC, HEIC)
#   AutoCrop.app/Contents/Resources/                icon, licence texts, native-deps.toml
#
# Environment: PREFIX (native prefix), RELEASE (cargo target/release), SRC (target/native, for the
# licence texts), OUT (an empty folder that receives AutoCrop.app), VERSION, COMMIT.
set -euo pipefail

: "${PREFIX:?}" "${RELEASE:?}" "${SRC:?}" "${OUT:?}" "${VERSION:?}" "${COMMIT:?}"
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)

app="$OUT/AutoCrop.app"
contents="$app/Contents"
fw="$contents/Frameworks"
rm -rf "$app"
mkdir -p "$contents/MacOS" "$fw/libheif" "$contents/Resources/licenses"

# Executables
cp "$RELEASE/AutoCrop" "$RELEASE/auto-crop" "$RELEASE/auto-crop-worker" "$contents/MacOS/"
# A shipped worker has none of the test-only escape hooks (crates/worker/tests/guard.rs).
if grep -a -q -e AUTOCROP_ESCAPE -e AUTOCROP-FAULT: "$contents/MacOS/auto-crop-worker"; then echo "the worker contains the escape-tests hooks" >&2; exit 1; fi

# Libraries: copy with the symlinks (libheif.dylib -> libheif.1.dylib -> libheif.1.23.5.dylib).
copy_libs() { # name-prefix
  local found=0 f
  for f in "$PREFIX"/lib/$1*.dylib; do
    [ -e "$f" ] || [ -L "$f" ] || continue
    cp -a "$f" "$fw/"
    found=1
  done
  [ "$found" = 1 ] || { echo "no $1*.dylib under $PREFIX/lib" >&2; exit 1; }
}
copy_libs libheif.
copy_libs libdav1d.
copy_libs libde265.
plugin=$(find "$PREFIX/lib/libheif" -maxdepth 1 -type f -name 'libheif-libde265.*' | head -1)
[ -n "$plugin" ] || { echo "the libde265 plugin of libheif was not found under $PREFIX/lib/libheif" >&2; exit 1; }
# Keep the file name the build gave it (libheif looks for the suffix of its own platform build).
plugin_name=$(basename "$plugin")
cp -a "$plugin" "$fw/libheif/$plugin_name"

echo "== Mach-O files before the edits"
find "$contents" -type f \( -name '*.dylib' -o -name '*.so' -o -path '*/MacOS/*' \) | sort | while read -r f; do
  echo "-- $f"; otool -L "$f" | tail -n +2
done

# Rewrites install names, dependency paths and rpaths of one Mach-O file so that nothing points into
# the build machine: every dependency that is not a system library becomes @rpath/<file name>, the
# install name of a library keeps its own file name (the executables name libheif.1.dylib), and the
# only rpath left is the one given.
fix_macho() { # file rpath
  local f=$1 rp=$2 dep base id
  # A dylib has an install name; the plugin (a Mach-O bundle) has none.
  id=$(otool -D "$f" | tail -n +2)
  if [ -n "$id" ]; then
    base=$(basename "$id")
    install_name_tool -id "@rpath/$base" "$f" </dev/null
  fi
  while read -r dep; do
    case "$dep" in
      /usr/lib/* | /System/* | @rpath/*) ;;
      *) install_name_tool -change "$dep" "@rpath/$(basename "$dep")" "$f" </dev/null ;;
    esac
  done < <(otool -L "$f" | tail -n +2 | awk '{print $1}')
  while read -r old; do
    install_name_tool -delete_rpath "$old" "$f" </dev/null
  done < <(otool -l "$f" | awk '/cmd LC_RPATH/ {r=1} r && /^ *path / {print $2; r=0}')
  install_name_tool -add_rpath "$rp" "$f"
}

real_files() { find "$1" -maxdepth 1 -type f -name "$2"; }

while read -r f; do fix_macho "$f" "@loader_path"; done < <(real_files "$fw" '*.dylib')
fix_macho "$fw/libheif/$plugin_name" "@loader_path/.."
for exe in AutoCrop auto-crop auto-crop-worker; do fix_macho "$contents/MacOS/$exe" "@executable_path/../Frameworks"; done

echo "== Mach-O files after the edits"
find "$contents" -type f \( -name '*.dylib' -o -name '*.so' -o -path '*/MacOS/*' \) | sort | while read -r f; do
  echo "-- $f"; otool -L "$f" | tail -n +2; otool -l "$f" | awk '/cmd LC_RPATH/ {r=1} r && /^ *path / {print "   rpath " $2; r=0}'
done

# Audit: nothing may point into the build machine, and every @rpath dependency must exist in the
# bundle. (This is the seed of ROADMAP M8.15.)
status=0
while read -r f; do
  while read -r dep; do
    case "$dep" in
      /usr/lib/* | /System/*) ;;
      @rpath/*)
        n=${dep#@rpath/}
        if [ ! -e "$fw/$n" ] && [ ! -e "$fw/../$n" ]; then echo "AUDIT: $f needs $dep, which is not in the bundle" >&2; status=1; fi ;;
      *) echo "AUDIT: $f depends on $dep" >&2; status=1 ;;
    esac
  done < <(otool -L "$f" | tail -n +2 | awk '{print $1}')
  while read -r rp; do
    case "$rp" in @loader_path | @loader_path/.. | @executable_path/../Frameworks) ;; *) echo "AUDIT: $f has the rpath $rp" >&2; status=1 ;; esac
  done < <(otool -l "$f" | awk '/cmd LC_RPATH/ {r=1} r && /^ *path / {print $2; r=0}')
done < <(find "$contents" -type f \( -name '*.dylib' -o -name '*.so' -o -path '*/MacOS/*' \))
[ "$status" = 0 ] || { echo "the bundle still refers to the build machine" >&2; exit 1; }
# No x265, x264 or AV1 encoder may be in the shipped libraries (decision B12).
if find "$fw" -type f \( -name '*.dylib' -o -name '*.so' \) -print0 | xargs -0 nm -gU 2>/dev/null | grep -E '_(x265_|x264_|aom_codec_av1_cx|rav1e_context_new|svt_av1_enc_init)'; then
  echo "an encoder symbol is in the bundle" >&2; exit 1
fi

# Licence texts (from the hash-checked source archives), notices and the pin list.
for lib in libheif libde265 dav1d; do
  srcdir=$(find "$SRC" -type d -name "$lib-*" -path '*/src/*' | head -1)
  [ -n "$srcdir" ] || { echo "source of $lib not found under $SRC" >&2; exit 1; }
  n=0
  for t in "$srcdir"/COPYING* "$srcdir"/LICENSE* "$srcdir"/COPYRIGHT*; do
    [ -f "$t" ] || continue
    cp "$t" "$contents/Resources/licenses/$lib-$(basename "$t")"; n=$((n + 1))
  done
  [ "$n" -gt 0 ] || { echo "no licence text found in $srcdir" >&2; exit 1; }
done
cp "$root/native-deps.toml" "$contents/Resources/licenses/native-deps.toml"
cp "$root/LICENSE-MIT" "$root/LICENSE-APACHE" "$contents/Resources/"
if [ -f "$root/THIRD_PARTY_NOTICES.md" ]; then cp "$root/THIRD_PARTY_NOTICES.md" "$contents/Resources/"; fi

# Icon: the repository has only the 32x32 PNG inside icon.ico (a placeholder); scaled up with sips.
work=$(mktemp -d)
python3 "$root/packaging/ico-to-png.py" "$root/crates/shell/icons/icon.ico" "$work/icon.png"
iconset="$work/AutoCrop.iconset"
mkdir -p "$iconset"
for s in 16 32 64 128 256 512 1024; do
  sips -z "$s" "$s" "$work/icon.png" --out "$iconset/tmp_$s.png" >/dev/null
done
cp "$iconset/tmp_16.png" "$iconset/icon_16x16.png"
cp "$iconset/tmp_32.png" "$iconset/icon_16x16@2x.png"
cp "$iconset/tmp_32.png" "$iconset/icon_32x32.png"
cp "$iconset/tmp_64.png" "$iconset/icon_32x32@2x.png"
cp "$iconset/tmp_128.png" "$iconset/icon_128x128.png"
cp "$iconset/tmp_256.png" "$iconset/icon_128x128@2x.png"
cp "$iconset/tmp_256.png" "$iconset/icon_256x256.png"
cp "$iconset/tmp_512.png" "$iconset/icon_256x256@2x.png"
cp "$iconset/tmp_512.png" "$iconset/icon_512x512.png"
cp "$iconset/tmp_1024.png" "$iconset/icon_512x512@2x.png"
rm -f "$iconset"/tmp_*.png
iconutil -c icns "$iconset" -o "$contents/Resources/AutoCrop.icns"
rm -rf "$work"

# Info.plist
min=${MACOSX_DEPLOYMENT_TARGET:-12.0}
sed -e "s/@VERSION@/$VERSION/g" -e "s/@MINIMUM_SYSTEM_VERSION@/$min/g" "$here/Info.plist.in" > "$contents/Info.plist"
plutil -lint "$contents/Info.plist"

# Ad-hoc signature (free, no Apple account): inner to outer. install_name_tool invalidated every
# earlier signature, so everything is signed again.
while read -r f; do codesign --force --sign - "$f" </dev/null; done < <(find "$fw" -type f \( -name '*.dylib' -o -name '*.so' \))
for exe in auto-crop auto-crop-worker AutoCrop; do codesign --force --sign - "$contents/MacOS/$exe"; done
codesign --force --deep --sign - "$app"
codesign --verify --deep --strict --verbose=2 "$app"
codesign -dvv "$app" 2>&1 | grep -E "^(Identifier|Format|Signature|Authority|TeamIdentifier)" || true
codesign -dvv "$app" 2>&1 | grep -q "Signature=adhoc" || { echo "the signature is not ad-hoc" >&2; exit 1; }
echo "assembled $app"
