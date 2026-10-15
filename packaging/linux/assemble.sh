#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
#
# Assembles the Linux x86_64 package tree (the folder that becomes the .tar.gz and the base of the
# AppImage and the .deb) from the release build and the native prefix of `cargo xtask build-native`.
# Run by .github/workflows/package-linux.yml on ubuntu-22.04 (glibc 2.35, the oldest supported); it
# needs patchelf, readelf, objdump, nm and python3.
#
# Layout (the engine finds the plugin folder through `packaged::plugin_dir_candidates`: bin/ to lib/libheif):
#   bin/AutoCrop, bin/auto-crop, bin/auto-crop-worker   RUNPATH $ORIGIN/../lib (the worker decodes HEIC and AVIF
#                                        in a sandbox, ADR-0013; it must sit beside the executables)
#   lib/libheif.so.1, libdav1d.so.7, libde265.so.0 (and their versioned files)   RUNPATH $ORIGIN
#   lib/libheif/libheif-libde265.so      the libde265 plugin of libheif (HEVC, HEIC)   RUNPATH $ORIGIN/..
#   share/applications/, share/icons/    desktop entry and icon
#   licenses/, LICENSE-*, THIRD_PARTY_NOTICES.md, README-PACKAGE.txt
#
# Environment: PREFIX (native prefix), RELEASE (cargo target/release), SRC (target/native, for the
# licence texts), OUT (the package folder to create), VERSION, COMMIT.
set -euo pipefail

: "${PREFIX:?}" "${RELEASE:?}" "${SRC:?}" "${OUT:?}" "${VERSION:?}" "${COMMIT:?}"
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
app_id=io.github.welfedted.AutoCrop

rm -rf "$OUT"
mkdir -p "$OUT/bin" "$OUT/lib/libheif" "$OUT/licenses" "$OUT/share/applications" "$OUT/share/icons/hicolor/32x32/apps"

cp "$RELEASE/AutoCrop" "$RELEASE/auto-crop" "$RELEASE/auto-crop-worker" "$OUT/bin/"
strip --strip-unneeded "$OUT/bin/AutoCrop" "$OUT/bin/auto-crop" "$OUT/bin/auto-crop-worker"
# A shipped worker has none of the test-only escape hooks (crates/worker/tests/guard.rs).
if grep -a -q -e AUTOCROP_ESCAPE -e AUTOCROP-FAULT: "$OUT/bin/auto-crop-worker"; then echo "the worker contains the escape-tests hooks" >&2; exit 1; fi

copy_libs() { # glob
  local found=0 f
  for f in "$PREFIX"/lib/$1; do
    [ -e "$f" ] || [ -L "$f" ] || continue
    cp -a "$f" "$OUT/lib/"
    found=1
  done
  [ "$found" = 1 ] || { echo "no $1 under $PREFIX/lib" >&2; exit 1; }
}
copy_libs 'libheif.so*'
copy_libs 'libdav1d.so*'
copy_libs 'libde265.so*'
plugin=$(find "$PREFIX/lib/libheif" -maxdepth 1 -type f -name 'libheif-libde265.so*' | head -1)
[ -n "$plugin" ] || { echo "the libde265 plugin of libheif was not found under $PREFIX/lib/libheif" >&2; exit 1; }
cp -a "$plugin" "$OUT/lib/libheif/"

# RUNPATHs: nothing may point into the build machine. libheif finds dav1d and libde265 beside itself,
# the plugin finds libheif and libde265 one folder up, the executables find lib/ beside bin/.
while read -r f; do patchelf --set-rpath '$ORIGIN' "$f"; strip --strip-unneeded "$f"; done < <(find "$OUT/lib" -maxdepth 1 -type f -name '*.so*')
for f in "$OUT"/lib/libheif/*; do patchelf --set-rpath '$ORIGIN/..' "$f"; strip --strip-unneeded "$f"; done
for exe in AutoCrop auto-crop auto-crop-worker; do patchelf --set-rpath '$ORIGIN/../lib' "$OUT/bin/$exe"; done

echo "== ELF files"
status=0
glibc_max=GLIBC_2.35
while read -r f; do
  echo "-- $f"
  readelf -d "$f" | grep -E "NEEDED|RUNPATH|RPATH|SONAME" || true
  case "$f" in
    */bin/*)
      # No RUNPATH other than ours, and no RPATH at all.
      readelf -d "$f" | grep -E "\((RPATH|RUNPATH)\)" | grep -qF '[$ORIGIN/../lib]' || { echo "AUDIT: $f has an unexpected RUNPATH" >&2; status=1; } ;;
  esac
  if readelf -d "$f" | grep -E "\((RPATH|RUNPATH)\)" | grep -E '/home/|/runner/|target/native'; then echo "AUDIT: $f points into the build tree" >&2; status=1; fi
  top=$(objdump -T "$f" 2>/dev/null | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1 || true)
  if [ -n "$top" ]; then
    echo "   needs up to $top"
    [ "$(printf '%s\n' "$glibc_max" "$top" | sort -V | tail -1)" = "$glibc_max" ] || { echo "AUDIT: $f needs $top, newer than $glibc_max" >&2; status=1; }
  fi
done < <(find "$OUT/bin" "$OUT/lib" -type f \( -name '*.so*' -o -path '*/bin/*' \))
# No x265, x264 or AV1 encoder may be in the shipped libraries (decision B12).
if find "$OUT/lib" -type f -name '*.so*' -print0 | xargs -0 nm -D 2>/dev/null | grep -E ' (x265_|x264_|aom_codec_av1_cx|rav1e_context_new|svt_av1_enc_init)'; then
  echo "AUDIT: an encoder symbol is in the package" >&2; status=1
fi
[ "$status" = 0 ] || { echo "the package audit failed" >&2; exit 1; }

# Licence texts (from the hash-checked source archives), notices and the pin list.
for lib in libheif libde265 dav1d; do
  srcdir=$(find "$SRC" -type d -name "$lib-*" -path '*/src/*' | head -1)
  [ -n "$srcdir" ] || { echo "source of $lib not found under $SRC" >&2; exit 1; }
  n=0
  for t in "$srcdir"/COPYING* "$srcdir"/LICENSE* "$srcdir"/COPYRIGHT*; do
    [ -f "$t" ] || continue
    cp "$t" "$OUT/licenses/$lib-$(basename "$t")"; n=$((n + 1))
  done
  [ "$n" -gt 0 ] || { echo "no licence text found in $srcdir" >&2; exit 1; }
done
cp "$root/native-deps.toml" "$OUT/licenses/native-deps.toml"
cp "$root/LICENSE-MIT" "$root/LICENSE-APACHE" "$OUT/"
if [ -f "$root/THIRD_PARTY_NOTICES.md" ]; then cp "$root/THIRD_PARTY_NOTICES.md" "$OUT/"; fi

# Desktop entry and icon (the icon is a 32 pixel placeholder).
cp "$here/$app_id.desktop" "$OUT/share/applications/$app_id.desktop"
python3 "$root/packaging/ico-to-png.py" "$root/crates/shell/icons/icon.ico" "$OUT/share/icons/hicolor/32x32/apps/$app_id.png"

sed -e "s/@VERSION@/$VERSION/g" -e "s/@COMMIT@/$COMMIT/g" "$here/README-PACKAGE.txt" > "$OUT/README-PACKAGE.txt"
echo "assembled $OUT"
