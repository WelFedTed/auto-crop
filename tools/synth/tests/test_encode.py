# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Formats, EXIF orientation and colour spaces (ROADMAP M1.34)."""

import io
import sys
import unittest
from pathlib import Path

import numpy as np
from PIL import Image, ImageCms, ImageOps

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from synth import encode, icc  # noqa: E402


def picture(h=40, w=64):
    """An asymmetric picture, so a wrong flip or turn cannot go unnoticed."""
    rng = np.random.default_rng(3)
    img = rng.integers(0, 256, size=(h, w, 3), dtype=np.uint8)
    img[:5, :9] = (255, 0, 0)  # red top-left marker
    return img


class OrientationTests(unittest.TestCase):
    def test_stored_pixels_turn_back_into_the_upright_picture_for_all_eight(self):
        up = picture()
        for o in range(1, 9):
            stored = encode.stored_for(up, o)
            self.assertEqual(stored.shape[:2] if o <= 4 else stored.shape[:2][::-1], up.shape[:2], o)
            np.testing.assert_array_equal(encode.display(stored, o), up, f"orientation {o}")

    def test_it_agrees_with_pillows_own_exif_transpose(self):
        up = picture()
        for o in range(1, 9):
            data = encode.encode(up, "png", 0, o)
            im = Image.open(io.BytesIO(data))
            self.assertEqual(im.getexif().get(0x0112, 1), o)
            shown = np.asarray(ImageOps.exif_transpose(im).convert("RGB"))
            np.testing.assert_array_equal(shown, up, f"orientation {o}")


class FormatTests(unittest.TestCase):
    def test_lossless_formats_reproduce_the_upright_picture_for_every_variant(self):
        up = picture()
        for fmt in ("png", "tiff"):
            for o in range(1, 9):
                for cs in ("srgb", "display-p3"):
                    data = encode.encode(up, fmt, 0, o, cs)
                    im = Image.open(io.BytesIO(data))
                    ref = icc.srgb_to_p3(up) if cs == "display-p3" else up
                    shown = np.asarray(ImageOps.exif_transpose(im).convert("RGB"))
                    np.testing.assert_array_equal(shown, ref, f"{fmt} o{o} {cs}")

    def test_lossless_webp_is_exact_and_lossy_formats_are_close(self):
        up = picture()
        for o in (1, 6, 8):
            data = encode.encode(up, "webp", 100, o, lossless_webp=True)
            shown = np.asarray(ImageOps.exif_transpose(Image.open(io.BytesIO(data))).convert("RGB"))
            np.testing.assert_array_equal(shown, up)
        smooth = np.tile(np.linspace(0, 255, 64, dtype=np.uint8)[None, :, None], (40, 1, 3))
        for fmt in ("jpeg", "webp"):
            data = encode.encode(smooth, fmt, 90, 3)
            shown = np.asarray(ImageOps.exif_transpose(Image.open(io.BytesIO(data))).convert("RGB"))
            self.assertLess(np.abs(shown.astype(int) - smooth.astype(int)).mean(), 3.0, fmt)

    def test_the_orientation_tag_is_present_in_every_format_and_absent_for_1(self):
        up = picture()
        for fmt in encode.FORMATS:
            for o in (1, 5, 6):
                im = Image.open(io.BytesIO(encode.encode(up, fmt, 90, o)))
                self.assertEqual(im.getexif().get(0x0112, 1), o, f"{fmt} {o}")
        self.assertNotIn(0x0112, Image.open(io.BytesIO(encode.encode(up, "jpeg", 90, 1))).getexif())

    def test_formats_are_what_they_say(self):
        up = picture()
        for fmt, magic in [("jpeg", b"\xff\xd8"), ("png", b"\x89PNG"), ("webp", b"RIFF"), ("tiff", b"II*\x00")]:
            self.assertTrue(encode.encode(up, fmt, 80, 1).startswith(magic), fmt)


class TiffDeterminismTests(unittest.TestCase):
    """Pillow's libtiff leaves one alignment byte before the IFD uninitialised when a strip has an
    odd length; the suite's TIFFs therefore differed in a single byte from run to run."""

    def tiff(self, seed):
        # compressible, so the deflate strip length varies (random noise stores to a fixed, even size)
        up = (np.random.default_rng(seed).integers(0, 6, size=(37, 53, 3)) * 40).astype(np.uint8)
        return up, encode.encode(up, "tiff", 0, 1)

    def test_a_garbage_pad_byte_is_zeroed_and_pixels_are_untouched(self):
        padded = 0
        for seed in range(40):
            up, data = self.tiff(seed)
            pad = encode.tiff_pad_range(data)
            self.assertEqual(encode.zero_tiff_padding(data), data)  # encode() already did it
            if pad is None:
                continue
            padded += 1
            start, end = pad
            garbage = data[:start] + b"\xaa" * (end - start) + data[end:]  # what libtiff may leave
            self.assertNotEqual(garbage, data)
            fixed = encode.zero_tiff_padding(garbage)
            self.assertEqual(fixed, data)
            np.testing.assert_array_equal(np.asarray(Image.open(io.BytesIO(garbage)).convert("RGB")), up)
        self.assertGreater(padded, 3, "no odd-length strip among 40 images: the test tested nothing")

    def test_two_processes_encode_identical_bytes(self):
        import hashlib
        import subprocess
        import sys

        code = (
            "import sys, hashlib, numpy as np; sys.path.insert(0, %r);"
            "from synth import encode;"
            "print(''.join(hashlib.sha256(encode.encode((np.random.default_rng(s).integers(0,6,size=(41,57,3))*40).astype(np.uint8),'tiff',0,s%%8+1,'display-p3' if s%%3==0 else 'srgb')).hexdigest()[:8] for s in range(60)))"
        ) % str(Path(__file__).resolve().parent.parent)
        runs = [subprocess.run([sys.executable, "-c", code], capture_output=True, text=True).stdout for _ in range(3)]
        self.assertTrue(runs[0].strip())
        self.assertEqual(runs[0], runs[1])
        self.assertEqual(runs[1], runs[2])
        del hashlib


class ColourTests(unittest.TestCase):
    def test_the_p3_profile_is_a_valid_icc_profile_pillows_cms_accepts(self):
        data = icc.display_p3_profile()
        self.assertEqual(data[36:40], b"acsp")
        self.assertEqual(int.from_bytes(data[:4], "big"), len(data))
        prof = ImageCms.ImageCmsProfile(io.BytesIO(data))
        self.assertIn("Display P3", ImageCms.getProfileDescription(prof))
        self.assertEqual(data, icc.display_p3_profile())  # reproducible bytes

    def test_the_srgb_profile_is_valid_and_has_no_timestamp_so_bytes_are_reproducible(self):
        data = icc.srgb_profile()
        ImageCms.ImageCmsProfile(io.BytesIO(data))
        self.assertEqual(data[24:36], (2026).to_bytes(2, "big") + (10).to_bytes(2, "big") + (3).to_bytes(2, "big") + bytes(6))
        up = np.random.default_rng(1).integers(0, 256, size=(24, 32, 3), dtype=np.uint8)
        a = encode.encode(up, "png", 0, 1, embed_srgb=True)
        self.assertEqual(a, encode.encode(up, "png", 0, 1, embed_srgb=True))
        # an sRGB profile of our own makes no change to the colours LittleCMS computes
        srgb = ImageCms.ImageCmsProfile(io.BytesIO(data))
        ref = ImageCms.ImageCmsProfile(ImageCms.createProfile("sRGB"))
        out = ImageCms.profileToProfile(Image.fromarray(up), srgb, ref, renderingIntent=1, outputMode="RGB")
        self.assertLessEqual(np.abs(np.asarray(out).astype(int) - up.astype(int)).max(), 2)

    def test_the_embedded_profile_survives_in_every_format(self):
        up = picture()
        for fmt in encode.FORMATS:
            im = Image.open(io.BytesIO(encode.encode(up, fmt, 90, 1, "display-p3")))
            self.assertEqual(im.info.get("icc_profile"), icc.display_p3_profile(), fmt)

    def test_converting_p3_pixels_back_through_the_profile_gives_the_srgb_picture(self):
        up = np.random.default_rng(5).integers(0, 256, size=(32, 48, 3), dtype=np.uint8)
        p3 = icc.srgb_to_p3(up)
        src = ImageCms.ImageCmsProfile(io.BytesIO(icc.display_p3_profile()))
        dst = ImageCms.ImageCmsProfile(ImageCms.createProfile("sRGB"))
        back = ImageCms.profileToProfile(Image.fromarray(p3), src, dst, renderingIntent=1, outputMode="RGB")
        err = np.abs(np.asarray(back).astype(int) - up.astype(int))
        self.assertLessEqual(err.mean(), 1.5)
        self.assertLessEqual(err.max(), 6)

    def test_p3_is_a_wider_gamut_so_saturated_sRGB_reds_get_less_saturated_values(self):
        red = np.array([[[255, 0, 0]]], dtype=np.uint8)
        p3 = icc.srgb_to_p3(red)[0, 0]
        self.assertLess(p3[0], 255)
        self.assertGreater(p3[1], 0)

    def test_grey_stays_grey(self):
        grey = np.full((2, 2, 3), 128, dtype=np.uint8)
        np.testing.assert_array_equal(icc.srgb_to_p3(grey), grey)


if __name__ == "__main__":
    unittest.main()
