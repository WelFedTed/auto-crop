# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Harness manifest from golden-set labels (EVALUATION ONLY, private data).

    python real_manifest.py --labels _data/golden/labels --images _data --out _data/real-single.jsonl

Mirrors ``crates/eval/src/golden.rs::manifest_items`` with ``single_only``: assisted labels, negatives
and multi-item labels are left out, ``quad`` is the one item, and the tags are the label's slices and
flags. The manifest is written next to the images, which are referenced by file name, so keep it
inside the private ``_data/`` folder (gitignored). The labels and pictures are the owner's: they are
never used to train, tune or choose a checkpoint or a threshold (B21), and nothing here is committed.
The decoded size of every picture is checked against the label's, since the harness and the
predictors map quads with the manifest's size.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from common import read_image_rgb  # noqa: E402

FLAGS = ("partial_frame", "curved", "touching", "hand_held", "folded")


def main(argv=None) -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--labels", required=True)
    ap.add_argument("--images", required=True)
    ap.add_argument("--out", required=True)
    a = ap.parse_args(argv)
    rows, left = [], {"assisted": 0, "negative": 0, "multi_item": 0, "other": 0}
    for p in sorted(Path(a.labels).glob("*.json")):
        if p.name.startswith("_"):
            continue
        d = json.loads(p.read_text(encoding="utf8"))
        if "items" not in d:
            left["other"] += 1
            continue
        if d.get("assisted"):
            left["assisted"] += 1
            continue
        if not d["items"]:
            left["negative"] += 1
            continue
        if len(d["items"]) > 1:
            left["multi_item"] += 1
            continue
        it = d["items"][0]
        img = read_image_rgb(str(Path(a.images) / d["image"]))
        if (img.shape[1], img.shape[0]) != (d["width"], d["height"]):
            raise SystemExit(f"{d['id']}: decoded size {img.shape[1]}x{img.shape[0]} differs from the label's {d['width']}x{d['height']}")
        tags = {f"slice-{s}": "yes" for s in d["slices"]}
        for f in FLAGS:
            if it.get(f) or (f == "curved" and it.get("curves")):
                tags[f"flag-{f}"] = "yes"
        tags["items"] = "1"
        rows.append({"v": 1, "id": d["id"], "image": d["image"], "scene_id": d.get("scene_id") or d["id"], "split": "dev",
                     "width": d["width"], "height": d["height"], "quad": it["quad"], "tags": tags})
    Path(a.out).write_text("".join(json.dumps(r, separators=(",", ":")) + "\n" for r in rows), encoding="utf8", newline="\n")
    print(f"{len(rows)} single-item rows written; left out: {left}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
