# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Cross-OS parity of the accuracy harness (ROADMAP M1.78).

Compares `auto-crop-eval run` results of the SAME predictor over the SAME images on several
operating systems. The first file is the reference (Linux, where the numeric thresholds are
enforced); every other file must agree with it within 0.2 points of mean IoU (the M1.78 bar).

Besides that bar, the report says how the results differ, because determinism across operating
systems is itself a finding:

* whether the files are byte-identical once the host fields are normalised;
* how many images have a different IoU at all, the largest difference, and how many images flip
  between failure and success (the unit of the 0.5-point failure gate is one image at n = 200);
* the difference of the failure rate and of the count of silent failures.

Usage: eval_parity.py REFERENCE.json OTHER.json [OTHER.json ...] [--max-iou-pt 0.2]
Exit status: 0 within the bar, 1 above it, 2 on bad input. Standard library only.
"""
from __future__ import annotations

import json
import sys

MAX_MEAN_IOU_PT = 0.2


def load(path: str) -> dict:
    with open(path, encoding="utf-8") as f:
        doc = json.load(f)
    for key in ("header", "summary", "images"):
        if key not in doc:
            raise ValueError(f"{path}: not an auto-crop-eval results file (no `{key}`)")
    return doc


def normalised(doc: dict) -> str:
    """The results with the fields that legitimately differ per host removed."""
    d = json.loads(json.dumps(doc))
    d["header"].pop("host", None)
    d["header"].pop("eval_version", None)
    return json.dumps(d, sort_keys=True)


def compare(ref: dict, other: dict) -> dict:
    ri = {i["id"]: i for i in ref["images"]}
    oi = {i["id"]: i for i in other["images"]}
    if set(ri) != set(oi):
        raise ValueError("the two results do not cover the same image ids")
    if ref["header"]["manifest_sha256"] != other["header"]["manifest_sha256"]:
        raise ValueError("the two results were scored over different manifests")
    diffs = [abs(ri[k]["iou"] - oi[k]["iou"]) for k in ri]
    flips = [k for k in ri if ri[k]["failure"] != oi[k]["failure"]]
    verdict_changes = [k for k in ri if ri[k].get("verdict") != oi[k].get("verdict")]
    rs, os_ = ref["summary"], other["summary"]
    return {
        "n": len(ri),
        "identical_bytes": normalised(ref) == normalised(other),
        "mean_iou_delta_pt": (os_["mean_iou"] - rs["mean_iou"]) * 100.0,
        "failure_rate_delta_pt": ((os_["failure_rate"] or 0.0) - (rs["failure_rate"] or 0.0)) * 100.0,
        "images_with_different_iou": sum(1 for d in diffs if d > 1e-12),
        "max_iou_delta": max(diffs) if diffs else 0.0,
        "failure_flips": len(flips),
        "verdict_changes": len(verdict_changes),
        "silent_failures_delta": os_["accepted"]["silent_failures"] - rs["accepted"]["silent_failures"],
    }


def label(doc: dict) -> str:
    h = doc["header"]["host"]
    return f"{h['os']}-{h['arch']}"


def report(docs: list[dict], max_pt: float) -> tuple[str, bool]:
    ref = docs[0]
    lines = [
        f"Reference: {label(ref)}, predictor `{ref['header']['predictor']}`, {ref['summary']['n']} images, "
        f"mean IoU {ref['summary']['mean_iou'] * 100:.4f}%, failure rate {(ref['summary']['failure_rate'] or 0) * 100:.2f}%.",
        "",
        "| OS | bytes identical | mean IoU delta (pt) | failure rate delta (pt) | images with a different IoU | largest IoU delta | failure flips | silent-failure delta | bar |",
        "|---|---|---|---|---|---|---|---|---|",
    ]
    ok = True
    for other in docs[1:]:
        c = compare(ref, other)
        within = abs(c["mean_iou_delta_pt"]) <= max_pt
        ok &= within
        lines.append(
            f"| {label(other)} | {'yes' if c['identical_bytes'] else 'no'} | {c['mean_iou_delta_pt']:+.4f} | "
            f"{c['failure_rate_delta_pt']:+.3f} | {c['images_with_different_iou']} of {c['n']} | "
            f"{c['max_iou_delta']:.3g} | {c['failure_flips']} | {c['silent_failures_delta']:+d} | "
            f"{'within' if within else 'ABOVE'} {max_pt} pt |"
        )
    return "\n".join(lines) + "\n", ok


def main(argv: list[str]) -> int:
    args = argv[1:]
    max_pt = MAX_MEAN_IOU_PT
    if "--max-iou-pt" in args:
        i = args.index("--max-iou-pt")
        try:
            max_pt = float(args[i + 1])
        except (IndexError, ValueError):
            print("--max-iou-pt needs a number", file=sys.stderr)
            return 2
        del args[i : i + 2]
    if len(args) < 2:
        print(__doc__, file=sys.stderr)
        return 2
    try:
        docs = [load(p) for p in args]
        text, ok = report(docs, max_pt)
    except (OSError, ValueError, KeyError) as e:
        print(f"error: {e}", file=sys.stderr)
        return 2
    sys.stdout.write(text)
    if not ok:
        print(f"mean IoU differs by more than {max_pt} points between operating systems", file=sys.stderr)
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
