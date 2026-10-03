# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Multi-item scenes (M10.51): quotas, valid ground truth, tags that match the geometry,
reproducibility at any worker count, and the single-item manifest staying unchanged."""

import json
import shutil
import sys
import tempfile
import unittest
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from synth import multi_item as M  # noqa: E402
from synth import suite  # noqa: E402

SEED = 99
COUNT = 8


def gap_frac(a, b, w, h):
    P, Q = np.asarray(a) * [w, h], np.asarray(b) * [w, h]
    return M.poly_gap(P, Q) / min(w, h)


class MultiItemTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = Path(tempfile.mkdtemp(prefix="synth-multi-"))
        cls.a, cls.b = cls.tmp / "a", cls.tmp / "b"
        M.generate_multi(cls.a, "mt", SEED, COUNT, 320, jobs=1, quiet=True)
        M.generate_multi(cls.b, "mt", SEED, COUNT, 320, jobs=2, quiet=True)
        cls.rows = [json.loads(l) for l in (cls.a / "manifest.jsonl").read_text(encoding="utf8").splitlines()]

    @classmethod
    def tearDownClass(cls):
        shutil.rmtree(cls.tmp, ignore_errors=True)

    def test_identical_bytes_at_any_worker_count(self):
        self.assertEqual(suite.hash_tree(self.a), suite.hash_tree(self.b))

    def test_quotas_are_exact(self):
        scenes = M.build_plan("q", 5, 200)
        for axis, weights in M.AXES.items():
            counts = {}
            for s in scenes:
                v = {"count": s.count_tag, "separation": s.separation, "bed": s.bed, "kind": s.kind, "clip": s.clip, "rotation": s.rotation}[axis]
                counts[v] = counts.get(v, 0) + 1
            total = sum(weights.values())
            for k, w in weights.items():
                self.assertLessEqual(abs(counts[k] - w / total * 200), 1.0, (axis, k))

    def test_rows_have_valid_items_and_a_single_item_reader_still_works(self):
        for r in self.rows:
            self.assertEqual(r["v"], 1)
            self.assertEqual(r["item_count"], len(r["items"]))
            self.assertEqual(r["quad"], r["items"][0])
            self.assertTrue(2 <= len(r["items"]) <= 8)
            for q in r["items"]:
                q = np.asarray(q)
                self.assertEqual(q.shape, (4, 2))
                area = 0.5 * (np.dot(q[:, 0], np.roll(q[:, 1], -1)) - np.dot(q[:, 1], np.roll(q[:, 0], -1)))
                self.assertGreater(area, 0, "clockwise in a y-down frame")
            self.assertEqual({"count", "separation", "bed", "kind", "clip", "contrast", "rotation"} <= set(r["tags"]), True)

    def test_the_separation_tag_matches_the_geometry(self):
        for r in self.rows:
            w, h = r["width"], r["height"]
            gaps = [gap_frac(a, b, w, h) for i, a in enumerate(r["items"]) for b in r["items"][i + 1 :]]
            cls = M._classify(min(gaps))
            if r["tags"]["bed"] not in M.FLATBED:
                # the picture is a perspective view of the plane: gaps shift by a few percent
                self.assertIn(cls, (None, r["tags"]["separation"], "close", "touching", "separated", "overlap"))
            else:
                self.assertEqual(cls, r["tags"]["separation"], r["id"])

    def test_clipped_items_leave_the_frame_only_when_tagged(self):
        for r in self.rows:
            partial = min(r["visible_fractions"]) < 0.999
            self.assertEqual(partial, r["tags"]["clip"] == "partial", r["id"])

    def test_polygon_helpers(self):
        a = M.rect_poly(50, 50, 40, 20, 0)
        b = M.rect_poly(100, 50, 40, 20, 0)
        self.assertAlmostEqual(M.poly_gap(a, b), 10.0, places=6)
        c = M.rect_poly(85, 50, 40, 20, 0)
        self.assertAlmostEqual(M.poly_gap(a, c), -5.0, places=6)
        self.assertAlmostEqual(M.poly_area_inside(a, 60, 60), 30 * 20, places=6)


if __name__ == "__main__":
    unittest.main()
