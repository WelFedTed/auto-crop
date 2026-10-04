# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""unpaper (mask detection and deskew) as a page finder (ROADMAP M1.29).

unpaper is a post-processor for scanned book pages. Two of its steps are geometric: it detects the
sheet against the scanner background (`mask-scan`) and rotates it upright (`deskew`). This adapter
runs unpaper with the pixel filters switched off, reads the detected mask rectangle and the
rotation from its verbose log, and reports the mask rectangle rotated by the detected skew as the
predicted page quad.

LICENCE BOUNDARY: unpaper is GPL-2.0-or-later. It is run strictly as an external process on files
(never linked, imported, vendored or bundled), only in Linux CI and the devcontainer; this adapter
is a dev tool and is not built on Windows. Nothing of unpaper's source or output format is copied
here; only its documented command-line options and its log are read.

What it is, honestly: unpaper expects flat scans of a sheet on a uniform dark or light scanner
background. It was never meant for camera photos with perspective, clutter and shadows, so low
numbers on the synthetic photo suites measure the mismatch, not a defect.

    python tools/baselines/unpaper_deskew.py --manifest M/manifest.jsonl --out unpaper.jsonl
    python tools/baselines/unpaper_deskew.py --log image.png     # print the raw unpaper log

Needs `unpaper` (6.x reads PNM only) and ImageMagick for the PNM conversion on PATH.
"""
from __future__ import annotations

import math
import os
import re
import shutil
import subprocess
import sys
import tempfile

import common
import im_deskew

# unpaper's geometric steps stay on; every step that only cleans pixels is off.
UNPAPER_ARGS = [
    "-vv",
    "--no-blackfilter",
    "--no-noisefilter",
    "--no-blurfilter",
    "--no-grayfilter",
    "--no-mask-center",
    "--no-wipe",
]
# The sign of the rotation unpaper reports relative to a clockwise-on-screen skew of the page.
# Pinned by tests/test_baselines.py on a page of known rotation (run where unpaper is installed).
ROTATION_SIGN = 1.0

MASK_RE = re.compile(r"auto-masking[^:]*:\s*\[?\s*(-?\d+)\s*,\s*(-?\d+)\s*,\s*(-?\d+)\s*,\s*(-?\d+)")
ROTATE_RE = re.compile(r"(?:rotat\w*|deskew\w*)[^\n]*?\(?\s*(-?\d+(?:\.\d+)?(?:[eE][-+]?\d+)?)\s*\)?\s*(?:degrees|\n|$)", re.I)


def ppm_size(path: str) -> tuple[int, int]:
    with open(path, "rb") as f:
        head = f.read(64).split()
    return int(head[1]), int(head[2])


def run_unpaper(path: str) -> tuple[str, tuple[int, int]]:
    """(unpaper's log, size of the EXIF-oriented image)."""
    exe = shutil.which("unpaper")
    if exe is None:
        raise RuntimeError("unpaper not found on PATH")
    with tempfile.TemporaryDirectory() as tmp:
        pnm, out = os.path.join(tmp, "in.ppm"), os.path.join(tmp, "out.ppm")
        subprocess.run(im_deskew.magick_command() + [path, "-auto-orient", "+repage", pnm], check=True, timeout=120)
        size = ppm_size(pnm)
        res = subprocess.run([exe] + UNPAPER_ARGS + [pnm, out], capture_output=True, text=True, timeout=120, check=False)
        return res.stdout + res.stderr, size


def parse_log(log: str, size: tuple[int, int]) -> tuple[tuple[int, int, int, int], float]:
    """(mask left, top, right, bottom; rotation in degrees). Defaults: the whole frame, no rotation."""
    w, h = size
    masks = MASK_RE.findall(log)
    mask = tuple(int(v) for v in masks[0]) if masks else (0, 0, w, h)
    angles = ROTATE_RE.findall(log)
    angle = float(angles[-1]) if angles else 0.0
    return mask, angle  # type: ignore[return-value]


def quad_from(mask: tuple[int, int, int, int], angle_deg: float, size: tuple[int, int]) -> list[list[float]]:
    left, top, right, bottom = mask
    cx, cy = (left + right) / 2.0, (top + bottom) / 2.0
    a = math.radians(-ROTATION_SIGN * angle_deg)
    cos, sin = math.cos(a), math.sin(a)
    pts = []
    for px, py in ((left, top), (right, top), (right, bottom), (left, bottom)):
        dx, dy = px - cx, py - cy
        pts.append((cx + dx * cos - dy * sin, cy + dx * sin + dy * cos))
    return common.normalise(common.order_clockwise(pts), size[0], size[1])


def predict(path: str) -> dict | None:
    log, size = run_unpaper(path)
    mask, angle = parse_log(log, size)
    if mask[2] - mask[0] <= 0 or mask[3] - mask[1] <= 0:
        return None
    return {"quad": quad_from(mask, angle, size)}


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "--log":
        log_text, dims = run_unpaper(sys.argv[2])
        print(f"size {dims}\n{log_text}")
        print("parsed:", parse_log(log_text, dims))
        sys.exit(0)
    sys.exit(common.run("unpaper_deskew (unpaper mask-scan and deskew)", predict))
