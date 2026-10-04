# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Tests of the metrics tools (python3 -m unittest discover -s tools/metrics)."""
from __future__ import annotations

import json
import os
import tempfile
import unittest

import build_dashboard
import records
import regress


def single(commit: str, mean_iou: float, failure: float, n: int = 5200, crashed: int = 0) -> dict:
    return {
        "schema": records.SINGLE_SCHEMA,
        "commit": commit,
        "suite": "full-py",
        "split": "all",
        "predictor": "classical-detector(good>=0.9)",
        "host": {"os": "linux", "arch": "x86_64", "tier": None},
        "n": n,
        "summary": {
            "n": n,
            "n_crashed": crashed,
            "n_missing": 0,
            "mean_iou": mean_iou,
            "failure_rate": failure,
            "accepted": {"risk_ub95": 0.01, "silent_failures": 0},
        },
        "slices": {
            "tilt=0-10": {"n": 1700, "status": "gated", "summary": {"mean_iou": mean_iou + 0.02, "failure_rate": failure - 0.02}},
            "tilt=30-45": {"n": 1700, "status": "gated", "summary": {"mean_iou": mean_iou - 0.02, "failure_rate": failure + 0.02}},
        },
        "suppressed_slice_count": 1,
    }


def multi(commit: str, matched: float, perfect: int, scans: int = 1200) -> dict:
    summary = {
        "scans": scans,
        "mean_matched_iou": matched,
        "perfect_scans": perfect,
        "crashed": 0,
        "missing": 0,
        "silent_wrong": 0,
        "silent_wrong_ub95": 0.002,
    }
    return {
        "schema": records.MULTI_SCHEMA,
        "commit": commit,
        "os": "linux",
        "arch": "x86_64",
        "n": scans,
        "summary": summary,
        "slices": {"separation=touching": {"n": 300, "summary": dict(summary, scans=300, perfect_scans=200)}},
    }


def put(root: str, suite: str, pred: str, stamp: str, doc: dict, ms: float | None = 12.5) -> None:
    d = os.path.join(root, suite, pred)
    os.makedirs(d, exist_ok=True)
    base = f"{stamp}-{doc['commit'][:7]}"
    with open(os.path.join(d, base + ".json"), "w", encoding="utf-8") as f:
        json.dump(doc, f)
    if ms is not None:
        with open(os.path.join(d, base + ".timings.json"), "w", encoding="utf-8") as f:
            json.dump({"wall_ms": 1000, "images": doc["n"], "ms_per_image_wall": ms}, f)


class Records(unittest.TestCase):
    def test_both_record_shapes_normalise_to_one_point_type(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            put(d, "full-py", "detector", "20261004T030000Z", single("abcdef1234", 0.70, 0.40))
            put(d, "multi-full", "items", "20261004T030000Z", multi("abcdef1234", 0.97, 900), ms=None)
            s = records.load_series(d)
        a = s[("full-py", "detector")][0]
        b = s[("multi-full", "items")][0]
        self.assertEqual((a["kind"], a["quality"], a["failure"], a["ms_per_item"]), ("single", 0.70, 0.40, 12.5))
        self.assertEqual((b["kind"], b["quality"], b["failure"]), ("multi", 0.97, 0.25))
        self.assertAlmostEqual(a["slices"]["tilt=30-45"]["quality"], 0.68)
        self.assertEqual(b["slices"]["separation=touching"]["failure"], 1.0 - 200 / 300)

    def test_an_absent_or_empty_tree_is_an_empty_series(self) -> None:
        self.assertEqual(records.load_series("/no/such/dir"), {})


class Regress(unittest.TestCase):
    def run_check(self, prev: dict, new: dict, pred: str = "detector") -> tuple[str, bool]:
        with tempfile.TemporaryDirectory() as d:
            old, nu = os.path.join(d, "old"), os.path.join(d, "new")
            put(old, "full-py", pred, "20261003T030000Z", prev)
            put(nu, "full-py", pred, "20261004T030000Z", new)
            return regress.check(records.load_series(old), records.load_series(nu))

    def test_a_quality_drop_of_0_3_points_regresses(self) -> None:
        text, bad = self.run_check(single("aaaaaaa1", 0.700, 0.40), single("bbbbbbb2", 0.697, 0.40))
        self.assertTrue(bad)
        self.assertIn("quality fell by 0.300 pt", text)

    def test_a_drop_just_under_the_threshold_passes(self) -> None:
        _, bad = self.run_check(single("aaaaaaa1", 0.700, 0.40), single("bbbbbbb2", 0.6972, 0.40))
        self.assertFalse(bad)

    def test_a_failure_rate_rise_of_0_5_points_regresses(self) -> None:
        text, bad = self.run_check(single("aaaaaaa1", 0.70, 0.400), single("bbbbbbb2", 0.70, 0.405))
        self.assertTrue(bad)
        self.assertIn("failure rate rose by 0.500 pt", text)

    def test_improvements_pass(self) -> None:
        _, bad = self.run_check(single("aaaaaaa1", 0.70, 0.40), single("bbbbbbb2", 0.75, 0.30))
        self.assertFalse(bad)

    def test_a_reference_series_is_reported_but_not_gated(self) -> None:
        text, bad = self.run_check(single("aaaaaaa1", 0.30, 1.0), single("bbbbbbb2", 0.20, 1.0), pred="full-frame")
        self.assertFalse(bad)
        self.assertIn("reference, not gated", text)

    def test_the_same_commit_must_give_identical_numbers(self) -> None:
        text, bad = self.run_check(single("aaaaaaa1", 0.70, 0.40), single("aaaaaaa1", 0.7001, 0.40), pred="full-frame")
        self.assertTrue(bad)
        self.assertIn("NOT DETERMINISTIC", text)
        _, ok = self.run_check(single("aaaaaaa1", 0.70, 0.40), single("aaaaaaa1", 0.70, 0.40))
        self.assertFalse(ok)

    def test_the_multi_item_series_is_gated_on_its_perfect_scan_rate(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            old, nu = os.path.join(d, "old"), os.path.join(d, "new")
            put(old, "multi-full", "items", "20261003T030000Z", multi("aaaaaaa1", 0.97, 900))
            put(nu, "multi-full", "items", "20261004T030000Z", multi("bbbbbbb2", 0.97, 893))  # 0.58 pt worse
            text, bad = regress.check(records.load_series(old), records.load_series(nu))
        self.assertTrue(bad)
        self.assertIn("failure rate rose", text)

    def test_the_first_record_has_nothing_to_compare(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            nu = os.path.join(d, "new")
            put(nu, "full-py", "detector", "20261004T030000Z", single("aaaaaaa1", 0.7, 0.4))
            text, bad = regress.check({}, records.load_series(nu))
        self.assertFalse(bad)
        self.assertIn("first record", text)


class Dashboard(unittest.TestCase):
    def test_gates_and_page(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            put(d, "full-py", "detector", "20261003T030000Z", single("aaaaaaa1", 0.70, 0.40))
            put(d, "full-py", "detector", "20261004T030000Z", single("aaaaaaa1", 0.70, 0.40))
            put(d, "multi-full", "items", "20261004T030000Z", multi("aaaaaaa1", 0.97, 900), ms=None)
            page = build_dashboard.build(d)
            rows = {g["name"]: g for g in build_dashboard.gates(records.load_series(d))}
        self.assertEqual(rows["G0: eval runs on >= 5,000 synthetic images (PROVISIONAL)"]["status"], "pass")
        self.assertEqual(rows["Two nightlies at one commit give identical numbers"]["status"], "pass")
        self.assertEqual(rows["Multi-item suite scored nightly"]["status"], "pass")
        self.assertEqual(rows["Private golden v0 aggregate published (>= 150 images)"]["status"], "pending")
        self.assertTrue(page.startswith("<!doctype html>"))
        self.assertIn('id="data"', page)
        # Self-contained: no external script, stylesheet, image or request of any kind.
        for banned in ("src=", "href=", "http://", "https://", "fetch(", "XMLHttpRequest", "@import"):
            self.assertNotIn(banned, page.replace("http://www.w3.org/2000/svg", ""), banned)

    def test_a_small_suite_fails_gate_g0_and_a_crash_fails_its_gate(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            put(d, "full-py", "detector", "20261004T030000Z", single("aaaaaaa1", 0.7, 0.4, n=200, crashed=2))
            rows = {g["name"]: g for g in build_dashboard.gates(records.load_series(d))}
        self.assertEqual(rows["G0: eval runs on >= 5,000 synthetic images (PROVISIONAL)"]["status"], "fail")
        self.assertEqual(rows["No crashed or missing predictions in the latest records"]["status"], "fail")

    def test_an_empty_tree_still_builds(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            self.assertIn("0 series", build_dashboard.build(d))


if __name__ == "__main__":
    unittest.main()
