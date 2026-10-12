# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Tests of the enhancement matrix aggregation and the degradation recipes (no Tesseract needed)."""

import contextlib
import io
import tempfile
import unittest
from pathlib import Path

import degrade
import enhance_matrix as m


def score(cer: float, matched: int = 5, total: int = 5) -> dict:
    return {"cer": cer, "ref_chars": 100, "edits": round(cer * 100), "amounts_matched": matched,
            "amounts_total": total, "numerics_matched": matched, "numerics_total": total}


class Aggregation(unittest.TestCase):
    def test_compare_counts_images_worse_by_more_than_a_point(self):
        base = {"a": score(0.10), "b": score(0.10), "c": score(0.10)}
        other = {"a": score(0.05), "b": score(0.105), "c": score(0.30)}
        c = m.compare(base, other)
        self.assertEqual((c["better"], c["worse"], c["n"]), (1, 1, 3))
        self.assertAlmostEqual(c["delta_mean"], (-0.05 + 0.005 + 0.20) / 3)

    def test_lost_amounts_only_counts_what_the_original_read(self):
        base = {"a": score(0.1, matched=5), "b": score(0.1, matched=2)}
        other = {"a": score(0.1, matched=3), "b": score(0.1, matched=4)}
        self.assertEqual(m.lost_amounts(base, other), 2)

    def test_tables_name_the_variant_that_got_worse(self):
        variants = [{"label": "original", "mode": "original"}, {"label": "doc", "mode": "document"}]
        results = {
            "original": {"s1": {0: {"a": score(0.20), "b": score(0.30)}}},
            "doc": {"s1": {0: {"a": score(0.05), "b": score(0.45, matched=3)}}},
        }
        summary, md = m.build_tables(results, variants, ["s1"], [0])
        cell = summary["0"]["s1"]["doc"]
        self.assertEqual((cell["worse"], cell["better"], cell["lost_amounts"]), (1, 1, 2))
        self.assertIn("thresholding_method=0", md)
        self.assertIn("| s1 |", md)
        self.assertIn("a2", md)
        self.assertIn("Mean over slices", md)

    def test_the_first_variant_must_be_the_original(self):
        with tempfile.TemporaryDirectory() as d:
            v = Path(d) / "v.json"
            v.write_text('[{"label": "clean", "mode": "clean"}]')
            with contextlib.redirect_stderr(io.StringIO()):
                code = m.main(["--bin", "x", "--slices-dir", d, "--variants", str(v), "--out", d])
            self.assertEqual(code, 2)


class Degradations(unittest.TestCase):
    def setUp(self):
        try:
            import PIL  # noqa: F401
        except ImportError:
            self.skipTest("Pillow not installed")

    def test_every_slice_keeps_the_size_and_darkens_or_fades_as_designed(self):
        from PIL import Image, ImageStat

        img = Image.new("L", (200, 150), 250)
        for name, recipe in degrade.SLICES.items():
            out = recipe(img)
            self.assertEqual(out.size, img.size, name)
            mean = ImageStat.Stat(out.convert("L")).mean[0]
            if name == "clean":
                self.assertEqual(mean, 250.0)
            elif name != "fade":  # fade keeps the paper level by design
                self.assertLess(mean, 245.0, name)
        # Shading really is uneven: the corners differ by a lot.
        shaded = degrade.SLICES["shading"](img)
        self.assertGreater(shaded.getpixel((5, 5)) - shaded.getpixel((195, 145)), 60)
        # Thermal fade keeps the paper and lifts the ink.
        faded = degrade.fade(Image.new("L", (4, 4), 25), 0.3)
        self.assertGreater(faded.getpixel((0, 0)), 150)
        self.assertEqual(degrade.fade(Image.new("L", (4, 4), 250), 0.3).getpixel((0, 0)), 250)

    def test_documents_are_written_with_exact_transcripts(self):
        font = next((f for f in degrade.DOC_FONTS if Path(f).exists()), None)
        if font is None:
            self.skipTest("no proportional font on this machine")
        with tempfile.TemporaryDirectory() as d, contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(degrade.main(["docs", "--out", d, "--count", "2", "--font", font]), 0)
            self.assertEqual(len(list(Path(d).glob("doc_*.png"))), 2)
            text = (Path(d) / "doc_000.txt").read_text()
            self.assertGreater(len(text.split()), 100)
            self.assertEqual(
                (Path(d) / "doc_000.txt").read_text(),
                (Path(d) / "doc_000.txt").read_text())
            with contextlib.redirect_stdout(io.StringIO()):
                code = degrade.main(["slices", "--src", d, "--out", str(Path(d) / "s"),
                                     "--family", "doc", "--slices", "clean", "fade"])
            self.assertEqual(code, 0)
            self.assertTrue((Path(d) / "s" / "doc-fade" / "doc_000.jpg").exists())
            self.assertTrue((Path(d) / "s" / "doc-fade" / "doc_000.txt").exists())
            self.assertTrue((Path(d) / "s" / "doc-clean" / "doc_000.png").exists())


class Prose(unittest.TestCase):
    def test_paragraphs_are_deterministic_and_have_amounts(self):
        import random

        a = degrade.paragraph(random.Random(3), 40)
        self.assertEqual(a, degrade.paragraph(random.Random(3), 40))
        self.assertRegex(a, r"\d+\.\d{2}")


if __name__ == "__main__":
    unittest.main()
