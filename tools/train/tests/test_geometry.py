# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Geometry and target tests for the learned-detector tools (run from tools/train: python -m unittest)."""

import sys
import unittest
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from common import canonical_iou, canonical_quad, letterbox_matrix  # noqa: E402
from data import render_targets  # noqa: E402
from decode import decode  # noqa: E402

Q = np.array([[60.0, 40.0], [180.0, 50.0], [170.0, 200.0], [50.0, 190.0]])


class GeometryTests(unittest.TestCase):
    def test_iou_is_one_for_the_same_quad_in_any_corner_order(self):
        for k in range(4):
            self.assertAlmostEqual(canonical_iou(Q, np.roll(Q, k, axis=0)), 1.0, places=6)

    def test_iou_of_a_shifted_parallelogram_matches_the_closed_form(self):
        a = np.array([[0.0, 0], [100, 0], [100, 50], [0, 50]])
        b = a + np.array([10.0, 0.0])  # shifted by 10% of the width
        self.assertAlmostEqual(canonical_iou(a, b), 0.9 / 1.1, places=6)

    def test_non_convex_prediction_scores_zero(self):
        bow = Q[[0, 2, 1, 3]]
        self.assertEqual(canonical_iou(Q, bow), 0.0)

    def test_canonical_quad_is_rotation_stable_and_clockwise(self):
        c = canonical_quad(Q)
        for k in range(4):
            np.testing.assert_allclose(canonical_quad(np.roll(Q, k, axis=0)), c)
        # clockwise in a y-down frame: positive shoelace area
        x, y = c[:, 0], c[:, 1]
        self.assertGreater(0.5 * (np.dot(x, np.roll(y, -1)) - np.dot(y, np.roll(x, -1))), 0)

    def test_letterbox_centres_and_fits_the_long_edge(self):
        m = letterbox_matrix(400, 100, 256)
        self.assertAlmostEqual(m[0, 0], 0.64)
        self.assertAlmostEqual(m[1, 2], (256 - 64) / 2)


class TargetTests(unittest.TestCase):
    def test_corner_peaks_sit_on_the_corners_and_decoding_recovers_the_quad(self):
        q = canonical_quad(Q)
        t = render_targets(q, 256, 4)
        self.assertEqual(int((t["heat"][0] >= 0.999).sum()), 4)
        # build a fake network output from the targets and decode it
        logit = lambda p: np.log(np.clip(p, 1e-4, 1 - 1e-4) / (1 - np.clip(p, 1e-4, 1 - 1e-4)))  # noqa: E731
        out = np.zeros((11, 64, 64), dtype=np.float32)
        out[0], out[1], out[2] = logit(t["heat"][0]), logit(t["heat"][1]), logit(t["heat"][2])
        out[3:11] = t["off"]
        dec = decode(out, 4)
        self.assertIsNotNone(dec)
        self.assertGreater(canonical_iou(q, dec["quad"]), 0.97)
        self.assertEqual(dec["feats"]["n_matched"], 4)

    def test_a_page_centre_outside_the_frame_gets_a_border_centre_target_that_reaches_the_corners(self):
        # A long page cut by the frame: the centre (and two corners) lie beyond the right edge.
        q = canonical_quad(np.array([[150.0, 60.0], [420.0, 60.0], [420.0, 100.0], [150.0, 100.0]]))
        t = render_targets(q, 256, 4)
        cy, cx = np.unravel_index(int(np.argmax(t["heat"][2])), t["heat"][2].shape)
        self.assertEqual((cx, cy), (63, 20))  # right border cell on the page's row
        self.assertEqual(float(t["heat"][2].max()), 1.0)
        cell = np.array([(cx + 0.5) * 4, (cy + 0.5) * 4])
        got = t["off"][:, cy, cx].reshape(4, 2) * 32.0 + cell
        np.testing.assert_allclose(got, q, atol=1e-4)


if __name__ == "__main__":
    unittest.main()
