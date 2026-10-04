# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""M4.49 question: does a stride-4 net at 256 px resolve thin pages? Failure rate against the page's
short side measured in network-input pixels (the quad's shortest edge after the 256 px letterbox).

    python resolution.py MANIFEST NAME=result.json [NAME=result.json ...]

Only pictures with framing=full (the whole page is in view) are counted. Prints a Markdown table.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import numpy as np

BUCKETS = [(0, 6), (6, 9), (9, 12), (12, 16), (16, 24), (24, 40), (40, 1000)]


def short_side_px(row: dict, size: int = 256) -> float:
    q = np.asarray(row["quad"]) * [row["width"], row["height"]]
    e = [np.linalg.norm(q[(i + 1) % 4] - q[i]) for i in range(4)]
    return float(min(e)) * size / max(row["width"], row["height"])


def main(argv: list[str]) -> int:
    rows = {}
    for line in Path(argv[0]).read_text(encoding="utf8").splitlines():
        r = json.loads(line)
        if r.get("tags", {}).get("framing", "full") == "full":
            rows[r["id"]] = short_side_px(r)
    res = {}
    for a in argv[1:]:
        name, _, path = a.partition("=")
        res[name] = {r["id"]: r for r in json.loads(Path(path).read_text(encoding="utf8"))["images"]}
    print("| page short side at 256 px | n | " + " | ".join(f"{n}: fail%, mean IoU" for n in res) + " |")
    print("|---|---|" + "---|" * len(res))
    for lo, hi in BUCKETS:
        ids = [i for i, s in rows.items() if lo <= s < hi]
        if not ids:
            continue
        cells = []
        for n, d in res.items():
            ious = np.array([d[i]["iou"] or 0.0 for i in ids if i in d])
            cells.append(f"{100 * (ious < 0.9).mean():.1f}%, {ious.mean():.3f}" if len(ious) else "-")
        print(f"| {lo} to {hi} px | {len(ids)} | " + " | ".join(cells) + " |")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
