#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
#
# Regenerates the evaluation suites of docs/perf/detector-baseline.md with the UNCHANGED default
# generator (no --train): smoke, smoke-b, smoke-c, mid-d and full. They are eval-only; the training
# seeds (tools/synth/synth/trainset.py) never equal these. Run from the repository root.
#
#   tools/train/gen_eval.sh [JOBS]
set -euo pipefail
JOBS="${1:-4}"
PY=target/synth-venv/Scripts/python.exe
[ -x "$PY" ] || PY=target/synth-venv/bin/python
OUT=target/synth
cd tools/synth
run() { # name, extra args...
  local name="$1"; shift
  [ -f "../../$OUT/$name/manifest.jsonl" ] || "../../$PY" -m synth --name "$name" --jobs "$JOBS" --out "../../$OUT/$name" "$@"
}
run smoke --suite smoke
run smoke-b --suite smoke --seed 1234567
run smoke-c --suite smoke --seed 987654321
run mid-d --suite full --count 1000 --seed 424242
run full --suite full
