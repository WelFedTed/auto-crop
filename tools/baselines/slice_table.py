# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Per-slice comparison table of several predictors over the same suite (ROADMAP M1.29).

    slice_table.py oracle=oracle.json detector=detector.json cv_quad=cv_quad.json ... > table.md

Every argument is `label=results.json` from `auto-crop-eval run` over the SAME manifest (checked by
the manifest hash). Prints a Markdown table: one headline block and, for every slice with n >= 30
(smaller slices are never reported, the harness's publishing rule), the mean IoU and the failure
rate (IoU < 0.9) per predictor, with n. Standard library only.
"""
from __future__ import annotations

import json
import sys


def load(path: str) -> dict:
    with open(path, encoding="utf-8") as f:
        doc = json.load(f)
    if "slices" not in doc or "summary" not in doc:
        raise ValueError(f"{path}: not an auto-crop-eval results file")
    return doc


def pct(v: float | None) -> str:
    return "n/a" if v is None else f"{v * 100:.1f}"


def table(runs: list[tuple[str, dict]]) -> str:
    hashes = {d["header"]["manifest_sha256"] for _, d in runs}
    if len(hashes) != 1:
        raise ValueError("the results were scored over different manifests")
    labels = [label for label, _ in runs]
    out = [
        f"| | {' | '.join(labels)} |",
        f"|---|{'---:|' * len(labels)}",
    ]

    def row(name: str, cell) -> None:
        out.append(f"| {name} | {' | '.join(cell(d) for _, d in runs)} |")

    n = runs[0][1]["summary"]["n"]
    row(f"**all images** (n = {n}): mean IoU %", lambda d: pct(d["summary"]["mean_iou"]))
    row("failure rate % (IoU < 0.9)", lambda d: pct(d["summary"]["failure_rate"]))
    row("answered (of n)", lambda d: str(d["summary"]["n_ok"]))
    row("IoU >= 0.95 %", lambda d: pct(d["summary"]["success_95"]))
    keys = sorted(
        {s["key"] for _, d in runs for s in d["slices"] if s["status"] != "suppressed"},
        key=lambda k: (k.split("=")[0], k),
    )
    by = [{s["key"]: s for s in d["slices"]} for _, d in runs]
    for key in keys:
        ref = next(m[key] for m in by if key in m)
        cells = []
        for m in by:
            s = m.get(key)
            cells.append("n/a" if s is None else f"{pct(s['summary']['mean_iou'])} / {pct(s['summary']['failure_rate'])}")
        out.append(f"| {key} (n = {ref['n']}) | {' | '.join(cells)} |")
    out.append("")
    out.append("Cells under a slice: mean IoU % / failure rate % (IoU < 0.9).")
    return "\n".join(out) + "\n"


def main(argv: list[str]) -> int:
    runs = []
    for arg in argv[1:]:
        label, _, path = arg.partition("=")
        if not path:
            print("arguments are label=results.json", file=sys.stderr)
            return 2
        runs.append((label, load(path)))
    if len(runs) < 2:
        print(__doc__, file=sys.stderr)
        return 2
    try:
        sys.stdout.write(table(runs))
    except ValueError as e:
        print(f"error: {e}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
