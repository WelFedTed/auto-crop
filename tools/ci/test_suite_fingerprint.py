# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Unit tests for suite_fingerprint.py (python3 -m unittest discover -s tools/ci)."""
from __future__ import annotations

import os
import tempfile
import unittest

import suite_fingerprint as s


def make_suite(root: str, files: dict[str, bytes], manifest: bytes = b"{}\n") -> str:
    os.makedirs(os.path.join(root, "images"))
    with open(os.path.join(root, "manifest.jsonl"), "wb") as f:
        f.write(manifest)
    for name, data in files.items():
        with open(os.path.join(root, "images", name), "wb") as f:
            f.write(data)
    return root


class Fingerprint(unittest.TestCase):
    def test_identical_suites_compare_identical_and_a_changed_image_is_counted(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            a = s.fingerprint(make_suite(os.path.join(d, "a"), {"1.png": b"x", "2.png": b"y"}))
            b = s.fingerprint(make_suite(os.path.join(d, "b"), {"1.png": b"x", "2.png": b"y"}))
            c = s.fingerprint(make_suite(os.path.join(d, "c"), {"1.png": b"x", "2.png": b"z"}, b"[]\n"))
            self.assertEqual(s.compare(a, b), "manifest identical; 2 of 2 image files byte-identical")
            self.assertEqual(s.compare(a, c), "manifest DIFFERENT; 1 of 2 image files byte-identical")


if __name__ == "__main__":
    unittest.main()
