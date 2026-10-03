# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Writes a 12 MP (4000 x 3000) photo-like AVIF for timing the decoder; never committed.

    python crates/codecs/tests/fixtures/heif/make_big_avif.py target/heif-12mp.avif [width height]

Needs Pillow with AVIF support (12.x wheels have it) and NumPy. The picture is procedural: smooth
colour fields, edges and fine noise, so the coded size and the decode work are in the range of a
phone photo (a few MB, all coding tools exercised). The same bytes are produced for the same
arguments (fixed seed), apart from encoder version differences.
"""

import sys

import numpy as np
from PIL import Image


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else "heif-12mp.avif"
    w = int(sys.argv[2]) if len(sys.argv) > 3 else 4000
    h = int(sys.argv[3]) if len(sys.argv) > 3 else 3000
    rng = np.random.default_rng(20261004)
    y, x = np.mgrid[0:h, 0:w].astype(np.float32)
    r = 128 + 90 * np.sin(x / 311.0) * np.cos(y / 197.0)
    g = 128 + 90 * np.sin((x + y) / 523.0)
    b = 128 + 90 * np.cos(x / 421.0 - y / 263.0)
    # Hard edges: a grid of rectangles and a few circles.
    edges = ((x // 250 + y // 250) % 2) * 35.0
    cx, cy = w * 0.62, h * 0.4
    disc = (((x - cx) ** 2 + (y - cy) ** 2) < (min(w, h) * 0.22) ** 2) * 60.0
    img = np.stack([r + edges, g - edges + disc, b + disc / 2], axis=-1)
    img += rng.normal(0, 6.0, img.shape).astype(np.float32)  # sensor-like noise
    im = Image.fromarray(np.clip(img, 0, 255).astype(np.uint8), "RGB")
    im.save(out, format="AVIF", quality=70, speed=8, subsampling="4:2:0")
    import os

    print(f"{out}: {w}x{h}, {os.path.getsize(out) / 1e6:.2f} MB")


if __name__ == "__main__":
    main()
