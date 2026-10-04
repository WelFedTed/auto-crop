# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Hands and fingers holding the page (TRAINING MODE only, see ``trainset``).

The evaluation suites (smoke, full, multi) have no hands: the classical detector fails on
receipts held between fingers, and a learned detector can only learn that from data that has it.
This module draws simple procedural hands over a finished desk-and-page composite: a thumb
pressing the page edge or a corner from in front, up to three fingers and a palm emerging from
behind the page on the same edge, or two thumbs holding opposite edges. The ground-truth quad is
NOT changed by an occluder: it stays the page's true corners, as a human labeller would draw them.

Everything is procedural (capsules, an ellipse, shading and a nail), seeded by the caller's stream,
and licence-free. It is a stand-in for skin, not a photographic hand: the point is the occlusion
topology (a skin-coloured blob that touches the page edge and hides a stretch of it or a corner),
not realism. Nothing here is used by the default suites.
"""

from __future__ import annotations

import math

import cv2
import numpy as np

SKIN_TONES = (
    (255, 224, 189), (246, 205, 170), (241, 194, 125), (224, 172, 105),
    (198, 134, 66), (166, 106, 62), (141, 85, 36), (110, 65, 40),
    (232, 190, 172), (250, 214, 190), (214, 160, 130),
)
FINGER_MM = 17.0  # fingertip width of an adult hand, about
THUMB_MM = 21.0


def _rot(v: np.ndarray, deg: float) -> np.ndarray:
    a = math.radians(deg)
    c, s = math.cos(a), math.sin(a)
    return np.array([v[0] * c - v[1] * s, v[0] * s + v[1] * c])


def _capsule_mask(shape, p0, p1, width: float) -> np.ndarray:
    m = np.zeros(shape, dtype=np.float32)
    r = max(1.0, width / 2.0)
    cv2.line(m, (int(round(p0[0])), int(round(p0[1]))), (int(round(p1[0])), int(round(p1[1]))), 1.0, int(round(2 * r)), cv2.LINE_AA)
    cv2.circle(m, (int(round(p0[0])), int(round(p0[1]))), int(round(r)), 1.0, -1, cv2.LINE_AA)
    cv2.circle(m, (int(round(p1[0])), int(round(p1[1]))), int(round(r)), 1.0, -1, cv2.LINE_AA)
    return m


def _paint(img: np.ndarray, mask: np.ndarray, base: np.ndarray, rng, scale: float, tip=None, tip_r: float = 0.0, axis=None) -> None:
    """Composite a shaded skin patch described by ``mask`` onto ``img`` (float32 RGB, in place)."""
    h, w = mask.shape
    mask = cv2.GaussianBlur(mask, (0, 0), 0.6)
    if mask.max() <= 0:
        return
    # Rounded look: brighter where deep inside the shape, darker towards its rim.
    inner = cv2.GaussianBlur(mask, (0, 0), max(1.5, 3.0 * scale))
    shade = 0.80 + 0.30 * np.clip(inner / max(inner.max(), 1e-6), 0, 1)
    tex = 1.0 + 0.035 * (cv2.GaussianBlur(rng.standard_normal((h, w)).astype(np.float32), (0, 0), 1.2) * 4.0)
    skin = np.clip(base[None, None, :] * (shade * tex)[..., None], 0, 255)
    rim = np.clip(mask - cv2.erode(mask, np.ones((3, 3), np.uint8)), 0, 1)
    skin = skin * (1.0 - 0.22 * rim[..., None])
    if tip is not None and tip_r >= 2.5:
        # A nail at the tip: a pale, slightly pink oval with a darker outline.
        nail = np.zeros((h, w), dtype=np.float32)
        ang = 0.0 if axis is None else math.degrees(math.atan2(axis[1], axis[0]))
        cv2.ellipse(nail, (int(round(tip[0])), int(round(tip[1]))), (max(1, int(tip_r * 0.62)), max(1, int(tip_r * 0.45))), ang, 0, 360, 1.0, -1, cv2.LINE_AA)
        nail = nail * mask
        pale = np.clip(base * np.array([1.04, 0.96, 0.95]) + 14, 0, 255)
        skin = skin * (1 - 0.7 * nail[..., None]) + pale[None, None, :] * 0.7 * nail[..., None]
    a = mask[..., None]
    img[:] = img * (1 - a) + skin * a


def plan_hand(rng, quad_px: np.ndarray, frame_wh: tuple[int, int], page_mm: tuple[float, float]) -> dict:
    """Decide a hand: a list of parts (``front`` over the page, ``behind`` under it) with geometry.

    ``quad_px`` are the page corners TL, TR, BR, BL in frame pixels; ``page_mm`` the page's width
    and height in millimetres, which sets the finger width relative to the page.
    """
    q = np.asarray(quad_px, dtype=np.float64)
    centre = q.mean(axis=0)
    short_px = min(np.linalg.norm(q[1] - q[0]), np.linalg.norm(q[3] - q[0]), np.linalg.norm(q[2] - q[1]), np.linalg.norm(q[3] - q[2]))
    short_mm = float(min(page_mm))
    ppm = max(short_px, 6.0) / max(short_mm, 20.0)  # pixels per millimetre at the page
    ppm = max(ppm, 0.12)
    fw = FINGER_MM * ppm * float(rng.uniform(0.85, 1.25))
    fw = max(fw, 3.5)
    kind = ["thumb", "grip", "two"][int(rng.choice(3, p=[0.35, 0.45, 0.20]))]
    parts: list[dict] = []
    skin = np.array(SKIN_TONES[int(rng.integers(len(SKIN_TONES)))], dtype=np.float32) * float(rng.uniform(0.85, 1.05))

    def edge_point(i: int, t: float):
        a, b = q[i], q[(i + 1) % 4]
        p = a + (b - a) * t
        d = (b - a) / max(np.linalg.norm(b - a), 1e-6)
        n = np.array([d[1], -d[0]])
        if np.dot(n, p - centre) < 0:
            n = -n
        return p, d, n

    def thumb(i: int, t: float):
        p, d, n = edge_point(i, t)
        depth = float(rng.uniform(0.1, 1.2)) * fw  # how far the tip reaches over the edge
        tip = p - n * depth + d * float(rng.uniform(-0.3, 0.3)) * fw
        ang = float(rng.uniform(-38, 38))
        direction = _rot(n, ang)
        length = float(rng.uniform(55, 95)) * ppm
        base = tip + direction * length
        parts.append({"layer": "front", "p0": base, "p1": tip, "w": fw * float(rng.uniform(1.0, 1.25)), "tip": True})

    def behind(i: int, t: float, n_fingers: int):
        p, d, n = edge_point(i, t)
        side = 1.0 if rng.random() < 0.5 else -1.0
        for k in range(n_fingers):
            off = side * (k + 0.5) * fw * float(rng.uniform(1.05, 1.25))
            pk = p + d * off
            ang = float(rng.uniform(-14, 14))
            direction = _rot(n, ang)
            tip = pk - n * float(rng.uniform(0.3, 1.5)) * fw  # tucked under the page
            base = tip + direction * float(rng.uniform(70, 110)) * ppm
            parts.append({"layer": "behind", "p0": base, "p1": tip, "w": fw * float(rng.uniform(0.95, 1.1)), "tip": False})
        pc = p + d * side * (n_fingers * 0.5) * fw + n * float(rng.uniform(75, 110)) * ppm
        parts.append({"layer": "behind", "palm": pc, "axes": (float(rng.uniform(38, 48)) * ppm, float(rng.uniform(48, 62)) * ppm), "angle": math.degrees(math.atan2(n[1], n[0])) + 90.0})

    corner = rng.random() < 0.3
    i = int(rng.integers(4))
    t = float(rng.choice([rng.uniform(0.0, 0.12), rng.uniform(0.88, 1.0)])) if corner else float(rng.uniform(0.15, 0.85))
    if kind == "thumb":
        thumb(i, t)
    elif kind == "grip":
        thumb(i, t)
        behind((i + 2) % 4 if rng.random() < 0.4 else i, float(rng.uniform(0.2, 0.8)), int(rng.integers(1, 4)))
    else:
        thumb(i, t)
        thumb((i + 2) % 4, float(rng.uniform(0.15, 0.85)))
        if rng.random() < 0.5:
            behind((i + 1) % 4, float(rng.uniform(0.25, 0.75)), int(rng.integers(1, 3)))
    return {"kind": kind, "skin": skin, "parts": parts, "ppm": ppm, "finger_px": fw}


def draw_hand(img: np.ndarray, hand: dict, layer: str, rng) -> None:
    """Draw the parts of ``layer`` (``behind`` or ``front``) onto ``img`` (float32 RGB), in place."""
    h, w = img.shape[:2]
    scale = max(h, w) / 512.0
    for part in hand["parts"]:
        if part["layer"] != layer:
            continue
        if "palm" in part:
            m = np.zeros((h, w), dtype=np.float32)
            pc = part["palm"]
            cv2.ellipse(m, (int(round(pc[0])), int(round(pc[1]))), (max(2, int(part["axes"][0])), max(2, int(part["axes"][1]))), part["angle"], 0, 360, 1.0, -1, cv2.LINE_AA)
            _paint(img, m, hand["skin"] * 0.97, rng, scale)
            continue
        m = _capsule_mask((h, w), part["p0"], part["p1"], part["w"])
        axis = part["p1"] - part["p0"]
        _paint(img, m, hand["skin"], rng, scale, tip=part["p1"] if part["tip"] else None, tip_r=part["w"] / 2.0, axis=axis)
