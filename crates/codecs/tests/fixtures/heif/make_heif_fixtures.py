# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Writes the committed AVIF test fixtures of `auto-crop-codecs` (feature `heif`).

Every image is procedural (gradients, four coloured corner squares, a diagonal), so nothing in it
is copied from anywhere. The files are encoded by Pillow's AVIF plugin (Pillow 12.3.0, MIT-CMU;
libavif and aom inside the wheel are BSD-2-Clause); the encoder is a tool used to *make* the files
and is not part of Auto Crop. ImageMagick (`magick`, the `ImageMagick` licence) writes the 10-bit
file because Pillow's plugin writes 8-bit only.

    cargo xtask synth-setup                       # creates target/synth-venv with Pillow
    target/synth-venv/Scripts/python crates/codecs/tests/fixtures/heif/make_heif_fixtures.py

Run it from the repository root. The outputs are listed in docs/provenance.md.
"""

import io
import shutil
import struct
import subprocess
import sys
from pathlib import Path

from PIL import Image, ImageCms, ImageDraw

OUT = Path(__file__).resolve().parent


def scene(w, h, mode="RGB"):
    """A gradient with four distinct corner squares and a diagonal: every flip or turn shows."""
    im = Image.new("RGB", (w, h))
    px = im.load()
    for y in range(h):
        for x in range(w):
            px[x, y] = (40 + x * 150 // max(w - 1, 1), 40 + y * 150 // max(h - 1, 1), 110)
    d = ImageDraw.Draw(im)
    s = max(min(w, h) // 5, 3)
    d.rectangle([0, 0, s, s], fill=(235, 30, 30))  # top left: red
    d.rectangle([w - 1 - s, 0, w - 1, s], fill=(30, 220, 40))  # top right: green
    d.rectangle([0, h - 1 - s, s, h - 1], fill=(40, 60, 235))  # bottom left: blue
    d.rectangle([w - 1 - s, h - 1 - s, w - 1, h - 1], fill=(240, 230, 40))  # bottom right: yellow
    d.line([s + 2, s + 2, w - s - 3, h - s - 3], fill=(250, 250, 250), width=1)
    return im.convert(mode) if mode != "RGB" else im


def exif_with(tag, value):
    """A tiny little-endian TIFF/Exif blob with one SHORT entry."""
    return b"II*\x00" + struct.pack("<IHHHII", 8, 1, tag, 3, 1, value) + b"\x00\x00\x00\x00"


def save(im, name, **kw):
    path = OUT / name
    im.save(path, format="AVIF", **kw)
    print(f"{name}: {path.stat().st_size} bytes")


def main():
    base = scene(48, 32)
    # Orientation: Pillow turns the Exif Orientation tag into irot/imir and drops it from the Exif.
    save(base, "gradient-444-48x32.avif", quality=90, subsampling="4:4:4", speed=6)
    for o in range(2, 9):
        save(
            base,
            f"orient{o}-444-48x32.avif",
            quality=90,
            subsampling="4:4:4",
            speed=6,
            exif=exif_with(0x0112, o),
        )
    save(scene(64, 48), "gradient-420-64x48.avif", quality=80, subsampling="4:2:0", speed=6)
    save(scene(32, 24, "L"), "gray-32x24.avif", quality=85, speed=6)

    rgba = scene(32, 24).convert("RGBA")
    a = Image.new("L", rgba.size, 255)
    ImageDraw.Draw(a).rectangle([0, 0, rgba.width // 2, rgba.height // 2], fill=0)
    rgba.putalpha(a)
    save(rgba, "alpha-32x24.avif", quality=90, subsampling="4:4:4", speed=6)

    srgb = ImageCms.ImageCmsProfile(ImageCms.createProfile("sRGB")).tobytes()
    save(scene(32, 24), "icc-srgb-32x24.avif", quality=85, speed=6, icc_profile=srgb)
    (OUT / "icc-srgb.icc").write_bytes(srgb)  # the exact bytes the file must return

    frames = [scene(32, 24), scene(32, 24).transpose(Image.Transpose.FLIP_LEFT_RIGHT)]
    frames[0].save(
        OUT / "seq-2frames-32x24.avif",
        format="AVIF",
        save_all=True,
        append_images=frames[1:],
        duration=100,
        quality=85,
        speed=6,
    )
    print(f"seq-2frames-32x24.avif: {(OUT / 'seq-2frames-32x24.avif').stat().st_size} bytes")

    # An Exif Orientation of 6 and *no* irot: the pixels must stay as stored (HEIF ignores the tag).
    # Pillow would turn a real Orientation tag into irot, so the entry is written under an unused
    # tag number and renamed in the finished file.
    fake = exif_with(0x9999, 6)
    save(base, "exif-orient6-noirot-48x32.avif", quality=90, subsampling="4:4:4", speed=6, exif=fake)
    p = OUT / "exif-orient6-noirot-48x32.avif"
    data = bytearray(p.read_bytes())
    entry = struct.pack("<HHII", 0x9999, 3, 1, 6)
    i = bytes(data).find(entry)
    assert i >= 0, "the Exif entry was not found in the AVIF"
    data[i : i + 2] = struct.pack("<H", 0x0112)
    p.write_bytes(bytes(data))

    magick = shutil.which("magick")
    if magick:
        png = io.BytesIO()
        scene(32, 24).save(png, format="PNG")
        subprocess.run(
            [magick, "png:-", "-depth", "10", "-quality", "85", str(OUT / "depth10-32x24.avif")],
            input=png.getvalue(),
            check=True,
        )
        print(f"depth10-32x24.avif: {(OUT / 'depth10-32x24.avif').stat().st_size} bytes")
    else:
        print("magick not found: depth10-32x24.avif was not rewritten", file=sys.stderr)


if __name__ == "__main__":
    main()
