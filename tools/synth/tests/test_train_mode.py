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

    def test_second_cycle_extras_leave_the_default_alone_and_keep_the_truth(self):
        scenes, images = plan.build("t", 0x7A1E_0002, 24, trainset.overrides({"framing": {"full": 1.0}}))
        none = {"hand_p": 0.0, "tight_p": 0.0, "mat_p": 0.0, "wow_p": 0.0}
        n_tight = n_mat = n_wow = 0
        for sc, spec in zip(scenes[::3], images[::3]):
            base_assets = scene.build_scene(sc)
            zero_assets = scene.build_scene(sc, none)
            self.assertTrue(np.array_equal(base_assets.texture, zero_assets.texture))
            base, q0, _ = scene.render_image(base_assets, spec, 160)
            zero, q1, _ = scene.render_image(zero_assets, spec, 160, extras=none)
            self.assertTrue(np.array_equal(base, zero) and np.array_equal(q0, q1))
            on = {"hand_p": 0.0, "tight_p": 1.0, "mat_p": 1.0, "wow_p": 1.0}
            assets = scene.build_scene(sc, on)
            img, q, meta = scene.render_image(assets, spec, 160, extras=on)
            self.assertTrue(np.isfinite(q).all())
            self.assertTrue(meta["tight"] and meta["white_on_white"])
            n_tight += meta["tight"]
            n_wow += bool(meta.get("white_on_white"))
            if sc.aspect == "document":
                self.assertTrue(meta.get("mat"))
                self.assertFalse(np.array_equal(assets.texture, base_assets.texture))
                n_mat += 1
            else:
                self.assertTrue(np.array_equal(assets.texture, base_assets.texture))
            # tight: the page's bounding box reaches (nearly) both a vertical and a horizontal limit
            cw, ch = img.shape[1], img.shape[0]
            lo, hi = q.min(axis=0), q.max(axis=0)
            self.assertTrue(max((hi - lo)[0], (hi - lo)[1]) > 0.88)
        self.assertGreater(n_tight + n_wow, 0)

    def test_mat_is_a_picture_inside_a_white_border(self):
        tex = np.full((120, 160, 3), 240, np.uint8)
        out = trainset.apply_mat(tex, R.stream(3, "m"))
        self.assertTrue(np.array_equal(out[0], tex[0]) and np.array_equal(out[:, 0], tex[:, 0]))
        self.assertFalse(np.array_equal(out, tex))
        self.assertGreater(float(out[40:80, 60:100].astype(float).std()), 3.0)


if __name__ == "__main__":
    unittest.main()
