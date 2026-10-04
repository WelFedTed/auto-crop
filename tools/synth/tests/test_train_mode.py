# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Training mode (``--train``) leaves the evaluation suites alone and keeps their seeds out of training."""

import sys
import unittest
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from synth import backgrounds, plan, scene, trainset  # noqa: E402
from synth import rng as R  # noqa: E402


class TrainModeTests(unittest.TestCase):
    def test_evaluation_seeds_are_refused(self):
        for seed in (plan.SUITES["smoke"]["seed"], plan.SUITES["full"]["seed"], 1234567, 987654321, 424242):
            with self.assertRaises(ValueError):
                trainset.check_seed(seed)
        trainset.check_seed(0x7A1E_0001)

    def test_training_quotas_name_only_known_kinds(self):
        known = set(backgrounds.KINDS) | set(backgrounds.TRAIN_KINDS)
        self.assertLessEqual(set(trainset.OVERRIDES["background"]), known)
        scenes, images = plan.build("t", 0x7A1E_0001, 60, trainset.overrides())
        self.assertTrue(all(i.exif == 1 and i.colorspace == "srgb" for i in images))
        self.assertTrue(all(s.background in known for s in scenes))

    def test_new_backgrounds_render(self):
        for kind in backgrounds.TRAIN_KINDS:
            bg = backgrounds.make(kind, R.stream(5, kind), 48, 64)
            self.assertEqual(bg.shape, (48, 64, 3))
            self.assertTrue(np.isfinite(bg).all())

    def test_hands_change_pixels_but_not_the_ground_truth_and_default_is_untouched(self):
        scenes, images = plan.build("t", 0x7A1E_0001, 3, trainset.overrides())
        assets = scene.build_scene(scenes[0])
        spec = images[0]
        base, q0, _ = scene.render_image(assets, spec, 200)
        same, q1, _ = scene.render_image(assets, spec, 200, extras=None)
        zero, q2, _ = scene.render_image(assets, spec, 200, extras={"hand_p": 0.0})
        hand, q3, meta = scene.render_image(assets, spec, 200, extras={"hand_p": 1.0})
        self.assertTrue(np.array_equal(base, same) and np.array_equal(base, zero))
        self.assertTrue(np.array_equal(q0, q3))
        self.assertIn("hand", meta)
        self.assertFalse(np.array_equal(base, hand))


if __name__ == "__main__":
    unittest.main()
