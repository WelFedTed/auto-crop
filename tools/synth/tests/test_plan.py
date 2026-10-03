# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Planning: quotas, balance and scene-disjoint splits (ROADMAP M1.33, M1.35)."""

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from synth import plan  # noqa: E402


class PlanTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.scenes, cls.images = plan.build("full", plan.SUITES["full"]["seed"], plan.FULL_COUNT)
        cls.rows = [i.tags() for i in cls.images]

    def test_full_suite_is_at_least_5000_images(self):
        self.assertGreaterEqual(len(self.images), 5000)

    def test_every_slice_cell_reaches_its_quota(self):
        self.assertEqual(plan.check_quotas(self.rows), [])
        hist = plan.histogram(self.rows)
        smallest = min(n for axis in hist.values() for n in axis.values())
        self.assertGreaterEqual(smallest, plan.QUOTA_MIN_FULL)

    def test_every_planned_value_is_present_on_the_smoke_suite_too(self):
        _, images = plan.build("smoke", plan.SUITES["smoke"]["seed"], plan.SMOKE_COUNT)
        self.assertEqual(len(images), 200)
        hist = plan.histogram([i.tags() for i in images])
        for axis, values in {**plan.SCENE_AXES, **plan.IMAGE_AXES}.items():
            for v in values:
                self.assertGreater(hist[axis].get(v, 0), 0, f"{axis}={v} missing from smoke")

    def test_plans_are_a_pure_function_of_the_seed(self):
        a = plan.build("x", 5, 90)
        b = plan.build("x", 5, 90)
        self.assertEqual(a, b)
        other = plan.build("x", 6, 90)
        self.assertNotEqual([i.tags() for i in other[1]], [i.tags() for i in a[1]])

    def test_a_scene_never_spans_two_splits_and_the_split_is_about_30_70(self):
        seen: dict[str, str] = {}
        for im in self.images:
            self.assertEqual(seen.setdefault(im.scene.scene_id, im.scene.split), im.scene.split)
        dev = sum(1 for s in self.scenes if s.split == "dev") / len(self.scenes)
        self.assertAlmostEqual(dev, 0.3, delta=0.04)

    def test_scene_properties_are_shared_inside_a_scene(self):
        by_scene: dict[int, set] = {}
        for im in self.images:
            by_scene.setdefault(im.scene.index, set()).add((im.scene.aspect, im.scene.paper, im.scene.background, im.scene.ink))
        self.assertTrue(all(len(v) == 1 for v in by_scene.values()))

    def test_only_receipts_fade(self):
        for s in self.scenes:
            if s.aspect == "document":
                self.assertEqual(s.ink, "normal")
        self.assertGreater(sum(s.ink == "faded" for s in self.scenes), 400)

    def test_balanced_gives_exact_counts(self):
        import numpy as np

        seq = plan.balanced(np.random.default_rng(1), 1000, {"a": 0.5, "b": 0.3, "c": 0.2})
        self.assertEqual((seq.count("a"), seq.count("b"), seq.count("c")), (500, 300, 200))

    def test_ids_and_scene_ids_are_unique_and_prefixed(self):
        self.assertEqual(len({i.image_id for i in self.images}), len(self.images))
        self.assertTrue(all(i.image_id.startswith("full-") for i in self.images))
        self.assertTrue(all(s.scene_id.startswith("full-s") for s in self.scenes))


if __name__ == "__main__":
    unittest.main()
