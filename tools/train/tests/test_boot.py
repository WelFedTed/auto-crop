# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""The interval helpers of ``boot.py`` (run from tools/train: python -m unittest)."""

import sys
import unittest
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from boot import arrays, ci, cp_interval  # noqa: E402


class BootTests(unittest.TestCase):
    def test_exact_interval_matches_known_values(self):
        # 0 of 17: the one-sided rule of three is about 3/17; the two-sided 97.5% bound is 0.195.
        lo, hi = cp_interval(0, 17)
        self.assertEqual(lo, 0.0)
        self.assertAlmostEqual(hi, 0.1951, places=3)
        # 11 of 17 (the classical detector on the owner's single-item photos): the end points are where
        # the binomial tail equals 2.5% (checked here from the definition, not from the implementation).
        lo, hi = cp_interval(11, 17)
        from math import comb

        self.assertAlmostEqual(sum(comb(17, i) * lo**i * (1 - lo) ** (17 - i) for i in range(11, 18)), 0.025, places=6)
        self.assertAlmostEqual(sum(comb(17, i) * hi**i * (1 - hi) ** (17 - i) for i in range(0, 12)), 0.025, places=6)
        self.assertEqual(cp_interval(17, 17)[1], 1.0)

    def test_arrays_count_a_missing_quad_as_a_failure_and_silent_means_accepted_and_failing(self):
        rows = {
            "a": {"iou": 0.95, "failure": False, "accepted": True},
            "b": {"iou": 0.5, "failure": True, "accepted": True},
            "c": {"iou": None, "failure": True, "accepted": False},
            "d": {"iou": 0.91, "accepted": False},
        }
        d = arrays(rows, ["a", "b", "c", "d"])
        self.assertEqual(d["fail"].tolist(), [0.0, 1.0, 1.0, 0.0])
        self.assertEqual(d["silent"].tolist(), [0.0, 1.0, 0.0, 0.0])
        self.assertAlmostEqual(float(d["iou"].mean()), (0.95 + 0.5 + 0.0 + 0.91) / 4)

    def test_bootstrap_interval_contains_the_mean_and_is_degenerate_for_constants(self):
        rng = np.random.default_rng(1)
        idx = rng.integers(0, 8, size=(2000, 8))
        x = np.array([0.0, 1.0, 1.0, 0.0, 1.0, 0.0, 1.0, 1.0])
        lo, hi = ci(x, idx)
        self.assertLess(lo, x.mean())
        self.assertGreater(hi, x.mean())
        self.assertEqual(ci(np.ones(8), idx), (1.0, 1.0))


if __name__ == "__main__":
    unittest.main()
