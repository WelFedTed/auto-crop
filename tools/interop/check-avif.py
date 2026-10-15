#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Reads the AVIF files of `write_samples` (feature `avif`) with decoders that are not ours.

    tools/interop/check-avif.py <samples-dir> [--require-all]

Readers: Pillow (its wheel bundles libavif with dav1d), `avifdec` (libavif tools), ImageMagick
(libheif, when its build has an AVIF delegate). Checks per file: the size, the pixel format, the PSNR
against the picture the encoder was given (the bar below), the ICC profile and the EXIF bytes
byte-exact, and that no rotation is applied. A missing optional tool is skipped with a message unless
`--require-all` (the CI Linux job), which makes it a failure. Exit 1 on any mismatch.

PSNR bars (documented in docs/dev/output-formats.md): quality 80 on the synthetic photo, 4:4:4 at
least 38 dB, 4:2:0 at least 36 dB (the chroma halving costs the saturated rule colours of the
pattern), grey at least 40 dB.
"""
import math
import os
import shutil
import subprocess
import sys
import tempfile

from PIL import Image, ImageChops

D = sys.argv[1]
REQUIRE = "--require-all" in sys.argv[2:]
fails = 0
checks = 0

BARS = {"avif_rgb444.avif": 38.0, "avif_rgb420.avif": 36.0, "avif_gray.avif": 40.0, "avif_meta.avif": 36.0}
REFS = {
    "avif_rgb444.avif": "ref_photo.png",
    "avif_rgb420.avif": "ref_photo.png",
    "avif_gray.avif": "ref_photo_gray.png",
    "avif_meta.avif": "ref_photo.png",
}


def ok(msg):
    global checks
    checks += 1
    print("ok  ", msg)


def bad(msg):
    global checks, fails
    checks += 1
    fails += 1
    print("FAIL", msg)


def psnr(a, b):
    """PSNR in dB of two RGB images of the same size (infinite when equal)."""
    diff = ImageChops.difference(a.convert("RGB"), b.convert("RGB"))
    hist = diff.histogram()
    sse = 0
    for band in range(3):
        for v in range(256):
            sse += hist[band * 256 + v] * v * v
    n = a.size[0] * a.size[1] * 3
    if sse == 0:
        return math.inf
    return 10 * math.log10(255 * 255 / (sse / n))


def read(name):
    with open(os.path.join(D, name), "rb") as f:
        return f.read()


print("Pillow", Image.__version__)
for name, bar in BARS.items():
    ref = Image.open(os.path.join(D, REFS[name])).convert("RGB")
    im = Image.open(os.path.join(D, name))
    if im.format != "AVIF":
        bad(f"Pillow does not see {name} as AVIF ({im.format})")
        continue
    im.load()
    if im.size == ref.size:
        ok(f"Pillow reads {name} at {im.size[0]}x{im.size[1]}")
    else:
        bad(f"Pillow size of {name}: {im.size} vs {ref.size}")
        continue
    p = psnr(im, ref)
    (ok if p >= bar else bad)(f"Pillow {name}: PSNR {p:.2f} dB (bar {bar})")
    ex = im.getexif()
    if ex.get(0x0112, 1) != 1:
        bad(f"{name} still carries an orientation")

# The profile and the EXIF come back byte for byte.
im = Image.open(os.path.join(D, "avif_meta.avif"))
im.load()
(ok if im.info.get("icc_profile") == read("avif_icc.bin") else bad)("Pillow: the ICC profile is byte-exact")
exif = im.info.get("exif", b"")
want = read("avif_exif.bin")
# Pillow hands the Exif item with or without its `Exif\0\0` prefix, depending on the build.
if exif.startswith(b"Exif\0\0"):
    exif = exif[6:]
(ok if exif == want else bad)(f"Pillow: the EXIF blob is byte-exact ({len(exif)} of {len(want)} bytes)")
(ok if im.getexif().get(0x0112, 1) == 1 else bad)("Pillow: Exif Orientation is 1")

# avifdec (libavif's own tool).
avifdec = shutil.which("avifdec")
if avifdec:
    print(subprocess.run([avifdec, "--version"], capture_output=True, text=True).stdout.strip().splitlines()[0])
    for name, bar in BARS.items():
        out = os.path.join(tempfile.mkdtemp(), "out.png")
        r = subprocess.run([avifdec, os.path.join(D, name), out], capture_output=True, text=True)
        if r.returncode != 0:
            bad(f"avifdec failed on {name}: {r.stderr.strip()[:200]}")
            continue
        p = psnr(Image.open(out), Image.open(os.path.join(D, REFS[name])))
        (ok if p >= bar else bad)(f"avifdec {name}: PSNR {p:.2f} dB (bar {bar})")
    info = subprocess.run([avifdec, "--info", os.path.join(D, "avif_meta.avif")], capture_output=True, text=True)
    text = info.stdout + info.stderr
    (ok if "ICC" in text.upper() or "icc" in text else bad)("avifdec --info sees the ICC profile")
    (ok if "xif" in text else bad)("avifdec --info sees the Exif")
    if "irot" in text.lower() or "rotation" in text.lower() and "0" not in text:
        bad("avifdec reports a rotation")
elif REQUIRE:
    bad("missing tool: avifdec")
else:
    print("SKIP missing tool: avifdec")

# ImageMagick, only where its build can read AVIF (libheif with an AV1 decoder).
magick = shutil.which("magick") or shutil.which("convert")
if magick:
    r = subprocess.run([magick, os.path.join(D, "avif_rgb444.avif"), os.path.join(tempfile.mkdtemp(), "o.png")], capture_output=True, text=True)
    if r.returncode == 0:
        out = os.path.join(tempfile.mkdtemp(), "o.png")
        subprocess.run([magick, os.path.join(D, "avif_rgb444.avif"), out], check=True)
        p = psnr(Image.open(out), Image.open(os.path.join(D, "ref_photo.png")))
        (ok if p >= 38 else bad)(f"ImageMagick avif_rgb444.avif: PSNR {p:.2f} dB (bar 38)")
    else:
        print("SKIP ImageMagick has no AVIF delegate in this build")

print("---")
print(f"{checks} checks, {fails} failed")
sys.exit(1 if fails else 0)
