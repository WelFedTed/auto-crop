# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Shared plumbing of the baseline adapters (ROADMAP M1.29).

A baseline adapter turns somebody else's tool into a predictor for the accuracy harness: it reads
a manifest (`manifest.jsonl`, see docs/testing/eval-harness.md), runs the tool on every image and
writes a JSON-lines predictions file that `auto-crop-eval run --predictor jsonl:FILE` scores:

    {"id": "smoke-00033", "quad": [[x, y] * 4], "state": "good"}
    {"id": "smoke-00034", "quad": null, "state": "failed"}

Coordinates are normalised to the EXIF-oriented image (x by width, y by height), the same
convention as the manifest, corners clockwise in a y-down frame. An adapter that cannot answer
returns None (a counted failure); an exception is recorded as a failure and the run continues.

DEV TOOLS ONLY. Nothing in this directory is linked into, bundled with or imported by the
application; the external tools (OpenCV, ImageMagick, unpaper) run as separate processes or in a
separate Python environment (docs/perf/baselines.md, docs/provenance.md).
"""
from __future__ import annotations

import argparse
import json
import math
import os
import sys
import time
from typing import Callable

Quad = list[list[float]]
Predict = Callable[[str], "dict | None"]


def read_manifest(path: str) -> list[dict]:
    """Rows with `id` and `path` (the image, absolute). Other fields are ignored: a baseline never
    sees the ground truth."""
    base = os.path.dirname(os.path.abspath(path))
    rows = []
    with open(path, encoding="utf-8") as f:
        for line in f:
            if not line.strip():
                continue
            row = json.loads(line)
            rows.append({"id": row["id"], "path": os.path.join(base, row["image"])})
    return rows


def order_clockwise(points: list[tuple[float, float]]) -> list[tuple[float, float]]:
    """Four corners in clockwise order (y-down frame), starting at the one nearest the top-left."""
    cx = sum(p[0] for p in points) / len(points)
    cy = sum(p[1] for p in points) / len(points)
    ordered = sorted(points, key=lambda p: math.atan2(p[1] - cy, p[0] - cx))
    start = min(range(len(ordered)), key=lambda i: ordered[i][0] + ordered[i][1])
    return ordered[start:] + ordered[:start]


def normalise(points: list[tuple[float, float]], width: int, height: int) -> Quad:
    return [[p[0] / width, p[1] / height] for p in points]


def prediction(row_id: str, answer: dict | None) -> dict:
    if answer is None or not answer.get("quad"):
        return {"id": row_id, "quad": None, "state": "failed"}
    out = {"id": row_id, "quad": answer["quad"], "state": answer.get("state", "good")}
    if answer.get("confidence") is not None:
        out["confidence"] = answer["confidence"]
    return out


def run(description: str, predict: Predict, argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=description)
    ap.add_argument("--manifest", required=True, help="manifest.jsonl of the suite")
    ap.add_argument("--out", required=True, help="predictions JSON-lines file to write")
    ap.add_argument("--limit", type=int, default=0, help="only the first N images (smoke test)")
    args = ap.parse_args(argv)
    rows = read_manifest(args.manifest)
    if args.limit:
        rows = rows[: args.limit]
    started = time.time()
    failed = 0
    os.makedirs(os.path.dirname(os.path.abspath(args.out)), exist_ok=True)
    with open(args.out, "w", encoding="utf-8", newline="\n") as f:
        for row in rows:
            try:
                answer = predict(row["path"])
            except Exception as e:  # noqa: BLE001 - a baseline crash is a counted failure, not an abort
                print(f"{row['id']}: {type(e).__name__}: {e}", file=sys.stderr)
                answer = None
            p = prediction(row["id"], answer)
            failed += p["quad"] is None
            f.write(json.dumps(p, separators=(",", ":")) + "\n")
    secs = time.time() - started
    print(
        f"{description}: {len(rows)} images, {failed} without an answer, "
        f"{secs:.1f} s ({secs * 1000 / max(1, len(rows)):.0f} ms per image); wrote {args.out}"
    )
    return 0
