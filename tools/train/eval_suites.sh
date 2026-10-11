#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
#
# Scores one checkpoint on the held-out SYNTHETIC suites with the repo's harness and writes the hybrid
# variants (agree, agree_l, snap, ...). Run from the repository root after
#   tools/train/gen_eval.sh            (the evaluation suites)
#   tools/train/classical_baseline.sh SUITE...   (classical answers and candidates, same suites)
# The confidence file comes from `predict.py --fit-conf` on the SYNTHETIC validation set only.
#
#   tools/train/eval_suites.sh TAG CKPT CONF SUITE...
set -euo pipefail
TAG="$1"; CKPT="$2"; CONF="$3"; shift 3
T=target/train-venv/Scripts/python.exe
[ -x "$T" ] || T=target/train-venv/bin/python
mkdir -p target/res target/preds
for s in "$@"; do
  m="target/synth/$s/manifest.jsonl"
  $T tools/train/predict.py --ckpt "$CKPT" --conf "$CONF" --manifest "$m" --out "target/preds/$s-$TAG.jsonl"
  "${EVAL_BIN:-target/release/auto-crop-eval}" run --manifest "$m" --predictor "jsonl:target/preds/$s-$TAG.jsonl" --out "target/res/$s-$TAG.json" --suite "$s" > "target/res/$s-$TAG.txt"
  $T tools/train/hybrid.py --manifest "$m" --learned "target/preds/$s-$TAG.jsonl" --classical "target/preds/$s-classical.jsonl" --out-dir "target/preds/hyb-$s-$TAG" --thr "$(python -c "import json;print(json.load(open('$CONF'))['threshold_good'])")"
  for h in agree agree_l snap fallback; do
    "${EVAL_BIN:-target/release/auto-crop-eval}" run --manifest "$m" --predictor "jsonl:target/preds/hyb-$s-$TAG/$h.jsonl" --out "target/res/$s-$TAG-$h.json" --suite "$s" > "target/res/$s-$TAG-$h.txt"
  done
  echo "== $s ($TAG)"; head -8 "target/res/$s-$TAG.txt"
done
