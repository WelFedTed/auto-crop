#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
#
# Smoke test of an UNPACKED package, shared by the macOS and Linux package workflows (the Windows
# workflow has the same steps in PowerShell). The caller has already unpacked the package into a clean
# folder, moved the native build tree away and cleared the library search variables; this script only
# runs the packaged command-line tool and checks what it does:
#
#   1. `--version` and `doctor` start; doctor reports libheif, HEVC and AV1 as found
#   2. a committed AVIF (48x32) decodes through the libheif and dav1d of the package
#   3. the HEIC of the pinned libheif archive (451x461) decodes through the libde265 plugin
#   4. with the plugin folder renamed, doctor says HEVC is NOT FOUND, the HEIC fails with
#      HevcDecoderMissing (so the plugin really comes from the package) and the AVIF still decodes
#   5. with the libheif library renamed the tool does not start (it is a real, local dependency)
#
# Usage: smoke-cli.sh <cli> <plugin-dir> <libheif-link> <avif> <heic> <out-dir>
#   <libheif-link>  the file the executable names in its dependency list (the soname link, for
#                   example libheif.so.1 or libheif.1.dylib), renamed for check 5
# Prints a Markdown summary to stdout on success; any failed check exits non-zero.
set -euo pipefail

cli=$1 plugin=$2 libheif=$3 avif=$4 heic=$5 out=$6
mkdir -p "$out"

fail() { echo "SMOKE FAILED: $*" >&2; exit 1; }

# Put every renamed file back, whatever happens.
restore() {
  if [ -e "$plugin.off" ]; then mv "$plugin.off" "$plugin"; fi
  if [ -e "$libheif.off" ]; then mv "$libheif.off" "$libheif"; fi
}
trap restore EXIT

CODE=0 TEXT=""
run() { # tag file -> sets CODE and TEXT
  local tag=$1 file=$2
  set +e
  TEXT=$("$cli" dev-pipeline "$file" --json --out "$out/$tag.jpg" 2>&1)
  CODE=$?
  set -e
  echo "[$tag] exit $CODE: ${TEXT:0:300}"
}

echo "version: $("$cli" --version)"
doctor=$("$cli" doctor 2>&1) || fail "doctor failed: $doctor"
echo "$doctor" | grep -E "^(heif|convert):" || true
echo "$doctor" | grep -Eq "^heif: +built in; libheif .*HEVC found, AV1 found" || fail "doctor does not report libheif with HEVC and AV1: $doctor"

run avif "$avif"
{ [ "$CODE" -eq 0 ] && grep -q '"source":\[48,32\]' <<<"$TEXT"; } || fail "the AVIF did not decode from the package"
[ -s "$out/avif.jpg" ] || fail "no output file for the AVIF"

run heic "$heic"
{ [ "$CODE" -eq 0 ] && grep -q '"source":\[451,461\]' <<<"$TEXT"; } || fail "the HEIC did not decode from the package"
[ -s "$out/heic.jpg" ] || fail "no output file for the HEIC"

mv "$plugin" "$plugin.off"
doctor_noplugin=$("$cli" doctor 2>&1) || true
echo "$doctor_noplugin" | grep -q "HEVC NOT FOUND" || fail "doctor still finds HEVC without the plugin folder (a plugin was found somewhere else?): $doctor_noplugin"
run heic-noplugin "$heic"
{ [ "$CODE" -ne 0 ] && grep -q 'HevcDecoderMissing' <<<"$TEXT"; } || fail "the HEIC did not fail with HevcDecoderMissing without the plugin folder"
run avif-noplugin "$avif"
{ [ "$CODE" -eq 0 ] && grep -q '"source":\[48,32\]' <<<"$TEXT"; } || fail "the AVIF must decode without the plugin folder"
mv "$plugin.off" "$plugin"

mv "$libheif" "$libheif.off"
set +e
"$cli" --version >"$out/nolib.txt" 2>&1
code=$?
set -e
echo "without $(basename "$libheif"): exit $code: $(head -c 300 "$out/nolib.txt")"
[ "$code" -ne 0 ] || fail "the tool started without $(basename "$libheif"): it is not loaded from the package"
mv "$libheif.off" "$libheif"

version=$(echo "$doctor" | sed -n 's/^heif: *built in; libheif \([0-9.]*\),.*/\1/p')
cat <<SUMMARY
* \`doctor\`: libheif ${version}, HEVC and AV1 found
* AVIF 48x32 decoded, HEIC 451x461 decoded from the unpacked package
* plugin folder renamed: doctor says HEVC NOT FOUND, the HEIC fails with HevcDecoderMissing, the AVIF still decodes
* libheif library renamed: the tool does not start
SUMMARY
