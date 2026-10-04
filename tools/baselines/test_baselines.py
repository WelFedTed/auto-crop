# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Tests of the baseline adapters (python -m unittest discover -s tools/baselines).

The pure-Python helpers always run; each adapter test runs only where its tool is installed
(OpenCV in the hash-locked environment, `magick`/`convert`, `unpaper`) so that a missing tool is a
skip, not a failure. Pages of KNOWN geometry are drawn here: nothing is read from a dataset.
"""
from __future__ import annotations

import json
import math
import os
import shutil
import sys
import tempfile
import unittest

import common

try:
    import cv2
    import numpy as np
except ImportError:  # pragma: no cover - the lock provides both in CI
    cv2 = None
    np = None


def polygon_iou(a: list[list[float]], b: list[list[float]]) -> float:
    pa = np.array(a, dtype=np.float32)
    pb = np.array(b, dtype=np.float32)
    inter, _ = cv2.intersectConvexConvex(pa, pb)
    union = cv2.contourArea(pa) + cv2.contourArea(pb) - inter
    return float(inter / union)


def rotated_page(w: int, h: int, pw: int, ph: int, deg: float, text: bool, bg: int) -> tuple[np.ndarray, list]:
    """A white page of pw x ph pixels rotated by `deg` degrees (clockwise on screen) about the
    middle of a w x h image on a background of grey level `bg`. Returns (BGR image, page corners)."""
    page = np.full((ph, pw, 3), 245, np.uint8)
    if text:
        for y in range(20, ph - 20, 14):
            page[y : y + 5, 20 : pw - 20] = 20
    img = np.full((h, w, 3), bg, np.uint8)
    m = cv2.getRotationMatrix2D((pw / 2, ph / 2), -deg, 1.0)  # cv2 angles are counter-clockwise
    m[0, 2] += w / 2 - pw / 2
    m[1, 2] += h / 2 - ph / 2
    mask = cv2.warpAffine(np.full((ph, pw), 255, np.uint8), m, (w, h))
    warped = cv2.warpAffine(page, m, (w, h))
    img = np.where(mask[..., None] > 127, warped, img)
    corners = [[0, 0], [pw, 0], [pw, ph], [0, ph]]
    out = [list(m @ np.array([x, y, 1.0])) for x, y in corners]
    return img, [[x / w, y / h] for x, y in out]


class Helpers(unittest.TestCase):
    def test_corners_are_ordered_clockwise_from_the_top_left(self) -> None:
        scrambled = [(9.0, 9.0), (1.0, 1.0), (1.0, 9.0), (9.0, 1.0)]
        self.assertEqual(common.order_clockwise(scrambled), [(1.0, 1.0), (9.0, 1.0), (9.0, 9.0), (1.0, 9.0)])

    def test_a_missing_answer_is_a_failed_prediction(self) -> None:
        self.assertEqual(common.prediction("a", None), {"id": "a", "quad": None, "state": "failed"})
        ok = common.prediction("b", {"quad": [[0, 0]] * 4})
        self.assertEqual((ok["state"], ok["quad"] is not None), ("good", True))

    def test_the_runner_writes_one_line_per_image_and_survives_a_crash(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            manifest = os.path.join(d, "manifest.jsonl")
            with open(manifest, "w", encoding="utf-8") as f:
                for i in range(3):
                    f.write(json.dumps({"id": f"x{i}", "image": f"images/x{i}.png", "quad": [[0, 0]] * 4}) + "\n")

            def predict(path: str) -> dict | None:
                if path.endswith("x1.png"):
                    raise RuntimeError("boom")
                return {"quad": [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]}

            out = os.path.join(d, "p.jsonl")
            common.run("t", predict, ["--manifest", manifest, "--out", out])
            with open(out, encoding="utf-8") as f:
                rows = [json.loads(line) for line in f]
            self.assertEqual([r["id"] for r in rows], ["x0", "x1", "x2"])
            self.assertEqual([r["state"] for r in rows], ["good", "failed", "good"])


@unittest.skipIf(cv2 is None, "OpenCV is not installed (tools/baselines/requirements.lock)")
class CvQuad(unittest.TestCase):
    def test_finds_a_tilted_page_on_a_dark_background(self) -> None:
        import cv_quad

        img, truth = rotated_page(480, 360, 260, 200, 12.0, text=False, bg=40)
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "page.png")
            cv2.imwrite(path, img)
            answer = cv_quad.predict(path)
        self.assertIsNotNone(answer)
        self.assertGreater(polygon_iou(answer["quad"], truth), 0.93)

    def test_a_blank_frame_is_no_answer(self) -> None:
        import cv_quad

        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "blank.png")
            cv2.imwrite(path, np.full((200, 300, 3), 128, np.uint8))
            self.assertIsNone(cv_quad.predict(path))


def has_magick() -> bool:
    return shutil.which("magick") is not None or (sys.platform != "win32" and shutil.which("convert") is not None)


@unittest.skipIf(cv2 is None or not has_magick(), "needs OpenCV and ImageMagick")
class ImDeskew(unittest.TestCase):
    def test_the_rotation_sign_matches_a_page_of_known_skew(self) -> None:
        import im_deskew

        # A text page, rotated clockwise by 6 degrees, on a background of the page's own colour
        # (so the trim removes nothing and the answer is the deskew rotation alone).
        img, _ = rotated_page(420, 300, 380, 270, 6.0, text=True, bg=245)
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "text.png")
            cv2.imwrite(path, img)
            answer = im_deskew.predict(path)
        self.assertIsNotNone(answer)
        q = answer["quad"]
        # Top edge of the predicted quad, in pixels (the image is 420 x 300).
        dx, dy = (q[1][0] - q[0][0]) * 420, (q[1][1] - q[0][1]) * 300
        edge_deg = math.degrees(math.atan2(dy, dx))
        self.assertAlmostEqual(edge_deg, 6.0, delta=1.0)


@unittest.skipIf(cv2 is None or shutil.which("unpaper") is None or not has_magick(), "needs OpenCV, ImageMagick and unpaper")
class Unpaper(unittest.TestCase):
    def test_answers_with_a_quad_inside_the_frame_for_a_page_on_a_dark_background(self) -> None:
        import unpaper_deskew

        img, _ = rotated_page(480, 360, 260, 200, 3.0, text=True, bg=20)
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "page.png")
            cv2.imwrite(path, img)
            answer = unpaper_deskew.predict(path)
        self.assertIsNotNone(answer)
        self.assertEqual(len(answer["quad"]), 4)


if __name__ == "__main__":
    unittest.main()
