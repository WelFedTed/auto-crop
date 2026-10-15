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
#   6. HEIC and AVIF are decoded by the SANDBOXED WORKER (ADR-0013): every decode above runs with
#      AUTOCROP_DECODE=worker, which never falls back, doctor prints the sandbox level the worker itself
#      reported and what it saw from inside, the worker carries none of the test hooks, and with the
#      worker renamed a forced-worker decode fails while the default mode says "sandbox: none"
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
worker="$(dirname "$cli")/auto-crop-worker"
restore() {
  if [ -e "$worker.off" ]; then mv "$worker.off" "$worker"; fi
  if [ -e "$plugin.off" ]; then mv "$plugin.off" "$plugin"; fi
  if [ -e "$libheif.off" ]; then mv "$libheif.off" "$libheif"; fi
}
trap restore EXIT

CODE=0 TEXT=""
run() { # tag file -> sets CODE and TEXT
  local tag=$1 file=$2
  set +e
  TEXT=$(AUTOCROP_DECODE=worker "$cli" dev-pipeline "$file" --json --out "$out/$tag.jpg" 2>&1)
  CODE=$?
  set -e
  echo "[$tag] exit $CODE: ${TEXT:0:300}"
}

echo "version: $("$cli" --version)"
doctor=$("$cli" doctor 2>&1) || fail "doctor failed: $doctor"
echo "$doctor" | grep -E "^(heif|convert):" || true
echo "$doctor" | grep -Eq "^heif: +built in; libheif .*HEVC found, AV1 found" || fail "doctor does not report libheif with HEVC and AV1: $doctor"

# The sandbox: the level is what the worker reported, never assumed.
[ -f "$worker" ] || fail "no auto-crop-worker beside the tool: $worker"
echo "$doctor" | grep -E "^(sandbox|worker):|checked from inside|attempt that worked" || true
echo "$doctor" | grep -Eq "^worker: +.*auto-crop-worker.*HEIC and AVIF decode there" || fail "doctor does not say the worker is in use: $doctor"
echo "$doctor" | grep -Eq "^sandbox: +[^ ]" || fail "doctor prints no sandbox level"
if echo "$doctor" | grep -Eq "^sandbox: +none "; then fail "the worker reports no sandbox at all: $doctor"; fi
echo "$doctor" | grep -Eq "checked from inside the worker: file=denied" || fail "the worker did not see its file access denied: $doctor"
echo "$doctor" | grep -Eq "checked from inside the worker: .*spawn=denied" || fail "the worker could start a process: $doctor"
level=$(echo "$doctor" | sed -n 's/^sandbox: *\(.*\) \[[a-z-]*\]$/\1/p')
tier=$(echo "$doctor" | sed -n 's/^sandbox: .*\[\([a-z-]*\)\]$/\1/p')
"$worker" --version | tee "$out/worker-version.txt"
grep -q "escape-tests=off" "$out/worker-version.txt" || fail "the shipped worker has the escape-tests feature"
if grep -a -q -e AUTOCROP_ESCAPE -e AUTOCROP-FAULT: "$worker"; then fail "the shipped worker contains test hooks"; fi

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

 # 6. without the worker a forced-worker decode fails; the default mode says so and decodes unsandboxed
mv "$worker" "$worker.off"
run noworker-forced "$avif"
[ "$CODE" -ne 0 ] || fail "a forced-worker decode succeeded without the worker (it ran in-process?)"
doctor_noworker=$("$cli" doctor 2>&1) || true
echo "$doctor_noworker" | grep -Eq "^sandbox: +none " || fail "doctor without the worker does not say sandbox none: $doctor_noworker"
echo "$doctor_noworker" | grep -q "not found beside the executable" || fail "doctor does not say the worker is missing"
TEXT=$("$cli" dev-pipeline "$avif" --json --out "$out/noworker-auto.jpg" 2>&1) && echo "auto without the worker decodes in-process (level none): ok"
echo "$TEXT" | grep -q "Sandbox level: none" || fail "no in-process notice without the worker: $TEXT"
mv "$worker.off" "$worker"

version=$(echo "$doctor" | sed -n 's/^heif: *built in; libheif \([0-9.]*\),.*/\1/p')
cat <<SUMMARY
* \`doctor\`: libheif ${version}, HEVC and AV1 found
* AVIF 48x32 decoded, HEIC 451x461 decoded from the unpacked package
* plugin folder renamed: doctor says HEVC NOT FOUND, the HEIC fails with HevcDecoderMissing, the AVIF still decodes
* libheif library renamed: the tool does not start
* HEIC and AVIF decoded by the sandboxed worker (AUTOCROP_DECODE=worker never falls back): sandbox level reported by the worker itself: \`${level}\` (tier ${tier})
* worker renamed: a forced-worker decode fails, the default mode prints the one-time notice and level none
SUMMARY
