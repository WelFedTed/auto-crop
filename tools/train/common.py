# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Geometry shared by training, decoding and evaluation: letterboxing, quad ordering, IoU."""

from __future__ import annotations

import json
import math
from pathlib import Path

import cv2
import numpy as np

PAD_GREY = 114
IN_SIZE = 256


def read_manifest(path: str | Path) -> list[dict]:
    p = Path(path)
    rows = [json.loads(line) for line in p.read_text(encoding="utf8").splitlines() if line.strip()]
    for r in rows:
        r["_path"] = str(p.parent / r["image"])
    return rows


def read_image_rgb(path: str) -> np.ndarray:
    """RGB uint8 with the EXIF orientation applied. JPEG goes through OpenCV (fast; it applies the
    orientation on read); every other format through Pillow, whose orientation handling is explicit."""
    if path.lower().endswith((".jpg", ".jpeg")):
        im = cv2.imread(path, cv2.IMREAD_COLOR)
        if im is not None:
            return cv2.cvtColor(im, cv2.COLOR_BGR2RGB)
    from PIL import Image, ImageOps

    with Image.open(path) as f:
        return np.asarray(ImageOps.exif_transpose(f).convert("RGB"))


def letterbox_matrix(w: int, h: int, size: int = IN_SIZE, fill: float = 1.0) -> np.ndarray:
    """3x3 affine mapping source pixels to the ``size`` square: long edge scaled to ``fill * size``, centred."""
    s = fill * size / max(w, h)
    return np.array([[s, 0, (size - s * w) / 2], [0, s, (size - s * h) / 2], [0, 0, 1]], dtype=np.float64)


def warp_to_canvas(img: np.ndarray, m: np.ndarray, size: int = IN_SIZE) -> np.ndarray:
    """Warp ``img`` with the 3x3 ``m`` onto a ``size`` square padded with grey (area pre-shrink first)."""
    sx = math.hypot(m[0, 0], m[1, 0])
    if sx < 0.7:  # shrink with an area filter first: bilinear sampling alone would alias
        r = max(sx, 0.2)
        img = cv2.resize(img, None, fx=r, fy=r, interpolation=cv2.INTER_AREA)
        m = m @ np.diag([1 / r, 1 / r, 1.0])
    return cv2.warpPerspective(img, m, (size, size), flags=cv2.INTER_LINEAR, borderMode=cv2.BORDER_CONSTANT, borderValue=(PAD_GREY,) * 3)


def apply_h(m: np.ndarray, pts: np.ndarray) -> np.ndarray:
    p = np.c_[np.asarray(pts, dtype=np.float64), np.ones(len(pts))] @ m.T
    return p[:, :2] / p[:, 2:3]


def canonical_quad(q: np.ndarray) -> np.ndarray:
    """Clockwise (y down), starting at the corner nearest the up-left direction from the centroid."""
    q = np.asarray(q, dtype=np.float64)
    c = q.mean(axis=0)
    ang = np.arctan2(q[:, 1] - c[1], q[:, 0] - c[0])
    order = np.argsort(ang)  # increasing angle is clockwise on screen (y down)
    q = q[order]
    ang = ang[order]
    d = np.abs((ang + 3 * math.pi / 4 + math.pi) % (2 * math.pi) - math.pi)
    k = int(np.argmin(d))
    return np.roll(q, -k, axis=0)


def poly_area(q: np.ndarray) -> float:
    x, y = q[:, 0], q[:, 1]
    return 0.5 * float(np.dot(x, np.roll(y, -1)) - np.dot(y, np.roll(x, -1)))


def is_convex(q: np.ndarray) -> bool:
    sgn = []
    for i in range(4):
        a, b, c = q[i], q[(i + 1) % 4], q[(i + 2) % 4]
        sgn.append((b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]))
    return all(s > 0 for s in sgn) or all(s < 0 for s in sgn)


def _clip_to_unit_square(poly: np.ndarray) -> float:
    """Area of a convex polygon (positive orientation) clipped to the unit square (Sutherland-Hodgman)."""
    pts = [tuple(p) for p in poly]
    for axis, edge, keep_less in ((0, 0.0, False), (0, 1.0, True), (1, 0.0, False), (1, 1.0, True)):
        if not pts:
            return 0.0
        inside = (lambda p: p[axis] <= edge) if keep_less else (lambda p: p[axis] >= edge)
        out = []
        for i, cur in enumerate(pts):
            prev = pts[i - 1]
            if inside(cur) != inside(prev):
                t = (edge - prev[axis]) / (cur[axis] - prev[axis])
                out.append((prev[0] + t * (cur[0] - prev[0]), prev[1] + t * (cur[1] - prev[1])))
            if inside(cur):
                out.append(cur)
        pts = out
    return abs(poly_area(np.array(pts))) if len(pts) >= 3 else 0.0


def canonical_iou(gt_px: np.ndarray, pred_px: np.ndarray) -> float:
    """IoU after the canonical warp (the homography taking ``gt`` to the unit square), like the harness.

    Quads are 4x2 pixel arrays in any consistent order; the predicted quad must be convex.
    """
    gt = np.asarray(gt_px, dtype=np.float32)
    pr = np.asarray(pred_px, dtype=np.float64)
    if not np.isfinite(pr).all() or abs(poly_area(pr)) < 1e-9:
        return 0.0
    try:
        h = cv2.getPerspectiveTransform(gt, np.array([[0, 0], [1, 0], [1, 1], [0, 1]], dtype=np.float32))
    except cv2.error:
        return 0.0
    # A point beyond the horizon of the page plane cannot be scored.
    den = np.c_[pr, np.ones(4)] @ h[2]
    if (den <= 1e-9).any():
        return 0.0
    w = apply_h(h.astype(np.float64), pr)
    if not is_convex(w):
        return 0.0
    w = w if poly_area(w) > 0 else w[::-1]
    inter = _clip_to_unit_square(w)
    union = 1.0 + abs(poly_area(w)) - inter
    return float(inter / union) if union > 0 else 0.0


def clamp_quad_to_frame(q_norm) -> list[list[float]] | None:
    """Clip a normalised quad to the frame [0,1]^2 and reduce the result to four vertices.

    A page cut by the frame is answered as its visible part (what the owner's labels do and what the
    product does: PLAN 4.x partial frames). Extra vertices created by the clip are removed by
    repeatedly dropping the one whose triangle with its neighbours has the smallest area.
    """
    pts = [tuple(map(float, p)) for p in q_norm]
    if poly_area(np.array(pts)) < 0:
        pts = pts[::-1]
    for axis, edge, keep_less in ((0, 0.0, False), (0, 1.0, True), (1, 0.0, False), (1, 1.0, True)):
        inside = (lambda p: p[axis] <= edge) if keep_less else (lambda p: p[axis] >= edge)
        out = []
        for i, cur in enumerate(pts):
            prev = pts[i - 1]
            if inside(cur) != inside(prev):
                t = (edge - prev[axis]) / (cur[axis] - prev[axis])
                out.append((prev[0] + t * (cur[0] - prev[0]), prev[1] + t * (cur[1] - prev[1])))
            if inside(cur):
                out.append(cur)
        pts = out
        if len(pts) < 3:
            return None
    while len(pts) > 4:
        areas = []
        for i in range(len(pts)):
            a, b, c = pts[i - 1], pts[i], pts[(i + 1) % len(pts)]
            areas.append(abs((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])) / 2)
        pts.pop(int(np.argmin(areas)))
    if len(pts) < 4:
        return None
    return [list(p) for p in pts]
