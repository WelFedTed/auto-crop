# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Camera geometry and analytic ground truth (ROADMAP M1.32).

``golden_quads.json`` holds the ground-truth quads of a fixed set of camera poses. These tests fail
if the camera math, the corner order or the edge-coordinate convention drifts: a changed ground
truth silently invalidates every accuracy number measured against old suites. To accept a deliberate
change, regenerate the file with ``python tests/test_geometry.py --regenerate`` and say why in the
commit.
"""

import json
import sys
import unittest
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from synth import camera, plan, scene, verify  # noqa: E402
from synth import rng as R  # noqa: E402

GOLDEN = Path(__file__).with_name("golden_quads.json")
TOL = 1e-6

# (name, texture size, pitch, yaw, roll, partial, curl)
POSES = [
    ("flat-front", (648, 838), 0.0, 0.0, 0.0, False, None),
    ("pitch45", (648, 838), 45.0, 0.0, 0.0, False, None),
    ("yaw-45", (648, 838), 0.0, -45.0, 0.0, False, None),
    ("both-and-roll", (320, 1600), 30.0, -40.0, 33.0, False, None),
    ("roll-170", (320, 1600), 5.0, 8.0, -170.0, False, None),
    ("strip", (320, 3600), -20.0, 12.0, 70.0, False, None),
    ("partial", (648, 838), 15.0, 25.0, 10.0, True, None),
    ("curl-edge", (648, 838), 20.0, 10.0, 5.0, False, camera.Curl("edge", 90.0, 0.7, 0.4)),
]


def compute_quads() -> dict:
    out = {}
    for name, tex, pitch, yaw, roll, partial, curl in POSES:
        rng = R.stream(20261003, "golden", name)
        cam = camera.fit(tex, pitch, yaw, roll, rng, 512, partial, curl)
        out[name] = {"canvas": list(cam.canvas), "quad": cam.quad_norm().tolist()}
    return out


class GeometryTests(unittest.TestCase):
    def test_ground_truth_has_not_drifted(self):
        golden = json.loads(GOLDEN.read_text(encoding="utf8"))
        now = compute_quads()
        self.assertEqual(set(now), set(golden))
        for name in golden:
            self.assertEqual(now[name]["canvas"], golden[name]["canvas"], name)
            diff = np.abs(np.array(now[name]["quad"]) - np.array(golden[name]["quad"])).max()
            self.assertLess(diff, TOL, f"{name}: ground truth moved by {diff:g} (normalised units)")

    def test_quads_are_clockwise_from_the_pages_top_left(self):
        for name, v in compute_quads().items():
            q = np.array(v["quad"]) * np.array(v["canvas"])
            area = 0.5 * (np.dot(q[:, 0], np.roll(q[:, 1], -1)) - np.dot(q[:, 1], np.roll(q[:, 0], -1)))
            self.assertGreater(area, 0, f"{name}: not clockwise in a y-down frame")

    def test_the_f64_matrix_maps_texture_corners_to_the_quad_and_back(self):
        rng = R.stream(1, "m")
        cam = camera.fit((648, 838), 25.0, -30.0, 12.0, rng, 512, False)
        wt, ht = cam.tex
        corners = np.array([[0, 0], [wt, 0], [wt, ht], [0, ht]], dtype=np.float64)
        v = np.c_[corners, np.ones(4)] @ cam.h_tex.T
        np.testing.assert_allclose(v[:, :2] / v[:, 2:3], cam.quad_px(), atol=1e-9)
        back = np.c_[cam.quad_px(), np.ones(4)] @ np.linalg.inv(cam.h_tex).T
        np.testing.assert_allclose(back[:, :2] / back[:, 2:3], corners, atol=1e-6)
        self.assertEqual(cam.h_tex.dtype, np.float64)

    def test_full_framing_keeps_all_corners_inside_and_partial_cuts_the_page(self):
        for i in range(12):
            rng = R.stream(7, "frame", i)
            full = camera.fit((648, 838), 30.0, 20.0, 40.0, rng, 512, False)
            q = full.quad_px()
            self.assertTrue(np.all(q[:, 0] > 0) and np.all(q[:, 0] < full.canvas[0]) and np.all(q[:, 1] > 0) and np.all(q[:, 1] < full.canvas[1]))
            part = camera.fit((648, 838), 30.0, 20.0, 40.0, R.stream(7, "part", i), 512, True)
            vis = camera.visible_fraction(part.quad_px(), part.canvas)
            self.assertTrue(0.55 <= vis <= 0.93, vis)

    def test_inverse_warp_matches_the_clean_render_for_every_tilt_and_aspect(self):
        rows = list(verify.geometry_samples(40, 20261003))
        self.assertGreaterEqual(len(rows), 20)
        self.assertEqual({r[2] for r in rows}, {"0-10", "10-30", "30-45"})
        self.assertGreaterEqual(len({r[3] for r in rows}), 3)
        worst = min(r[1] for r in rows)
        self.assertGreaterEqual(worst, verify.SSIM_MIN, f"worst SSIM {worst:.4f}")

    def test_the_check_has_teeth_a_one_and_a_half_pixel_error_fails_it(self):
        scenes, images = plan.build("geo", 20261003, 24)
        a = verify.clean_assets(images[0].scene)
        im = next(i for i in images if i.scene.index == images[0].scene.index and i.framing == "full" and i.curl == "flat")
        img, quad, meta = scene.render_image(a, im, 640, geometry_only=True)
        good, _ = verify.unwarp_ssim(img, quad, a.texture)
        h, w = img.shape[:2]
        bad, _ = verify.unwarp_ssim(img, quad + np.array([1.5 / w, 1.5 / h]), a.texture)
        self.assertGreaterEqual(good, verify.SSIM_MIN)
        self.assertLess(bad, verify.SSIM_MIN)

    def test_a_curled_page_is_framed_with_its_displaced_corners(self):
        rng = R.stream(3, "curl")
        curl = camera.Curl("edge", 120.0, 0.0, 0.2)
        flat = camera.fit((648, 838), 0.0, 0.0, 0.0, R.stream(3, "c"), 512, False)
        cur = camera.fit((648, 838), 0.0, 0.0, 0.0, R.stream(3, "c"), 512, False, curl)
        # Lifting the right edge towards the camera enlarges it: the right corners move outwards.
        self.assertNotEqual(flat.quad_norm().tolist(), cur.quad_norm().tolist())
        rgb, alpha, shade = camera.render(cur, np.full((838, 648, 3), 200, np.uint8))
        self.assertEqual(rgb.shape[:2], alpha.shape)
        # The rendered page covers the area the ground-truth quad encloses, within a few percent.
        cw, ch = cur.canvas
        q = (cur.quad_norm() * [cw, ch]).astype(np.float32)
        import cv2

        mask = np.zeros((ch, cw), np.float32)
        cv2.fillConvexPoly(mask, np.round(q).astype(np.int32), 1.0)
        inter = float((mask * (alpha > 0.5)).sum())
        union = float(np.maximum(mask, alpha > 0.5).sum())
        self.assertGreater(inter / union, 0.93)
        del rng


def regenerate():
    GOLDEN.write_text(json.dumps(compute_quads(), indent=1) + "\n", encoding="utf8")
    print("wrote", GOLDEN)


if __name__ == "__main__":
    if "--regenerate" in sys.argv:
        regenerate()
    else:
        unittest.main()
