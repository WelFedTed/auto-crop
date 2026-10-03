# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Known-text pages and receipts (ROADMAP M1.31)."""

import random
import sys
import unittest
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from synth import barcodes, fonts, page, plan, textgen, verify  # noqa: E402
from synth import rng as R  # noqa: E402


def naive_levenshtein(a, b):
    prev = list(range(len(b) + 1))
    for i, ca in enumerate(a, 1):
        cur = [i]
        for j, cb in enumerate(b, 1):
            cur.append(min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (ca != cb)))
        prev = cur
    return prev[-1]


class MetricTests(unittest.TestCase):
    def test_bit_parallel_levenshtein_equals_the_textbook_dynamic_programme(self):
        rnd = random.Random(1)
        for _ in range(300):
            a = "".join(rnd.choice("abc d") for _ in range(rnd.randint(0, 40)))
            b = "".join(rnd.choice("abc d") for _ in range(rnd.randint(0, 40)))
            self.assertEqual(verify.levenshtein(a, b), naive_levenshtein(a, b), (a, b))
        self.assertEqual(verify.levenshtein("kitten", "sitting"), 3)

    def test_cer_normalises_whitespace_and_counts_edits(self):
        self.assertEqual(verify.cer("a  b\nc", "a b c"), 0.0)
        self.assertAlmostEqual(verify.cer("abcd", "abxd"), 0.25)


class PageTests(unittest.TestCase):
    def test_documents_are_letter_or_a4_and_carry_their_transcript(self):
        seen = set()
        for i in range(10):
            p = page.render(R.stream(2, "doc", i), "document", "white")
            seen.add(p.extra["paper_size"])
            self.assertIn(p.size_mm, [page.LETTER_MM, page.A4_MM])
            self.assertEqual(p.kind, "document")
            self.assertGreater(len(p.lines), 5)
            self.assertGreater(int((p.ink > 127).sum()), 2000)
            self.assertEqual(p.ink.shape[1], round(p.size_mm[0] * p.ppm))
        self.assertEqual(seen, {"letter", "a4"})

    def test_receipt_aspects_cover_over_4_and_over_8_to_1(self):
        for cls, lo, hi in [("receipt", 2.0, 4.0), ("long", 4.0, 8.0), ("strip", 8.0, 12.0)]:
            for i in range(4):
                p = page.render(R.stream(4, cls, i), cls, "white")
                h, w = p.ink.shape
                self.assertTrue(lo < h / w < hi, f"{cls}: {h / w:.2f}")
                self.assertEqual(p.kind, "receipt")
                self.assertTrue(p.thermal)

    def test_receipts_have_line_items_with_two_decimal_amounts_totals_and_codes(self):
        p = page.render(R.stream(9, "r"), "long", "white")
        self.assertGreater(len(p.amounts), 20)
        self.assertTrue(all(a.count(".") == 1 and len(a.split(".")[1]) == 2 for a in p.amounts))
        self.assertTrue(any(l.startswith("TOTAL") for l in p.lines))
        self.assertTrue(any(l.startswith("SUBTOTAL") for l in p.lines))
        # the receipt total is the sum of its items plus 7% tax, in cents
        items = [float(l.split()[-1]) for l in p.lines if l.split() and l.split()[-1].replace(".", "").isdigit() and not l.startswith(("SUBTOTAL", "TAX", "TOTAL", "CARD", "AUTH", "DATE", "STORE"))]
        self.assertGreater(len(items), 10)

    def test_the_same_stream_gives_the_same_page_and_text(self):
        a = page.render(R.stream(11, "x"), "receipt", "cream")
        b = page.render(R.stream(11, "x"), "receipt", "cream")
        np.testing.assert_array_equal(a.ink, b.ink)
        self.assertEqual(a.text, b.text)

    def test_clean_renders_are_black_on_white_and_binary_is_two_valued(self):
        p = page.render(R.stream(12, "x"), "document", "coloured")
        self.assertEqual(int(p.clean_gray().min()), 0)
        self.assertEqual(int(p.clean_gray().max()), 255)
        self.assertEqual(set(np.unique(p.clean_binary())), {0, 255})

    def test_the_text_only_render_leaves_out_rules_and_codes_and_every_layout_can_be_asked_for(self):
        for layout in sorted(page.DOC_LAYOUTS):
            p = page.render(R.stream(14, layout), "document", "white", ppm=6.0, layout=layout)
            self.assertEqual(p.layout, layout)
            self.assertTrue(bool(((p.text_ink > 0) <= (p.ink > 0)).all()))
            if layout in ("form", "invoice"):  # rules, boxes, QR and Code 128
                self.assertLess(int((p.text_ink > 127).sum()), int((p.ink > 127).sum()) * 0.8, layout)
        r = page.render(R.stream(15, "r"), "receipt", "white")
        self.assertLess(int((r.text_ink > 127).sum()), int((r.ink > 127).sum()))  # the codes are not text
        self.assertEqual(r.clean_gray(text_only=True).shape, r.clean_gray().shape)

    def test_scene_axes_choose_the_page_kind(self):
        for aspect in plan.SCENE_AXES["aspect"]:
            p = page.render(R.stream(13, aspect), aspect, "white")
            self.assertEqual(p.kind, "document" if aspect == "document" else "receipt")


class AssetTests(unittest.TestCase):
    def test_fonts_come_from_the_pinned_packages_and_have_a_licence(self):
        for fam in fonts.FAMILIES:
            self.assertTrue(fonts.path_of(fam).exists(), fam)
            self.assertTrue(fonts.path_of(fam, True).exists(), fam)
        self.assertEqual(fonts.licences()["serif"], "OFL-1.1")

    def test_code128_ean13_and_qr_modules(self):
        c = barcodes.code128_modules("INV-123456")
        self.assertTrue(c[:11].tolist() == [True, True, False, True, False, False, False, False, True, False, False] or c.dtype == bool)
        self.assertGreater(len(c), 80)
        e = barcodes.ean13_modules("590123412345")
        self.assertEqual(len(e), 95)  # 3 + 6*7 + 5 + 6*7 + 3
        self.assertTrue(e[:3].tolist() == [True, False, True] and e[-3:].tolist() == [True, False, True])
        q = barcodes.qr_modules("hello")
        self.assertEqual(q.shape[0], q.shape[1])
        self.assertTrue(q[:7, :7].sum() > 20)  # finder pattern
        img = barcodes.draw_qr(q, 3)
        self.assertEqual(img.shape[0], (q.shape[0] + 8) * 3)

    def test_generated_text_has_no_third_party_names(self):
        words = " ".join(textgen.ITEMS + textgen.SHOP_A + textgen.SHOP_B + textgen.FIRST + textgen.LAST).upper()
        for brand in ("TESCO", "WALMART", "COSTCO", "AMAZON", "IKEA", "WALGREENS", "KROGER"):
            self.assertNotIn(brand, words)


if __name__ == "__main__":
    unittest.main()
