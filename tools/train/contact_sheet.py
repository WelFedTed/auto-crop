# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""A contact sheet of a manifest with the ground-truth quad drawn (and optional predictions).

    python contact_sheet.py MANIFEST_DIR OUT.jpg [N] [--preds preds.jsonl]

Reads images and quads only. Never point it at a folder you may not copy: it writes to OUT only.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import cv2
import numpy as np

COLOURS = [(0, 0, 255), (0, 255, 255), (255, 0, 0), (255, 0, 255)]


def main(argv: list[str]) -> int:
    preds = {}
    if "--preds" in argv:
        i = argv.index("--preds")
        for line in Path(argv[i + 1]).read_text(encoding="utf8").splitlines():
            r = json.loads(line)
            preds[r["id"]] = r
        argv = argv[:i] + argv[i + 2 :]
    d, out = Path(argv[0]), argv[1]
    n = int(argv[2]) if len(argv) > 2 else 24
    rows = [json.loads(line) for line in (d / "manifest.jsonl").read_text(encoding="utf8").splitlines()][:n]
    tiles = []
    for r in rows:
        im = cv2.imread(str(d / r["image"]))
        h, w = im.shape[:2]
        q = (np.array(r["quad"]) * [w, h]).astype(np.int32)
        cv2.polylines(im, [q], True, (0, 255, 0), 1, cv2.LINE_AA)
        p = preds.get(r["id"])
        if p and p.get("quad"):
            pq = (np.array(p["quad"]) * [w, h]).astype(np.int32)
            cv2.polylines(im, [pq], True, (0, 128, 255), 1, cv2.LINE_AA)
        for i, pt in enumerate(q):
            cv2.circle(im, (int(pt[0]), int(pt[1])), 3, COLOURS[i], -1)
        s = 256 / max(h, w)
        im = cv2.resize(im, (max(1, int(w * s)), max(1, int(h * s))))
        t = np.zeros((256, 256, 3), np.uint8)
        t[: im.shape[0], : im.shape[1]] = im
        tags = r.get("tags", {})
        cv2.putText(t, f"{tags.get('aspect', '')[:4]} {tags.get('background', '')[:6]} {r.get('train_hand')}", (3, 12), cv2.FONT_HERSHEY_SIMPLEX, 0.4, (0, 255, 255), 1)
        tiles.append(t)
    while len(tiles) % 6:
        tiles.append(np.zeros((256, 256, 3), np.uint8))
    cv2.imwrite(out, np.vstack([np.hstack(tiles[i : i + 6]) for i in range(0, len(tiles), 6)]))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
