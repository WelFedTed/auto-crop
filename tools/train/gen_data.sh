#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
#
# Generates the training, validation and held-out test data of the learned-detector study with the
# synthetic generator in TRAINING mode (tools/synth, `--train`). Run from the repository root after
#   cargo xtask synth-setup        (or: python -m venv target/synth-venv and pip install the lock)
# Output goes under target/synth/ (ignored by version control). Seeds are disjoint from every
# evaluation suite (the generator refuses an evaluation seed).
#
#   tools/train/gen_data.sh [JOBS]
set -euo pipefail
JOBS="${1:-10}"
PY=target/synth-venv/Scripts/python.exe
[ -x "$PY" ] || PY=target/synth-venv/bin/python
OUT=target/synth
cd tools/synth
gen() { # name seed count
  "../../$PY" -m synth --train --name "$1" --seed "$2" --count "$3" --truth none --jobs "$JOBS" --out "../../$OUT/$1"
}
[ -f "../../$OUT/val/manifest.jsonl" ] || gen val "$((0x7A1E1001))" 1000
for k in 1 2 3 4; do
  [ -f "../../$OUT/train$k/manifest.jsonl" ] || gen "train$k" "$((0x7A1E0000 + k))" 5000
done
[ -f "../../$OUT/test-train-mode/manifest.jsonl" ] || gen test-train-mode "$((0x7E570001))" 1000
