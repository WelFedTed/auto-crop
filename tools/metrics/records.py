# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Reading the `metrics` branch data tree (ROADMAP M1.64, M1.81).

Layout, written by the nightly workflow (.github/workflows/nightly-metrics.yml):

    data/<suite>/<predictor>/<YYYYMMDDTHHMMSSZ>-<sha7>.json          a PublishableMetrics (or the
                                                                      multi-item variant) as written
                                                                      by `auto-crop-eval publish`
    data/<suite>/<predictor>/<YYYYMMDDTHHMMSSZ>-<sha7>.timings.json  {"wall_ms", "images"|"scans", ...}

Only aggregates exist in this tree (the harness's publishing guard, golden-set policy B21): no image
ids, paths or per-image rows. This module normalises the two record shapes to one point type so that
the regression check and the dashboard treat them alike. Standard library only.
"""
from __future__ import annotations

import datetime as dt
import json
import os
import re

STAMP_RE = re.compile(r"^(\d{8}T\d{6}Z)-([0-9a-f]{7,40})\.json$")
SINGLE_SCHEMA = "auto-crop-metrics/1"
MULTI_SCHEMA = "auto-crop-metrics-multi/1"


def parse_stamp(stamp: str) -> str:
    return dt.datetime.strptime(stamp, "%Y%m%dT%H%M%SZ").replace(tzinfo=dt.timezone.utc).isoformat()


def _rate(k: float | None, n: float | None) -> float | None:
    return None if not n else k / n


def point_from(doc: dict, timings: dict | None, stamp: str, sha: str) -> dict:
    """One normalised point from a record. `quality` and `failure` are fractions (0..1)."""
    schema = doc.get("schema")
    if schema == SINGLE_SCHEMA:
        kind, s = "single", doc["summary"]
        quality, failure = s["mean_iou"], s["failure_rate"]
        crashed = s["n_crashed"] + s["n_missing"]
        extra = {"silent_risk_ub95": s["accepted"]["risk_ub95"], "silent_failures": s["accepted"]["silent_failures"]}

        def slice_of(v: dict) -> dict:
            return {"n": v["n"], "quality": v["summary"]["mean_iou"], "failure": v["summary"]["failure_rate"]}

    elif schema == MULTI_SCHEMA:
        kind, s = "multi", doc["summary"]
        quality = s["mean_matched_iou"]
        failure = None if not s["scans"] else 1.0 - s["perfect_scans"] / s["scans"]
        crashed = s["crashed"] + s["missing"]
        extra = {"silent_risk_ub95": s["silent_wrong_ub95"], "silent_failures": s["silent_wrong"]}

        def slice_of(v: dict) -> dict:
            sm = v["summary"]
            return {
                "n": v["n"],
                "quality": sm["mean_matched_iou"],
                "failure": None if not sm["scans"] else 1.0 - sm["perfect_scans"] / sm["scans"],
            }

    else:
        raise ValueError(f"unknown record schema {schema!r}")
    count = timings and (timings.get("images") or timings.get("scans"))
    ms = timings and (timings.get("ms_per_image_wall") or timings.get("ms_per_scan_wall"))
    return {
        "t": parse_stamp(stamp),
        "stamp": stamp,
        "commit": doc.get("commit", sha)[:7],
        "kind": kind,
        "n": doc["n"],
        "quality": quality,
        "failure": failure,
        "crashed": crashed,
        "ms_per_item": ms,
        "timed_items": count,
        "slices": {k: slice_of(v) for k, v in doc["slices"].items()},
        "suppressed_slice_count": doc.get("suppressed_slice_count", 0),
        "host": f"{doc['host']['os']}-{doc['host']['arch']}" if "host" in doc else f"{doc.get('os')}-{doc.get('arch')}",
        **extra,
    }


def load_series(root: str) -> dict[tuple[str, str], list[dict]]:
    """{(suite, predictor): points sorted by time} from `root` (the `data/` directory)."""
    out: dict[tuple[str, str], list[dict]] = {}
    if not os.path.isdir(root):
        return out
    for suite in sorted(os.listdir(root)):
        for pred in sorted(os.listdir(os.path.join(root, suite))):
            d = os.path.join(root, suite, pred)
            if not os.path.isdir(d):
                continue
            pts = []
            for name in sorted(os.listdir(d)):
                m = STAMP_RE.match(name)
                if not m:
                    continue
                with open(os.path.join(d, name), encoding="utf-8") as f:
                    doc = json.load(f)
                timings = None
                tpath = os.path.join(d, name[: -len(".json")] + ".timings.json")
                if os.path.exists(tpath):
                    with open(tpath, encoding="utf-8") as f:
                        timings = json.load(f)
                pts.append(point_from(doc, timings, m.group(1), m.group(2)))
            if pts:
                out[(suite, pred)] = sorted(pts, key=lambda p: p["stamp"])
    return out
