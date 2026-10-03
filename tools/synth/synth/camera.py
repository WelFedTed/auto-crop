# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Pinhole camera, perspective warp and analytic ground truth (ROADMAP M1.32).

A page is a rectangle of ``Wt x Ht`` texture pixels, centred on the optical axis at distance ``d``,
turned by pitch and yaw (up to 45 degrees each) and rolled about the axis (any angle). With the
focal length folded into the scale ``s`` and the principal point ``(cx, cy)`` the whole camera is
one 3x3 matrix in float64,

    H = A . [r1 r2 t] . T(-Wt/2, -Ht/2),     A = [[s, 0, cx], [0, s, cy], [0, 0, 1]],

taking a point of the page, in texture *edge* coordinates (the page outline is exactly
``0..Wt x 0..Ht``), to the image, in edge coordinates too (the image outline is ``0..w x 0..h``).
The ground-truth quad is those four corners through ``H``, divided by the image size: it is
analytic, not measured from the render, and it may lie outside the frame (partial framing). A
curled page lifts points along the page normal by ``lift(x, y)``; the ground truth is then the four
displaced corners seen by the same camera, and the picture is rendered by inverting that map with
Newton iterations.

This module imports nothing from the application (ROADMAP M1.32; `tests/test_independence.py`).
"""

from __future__ import annotations

import math
from dataclasses import dataclass

import cv2
import numpy as np

SS = 2  # supersampling of the render before the area-average down to the image size
CANVAS_ASPECTS = [(9, 16), (1, 2), (2, 3), (3, 4), (1, 1), (4, 3), (3, 2), (16, 9), (2, 1)]


def rotation(pitch_deg: float, yaw_deg: float, roll_deg: float) -> np.ndarray:
    """R = Rz(roll) . Rx(pitch) . Ry(yaw) in float64 (camera frame: x right, y down, z forward)."""
    p, y, r = (math.radians(a) for a in (pitch_deg, yaw_deg, roll_deg))
    cp, sp, cy, sy, cr, sr = math.cos(p), math.sin(p), math.cos(y), math.sin(y), math.cos(r), math.sin(r)
    rx = np.array([[1, 0, 0], [0, cp, -sp], [0, sp, cp]], dtype=np.float64)
    ry = np.array([[cy, 0, sy], [0, 1, 0], [-sy, 0, cy]], dtype=np.float64)
    rz = np.array([[cr, -sr, 0], [sr, cr, 0], [0, 0, 1]], dtype=np.float64)
    return rz @ rx @ ry


@dataclass(frozen=True)
class Curl:
    """A smooth lift of the page towards the camera, ``lift(x, y)`` in texture pixels."""

    kind: str  # cylinder | edge | corner
    amp: float
    angle: float  # direction of the curl axis in radians
    onset: float  # 0..1 position where the lift starts (edge, corner)

    def lift(self, x: np.ndarray, y: np.ndarray, w: float, h: float) -> np.ndarray:
        c, s = math.cos(self.angle), math.sin(self.angle)
        u = (x * c + y * s) / (0.5 * (abs(w * c) + abs(h * s)))  # -1..1 along the curl direction
        if self.kind == "cylinder":
            return self.amp * (1.0 - np.clip(u, -1, 1) ** 2)
        t = np.clip((u - (2 * self.onset - 1)) / (2 - (2 * self.onset - 1)), 0.0, 1.0)
        return self.amp * t**2


@dataclass(frozen=True)
class Camera:
    h_tex: np.ndarray  # 3x3 float64, texture edge coords -> canvas edge coords
    canvas: tuple[int, int]
    tex: tuple[int, int]
    rot: np.ndarray  # 3x3
    d: float
    s: float
    c: tuple[float, float]
    pitch: float
    yaw: float
    roll: float
    curl: Curl | None

    def quad_px(self) -> np.ndarray:
        """Ground-truth corners TL, TR, BR, BL in canvas pixels (float64, may be off-frame)."""
        wt, ht = self.tex
        pts = np.array([[0, 0], [wt, 0], [wt, ht], [0, ht]], dtype=np.float64)
        if self.curl is None:
            v = np.c_[pts, np.ones(4)] @ self.h_tex.T
            return v[:, :2] / v[:, 2:3]
        x, y = pts[:, 0] - wt / 2, pts[:, 1] - ht / 2
        u, v = self.project(x, y)
        return np.c_[u, v]

    def quad_norm(self) -> np.ndarray:
        q = self.quad_px()
        return q / np.array([self.canvas[0], self.canvas[1]], dtype=np.float64)

    def project(self, x, y):
        """Centred page coordinates (texture px) to canvas edge coordinates, with curl."""
        wt, ht = self.tex
        z = -self.curl.lift(x, y, wt, ht) if self.curl else 0.0 * x
        r = self.rot
        px = r[0, 0] * x + r[0, 1] * y + r[0, 2] * z
        py = r[1, 0] * x + r[1, 1] * y + r[1, 2] * z
        pz = r[2, 0] * x + r[2, 1] * y + r[2, 2] * z + self.d
        return self.s * px / pz + self.c[0], self.s * py / pz + self.c[1]


def _pick_canvas(aspect: float, max_edge: int, rng) -> tuple[int, int]:
    """A common photo shape close to ``aspect`` (width / height), with a long edge ``max_edge``."""
    best = sorted(CANVAS_ASPECTS, key=lambda a: abs(math.log(a[0] / a[1] / aspect)))
    k = 0 if rng.random() < 0.75 else min(1, len(best) - 1)
    a, b = best[k]
    if a >= b:
        return max_edge, max(8, round(max_edge * b / a))
    return max(8, round(max_edge * a / b)), max_edge


def _shoelace(q: np.ndarray) -> float:
    x, y = q[:, 0], q[:, 1]
    return 0.5 * float(np.dot(x, np.roll(y, -1)) - np.dot(y, np.roll(x, -1)))


def visible_fraction(q: np.ndarray, canvas: tuple[int, int]) -> float:
    """Share of the quad's area inside the frame (convex clip of the quad against the frame)."""
    poly = [tuple(p) for p in q]
    for edge in range(4):
        out = []
        for i, a in enumerate(poly):
            b = poly[(i + 1) % len(poly)]

            def inside(p, e=edge):
                return [p[0] >= 0, p[0] <= canvas[0], p[1] >= 0, p[1] <= canvas[1]][e]

            def cut(p, q2, e=edge):
                lim = [0, canvas[0], 0, canvas[1]][e]
                ax = 0 if e < 2 else 1
                t = (lim - p[ax]) / (q2[ax] - p[ax])
                return (p[0] + t * (q2[0] - p[0]), p[1] + t * (q2[1] - p[1]))

            if inside(a):
                out.append(a)
                if not inside(b):
                    out.append(cut(a, b))
            elif inside(b):
                out.append(cut(a, b))
        poly = out
        if not poly:
            return 0.0
    area = abs(_shoelace(np.array(poly))) if len(poly) >= 3 else 0.0
    return area / abs(_shoelace(q))


def fit(
    tex: tuple[int, int],
    pitch: float,
    yaw: float,
    roll: float,
    rng,
    max_edge: int,
    partial: bool,
    curl: Curl | None = None,
) -> Camera:
    """A camera that frames the page: full (all corners inside) or partial (corners cut off)."""
    wt, ht = tex
    big = float(max(wt, ht))
    d = big * float(rng.uniform(1.9, 3.4))
    r = rotation(pitch, yaw, roll)

    def centred_quad(cam_s, cx, cy):
        cam = Camera(np.eye(3), (1, 1), tex, r, d, cam_s, (cx, cy), pitch, yaw, roll, curl)
        pts = np.array([[0, 0], [wt, 0], [wt, ht], [0, ht]], dtype=np.float64)
        u, v = cam.project(pts[:, 0] - wt / 2, pts[:, 1] - ht / 2)
        return np.c_[u, v]

    q0 = centred_quad(1.0, 0.0, 0.0)  # unit scale, principal point at the origin
    # Sample the whole outline (not only the corners) so a curled page is framed by its true extent.
    xs, ys = q0[:, 0], q0[:, 1]
    bw, bh = xs.max() - xs.min(), ys.max() - ys.min()
    cw, ch = _pick_canvas(bw / bh, max_edge, rng)
    margin = 0.04
    fit_s = min(cw * (1 - 2 * margin) / bw, ch * (1 - 2 * margin) / bh)
    mid = np.array([(xs.max() + xs.min()) / 2, (ys.max() + ys.min()) / 2])
    for attempt in range(12):
        if partial:
            s = fit_s * float(rng.uniform(1.12, 1.45))
            # Push the page off one or two sides of the frame.
            shift = rng.uniform(-1, 1, size=2) * np.array([cw, ch]) * 0.28
            if rng.random() < 0.5:
                shift[int(rng.integers(2))] *= 0.25
        else:
            s = fit_s * float(rng.uniform(0.62, 0.97))
            slack = np.array([cw - s * bw, ch - s * bh]) / 2
            shift = rng.uniform(-1, 1, size=2) * np.maximum(slack - margin * np.array([cw, ch]), 0)
        cx, cy = cw / 2 + shift[0] - s * mid[0], ch / 2 + shift[1] - s * mid[1]
        q = centred_quad(s, cx, cy)
        vis = visible_fraction(q, (cw, ch))
        inside = bool(np.all((q[:, 0] > 0.01 * cw) & (q[:, 0] < 0.99 * cw) & (q[:, 1] > 0.01 * ch) & (q[:, 1] < 0.99 * ch)))
        if (not partial and inside) or (partial and 0.55 <= vis <= 0.93):
            break
    else:  # fall back to something valid rather than loop for ever
        if partial:
            s, vis = fit_s * 1.2, 0.8
            cx, cy = cw / 2 - s * mid[0] + 0.2 * cw, ch / 2 - s * mid[1]
        else:
            s = fit_s * 0.8
            cx, cy = cw / 2 - s * mid[0], ch / 2 - s * mid[1]
    a = np.array([[s, 0, cx], [0, s, cy], [0, 0, 1]], dtype=np.float64)
    m3 = np.column_stack([r[:, 0], r[:, 1], [0.0, 0.0, d]])
    tc = np.array([[1, 0, -wt / 2], [0, 1, -ht / 2], [0, 0, 1]], dtype=np.float64)
    return Camera(a @ m3 @ tc, (cw, ch), tex, r, d, s, (cx, cy), pitch, yaw, roll, curl)


def _cv_matrix(h_edge: np.ndarray, scale: float) -> np.ndarray:
    """Edge-coordinate homography to OpenCV's pixel-centre convention, at ``scale`` x the canvas."""
    tp = np.array([[1, 0, 0.5], [0, 1, 0.5], [0, 0, 1]], dtype=np.float64)
    tm = np.array([[1, 0, -0.5], [0, 1, -0.5], [0, 0, 1]], dtype=np.float64)
    sc = np.diag([scale, scale, 1.0])
    return tm @ sc @ h_edge @ tp


def _local_scale(h: np.ndarray, x: float, y: float) -> float:
    """Linear scale (sqrt of the Jacobian determinant) of homography ``h`` at (x, y)."""
    v = h @ np.array([x, y, 1.0])
    w = v[2]
    j00 = (h[0, 0] * w - v[0] * h[2, 0]) / w**2
    j01 = (h[0, 1] * w - v[0] * h[2, 1]) / w**2
    j10 = (h[1, 0] * w - v[1] * h[2, 0]) / w**2
    j11 = (h[1, 1] * w - v[1] * h[2, 1]) / w**2
    return math.sqrt(abs(j00 * j11 - j01 * j10))


def _flat_inverse_grid(cam: Camera, ss: int):
    """Centred page coordinates under every ss-canvas pixel centre, by the flat homography."""
    cw, ch = cam.canvas
    gx, gy = np.meshgrid((np.arange(cw * ss) + 0.5) / ss, (np.arange(ch * ss) + 0.5) / ss)
    inv = np.linalg.inv(cam.h_tex)
    v = inv @ np.stack([gx.ravel(), gy.ravel(), np.ones(gx.size)])
    ex, ey = (v[0] / v[2]).reshape(gx.shape), (v[1] / v[2]).reshape(gx.shape)
    return gx, gy, ex, ey


def render(cam: Camera, tex_rgb: np.ndarray, ss: int = SS):
    """Warp the page texture into the frame.

    Returns ``(rgb, alpha, shade)`` at the canvas size: premultiplication is undone, so ``rgb`` is
    the page colour where ``alpha`` > 0; ``shade`` is a lighting factor from the page's slope
    (1.0 for a flat page).
    """
    wt, ht = cam.tex
    cw, ch = cam.canvas
    # Mip-style pre-shrink so a heavily minified page is not aliased.
    sc = _local_scale(cam.h_tex, wt / 2, ht / 2) * ss
    shrink = min(1.0, max(0.25, sc * 1.1))
    if shrink < 0.95:
        sw, sh = max(8, round(wt * shrink)), max(8, round(ht * shrink))
        src = cv2.resize(tex_rgb, (sw, sh), interpolation=cv2.INTER_AREA)
    else:
        sw, sh, src = wt, ht, tex_rgb
    fx, fy = sw / wt, sh / ht
    prem = src.astype(np.float32)
    ones = np.ones((sh, sw), dtype=np.float32)  # coverage: 1 everywhere inside the texture
    shade = None
    if cam.curl is None:
        h_s = cam.h_tex @ np.diag([1 / fx, 1 / fy, 1.0])
        m = _cv_matrix(h_s, ss)
        flags = cv2.INTER_LINEAR
        rgb_w = cv2.warpPerspective(prem, m, (cw * ss, ch * ss), flags=flags, borderMode=cv2.BORDER_CONSTANT, borderValue=0)
        a_w = cv2.warpPerspective(ones, m, (cw * ss, ch * ss), flags=flags, borderMode=cv2.BORDER_CONSTANT, borderValue=0)
    else:
        # Newton's method on a grid at the canvas size (the map is smooth), then upsample it.
        gx, gy, ex, ey = _flat_inverse_grid(cam, 1)
        x, y = ex - wt / 2, ey - ht / 2
        hh = 0.5
        for _ in range(5):
            u, v = cam.project(x, y)
            ux, vx = cam.project(x + hh, y)
            uy, vy = cam.project(x, y + hh)
            j00, j10 = (ux - u) / hh, (vx - v) / hh
            j01, j11 = (uy - u) / hh, (vy - v) / hh
            det = j00 * j11 - j01 * j10
            det = np.where(np.abs(det) < 1e-12, 1e-12, det)
            du, dv = gx - u, gy - v
            x = x + (j11 * du - j01 * dv) / det
            y = y + (-j10 * du + j00 * dv) / det
        # Lighting from the slope along x (the surface turns towards or away from the light).
        slope = np.clip(cam.curl.lift(x + 1.0, y, wt, ht) - cam.curl.lift(x, y, wt, ht), -0.6, 0.6)
        shade = (1.0 - 0.9 * slope).astype(np.float32)
        # Map from the page back to the (ss x) pixel grid: texture-pixel coordinates per sample.
        big = (cw * ss, ch * ss)
        xs = cv2.resize(x.astype(np.float32), big, interpolation=cv2.INTER_LINEAR)
        ys = cv2.resize(y.astype(np.float32), big, interpolation=cv2.INTER_LINEAR)
        mapx = ((xs + wt / 2) * fx - 0.5).astype(np.float32)
        mapy = ((ys + ht / 2) * fy - 0.5).astype(np.float32)
        rgb_w = cv2.remap(prem, mapx, mapy, cv2.INTER_LINEAR, borderMode=cv2.BORDER_CONSTANT, borderValue=0)
        a_w = cv2.remap(ones, mapx, mapy, cv2.INTER_LINEAR, borderMode=cv2.BORDER_CONSTANT, borderValue=0)
    # The colour was sampled with a zero border, so at the page edge `rgb_w` is already the colour
    # times the coverage (premultiplied): average both, then divide.
    alpha = cv2.resize(a_w, (cw, ch), interpolation=cv2.INTER_AREA)
    rgb_prem = cv2.resize(rgb_w, (cw, ch), interpolation=cv2.INTER_AREA)
    rgb = np.where(alpha[..., None] > 1e-4, rgb_prem / np.maximum(alpha[..., None], 1e-4), 0.0)
    return rgb.astype(np.float32), np.clip(alpha, 0, 1).astype(np.float32), shade


def inverse_warp(image: np.ndarray, quad_norm: np.ndarray, out_size: tuple[int, int]) -> np.ndarray:
    """Undo the camera with only the ground-truth quad: ``image`` -> page rectangle ``out_size``.

    Uses edge coordinates (corner = pixel boundary), like the ground truth itself.
    """
    h, w = image.shape[:2]
    ow, oh = out_size
    src = (np.asarray(quad_norm, dtype=np.float64) * np.array([w, h]) - 0.5).astype(np.float32)
    dst = np.array([[-0.5, -0.5], [ow - 0.5, -0.5], [ow - 0.5, oh - 0.5], [-0.5, oh - 0.5]], dtype=np.float32)
    m = cv2.getPerspectiveTransform(src, dst)
    return cv2.warpPerspective(image, m, (ow, oh), flags=cv2.INTER_AREA | cv2.WARP_FILL_OUTLIERS)
