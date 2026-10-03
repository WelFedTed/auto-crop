# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Multi-item scenes: several photos, receipts and cards on one scan or table (ROADMAP M10.51).

A scene is 2 to 8 items (photographic prints with white borders, receipts with known text, ID-1
cards) lying flat on a flatbed-like bed (white, grey or black lid, with a platen frame shadow, edge
lines, dust and soft item shadows) or on a desk (wood, stone, fabric, dark mat) photographed from a
little off-axis. Every item has an **analytic ground-truth quad** (its four corners through the
scene's plane and, for desks, a perspective transform), so the manifest can say exactly how many
items there are and where.

Gaps between items are controlled and tagged: `separated` (the closest pair at least 3.5% of the
shorter side apart), `close` (1.5% to 3.2%), `touching` (0 to 0.8%) and `overlap` (one item over
another; each item's quad is its full rectangle, including the hidden part). Items may also be
clipped by the frame (`clip = partial`). The generator draws every random choice from a named
stream (`rng.stream`), so a scene depends on the suite seed and its own index only.

The manifest row is the v1 row of `suite.py` plus `items` (the list of item quads, in drawing
order) and `item_count`; `quad` is the first item's quad so a single-item reader still gets a valid
quad. Single-item manifests are unchanged.
"""

from __future__ import annotations

import hashlib
import json
import math
import multiprocessing as mp
import os
import shutil
import sys
import time
from concurrent.futures import ProcessPoolExecutor, as_completed
from concurrent.futures.process import BrokenProcessPool
from dataclasses import dataclass
from pathlib import Path

import cv2
import numpy as np

from . import GENERATOR, MANIFEST_VERSION, backgrounds, degrade, encode, plan
from . import page as pagemod
from . import rng as R

SOURCE = "tools/synth multi_item"
LICENCE = "MIT OR Apache-2.0"

# ---------------------------------------------------------------------------------------------
# Plan: which scene gets which tags
# ---------------------------------------------------------------------------------------------

COUNT_VALUES = {"2": 2, "3": 3, "4": 4, "5-6": 5, "7-8": 7}
AXES: dict[str, dict[str, float]] = {
    "count": {"2": 0.20, "3": 0.24, "4": 0.22, "5-6": 0.20, "7-8": 0.14},
    "separation": {"separated": 0.42, "close": 0.20, "touching": 0.20, "overlap": 0.18},
    "bed": {
        "flatbed-white": 0.20,
        "flatbed-grey": 0.12,
        "flatbed-black": 0.14,
        "wood": 0.16,
        "stone": 0.12,
        "fabric": 0.12,
        "dark-mat": 0.14,
    },
    "kind": {"photos": 0.42, "receipts": 0.24, "cards": 0.08, "mixed": 0.26},
    "clip": {"none": 0.90, "partial": 0.10},
    "rotation": {"aligned": 0.36, "tilted": 0.34, "any": 0.30},
}
FLATBED = ("flatbed-white", "flatbed-grey", "flatbed-black")

SUITES = {
    "multi-smoke": {"count": 160, "seed": 0x6D17_0001, "max_edge": 640, "jpeg": (62, 82), "png_share": 0.0},
    "multi-full": {"count": 1200, "seed": 0x6D17_0002, "max_edge": 800, "jpeg": (60, 92), "png_share": 0.10},
}

# Gap bands as a share of the shorter canvas side; a scene whose closest pair lands between the
# bands is redrawn so that every tag is unambiguous.
SEPARATED_MIN = 0.035
CLOSE_RANGE = (0.015, 0.032)
TOUCH_MAX = 0.008


@dataclass(frozen=True)
class MultiSceneSpec:
    index: int
    scene_id: str
    split: str
    seed: int
    count_tag: str
    n_items: int
    separation: str
    bed: str
    kind: str
    clip: str
    rotation: str
    fmt: str
    jpeg_quality: int


def build_plan(name: str, seed: int, count: int, jpeg=(62, 82), png_share: float = 0.0, pins=None):
    """One scene per image. Tag values are exact quotas (balanced shuffles, as in `plan`)."""
    pins = pins or {}
    cols = {a: plan.balanced(R.stream(seed, "multi-axis", a), count, pins.get(a, w)) for a, w in AXES.items()}
    scenes = []
    for i in range(count):
        r = R.stream(seed, "multi-misc", i)
        ct = cols["count"][i]
        n = COUNT_VALUES[ct]
        if ct in ("5-6", "7-8"):
            n += int(r.integers(0, 2 if ct == "5-6" else 2))  # 5 or 6; 7 or 8
        sep = cols["separation"][i]
        scenes.append(
            MultiSceneSpec(
                index=i,
                scene_id=f"{name}-s{i:05d}",
                split=plan.split_of(seed, i),
                seed=R.int_seed(seed, "multi-scene", i),
                count_tag=ct,
                n_items=n,
                separation=sep,
                bed=cols["bed"][i],
                kind=cols["kind"][i],
                clip=cols["clip"][i],
                rotation=cols["rotation"][i],
                fmt="png" if r.random() < png_share else "jpeg",
                jpeg_quality=int(r.integers(jpeg[0], jpeg[1] + 1)),
            )
        )
    return scenes


# ---------------------------------------------------------------------------------------------
# Geometry (no shapely: convex quads only)
# ---------------------------------------------------------------------------------------------


def rect_poly(cx: float, cy: float, w: float, h: float, ang_deg: float) -> np.ndarray:
    """Corners TL, TR, BR, BL of a w x h rectangle turned clockwise by ``ang_deg`` (y down)."""
    a = math.radians(ang_deg)
    c, s = math.cos(a), math.sin(a)
    pts = [(-w / 2, -h / 2), (w / 2, -h / 2), (w / 2, h / 2), (-w / 2, h / 2)]
    return np.array([[cx + x * c - y * s, cy + x * s + y * c] for x, y in pts], dtype=np.float64)


def _min_vertex_edge(A: np.ndarray, B: np.ndarray) -> float:
    """Smallest distance from a vertex of ``A`` to an edge of ``B``."""
    a = B
    ab = np.roll(B, -1, axis=0) - B
    ap = A[:, None, :] - a[None, :, :]
    t = np.clip((ap * ab[None]).sum(-1) / np.maximum((ab * ab).sum(-1), 1e-12)[None], 0.0, 1.0)
    d = A[:, None, :] - (a[None] + t[..., None] * ab[None])
    return float(np.sqrt((d * d).sum(-1)).min())


_AXES_CACHE: dict[int, np.ndarray] = {}


def _unit_normals(P: np.ndarray) -> np.ndarray:
    e = np.roll(P, -1, axis=0) - P
    n = np.stack([-e[:, 1], e[:, 0]], axis=1)
    return n / np.maximum(np.linalg.norm(n, axis=1, keepdims=True), 1e-12)


def poly_gap(P: np.ndarray, Q: np.ndarray) -> float:
    """Distance between two convex quads; minus the penetration depth when they overlap."""
    axes = np.vstack([_unit_normals(P), _unit_normals(Q)])  # (8, 2)
    pp, qq = P @ axes.T, Q @ axes.T  # (4, 8)
    ov = np.minimum(pp.max(0), qq.max(0)) - np.maximum(pp.min(0), qq.min(0))
    if (ov <= 0).any():
        return min(_min_vertex_edge(P, Q), _min_vertex_edge(Q, P))
    return -float(ov.min())


def poly_area_inside(P: np.ndarray, W: float, H: float) -> float:
    """Area of the quad ``P`` inside the frame ``[0, W] x [0, H]`` (Sutherland-Hodgman)."""
    out = [tuple(p) for p in P]
    for axis, lim, keep_lt in ((0, 0.0, False), (0, W, True), (1, 0.0, False), (1, H, True)):
        inp, out = out, []
        if not inp:
            break
        for i in range(len(inp)):
            a, b = inp[i], inp[(i + 1) % len(inp)]
            ina = (a[axis] <= lim) if keep_lt else (a[axis] >= lim)
            inb = (b[axis] <= lim) if keep_lt else (b[axis] >= lim)
            if ina != inb:
                t = (lim - a[axis]) / (b[axis] - a[axis])
                out.append((a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])))
            if inb:
                out.append(b)
    if len(out) < 3:
        return 0.0
    q = np.array(out)
    x, y = q[:, 0], q[:, 1]
    return float(abs(np.dot(x, np.roll(y, -1)) - np.dot(y, np.roll(x, -1))) / 2)


def quad_area(P: np.ndarray) -> float:
    x, y = P[:, 0], P[:, 1]
    return float(abs(np.dot(x, np.roll(y, -1)) - np.dot(y, np.roll(x, -1))) / 2)


# ---------------------------------------------------------------------------------------------
# Item textures
# ---------------------------------------------------------------------------------------------


def _fbm(rng, h: int, w: int, octaves: int = 5, base: int = 3) -> np.ndarray:
    return backgrounds.fbm(rng, max(h, 4), max(w, 4), octaves, base)


def photo_texture(rng, w: int, h: int, border: str) -> tuple[np.ndarray, tuple[int, int, int]]:
    """A procedural photograph (sky, ground, blobs) with an optional white border.

    Returns the RGB texture and the colour of the item's outer edge (the border or the picture)."""
    w, h = max(w, 24), max(h, 24)
    if border == "none":
        bl = bt = br = bb = 0
    elif border == "polaroid":
        bl = br = bt = int(0.06 * min(w, h))
        bb = int(0.20 * h)
    else:
        bl = br = bt = bb = int(float(rng.uniform(0.025, 0.07)) * min(w, h))
    iw, ih = max(w - bl - br, 8), max(h - bt - bb, 8)
    yy = np.linspace(0, 1, ih, dtype=np.float32)[:, None]
    horizon = float(rng.uniform(0.35, 0.65))
    # A real horizon is never a straight line across the whole picture: it wanders.
    wander = (_fbm(rng, 4, iw, 4, 2).mean(axis=0) - 0.5) * 2.0
    hz = (horizon + 0.14 * wander)[None, :].astype(np.float32)
    sky = np.array(rng.uniform((90, 130, 170), (200, 215, 235)), dtype=np.float32)
    sky2 = np.array(rng.uniform((150, 160, 170), (250, 235, 220)), dtype=np.float32)
    ground = np.array(rng.uniform((40, 55, 30), (150, 130, 90)), dtype=np.float32)
    ground2 = np.array(rng.uniform((20, 20, 20), (110, 100, 80)), dtype=np.float32)
    t = np.clip(yy / np.maximum(hz, 1e-3), 0, 1)
    top = sky * (1 - t[..., None]) + sky2 * t[..., None]
    g = np.clip((yy - hz) / np.maximum(1 - hz, 1e-3), 0, 1)
    bot = ground * (1 - g[..., None]) + ground2 * g[..., None]
    img = np.where((yy < hz)[..., None], top, bot)
    tex = _fbm(rng, ih, iw, 5, int(rng.integers(3, 7)))
    img *= (0.62 + 0.76 * tex)[..., None]
    for _ in range(int(rng.integers(2, 7))):  # subjects: soft blobs and shapes
        cx, cy = float(rng.uniform(0, iw)), float(rng.uniform(0.2 * ih, ih))
        r = float(rng.uniform(0.05, 0.22)) * min(iw, ih)
        col = tuple(float(c) for c in rng.uniform(10, 245, size=3))
        cv2.ellipse(img, (int(cx), int(cy)), (max(int(r * rng.uniform(0.6, 1.4)), 2), max(int(r), 2)), float(rng.uniform(0, 180)), 0, 360, col, -1, cv2.LINE_AA)
    img = cv2.GaussianBlur(img, (0, 0), float(rng.uniform(0.6, 1.8)))
    mode = rng.random()
    if mode < 0.22:  # black and white print
        gray = img.mean(axis=2, keepdims=True)
        img = np.repeat(gray, 3, axis=2) * float(rng.uniform(0.9, 1.1))
    elif mode < 0.32:  # sepia
        gray = img.mean(axis=2, keepdims=True)
        img = gray * np.array([1.12, 0.98, 0.78], dtype=np.float32)
    img = np.clip(img, 0, 255)
    paper = np.array(rng.uniform((238, 236, 230), (254, 253, 250)), dtype=np.float32)
    out = np.empty((h, w, 3), dtype=np.float32)
    out[:] = paper
    out[bt : bt + ih, bl : bl + iw] = img
    out *= (1.0 + 0.012 * rng.normal(size=(h, w, 1))).astype(np.float32)
    edge = tuple(int(v) for v in (paper if border != "none" else img[0, 0]))
    return np.clip(out + 0.5, 0, 255).astype(np.uint8), edge  # type: ignore[return-value]


def receipt_texture(rng, w: int, h: int, long_form: bool) -> tuple[np.ndarray, tuple[int, int, int]]:
    """A receipt page (known text) scaled to ``w x h`` px, with paper grain."""
    paper = rng.choice(["white", "white", "white", "cream"])
    pg = pagemod.render(rng, "long" if long_form else "receipt", str(paper))
    c0 = None if rng.random() < 0.7 else float(rng.uniform(0.25, 0.7))
    rgb, _ = degrade.compose_page(pg, rng, c0)
    rgb = cv2.resize(rgb, (max(w, 8), max(h, 8)), interpolation=cv2.INTER_AREA).astype(np.float32)
    grain = _fbm(rng, rgb.shape[0], rgb.shape[1], 3, 12)
    rgb *= (0.97 + 0.04 * grain)[..., None]
    rgb += rng.normal(0, 1.2, size=rgb.shape[:2] + (1,)).astype(np.float32)
    return np.clip(rgb + 0.5, 0, 255).astype(np.uint8), tuple(int(v) for v in pg.paper_rgb)  # type: ignore[return-value]


def card_texture(rng, w: int, h: int) -> tuple[np.ndarray, tuple[int, int, int]]:
    """An ID-1 style card: coloured body, photo patch, text bars."""
    base = np.array(rng.uniform((60, 80, 110), (240, 240, 245)), dtype=np.float32)
    img = np.empty((max(h, 16), max(w, 16), 3), dtype=np.float32)
    img[:] = base
    img *= (0.9 + 0.2 * _fbm(rng, img.shape[0], img.shape[1], 3, 4))[..., None]
    hh, ww = img.shape[:2]
    cv2.rectangle(img, (int(0.04 * ww), int(0.28 * hh)), (int(0.30 * ww), int(0.80 * hh)), tuple(float(c) for c in rng.uniform(80, 200, size=3)), -1)
    for k in range(4):
        y = int((0.3 + 0.12 * k) * hh)
        cv2.line(img, (int(0.36 * ww), y), (int(rng.uniform(0.7, 0.95) * ww), y), (40.0, 40.0, 44.0), max(1, hh // 40), cv2.LINE_AA)
    cv2.rectangle(img, (0, 0), (ww, int(0.16 * hh)), tuple(float(c) for c in base * 0.55), -1)
    return np.clip(img, 0, 255).astype(np.uint8), tuple(int(v) for v in img[2, 2])  # type: ignore[return-value]


@dataclass
class ItemPlan:
    kind: str  # photo | receipt | card
    w: float
    h: float
    border: str = "white"
    long_form: bool = False


def draw_item_plans(rng, spec: MultiSceneSpec, shorter: float) -> list[ItemPlan]:
    """Sizes in canvas pixels. The total ink budget shrinks with the item count."""
    n = spec.n_items
    budget = {2: 0.5, 3: 0.46, 4: 0.42, 5: 0.4, 6: 0.37, 7: 0.35, 8: 0.33}[n]
    long_side = shorter * math.sqrt(budget / n) * 1.28
    kinds = []
    for _ in range(n):
        if spec.kind == "photos":
            kinds.append("photo")
        elif spec.kind == "receipts":
            kinds.append("receipt")
        elif spec.kind == "cards":
            kinds.append("card")
        else:
            kinds.append(["photo", "photo", "receipt", "card"][int(rng.integers(4))])
    if spec.kind == "mixed" and len(set(kinds)) == 1:
        kinds[0] = "receipt" if kinds[0] != "receipt" else "photo"
    out = []
    border_mode = ["white", "white", "white", "polaroid", "none"][int(rng.integers(5))]
    for k in kinds:
        f = float(rng.uniform(0.85, 1.12))
        if k == "photo":
            ar = [1.5, 1.333, 1.0, 1.25][int(rng.integers(4))]
            lng = long_side * f
            sht = lng / ar
            w, h = (lng, sht) if rng.random() < 0.6 else (sht, lng)
            b = border_mode if rng.random() < 0.8 else ["white", "none"][int(rng.integers(2))]
            if b == "polaroid" and abs(ar - 1.0) > 0.01:
                w = h = min(w, h) * 1.15  # an instant print is nearly square
                h = h * 1.08
            out.append(ItemPlan("photo", w, h, b))
        elif k == "receipt":
            lf = rng.random() < 0.25
            ar = float(rng.uniform(4.2, 6.0) if lf else rng.uniform(1.9, 3.8))
            lng = min(long_side * 1.45 * f, shorter * 0.82)
            out.append(ItemPlan("receipt", lng / ar, lng, "none", lf))
        else:
            lng = long_side * 0.8 * f
            w, h = (lng, lng / 1.586) if rng.random() < 0.7 else (lng / 1.586, lng)
            out.append(ItemPlan("card", w, h, "none"))
    return out


# ---------------------------------------------------------------------------------------------
# Placement
# ---------------------------------------------------------------------------------------------


def far_enough(P: np.ndarray, others: list[np.ndarray], thr: float, skip: int = -1) -> bool:
    """True if ``P`` is at least ``thr`` from every other quad (a bounding-circle test first)."""
    cp = P.mean(axis=0)
    rp = float(np.linalg.norm(P - cp, axis=1).max())
    for j, Q in enumerate(others):
        if j == skip:
            continue
        cq = Q.mean(axis=0)
        if float(np.linalg.norm(cp - cq)) - rp - float(np.linalg.norm(Q - cq, axis=1).max()) >= thr:
            continue
        if poly_gap(P, Q) < thr:
            return False
    return True


def _angle_for(rng, mode: str) -> float:
    if mode == "aligned":
        return float(rng.uniform(-3.0, 3.0))
    if mode == "tilted":
        return float(rng.uniform(5.0, 30.0)) * (1 if rng.random() < 0.5 else -1)
    return float(rng.uniform(0.0, 360.0))


def _inside(P: np.ndarray, W: float, H: float, margin: float) -> bool:
    return bool((P[:, 0] >= margin).all() and (P[:, 0] <= W - margin).all() and (P[:, 1] >= margin).all() and (P[:, 1] <= H - margin).all())


def _classify(gap_frac: float) -> str | None:
    if gap_frac < 0:
        return "overlap"
    if gap_frac <= TOUCH_MAX:
        return "touching"
    if CLOSE_RANGE[0] <= gap_frac <= CLOSE_RANGE[1]:
        return "close"
    if gap_frac >= SEPARATED_MIN:
        return "separated"
    return None


def _pair_target(rng, separation: str, shorter: float, dims: tuple[float, float]) -> float:
    if separation == "close":
        return float(rng.uniform(CLOSE_RANGE[0] + 0.001, CLOSE_RANGE[1] - 0.001)) * shorter
    if separation == "touching":
        return float(rng.uniform(0.0, TOUCH_MAX - 0.001)) * shorter
    # overlap: penetration of 4% to 18% of the smaller item dimension
    return -float(rng.uniform(0.04, 0.18)) * min(dims)


def _place_adjacent(rng, A: np.ndarray, plan_b: ItemPlan, ang_a: float, rot_mode: str, target: float, centre: np.ndarray):
    """Centre and angle of item B so that gap(A, B) == target, next to a side of A (mostly one that
    faces the middle of the canvas, where there is room)."""
    sides = []
    for sd in range(4):
        e = A[(sd + 1) % 4] - A[sd]
        n = np.array([e[1], -e[0]]) / max(float(np.linalg.norm(e)), 1e-9)
        mid = (A[sd] + A[(sd + 1) % 4]) / 2
        if float(np.dot(n, mid - A.mean(axis=0))) < 0:
            n = -n
        sides.append((float(np.dot(n, centre - A.mean(axis=0))), sd))
    sides.sort(reverse=True)
    side = sides[int(rng.integers(2))][1] if rng.random() < 0.75 else int(rng.integers(4))
    e = A[(side + 1) % 4] - A[side]
    length = float(np.linalg.norm(e))
    t_hat = e / length
    n_hat = np.array([t_hat[1], -t_hat[0]])
    mid = (A[side] + A[(side + 1) % 4]) / 2
    if float(np.dot(n_hat, mid - A.mean(axis=0))) < 0:
        n_hat = -n_hat
    swap = rng.random() < 0.5
    w, h = (plan_b.h, plan_b.w) if swap else (plan_b.w, plan_b.h)
    ang = ang_a + (0.0 if rng.random() < 0.6 else float(rng.uniform(-6, 6)))
    if rot_mode == "any" and rng.random() < 0.5:
        ang = ang_a + float(rng.uniform(-25, 25))
    shift = float(rng.uniform(-0.45, 0.45)) * length
    base = mid + t_hat * shift
    lo, hi = 0.0, float(np.hypot(w, h) + np.hypot(*np.ptp(A, axis=0)) + 10)
    for _ in range(26):  # gap grows with the distance along the normal
        d = (lo + hi) / 2
        g = poly_gap(A, rect_poly(*(base + n_hat * d), w, h, ang))
        if g > target:
            hi = d
        else:
            lo = d
    d = (lo + hi) / 2
    c = base + n_hat * d
    return c, w, h, ang


def place_items(rng, spec: MultiSceneSpec, plans: list[ItemPlan], W: float, H: float):
    """Polygons (TL, TR, BR, BL) and angles of every item, with the planned separation.

    Returns ``(polys, angles, min_gap_frac)``; raises ``RuntimeError`` if no layout was found."""
    shorter = min(W, H)
    margin = 0.035 * shorter
    g_sep = 0.042 * shorter
    n = len(plans)
    for attempt in range(80):
        k = 0.93 ** (attempt // 8)
        polys: list[np.ndarray] = []
        angs: list[float] = []
        dims: list[tuple[float, float]] = []
        ok = True
        n_special = 0 if spec.separation == "separated" else (1 if n < 4 or rng.random() < 0.6 else 2)
        order = list(range(n))
        for idx in order:
            p = plans[idx]
            w, h = p.w * k, p.h * k
            special = 1 <= idx <= n_special and polys
            placed = False
            for _ in range(250 if not special else 120):
                if special:
                    anchor = int(rng.integers(len(polys))) if idx > 1 else 0
                    tgt = _pair_target(rng, spec.separation, shorter, (min(w, h), min(dims[anchor])))
                    c, w2, h2, ang = _place_adjacent(rng, polys[anchor], ItemPlan(p.kind, w, h), angs[anchor], spec.rotation, tgt, np.array([W / 2, H / 2]))
                    P = rect_poly(c[0], c[1], w2, h2, ang)
                    if not _inside(P, W, H, margin):
                        continue
                    if not far_enough(P, polys, g_sep, skip=anchor):
                        continue
                    polys.append(P)
                    angs.append(ang)
                    dims.append((w2, h2))
                    placed = True
                    break
                ang = _angle_for(rng, spec.rotation)
                if spec.rotation == "aligned" and rng.random() < 0.15:
                    ang += 90.0
                lo_x, hi_x = (0.3 * W, 0.7 * W) if idx == 0 and n_special else (margin, W - margin)
                lo_y, hi_y = (0.3 * H, 0.7 * H) if idx == 0 and n_special else (margin, H - margin)
                cx = float(rng.uniform(lo_x, hi_x))
                cy = float(rng.uniform(lo_y, hi_y))
                P = rect_poly(cx, cy, w, h, ang)
                if not _inside(P, W, H, margin):
                    continue
                if not far_enough(P, polys, g_sep):
                    continue
                polys.append(P)
                angs.append(ang)
                dims.append((w, h))
                placed = True
                break
            if not placed:
                ok = False
                break
        if not ok:
            continue
        # the closest pair decides the scene's separation tag
        gmin = min((poly_gap(polys[i], polys[j]) for i in range(n) for j in range(i + 1, n)), default=math.inf)
        if _classify(gmin / shorter) != spec.separation:
            continue
        return polys, angs, gmin / shorter
    raise RuntimeError(f"no layout for {spec.scene_id}")


def clip_one(rng, polys, angs, W, H, g_min) -> int | None:
    """Pushes one item over the frame so that 6% to 22% of its area is outside; returns its index."""
    order = list(rng.permutation(len(polys)))
    for i in order:
        P = polys[i]
        for _ in range(30):
            axis = int(rng.integers(2))
            lim = (W, H)[axis]
            toward_hi = P[:, axis].mean() > lim / 2
            frac = float(rng.uniform(0.07, 0.2))
            ext = float(np.ptp(P[:, axis]))
            target_out = frac * ext
            cur_out = (P[:, axis].max() - lim) if toward_hi else (0.0 - P[:, axis].min())
            delta = (target_out - cur_out) * (1 if toward_hi else -1)
            Q = P.copy()
            Q[:, axis] += delta
            inside = poly_area_inside(Q, W, H) / quad_area(Q)
            if not 0.7 <= inside <= 0.95:
                continue
            if all(poly_gap(Q, polys[j]) >= g_min for j in range(len(polys)) if j != i):
                polys[i] = Q
                return int(i)
    return None


# ---------------------------------------------------------------------------------------------
# Rendering
# ---------------------------------------------------------------------------------------------


def _lid_colour(rng, bed: str) -> np.ndarray:
    if bed == "flatbed-white":
        v = float(rng.uniform(232, 252))
        return np.array([v - float(rng.uniform(0, 3)), v, v - float(rng.uniform(-2, 3))], dtype=np.float32)
    if bed == "flatbed-grey":
        v = float(rng.uniform(125, 205))
        return np.array([v, v + float(rng.uniform(-3, 3)), v + float(rng.uniform(-2, 5))], dtype=np.float32)
    v = float(rng.uniform(6, 38))
    return np.array([v, v, v + float(rng.uniform(0, 4))], dtype=np.float32)


def flatbed_background(rng, bed: str, H: int, W: int) -> np.ndarray:
    """Lid colour with a vignette and noise, the platen frame shadow and a few edge lines."""
    lid = _lid_colour(rng, bed)
    yy, xx = np.mgrid[0:H, 0:W].astype(np.float32)
    ang = float(rng.uniform(0, 2 * math.pi))
    grad = ((xx - W / 2) * math.cos(ang) + (yy - H / 2) * math.sin(ang)) / max(H, W)
    img = np.empty((H, W, 3), dtype=np.float32)
    img[:] = lid
    img *= (1.0 + float(rng.uniform(0.01, 0.05)) * grad)[..., None]
    img *= (1.0 + 0.012 * (_fbm(rng, H, W, 3, 3) - 0.5))[..., None]
    dark = bed == "flatbed-black"
    for side in range(4):
        if rng.random() < 0.55:
            wid = max(2, int(float(rng.uniform(0.004, 0.03)) * min(H, W)))
            ramp = np.linspace(1.0, 0.0, wid, dtype=np.float32)
            depth = float(rng.uniform(0.12, 0.4)) if not dark else float(rng.uniform(-0.6, -0.2))
            strip = 1.0 - depth * ramp
            if side == 0:
                img[:wid] *= strip[:, None, None]
            elif side == 1:
                img[:, W - wid :] *= strip[::-1][None, :, None]
            elif side == 2:
                img[H - wid :] *= strip[::-1][:, None, None]
            else:
                img[:, :wid] *= strip[None, :, None]
        if rng.random() < 0.45:  # a 1-5 px edge line a little inside the border
            off = int(float(rng.uniform(0.002, 0.03)) * min(H, W))
            thick = int(rng.integers(1, 4))
            tone = float(rng.uniform(0.55, 0.9)) if not dark else float(rng.uniform(1.8, 4.0))
            if side == 0:
                img[off : off + thick] *= tone
            elif side == 1:
                img[:, max(W - off - thick, 0) : W - off] *= tone
            elif side == 2:
                img[max(H - off - thick, 0) : H - off] *= tone
            else:
                img[:, off : off + thick] *= tone
    return np.clip(img, 0, 255)


def add_dust(rng, img: np.ndarray, level: int) -> None:
    H, W = img.shape[:2]
    dark_bed = img.mean() < 90
    for _ in range(level):
        x, y = int(rng.integers(0, W)), int(rng.integers(0, H))
        r = float(rng.uniform(0.4, 1.8))
        tone = float(rng.uniform(150, 230)) if dark_bed else float(rng.uniform(40, 130))
        cv2.circle(img, (x, y), max(int(round(r)), 1), (tone, tone, tone), -1, cv2.LINE_AA)
    for _ in range(int(rng.integers(0, 3)) if rng.random() < 0.5 else 0):  # hairs
        start = rng.uniform((0, 0), (W, H))
        step = rng.normal(0, 0.05 * max(W, H), size=(3, 2))
        p = start + np.cumsum(step, axis=0)
        pts = np.round(p).astype(np.int32)
        tone = float(rng.uniform(150, 220)) if dark_bed else float(rng.uniform(60, 120))
        cv2.polylines(img, [pts], False, (tone, tone, tone), 1, cv2.LINE_AA)


def _paste(canvas: np.ndarray, tex: np.ndarray, poly: np.ndarray, shadow, ss: int = 3) -> None:
    """Draws ``tex`` into ``canvas`` (float32) at the quad ``poly`` with a soft drop shadow.

    ``shadow`` is ``(offset_xy, blur_sigma, strength)`` in canvas pixels."""
    H, W = canvas.shape[:2]
    th, tw = tex.shape[:2]
    pad = int(shadow[1] * 3 + abs(shadow[0][0]) + abs(shadow[0][1]) + 3)
    x0, y0 = int(math.floor(poly[:, 0].min())) - pad, int(math.floor(poly[:, 1].min())) - pad
    x1, y1 = int(math.ceil(poly[:, 0].max())) + pad, int(math.ceil(poly[:, 1].max())) + pad
    x0c, y0c, x1c, y1c = max(x0, 0), max(y0, 0), min(x1, W), min(y1, H)
    if x1c <= x0c or y1c <= y0c:
        return
    rw, rh = x1c - x0c, y1c - y0c
    src = np.float32([[0, 0], [tw, 0], [tw, th], [0, th]])
    dst = ((poly - [x0c, y0c]) * ss).astype(np.float32)
    M = cv2.getPerspectiveTransform(src, dst)
    big = (rw * ss, rh * ss)
    rgb = cv2.warpPerspective(tex, M, big, flags=cv2.INTER_LINEAR, borderMode=cv2.BORDER_CONSTANT)
    mask = np.zeros((big[1], big[0]), dtype=np.float32)
    cv2.fillConvexPoly(mask, np.round(dst).astype(np.int32), 1.0, lineType=cv2.LINE_AA)
    rgb = cv2.resize(rgb, (rw, rh), interpolation=cv2.INTER_AREA).astype(np.float32)
    alpha = cv2.resize(mask, (rw, rh), interpolation=cv2.INTER_AREA)
    # Shadow: the silhouette shifted and blurred, darkening what is under the item.
    (ox, oy), sigma, strength = shadow
    if strength > 0:
        sh = cv2.warpAffine(alpha, np.float32([[1, 0, ox], [0, 1, oy]]), (rw, rh))
        sh = cv2.GaussianBlur(sh, (0, 0), max(sigma, 0.3))
        region = canvas[y0c:y1c, x0c:x1c]
        region *= (1.0 - strength * sh * (1.0 - alpha))[..., None]
    region = canvas[y0c:y1c, x0c:x1c]
    # A thin darker rim: paper has thickness and its edge catches little light.
    inner = cv2.erode(alpha, np.ones((3, 3), np.uint8))
    rgb *= (1.0 - 0.07 * np.clip(alpha - inner, 0, 1))[..., None]
    region[:] = region * (1 - alpha[..., None]) + rgb * alpha[..., None]


def _perspective(rng, W: int, H: int, margin: float, strength: float):
    """3x3 matrix taking the plane (``W x H`` plus ``margin`` on every side) to the picture, with a
    small keystone, and the plane-to-picture offset."""
    ox, oy = margin * W, margin * H
    src = np.float32([[ox, oy], [ox + W, oy], [ox + W, oy + H], [ox, oy + H]])
    j = lambda: float(rng.uniform(-strength, strength))  # noqa: E731
    src = src + np.float32([[j() * W, j() * H], [j() * W, j() * H], [j() * W, j() * H], [j() * W, j() * H]])
    dst = np.float32([[0, 0], [W, 0], [W, H], [0, H]])
    return cv2.getPerspectiveTransform(src, dst), (ox, oy)


def render_scene(spec: MultiSceneSpec, max_edge: int):
    """Returns ``(uint8 RGB picture, item quads normalised, meta)``; quads TL, TR, BR, BL of each
    upright item, clockwise, y down, in drawing order (the last one is on top)."""
    rng = R.stream(spec.seed, "multi")
    flat = spec.bed in FLATBED
    # canvas
    if flat:
        ratio = float(rng.choice([1.414, 1.414, 1.294, 1.0]))
    else:
        ratio = float(rng.choice([1.333, 1.5, 1.333]))
    landscape = rng.random() < 0.5
    if landscape:
        W, H = max_edge, max(int(round(max_edge / ratio)), 64)
    else:
        W, H = max(int(round(max_edge / ratio)), 64), max_edge
    margin = 0.0 if flat else 0.16
    PW, PH = int(round(W * (1 + 2 * margin))), int(round(H * (1 + 2 * margin)))
    ox, oy = margin * W, margin * H
    shorter = float(min(W, H))
    plans = draw_item_plans(rng, spec, shorter)
    polys, angs, gap_frac = place_items(rng, spec, plans, float(W), float(H))
    clipped_idx = None
    if spec.clip == "partial":
        clipped_idx = clip_one(rng, polys, angs, float(W), float(H), 0.05 * shorter if spec.separation == "separated" else 0.0)
    # draw order: a random permutation so overlaps have a random item on top
    order = [int(i) for i in rng.permutation(len(polys))]

    # background
    if flat:
        bg = flatbed_background(rng, spec.bed, PH, PW)
    else:
        bg = backgrounds.make(spec.bed, R.stream(spec.seed, "bg"), PH, PW)
    canvas = bg.astype(np.float32)
    edge_cols: list[np.ndarray] = []
    ss_offset = np.array([ox, oy])
    shadow_scale = max(W, H) / 800.0
    item_meta = []
    for i in order:
        p = plans[i]
        w_px = int(round(np.linalg.norm(polys[i][1] - polys[i][0])))
        h_px = int(round(np.linalg.norm(polys[i][3] - polys[i][0])))
        trng = R.stream(spec.seed, "item", i)
        if p.kind == "photo":
            tex, edge = photo_texture(trng, w_px, h_px, p.border)
        elif p.kind == "receipt":
            tex, edge = receipt_texture(trng, w_px, h_px, p.long_form)
        else:
            tex, edge = card_texture(trng, w_px, h_px)
        edge_cols.append(np.array(edge, dtype=np.float32))
        if flat:
            sh = ((float(rng.uniform(0.5, 2.5)) * shadow_scale, float(rng.uniform(0.5, 3.0)) * shadow_scale), float(rng.uniform(1.0, 5.0)) * shadow_scale, float(rng.uniform(0.12, 0.45)))
        else:
            sh = ((float(rng.uniform(-3, 4)) * shadow_scale * 1.6, float(rng.uniform(1, 6)) * shadow_scale * 1.6), float(rng.uniform(2, 7)) * shadow_scale, float(rng.uniform(0.25, 0.55)))
        _paste(canvas, tex, polys[i] + ss_offset, sh)
        item_meta.append({"kind": p.kind, "border": p.border})
    if flat:
        add_dust(rng, canvas, int(rng.integers(0, 6)) if rng.random() < 0.7 else int(rng.integers(15, 60)))

    # picture: perspective for desks, a crop for the flatbed
    if flat:
        pic = canvas
        Hm = np.eye(3)
    else:
        M, _ = _perspective(rng, W, H, margin, 0.035)
        pic = cv2.warpPerspective(canvas, M, (W, H), flags=cv2.INTER_LINEAR, borderMode=cv2.BORDER_REPLICATE)
        Hm = M.astype(np.float64)
    # light: a gentle gradient (desks: stronger, with a colour cast)
    yy, xx = np.mgrid[0:H, 0:W].astype(np.float32)
    ang = float(rng.uniform(0, 2 * math.pi))
    grad = ((xx - W / 2) * math.cos(ang) + (yy - H / 2) * math.sin(ang)) / max(H, W)
    strength = float(rng.uniform(0.02, 0.07)) if flat else float(rng.uniform(0.08, 0.22))
    r2 = ((xx - W / 2) ** 2 + (yy - H / 2) ** 2) / (0.25 * (W * W + H * H))
    light = (1.0 + strength * grad) * (1.0 - (0.0 if flat else float(rng.uniform(0.04, 0.14))) * r2)
    pic = pic * light[..., None]
    if not flat:
        gains = np.array(rng.uniform(0.92, 1.08, size=3), dtype=np.float32)
        pic = pic * (gains / gains.mean())
    out = np.clip(pic + 0.5, 0, 255).astype(np.uint8)
    scale = max(W, H) / 512.0
    out = degrade.blur(out, "sharp" if rng.random() < 0.6 else "soft", rng, scale * (0.7 if flat else 1.0))
    out = degrade.sensor_noise(out, "low" if rng.random() < 0.7 else "medium", rng, 1.0)

    # ground truth through the plane-to-picture transform
    quads = []
    for i in range(len(polys)):
        pts = (polys[i] + ss_offset).astype(np.float64)
        h3 = np.c_[pts, np.ones(4)] @ Hm.T
        q = h3[:, :2] / h3[:, 2:3]
        quads.append(q / np.array([W, H]))
    # contrast tag: how different is the bed from the items' outer edge colour
    bg_pic = cv2.resize(bg, (W, H), interpolation=cv2.INTER_AREA) if flat else bg
    lums = [float(c @ np.array([0.299, 0.587, 0.114])) for c in edge_cols]
    bed_l = float(np.mean(bg_pic @ np.array([0.299, 0.587, 0.114], dtype=np.float32)))
    low = sum(abs(v - bed_l) < 28 for v in lums) > len(lums) / 2
    meta = {
        "gap_pct": round(gap_frac * 100, 3),
        "clipped_item": clipped_idx,
        "draw_order": order,
        "items": [item_meta[order.index(i)] for i in range(len(polys))],
        "contrast": "low" if low else "normal",
        "bed_luma": round(bed_l, 1),
        "angles": [round(a % 360, 2) for a in angs],
        "size": [W, H],
    }
    return out, quads, meta


# ---------------------------------------------------------------------------------------------
# Suite
# ---------------------------------------------------------------------------------------------


def _r9(a) -> list:
    return [[round(float(x), 9) for x in p] for p in a]


def _reading_key(q: np.ndarray):
    c = q.mean(axis=0)
    return (round(float(c[1]) * 4), float(c[0]))


def _render_task(task):
    spec, out, max_edge, suite = task
    out = Path(out)
    img, quads, meta = render_scene(spec, max_edge)
    data = encode.encode(img, spec.fmt, spec.jpeg_quality, 1, "srgb")
    rel = f"images/{spec.scene_id}.{encode.EXT[spec.fmt]}"
    (out / rel).write_bytes(data)
    h, w = img.shape[:2]
    # A single-item reader sees `quad` = the item that is first in reading order.
    items = sorted(range(len(quads)), key=lambda i: _reading_key(quads[i]))
    ordered = [quads[i] for i in items]
    inside = [round(poly_area_inside(q * [w, h], w, h) / quad_area(q * [w, h]), 4) for q in ordered]
    tags = {
        "count": spec.count_tag,
        "separation": spec.separation,
        "bed": spec.bed,
        "kind": spec.kind,
        "clip": spec.clip,
        "contrast": meta["contrast"],
        "rotation": spec.rotation,
        "format": spec.fmt,
    }
    return {
        "v": MANIFEST_VERSION,
        "id": spec.scene_id,
        "image": rel,
        "scene_id": spec.scene_id,
        "split": spec.split,
        "width": int(w),
        "height": int(h),
        "quad": _r9(ordered[0]),
        "items": [_r9(q) for q in ordered],
        "tags": tags,
        "generator": GENERATOR,
        "suite": suite,
        "scene_seed": spec.seed,
        "item_count": len(ordered),
        "visible_fractions": inside,
        "min_gap_pct": meta["gap_pct"],
        "item_kinds": [meta["items"][i]["kind"] for i in items],
        "angles_deg": [meta["angles"][i] for i in items],
        "bed_luma": meta["bed_luma"],
        "jpeg_quality": spec.jpeg_quality if spec.fmt == "jpeg" else None,
        "licence": LICENCE,
        "source": SOURCE,
    }


def generate_multi(out: Path, name: str, seed: int, count: int, max_edge: int = 640, jobs: int | None = None, jpeg=(62, 82), png_share: float = 0.0, pins=None, quiet: bool = False) -> dict:
    """Writes a multi-item suite under ``out`` (images, manifest.jsonl, suite.json)."""
    out = Path(out)
    shutil.rmtree(out / "images", ignore_errors=True)
    (out / "images").mkdir(parents=True, exist_ok=True)
    scenes = build_plan(name, seed, count, jpeg, png_share, pins)
    tasks = [(s, str(out), max_edge, name) for s in scenes]
    jobs = jobs or min(os.cpu_count() or 1, 8)
    started = time.time()
    rows: dict[int, dict] = {}
    if jobs <= 1:
        for i, t in enumerate(tasks):
            rows[i] = _render_task(t)
    else:
        for attempt in range(3):
            pending = [i for i in range(len(tasks)) if i not in rows]
            if not pending:
                break
            try:
                with ProcessPoolExecutor(max_workers=jobs, mp_context=mp.get_context("spawn")) as pool:
                    futs = {pool.submit(_render_task, tasks[i]): i for i in pending}
                    for k, f in enumerate(as_completed(futs)):
                        rows[futs[f]] = f.result()
                        if not quiet and (k + 1) % max(1, count // 10) == 0:
                            print(f"  {len(rows)}/{count} scenes, {time.time() - started:.0f}s", file=sys.stderr, flush=True)
            except BrokenProcessPool:
                print("  a worker process died; retrying unfinished scenes", file=sys.stderr, flush=True)
        if len(rows) != len(tasks):
            raise RuntimeError("scenes could not be rendered: workers keep dying")
    lines = [rows[i] for i in sorted(rows)]
    lines.sort(key=lambda r: r["id"])
    text = "".join(json.dumps(r, separators=(",", ":")) + "\n" for r in lines)
    (out / "manifest.jsonl").write_text(text, encoding="utf8", newline="\n")
    summary = {
        "generator": GENERATOR,
        "name": name,
        "seed": seed,
        "count": count,
        "scenes": count,
        "max_edge": max_edge,
        "degrade_backend": "builtin",
        "manifest_sha256": hashlib.sha256(text.encode("utf8")).hexdigest(),
        "image_bytes": sum((out / r["image"]).stat().st_size for r in lines),
        "seconds": round(time.time() - started, 1),
        "tag_histogram": plan.histogram([r["tags"] for r in lines]),
    }
    (out / "suite.json").write_text(json.dumps(summary, indent=1, sort_keys=True) + "\n", encoding="utf8")
    return summary
