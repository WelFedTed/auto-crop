# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Unit tests for eval_parity.py (python3 -m unittest discover -s tools/ci)."""
from __future__ import annotations

import copy
import json
import os
import tempfile
import unittest

import eval_parity as p


def doc(os_name: str, ious: list[float], accepted: int = 0) -> dict:
    images = [
        {"id": f"i{k}", "iou": v, "failure": v < 0.9, "verdict": "good" if v >= 0.9 else "bad"}
        for k, v in enumerate(ious)
    ]
    n = len(ious)
    fails = sum(1 for v in ious if v < 0.9)
    return {
        "header": {
            "host": {"os": os_name, "arch": "x86_64", "tier": None},
            "eval_version": "0.0.1",
            "manifest_sha256": "abc",
            "predictor": "detector",
        },
        "summary": {
            "n": n,
            "mean_iou": sum(ious) / n,
            "failure_rate": fails / n,
            "accepted": {"silent_failures": accepted},
        },
        "images": images,
    }


class Parity(unittest.TestCase):
    def test_identical_results_on_another_host_are_byte_identical(self) -> None:
        a = doc("linux", [0.95, 0.99, 0.5])
        b = copy.deepcopy(a)
        b["header"]["host"]["os"] = "windows"
        c = p.compare(a, b)
        self.assertTrue(c["identical_bytes"])
        self.assertEqual(c["images_with_different_iou"], 0)
        self.assertEqual(c["mean_iou_delta_pt"], 0.0)

    def test_a_small_difference_is_reported_and_within_the_bar(self) -> None:
        ious = [0.95] * 999 + [0.5]
        a = doc("linux", ious)
        b = doc("macos", ious[:-1] + [0.5 + 1e-7])
        c = p.compare(a, b)
        self.assertFalse(c["identical_bytes"])
        self.assertEqual(c["images_with_different_iou"], 1)
        text, ok = p.report([a, b], 0.2)
        self.assertTrue(ok)
        self.assertIn("within 0.2 pt", text)

    def test_a_difference_above_the_bar_fails(self) -> None:
        a = doc("linux", [0.95] * 100)
        b = doc("windows", [0.95] * 99 + [0.0])
        text, ok = p.report([a, b], 0.2)
        self.assertFalse(ok)
        self.assertIn("ABOVE 0.2 pt", text)

    def test_failure_flips_are_counted(self) -> None:
        a = doc("linux", [0.95, 0.89])
        b = doc("windows", [0.95, 0.91])
        self.assertEqual(p.compare(a, b)["failure_flips"], 1)

    def test_different_images_or_manifests_are_errors_not_passes(self) -> None:
        a = doc("linux", [0.9, 0.9])
        b = doc("windows", [0.9])
        with self.assertRaises(ValueError):
            p.compare(a, b)
        c = doc("windows", [0.9, 0.9])
        c["header"]["manifest_sha256"] = "other"
        with self.assertRaises(ValueError):
            p.compare(a, c)

    def test_command_line_exit_codes(self) -> None:
        a = doc("linux", [0.95] * 100)
        b = doc("windows", [0.95] * 99 + [0.0])
        with tempfile.TemporaryDirectory() as d:
            pa, pb = os.path.join(d, "a.json"), os.path.join(d, "b.json")
            for path, value in ((pa, a), (pb, b)):
                with open(path, "w", encoding="utf-8") as f:
                    json.dump(value, f)
            self.assertEqual(p.main(["x", pa, pa]), 0)
            self.assertEqual(p.main(["x", pa, pb]), 1)
            self.assertEqual(p.main(["x", pa]), 2)
            self.assertEqual(p.main(["x", pa, os.path.join(d, "missing.json")]), 2)


if __name__ == "__main__":
    unittest.main()
