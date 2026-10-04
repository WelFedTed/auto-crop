#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
#
# Scores the CLASSICAL detector on the given suites with the repo's harness and dumps its answers
# and candidates for the offline hybrids (tools/train/hybrid.py). Run from the repository root after
#   cargo build --release -p auto-crop-eval --examples
#
#   tools/train/classical_baseline.sh smoke smoke-b test-train-mode ...      (suite directories under target/synth)
set -euo pipefail
THREADS="${THREADS:-2}"
mkdir -p target/res target/preds
for s in "$@"; do
  m="target/synth/$s/manifest.jsonl"
  target/release/auto-crop-eval run --manifest "$m" --predictor detector --threads "$THREADS" --out "target/res/$s-classical.json" --suite "$s" > "target/res/$s-classical.txt"
  RAYON_NUM_THREADS="$THREADS" target/release/examples/dump_classical "$m" "target/preds/$s-classical.jsonl" 8
done
