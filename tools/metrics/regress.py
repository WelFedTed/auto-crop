# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Nightly regression check (ROADMAP M1.81): the new records against the previous nightly.

    regress.py --data metrics-branch/data --new staging [--out regression.md]

`--new` holds the records of tonight's run in the same layout as `--data`. For every series
(suite and predictor) the newest record of `--new` is compared with the newest record in `--data`
that is older than it. A **gated** series regresses when its quality (mean IoU; mean matched IoU
for the multi-item suite) falls by 0.3 points or more, or its failure rate (IoU < 0.9; share of
scans that are not perfect for the multi-item suite) rises by 0.5 points or more: the same
thresholds as the per-PR smoke gate (accuracy-smoke.yml), applied between two nightlies. Reference
series (full-frame) are only reported.

Also a finding rather than a threshold: **two records of the same commit must be identical**
(M1.81 acceptance: two nightlies give identical synthetic numbers). A difference at the same
commit means the generator, the harness or the host is not deterministic and is reported as a
regression-class problem.

Writes a Markdown report to `--out` (and stdout) and, when GITHUB_OUTPUT is set, `regressed=true|false`.
Exit status is always 0 for a regression (the workflow is non-blocking; an issue is the signal), 2 on bad input.
Standard library only.
"""
from __future__ import annotations

import argparse
import os
import sys

import records

QUALITY_DROP_PT = 0.3
FAILURE_RISE_PT = 0.5
GATED_PREDICTORS = {"detector", "items"}
# Floating-point noise between two runs of the same build is not a difference (parity across
# operating systems was measured at 1.7e-15; see docs/testing/eval-harness.md).
IDENTICAL_TOLERANCE = 1e-9


def pt(x: float | None) -> str:
    return "n/a" if x is None else f"{x * 100:.3f}%"


def signed(x: float | None) -> str:
    return "n/a" if x is None else "%+.3f pt" % x


def compare_points(prev: dict, new: dict) -> dict:
    """Deltas (in points) and verdicts of `new` against `prev`."""
    dq = None if prev["quality"] is None or new["quality"] is None else (new["quality"] - prev["quality"]) * 100
    df = None if prev["failure"] is None or new["failure"] is None else (new["failure"] - prev["failure"]) * 100
    same_commit = prev["commit"] == new["commit"]
    identical = (
        prev["n"] == new["n"]
        and (dq is None or abs(dq) / 100 <= IDENTICAL_TOLERANCE)
        and (df is None or abs(df) / 100 <= IDENTICAL_TOLERANCE)
    )
    return {
        "dq_pt": dq,
        "df_pt": df,
        "same_commit": same_commit,
        "identical": identical,
        "quality_regressed": dq is not None and dq <= -QUALITY_DROP_PT,
        "failure_regressed": df is not None and df >= FAILURE_RISE_PT,
        "nondeterministic": same_commit and not identical,
    }


def check(data: dict, new: dict) -> tuple[str, bool]:
    lines = [
        "| series | previous | new | quality change | failure-rate change | verdict |",
        "|---|---|---|---|---|---|",
    ]
    regressed = False
    for key, pts in sorted(new.items()):
        latest = pts[-1]
        older = [p for p in data.get(key, []) if p["stamp"] < latest["stamp"]]
        name = f"{key[0]} / {key[1]}"
        if not older:
            lines.append(f"| {name} | none | {latest['commit']} ({latest['n']}) | | | first record, nothing to compare |")
            continue
        prev = older[-1]
        c = compare_points(prev, latest)
        gated = key[1] in GATED_PREDICTORS
        verdict = []
        bad = False
        if c["nondeterministic"]:
            verdict.append("**NOT DETERMINISTIC**: same commit, different numbers")
            bad = True
        if gated and c["quality_regressed"]:
            verdict.append(f"**quality fell by {abs(c['dq_pt']):.3f} pt** (threshold {QUALITY_DROP_PT})")
            bad = True
        if gated and c["failure_regressed"]:
            verdict.append(f"**failure rate rose by {c['df_pt']:.3f} pt** (threshold {FAILURE_RISE_PT})")
            bad = True
        if not verdict:
            verdict.append("identical" if c["identical"] else ("ok" if gated else "reference, not gated"))
        regressed |= bad
        lines.append(
            f"| {name} | {prev['commit']} ({pt(prev['quality'])}) | {latest['commit']} ({pt(latest['quality'])}) | "
            f"{signed(c['dq_pt'])} | {signed(c['df_pt'])} | {'; '.join(verdict)} |"
        )
    return "\n".join(lines) + "\n", regressed


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--data", required=True, help="the data/ directory of the metrics branch (may be empty or absent)")
    ap.add_argument("--new", required=True, help="tonight's records, same layout")
    ap.add_argument("--out", help="write the Markdown report here")
    args = ap.parse_args(argv)
    try:
        new = records.load_series(args.new)
        if not new:
            print(f"no records under {args.new}", file=sys.stderr)
            return 2
        text, regressed = check(records.load_series(args.data), new)
    except (OSError, ValueError, KeyError) as e:
        print(f"error: {e}", file=sys.stderr)
        return 2
    header = (
        "Accuracy regression against the previous nightly (synthetic suites; detects change, "
        f"never real-world accuracy). Thresholds: quality -{QUALITY_DROP_PT} pt, failure rate +{FAILURE_RISE_PT} pt.\n\n"
    )
    sys.stdout.write(header + text)
    if args.out:
        with open(args.out, "w", encoding="utf-8", newline="\n") as f:
            f.write(header + text)
    out = os.environ.get("GITHUB_OUTPUT")
    if out:
        with open(out, "a", encoding="utf-8") as f:
            f.write(f"regressed={'true' if regressed else 'false'}\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
