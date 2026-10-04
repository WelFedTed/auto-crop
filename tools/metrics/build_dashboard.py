# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Build the accuracy dashboard (ROADMAP M1.64): one self-contained HTML file.

    build_dashboard.py --data metrics-branch/data --out index.html

Reads the data tree of the `metrics` branch (see records.py), computes the gate table, and writes a
single HTML file with the CSS, the JavaScript (vanilla, inline SVG charts, no library and no
network request) and the data inlined. The nightly workflow commits it to the `metrics` branch as
`index.html` and attaches it as a workflow artifact. Nothing publishes it: serving it is the
owner's one-click decision (repository settings, Pages, source `metrics` branch). Aggregates only.
Standard library only.
"""
from __future__ import annotations

import argparse
import datetime as dt
import html
import json
import os
import sys

import records
import regress

HERE = os.path.dirname(os.path.abspath(__file__))
FULL_SUITE_MIN_IMAGES = 5000  # M1 exit gate G0 (PROVISIONAL)


def latest(series: dict, suite: str, pred: str) -> dict | None:
    pts = series.get((suite, pred))
    return pts[-1] if pts else None


def gates(series: dict) -> list[dict]:
    rows: list[dict] = []

    def add(name: str, status: str, detail: str) -> None:
        rows.append({"name": name, "status": status, "detail": detail})

    det = latest(series, "full-py", "detector")
    if det is None:
        add("G0: eval runs on >= 5,000 synthetic images", "pending", "no nightly record of the full suite yet")
    else:
        ok = det["n"] >= FULL_SUITE_MIN_IMAGES
        add(
            "G0: eval runs on >= 5,000 synthetic images (PROVISIONAL)",
            "pass" if ok else "fail",
            f"{det['n']} images, commit {det['commit']}, {det['t'][:10]}",
        )
    multi = latest(series, "multi-full", "items")
    add(
        "Multi-item suite scored nightly",
        "pass" if multi else "pending",
        f"{multi['n']} scans, commit {multi['commit']}" if multi else "no nightly record yet",
    )
    bad = [f"{s[0]}/{s[1]}: {p[-1]['crashed']}" for s, p in sorted(series.items()) if p[-1]["crashed"]]
    add(
        "No crashed or missing predictions in the latest records",
        "fail" if bad else ("pass" if series else "pending"),
        "; ".join(bad) if bad else f"{len(series)} series checked",
    )
    for (suite, pred), pts in sorted(series.items()):
        if pred not in regress.GATED_PREDICTORS:
            continue
        if len(pts) < 2:
            add(f"No regression vs the previous nightly: {suite} / {pred}", "pending", "needs two records")
            continue
        c = regress.compare_points(pts[-2], pts[-1])
        ok = not (c["quality_regressed"] or c["failure_regressed"])
        add(
            f"No regression vs the previous nightly: {suite} / {pred}",
            "pass" if ok else "fail",
            f"quality {regress.signed(c['dq_pt'])} (fails at -{regress.QUALITY_DROP_PT}), "
            f"failure rate {regress.signed(c['df_pt'])} (fails at +{regress.FAILURE_RISE_PT}); "
            f"{pts[-2]['commit']} to {pts[-1]['commit']}",
        )
    same: list[tuple[str, str, dict, dict]] = []
    for (suite, pred), pts in sorted(series.items()):
        for a, b in zip(pts, pts[1:]):
            if a["commit"] == b["commit"]:
                same.append((suite, pred, a, b))
    if not same:
        add("Two nightlies at one commit give identical numbers", "pending", "no two records share a commit yet")
    else:
        diff = sorted({f"{s}/{p}" for s, p, a, b in same if not regress.compare_points(a, b)["identical"]})
        add(
            "Two nightlies at one commit give identical numbers",
            "fail" if diff else "pass",
            ("differ: " + ", ".join(diff)) if diff else f"{len(same)} pair(s) of same-commit records identical",
        )
    for (suite, pred), pts in sorted(series.items()):
        p = pts[-1]
        if p["silent_risk_ub95"] is not None and pred in regress.GATED_PREDICTORS:
            add(
                f"Silent-failure risk, 95% upper bound: {suite} / {pred}",
                "info",
                f"{p['silent_risk_ub95'] * 100:.2f}% over auto-accepted results ({p['silent_failures']} silent failures); no bar yet (calibration is gated from M4)",
            )
    golden = [k for k in series if k[0].startswith("golden")]
    if golden:
        g = latest(series, *golden[0])
        add("Private golden v0 aggregate published (>= 150 images)", "pass" if g["n"] >= 150 else "fail", f"{g['n']} images")
    else:
        add("Private golden v0 aggregate published (>= 150 images)", "pending", "private set, published by hand from the owner's machine (M1.51); not part of this nightly")
    return rows


def build(data_dir: str) -> str:
    series = records.load_series(data_dir)
    payload = {
        "generated": dt.datetime.now(dt.timezone.utc).isoformat(timespec="seconds"),
        "series": [{"suite": s, "predictor": p, "kind": pts[-1]["kind"], "points": pts} for (s, p), pts in sorted(series.items())],
        "gates": gates(series),
    }
    blob = json.dumps(payload, separators=(",", ":")).replace("</", "<\\/")
    with open(os.path.join(HERE, "dashboard.css"), encoding="utf-8") as f:
        css = f.read()
    with open(os.path.join(HERE, "dashboard.js"), encoding="utf-8") as f:
        js = f.read().replace("</script", "<\\/script")
    n_rec = sum(len(p) for p in series.values())
    title = "Auto Crop accuracy"
    return f"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{html.escape(title)}</title>
<style>
{css}</style>
</head>
<body>
<header>
<h1>Auto Crop accuracy</h1>
<p class="sub">Nightly aggregates from the <code>metrics</code> branch: {len(series)} series, {n_rec} records, built {html.escape(payload['generated'])}.
Synthetic data detects change between builds; it never backs a real-world accuracy claim. Aggregates only, no image data.</p>
</header>
<main id="app"></main>
<footer><p>Static page: no network requests, no library. Data: <code>auto-crop-metrics/1</code> and <code>auto-crop-metrics-multi/1</code> records written by <code>auto-crop-eval publish</code>.</p></footer>
<script type="application/json" id="data">{blob}</script>
<script>
{js}</script>
</body>
</html>
"""


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--data", required=True)
    ap.add_argument("--out", required=True)
    args = ap.parse_args(argv)
    try:
        page = build(args.data)
    except (OSError, ValueError, KeyError) as e:
        print(f"error: {e}", file=sys.stderr)
        return 2
    with open(args.out, "w", encoding="utf-8", newline="\n") as f:
        f.write(page)
    print(f"wrote {args.out} ({len(page) // 1024} KiB)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
