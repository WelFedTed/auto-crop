# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Bootstrap intervals for harness result files, several predictors on the SAME manifest.

    python boot.py NAME=result.json [NAME=result.json ...] [--base NAME] [--filter TAG=VALUE] [--reps 10000] [--seed 1]

Per predictor: mean IoU, failure rate (IoU < 0.90, no quad counts as a failure), auto-accepted count,
silent failures (auto-accepted and failing) and the silent-failure rate among all images, each with a
95% percentile-bootstrap interval over images (resampling images, not scenes: with n this small, and
with one image per scene in the owner's set, that is the unit available). With ``--base`` the
paired difference of every other predictor against it (mean IoU and failure rate; the same resampled
images for both) is reported, so "separated intervals" can be judged on the difference. Exact
Clopper-Pearson 95% intervals are added for the failure rate. Reads only the per-image rows
(id, iou, failure, accepted, tags); aggregates are all it prints.
"""

from __future__ import annotations

import argparse
import json
import math
import sys

import numpy as np


def load(path: str) -> dict[str, dict]:
    return {r["id"]: r for r in json.loads(open(path, encoding="utf8").read())["images"]}


def cp_interval(k: int, n: int, alpha: float = 0.05) -> tuple[float, float]:
    """Exact (Clopper-Pearson) interval for k of n, by bisection on the binomial tail."""
    def tail_ge(p, k, n):  # P(X >= k)
        return 1.0 - sum(math.comb(n, i) * p**i * (1 - p) ** (n - i) for i in range(k))

    def tail_le(p, k, n):  # P(X <= k)
        return sum(math.comb(n, i) * p**i * (1 - p) ** (n - i) for i in range(k + 1))

    lo = 0.0
    if k > 0:
        a, b = 0.0, 1.0
        for _ in range(60):
            m = (a + b) / 2
            if tail_ge(m, k, n) < alpha / 2:
                a = m
            else:
                b = m
        lo = (a + b) / 2
    hi = 1.0
    if k < n:
        a, b = 0.0, 1.0
        for _ in range(60):
            m = (a + b) / 2
            if tail_le(m, k, n) > alpha / 2:
                a = m
            else:
                b = m
        hi = (a + b) / 2
    return lo, hi


def arrays(rows: dict[str, dict], ids: list[str]) -> dict[str, np.ndarray]:
    iou = np.array([rows[i].get("iou") or 0.0 for i in ids], dtype=float)
    fail = np.array([1.0 if (rows[i].get("failure") or (rows[i].get("iou") or 0.0) < 0.9) else 0.0 for i in ids])
    acc = np.array([1.0 if rows[i].get("accepted") else 0.0 for i in ids])
    return {"iou": iou, "fail": fail, "acc": acc, "silent": acc * fail}


def ci(x: np.ndarray, idx: np.ndarray) -> tuple[float, float]:
    b = x[idx].mean(axis=1)
    return float(np.percentile(b, 2.5)), float(np.percentile(b, 97.5))


def main(argv=None) -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("sets", nargs="+")
    ap.add_argument("--base")
    ap.add_argument("--filter", action="append", default=[], help="TAG=VALUE: only images whose tag has this value")
    ap.add_argument("--reps", type=int, default=10000)
    ap.add_argument("--seed", type=int, default=1)
    a = ap.parse_args(argv)
    sets = {}
    for s in a.sets:
        name, _, path = s.partition("=")
        sets[name] = load(path)
    names = list(sets)
    ids = sorted(set.intersection(*[set(v) for v in sets.values()]))
    for f in a.filter:
        k, _, v = f.partition("=")
        ids = [i for i in ids if sets[names[0]][i].get("tags", {}).get(k) == v]
    n = len(ids)
    rng = np.random.default_rng(a.seed)
    idx = rng.integers(0, n, size=(a.reps, n))
    data = {nm: arrays(sets[nm], ids) for nm in names}
    print(f"n = {n} images, {a.reps} bootstrap resamples, percentile 95% intervals")
    for nm in names:
        d = data[nm]
        k = int(d["fail"].sum())
        lo, hi = cp_interval(k, n)
        m_lo, m_hi = ci(d["iou"], idx)
        f_lo, f_hi = ci(d["fail"], idx)
        s_lo, s_hi = ci(d["silent"], idx)
        print(
            f"{nm}: mean IoU {d['iou'].mean():.3f} ({m_lo:.3f}-{m_hi:.3f}); failure {100 * d['fail'].mean():.1f}% = {k}/{n} (bootstrap {100 * f_lo:.0f}-{100 * f_hi:.0f}, exact {100 * lo:.0f}-{100 * hi:.0f}); "
            f"auto-accepted {int(d['acc'].sum())}, silent failures {int(d['silent'].sum())} ({100 * d['silent'].mean():.1f}% of all images, {100 * s_lo:.0f}-{100 * s_hi:.0f})"
        )
    if a.base:
        b = data[a.base]
        for nm in names:
            if nm == a.base:
                continue
            d = data[nm]
            for key, label in (("iou", "mean IoU"), ("fail", "failure rate")):
                diff = d[key] - b[key]
                lo, hi = ci(diff, idx)
                sep = "interval excludes 0" if (lo > 0 or hi < 0) else "interval includes 0"
                unit = 1.0 if key == "iou" else 100.0
                print(f"{nm} minus {a.base}, {label}: {unit * diff.mean():+.3f}{'' if key == 'iou' else ' points'} ({unit * lo:+.3f} to {unit * hi:+.3f}), {sep}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
