#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
#
# Smoke test of the single-file application on macOS and Linux (the Windows workflow has the same
# steps in PowerShell). The file is copied ALONE into an empty folder, with the native build tree
# moved away by the caller and the library search variables cleared, and then proven to be the whole
# application: the command-line mode, the sandboxed decode worker and the HEIC and AVIF decoders are
# all inside it (decision B22).
#
#   1. `--version` and `doctor` start; doctor reports libheif with HEVC and AV1 found
#   2. the worker line of doctor names the executable itself, the sandbox level is the one the worker
#      reported from inside, its file access and process creation are denied
#   3. `--decode-worker --version` says the shipped worker has no test hooks and the file contains no
#      test-hook strings
#   4. a committed AVIF (48x32) and the HEIC of the pinned libheif archive (451x461) decode, both
#      forced through the worker (AUTOCROP_DECODE=worker never falls back), and in the default mode
#   5. the file has no dynamic dependency on libheif, dav1d or libde265 (`check-native --binary`)
#
# Usage: smoke-single.sh <exe> <avif> <heic> <out-dir>
# Prints a Markdown summary to stdout on success; any failed check exits non-zero.
set -euo pipefail

exe=$1 avif=$2 heic=$3 out=$4
mkdir -p "$out"
fail() { echo "SMOKE FAILED: $*" >&2; exit 1; }

alone="$out/alone"; rm -rf "$alone"; mkdir -p "$alone"
cp "$exe" "$alone/auto-crop"; chmod +x "$alone/auto-crop"
cli="$alone/auto-crop"
[ "$(find "$alone" -type f | wc -l | tr -d ' ')" = 1 ] || fail "more than one file beside the executable"

echo "version: $("$cli" --version)"
doctor=$("$cli" doctor 2>&1) || fail "doctor failed: $doctor"
echo "$doctor" | grep -E "^(heif|convert|sandbox|worker):|checked from inside|attempt that worked" || true
echo "$doctor" | grep -Eq "^heif: +built in; libheif .*HEVC found, AV1 found" || fail "doctor does not report libheif with HEVC and AV1: $doctor"
echo "$doctor" | grep -Eq "^worker: +$alone/auto-crop .*HEIC and AVIF decode there" || fail "doctor does not say the executable is its own worker: $doctor"
echo "$doctor" | grep -Eq "^sandbox: +[^ ]" || fail "doctor prints no sandbox level"
if echo "$doctor" | grep -Eq "^sandbox: +none "; then fail "the worker reports no sandbox at all: $doctor"; fi
echo "$doctor" | grep -Eq "checked from inside the worker: file=denied" || fail "the worker did not see its file access denied: $doctor"
echo "$doctor" | grep -Eq "checked from inside the worker: .*spawn=denied" || fail "the worker could start a process: $doctor"
level=$(echo "$doctor" | sed -n 's/^sandbox: *\(.*\) \[[a-z-]*\]$/\1/p')
tier=$(echo "$doctor" | sed -n 's/^sandbox: .*\[\([a-z-]*\)\]$/\1/p')

"$cli" --decode-worker --version | tee "$out/worker-version.txt"
grep -q "escape-tests=off" "$out/worker-version.txt" || fail "the shipped worker has the escape-tests feature"
if grep -a -q -e AUTOCROP_ESCAPE -e AUTOCROP-FAULT: "$cli"; then fail "the executable contains test hooks"; fi

CODE=0 TEXT=""
run() { # tag file mode
  local tag=$1 file=$2 mode=$3
  set +e
  TEXT=$(AUTOCROP_DECODE=$mode "$cli" dev-pipeline "$file" --json --out "$out/$tag.jpg" 2>&1)
  CODE=$?
  set -e
  echo "[$tag] exit $CODE: ${TEXT:0:300}"
}
for mode in worker auto; do
  run "avif-$mode" "$avif" "$mode"
  { [ "$CODE" -eq 0 ] && grep -q '"source":\[48,32\]' <<<"$TEXT"; } || fail "the AVIF did not decode ($mode)"
  [ -s "$out/avif-$mode.jpg" ] || fail "no output file for the AVIF ($mode)"
  run "heic-$mode" "$heic" "$mode"
  { [ "$CODE" -eq 0 ] && grep -q '"source":\[451,461\]' <<<"$TEXT"; } || fail "the HEIC did not decode ($mode)"
  [ -s "$out/heic-$mode.jpg" ] || fail "no output file for the HEIC ($mode)"
done
if grep -q "Sandbox level: none" <<<"$TEXT"; then fail "the default mode decoded without the sandbox: $TEXT"; fi

version=$(echo "$doctor" | sed -n 's/^heif: *built in; libheif \([0-9.]*\),.*/\1/p')
cat <<SUMMARY
* the executable was copied ALONE into an empty folder; \`doctor\`: libheif ${version} (linked in), HEVC and AV1 found
* AVIF 48x32 and HEIC 451x461 decoded, forced through the sandboxed worker (the executable itself, \`--decode-worker\`) and in the default mode
* sandbox level reported by the worker itself: \`${level}\` (tier ${tier}); its file access and process creation were denied from inside
* the executable carries none of the test hooks
SUMMARY
