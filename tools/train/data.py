# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Datasets: augmentation and target rendering for the corner network.

A sample is a 256x256 canvas (the picture letterboxed, then randomly rotated by any angle, scaled,
shifted, tilted with a small homography and optionally mirrored), photometric noise on top, and the
targets at stride ``stride``:

  heat[0]   class-agnostic corner heatmap (Gaussians, sigma adapts to the page's short side)
  heat[1]   page mask
  heat[2]   centre heatmap
  off       (8, S, S) offsets cell -> the four corners, clockwise, canonical start, /32
  off_mask  (S, S) cells where the offsets are supervised (3x3 around the centre cell)

The evaluation and decoding code sees only ``EvalSet``: plain letterbox, no augmentation.
"""

from __future__ import annotations

import math

import cv2
import numpy as np
import torch
from torch.utils.data import Dataset

from common import IN_SIZE, PAD_GREY, apply_h, canonical_quad, letterbox_matrix, poly_area, read_image_rgb, read_manifest, warp_to_canvas

OFFSET_UNIT = 32.0


def render_targets(quad: np.ndarray, size: int = IN_SIZE, stride: int = 4) -> dict:
    """Targets for one 4x2 quad in canvas pixels (clockwise, canonical start)."""
    s = size // stride
    heat = np.zeros((3, s, s), dtype=np.float32)
    yy, xx = np.mgrid[0:s, 0:s].astype(np.float32)
    area = abs(poly_area(quad))
    edges = [np.linalg.norm(quad[(i + 1) % 4] - quad[i]) for i in range(4)]
    short_cells = max(min(edges) / stride, 0.5)
    sigma = float(np.clip(0.28 * short_cells, 0.8, 2.0))
    for p in quad:
        cx, cy = p[0] / stride - 0.5, p[1] / stride - 0.5
        if not (-0.5 <= cx <= s - 0.5 and -0.5 <= cy <= s - 0.5):
            continue
        g = np.exp(-((xx - cx) ** 2 + (yy - cy) ** 2) / (2 * sigma**2))
        heat[0] = np.maximum(heat[0], g)
        ix, iy = int(round(cx)), int(round(cy))
        if 0 <= ix < s and 0 <= iy < s:
            heat[0][iy, ix] = 1.0
    # mask, rendered at 4x and area-averaged
    big = np.zeros((s * 4, s * 4), dtype=np.uint8)
    cv2.fillPoly(big, [np.round(quad / stride * 4).astype(np.int32)], 1, lineType=cv2.LINE_8)
    heat[1] = cv2.resize(big.astype(np.float32), (s, s), interpolation=cv2.INTER_AREA)
    # centre
    c = quad.mean(axis=0)
    # A page whose centre lies outside the frame (a long receipt cut by the picture) still gets a
    # centre target: on the nearest border cell, so the decoder always finds a centre peak and the
    # offsets (cell -> corners) reach the corners beyond the frame.
    ccx, ccy = float(np.clip(c[0] / stride - 0.5, 0, s - 1)), float(np.clip(c[1] / stride - 0.5, 0, s - 1))
    off = np.zeros((8, s, s), dtype=np.float32)
    off_mask = np.zeros((s, s), dtype=np.float32)
    if 0 <= round(ccx) < s and 0 <= round(ccy) < s:
        csig = float(np.clip(0.07 * math.sqrt(max(area, 1.0)) / stride, 1.0, 3.5))
        heat[2] = np.exp(-((xx - ccx) ** 2 + (yy - ccy) ** 2) / (2 * csig**2))
        ix, iy = int(round(ccx)), int(round(ccy))
        heat[2][iy, ix] = 1.0
        for dy in (-1, 0, 1):
            for dx in (-1, 0, 1):
                x, y = ix + dx, iy + dy
                if 0 <= x < s and 0 <= y < s:
                    cell = np.array([(x + 0.5) * stride, (y + 0.5) * stride])
                    off[:, y, x] = ((quad - cell) / OFFSET_UNIT).reshape(-1)
                    off_mask[y, x] = 1.0
    return {"heat": heat, "off": off, "off_mask": off_mask}


def _refit(m: np.ndarray, quad: np.ndarray, rng: np.random.Generator, size: int) -> np.ndarray:
    """Zoom so that the page's bounding box fills the canvas (0.9 to 1.04 of the long side), a few
    percent off centre: the full-frame receipt of a hand-held phone photo, with corners at or just
    past the border. Skipped when the page is degenerate."""
    q = apply_h(m, quad)
    lo, hi = q.min(axis=0), q.max(axis=0)
    ext = float(max(hi[0] - lo[0], hi[1] - lo[1]))
    if not np.isfinite(ext) or ext < 4.0:
        return m
    k = float(rng.uniform(0.9, 1.04)) * size / ext
    mid = (lo + hi) / 2
    jit = rng.uniform(-0.03, 0.03, size=2) * size
    t0 = np.array([[1, 0, -mid[0]], [0, 1, -mid[1]], [0, 0, 1]], dtype=np.float64)
    sc = np.diag([k, k, 1.0])
    t1 = np.array([[1, 0, size / 2 + jit[0]], [0, 1, size / 2 + jit[1]], [0, 0, 1]], dtype=np.float64)
    return t1 @ sc @ t0 @ m


def _random_occluder(img: np.ndarray, rng: np.random.Generator) -> np.ndarray:
    """A skin-toned or plain blob (finger-like capsule or ellipse) laid over the picture at a random
    place, with soft edges: robustness to fingers and objects hiding an edge or a corner. The label
    is NOT changed (the page's true corners), like the generator's hands."""
    h, w = img.shape[:2]
    m = np.zeros((h, w), dtype=np.float32)
    p0 = rng.uniform(0, 1, 2) * [w, h]
    ang = rng.uniform(0, 2 * math.pi)
    ln = rng.uniform(0.08, 0.3) * max(h, w)
    p1 = p0 + ln * np.array([math.cos(ang), math.sin(ang)])
    wid = int(rng.uniform(0.04, 0.10) * max(h, w))
    cv2.line(m, tuple(int(v) for v in p0), tuple(int(v) for v in p1), 1.0, max(wid, 2), cv2.LINE_AA)
    cv2.circle(m, tuple(int(v) for v in p1), max(wid // 2, 1), 1.0, -1, cv2.LINE_AA)
    m = cv2.GaussianBlur(m, (0, 0), 0.8)
    if rng.random() < 0.7:  # skin tones
        base = np.array([rng.uniform(150, 250), rng.uniform(100, 205), rng.uniform(70, 175)], dtype=np.float32)
    else:
        base = rng.uniform(20, 240, size=3).astype(np.float32)
    shade = 0.85 + 0.25 * cv2.GaussianBlur(m, (0, 0), max(2.0, wid / 3))
    out = img.astype(np.float32) * (1 - m[..., None]) + np.clip(base * shade[..., None], 0, 255) * m[..., None]
    return np.clip(out, 0, 255).astype(np.uint8)


def _random_matrix(rng: np.random.Generator, w: int, h: int, size: int) -> tuple[np.ndarray, bool]:
    """Source pixels -> canvas: scale, rotation, shift, small tilt, mirror."""
    m0 = letterbox_matrix(w, h, size)
    s = float(np.exp(rng.uniform(math.log(0.55), math.log(1.35))))
    u = rng.random()
    if u < 0.55:
        th = rng.uniform(0, 2 * math.pi)
    elif u < 0.8:
        th = rng.integers(0, 4) * math.pi / 2 + rng.normal(0, 0.05)
    else:
        th = rng.normal(0, 0.12)
    mirror = rng.random() < 0.5
    c = size / 2
    t = np.array([[1, 0, c + rng.uniform(-0.28, 0.28) * size], [0, 1, c + rng.uniform(-0.28, 0.28) * size], [0, 0, 1]])
    r = np.array([[math.cos(th), -math.sin(th), 0], [math.sin(th), math.cos(th), 0], [0, 0, 1]])
    sc = np.diag([s * (-1 if mirror else 1), s, 1.0])
    persp = np.eye(3)
    if rng.random() < 0.5:
        persp[2, 0] = rng.uniform(-0.0012, 0.0012)
        persp[2, 1] = rng.uniform(-0.0012, 0.0012)
    un = np.array([[1, 0, -c], [0, 1, -c], [0, 0, 1]])
    return t @ r @ sc @ persp @ un @ m0, mirror


def _photometric(img: np.ndarray, rng: np.random.Generator) -> np.ndarray:
    x = img.astype(np.float32)
    if rng.random() < 0.9:  # brightness, contrast, gamma
        x = (x - 127.5) * rng.uniform(0.6, 1.4) + 127.5 + rng.uniform(-35, 35)
        x = 255.0 * np.clip(x / 255.0, 0, 1) ** rng.uniform(0.7, 1.4)
    if rng.random() < 0.5:  # colour cast
        x = x * rng.uniform(0.85, 1.15, size=3).astype(np.float32)
    if rng.random() < 0.3:  # saturation
        g = x.mean(axis=2, keepdims=True)
        x = g + (x - g) * rng.uniform(0.0, 1.5)
    if rng.random() < 0.4:  # light gradient or shadow band
        ang = rng.uniform(0, 2 * math.pi)
        yy, xx = np.mgrid[0 : x.shape[0], 0 : x.shape[1]].astype(np.float32)
        t = ((xx - x.shape[1] / 2) * math.cos(ang) + (yy - x.shape[0] / 2) * math.sin(ang)) / x.shape[0]
        x = x * (1.0 + rng.uniform(-0.45, 0.45) * np.tanh(t * rng.uniform(1.0, 6.0)))[..., None]
    x = np.clip(x, 0, 255)
    if rng.random() < 0.35:  # blur
        k = rng.uniform(0.4, 1.8)
        x = cv2.GaussianBlur(x, (0, 0), k)
    if rng.random() < 0.4:  # sensor noise
        x = x + rng.normal(0, rng.uniform(1.0, 9.0), size=x.shape).astype(np.float32)
    x = np.clip(x, 0, 255).astype(np.uint8)
    if rng.random() < 0.5:  # JPEG
        ok, enc = cv2.imencode(".jpg", x[..., ::-1], [cv2.IMWRITE_JPEG_QUALITY, int(rng.integers(30, 96))])
        if ok:
            x = cv2.imdecode(enc, cv2.IMREAD_COLOR)[..., ::-1]
    if rng.random() < 0.06:
        g = cv2.cvtColor(x, cv2.COLOR_RGB2GRAY)
        x = np.stack([g, g, g], axis=2)
    return x


def to_tensor(img: np.ndarray) -> torch.Tensor:
    """uint8 (3, H, W); the network input is ``x / 127.5 - 1``, applied on the device (``prep``)."""
    return torch.from_numpy(np.ascontiguousarray(img.transpose(2, 0, 1)))


def prep(x: torch.Tensor) -> torch.Tensor:
    return x.float().div_(127.5).sub_(1.0)


class TrainSet(Dataset):
    def __init__(self, manifests: list[str], stride: int = 4, size: int = IN_SIZE, seed: int = 0, limit: int | None = None, augment: bool = True, refit_p: float = 0.0, occ_p: float = 0.0):
        self.rows = []
        for m in manifests:
            self.rows += read_manifest(m)
        if limit:
            self.rows = self.rows[:limit]
        self.stride, self.size, self.seed, self.augment = stride, size, seed, augment
        self.refit_p, self.occ_p = refit_p, occ_p
        self.epoch = 0

    def __len__(self):
        return len(self.rows)

    def set_epoch(self, e: int):
        self.epoch = e

    def __getitem__(self, i: int):
        r = self.rows[i]
        rng = np.random.default_rng([self.seed, self.epoch, i])
        img = read_image_rgb(r["_path"])
        h, w = img.shape[:2]
        quad = np.asarray(r["quad"], dtype=np.float64) * [w, h]
        if self.augment:
            m, mirror = _random_matrix(rng, w, h, self.size)
            if self.refit_p > 0 and rng.random() < self.refit_p:
                m = _refit(m, quad, rng, self.size)
        else:
            m, mirror = letterbox_matrix(w, h, self.size), False
        canvas = warp_to_canvas(img, m, self.size)
        q = apply_h(m, quad)
        if mirror:
            q = q[::-1]  # keep clockwise
        q = canonical_quad(q)
        if self.augment:
            if self.occ_p > 0 and rng.random() < self.occ_p:
                canvas = _random_occluder(canvas, rng)
            canvas = _photometric(canvas, rng)
        t = render_targets(q, self.size, self.stride)
        return to_tensor(canvas), torch.from_numpy(t["heat"]), torch.from_numpy(t["off"]), torch.from_numpy(t["off_mask"])


class EvalSet(Dataset):
    """Plain letterbox of every manifest row; returns the tensor and the row index."""

    def __init__(self, manifest: str, size: int = IN_SIZE, limit: int | None = None):
        self.rows = read_manifest(manifest)
        if limit:
            self.rows = self.rows[:limit]
        self.size = size

    def __len__(self):
        return len(self.rows)

    def __getitem__(self, i: int):
        r = self.rows[i]
        img = read_image_rgb(r["_path"])
        h, w = img.shape[:2]
        m = letterbox_matrix(w, h, self.size)
        return to_tensor(warp_to_canvas(img, m, self.size)), i
