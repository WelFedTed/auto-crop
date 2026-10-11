# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Curved-boundary suites: the spline matches the shared vectors, the ground truth follows the render,
and the default suites are untouched (the same tags, no new fields)."""

import json
import math
import shutil
import sys
import tempfile
import unittest
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from synth import camera, curved, plan, scene, suite  # noqa: E402
from synth import rng as R  # noqa: E402

VECTORS = Path(__file__).resolve().parents[3] / "docs" / "dev" / "curved-pages-vectors.json"


class SplineTests(unittest.TestCase):
    def test_the_catmull_rom_matches_the_shared_vectors(self):
        vec = json.loads(VECTORS.read_text(encoding="utf8"))
        checked = 0
        for c in vec["curves"]:
            pts = np.array(c["points"], dtype=np.float64)
            poly = curved.cr_polyline(pts, 64)
            self.assertEqual(len(poly), 64 * (len(pts) - 1) + 1)
            for e in c["eval"]:
                k = e["segment"] * 64 + round(e["s"] * 64)
                np.testing.assert_allclose(poly[k], e["point"], atol=1e-9, err_msg=c["name"])
                checked += 1
        self.assertGreater(checked, 20)

    def test_control_points_reproduce_an_analytic_edge(self):
        t = np.linspace(0, 1, 129)
        dense = np.c_[0.1 + 0.8 * t, 0.2 + 0.1 * np.sin(math.pi * t) + 0.02 * np.sin(3 * math.pi * t)]
        pts = curved.fit_control_points(dense, (512, 384))
        self.assertTrue(2 < len(pts) <= 32)
        np.testing.assert_array_equal(pts[0], dense[0])
        np.testing.assert_array_equal(pts[-1], dense[-1])
        spline = curved.cr_polyline(pts) * np.array([512, 384])
        err = curved._dist_to_polyline(dense * np.array([512, 384]), spline)
        self.assertLess(err.max(), curved.FIT_TOLERANCE_PX + 1e-9)
        # A straight edge needs only its two end points.
        line = np.c_[np.linspace(0.1, 0.9, 65), np.linspace(0.2, 0.5, 65)]
        self.assertEqual(len(curved.fit_control_points(line, (512, 384))), 2)


class DefaultsUntouchedTests(unittest.TestCase):
    def test_a_default_plan_has_no_bend_and_keeps_its_tags(self):
        _, images = plan.build("t", 424242, 9)
        for im in images:
            tags = im.tags()
            self.assertIn("curl", tags)
            self.assertNotIn("bend", tags)
            self.assertNotIn("curvature", tags)
            self.assertEqual(im.bend, "")

    def test_curved_suites_are_registered_and_their_seeds_reserved(self):
        from synth import trainset

        for s in curved.SUITES.values():
            self.assertIn(s["seed"], trainset.EVAL_SEEDS)


class CurvedSuiteTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = Path(tempfile.mkdtemp(prefix="synth-curved-"))
        cls.summary = suite.generate(
            cls.tmp, "ctest", 777, 6, 256, jobs=1, truth="none", quiet=True, planner=curved.build
        )
        cls.rows = [json.loads(l) for l in (cls.tmp / "manifest.jsonl").read_text(encoding="utf8").splitlines()]

    @classmethod
    def tearDownClass(cls):
        shutil.rmtree(cls.tmp, ignore_errors=True)

    def test_rows_carry_curves_next_to_the_quad_and_curved_tags(self):
        self.assertEqual(len(self.rows), 6)
        for r in self.rows:
            keys = list(r)
            if "curves" in r:
                self.assertEqual(keys.index("curves"), keys.index("quad") + 1)
            self.assertIn(r["tags"]["bend"], curved.BEND_QUOTA)
            self.assertIn(r["tags"]["curvature"], [b[0] for b in curved.BUCKETS])
            self.assertNotIn("curl", r["tags"])
            self.assertEqual(r["tags"]["curvature"], curved.bucket_of(max(r["curvature_pct"])))
            self.assertEqual(len(r["curvature_pct"]), 4)

    def test_the_curves_agree_with_the_quad_and_are_valid(self):
        names = ["top", "right", "bottom", "left"]
        bent = 0
        for r in self.rows:
            q = np.array(r["quad"])
            for i, n in enumerate(names):
                pts = r.get("curves", {}).get(n)
                if pts is None:
                    continue
                bent += 1
                pts = np.array(pts)
                self.assertTrue(3 <= len(pts) <= 32)
                np.testing.assert_allclose(pts[0], q[i], atol=1e-6)
                np.testing.assert_allclose(pts[-1], q[(i + 1) % 4], atol=1e-6)
                # strictly in order along the curve: no two neighbours coincide
                self.assertTrue(np.all(np.hypot(*np.diff(pts, axis=0).T) > 1e-9))
        self.assertGreater(bent, 0)

    def test_the_truth_follows_the_rendered_page_edge(self):
        """Two pixels inside the ground-truth boundary the page is opaque, two outside it is not."""
        scenes, images = curved.build("ct", 777, 3)
        im = images[0]
        assets = scene.build_scene(im.scene)
        rng = R.stream(im.seed, "image")
        pitch, yaw, roll = scene._pose(im, rng)
        tex = (assets.page.ink.shape[1], assets.page.ink.shape[0])
        cam, bend = scene._bent_camera(im, im.bend, tex, (pitch, yaw, roll), rng, 256)
        _, alpha, _ = camera.render(cam, assets.texture)
        cw, ch = cam.canvas
        inside = outside = total = 0
        for edge in curved.edge_samples(cam, 41)[:4]:
            for k in range(4, len(edge) - 4, 3):
                tvec = edge[k + 1] - edge[k - 1]
                tvec /= np.hypot(*tvec)
                n = np.array([tvec[1], -tvec[0]])  # one side; the sign is fixed below
                p_in, p_out = edge[k] - 2.5 * n, edge[k] + 2.5 * n
                a_in = alpha[int(np.clip(p_in[1], 0, ch - 1)), int(np.clip(p_in[0], 0, cw - 1))]
                a_out = alpha[int(np.clip(p_out[1], 0, ch - 1)), int(np.clip(p_out[0], 0, cw - 1))]
                hi, lo = max(a_in, a_out), min(a_in, a_out)
                total += 1
                inside += hi > 0.85
                outside += lo < 0.15
        self.assertGreater(total, 40)
        self.assertGreater(inside / total, 0.97)
        self.assertGreater(outside / total, 0.97)

    def test_a_bend_keeps_the_arc_length(self):
        """The in-plane compression keeps a lifted strip as long as the flat one (inextensible)."""
        b = curved.Bend("cylinder", 60.0, 0.0)
        x = np.linspace(-200, 200, 4001)
        z = b.lift(x, np.zeros_like(x), 400, 300)
        dx = b.shift(x, np.zeros_like(x), 400, 300)[0]
        xs = x + dx
        arc = np.hypot(np.diff(xs), np.diff(z)).sum()
        self.assertAlmostEqual(arc, 400.0, delta=1.5)


if __name__ == "__main__":
    unittest.main()
