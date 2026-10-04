# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""ImageMagick `-deskew` + `-trim` as a page finder (ROADMAP M1.29).

ImageMagick has no page detector; the closest thing a user can do with it is
`magick in -auto-orient -deskew 40% -fuzz 15% -trim out`: rotate by the Radon-transform estimate of
the skew, then cut away everything the same colour (within the fuzz) as the corner pixel. This
adapter runs exactly those operations and reports the trimmed rectangle, mapped back through the
deskew rotation into the original image, as the predicted page quad. It is an external process
(Apache-2.0 licensed ImageMagick, never linked or bundled); dev tool only.

What it is, honestly: a deskew tool is not a page finder. On a plain background it recovers the
page rotation and a tight rectangle; on cluttered or textured backgrounds the "background colour"
assumption fails, the trim removes (almost) nothing and the answer is close to the full frame.
A perspective-tilted page is not a rotated rectangle either, so a good rotation estimate still
scores below 1.0. The numbers in docs/perf/baselines.md must be read with that in mind.

    python tools/baselines/im_deskew.py --manifest M/manifest.jsonl --out im_deskew.jsonl

ImageMagick 7 (`magick`) or 6 (`convert`, Ubuntu 22.04) is found on PATH. Never `convert` on
Windows (it is the file-system converter there).
"""
from __future__ import annotations

import math
import shutil
import subprocess
import sys

import common

DESKEW_THRESHOLD = "40%"  # the value ImageMagick's documentation recommends
TRIM_FUZZ = "15%"
# IM rotates by `deskew:angle` degrees, clockwise on screen (y down). The original is recovered by
# rotating the other way. Pinned by tests/test_baselines.py on a page of known rotation.
ROTATION_SIGN = 1.0


def magick_command() -> list[str]:
    for exe in ("magick", "convert") if sys.platform != "win32" else ("magick",):
        path = shutil.which(exe)
        if path:
            return [path]
    raise RuntimeError("ImageMagick not found on PATH (magick or convert)")


def run_magick(path: str) -> list[str]:
    cmd = magick_command() + [
        path,
        "-auto-orient",
        "+repage",
        "-format",
        "%w %h\n",
        "-write",
        "info:",
        "-deskew",
        DESKEW_THRESHOLD,
        "+repage",
        "-format",
        "%[deskew:angle] %w %h\n",
        "-write",
        "info:",
        "-fuzz",
        TRIM_FUZZ,
        "-trim",
        "-format",
        "%w %h %X %Y\n",
        "info:",
    ]
    out = subprocess.run(cmd, capture_output=True, text=True, timeout=120, check=False)
    lines = [ln for ln in out.stdout.splitlines() if ln.strip()]
    if out.returncode not in (0, 1) or len(lines) < 3:  # 1 = warnings only (an empty trim)
        raise RuntimeError(f"magick failed ({out.returncode}): {out.stderr.strip()[:200]}")
    return lines[-3:]


def to_original(
    box: tuple[float, float, float, float],
    angle_deg: float,
    original: tuple[int, int],
    deskewed: tuple[int, int],
) -> list[tuple[float, float]]:
    """Corners of `box` (x, y, w, h in the deskewed image) in the original image."""
    x, y, w, h = box
    a = math.radians(ROTATION_SIGN * angle_deg)
    cos, sin = math.cos(-a), math.sin(-a)
    c1 = (original[0] / 2.0, original[1] / 2.0)
    c2 = (deskewed[0] / 2.0, deskewed[1] / 2.0)
    out = []
    for px, py in ((x, y), (x + w, y), (x + w, y + h), (x, y + h)):
        dx, dy = px - c2[0], py - c2[1]
        out.append((c1[0] + dx * cos - dy * sin, c1[1] + dx * sin + dy * cos))
    return out


def predict(path: str) -> dict | None:
    first, second, third = run_magick(path)
    ow, oh = (int(v) for v in first.split())
    angle_s, dw_s, dh_s = second.split()
    tw, th, tx, ty = third.split()
    if int(tw) <= 0 or int(th) <= 0:
        return None
    corners = to_original(
        (float(tx), float(ty), float(tw), float(th)),
        float(angle_s),
        (ow, oh),
        (int(dw_s), int(dh_s)),
    )
    return {"quad": common.normalise(common.order_clockwise(corners), ow, oh)}


if __name__ == "__main__":
    sys.exit(common.run("im_deskew (ImageMagick -deskew -trim)", predict))
