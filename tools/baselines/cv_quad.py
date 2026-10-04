# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Classical OpenCV quad finder: the baseline every document-scanner tutorial builds (ROADMAP M1.29).

Written from scratch for this repository (nothing copied from tutorials or other projects). The
steps are the textbook ones: blur, Canny edges, closed by a small dilation, external-ish contours,
convex hull, `approxPolyDP` until four vertices remain, and the largest convex quadrilateral wins.
If the edge map yields none, the same search runs on an Otsu mask (and its inverse). No quad means
no answer (a counted failure). Image coordinates in, normalised quad out.

    python tools/baselines/cv_quad.py --manifest M/manifest.jsonl --out cv_quad.jsonl
    auto-crop-eval run --manifest M/manifest.jsonl --predictor jsonl:cv_quad.jsonl --out r.json

OpenCV (Apache-2.0, `opencv-python-headless`, hash-locked in requirements.lock) is a DEV
dependency of this script only; it is never linked into, bundled with or imported by the app.
`cv2.imread` applies the EXIF orientation, which is what the manifest's coordinates assume (Pillow
does the same for the files OpenCV cannot read).
"""
from __future__ import annotations

import sys

import cv2
import numpy as np

import common

cv2.utils.logging.setLogLevel(cv2.utils.logging.LOG_LEVEL_SILENT)  # TIFFs OpenCV cannot read fall back to Pillow quietly

WORK_EDGE = 800  # long edge of the working image, pixels
MIN_AREA_FRACTION = 0.05  # a page smaller than this share of the frame is not looked for
EPSILONS = (0.02, 0.03, 0.04, 0.05, 0.07, 0.01)  # approxPolyDP tolerance, share of the perimeter


def largest_convex_quad(mask: np.ndarray) -> np.ndarray | None:
    """The largest convex 4-gon among the contours of a binary mask, as a 4 x 2 float array."""
    h, w = mask.shape[:2]
    contours, _ = cv2.findContours(mask, cv2.RETR_LIST, cv2.CHAIN_APPROX_SIMPLE)
    contours = sorted(contours, key=cv2.contourArea, reverse=True)[:12]
    best, best_area = None, 0.0
    for c in contours:
        if cv2.contourArea(c) < MIN_AREA_FRACTION * w * h:
            break
        hull = cv2.convexHull(c)
        peri = cv2.arcLength(hull, True)
        for eps in EPSILONS:
            approx = cv2.approxPolyDP(hull, eps * peri, True)
            if len(approx) == 4 and cv2.isContourConvex(approx):
                quad = approx.reshape(4, 2).astype(np.float64)
                x, y = quad[:, 0], quad[:, 1]
                if x.max() - x.min() >= 0.99 * w and y.max() - y.min() >= 0.99 * h:
                    break  # the frame itself, not a page
                area = cv2.contourArea(quad.astype(np.float32))
                if area > best_area:
                    best, best_area = quad, area
                break
    return best


def find_quad(bgr: np.ndarray) -> tuple[np.ndarray | None, float]:
    """(quad in the coordinates of `bgr`, or None; scale of the working image)."""
    h, w = bgr.shape[:2]
    scale = min(1.0, WORK_EDGE / max(h, w))
    small = cv2.resize(bgr, None, fx=scale, fy=scale, interpolation=cv2.INTER_AREA) if scale < 1.0 else bgr
    gray = cv2.cvtColor(small, cv2.COLOR_BGR2GRAY)
    blur = cv2.GaussianBlur(gray, (5, 5), 0)
    med = float(np.median(blur))
    edges = cv2.Canny(blur, int(max(0.0, 0.66 * med)), int(min(255.0, 1.33 * med)))
    edges = cv2.dilate(edges, np.ones((3, 3), np.uint8), iterations=1)
    quad = largest_convex_quad(edges)
    if quad is None:
        _, otsu = cv2.threshold(blur, 0, 255, cv2.THRESH_BINARY + cv2.THRESH_OTSU)
        candidates = [q for q in (largest_convex_quad(otsu), largest_convex_quad(255 - otsu)) if q is not None]
        quad = max(candidates, key=lambda q: cv2.contourArea(q.astype(np.float32)), default=None)
    return quad, scale


def load_bgr(path: str) -> np.ndarray:
    """The image as 8-bit BGR with the EXIF orientation applied. OpenCV reads it directly; where
    its bundled TIFF reader refuses a file (some of the suite's TIFFs), Pillow is the fallback."""
    bgr = cv2.imread(path, cv2.IMREAD_COLOR)  # applies the EXIF orientation
    if bgr is not None:
        return bgr
    from PIL import Image, ImageOps

    with Image.open(path) as im:
        rgb = np.asarray(ImageOps.exif_transpose(im).convert("RGB"))
    return cv2.cvtColor(rgb, cv2.COLOR_RGB2BGR)


def predict(path: str) -> dict | None:
    bgr = load_bgr(path)
    h, w = bgr.shape[:2]
    quad, scale = find_quad(bgr)
    if quad is None:
        return None
    pts = common.order_clockwise([(float(x) / scale, float(y) / scale) for x, y in quad])
    return {"quad": common.normalise(pts, w, h)}


if __name__ == "__main__":
    sys.exit(common.run("cv_quad (OpenCV classic quad finder)", predict))
