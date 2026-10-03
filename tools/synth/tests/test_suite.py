# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""End to end: a small suite is valid, reproducible and independent of the worker count (M1.30)."""

import json
import os
import shutil
import sys
import tempfile
import unittest
from pathlib import Path

import numpy as np
from PIL import Image, ImageOps

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from synth import plan, suite  # noqa: E402

SEED = 424242
COUNT = 9  # three scenes


def signed_area(q):
    q = np.asarray(q)
    return 0.5 * float(np.dot(q[:, 0], np.roll(q[:, 1], -1)) - np.dot(q[:, 1], np.roll(q[:, 0], -1)))


def segments_cross(p1, p2, p3, p4):
    def ccw(a, b, c):
        return (c[1] - a[1]) * (b[0] - a[0]) > (b[1] - a[1]) * (c[0] - a[0])

    return ccw(p1, p3, p4) != ccw(p2, p3, p4) and ccw(p1, p2, p3) != ccw(p1, p2, p4)


class SuiteTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = Path(tempfile.mkdtemp(prefix="synth-test-"))
        cls.a, cls.b = cls.tmp / "a", cls.tmp / "b"
        # A is made in this process, B by two spawned worker processes.
        cls.summary_a = suite.generate(cls.a, "t", SEED, COUNT, 256, jobs=1, truth="full", quiet=True)
        cls.summary_b = suite.generate(cls.b, "t", SEED, COUNT, 256, jobs=2, truth="full", quiet=True)
        cls.rows = [json.loads(l) for l in (cls.a / "manifest.jsonl").read_text(encoding="utf8").splitlines()]

    @classmethod
    def tearDownClass(cls):
        shutil.rmtree(cls.tmp, ignore_errors=True)

    def test_same_seed_gives_identical_manifest_and_image_bytes_at_any_worker_count(self):
        ha, hb = suite.hash_tree(self.a), suite.hash_tree(self.b)
        self.assertEqual(ha, hb)
        self.assertEqual(self.summary_a["manifest_sha256"], self.summary_b["manifest_sha256"])
        self.assertEqual(len(ha), COUNT + 1)

    def test_the_manifest_is_v1_and_every_item_is_valid(self):
        self.assertEqual(len(self.rows), COUNT)
        ids = [r["id"] for r in self.rows]
        self.assertEqual(len(set(ids)), COUNT)
        for r in self.rows:
            self.assertEqual(r["v"], 1)
            for key in ("id", "image", "scene_id", "split", "width", "height", "quad", "tags"):
                self.assertIn(key, r)
            q = np.array(r["quad"])
            self.assertEqual(q.shape, (4, 2))
            self.assertTrue(np.isfinite(q).all())
            self.assertGreater(signed_area(q), 0, "clockwise from the top-left, y down")
            self.assertFalse(segments_cross(q[0], q[1], q[2], q[3]) or segments_cross(q[1], q[2], q[3], q[0]))
            p = Path(r["image"])
            self.assertFalse(p.is_absolute() or ".." in p.parts or "\\" in r["image"])
            self.assertIn(r["split"], ("dev", "test"))
            self.assertEqual(r["licence"], "MIT OR Apache-2.0")
            self.assertIn("procedural", r["background"]["licence"])
            self.assertNotIn("dtd", json.dumps(r).lower())

    def test_dimensions_are_those_of_the_exif_oriented_picture_and_the_tags_are_true(self):
        for r in self.rows:
            im = Image.open(self.a / r["image"])
            self.assertEqual(im.getexif().get(0x0112, 1), int(r["tags"]["exif"]), r["id"])
            self.assertEqual(r["exif_orientation"], int(r["tags"]["exif"]))
            up = ImageOps.exif_transpose(im)
            self.assertEqual(up.size, (r["width"], r["height"]), r["id"])
            self.assertEqual(max(up.size), 256)
            fmt = {"jpeg": "JPEG", "png": "PNG", "tiff": "TIFF", "webp": "WEBP"}[r["tags"]["format"]]
            self.assertEqual(im.format, fmt)
            if r["tags"]["colorspace"] == "display-p3":
                self.assertIsNotNone(im.info.get("icc_profile"))

    def test_tags_and_splits_follow_the_plan_and_scenes_do_not_straddle_splits(self):
        _, images = plan.build("t", SEED, COUNT)
        for r, im in zip(self.rows, images):
            self.assertEqual(r["tags"], im.tags())
            self.assertEqual(r["split"], im.scene.split)
        by_scene = {}
        for r in self.rows:
            self.assertEqual(by_scene.setdefault(r["scene_id"], r["split"]), r["split"])

    def test_truth_has_the_transcript_and_a_binary_clean_render(self):
        for r in self.rows:
            t = json.loads((self.a / r["truth"]).read_text(encoding="utf8"))
            self.assertEqual(t["id"], r["id"])
            self.assertGreater(len(t["lines"]), 3)
            self.assertEqual(t["quad"], r["quad"])
            clean = np.asarray(Image.open(self.a / r["clean_render"]))
            self.assertEqual(set(np.unique(clean)), {0, 255})

    def test_a_different_seed_changes_the_data(self):
        other = self.tmp / "c"
        suite.generate(other, "t", SEED + 1, 3, 256, jobs=1, truth="none", quiet=True)
        self.assertNotEqual(suite.hash_tree(other)["manifest.jsonl"], suite.hash_tree(self.a)["manifest.jsonl"])

    def test_the_suite_summary_records_the_backend_and_the_histogram(self):
        s = json.loads((self.a / "suite.json").read_text(encoding="utf8"))
        self.assertEqual(s["count"], COUNT)
        self.assertIn(s["degrade_backend"], ("builtin",) + tuple(f"augraphy-{v}" for v in ("8.2.6",)))
        self.assertIn("lighting", s["tag_histogram"])


def crash_once_worker(task):
    """A scene worker that kills its own process the first time any worker runs (marker file)."""
    marker = Path(tempfile.gettempdir()) / f"synth-crash-{os.getppid()}"
    if not marker.exists():
        marker.write_text("x")
        os._exit(1)
    return suite._render_scene(task)


class WorkerDeathTests(unittest.TestCase):
    def test_a_dying_worker_is_retried_instead_of_hanging_the_run(self):
        old = os.environ.get("SYNTH_BACKEND")
        os.environ["SYNTH_BACKEND"] = "builtin"  # the point is the pool, not Augraphy
        tmp = Path(tempfile.mkdtemp(prefix="synth-crash-test-"))
        marker = Path(tempfile.gettempdir()) / f"synth-crash-{os.getpid()}"
        marker.unlink(missing_ok=True)
        try:
            suite.generate(tmp / "ref", "w", 9, 6, 160, jobs=1, truth="none", quiet=True)
            suite.generate(tmp / "crash", "w", 9, 6, 160, jobs=2, truth="none", quiet=True, _worker=crash_once_worker)
            self.assertTrue(marker.exists(), "the worker never died, so nothing was tested")
            self.assertEqual(suite.hash_tree(tmp / "ref"), suite.hash_tree(tmp / "crash"))
        finally:
            marker.unlink(missing_ok=True)
            shutil.rmtree(tmp, ignore_errors=True)
            if old is None:
                os.environ.pop("SYNTH_BACKEND", None)
            else:
                os.environ["SYNTH_BACKEND"] = old


@unittest.skipUnless(os.environ.get("SYNTH_BACKEND") != "builtin", "needs Augraphy")
class BackendTests(unittest.TestCase):
    def test_the_default_backend_is_the_pinned_augraphy(self):
        from synth import degrade

        self.assertEqual(degrade.backend_name(), "augraphy-8.2.6")


if __name__ == "__main__":
    unittest.main()
