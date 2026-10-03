# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Tests of the OCR oracle's metrics and plumbing (no Tesseract needed).

Run: `python3 -m unittest discover -s tools/ocr_oracle`.
"""

from __future__ import annotations

import contextlib
import io
import json
import random
import tempfile
import unittest
from pathlib import Path

import ocr_oracle as o


class Levenshtein(unittest.TestCase):
    def test_known_distances(self):
        self.assertEqual(o.levenshtein("kitten", "sitting"), 3)
        self.assertEqual(o.levenshtein("", "abc"), 3)
        self.assertEqual(o.levenshtein("abc", ""), 3)
        self.assertEqual(o.levenshtein("same", "same"), 0)
        self.assertEqual(o.levenshtein("flaw", "lawn"), 2)

    def test_symmetric(self):
        rng = random.Random(1)
        for _ in range(50):
            a = "".join(rng.choice("ab 1.") for _ in range(rng.randrange(0, 12)))
            b = "".join(rng.choice("ab 1.") for _ in range(rng.randrange(0, 12)))
            self.assertEqual(o.levenshtein(a, b), o.levenshtein(b, a))


class Cer(unittest.TestCase):
    def test_identical_and_whitespace_only_differences(self):
        ref = "TOTAL      12.50\nVAT  2.08\n"
        self.assertEqual(o.cer(ref, "TOTAL 12.50\n\n\nVAT 2.08"), 0.0)

    def test_separator_rules_are_not_scored(self):
        ref = "TOTAL 1.00\n--------------------------------\nTHANK YOU\n"
        self.assertEqual(o.cer(ref, "TOTAL 1.00\nTHANK YOU"), 0.0)  # OCR dropped the rule
        self.assertEqual(o.cer(ref, "TOTAL 1.00\n_____\nTHANK YOU"), 0.0)  # or garbled it
        self.assertGreater(o.cer(ref, "TOTAL 1.00\n- 5 -\nTHANK YOU"), 0.0)  # short debris counts

    def test_one_wrong_digit(self):
        ref = "TOTAL 12.50"  # 11 characters
        self.assertAlmostEqual(o.cer(ref, "TOTAL 12.60"), 1 / 11)

    def test_empty_hypothesis_is_total_loss(self):
        self.assertEqual(o.cer("abc def", ""), 1.0)

    def test_empty_reference_is_an_error(self):
        with self.assertRaises(ValueError):
            o.cer("  \n", "x")


class Tokens(unittest.TestCase):
    def test_amounts_need_two_decimals_and_exact_match(self):
        ref = "MILK 1.20\nBREAD 2,35\nTOTAL 3.55\nQTY 3"
        hyp = "MILK 1.20\nBREAD 2.35\nTOTAL 3.55"  # comma became a point: wrong
        self.assertEqual(o.token_recall(o.AMOUNT_RE, ref, hyp), (2, 3))

    def test_multiset(self):
        ref = "1.00 1.00 1.00"
        self.assertEqual(o.token_recall(o.AMOUNT_RE, ref, "1.00"), (1, 3))
        self.assertEqual(o.token_recall(o.AMOUNT_RE, ref, "1.00 1.00 1.00 1.00"), (3, 3))

    def test_numerics_include_integers(self):
        m, t = o.token_recall(o.NUMERIC_RE, "Receipt 12345 on 03/10/2026 total 9.99", "Receipt 12345 total 9.99")
        self.assertEqual(t, 5)  # 12345, 03, 10, 2026, 9.99
        self.assertEqual(m, 2)

    def test_pool(self):
        a = o.score("TOTAL 12.50", "TOTAL 12.50")
        b = o.score("TOTAL 12.50", "TOTAL 12.60")
        agg = o.pool([a, b])
        self.assertEqual(agg["images"], 2)
        self.assertAlmostEqual(agg["cer_mean"], 0.5 / 11)
        self.assertAlmostEqual(agg["cer_pooled"], 1 / 22)
        self.assertEqual(agg["amounts_accuracy"], 0.5)


class Settings(unittest.TestCase):
    def test_command_line_carries_every_fixed_setting(self):
        argv = o.tesseract_argv("tesseract", Path("a.png"), 2, ["-c", "preserve_interword_spaces=1"])
        joined = " ".join(argv)
        for needle in ("-l eng", "--oem 1", "--psm 6", "--dpi 300", "thresholding_method=2",
                       "preserve_interword_spaces=1"):
            self.assertIn(needle, joined)

    def test_thresholding_is_a_required_choice(self):
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            o.main(["run", "--images", ".", "--label", "x"])


class Synthetic(unittest.TestCase):
    def test_transcripts_are_deterministic_and_well_formed(self):
        a = o.receipt_text(random.Random(7))
        b = o.receipt_text(random.Random(7))
        self.assertEqual(a, b)
        self.assertNotEqual(a, o.receipt_text(random.Random(8)))
        self.assertTrue(all(len(line) <= o.WIDTH_CHARS for line in a.split("\n")))
        self.assertIn("TOTAL", a)
        self.assertGreaterEqual(len(o.AMOUNT_RE.findall(a)), 6)

    def test_total_is_the_sum_of_the_items(self):
        text = o.receipt_text(random.Random(3))
        lines = text.split("\n")
        rules = [i for i, line in enumerate(lines) if line.startswith("-")]
        item_lines = lines[rules[1] + 1:rules[2]]
        cents = sum(int(line.split()[-1].replace(".", "")) for line in item_lines)
        total_line = next(ln for ln in lines if ln.startswith("TOTAL"))
        self.assertEqual(cents, int(total_line.split()[-1].replace(".", "")))

    def test_synth_writes_pairs_when_pillow_is_present(self):
        try:
            import PIL  # noqa: F401
        except ImportError:
            self.skipTest("Pillow not installed")
        font = next((f for f in o.FONT_CANDIDATES if Path(f).exists()), None)
        if font is None:
            self.skipTest("no monospace font on this machine")
        with tempfile.TemporaryDirectory() as d:
            buf = io.StringIO()
            with contextlib.redirect_stdout(buf):
                code = o.main(["synth", "--out", d, "--count", "2", "--font", font])
            self.assertEqual(code, 0)
            for sub in ("clean", "blurred"):
                self.assertEqual(len(list((Path(d) / sub).glob("*.png"))), 2)
            self.assertEqual(len(list((Path(d) / "clean").glob("*.txt"))), 2)
            manifest = json.loads((Path(d) / "manifest.json").read_text())
            self.assertEqual(manifest["count"], 2)


class Check(unittest.TestCase):
    def report(self, d: Path, name: str, cer_mean: float, tess: str = "5.5.0") -> Path:
        p = d / f"{name}.json"
        p.write_text(json.dumps({
            "tesseract": {"version": tess},
            "settings": {"thresholding_method": 0},
            "aggregate": {"cer_mean": cer_mean, "amounts_accuracy": 1.0, "numerics_accuracy": 1.0},
        }))
        return p

    def run_check(self, clean: float, blurred: float, tess_b: str = "5.5.0") -> int:
        with tempfile.TemporaryDirectory() as d, contextlib.redirect_stdout(io.StringIO()), \
                contextlib.redirect_stderr(io.StringIO()):
            dd = Path(d)
            c = self.report(dd, "c", clean)
            b = self.report(dd, "b", blurred, tess_b)
            return o.main(["check", "--clean", str(c), "--blurred", str(b)])

    def test_pass_when_clean_is_good_and_blur_is_worse(self):
        self.assertEqual(self.run_check(0.02, 0.30), 0)

    def test_fail_when_clean_misses_the_bar(self):
        self.assertEqual(self.run_check(0.06, 0.30), 1)

    def test_fail_when_blur_is_not_worse(self):
        self.assertEqual(self.run_check(0.02, 0.02), 1)

    def test_mismatched_setups_are_refused(self):
        self.assertEqual(self.run_check(0.02, 0.30, tess_b="5.4.1"), 2)


if __name__ == "__main__":
    unittest.main()
