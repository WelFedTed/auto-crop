# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Markdown tables from harness result files (``auto-crop-eval run`` output), several predictors side by side.

    python slices.py NAME=result.json [NAME=result.json ...] [--axes aspect framing background]

Reads only the aggregate fields and the per-image rows of each result (ids, IoU, verdict, tags).
Prints overall numbers and, per requested axis, the failure rate (IoU < 0.90, no quad counts as a
failure) and mean IoU per value. Silent failure = auto-accepted with IoU < 0.90.
"""

from __future__ import annotations

import json
import sys
from collections import defaultdict


def load(path: str) -> list[dict]:
    return json.loads(open(path, encoding="utf8").read())["images"]


def agg(rows: list[dict]) -> dict:
    n = len(rows)
    if n == 0:
        return {"n": 0}
    ious = [r.get("iou") or 0.0 for r in rows]
    fails = sum(1 for r in rows if r.get("failure") or (r.get("iou") or 0.0) < 0.9)
    acc = [r for r in rows if r.get("accepted")]
    silent = sum(1 for r in acc if (r.get("iou") or 0.0) < 0.9)
    return {
        "n": n,
        "mean_iou": sum(ious) / n,
        "fail": fails / n,
        "accepted": len(acc),
        "silent": silent,
        "no_quad": sum(1 for r in rows if r.get("status") != "ok"),
        "iou90": sum(1 for v in ious if v >= 0.9),
    }


def main(argv: list[str]) -> int:
    axes = ["aspect", "framing", "background"]
    if "--axes" in argv:
        i = argv.index("--axes")
        axes = argv[i + 1 :]
        argv = argv[:i]
    sets = {}
    for a in argv:
        name, _, path = a.partition("=")
        sets[name] = load(path)
    names = list(sets)
    print("| Slice | n | " + " | ".join(f"{n}: fail%, mean IoU" for n in names) + " |")
    print("|---|---|" + "---|" * len(names))
    first = sets[names[0]]
    groups = [("all", lambda r: True)]
    for ax in axes:
        vals = sorted({r["tags"].get(ax) for r in first if r.get("tags")} - {None})
        for v in vals:
            groups.append((f"{ax}={v}", (lambda r, ax=ax, v=v: r.get("tags", {}).get(ax) == v)))
    for label, pred in groups:
        cells, n0 = [], 0
        for n in names:
            sub = [r for r in sets[n] if pred(r)]
            a = agg(sub)
            n0 = a["n"]
            cells.append(f"{100 * a['fail']:.1f}%, {a['mean_iou']:.3f}" if a["n"] else "-")
        print(f"| {label} | {n0} | " + " | ".join(cells) + " |")
    print()
    print("| Predictor | n | mean IoU | failure % | IoU>=0.90 | no quad | auto-accepted | silent failures |")
    print("|---|---|---|---|---|---|---|---|")
    for n in names:
        a = agg(sets[n])
        print(f"| {n} | {a['n']} | {a['mean_iou']:.4f} | {100 * a['fail']:.2f} | {a['iou90']} | {a['no_quad']} | {a['accepted']} | {a['silent']} |")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
